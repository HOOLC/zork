package ing.zork.android

import android.graphics.Bitmap
import android.graphics.Rect
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SharedFilesTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation
    private fun settle() { Thread.sleep(500); instrumentation.waitForIdleSync() }
    private fun nodes(label: String, prefix: Boolean = false): List<AccessibilityNodeInfo> {
        automation.clearCache()
        val result = mutableListOf<AccessibilityNodeInfo>()
        fun visit(node: AccessibilityNodeInfo?) {
            if (node == null) return
            val text = node.text?.toString().orEmpty()
            if ((text == label || (prefix && text.startsWith(label)) || node.contentDescription?.toString() == label) && node.isVisibleToUser) result.add(node)
            for (i in 0 until node.childCount) visit(node.getChild(i))
        }
        visit(automation.rootInActiveWindow)
        return result
    }
    private fun click(label: String, prefix: Boolean = false) {
        var target = nodes(label, prefix).firstOrNull() ?: error("Missing $label")
        while (!target.isClickable) target = target.parent ?: error("Not clickable: $label")
        val bounds = Rect().also { target.getBoundsInScreen(it) }
        val density = instrumentation.targetContext.resources.displayMetrics.density
        if (bounds.height() / density < 43.5f) {
            val tree = StringBuilder()
            fun dump(node: AccessibilityNodeInfo?, depth: Int = 0) {
                if (node == null) return
                val rect = Rect().also { node.getBoundsInScreen(it) }
                tree.appendLine("${" ".repeat(depth)}${node.className} text=${node.text} description=${node.contentDescription} clickable=${node.isClickable} $rect")
                for (i in 0 until node.childCount) dump(node.getChild(i), depth + 1)
            }
            dump(automation.rootInActiveWindow)
            instrumentation.targetContext.filesDir.resolve("shared-small-target.txt").writeText(tree.toString())
            capture("small-target")
        }
        assertTrue("Small touch target: $label $bounds", bounds.height() / density >= 43.5f)
        assertTrue(target.performAction(AccessibilityNodeInfo.ACTION_CLICK))
        settle()
    }
    private fun capture(name: String) {
        val image = automation.takeScreenshot() ?: error("Screenshot unavailable")
        instrumentation.targetContext.filesDir.resolve("shared-$name.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
        image.recycle()
    }
    @Test fun nativeCommandsSnapshotsAndDescriptorOwnership() {
        val context = instrumentation.targetContext
        NativeBridge.initialize(context)
        val root = context.noBackupFilesDir.resolve("shared-files-jni-test").absolutePath
        val command = JSONObject().put("op", "shared_files").put("operation", JSONObject().put("action", "activate").put("active", false)).toString()
        assertTrue(NativeBridge.isLocal(command))
        val reply = JSONObject(NativeBridge.call(root, command))
        assertTrue(reply.toString(), reply.getBoolean("ok"))
        fun observe(request: JSONObject): JSONObject {
            val response = JSONObject(NativeBridge.observe(root, request.toString()))
            assertTrue(response.toString(), response.getBoolean("ok"))
            return response.optJSONObject("data") ?: JSONObject()
        }
        val generation = 91L
        val handle = observe(JSONObject().put("op", "open").put("generation", generation)
            .put("key", JSONObject().put("projection", "shared_files"))).getLong("handle")
        fun request(op: String) = JSONObject().put("op", op).put("handle", handle).put("generation", generation)
        try {
            val frame = observe(request("prepare"))
            assertEquals(0, frame.getJSONObject("snapshot").getJSONArray("entries").length())
            observe(request("finish").put("batch", frame.getLong("batch")).put("applied", true))
            fun sharedAction(operation: JSONObject) {
                val reply = JSONObject(NativeBridge.call(root, JSONObject().put("op", "shared_files").put("operation", operation).toString()))
                assertTrue(reply.toString(), reply.getBoolean("ok"))
            }
            fun awaitActive(active: Boolean) {
                repeat(100) {
                    val next = observe(request("prepare"))
                    if (next.has("batch")) {
                        val matches = next.getJSONObject("snapshot").getBoolean("active") == active
                        observe(request("finish").put("batch", next.getLong("batch")).put("applied", true))
                        if (matches) return
                    }
                    Thread.sleep(20)
                }
                error("Shared page did not reach active=$active")
            }
            sharedAction(JSONObject().put("action", "activate").put("active", true))
            awaitActive(true)
            sharedAction(JSONObject().put("action", "back"))
            awaitActive(false)
            val unknown = parseSharedFiles(JSONObject("""{"devices":[{"id":"unknown","name":"Unknown","online":null}],"spaces":[],"entries":[]}"""))
            assertNull(unknown.devices.single().online)
            assertTrue(NativeBridge.sharedFileBytes(root, "missing-content").isEmpty())
            val file = context.cacheDir.resolve("shared-descriptor-test.txt")
            android.os.ParcelFileDescriptor.open(file, android.os.ParcelFileDescriptor.MODE_CREATE or android.os.ParcelFileDescriptor.MODE_READ_WRITE or android.os.ParcelFileDescriptor.MODE_TRUNCATE).use { descriptor ->
                val saved = JSONObject(NativeBridge.saveSharedFile(root, "missing-ticket", descriptor.fd))
                assertFalse(saved.getBoolean("ok"))
                android.system.Os.write(descriptor.fileDescriptor, byteArrayOf(7), 0, 1)
            }
            assertArrayEquals(byteArrayOf(7), file.readBytes())
            file.delete()
        } finally { observe(request("close")) }
    }
    @Test fun listPreviewReturnSourcesVersionsAndLargeDirectory() {
        ActivityScenario.launch(SharedFilesPreviewActivity::class.java).use { scenario ->
            settle()
            capture("list")
            scenario.onActivity {
                assertEquals(100_000, it.data.entries.size)
                assertTrue(it.listState!!.layoutInfo.visibleItemsInfo.size in 1..24)
                it.scrollTo(50_000)
            }
            settle()
            scenario.onActivity {
                assertEquals(50_000, it.listState!!.firstVisibleItemIndex)
                assertTrue(it.listState!!.layoutInfo.visibleItemsInfo.size in 1..24)
            }
            click("项目资料 050000.txt")
            capture("preview")
            click("120 B")
            capture("versions")
            click("笔记本 · 100 B", prefix = true)
            scenario.onActivity { assertEquals("second", it.data.preview!!.selected) }
            click("保存副本")
            scenario.onActivity { assertEquals("save", it.commands.last()) }
            click("返回")
            scenario.onActivity { assertEquals(50_000, it.listState!!.firstVisibleItemIndex) }
            click("更多")
            click("共享来源")
            capture("sources")
            click("笔记本 · 离线")
            scenario.onActivity { assertEquals("source:laptop", it.commands.last()) }
            scenario.onActivity { it.scrollTo(99_999) }
            settle()
            assertTrue(nodes("项目资料 099999.txt").isNotEmpty())
            scenario.onActivity { synchronized(it.frameTimes) { it.frameTimes.clear() } }
            for (index in 99_980 downTo 99_961) {
                scenario.onActivity { it.scrollTo(index) }
                Thread.sleep(60)
                instrumentation.waitForIdleSync()
            }
            scenario.onActivity {
                val samples = synchronized(it.frameTimes) { it.frameTimes.sorted() }
                assertTrue(samples.size >= 10)
                fun percentile(fraction: Double) = samples[(kotlin.math.ceil(samples.size * fraction).toInt() - 1).coerceAtLeast(0)]
                val p95 = percentile(.95)
                assertTrue("Shared-file layout/draw p95 $p95 ms exceeded 50 ms", p95 < 50)
                val report = JSONObject().put("items", it.data.entries.size).put("visible", it.listState!!.layoutInfo.visibleItemsInfo.size)
                    .put("frames", samples.size).put("layout_draw_p95_ms", p95).put("layout_draw_p99_ms", percentile(.99))
                instrumentation.targetContext.filesDir.resolve("shared-report.json").writeText(report.toString())
            }
            click("更多")
            click("图标布局")
            capture("grid")
        }
    }
}
