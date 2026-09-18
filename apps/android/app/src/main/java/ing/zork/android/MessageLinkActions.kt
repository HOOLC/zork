package ing.zork.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.res.ColorStateList
import android.graphics.Rect
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.net.Uri
import android.os.SystemClock
import android.text.Spanned
import android.text.TextUtils
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.ViewTreeObserver
import android.view.ViewConfiguration
import android.widget.LinearLayout
import android.widget.PopupWindow
import android.widget.TextView
import android.widget.Toast
import androidx.compose.ui.graphics.toArgb
import androidx.core.content.res.ResourcesCompat
import io.noties.markwon.core.spans.LinkSpan
import java.util.Locale
import kotlin.math.roundToInt

/** Link actions are created only on activation; parsing caches never retain a popup or View. */
internal class MarkdownTextView(context: Context) : MessageTextView(context) {
    private var linkPopup: PopupWindow? = null
    private var touchTime = 0L
    private var touchX = 0f
    private var touchY = 0f
    private var pressedLink: LinkSpan? = null
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop

    private fun linkAt(x: Float, y: Float): LinkSpan? {
        val content = text as? Spanned ?: return null
        val lines = layout ?: return null
        val localX = x - totalPaddingLeft + scrollX
        val localY = y - totalPaddingTop + scrollY
        if (localY < 0 || localY >= lines.height) return null
        val line = lines.getLineForVertical(localY.toInt())
        if (localX < lines.getLineLeft(line) || localX > lines.getLineRight(line)) return null
        val offset = lines.getOffsetForHorizontal(line, localX)
        return content.getSpans(offset, offset, LinkSpan::class.java).firstOrNull()
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                touchTime = SystemClock.uptimeMillis(); touchX = event.x; touchY = event.y
                pressedLink = linkAt(event.x, event.y)
            }
            MotionEvent.ACTION_MOVE -> {
                if (kotlin.math.abs(event.x - touchX) > touchSlop || kotlin.math.abs(event.y - touchY) > touchSlop) pressedLink = null
            }
            MotionEvent.ACTION_CANCEL -> pressedLink = null
            MotionEvent.ACTION_UP -> {
                val link = pressedLink; pressedLink = null
                if (link != null && linkAt(event.x, event.y) === link &&
                    event.eventTime - event.downTime < ViewConfiguration.getLongPressTimeout()) {
                    // Text selection otherwise consumes the first short tap. Long presses
                    // and drags stay with TextView's selection/scroll handling.
                    val cancel = MotionEvent.obtain(event).apply { action = MotionEvent.ACTION_CANCEL }
                    super.onTouchEvent(cancel); cancel.recycle()
                    showLinkActions(link.link)
                    return true
                }
            }
        }
        return super.onTouchEvent(event)
    }

    fun dismissLinkActions() { linkPopup?.dismiss() }
    override fun onDetachedFromWindow() { dismissLinkActions(); super.onDetachedFromWindow() }

    fun showLinkActions(link: String) {
        if (!isAttachedToWindow) return
        dismissLinkActions()
        val density = resources.displayMetrics.density
        fun dp(value: Int) = (value * density).roundToInt()
        fun rounded(color: Int, radius: Int) = GradientDrawable().apply {
            setColor(color); cornerRadius = dp(radius).toFloat()
        }
        val uri = Uri.parse(link)
        val sharedService = uri.scheme == "zork" && uri.host == "service"
        val canOpen = sharedService || uri.scheme?.lowercase(Locale.ROOT) in listOf("http", "https", "mailto")
        val frame = Rect().also(::getWindowVisibleDisplayFrame)
        val panel = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(4), dp(6), dp(4), dp(4))
        }
        val font = ResourcesCompat.getFont(context, R.font.inter)
        panel.addView(TextView(context).apply {
            text = link; textSize = 12f; typeface = font
            setTextColor(ZorkColors.Muted.toArgb()); maxLines = 1; ellipsize = TextUtils.TruncateAt.END
            setPadding(dp(10), dp(4), dp(10), dp(6))
        }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT))
        val actions = LinearLayout(context).apply { orientation = LinearLayout.HORIZONTAL }
        panel.addView(actions)
        val popup = PopupWindow(panel, minOf(dp(240), frame.width() - dp(16)), LinearLayout.LayoutParams.WRAP_CONTENT, true).apply {
            setBackgroundDrawable(rounded(ZorkColors.Canvas.toArgb(), 12))
            elevation = dp(6).toFloat(); isOutsideTouchable = true
            inputMethodMode = PopupWindow.INPUT_METHOD_NOT_NEEDED
            animationStyle = 0
        }
        fun action(label: String, enabled: Boolean = true, run: () -> Unit) {
            actions.addView(TextView(context).apply {
                text = label; textSize = 14f; typeface = font; gravity = Gravity.CENTER
                minHeight = dp(44); isEnabled = enabled; isFocusable = true
                setTextColor(if (enabled) ZorkColors.Ink.toArgb() else ZorkColors.Muted.toArgb())
                background = RippleDrawable(ColorStateList.valueOf(ZorkColors.Selected.toArgb()),
                    rounded(ZorkColors.Canvas.toArgb(), 8), null)
                setOnClickListener { popup.dismiss(); run() }
            }, LinearLayout.LayoutParams(0, dp(44), 1f))
        }
        action("复制链接") {
            context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("链接", link))
        }
        action("打开链接", canOpen) {
            runCatching {
                if (sharedService) openServiceBrowser(context, link)
                else context.startActivity(Intent(Intent.ACTION_VIEW, uri))
            }
                .onFailure { Toast.makeText(context, "没有可打开此链接的应用", Toast.LENGTH_SHORT).show() }
        }
        panel.measure(View.MeasureSpec.makeMeasureSpec(popup.width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(frame.height(), View.MeasureSpec.AT_MOST))
        val location = IntArray(2).also(::getLocationOnScreen)
        var anchorX = width / 2f
        var anchorY = height / 2f
        if (touchTime != 0L && SystemClock.uptimeMillis() - touchTime < 1500) {
            anchorX = touchX; anchorY = touchY
        } else {
            // Accessibility/programmatic activation has no touch position.
            val spans = text as? Spanned
            val span = spans?.getSpans(0, spans.length, LinkSpan::class.java)?.firstOrNull { it.link == link }
            if (span != null && layout != null) {
                val offset = spans.getSpanStart(span)
                val line = layout.getLineForOffset(offset)
                anchorX = totalPaddingLeft + layout.getPrimaryHorizontal(offset) - scrollX
                anchorY = totalPaddingTop + (layout.getLineTop(line) + layout.getLineBottom(line)) / 2f - scrollY
            }
        }
        touchTime = 0L
        val x = (location[0] + anchorX - popup.width / 2).roundToInt()
            .coerceIn(frame.left + dp(8), maxOf(frame.left + dp(8), frame.right - popup.width - dp(8)))
        val above = (location[1] + anchorY - panel.measuredHeight - dp(16)).roundToInt()
        val y = (if (above >= frame.top + dp(8)) above else (location[1] + anchorY + dp(20)).roundToInt())
            .coerceIn(frame.top + dp(8), maxOf(frame.top + dp(8), frame.bottom - panel.measuredHeight - dp(8)))
        val rootScreen = IntArray(2).also { rootView.getLocationOnScreen(it) }
        val rootWindow = IntArray(2).also { rootView.getLocationInWindow(it) }
        val observer = viewTreeObserver
        val scrollListener = ViewTreeObserver.OnScrollChangedListener { popup.dismiss() }
        popup.setOnDismissListener {
            if (observer.isAlive) observer.removeOnScrollChangedListener(scrollListener)
            if (linkPopup === popup) linkPopup = null
        }
        linkPopup = popup
        popup.showAtLocation(this, Gravity.TOP or Gravity.LEFT,
            x - rootScreen[0] + rootWindow[0], y - rootScreen[1] + rootWindow[1])
        observer.addOnScrollChangedListener(scrollListener)
    }
}
