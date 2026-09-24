package ing.zork.android

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.transformWhile
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

/** Mirrors core's per-file limit so a picked file stops copying early. */
private const val ATTACHMENT_LIMIT = 300L * 1024 * 1024

internal interface NativeObserver {
    fun onReady(handle: Long, generation: Long, urgentHint: Boolean, sourceClosed: Boolean)
}

internal object NativeBridge {
    init { System.loadLibrary("zork_android") }
    external fun newId(): String
    external fun textAttachmentLimit(): Int
    external fun validateModel(input: String, models: String): String
    external fun previewModels(input: String, models: String): String
    external fun isLocal(request: String): Boolean
    external fun agentChoices(profiles: String, profile: String, model: String, thinking: String): String
    external fun modelForm(query: String): String
    external fun copyableModel(model: String): Boolean
    external fun connectionChoices(catalog: String, subscription: Boolean, provider: String, billing: String): String
    external fun composerState(input: String): String
    external fun initialize(context: Context)
    external fun clearData(root: String, confirmed: Boolean, context: Context): String
    external fun call(root: String, request: String): String
    external fun observe(root: String, request: String): String
    external fun chatFileBytes(root: String, key: String): ByteArray
    external fun fileThumbnail(root: String, peer: String, session: String, message: String, file: String): ByteArray
    external fun saveChatFile(root: String, ticket: String, descriptor: Int): String
    external fun watch(root: String, handle: Long, generation: Long, observer: NativeObserver): String
}

internal class ClientRepository(context: Context, dataDirectory: File = context.noBackupFilesDir.resolve("client")) {
    private val applicationContext = context.applicationContext
    private val resolver = context.applicationContext.contentResolver
    private val root = dataDirectory.absolutePath
    private val legacyPeer = context.getSharedPreferences("navigation", 0).getString("peer", null)
    private val initialization = Mutex()
    private var initialized = false
    private val multicast = (context.applicationContext.getSystemService(Context.WIFI_SERVICE) as? android.net.wifi.WifiManager)
        ?.createMulticastLock("zork-lan-discovery")?.apply { setReferenceCounted(false) }
    private val networkGate = Mutex()
    private val localGate = Mutex()
    private val observations = Observations(root)

    init { NativeBridge.initialize(context.applicationContext) }

    // Legacy text attachments in old messages can still be exported; new files go through attachFile.
    suspend fun exportText(uri: android.net.Uri, content: String) = withContext(Dispatchers.IO) {
        requireNotNull(resolver.openOutputStream(uri)).use { it.write(content.toByteArray(Charsets.UTF_8)) }
    }

    // Script core validates the selected URI and supplies the byte bound.
    suspend fun localDocumentBytes(uri: android.net.Uri, max: Int): ByteArray = withContext(Dispatchers.IO) {
        val buffer = ByteArray(max + 1); var used = 0
        requireNotNull(resolver.openInputStream(uri)).use { stream ->
            while (used < buffer.size) { val count = stream.read(buffer, used, buffer.size - used); if (count < 0) break; if (count > 0) used += count }
        }
        check(used <= max) { "Document too large" }
        buffer.copyOf(used)
    }
    suspend fun writeLocalDocument(uri: android.net.Uri, bytes: ByteArray): Int = withContext(Dispatchers.IO) {
        requireNotNull(resolver.openOutputStream(uri, "wt")).use { it.write(bytes) }
        bytes.size
    }

    fun events(peer: String, session: String?) = observations.conversation(peer, session)
    fun settingsEvents(peer: String) = observations.settings(peer)
    fun newChatEvents(peer: String) = observations.newChat(peer)
    fun notificationEvents() = observations.notifications()
    fun navigationEvents() = observations.navigation()
    fun adbEvents() = observations.adb()
    fun accountEvents() = observations.account()
    fun dataResetEvents() = observations.dataReset()
    suspend fun clearData() = withContext(Dispatchers.IO) {
        val response = JSONObject(NativeBridge.clearData(root, true, applicationContext))
        check(response.optBoolean("ok")) { response.optString("error", "无法清空数据") }
    }
    fun localScriptEvents() = observations.localScripts()
    fun resourceEvents(selection: ResourceSelection) = observations.resources(selection)
    fun chatFileEvents() = observations.chatFiles()
    suspend fun chatPreviewBytes(key: String): ByteArray = withContext(Dispatchers.IO) { NativeBridge.chatFileBytes(root, key) }
    /** Verified image bytes for a thumbnail; a null [message] selects a draft file. */
    suspend fun fileThumbnail(peer: String, session: String, message: String?, file: String): ByteArray =
        withContext(Dispatchers.IO) { NativeBridge.fileThumbnail(root, peer, session, message.orEmpty(), file) }

