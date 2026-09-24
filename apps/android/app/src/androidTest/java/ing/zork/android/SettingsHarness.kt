package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Rect
import android.os.Bundle
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue

/** Accessibility-driven helpers for the settings fixture (Nav7PreviewActivity). */
internal class SettingsHarness(private val folder: String) {
    val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation

    fun launch(page: String): ActivityScenario<Nav7PreviewActivity> = ActivityScenario.launch(
        Intent(instrumentation.targetContext, Nav7PreviewActivity::class.java).putExtra("screen", page).putExtra("width", 0))

    fun settle() { instrumentation.waitForIdleSync(); Thread.sleep(300) }

    fun nodes(): List<AccessibilityNodeInfo> {
        automation.clearCache()
        val result = mutableListOf<AccessibilityNodeInfo>()
        fun walk(node: AccessibilityNodeInfo?) {
            if (node == null) return
            result += node
            for (i in 0 until node.childCount) walk(node.getChild(i))
        }
        walk(automation.rootInActiveWindow)
        return result
    }

    private fun label(node: AccessibilityNodeInfo) = listOfNotNull(node.contentDescription?.toString(), node.text?.toString())

    fun find(label: String): AccessibilityNodeInfo? = nodes().filter { it.isVisibleToUser && label in label(it) }.let { matches ->
        matches.firstOrNull { it.isClickable } ?: matches.firstOrNull { it.contentDescription?.toString() == label } ?: matches.firstOrNull()
    }

    fun findPrefix(prefix: String): AccessibilityNodeInfo? =
        nodes().filter { it.isVisibleToUser && label(it).any { l -> l.startsWith(prefix) } }.let { m -> m.firstOrNull { it.isClickable } ?: m.firstOrNull() }

    fun has(label: String) = find(label) != null
    fun hasPrefix(prefix: String) = findPrefix(prefix) != null

    /** Waits for [label] to appear, then looks further down (and up) the scrolled sheet. */
    fun await(label: String): AccessibilityNodeInfo {
        val deadline = SystemClock.uptimeMillis() + 2500
        while (SystemClock.uptimeMillis() < deadline) { find(label)?.let { return it }; Thread.sleep(80) }
        return reveal({ find(label) }, label)
    }

    fun awaitPrefix(prefix: String): AccessibilityNodeInfo {
        val deadline = SystemClock.uptimeMillis() + 2500
        while (SystemClock.uptimeMillis() < deadline) { findPrefix(prefix)?.let { return it }; Thread.sleep(80) }
        return reveal({ findPrefix(prefix) }, "$prefix…")
    }

    fun awaitGone(label: String) {
        val deadline = SystemClock.uptimeMillis() + 5000
        while (SystemClock.uptimeMillis() < deadline) { if (find(label) == null) return; Thread.sleep(80) }
        error("Still showing $label")
    }

    fun dump() = nodes().filter { it.isVisibleToUser }.joinToString("\n") {
        "${it.className} text=${it.text} desc=${it.contentDescription} click=${it.isClickable} enabled=${it.isEnabled} checked=${it.isChecked} selected=${it.isSelected} state=${it.stateDescription} focused=${it.isFocused}"
    }

    /** The outermost scroller: the sheet (or page), not a horizontal chip row inside it. */
    private fun scrollable() = nodes().firstOrNull { it.isScrollable && it.isVisibleToUser }

