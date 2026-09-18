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
    @Test fun previewReaderAndAnimatedTail() {
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
            await("查看完整消息")
            assertNull("Full body leaked into message list", node("FULL-MESSAGE-END"))
            capture("preview.png")
            var before = 0
            scenario.onActivity { before = it.scroll.firstVisibleItemIndex }
            val target = await("查看完整消息")
            val bounds = android.graphics.Rect().also { target.getBoundsInScreen(it) }
            val down = SystemClock.uptimeMillis()
            for (action in listOf(android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP)) {
                val event = android.view.MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, bounds.exactCenterX(), bounds.exactCenterY(), 0)
                assertTrue(automation.injectInputEvent(event, true))
                event.recycle()
            }
            await("完整消息"); Thread.sleep(300); capture("full-message.png")
            assertNotNull(node("复制全文"))
            assertTrue(await("返回对话").performAction(AccessibilityNodeInfo.ACTION_CLICK))
            Thread.sleep(350); instrumentation.waitForIdleSync()
            scenario.onActivity { assertEquals("Returning moved reading anchor", before, it.scroll.firstVisibleItemIndex) }
            capture("returned.png")
        }
    }
}