    /**
     * Copies a picked or shared URI into a private file, then hands it to core,
     * which snapshots the bytes into the draft; the copy is removed afterwards.
     */
    suspend fun attachFile(uri: android.net.Uri, peer: String, session: String): JSONObject {
        val staged = withContext(Dispatchers.IO) {
            val name = resolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            }?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "附件"
            val directory = applicationContext.cacheDir.resolve("attach").apply { mkdirs() }
            val copy = File.createTempFile("pick-", ".tmp", directory)
            try {
                requireNotNull(resolver.openInputStream(uri)) { "无法读取所选文件" }.use { input ->
                    copy.outputStream().use { output ->
                        val buffer = ByteArray(64 * 1024); var total = 0L
                        while (true) {
                            val n = input.read(buffer); if (n < 0) break
                            total += n
                            check(total <= ATTACHMENT_LIMIT) { "单个附件不能超过 300 MiB" }
                            output.write(buffer, 0, n)
                        }
                    }
                }
            } catch (e: Throwable) { copy.delete(); throw e }
            name to copy
        }
        val (name, copy) = staged
        return try { command("attach_file", "peer" to peer, "session" to session, "path" to copy.absolutePath, "name" to name) }
        finally { withContext(Dispatchers.IO) { copy.delete() } }
    }

    /** A file the camera writes before it is attached. */
    fun cameraTarget(): android.net.Uri {
        // Only the latest shot is kept; attaching snapshots it into core.
        val directory = applicationContext.cacheDir.resolve("camera").apply { deleteRecursively(); mkdirs() }
        val name = "IMG_" + java.text.SimpleDateFormat("yyyyMMdd_HHmmss", java.util.Locale.ROOT).format(java.util.Date()) + ".jpg"
        return androidx.core.content.FileProvider.getUriForFile(applicationContext, "${applicationContext.packageName}.files", directory.resolve(name))
    }
    suspend fun saveChatFile(uri: android.net.Uri, ticket: String) = withContext(Dispatchers.IO) {
        requireNotNull(resolver.openFileDescriptor(uri, "w")).use { destination ->
            val result = JSONObject(NativeBridge.saveChatFile(root, ticket, destination.fd))
            check(result.optBoolean("ok")) { result.text("error", "保存副本失败") }
        }
    }
    /** Writes a prepared copy into the private cache and returns a shareable URI for another app. */
    suspend fun openChatFileCopy(ticket: String, name: String): android.net.Uri = withContext(Dispatchers.IO) {
        val directory = applicationContext.cacheDir.resolve("open").apply { deleteRecursively(); mkdirs() }
        val safe = name.replace(Regex("[/\\\\:]"), "_").ifBlank { "file" }
        val target = directory.resolve(safe)
        android.os.ParcelFileDescriptor.open(target, android.os.ParcelFileDescriptor.MODE_CREATE or
            android.os.ParcelFileDescriptor.MODE_TRUNCATE or android.os.ParcelFileDescriptor.MODE_WRITE_ONLY).use { destination ->
            val result = JSONObject(NativeBridge.saveChatFile(root, ticket, destination.fd))
            check(result.optBoolean("ok")) { result.text("error", "无法打开文件") }
        }
        androidx.core.content.FileProvider.getUriForFile(applicationContext, "${applicationContext.packageName}.files", target)
    }
    fun directoryEvents() = observations.directory()
    fun historyEvents(peer: String, session: String) = observations.history(peer, session)
    suspend fun historyOlder(peer: String, session: String) = observations.historyChange(peer, session, "older")
    suspend fun historyNewer(peer: String, session: String) = observations.historyChange(peer, session, "newer")
    suspend fun historyLatest(peer: String, session: String) = observations.historyChange(peer, session, "window")
    suspend fun historyAnchor(peer: String, session: String, id: String?) = observations.historyChange(peer, session, "window", id)
    suspend fun historyDetail(peer: String, session: String, id: String?) = observations.historyChange(peer, session, "detail", id)
    suspend fun historyRefresh(peer: String, session: String) = observations.historyChange(peer, session, "refresh")
    suspend fun older(peer: String, session: String) = observations.older(peer, session)
    suspend fun newer(peer: String, session: String) = observations.newer(peer, session)
    suspend fun windowAnchor(peer: String, session: String, anchor: String?) = observations.windowAnchor(peer, session, anchor)
    // Preserve ordering among local edits/sends, without waiting for a slow
    // network operation. The Rust store also serializes flush vs withdrawal.
    suspend fun command(op: String, vararg fields: Pair<String, Any?>): JSONObject {
        initialization.withLock {
            if (!initialized) {
                withContext(Dispatchers.IO) {
                    val imported = JSONObject(NativeBridge.call(root, JSONObject().put("op", "restore_navigation")
                        .put("peer", legacyPeer ?: JSONObject.NULL).toString()))
                    check(imported.optBoolean("ok")) { imported.optString("error") }
                }
                initialized = true
            }
        }
        val request = JSONObject().put("op", op)
        fields.forEach { (key, value) -> request.put(key, value ?: JSONObject.NULL) }
        val local = NativeBridge.isLocal(request.toString())
        return (if (local) localGate else networkGate).withLock {
            withContext(Dispatchers.IO) {
                val activate = op == "resume" || (op == "host_visibility" && request.optBoolean("visible")) || (op in listOf("background_service", "adb_background_service") && request.optBoolean("running"))
                if (activate) org.rustls.platformverifier.CertificateVerifier.clearFailures()
                if (activate && multicast?.isHeld == false) multicast.acquire()
                val response = try { JSONObject(NativeBridge.call(root, request.toString())) }
                    finally {
                        val release = op == "pause" || (op in listOf("background_service", "adb_background_service") && !request.optBoolean("running"))
                        if (release && multicast?.isHeld == true) multicast.release()
                    }
                if (activate && !response.optBoolean("ok") && multicast?.isHeld == true) multicast.release()
                check(response.optBoolean("ok")) { response.optString("error", "操作未完成") }
                val data = response.optJSONObject("data") ?: JSONObject()
                if (op == "host_visibility" && !data.optBoolean("host_visible") && multicast?.isHeld == true) multicast.release()
                if (data.has("network_active") && !data.optBoolean("network_active") && multicast?.isHeld == true) multicast.release()
                data
            }
        }
    }
}
