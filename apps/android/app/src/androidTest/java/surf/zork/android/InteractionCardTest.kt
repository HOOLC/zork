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
class InteractionCardTest {
    @Test fun rendersCoreSnapshotsAndSubmitsUserInput() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        fun snapshot(name: String) = instrumentation.context.assets.open("interaction-$name.json").bufferedReader().use { it.readText() }
        fun node(label: String): AccessibilityNodeInfo? {
            automation.clearCache()
            fun find(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.isVisibleToUser && (n.contentDescription?.toString() == label || n.text?.toString() == label)) return n
                for (i in 0 until n.childCount) find(n.getChild(i))?.let { return it }
                return null
            }
            return find(automation.rootInActiveWindow)
        }
        fun await(label: String): AccessibilityNodeInfo {
            val end = SystemClock.uptimeMillis() + 6000
            while (SystemClock.uptimeMillis() < end) { node(label)?.let { return it }; Thread.sleep(40) }
            throw AssertionError("Missing $label")
        }
        fun capture(name: String) {
            val folder = File(instrumentation.targetContext.filesDir, "interactive-messages").apply { mkdirs() }
            automation.takeScreenshot()?.let { bitmap -> File(folder, name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle() }
        }
        ActivityScenario.launch<InteractionPreviewActivity>(Intent(instrumentation.targetContext, InteractionPreviewActivity::class.java).putExtra("card", snapshot("ready"))).use { scenario ->
            await("安排执行任务")
            capture("ready.png")
            fun findEditable(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.contentDescription?.toString() == "任务名称") {
                    var parent: AccessibilityNodeInfo? = n
                    while (parent != null) {
                        if (parent.isEditable) return parent
                        parent = parent.parent
                    }
                }
                for (i in 0 until n.childCount) findEditable(n.getChild(i))?.let { return it }
                return null
            }
            val input = findEditable(automation.rootInActiveWindow) ?: throw AssertionError("Missing text input")
            assertTrue(input.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "实现消息卡片 ✓") }))
            fun click(label: String) {
                var target: AccessibilityNodeInfo? = await(label)
                while (target != null && !target.isClickable) target = target.parent
                assertTrue("Cannot click $label", target?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true)
                instrumentation.waitForIdleSync(); Thread.sleep(200)
            }
            click("测试环境")
            click("预览环境")
            click("提交")
            instrumentation.waitForIdleSync()
            scenario.onActivity { activity ->
                assertEquals(1, activity.activations.size)
                val (action, values) = activity.activations.single()
                assertEquals("submit", action)
                assertEquals("实现消息卡片 ✓", values["title"])
                assertEquals("preview", values["target"])
                assertEquals("保留现有工作。", values["notes"])
                activity.card = parseInteractionCard(JSONObject(snapshot("completed")))
            }
            await("已完成")
            assertNull(node("提交"))
            assertNotNull(node("实现消息卡片 ✓"))
            capture("completed.png")
        }
    }
}
