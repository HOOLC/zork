package ing.zork.android

import android.app.Application
import android.graphics.Bitmap
import android.os.ParcelFileDescriptor
import android.view.accessibility.AccessibilityNodeInfo
import androidx.activity.compose.setContent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.security.MessageDigest

/** Only scripts/android/test_file_delivery.py creates this isolated fixture. */
@RunWith(AndroidJUnit4::class)
class ChatFileDeliveryTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val args = InstrumentationRegistry.getArguments()
    private val directory get() = context.noBackupFilesDir.resolve("file-delivery-client")
    private val root get() = directory.absolutePath

    private fun initialize() {
        assumeTrue("task-owned emulator required", android.os.Build.HARDWARE == "ranchu" && args.getString("file_fixture") == "true")
        NativeBridge.initialize(context)
    }
    private fun call(op: String, vararg fields: Pair<String, Any?>): JSONObject {
        val request = JSONObject().put("op", op)
        fields.forEach { (key, value) -> request.put(key, value ?: JSONObject.NULL) }
        val reply = JSONObject(NativeBridge.call(root, request.toString()))
        assertTrue(reply.toString(), reply.getBoolean("ok"))
        return reply.optJSONObject("data") ?: JSONObject()
    }
    private fun action(name: String, vararg fields: Pair<String, String>) {
        val operation = JSONObject().put("action", name)
        fields.forEach { (key, value) -> operation.put(key, value) }
        call("chat_files", "operation" to operation)
    }
    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private fun capture(name: String) {
        val image = instrumentation.uiAutomation.takeScreenshot() ?: error("screenshot unavailable")
        context.filesDir.resolve("file-delivery-$name.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
        image.recycle()
    }
    private fun click(label: String) {
        instrumentation.uiAutomation.clearCache()
        fun find(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            if (node == null) return null
            if (node.isVisibleToUser && (node.text?.toString() == label || node.contentDescription?.toString() == label)) return node
            for (i in 0 until node.childCount) find(node.getChild(i))?.let { return it }
            return null
        }
        var target = find(instrumentation.uiAutomation.rootInActiveWindow) ?: error("missing $label")
        while (!target.isClickable) target = target.parent ?: error("not clickable: $label")
        assertTrue(target.performAction(AccessibilityNodeInfo.ACTION_CLICK))
    }
    @Test fun bootstrap() {
        initialize()
        directory.deleteRecursively()
        call("network", "network" to JSONObject().put("direct_only", true))
        context.filesDir.resolve("file-delivery-identity.json").writeText(call("snapshot").toString())
    }
    @Test fun realSharedFilesAndChatImagesUseCoreAndNativeExport() = runBlocking<Unit> {
        initialize()
        val peer = args.getString("origin")!!
        val session = args.getString("chat")!!
        val message = args.getString("message")!!
        val binary = args.getString("binary")!!
        call("resume")
        call("save_peer", "origin" to peer, "name" to "文件交付测试", "address" to args.getString("address"))
        call("resume")
        val models = androidx.lifecycle.ViewModelStore()
        ActivityScenario.launch(Nav7PreviewActivity::class.java).use { scenario ->
            lateinit var model: ClientViewModel
            scenario.onActivity { activity ->
                model = ClientViewModel(context.applicationContext as Application, ClientRepository(context, directory))
                models.put("files", model)
                activity.setContent { ZorkTheme { ClientScreen(model) } }
                model.foreground(true)
            }
            suspend fun awaitModel(label: String, ready: (ClientViewModel) -> Boolean) {
              try { withTimeout(45000) {
                while (true) {
                    var done = false
                    scenario.onActivity { done = ready(model) }
                    if (done) break
                    delay(30)
                }
              } } catch (error: Exception) {
                var detail = ""
                scenario.onActivity { detail = "ready=${model.ready} connected=${model.connected} notice=${model.notice} rows=${model.messages.map { it.id to it.deliveredFiles }} preview=${model.chatFile} shared=${model.sharedFiles}" }
                capture("failure")
                throw AssertionError("$label: $detail", error)
              }
            }
            try {
                awaitModel("client ready") { it.ready }
                scenario.onActivity { model.selectPeer(Peer(peer, "文件交付测试", "")) }
                awaitModel("peer connected") { it.connected }
                scenario.onActivity { model.openSession(JSONObject().put("chat_id", session).put("title", "文件交付测试")) }
                awaitModel("delivered file row") { it.messages.any { row -> row.id == message && row.deliveredFiles.size == 2 } }
                delay(700)
                capture("chat")
                click("delivery.png")
                awaitModel("Chat image preview") { it.chatFile?.contentReady == true && it.chatFileImage != null }
                scenario.onActivity {
                    assertEquals(96, model.chatFileImage!!.width)
                    assertEquals(64, model.chatFileImage!!.height)
                    assertEquals(args.getString("image_sha"), sha(NativeBridge.chatFileBytes(root, model.chatFile!!.key)))
                }
                delay(500)
                capture("chat-image")
                click("关闭")
                awaitModel("Chat preview closed") { it.chatFile == null }
                scenario.onActivity { model.openSharedFiles() }
                awaitModel("shared image entry") { it.sharedFiles?.entries?.any { entry -> entry.name == "visible.png" } == true }
                delay(500)
                click("visible.png")
                awaitModel("shared image preview") { it.sharedFileImage != null && it.sharedFiles?.preview?.loading == false }
                scenario.onActivity {
                    val selected = model.sharedFiles!!.preview!!.selected
                    assertEquals(args.getString("image_sha"), sha(NativeBridge.sharedFileBytes(root, selected)))
                }
                delay(500)
                capture("shared-image")
            } finally {
                scenario.onActivity { model.foreground(false); models.clear() }
            }
        }
        // The same public core intent and JNI descriptor adapter used by the
        // system picker, with a private destination we can verify byte for byte.
        call("resume")
        action("open", "peer" to peer, "session" to session, "message" to message, "file" to binary)
        suspend fun preview(ready: (JSONObject) -> Boolean): JSONObject {
            var result = JSONObject()
            withTimeout(30000) {
                Observations(root).chatFiles().first { frame ->
                    result = frame.value.getJSONObject("snapshot").optJSONObject("preview") ?: JSONObject()
                    ready(result)
                }
            }
            return result
        }
        val opened = preview { it.has("key") }
        action("prepare_save", "key" to opened.getString("key"))
        val prepared = preview { !it.isNull("save_ticket") }
        call("pause")
        val saved = context.cacheDir.resolve("delivered.bin")
        ParcelFileDescriptor.open(saved, ParcelFileDescriptor.MODE_CREATE or ParcelFileDescriptor.MODE_READ_WRITE or ParcelFileDescriptor.MODE_TRUNCATE).use { fd ->
            val response = JSONObject(NativeBridge.saveChatFile(root, prepared.getString("save_ticket"), fd.fd))
            assertTrue(response.toString(), response.getBoolean("ok"))
            android.system.Os.fsync(fd.fileDescriptor) // adapter did not close the host descriptor
        }
        assertEquals(args.getString("binary_sha"), sha(saved.readBytes()))
        context.filesDir.resolve("file-delivery-result.json").writeText(JSONObject()
            .put("chat_image", true).put("shared_image", true).put("image_sha256", args.getString("image_sha"))
            .put("export_bytes", saved.length()).put("export_sha256", sha(saved.readBytes()))
            .put("picker_pause_preserves_ticket", true).toString())
    }
}
