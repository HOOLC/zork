package ing.zork.android

import android.content.Context
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.transformWhile
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File

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
    external fun sharedFileBytes(root: String, content: String): ByteArray
    external fun saveSharedFile(root: String, ticket: String, descriptor: Int): String
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

    // Android supplies URI bytes; the core decides encoding, limits and draft membership.
    suspend fun attachText(uri: android.net.Uri, peer: String, session: String) {
        val (name, bytes) = withContext(Dispatchers.IO) {
            val name = resolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            } ?: "attachment.txt"
            val bytes = ByteArray(NativeBridge.textAttachmentLimit() + 1)
            val length = requireNotNull(resolver.openInputStream(uri)).use { input ->
                var used = 0
                while (used < bytes.size) { val n = input.read(bytes, used, bytes.size - used); if (n < 0) break; used += n }
                used
            }
            name to bytes.copyOf(length)
        }
        command("draft_action", "peer" to peer, "session" to session, "operation" to JSONObject().put("action", "attach_text")
            .put("name", name).put("bytes", org.json.JSONArray(bytes.map { it.toInt() and 255 })))
    }
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
    fun notificationEvents() = observations.notifications()
    fun adbEvents() = observations.adb()
    fun accountEvents() = observations.account()
    fun dataResetEvents() = observations.dataReset()
    suspend fun clearData() = withContext(Dispatchers.IO) {
        val response = JSONObject(NativeBridge.clearData(root, true, applicationContext))
        check(response.optBoolean("ok")) { response.optString("error", "无法清空数据") }
    }
    fun localScriptEvents() = observations.localScripts()
    fun resourceEvents(selection: ResourceSelection) = observations.resources(selection)
    fun sharedFileEvents() = observations.sharedFiles()
    suspend fun sharedPreviewBytes(content: String): ByteArray = withContext(Dispatchers.IO) { NativeBridge.sharedFileBytes(root, content) }
    suspend fun saveSharedFile(uri: android.net.Uri, ticket: String) = withContext(Dispatchers.IO) {
        requireNotNull(resolver.openFileDescriptor(uri, "w")).use { destination ->
            val result = JSONObject(NativeBridge.saveSharedFile(root, ticket, destination.fd))
            check(result.optBoolean("ok")) { result.text("error", "保存副本失败") }
        }
    }
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
    @OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
    fun invitationEvents() = observations.invitation().transformWhile {
        val value = it.value.getJSONObject("snapshot")
        emit(value)
        !value.optBoolean("done")
    }

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
