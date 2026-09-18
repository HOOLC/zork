package ing.zork.android

import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.os.Bundle
import android.text.Spanned
import android.view.View
import android.view.ViewGroup
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.TextView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LiteralUserMessageTest {
    @Test fun userTextAndCopyRemainLiteralWhileAssistantKeepsMarkdown() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        fun texts(view: View): List<TextView> = when (view) {
            is TextView -> listOf(view)
            is ViewGroup -> (0 until view.childCount).flatMap { texts(view.getChildAt(it)) }
            else -> emptyList()
        }
        ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java)
            .putExtra("screen", "literal-user").putExtra("width", 0)).use { scenario ->
            instrumentation.waitForIdleSync(); Thread.sleep(500)
            repeat(2) { pass ->
                scenario.onActivity { activity ->
                    val views = texts(activity.window.decorView)
                    val user = views.single { it.text.toString() == literalUserFixture }
                    assertTrue(user.isTextSelectable)
                    assertNotNull(user.customSelectionActionModeCallback)
                    val spans = (user.text as? Spanned)?.getSpans(0, user.text.length, Any::class.java).orEmpty()
                    assertFalse(spans.any { it.javaClass.name.startsWith("io.noties.markwon") })
                    assertTrue(views.any { it.text.toString() == "助手仍用 Markdown" })
                    if (pass == 0) {
                        val range = Bundle().apply {
                            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 0)
                            putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, user.text.length)
                        }
                        assertTrue(user.performAccessibilityAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, range))
                        assertTrue(user.performAccessibilityAction(AccessibilityNodeInfo.ACTION_COPY, null))
                        val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                        assertEquals(literalUserFixture, clipboard.primaryClip!!.getItemAt(0).text.toString())
                    }
                }
                if (pass == 0) { scenario.recreate(); instrumentation.waitForIdleSync(); Thread.sleep(500) }
            }
            Thread.sleep(8000) // Let the system clipboard preview dismiss before visual evidence.
            val screenshot = instrumentation.uiAutomation.takeScreenshot()
            val folder = context.filesDir.resolve("literal-user").apply { mkdirs() }
            folder.resolve("conversation.png").outputStream().use { screenshot.compress(Bitmap.CompressFormat.PNG, 100, it) }
            screenshot.recycle()
        }
    }
}
