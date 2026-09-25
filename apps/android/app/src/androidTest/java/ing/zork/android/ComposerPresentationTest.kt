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
class ComposerPresentationTest {
    @Test fun threeLinesPresenceAndReadingAnchor() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        fun editor(): AccessibilityNodeInfo {
            automation.clearCache()
            fun find(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.isEditable) return n
                for (i in 0 until n.childCount) find(n.getChild(i))?.let { return it }
                return null
            }
            return find(automation.rootInActiveWindow) ?: error("Missing composer editor")
        }
        fun textBounds(prefix: String): android.graphics.Rect {
            automation.clearCache()
            fun find(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.isVisibleToUser && (n.text?.toString()?.startsWith(prefix) == true || n.contentDescription?.toString()?.startsWith(prefix) == true)) return n
                for (i in 0 until n.childCount) find(n.getChild(i))?.let { return it }
                return null
            }
            return android.graphics.Rect().also { (find(automation.rootInActiveWindow) ?: error("Missing $prefix")).getBoundsInScreen(it) }
        }
        fun visible(prefix: String): Boolean {
            automation.clearCache()
            fun find(n: AccessibilityNodeInfo?): Boolean {
                if (n == null) return false
                if (n.isVisibleToUser && (n.text?.toString()?.startsWith(prefix) == true || n.contentDescription?.toString()?.startsWith(prefix) == true)) return true
                for (i in 0 until n.childCount) if (find(n.getChild(i))) return true
                return false
            }
            return find(automation.rootInActiveWindow)
        }
        // Activity is the list's last item: after the latest message, clear of the composer.
        fun assertTailVisible() {
            val activity = textBounds("产品会话 ·")
            assertTrue("Activity is not after the last message", textBounds("消息 19").bottom <= activity.top)
            assertTrue("Activity hides behind the composer", activity.bottom <= android.graphics.Rect().also { editor().getBoundsInScreen(it) }.top)
        }
        fun height() = android.graphics.Rect().also { editor().getBoundsInScreen(it) }.height()
        fun settle() { Thread.sleep(750); instrumentation.waitForIdleSync() }
        fun capture(name: String) {
            val folder = File(instrumentation.targetContext.getExternalFilesDir(null), "composer-presentation").apply { mkdirs() }
            automation.takeScreenshot()?.let { bitmap -> File(folder, name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle() }
        }
        ActivityScenario.launch<ComposerPreviewActivity>(Intent(instrumentation.targetContext, ComposerPreviewActivity::class.java)).use { scenario ->
            settle(); capture("idle.png")
            assertFalse("Idle members shown as activity", visible("产品会话 ·"))
            scenario.onActivity { it.draft = "第一行\n第二行\n第三行" }; settle()
            val three = height()
            scenario.onActivity { it.draft = "第一行\n第二行\n第三行\n第四行\n第五行\n第六行" }; settle()
            assertEquals("Editor exceeded three rows", three, height())
            assertTrue("Draft was truncated", editor().text.toString().contains("第六行"))
            capture("three-lines.png")
            scenario.onActivity { it.frames.clear(); it.active = true }
            settle()
            scenario.onActivity {
                assertFalse("Tail hidden by presence", it.scroll.canScrollForward)
                val samples = synchronized(it.frames) { it.frames.sorted() }
                File(instrumentation.targetContext.getExternalFilesDir(null), "composer-presentation/frame-cost.txt").writeText("samples=${samples.size}, p95=${samples[(samples.size * .95).toInt().coerceAtMost(samples.lastIndex)]}ms")
                it.viewportWidth = 320
            }
            settle(); assertTailVisible(); capture("active-320.png")
            val activityTop = textBounds("产品会话 ·").top
            // A real finger scroll exits tail following; a programmatic request
            // intentionally does not represent the user's reading intent.
            val editorBounds = android.graphics.Rect().also { editor().getBoundsInScreen(it) }
            val down = SystemClock.uptimeMillis()
            for (step in 0..20) {
                val action = if (step == 0) android.view.MotionEvent.ACTION_DOWN else if (step == 20) android.view.MotionEvent.ACTION_UP else android.view.MotionEvent.ACTION_MOVE
                val event = android.view.MotionEvent.obtain(down, down + step * 20L, action, editorBounds.exactCenterX(), editorBounds.top - 330f + step * 8f, 0)
                automation.injectInputEvent(event, true); event.recycle()
            }
            settle()
            var first = 0; var offset = 0
            assertTrue("Activity did not scroll with the messages", !visible("产品会话 ·") || textBounds("产品会话 ·").top > activityTop + 100)
            capture("active-scrolled.png")
            scenario.onActivity { assertTrue("Gesture did not leave the tail", it.scroll.canScrollForward); first = it.scroll.firstVisibleItemIndex; offset = it.scroll.firstVisibleItemScrollOffset; it.active = false }
            Thread.sleep(1900); instrumentation.waitForIdleSync()
            assertFalse("Activity stayed after the round ended", visible("产品会话 ·"))
            scenario.onActivity { assertEquals(first, it.scroll.firstVisibleItemIndex); assertEquals(offset, it.scroll.firstVisibleItemScrollOffset); it.active = true }
            // Appearing while the reader is away from the tail does not move them.
            settle()
            scenario.onActivity { assertEquals(first, it.scroll.firstVisibleItemIndex); assertEquals(offset, it.scroll.firstVisibleItemScrollOffset); it.fontScale = 1.5f }
            settle(); capture("large-font.png")
        }
    }
}
