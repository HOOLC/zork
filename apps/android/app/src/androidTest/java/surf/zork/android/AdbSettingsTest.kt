package surf.zork.android

import android.content.Intent
import android.os.Bundle
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class AdbSettingsTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val automation = instrumentation.uiAutomation
    private val examples by lazy {
        JSONObject(instrumentation.context.assets.open("adb-states.json").bufferedReader().use { it.readText() })
    }
    private fun snapshot(name: String) = JSONObject(examples.getJSONObject(name).toString())
    private fun node(label: String): AccessibilityNodeInfo? {
        automation.clearCache()
        fun find(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            if (node == null) return null
            if (node.isVisibleToUser && (node.text?.toString() == label || node.contentDescription?.toString() == label)) return node
            for (i in 0 until node.childCount) find(node.getChild(i))?.let { return it }
            return null
        }
        return find(automation.rootInActiveWindow)
    }
    private fun await(label: String): AccessibilityNodeInfo {
        val end = SystemClock.uptimeMillis() + 6000
        while (SystemClock.uptimeMillis() < end) { node(label)?.let { return it }; Thread.sleep(40) }
        throw AssertionError("Missing $label")
    }
    private fun click(label: String) {
        var target: AccessibilityNodeInfo? = await(label)
        while (target != null && !target.isClickable) target = target.parent
        assertTrue("Cannot click $label", target?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true)
        instrumentation.waitForIdleSync()
    }
    private fun capture(name: String) {
        val folder = File(instrumentation.targetContext.filesDir, "adb-settings").apply { mkdirs() }
        automation.takeScreenshot()?.let { bitmap ->
            File(folder, name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
            bitmap.recycle()
        }
    }
    private fun launch(name: String, width: Int = 0): ActivityScenario<AdbPreviewActivity> =
        ActivityScenario.launch(Intent(instrumentation.targetContext, AdbPreviewActivity::class.java)
            .putExtra("snapshot", snapshot(name).toString()).putExtra("width", width))

    @Test fun onlyTheCurrentPhoneStepIsVisibleAndPortInputReachesCoreUnchanged() {
        launch("disabled").use { scenario ->
            await("开始设置")
            assertNull(node("打开开发者选项"))
            assertNull(node("查看激活方法"))
            assertNull(node("Mac mini"))
            click("开始设置")
            scenario.onActivity {
                assertEquals("set_enabled", it.operations.single().getString("action"))
                assertTrue(it.operations.single().getBoolean("enabled"))
                assertFalse(it.operations.single().has("peer"))
                it.snapshot = snapshot("developer")
            }
            await("打开系统设置")
            assertNull(node("打开开发者选项"))
            assertNull(node("查看激活方法"))
            capture("developer.png")
            scenario.onActivity { it.snapshot = snapshot("usb") }
            await("打开开发者选项")
            assertNull(node("查看激活方法"))
            scenario.onActivity { it.snapshot = snapshot("activation") }
            await("查看激活方法")
            assertNull(node("手机调试端口"))
            click("查看激活方法")
            await("通过 USB 激活")
            assertNull(node("手机调试端口"))
            capture("activation.png")
            click("使用了其他调试端口？")
            await("手机调试端口")
            fun editable(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (node == null) return null
                if (node.isEditable) return node
                for (i in 0 until node.childCount) editable(node.getChild(i))?.let { return it }
                return null
            }
            val input = editable(automation.rootInActiveWindow) ?: throw AssertionError("Missing port input")
            assertTrue(input.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "not-a-port")
            }))
            scenario.onActivity { it.failure = "端口校验结果" }
            click("保存端口")
            await("端口校验结果")
            scenario.onActivity {
                assertEquals("set_port", it.operations.last().getString("action"))
                assertEquals("not-a-port", it.operations.last().getString("port"))
                it.snapshot = snapshot("ready")
            }
            await("远程调试已开启")
            assertNull(node("通过 USB 激活"))
            assertNull(node("手机调试端口"))
        }
    }

    @Test fun stationAuthorizationDoesNotBlockOtherConnectionsAndStaleHelpCloses() {
        launch("mixed", 320).use { scenario ->
            await("远程调试已开启")
            await("Mac mini")
            await("MacBook Air")
            await("已连接")
            await("等待手机授权")
            assertNull(node("开始设置"))
            assertNull(node("查看激活方法"))
            capture("mixed-320.png")
            click("查看提示")
            await("在手机上允许调试")
            scenario.onActivity { it.snapshot = snapshot("ready") }
            await("关闭调试")
            assertNull(node("在手机上允许调试"))
            assertNull(node("查看提示"))
            capture("ready-320.png")
            click("详情")
            await("127.0.0.1:41001")
            await("127.0.0.1:41002")
            click("关闭")
            click("关闭调试")
            scenario.onActivity {
                assertEquals("set_enabled", it.operations.single().getString("action"))
                assertFalse(it.operations.single().getBoolean("enabled"))
                it.snapshot = snapshot("waiting")
            }
            await("等待 Station 连接")
            assertNull(node("Mac mini"))
            assertNotNull(node("关闭调试"))
        }
    }
}
