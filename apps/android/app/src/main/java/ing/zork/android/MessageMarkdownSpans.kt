package ing.zork.android

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.text.Layout
import android.text.Spanned
import android.text.TextPaint
import android.text.TextUtils
import android.text.style.LeadingMarginSpan
import android.text.style.LineBackgroundSpan
import android.text.style.LineHeightSpan
import android.text.style.MetricAffectingSpan
import androidx.compose.ui.graphics.toArgb
import kotlin.math.ceil
import kotlin.math.max

internal class MarkdownGapSpan(private val height: Int, private val extra: Float) : LineHeightSpan {
    override fun chooseHeight(text: CharSequence, start: Int, end: Int, spanstartv: Int, v: Int, fm: Paint.FontMetricsInt) {
        fm.ascent = -max(1, height - extra.toInt()); fm.top = fm.ascent
        fm.descent = 0; fm.bottom = 0
    }
}

internal class MarkdownNumberSpan(
    val number: Int, density: Float, textPixels: Float, mono: Typeface,
) : LeadingMarginSpan {
    private val marker = "$number."
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = mono; textSize = textPixels; color = ZorkColors.Muted.toArgb()
    }
    private val gap = 6f * density
    private val width = max(24f * density, ceil(paint.measureText(marker)))
    override fun getLeadingMargin(first: Boolean) = ceil(width + gap).toInt()
    override fun drawLeadingMargin(c: Canvas, p: Paint, x: Int, dir: Int, top: Int, baseline: Int,
        bottom: Int, text: CharSequence, start: Int, end: Int, first: Boolean, layout: Layout) {
        if (first && (text as Spanned).getSpanStart(this) == start) {
            val left = if (dir > 0) x + width - paint.measureText(marker) else x - width
            c.drawText(marker, left, baseline.toFloat(), paint)
        }
    }
}

/** One selectable TextView retains cross-paragraph/code selection. Geometry
 * supplies padding and the label, so neither changes the copied code. */
internal class MarkdownCodePanelSpan(
    val language: String, private val density: Float, private val textPixels: Float,
    private val mono: Typeface, body: Typeface,
) : MetricAffectingSpan(), LeadingMarginSpan, LineBackgroundSpan, LineHeightSpan {
    private val radius = 8f * density
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = ZorkColors.Paper.toArgb() }
    private val border = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = ZorkColors.Border.toArgb(); style = Paint.Style.STROKE; strokeWidth = density
    }
    private val label = TextPaint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = body; textSize = textPixels * .8f; color = ZorkColors.Muted.toArgb()
    }
    override fun updateMeasureState(p: TextPaint) { p.typeface = mono; p.textSize = textPixels }
    override fun updateDrawState(p: TextPaint) = updateMeasureState(p)
    override fun getLeadingMargin(first: Boolean) = (12f * density).toInt()
    override fun drawLeadingMargin(c: Canvas, p: Paint, x: Int, dir: Int, top: Int, baseline: Int,
        bottom: Int, text: CharSequence, start: Int, end: Int, first: Boolean, layout: Layout) {
        if (language.isNotBlank() && (text as Spanned).getSpanStart(this) == start) {
            val headerBottom = top + headerHeight()
            val available = max(1f, layout.width - x - 24f * density)
            val shown = TextUtils.ellipsize(language, label, available, TextUtils.TruncateAt.END)
            val metrics = label.fontMetrics
            c.drawText(shown.toString(), x + 12f * density, (top + headerBottom - metrics.ascent - metrics.descent) / 2f, label)
        }
    }
    private fun headerHeight() = if (language.isBlank()) 0f else max(26f * density, textPixels * 1.65f)

    override fun chooseHeight(text: CharSequence, start: Int, end: Int, spanstartv: Int, v: Int, fm: Paint.FontMetricsInt) {
        val spanned = text as? Spanned ?: return
        val first = start == spanned.getSpanStart(this)
        val last = end >= spanned.getSpanEnd(this)
        if (first) {
            val extra = ceil(headerHeight() + 8f * density).toInt()
            fm.ascent -= extra; fm.top -= extra
        }
        if (last) {
            val extra = max(0, ceil(8f * density - textPixels * .4f).toInt())
            fm.descent += extra; fm.bottom += extra
        }
    }

    override fun drawBackground(c: Canvas, p: Paint, left: Int, right: Int, top: Int, baseline: Int,
        bottom: Int, text: CharSequence, start: Int, end: Int, lineNumber: Int) {
        val spanned = text as? Spanned ?: return
        val first = start == spanned.getSpanStart(this)
        val last = end >= spanned.getSpanEnd(this)
        val indent = spanned.getSpans(start, end, LeadingMarginSpan::class.java)
            .filter { it !== this }.sumOf { it.getLeadingMargin(first) }
        val l = left + indent.toFloat() + density / 2f
        val r = right - density / 2f
        if (r <= l) return
        val t = top.toFloat(); val b = bottom.toFloat()
        c.save(); c.clipRect(l - density, t, r + density, b)
        if (first || last) {
            val roundTop = if (first) t + density / 2f else t - radius
            val roundBottom = if (last) b - density / 2f else b + radius
            c.drawRoundRect(l, roundTop, r, roundBottom, radius, radius, fill)
            c.drawRoundRect(l, roundTop, r, roundBottom, radius, radius, border)
        } else {
            c.drawRect(l, t, r, b, fill)
            c.drawLine(l, t, l, b, border); c.drawLine(r, t, r, b, border)
        }
        if (first && language.isNotBlank()) {
            c.drawLine(l, t + headerHeight(), r, t + headerHeight(), border)
        }
        c.restore()
    }
}
