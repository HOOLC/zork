package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class ClientSettingsTest {
    @Test fun clientSettingsAndDeviceNavigationAreAvailable() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val folder = File(context.filesDir, "client-settings").apply { mkdirs() }
        fun find(text: String): AccessibilityNodeInfo? {
            instrumentation.uiAutomation.clearCache()
            fun walk(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (node == null) return null
                if (node.text?.toString() == text || node.text?.toString()?.lineSequence()?.any { it == text } == true || node.contentDescription?.toString() == text) return node
                for (i in 0 until node.childCount) walk(node.getChild(i))?.let { return it }
                return null
            }
            return walk(instrumentation.uiAutomation.rootInActiveWindow)
        }
        fun click(text: String) {
            var node = find(text)
            val until = System.currentTimeMillis() + 5000
            while (node == null && System.currentTimeMillis() < until) { Thread.sleep(80); node = find(text) }
            assertNotNull(text, node)
            while (node != null && !node.isClickable) node = node.parent
            assertTrue(text, node!!.performAction(AccessibilityNodeInfo.ACTION_CLICK))
            instrumentation.waitForIdleSync(); Thread.sleep(300)
        }
        var barInsets = androidx.core.graphics.Insets.NONE
        fun capture(name: String) {
            val image = instrumentation.uiAutomation.takeScreenshot()
            assertNotNull(image)
            if (name != "home-failure") {
                assertTrue("status bar inset", barInsets.top > 0)
                assertTrue("navigation bar inset", barInsets.bottom > 0)
                // Status icons and the clock sit on the bar, so judge its background by
                // the colour most of the row shows rather than one pixel.
                fun background(y: Int) = (0 until image.width step 4).map { image.getPixel(it, y) }
                    .groupingBy { it }.eachCount().maxBy { it.value }.key
                assertEquals("$name status bar background", android.graphics.Color.WHITE,
                    background(barInsets.top / 2))
                assertEquals("$name navigation bar background", android.graphics.Color.WHITE,
                    background(image.height - barInsets.bottom / 2))
            }
            File(folder, "$name.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG, 100, it) }
            image.recycle()
        }
        fun await(text: String) {
            val until = System.currentTimeMillis() + 5000
            while (find(text) == null && System.currentTimeMillis() < until) Thread.sleep(80)
            if (find(text) == null) capture("home-failure")
            assertNotNull(text, find(text))
        }
        run {
            ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java).putExtra("screen", "home").putExtra("width", 0)).use { scenario ->
                instrumentation.waitForIdleSync(); await("模型连接")
                scenario.onActivity { activity ->
                    barInsets = androidx.core.view.ViewCompat.getRootWindowInsets(activity.window.decorView)!!
                        .getInsets(androidx.core.view.WindowInsetsCompat.Type.systemBars())
                }
                assertNull(find("这台手机")); assertNull(find("查看连接身份")); assertNotNull(find("通知")); assertNotNull(find("Mesh")); assertNull(find("工具连接"))
                capture("home")
                assertNotNull(find("外观")); assertNull(find("文字大小"))
                scenario.recreate(); instrumentation.waitForIdleSync(); await("外观")
                assertNotNull(find("外观"))
                assertNull(find("帮助与诊断")); assertNull(find("关于 Zork"))
                click("mini1")
                assertNotNull(find("服务")); assertNull(find("连接其他设备"))
                capture("device")
                click("返回对话"); await("模型连接")
            }

        }
    }
}
