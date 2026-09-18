package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import android.content.ClipboardManager
import android.content.Context
import android.os.Bundle
import android.text.Spannable
import android.text.Spanned
import android.text.Selection
import android.text.TextPaint
import android.text.style.ForegroundColorSpan
import android.text.style.MetricAffectingSpan
import android.view.View
import android.view.ViewGroup
import android.view.ActionMode
import android.view.accessibility.AccessibilityNodeInfo
import android.widget.TextView
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MarkdownRenderingTest {
    @Test fun captureMarkdownFixtures() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val phase = InstrumentationRegistry.getArguments().getString("phase") ?: "current"
        val folder = context.filesDir.resolve("markdown-$phase").apply { mkdirs() }
        for (name in markdownFixtures.keys) {
            for (width in listOf(320, 390, 768)) {
                ActivityScenario.launch<MarkdownPreviewActivity>(Intent(context, MarkdownPreviewActivity::class.java)
                    .putExtra("case", name).putExtra("width", width)).use { scenario ->
                    instrumentation.waitForIdleSync(); Thread.sleep(650)
                    var bounds = android.graphics.Rect()
                    scenario.onActivity { bounds = it.contentBounds }
                    val screenshot = instrumentation.uiAutomation.takeScreenshot()
                    val crop = Bitmap.createBitmap(screenshot, bounds.left, bounds.top, bounds.width(), bounds.height())
                    folder.resolve("$name-$width.png").outputStream().use { crop.compress(Bitmap.CompressFormat.PNG, 100, it) }
                    crop.recycle(); screenshot.recycle()
                }
            }
        }
    }
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private fun settled() { instrumentation.waitForIdleSync(); Thread.sleep(180); instrumentation.waitForIdleSync() }
    private fun textViews(view: View): List<TextView> = when (view) {
        is TextView -> listOf(view)
        is ViewGroup -> (0 until view.childCount).flatMap { textViews(view.getChildAt(it)) }
        else -> emptyList()
    }
    private fun markdown(activity: MarkdownPreviewActivity) = textViews(activity.window.decorView).single { it.isTextSelectable }

    @Test fun fontsListsResizeAndEditsPreserveSelection() {
        val context = instrumentation.targetContext
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(context, MarkdownPreviewActivity::class.java)
            .putExtra("case", "typography").putExtra("width", 320)).use { scenario ->
            settled()
            var lines = 0; var parses = 0L
            fun verify(view: TextView, expectedSize: Float) {
                assertEquals(expectedSize, view.textSize, .1f)
                val spanned = view.text as Spanned
                val start = spanned.toString().indexOf("inline_code")
                assertTrue(start >= 0)
                assertTrue(spanned.getSpans(start, start + 11, ForegroundColorSpan::class.java).any { it.foregroundColor == MarkdownInlineCodeColor })
                val codePaint = TextPaint(view.paint)
                spanned.getSpans(start, start + 11, MetricAffectingSpan::class.java).forEach { it.updateMeasureState(codePaint) }
                assertEquals(codePaint.measureText("iiii"), codePaint.measureText("WWWW"), .1f)
                assertTrue(kotlin.math.abs(view.paint.measureText("iiii") - view.paint.measureText("WWWW")) > 1f)
                assertEquals(listOf(9, 10), spanned.getSpans(0, spanned.length, MarkdownNumberSpan::class.java).sortedBy { spanned.getSpanStart(it) }.map { it.number })
                assertTrue(view.isTextSelectable)
                assertNotNull(view.customSelectionActionModeCallback)
            }
            scenario.onActivity { a ->
                val view = markdown(a); verify(view, 15f)
                lines = view.lineCount; parses = MessageMarkdownCache.parses; a.width = 768
            }
            settled()
            scenario.onActivity { a ->
                val view = markdown(a); verify(view, 15f)
                assertTrue("Text did not reflow at a wider width", view.lineCount < lines)
                assertEquals("Width-only changes must reuse the parsed document", parses, MessageMarkdownCache.parses)
                a.fontScale = 1.4f
            }
            settled()
            scenario.onActivity { a ->
                verify(markdown(a), 21f)
                assertEquals("Font changes must not reparse Markdown", parses, MessageMarkdownCache.parses)
                a.content = "已更新：中文🐈 Café\n第二行"
            }
            settled()
            scenario.onActivity { a ->
                val view = markdown(a)
                assertEquals("已更新：中文🐈 Café\n第二行", view.text.toString())
                val range = Bundle().apply {
                    putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 0)
                    putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, view.text.length)
                }
                assertTrue(view.performAccessibilityAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, range))
                assertTrue(view.onTextContextMenuItem(android.R.id.copy))
                val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                assertEquals(view.text.toString(), clipboard.primaryClip!!.getItemAt(0).text.toString())
            }
            scenario.recreate(); settled()
            scenario.onActivity { verify(markdown(it), 15f) }
        }
    }

    @Test fun codeAndCommentsKeepExactTextAcrossBlocks() {
        val context = instrumentation.targetContext
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(context, MarkdownPreviewActivity::class.java)
            .putExtra("case", "code")).use { scenario ->
            settled()
            scenario.onActivity { a ->
                val view = markdown(a); val text = view.text as Spanned
                assertEquals(listOf("kotlin", "rust"), text.getSpans(0, text.length, MarkdownCodePanelSpan::class.java).sortedBy { text.getSpanStart(it) }.map { it.language })
                assertTrue(text.getSpans(0, text.length, ForegroundColorSpan::class.java).map { it.foregroundColor }.distinct().size >= 4)
                assertFalse("Language labels must not be copied as message text", text.toString().contains("kotlin"))
                assertFalse("Layout padding must not introduce nonbreaking-space lines", text.toString().contains('\u00a0'))
                val start = text.toString().indexOf("中文注释")
                val end = text.toString().indexOf("代码后的段落") + "代码后的段落".length
                val expected = text.subSequence(start, end).toString()
                assertTrue(start >= 0 && end > start)
                Selection.setSelection(view.text as Spannable, start, end)
                val callback = view.customSelectionActionModeCallback!!
                val mode = view.startActionMode(callback, ActionMode.TYPE_FLOATING)!!
                val item = mode.menu.findItem(701)
                assertNotNull(item)
                assertTrue(callback.onActionItemClicked(mode, item))
                assertEquals(expected, a.lastQuote)
            }
        }
    }

    @Test fun parsedListsAndCodeAreBoundedAndRepeatable() {
        val context = instrumentation.targetContext
        instrumentation.runOnMainSync {
            val renderer = MessageMarkdownCache.renderer(context, 1f, 15f)
            repeat(4) {
                val text = MessageMarkdownCache.render(renderer, "9. first\n10. second\n\n    3. nested\n    4. next", 1f, 15f)
                assertEquals(listOf(9, 10, 3, 4).sorted(), text.getSpans(0, text.length, MarkdownNumberSpan::class.java).map { it.number }.sorted())
            }
            repeat(700) { MessageMarkdownCache.render(renderer, "文档 $it\n\n**中文🐈**", 1f, 15f) }
            assertTrue(MessageMarkdownCache.entries <= 384)
            assertTrue(MessageMarkdownCache.estimatedBytes <= 8 * 1024 * 1024)
            for (language in listOf("rust", "kotlin", "python", "javascript", "json", "shell", "unknown")) {
                val code = "value = \"中文🐈\"; // comment\n"
                assertEquals(code, MessageCodeColors.highlight(language, code).toString())
            }
            val duplicate = MessageMarkdownCache.render(renderer, "```rust\nlet x = 1;\n```\n\n```rust\nlet x = 1;\n```", 1f, 15f)
            val blocks = duplicate.getSpans(0, duplicate.length, MarkdownCodePanelSpan::class.java)
            assertEquals(2, blocks.size)
            blocks.forEach {
                assertTrue("Identical code blocks must retain their own colors", duplicate.getSpans(duplicate.getSpanStart(it), duplicate.getSpanEnd(it), ForegroundColorSpan::class.java).size >= 2)
            }
            val long = "x".repeat(5000)
            assertEquals(long, MessageCodeColors.highlight("rust", long).toString())
            assertFalse(MessageCodeColors.highlight("rust", long) is Spanned)
            val untagged = MessageMarkdownCache.render(renderer, "```\nplain 中文🐈\n```", 1f, 15f)
            assertTrue(untagged.toString().contains("plain 中文🐈"))
            val fallback = MessageMarkdownCache.render(renderer, "![图片说明](https://example.com/photo.png)\n\n<span>原文</span>", 1f, 15f)
            assertTrue(fallback.toString().contains("图片说明")); assertTrue(fallback.toString().contains("<span>原文</span>"))
        }
    }


    private fun action(label: String): AccessibilityNodeInfo? {
        instrumentation.uiAutomation.clearCache()
        fun find(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
            if (node == null) return null
            if (node.text?.toString() == label) return node
            for (index in 0 until node.childCount) find(node.getChild(index))?.let { return it }
            return null
        }
        return find(instrumentation.uiAutomation.rootInActiveWindow)
    }

    @Test fun unsupportedSchemesOfferCopyWithoutOpening() {
        val context = instrumentation.targetContext
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(context, MarkdownPreviewActivity::class.java)).use { scenario ->
            scenario.onActivity { it.captureLinks = true; it.content = "[保留链接文本](javascript:alert)" }
            settled(); Thread.sleep(700)
            scenario.onActivity { a ->
                val view = markdown(a); val text = view.text as Spanned
                text.getSpans(0, text.length, android.text.style.ClickableSpan::class.java).single().onClick(view)
            }
            val deadline = android.os.SystemClock.uptimeMillis() + 3000
            while (action("复制链接") == null && android.os.SystemClock.uptimeMillis() < deadline) Thread.sleep(30)
            assertNotNull(action("复制链接"))
            assertFalse("Unsupported URLs must not have an enabled open action", action("打开链接")!!.isEnabled)
            scenario.onActivity { assertTrue(it.requestedLinks.isEmpty()) }
            instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_BACK)
        }
    }

    @Test fun tablesKeepRoundedEdgesAndReflowAtLargeTextSizes() {
        val context = instrumentation.targetContext
        ActivityScenario.launch<MarkdownPreviewActivity>(Intent(context, MarkdownPreviewActivity::class.java)
            .putExtra("case", "table").putExtra("width", 320)).use { scenario ->
            settled()
            var narrowCell = 0
            repeat(2) { pass ->
                scenario.onActivity { a ->
                    val view = markdown(a); val text = view.text as Spanned
                    val rows = text.getSpans(0, text.length, RoundedMarkdownTableRow::class.java).sortedBy { text.getSpanStart(it) }
                    assertEquals(4, rows.size)
                    assertTrue(rows.first().cellWidth() > 0)
                    if (pass == 0) narrowCell = rows.first().cellWidth() else assertTrue(rows.first().cellWidth() > narrowCell)
                    val bitmap = Bitmap.createBitmap(view.width, view.height, Bitmap.Config.ARGB_8888)
                    val canvas = android.graphics.Canvas(bitmap); canvas.drawColor(android.graphics.Color.WHITE); view.draw(canvas)
                    val top = view.layout.getLineTop(view.layout.getLineForOffset(text.getSpanStart(rows.first())))
                    assertEquals("A rounded corner should reveal the canvas", android.graphics.Color.WHITE, bitmap.getPixel(0, top))
                    assertNotEquals("The middle of the header must retain its fill", android.graphics.Color.WHITE, bitmap.getPixel(view.width / 2, top + 3))
                    context.filesDir.resolve("table-corners-$pass.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
                    bitmap.recycle()
                    if (pass == 0) { a.width = 768; a.fontScale = 1.4f }
                }
                settled()
            }
        }
    }
}
