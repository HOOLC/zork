package ing.zork.android

import android.animation.ValueAnimator
import android.content.Context
import android.text.SpannableString
import android.view.View.MeasureSpec
import android.widget.TextView
import androidx.activity.compose.BackHandler
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Brush
import androidx.compose.material3.HorizontalDivider
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.SubcomposeLayout
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** The measure result is read by SubcomposeLayout in the same pass, not posted
 * as state for a later frame. Hidden text is removed from the selectable view. */
internal class MessagePreviewMeasure(val height: Int, val lines: Int = 8) {
    var more = false
}
internal open class MessageTextView(context: Context) : TextView(context) {
    init {
        // Native interop children cast their own frame-rate votes; the Compose
        // parent's preference does not propagate into these TextViews.
        if (android.os.Build.VERSION.SDK_INT >= 35) requestedFrameRate = 120f
    }
    var preview: MessagePreviewMeasure? = null
    // Selection's movement method can scroll even a height-limited TextView.
    // Chat previews belong to the outer list; the full reader stays selectable.
    override fun scrollTo(x: Int, y: Int) {
        if (preview != null) super.scrollTo(0, 0) else super.scrollTo(x, y)
    }
    override fun canScrollVertically(direction: Int) = preview == null && super.canScrollVertically(direction)
    override fun canScrollHorizontally(direction: Int) = preview == null && super.canScrollHorizontally(direction)
    private var original: CharSequence? = null
    private var measuredWidthSpec = Int.MIN_VALUE
    private var measuredPreview: MessagePreviewMeasure? = null
    fun invalidatePreview() { measuredWidthSpec = Int.MIN_VALUE }
    fun retainMessageText() {
        original = SpannableString(text)
        measuredWidthSpec = Int.MIN_VALUE
    }
    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val limit = preview
        if (limit == null) { super.onMeasure(widthMeasureSpec, heightMeasureSpec); return }
        if (widthMeasureSpec != measuredWidthSpec || measuredPreview !== limit) {
            original?.let { text = it }
            maxLines = Int.MAX_VALUE; maxHeight = Int.MAX_VALUE
            super.onMeasure(widthMeasureSpec, MeasureSpec.makeMeasureSpec(0, MeasureSpec.UNSPECIFIED))
            val layout = layout
            var visible = minOf(layout?.lineCount ?: 0, limit.lines)
            while (visible > 0 && layout!!.getLineBottom(visible - 1) > limit.height) visible--
            var end = if (visible > 0) layout!!.getLineEnd(visible - 1) else 0
            limit.more = measuredHeight > limit.height || (layout?.lineCount ?: 0) > limit.lines
            if (limit.more && end < text.length) {
                // getLineEnd includes the separator; keeping it creates another
                // empty line below the visible preview (and a scroll range).
                while (end > 0 && (text[end - 1] == '\n' || text[end - 1] == '\r')) end--
                text = SpannableString(text.subSequence(0, end))
            }
            scrollTo(0, 0)
            maxLines = limit.lines; maxHeight = limit.height
            measuredWidthSpec = widthMeasureSpec; measuredPreview = limit
        }
        super.onMeasure(widthMeasureSpec, heightMeasureSpec)
    }
}

/** Messages always show in full; very long ones render in independent chunks
 * so no single native text view holds the whole body. */
@Composable
internal fun MessageBody(row: ChatMessage, comment: (String) -> Unit) {
    val parts = remember(row.id, row.content) { messageReaderParts(row.content, row.user) }
    Column(Modifier.fillMaxWidth()) {
        parts.forEach { part ->
            if (row.user) PlainMessage(part, Modifier.fillMaxWidth(), onComment = comment)
            else Markdown(part, Modifier.fillMaxWidth(), onComment = comment)
        }
    }
}

@Composable
internal fun MessageEntry(start: Long?, user: Boolean, content: @Composable () -> Unit) {
    // Only rows core has just accepted animate: fade in while rising 6 dp. Rows
    // present at load, and text streaming into an existing row, never animate.
    val reduced = LocalReducedMotion.current
    val enabled = start != null && android.os.SystemClock.uptimeMillis() - start < ZorkMotion.SURFACE && ValueAnimator.areAnimatorsEnabled()
    val progress = remember(start) { Animatable(if (enabled) 0f else 1f) }
    val distance = with(LocalDensity.current) { ZorkMotion.RiseShift.toPx() }
    LaunchedEffect(start) {
        if (enabled) progress.animateTo(1f, if (reduced) tween(ZorkMotion.REDUCED_FADE) else ZorkMotion.enter(ZorkMotion.BASE))
    }
    Box(Modifier.fillMaxWidth().graphicsLayer {
        alpha = progress.value
        if (!reduced) translationY = distance * (1f - progress.value)
    }) { content() }
}

/** Split only for the independent reader; copy-full continues to use the source.
 * Fenced code carries its language across chunks without putting it in chat. */
internal fun messageReaderParts(text: String, plain: Boolean): List<String> {
    if (text.length <= 4096) return listOf(text)
    val parts = mutableListOf<String>(); val current = StringBuilder()
    var fence: String? = null
    text.lineSequence().forEach { line ->
        val marker = line.trimStart().takeIf { !plain && (it.startsWith("```") || it.startsWith("~~~")) }
        if (current.length >= 4096 && (fence != null || line.isBlank() || current.length >= 8192)) {
            parts += current.toString() + (fence?.let { "\n" + it.take(3) } ?: "")
            current.clear(); fence?.let { current.append(it).append('\n') }
        }
        if (marker != null) fence = if (fence == null) marker else if (marker.startsWith(fence!!.take(3))) null else fence
        // A single line may itself be enormous. Keep chunks on UTF-16 boundaries.
        var offset = 0
        do {
            var end = minOf(offset + 8192, line.length)
            if (end < line.length && end > offset && Character.isHighSurrogate(line[end - 1])) end--
            current.append(line, offset, end); offset = end
            if (offset < line.length) {
                parts += current.toString() + (fence?.let { "\n" + it.take(3) } ?: "")
                current.clear(); fence?.let { current.append(it).append('\n') }
            }
        } while (offset < line.length)
        current.append('\n')
    }
    if (current.isNotEmpty()) parts += current.toString()
    return parts
}
