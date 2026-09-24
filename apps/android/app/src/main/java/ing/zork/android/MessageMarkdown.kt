package ing.zork.android

import android.content.Context
import android.graphics.Typeface
import android.text.Layout
import android.text.SpannableString
import android.text.Spanned
import android.text.style.ForegroundColorSpan
import android.text.style.TypefaceSpan
import android.view.ActionMode
import android.view.Gravity
import android.view.Menu
import android.view.MenuItem
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.res.ResourcesCompat
import io.noties.markwon.AbstractMarkwonPlugin
import io.noties.markwon.Markwon
import io.noties.markwon.MarkwonConfiguration
import io.noties.markwon.MarkwonSpansFactory
import io.noties.markwon.MarkwonVisitor
import io.noties.markwon.core.CoreProps
import io.noties.markwon.core.MarkwonTheme
import io.noties.markwon.core.spans.BulletListItemSpan
import io.noties.markwon.core.spans.LinkSpan
import io.noties.markwon.ext.strikethrough.StrikethroughPlugin
import io.noties.markwon.ext.tables.TablePlugin
import io.noties.markwon.ext.tables.TableTheme
import org.commonmark.node.*
import kotlin.math.ceil

internal const val MarkdownInlineCodeColor = 0xFF7C3FA0.toInt()

/** Access-order caches retain parsed structure, never a View or its mutable spans. */
internal class MarkdownBoundedCache<K, V>(private val limit: Int, private val byteLimit: Int) {
    private data class Entry<V>(val value: V, val bytes: Int)
    private val entries = LinkedHashMap<K, Entry<V>>(16, .75f, true)
    var estimatedBytes = 0; private set
    val size get() = entries.size
    @Synchronized fun getOrPut(key: K, bytes: Int, create: () -> V): V {
        entries[key]?.let { return it.value }
        val value = create()
        if (bytes > byteLimit) return value
        while (entries.isNotEmpty() && (entries.size >= limit || estimatedBytes + bytes > byteLimit)) {
            val oldest = entries.entries.iterator().next()
            estimatedBytes -= oldest.value.bytes; entries.remove(oldest.key)
        }
        entries[key] = Entry(value, bytes); estimatedBytes += bytes
        return value
    }
}

internal object MessageMarkdownCache {
    // Renderer spans bake theme colors, so each palette keeps its own renderer.
    private data class Style(val density: Float, val textPixels: Float, val dark: Boolean)
    private val renderers = MarkdownBoundedCache<Style, Markwon>(4, Int.MAX_VALUE)
    private data class Document(val root: Node, val ordinals: List<Pair<OrderedList, Int>>)
    private val documents = MarkdownBoundedCache<String, Document>(384, 8 * 1024 * 1024)
    var parses = 0L; private set
    val entries get() = documents.size
    val estimatedBytes get() = documents.estimatedBytes
    fun renderer(context: Context, density: Float, textPixels: Float): Markwon =
        renderers.getOrPut(Style(density, textPixels, ZorkColors.dark), 1) { createRenderer(context.applicationContext, density, textPixels) }

