package ing.zork.android

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import androidx.compose.ui.graphics.toArgb
import io.noties.markwon.MarkwonVisitor
import io.noties.markwon.ext.tables.TableRowSpan
import io.noties.markwon.ext.tables.TableTheme
import io.noties.markwon.utils.SpanUtils
import org.commonmark.ext.gfm.tables.TableBlock
import org.commonmark.ext.gfm.tables.TableCell
import org.commonmark.node.Node

/** Retains Markwon's cell layout, alignment, links and resize invalidation. */
internal class RoundedMarkdownTableRow(
    theme: TableTheme, cells: List<TableRowSpan.Cell>, header: Boolean,
    private val firstRow: Boolean, private val lastRow: Boolean, private val density: Float,
) : TableRowSpan(theme, cells, header, false) {
    private val clip = Path()
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = ZorkColors.Border.toArgb(); style = Paint.Style.STROKE; strokeWidth = density
    }
    private var lastWidth = -1
    private var lastHeight = -1
    override fun draw(c: Canvas, text: CharSequence, start: Int, end: Int, x: Float,
        top: Int, y: Int, bottom: Int, paint: Paint) {
        if (!firstRow && !lastRow) { super.draw(c, text, start, end, x, top, y, bottom, paint); return }
        val width = SpanUtils.width(c, text)
        val height = bottom - top
        if (width <= 0 || height <= 0) { super.draw(c, text, start, end, x, top, y, bottom, paint); return }
        val radius = 8f * density
        if (lastWidth != width || lastHeight != height) {
            lastWidth = width; lastHeight = height
            val topRadius = if (firstRow) radius else 0f
            val bottomRadius = if (lastRow) radius else 0f
            clip.reset()
            clip.addRoundRect(RectF(0f, 0f, width.toFloat(), height.toFloat()),
                floatArrayOf(topRadius, topRadius, topRadius, topRadius, bottomRadius, bottomRadius, bottomRadius, bottomRadius), Path.Direction.CW)
        }
        c.save(); c.translate(x, top.toFloat()); c.clipPath(clip)
        super.draw(c, text, start, end, 0f, 0, y - top, height, paint)
        c.drawRoundRect(density / 2f, if (firstRow) density / 2f else -radius,
            width - density / 2f, if (lastRow) height - density / 2f else height + radius,
            radius, radius, stroke)
        c.restore()
    }
}

internal fun renderRoundedMarkdownTable(visitor: MarkwonVisitor, table: TableBlock, theme: TableTheme, density: Float) {
    val rows = mutableListOf<Node>()
    fun collect(node: Node) {
        if (node.firstChild is TableCell) rows.add(node) else {
            var child = node.firstChild
            while (child != null) { collect(child); child = child.next }
        }
    }
    collect(table)
    visitor.blockStart(table)
    val tableStart = visitor.length()
    rows.forEachIndexed { index, row ->
        val cells = mutableListOf<TableRowSpan.Cell>()
        var header = false
        var child = row.firstChild
        while (child is TableCell) {
            val start = visitor.length()
            visitor.visitChildren(child)
            val alignment = when (child.alignment) {
                TableCell.Alignment.CENTER -> TableRowSpan.ALIGN_CENTER
                TableCell.Alignment.RIGHT -> TableRowSpan.ALIGN_RIGHT
                else -> TableRowSpan.ALIGN_LEFT
            }
            cells.add(TableRowSpan.Cell(alignment, visitor.builder().removeFromEnd(start)))
            header = child.isHeader
            child = child.next
        }
        visitor.ensureNewLine()
        val start = visitor.length()
        visitor.builder().append('\u00a0')
        visitor.setSpans(start, RoundedMarkdownTableRow(theme, cells, header, index == 0, index == rows.lastIndex, density))
    }
    visitor.setSpans(tableStart, io.noties.markwon.ext.tables.TableSpan())
    visitor.blockEnd(table)
}
