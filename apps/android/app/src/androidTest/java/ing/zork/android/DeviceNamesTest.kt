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

/** The home device strip shows short Mesh display names; touch has no hover,
 * so a long press shows the machine name each device registered with. */
@RunWith(AndroidJUnit4::class)
class DeviceNamesTest {
    @Test fun displayNamesAndMachineNameOnLongPress() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val folder = File(context.filesDir, "device-names").apply { mkdirs() }
        fun find(match: (String) -> Boolean): AccessibilityNodeInfo? {
            instrumentation.uiAutomation.clearCache()
            fun walk(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (node == null) return null
                val text = node.text?.toString().orEmpty(); val description = node.contentDescription?.toString().orEmpty()
                if (match(text) || match(description)) return node
                for (i in 0 until node.childCount) walk(node.getChild(i))?.let { return it }
                return null
            }
            // The tooltip is a popup window of its own.
            val automation = instrumentation.uiAutomation
            automation.serviceInfo = automation.serviceInfo.apply {
                flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
            }
            val roots = automation.windows.mapNotNull { it.root }.ifEmpty { listOfNotNull(automation.rootInActiveWindow) }
            for (root in roots) walk(root)?.let { return it }
            return null
        }
        fun await(label: String, match: (String) -> Boolean): AccessibilityNodeInfo {
            val end = System.currentTimeMillis() + 5000
            var node = find(match)
            while (node == null && System.currentTimeMillis() < end) { Thread.sleep(80); node = find(match) }
            assertNotNull(label, node); return node!!
        }
        fun capture(name: String) {
            Thread.sleep(800)
            val shot = instrumentation.uiAutomation.takeScreenshot()
            File(folder, "$name.png").outputStream().use { shot.compress(Bitmap.CompressFormat.PNG, 100, it) }
            shot.recycle()
        }
        ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java)
            .putExtra("screen", "device-names").putExtra("width", 0)).use {
            instrumentation.waitForIdleSync(); Thread.sleep(500)
            val chip = await("display-name chip") { it == "A（zuozijiandeMacBook-Air） · 直连 · 设备设置" }
            await("relay device") { it == "B（zuozijians-Mac-Studio） · 中继 · 设备设置" }
            assertNull("the long machine name is not shown at first glance", find { it == "zuozijiandeMacBook-Air" })
            capture("device-strip")
            var target: AccessibilityNodeInfo? = chip
            while (target != null && !target.isLongClickable) target = target.parent
            assertTrue(target!!.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK))
            instrumentation.waitForIdleSync(); capture("machine-name")
            await("machine name tooltip") { it == "机器名称：zuozijiandeMacBook-Air" }
        }
    }
}