    fun render(renderer: Markwon, content: String, density: Float, textPixels: Float): Spanned {
        // Conservative source + AST allowance; the cap is an estimate, not just string bytes.
        val weight = (content.length.toLong() * 48 + 512).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
        val node = documents.getOrPut(content, weight) {
            parses++
            val root = renderer.parse(content)
            val ordinals = mutableListOf<Pair<OrderedList, Int>>()
            root.accept(object : AbstractVisitor() {
                override fun visit(orderedList: OrderedList) {
                    ordinals.add(orderedList to orderedList.startNumber); super.visit(orderedList)
                }
            })
            Document(root, ordinals)
        }
        // Table spans contain measured geometry: produce fresh spans for each TextView.
        val text = SpannableString(try { renderer.render(node.root) } finally {
            // Markwon 4.6 increments OrderedList.startNumber while visiting items.
            // Restore parser state even if rendering fails, before this AST is reused.
            node.ordinals.forEach { (list, number) -> list.startNumber = number }
        })
        for (i in 1 until text.length) {
            if (text[i - 1] == '\n' && text[i] == '\n' && text.getSpans(i, i + 1, MarkdownCodePanelSpan::class.java).isEmpty()) {
                text.setSpan(MarkdownGapSpan(ceil(textPixels * .6f).toInt(), textPixels * .4f), i, i + 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            }
        }
        return text
    }

    private fun createRenderer(context: Context, density: Float, textPixels: Float): Markwon {
        fun dp(value: Float) = ceil(value * density).toInt()
        val body = ResourcesCompat.getFont(context, R.font.inter)!!
        val mono = ResourcesCompat.getFont(context, R.font.jetbrains_mono)!!
        val table = TableTheme.emptyBuilder().tableCellPadding(dp(8f)).tableBorderWidth(dp(1f))
            .tableBorderColor(ZorkColors.Border.toArgb()).tableHeaderRowBackgroundColor(ZorkColors.Paper.toArgb())
            .tableOddRowBackgroundColor(ZorkColors.Canvas.toArgb()).tableEvenRowBackgroundColor(ZorkColors.Canvas.toArgb()).build()
        return Markwon.builder(context)
            .usePlugin(io.noties.markwon.SoftBreakAddsNewLinePlugin.create())
            .usePlugin(StrikethroughPlugin.create()).usePlugin(TablePlugin.create(table))
            .usePlugin(object : AbstractMarkwonPlugin() {
                override fun configureTheme(builder: MarkwonTheme.Builder) {
                    builder.linkColor(ZorkColors.Ink.toArgb()).isLinkUnderlined(true)
                        .blockMargin(dp(16f)).blockQuoteWidth(dp(2f)).blockQuoteColor(ZorkColors.FieldBorder.toArgb())
                        .listItemColor(ZorkColors.Muted.toArgb()).bulletWidth(dp(4f)).bulletListItemStrokeWidth(dp(1f))
                        .codeTypeface(mono).codeBlockTypeface(mono).codeTextSize(textPixels.toInt()).codeBlockTextSize(textPixels.toInt())
                        .codeBlockTextColor(ZorkColors.Ink.toArgb()).codeBlockBackgroundColor(ZorkColors.Paper.toArgb()).codeBlockMargin(dp(12f))
                        .headingBreakHeight(0).headingTypeface(Typeface.create(body, 600, false))
                        .headingTextSizeMultipliers(floatArrayOf(20f / 13f, 18f / 13f, 16f / 13f, 14f / 13f, 14f / 13f, 14f / 13f))
                        .thematicBreakColor(ZorkColors.Border.toArgb()).thematicBreakHeight(dp(1f))
                }
                override fun configureConfiguration(builder: MarkwonConfiguration.Builder) {
                    builder.syntaxHighlight { info, code -> MessageCodeColors.highlight(info, code) }
                    builder.linkResolver { view, link ->
                        (view as? MarkdownTextView)?.showLinkActions(link)
                    }
                }
                override fun configureSpansFactory(builder: MarkwonSpansFactory.Builder) {
                    builder.setFactory(Code::class.java) { _, _ -> arrayOf(TypefaceSpan(mono), ForegroundColorSpan(MarkdownInlineCodeColor)) }
                    builder.setFactory(ListItem::class.java) { configuration, props ->
                        if (CoreProps.LIST_ITEM_TYPE.require(props) == CoreProps.ListItemType.ORDERED) {
                            MarkdownNumberSpan(CoreProps.ORDERED_LIST_ITEM_NUMBER.require(props), density, textPixels, mono)
                        } else BulletListItemSpan(configuration.theme(), CoreProps.BULLET_LIST_ITEM_LEVEL.require(props))
                    }
                    builder.setFactory(FencedCodeBlock::class.java) { _, props ->
                        MarkdownCodePanelSpan(CoreProps.CODE_BLOCK_INFO.get(props).orEmpty().trim().substringBefore(' ').take(48), density, textPixels, mono, body)
                    }
                    builder.setFactory(IndentedCodeBlock::class.java) { _, _ -> MarkdownCodePanelSpan("", density, textPixels, mono, body) }
                }
                override fun configureVisitor(builder: MarkwonVisitor.Builder) {
                    builder.on(org.commonmark.ext.gfm.tables.TableBlock::class.java) { visitor, node ->
                        renderRoundedMarkdownTable(visitor, node, table, density)
                    }
                    builder.on(Code::class.java) { visitor, node ->
                        val start = visitor.length(); visitor.builder().append(node.literal)
                        visitor.setSpansForNode(node, start)
                    }
                    builder.on(FencedCodeBlock::class.java) { visitor, node -> appendCode(visitor, node, node.info, node.literal) }
                    builder.on(IndentedCodeBlock::class.java) { visitor, node -> appendCode(visitor, node, "", node.literal) }
                    // Match desktop's readable image fallback; no remote image loads while scrolling.
                    builder.on(Image::class.java) { visitor, node ->
                        val start = visitor.length(); visitor.visitChildren(node)
                        if (visitor.length() == start) visitor.builder().append("图片")
                        if (node.parent !is Link) visitor.setSpans(start, LinkSpan(visitor.configuration().theme(), node.destination, visitor.configuration().linkResolver()))
                    }
                    builder.on(HtmlInline::class.java) { visitor, node -> visitor.builder().append(node.literal) }
                    builder.on(HtmlBlock::class.java) { visitor, node ->
                        visitor.blockStart(node); visitor.builder().append(node.literal.trimEnd()); visitor.blockEnd(node)
                    }
                }
            }).build()
    }
}

private fun appendCode(visitor: MarkwonVisitor, node: Node, info: String?, code: String) {
    visitor.blockStart(node)
    val start = visitor.length()
    // Geometry supplies padding and the language label, not copied placeholder lines.
    visitor.builder().append(if (code.isEmpty()) " " else MessageCodeColors.highlight(info, code))
    CoreProps.CODE_BLOCK_INFO.set(visitor.renderProps(), info.orEmpty())
    visitor.setSpansForNodeOptional(node, start)
    visitor.blockEnd(node)
}

private data class MarkdownBinding(val content: String, val density: Float, val textPixels: Float, val dark: Boolean)

@Composable
internal fun Markdown(content: String, modifier: Modifier = Modifier, preview: MessagePreviewMeasure? = null, onComment: ((String) -> Unit)? = null) {
    val context = LocalContext.current
    val density = LocalDensity.current
    val textPixels = with(density) { 15.sp.toPx() }
    val dark = ZorkColors.dark
    val renderer = remember(context.applicationContext, density.density, textPixels, dark) {
        MessageMarkdownCache.renderer(context, density.density, textPixels)
    }
    AndroidView(modifier = modifier, factory = { ctx ->
        MarkdownTextView(ctx).apply {
            gravity = Gravity.TOP
            typeface = ResourcesCompat.getFont(ctx, R.font.inter)
            includeFontPadding = false; setTextIsSelectable(true)
            breakStrategy = Layout.BREAK_STRATEGY_SIMPLE
            hyphenationFrequency = Layout.HYPHENATION_FREQUENCY_NONE
        }
    }, update = { view ->
        view.preview = preview
        // Read in update so a theme switch recolors views that already exist.
        view.setTextColor(ZorkColors.Ink.toArgb()); view.setLinkTextColor(ZorkColors.Ink.toArgb())
        view.highlightColor = ZorkColors.Selected.toArgb()
        val binding = MarkdownBinding(content, density.density, textPixels, dark)
        if (view.tag != binding) {
            view.dismissLinkActions()
            view.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, textPixels)
            view.setLineSpacing(textPixels * .4f, 1f)
            renderer.setParsedMarkdown(view, MessageMarkdownCache.render(renderer, content, density.density, textPixels))
            view.retainMessageText()
            view.movementMethod = io.noties.markwon.ext.tables.TableAwareMovementMethod.wrap(android.text.method.ArrowKeyMovementMethod.getInstance())
            view.tag = binding
        }
        view.customSelectionActionModeCallback = if (onComment == null) null else object : ActionMode.Callback {
            override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
                menu.add(0, 701, 0, "评论").setShowAsAction(MenuItem.SHOW_AS_ACTION_IF_ROOM); return true
            }
            override fun onPrepareActionMode(mode: ActionMode, menu: Menu) = false
            override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
                if (item.itemId != 701) return false
                val start = view.selectionStart; val end = view.selectionEnd
                if (start >= 0 && end > start) onComment(view.text.subSequence(start, end).toString())
                mode.finish(); return true
            }
            override fun onDestroyActionMode(mode: ActionMode) {}
        }
    })
}
