package ing.zork.android

import android.content.Intent
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class MessagePresentationTest {
    @Test fun longMessagesShowInFullAndTailAnimates() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        fun node(label: String): AccessibilityNodeInfo? {
            automation.clearCache()
            fun find(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.isVisibleToUser && (n.text?.toString() == label || n.contentDescription?.toString() == label)) return n
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
            val folder = File(instrumentation.targetContext.filesDir, "message-presentation").apply { mkdirs() }
            automation.takeScreenshot()?.let { bitmap -> File(folder, name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle() }
        }
        ActivityScenario.launch<MessagePresentationActivity>(Intent(instrumentation.targetContext, MessagePresentationActivity::class.java)).use { scenario ->
            Thread.sleep(500); instrumentation.waitForIdleSync()
            scenario.onActivity { it.frames.clear(); it.append() }
            Thread.sleep(500); instrumentation.waitForIdleSync()
            scenario.onActivity { activity ->
                val frames = activity.frames.filter { it.count == 14 }
                assertTrue("Tail did not settle", frames.last().atTail)
                if (android.animation.ValueAnimator.areAnimatorsEnabled()) assertTrue("New message jumped instead of scrolling", frames.map { it.first to it.offset }.distinct().size > 1)
            }
            scenario.onActivity { it.appendLong() }
            Thread.sleep(500); instrumentation.waitForIdleSync()
            // Messages are never folded: the end of a long body is in the list itself.
            await("FULL-MESSAGE-END")
            assertNull("Folding entry must not exist", node("查看完整消息"))
            capture("long-message.png")
        }
    }
}
