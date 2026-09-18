package ing.zork.android

import android.app.Activity
import android.app.Instrumentation
import android.content.ClipboardManager
import android.content.Intent
import android.os.SystemClock
import android.text.Spanned
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.noties.markwon.core.spans.LinkSpan
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MessageLinkActionsTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val url = "https://example.com/docs?q=zork#section"
    private fun textView(view: View): MarkdownTextView? {
        if (view is MarkdownTextView) return view
        if (view is ViewGroup) for (i in 0 until view.childCount) textView(view.getChildAt(i))?.let { return it }
        return null
    }
    private fun find(label: String): AccessibilityNodeInfo? {
        instrumentation.uiAutomation.clearCache()
        fun walk(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            if (node == null) return null
            if (node.text?.toString() == label) return node
            for (i in 0 until node.childCount) walk(node.getChild(i))?.let { return it }
            return null
        }
        return walk(instrumentation.uiAutomation.rootInActiveWindow)
    }
    private fun dismissFullscreenHint() {
        Thread.sleep(700)
        // A fresh emulator shows Android's one-time fullscreen tutorial over the fixture.
        find("Got it")?.takeIf { it.packageName == "com.android.systemui" }?.let {
            it.performAction(AccessibilityNodeInfo.ACTION_CLICK)
            instrumentation.waitForIdleSync(); Thread.sleep(350)
        }
    }
    private fun waitFor(label: String): AccessibilityNodeInfo {
        val deadline = SystemClock.uptimeMillis() + 3000
        do { find(label)?.let { return it }; Thread.sleep(30) } while (SystemClock.uptimeMillis() < deadline)
        instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
            instrumentation.targetContext.filesDir.resolve("link-actions-failure.png").outputStream().use {
                bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG,100,it)
            };bitmap.recycle()
        }
        val nodes = mutableListOf<String>()
        fun dump(node: AccessibilityNodeInfo?) { if (node == null) return; nodes.add(node.toString()); for (i in 0 until node.childCount) dump(node.getChild(i)) }
        dump(instrumentation.uiAutomation.rootInActiveWindow)
        instrumentation.targetContext.filesDir.resolve("link-actions-failure.txt").writeText(nodes.joinToString("\n"))
        throw AssertionError("Missing action: $label")
    }
    private fun tap(x: Float, y: Float, hold: Long = 40) {
        val now = SystemClock.uptimeMillis()
        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
            if (action == MotionEvent.ACTION_UP && hold > 0) Thread.sleep(hold)
            val event = MotionEvent.obtain(now, SystemClock.uptimeMillis(), action, x, y, 0)
            instrumentation.sendPointerSync(event); event.recycle()
        }
        instrumentation.waitForIdleSync()
    }
    private fun tapLink(scenario: ActivityScenario<MarkdownPreviewActivity>, hold: Long = 40) {
        var x = 0f; var y = 0f
        scenario.onActivity {
            val view = textView(it.window.decorView)!!
            val text = view.text as Spanned
            val span = text.getSpans(0, text.length, LinkSpan::class.java).first()
            val offset = (text.getSpanStart(span) + text.getSpanEnd(span)) / 2
            val line = view.layout.getLineForOffset(offset)
            val origin = IntArray(2); view.getLocationOnScreen(origin)
            x = origin[0] + view.totalPaddingLeft + view.layout.getPrimaryHorizontal(offset) + 2f
            y = origin[1] + view.totalPaddingTop + (view.layout.getLineTop(line) + view.layout.getLineBottom(line)) / 2f
        }
        tap(x, y, hold)
        scenario.onActivity {
            val view = textView(it.window.decorView)!!
            fun field(name: String): Any? = MarkdownTextView::class.java.getDeclaredField(name).apply { isAccessible = true }.get(view)
            val popup = field("linkPopup") as? android.widget.PopupWindow
            instrumentation.targetContext.filesDir.resolve("link-tap.json").writeText(org.json.JSONObject()
                .put("x",x).put("y",y).put("touch_x",field("touchX")).put("touch_y",field("touchY"))
                .put("touch_time",field("touchTime")).put("popup_showing",popup?.isShowing ?: false).toString())
        }
    }

    @Test fun tapOffersCopyAndExplicitOpenWithoutNavigating() {
        val opened = mutableListOf<String?>()
        val monitor = object : Instrumentation.ActivityMonitor() {
            override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? {
                if (intent.action != Intent.ACTION_VIEW) return null
                opened.add(intent.dataString)
                return Instrumentation.ActivityResult(Activity.RESULT_CANCELED, null)
            }
        }
        instrumentation.addMonitor(monitor)
        try {
            ActivityScenario.launch<MarkdownPreviewActivity>(Intent(instrumentation.targetContext, MarkdownPreviewActivity::class.java)).use { scenario ->
                scenario.onActivity { it.content = "# 链接操作\n\n正文可以长按选择。\n\n文字里的 [文档链接]($url)，点击后再选择操作。" }
                instrumentation.waitForIdleSync(); Thread.sleep(350); dismissFullscreenHint()
                tapLink(scenario)
                waitFor("复制链接"); waitFor("打开链接")
                assertTrue("Tapping a link must not open it", opened.isEmpty())
                Thread.sleep(180) // Wait for the popup surface to be presented before capture.
                instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                    instrumentation.targetContext.filesDir.resolve("link-actions.png").outputStream().use {
                        bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                    }; bitmap.recycle()
                }
                assertTrue(waitFor("复制链接").performAction(AccessibilityNodeInfo.ACTION_CLICK))
                instrumentation.waitForIdleSync(); Thread.sleep(150)
                scenario.onActivity {
                    assertEquals(url, it.getSystemService(ClipboardManager::class.java).primaryClip?.getItemAt(0)?.text?.toString())
                }
                assertNull(find("复制链接")); assertTrue(opened.isEmpty())
                tapLink(scenario)
                assertTrue(waitFor("打开链接").performAction(AccessibilityNodeInfo.ACTION_CLICK))
                instrumentation.waitForIdleSync(); Thread.sleep(150)
                assertEquals(listOf(url), opened)
                assertNull(find("打开链接"))
                tapLink(scenario); waitFor("复制链接")
                instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
                instrumentation.waitForIdleSync(); assertNull(find("复制链接"))
                tapLink(scenario); waitFor("复制链接")
                tap(500f, 1000f)
                assertNull(find("复制链接"))
                assertEquals(listOf(url), opened)
            }
        } finally { instrumentation.removeMonitor(monitor) }
    }

    @Test fun popupClosesWhenMessageIsRebound() {
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(instrumentation.targetContext, MarkdownPreviewActivity::class.java)).use { scenario ->
            scenario.onActivity { it.content = "# 链接操作\n\n正文可以长按选择。\n\n[文档链接]($url)" }
            instrumentation.waitForIdleSync(); Thread.sleep(300); dismissFullscreenHint()
            // Accessibility/span activation has no touch coordinates.
            scenario.onActivity {
                val view = textView(it.window.decorView)!!
                val text = view.text as Spanned
                text.getSpans(0, text.length, LinkSpan::class.java).first().onClick(view)
            }
            waitFor("复制链接")
            scenario.onActivity { it.content = "已更新的消息" }
            instrumentation.waitForIdleSync(); Thread.sleep(150)
            assertNull(find("复制链接"))
        }
    }
    @Test fun longPressStillSelectsText() {
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(instrumentation.targetContext, MarkdownPreviewActivity::class.java)).use { scenario ->
            scenario.onActivity { it.content = "# 链接操作\n\n正文可以长按选择。\n\n[文档链接]($url)" }
            instrumentation.waitForIdleSync(); Thread.sleep(350); dismissFullscreenHint()
            tapLink(scenario, android.view.ViewConfiguration.getLongPressTimeout().toLong() + 150)
            assertNull(find("复制链接"))
            scenario.onActivity {
                val view = textView(it.window.decorView)!!
                assertTrue("Long-press selection must remain available", view.selectionStart >= 0 && view.selectionEnd > view.selectionStart)
                assertTrue(view.text.subSequence(view.selectionStart, view.selectionEnd).contains("文档"))
            }
        }
    }

    @Test fun captureNativePreview() {
        org.junit.Assume.assumeTrue(InstrumentationRegistry.getArguments().getString("native_link_preview") == "true")
        val metrics = instrumentation.targetContext.resources.displayMetrics
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(instrumentation.targetContext, MarkdownPreviewActivity::class.java)
            .putExtra("width", metrics.widthPixels).putExtra("fontScale", metrics.scaledDensity)).use { scenario ->
            scenario.onActivity { it.content = "# 链接操作\n\n点击消息里的链接，选择要执行的操作。\n\n[帮助文档]($url)" }
            instrumentation.waitForIdleSync(); Thread.sleep(350); dismissFullscreenHint()
            tapLink(scenario)
            waitFor("复制链接"); waitFor("打开链接"); Thread.sleep(200)
            instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                instrumentation.targetContext.filesDir.resolve("link-actions-native.png").outputStream().use {
                    bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG,100,it)
                };bitmap.recycle()
            }
            instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
        }
    }

}
