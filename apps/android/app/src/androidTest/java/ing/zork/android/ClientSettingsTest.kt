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
                // Documented order: 客户端 · Mesh · 高级, rows in their documented order.
                fun assertOrder(order: List<String>) {
                    val tops = order.map { label ->
                        var node = find(label)
                        val until = System.currentTimeMillis() + 3000
                        while (node == null && System.currentTimeMillis() < until) { Thread.sleep(80); node = find(label) }
                        assertNotNull(label, node)
                        android.graphics.Rect().also { node!!.getBoundsInScreen(it) }.top
                    }
                    order.indices.drop(1).forEach { i -> assertTrue("${order[i - 1]} before ${order[i]}", tops[i - 1] < tops[i]) }
                }
                assertOrder(listOf("客户端", "Zork 账号", "外观", "通知", "已归档的 Chat", "Mesh", "模型连接", "mini1", "连接设备"))
                // 高级 sits below the fold on a phone: scroll, then check the tail of the order.
                fun scrollable(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                    if (node == null) return null
                    if (node.isScrollable) return node
                    for (i in 0 until node.childCount) scrollable(node.getChild(i))?.let { return it }
                    return null
                }
                instrumentation.uiAutomation.clearCache()
                scrollable(instrumentation.uiAutomation.rootInActiveWindow)?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
                instrumentation.waitForIdleSync(); Thread.sleep(300)
                assertOrder(listOf("连接设备", "高级", "安卓调试"))
                scrollable(instrumentation.uiAutomation.rootInActiveWindow)?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD)
                instrumentation.waitForIdleSync(); Thread.sleep(300)
                // Archived Chats open from client settings and can be restored there.
                click("已归档的 Chat"); await("旧版导航"); capture("archived")
                click("取消归档")
                scenario.onActivity { assertEquals("archive:mini1/old/false", it.lastAction) }
                await("没有已归档的 Chat")
                click("返回"); await("模型连接")
                // The "连接设备" row hands straight to the connect action.
                click("连接设备")
                scenario.onActivity { assertEquals("add-device", it.lastAction) }
                assertNotNull(find("外观")); assertNull(find("文字大小"))
                scenario.recreate(); instrumentation.waitForIdleSync(); await("外观")
                assertNotNull(find("外观"))
                assertNull(find("帮助与诊断")); assertNull(find("关于 Zork"))
                click("mini1")
                assertNotNull(find("服务")); assertNull(find("连接其他设备"))
                capture("device")
                click("返回对话"); await("模型连接")
            }
            // The home list no longer carries an archived entry; it lives in settings.
            ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java).putExtra("screen", "navigation").putExtra("width", 0)).use {
                instrumentation.waitForIdleSync(); await("品牌规范整理")
                assertNull(find("已归档")); assertNull(find("旧版导航"))
            }
        }
    }
}
