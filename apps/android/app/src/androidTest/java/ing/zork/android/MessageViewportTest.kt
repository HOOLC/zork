package ing.zork.android

import android.content.Intent
import android.view.View
import android.view.ViewGroup
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MessageViewportTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private fun settle() { Thread.sleep(400); instrumentation.waitForIdleSync() }
    private fun texts(view: View): List<MessageTextView> = when (view) {
        is MessageTextView -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { texts(view.getChildAt(it)) }
        else -> emptyList()
    }
    private fun launch() = ActivityScenario.launch<MessagePresentationActivity>(
        Intent(instrumentation.targetContext, MessagePresentationActivity::class.java))

    private fun gesture(x: Float, start: Float, end: Float) {
        val down = android.os.SystemClock.uptimeMillis()
        for (step in 0..20) {
            val action = when (step) {
                0 -> android.view.MotionEvent.ACTION_DOWN
                20 -> android.view.MotionEvent.ACTION_UP
                else -> android.view.MotionEvent.ACTION_MOVE
            }
            val event = android.view.MotionEvent.obtain(down, android.os.SystemClock.uptimeMillis(), action,
                x, start + (end - start) * step / 20f, 0)
            assertTrue(instrumentation.uiAutomation.injectInputEvent(event, true)); event.recycle()
            Thread.sleep(16)
        }
    }

    @Test fun realKeyboardFollowsTailButPreservesHistory() {
        launch().use { scenario ->
            fun keyboard(show: Boolean) {
                if (show) {
                    val automation = instrumentation.uiAutomation
                    automation.clearCache()
                    fun editor(node: android.view.accessibility.AccessibilityNodeInfo?): android.view.accessibility.AccessibilityNodeInfo? {
                        if (node == null || node.isEditable || node.contentDescription?.toString() == "消息输入框") return node
                        for (i in 0 until node.childCount) editor(node.getChild(i))?.let { return it }
                        return null
                    }
                    var target = editor(automation.rootInActiveWindow)
                    val deadline = android.os.SystemClock.uptimeMillis() + 5000
                    while (target == null && android.os.SystemClock.uptimeMillis() < deadline) {
                        Thread.sleep(100); automation.clearCache(); target = editor(automation.rootInActiveWindow)
                    }
                    val node = target ?: error("Missing composer")
                    val rect = android.graphics.Rect().also { node.getBoundsInScreen(it) }
                    val down = android.os.SystemClock.uptimeMillis()
                    for (action in listOf(android.view.MotionEvent.ACTION_DOWN, android.view.MotionEvent.ACTION_UP)) {
                        val event = android.view.MotionEvent.obtain(down, android.os.SystemClock.uptimeMillis(), action,
                            rect.exactCenterX(), rect.exactCenterY(), 0)
                        assertTrue(automation.injectInputEvent(event, true)); event.recycle()
                    }
                } else scenario.onActivity { it.window.insetsController!!.hide(android.view.WindowInsets.Type.ime()) }
                var visible = !show
                val deadline = android.os.SystemClock.uptimeMillis() + 5000
                while (visible != show && android.os.SystemClock.uptimeMillis() < deadline) {
                    settle()
                    scenario.onActivity { visible = it.window.decorView.rootWindowInsets.isVisible(android.view.WindowInsets.Type.ime()) }
                }
                assertEquals("IME visibility", show, visible); settle()
            }
            settle(); keyboard(true)
            scenario.onActivity { assertFalse("Keyboard covered tail", it.scroll.canScrollForward) }
            keyboard(false)
            var width = 0; var height = 0
            scenario.onActivity { width = it.window.decorView.width; height = it.window.decorView.height }
            gesture(width * .5f, height * .3f, height * .65f); settle()
            var first = 0; var offset = 0
            scenario.onActivity {
                assertTrue("Gesture did not enter history", it.scroll.canScrollForward)
                first = it.scroll.firstVisibleItemIndex; offset = it.scroll.firstVisibleItemScrollOffset
            }
            keyboard(true)
            scenario.onActivity {
                assertEquals("Keyboard jumped away from history", first, it.scroll.firstVisibleItemIndex)
                assertEquals("Keyboard moved history offset", offset, it.scroll.firstVisibleItemScrollOffset)
            }
            keyboard(false)
        }
    }

    @Test fun viewportShrinkKeepsEveryDrawAtTail() {
        launch().use { scenario ->
            settle()
            for (inset in listOf(80, 160, 250, 160, 0)) {
                scenario.onActivity { it.frames.clear(); it.keyboardInset = inset }
                settle()
                scenario.onActivity {
                    assertTrue("No draw after resize", it.frames.isNotEmpty())
                    assertTrue("Resize painted a hidden tail: ${it.frames}", it.frames.all { frame -> frame.atTail })
                }
            }
        }
    }

    @Test fun keyboardResizeKeepsPreviewTextAndView() {
        launch().use { scenario ->
            settle(); scenario.onActivity { it.appendLong() }; settle()
            lateinit var original: MessageTextView
            var content = ""
            var height = 0
            scenario.onActivity {
                original = texts(it.window.decorView).last { view -> view.text.contains("完整方案") }
                content = original.text.toString(); height = original.height
            }
            for (inset in listOf(80, 160, 250, 0)) {
                scenario.onActivity { it.keyboardInset = inset }; settle()
                scenario.onActivity {
                    val current = texts(it.window.decorView).last { view -> view.text.contains("完整方案") }
                    assertSame("Keyboard recreated the message view", original, current)
                    assertEquals("Keyboard retruncated the message", content, current.text.toString())
                    assertEquals("Keyboard changed preview height", height, current.height)
                }
            }
        }
    }

    @Test fun selectablePreviewHasNoInternalScroll() {
        instrumentation.runOnMainSync {
            val view = MessageTextView(instrumentation.targetContext).apply {
                layoutParams = ViewGroup.LayoutParams(600, ViewGroup.LayoutParams.WRAP_CONTENT)
                setTextIsSelectable(true)
                textSize = 15f
                text = (1..80).joinToString("\n") { "Line $it" }
                retainMessageText()
                preview = MessagePreviewMeasure(160)
            }
            view.measure(View.MeasureSpec.makeMeasureSpec(600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(160, View.MeasureSpec.AT_MOST))
            view.layout(0, 0, view.measuredWidth, view.measuredHeight)
            assertTrue(view.preview!!.more)
            assertTrue("Text selection disabled", view.isTextSelectable)
            view.scrollBy(0, 100)
            assertEquals("Preview scrolled internally", 0, view.scrollY)
            assertFalse("Preview advertises hidden vertical content", view.canScrollVertically(1))
            assertFalse("Clipping left an extra empty line", view.text.endsWith("\n"))
        }
    }
}
