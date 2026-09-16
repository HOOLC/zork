package surf.zork.android

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.graphics.Bitmap
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class LocalScriptsTest {
    @Test fun explicitClickNativeCallbacksRecreationAndCancellation() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val automation = instrumentation.uiAutomation
        val service = automation.serviceInfo
        service.flags = service.flags or AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
        automation.serviceInfo = service
        val clipboard = context.getSystemService(ClipboardManager::class.java)
        fun find(text: String? = null, resource: String? = null): AccessibilityNodeInfo? {
            automation.clearCache()
            fun walk(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (node == null) return null
                if ((text != null && (node.text?.toString() == text || node.contentDescription?.toString() == text)) || (resource != null && node.viewIdResourceName == resource)) return node
                repeat(node.childCount) { walk(node.getChild(it))?.let { found -> return found } }
                return null
            }
            return walk(automation.rootInActiveWindow)
        }
        fun await(label: String, test: () -> Boolean) {
            val until = System.currentTimeMillis() + 10_000
            while (System.currentTimeMillis() < until && !test()) Thread.sleep(40)
            assertTrue(label, test())
        }
        fun click(text: String? = null, resource: String? = null) {
            await(text ?: resource!!) { find(text, resource) != null }
            var node = find(text, resource)
            while (node != null && !node.isClickable) node = node.parent
            assertTrue(text ?: resource, node?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true)
            instrumentation.waitForIdleSync()
        }
        fun capture(name: String) {
            val directory = File(context.filesDir, "local-script-evidence").apply { mkdirs() }
            val image = automation.takeScreenshot()
            File(directory, "$name.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
            image.recycle()
        }
        fun closePanel() {
            click("关闭")
            await("script panel dismissed") { find("在本机执行") != null }
        }
        fun text(): String? {
            var value: String? = null
            instrumentation.runOnMainSync { value = clipboard.primaryClip?.getItemAt(0)?.text?.toString() }
            return value
        }
        fun launch(source: String) = ActivityScenario.launch<LocalScriptPreviewActivity>(Intent(context, LocalScriptPreviewActivity::class.java).putExtra("source", source))

        val source = "if ((await android.clipboard.read()) !== 'unchanged') throw new Error('unexpected clipboard'); await android.clipboard.write('script executed'); console.log('local-only'); await android.startActivity({action:'android.settings.APPLICATION_DEVELOPMENT_SETTINGS'});"
        launch(source).use { scenario ->
            await("script card") { find("在本机执行") != null }
            instrumentation.runOnMainSync { clipboard.setPrimaryClip(ClipData.newPlainText("", "unchanged")) }
            scenario.recreate()
            await("card after recreation") { find("在本机执行") != null }
            assertEquals("unchanged", text())
            capture("card-before-run")
            click("在本机执行")
            await("real settings activity") { automation.rootInActiveWindow?.packageName?.toString() == "com.android.settings" }
            capture("developer-options")
            automation.performGlobalAction(AccessibilityService.GLOBAL_ACTION_BACK)
            await("local completion") { find("脚本已完成") != null }
            assertEquals("script executed", text())
            assertNotNull(find("local-only"))
            capture("local-completion")
            lateinit var model: LocalScriptPreviewModel
            scenario.onActivity { model = it.model }
            val messages = runBlocking { model.repo.command("cached_messages", "peer" to "local-script-node", "session" to model.chat) }
                .getJSONObject("snapshot").getJSONObject("body").getJSONArray("items")
            assertEquals("execution does not append a result message", 1, messages.length())
            assertEquals(source, messages.getJSONObject(0).getJSONObject("interaction").getString("source"))
            closePanel()
        }

        launch("const r = await android.startActivityForResult({action:'surf.zork.android.SCRIPT_RESULT',package:'${context.packageName}'}); console.log('answer=' + r.extras.answer);").use { scenario ->
            click("在本机执行")
            await("native callback fixture") { find("返回结果") != null }
            // Keep the callback Activity in front while recreating its stopped owner.
            lateinit var previous: LocalScriptPreviewActivity
            scenario.onActivity { previous = it; it.recreate() }
            click("返回结果")
            await("callback after parent recreation") { find("answer=42") != null }
            scenario.onActivity { assertNotSame(previous, it) }
            assertNotNull(find("脚本已完成"))
            capture("activity-callback")
            closePanel()
        }

        launch("const granted = await android.requestPermissions(['android.permission.CAMERA']); console.log(granted['android.permission.CAMERA'] ? 'Granted' : 'Denied');").use {
            click("在本机执行")
            click(resource = "com.android.permissioncontroller:id/permission_deny_button")
            await("denied permission resolves locally") { find("Denied") != null }
            assertNotNull(find("脚本已完成"))
            capture("permission-denied")
            closePanel()
        }

        launch("await android.sleep(30000); await android.clipboard.write('must not happen');").use {
            click("在本机执行")
            click("停止执行")
            await("cancellation") { find("脚本已取消") != null }
            assertEquals("script executed", text())
            capture("cancelled")
            closePanel()
        }
    }
}