    private fun reveal(match: () -> AccessibilityNodeInfo?, name: String): AccessibilityNodeInfo {
        repeat(3) { match()?.let { return it }; Thread.sleep(150) }
        repeat(8) {
            match()?.let { return it }
            scrollable()?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD); settle()
        }
        repeat(10) {
            match()?.let { return it }
            scrollable()?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD); settle()
        }
        return match() ?: error("Missing $name\n" + dump())
    }

    private fun clickNode(start: AccessibilityNodeInfo?, name: String) {
        var node = start
        while (node != null) {
            if (node.isClickable) {
                assertTrue("$name is disabled", node.isEnabled)
                assertTrue("Cannot click $name", node.performAction(AccessibilityNodeInfo.ACTION_CLICK))
                settle(); return
            }
            node = node.parent
        }
        error("Cannot click $name")
    }

    fun click(label: String) { reveal({ find(label) }, label); clickNode(find(label), label) }
    fun clickPrefix(prefix: String) { reveal({ findPrefix(prefix) }, prefix); clickNode(findPrefix(prefix), prefix) }
    fun reveal(label: String) = reveal({ find(label) }, label)

    fun isEnabled(label: String): Boolean {
        var node = reveal(label)
        while (true) { if (node.isClickable) return node.isEnabled; node = node.parent ?: return node.isEnabled }
    }

    fun editable(label: String): AccessibilityNodeInfo? {
        for (node in nodes().filter { it.contentDescription?.toString() == label }) {
            var parent: AccessibilityNodeInfo? = node
            while (parent != null) {
                if (parent.isEditable) return parent
                parent = parent.parent
            }
            // Compose may put the description on the edit text's own child.
            if (node.isEditable) return node
        }
        return null
    }

    fun type(label: String, value: String) {
        val field = reveal({ editable(label) }, label)
        assertTrue(field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
            putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value)
        }))
        settle()
        assertEquals("Edited the wrong field: $label", value, editable(label)?.text?.toString())
    }

    fun focus(label: String) {
        val field = reveal({ editable(label) }, label)
        field.performAction(AccessibilityNodeInfo.ACTION_CLICK); field.performAction(AccessibilityNodeInfo.ACTION_FOCUS); settle()
    }

    /** The IME action (Done) of a text field. */
    fun imeDone(label: String) {
        val field = reveal({ editable(label) }, label)
        assertTrue("No IME action on $label", field.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id))
        settle()
    }

    fun text(label: String) = reveal({ editable(label) }, label).text?.toString()

    /** A radio or selectable chip is on (Compose reports radios as checked). */
    fun isOn(label: String) = reveal(label).let { it.isChecked || it.isSelected }

    fun assertOn(label: String) {
        var node: AccessibilityNodeInfo? = reveal(label)
        val states = mutableListOf<String>()
        while (node != null) {
            if (node.isChecked || node.isSelected || node.stateDescription?.toString() == "已选中") return
            states += "checkable=${node.isCheckable} checked=${node.isChecked} selected=${node.isSelected} state=${node.stateDescription}"
            node = node.parent
        }
        error("$label is not on: $states\n" + dump())
    }

    /** Scrolls [label] fully into view (Compose brings the node into its scroller's viewport). */
    fun showOnScreen(label: String) {
        reveal(label).performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SHOW_ON_SCREEN.id); settle()
    }

    fun bounds(label: String) = Rect().also { reveal(label).getBoundsInScreen(it) }

    /** Long-press [from], drag it onto [to], release. [dx] shifts both points (px), e.g. from a chip's × onto its name. */
    fun longPressDrag(from: String, to: String, dx: Int = 0) {
        reveal(from); reveal(to); settle()
        // Both must be fully on screen, measured after scrolling has stopped.
        val a = bounds(from).apply { offset(dx, 0) }; val b = bounds(to).apply { offset(dx, 0) }
        assertTrue("$from is clipped: $a", a.height() >= 40); assertTrue("$to is clipped: $b", b.height() >= 40)
        val start = SystemClock.uptimeMillis()
        fun send(action: Int, x: Float, y: Float, at: Long) {
            val event = MotionEvent.obtain(start, at, action, x, y, 0).apply { source = InputDevice.SOURCE_TOUCHSCREEN }
            automation.injectInputEvent(event, true); event.recycle()
        }
        send(MotionEvent.ACTION_DOWN, a.exactCenterX(), a.exactCenterY(), start)
        Thread.sleep(800)
        val steps = 20
        for (i in 1..steps) {
            val x = a.exactCenterX() + (b.exactCenterX() - a.exactCenterX()) * i / steps
            val y = a.exactCenterY() + (b.exactCenterY() - a.exactCenterY()) * i / steps
            send(MotionEvent.ACTION_MOVE, x, y, SystemClock.uptimeMillis()); Thread.sleep(25)
        }
        Thread.sleep(150)
        send(MotionEvent.ACTION_UP, b.exactCenterX(), b.exactCenterY(), SystemClock.uptimeMillis())
        settle()
    }

    fun back() { instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK); settle() }

    /** Which editable has input focus, and whether the keyboard is up (for logs). */
    fun focusState(): String {
        val focused = nodes().filter { it.isFocused && it.isEditable }.joinToString { n ->
            var p: AccessibilityNodeInfo? = n; var d: String? = null
            while (p != null && d == null) { d = p.contentDescription?.toString(); p = p.parent }
            d ?: n.text?.toString().orEmpty()
        }
        val ime = automation.windows.any { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_INPUT_METHOD }
        return "focused=[$focused] ime=$ime"
    }

    fun capture(name: String) {
        android.util.Log.i("ModelEditorTest", "$name ${focusState()}")
        val dir = instrumentation.targetContext.filesDir.resolve(folder).apply { mkdirs() }
        Thread.sleep(350)
        automation.takeScreenshot()?.let { bitmap ->
            dir.resolve("$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle()
        }
    }
}
