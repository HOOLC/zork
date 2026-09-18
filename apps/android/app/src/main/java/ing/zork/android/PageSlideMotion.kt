package ing.zork.android

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer

/** Shared stack motion for chat and settings. Visibility is always the fraction
 * of the front page shown: 0 outside the right edge, 1 fully on screen.
 * Pushing increases visibility; popping decreases it. */
internal object PageSlideMotion {
    fun <T> spec(entering: Boolean) = tween<T>(
        durationMillis = if (entering) 220 else 180,
        easing = FastOutSlowInEasing,
    )
    fun frontOffset(visibility: Float) = 1f - visibility
    fun backOffset(visibility: Float) = -.25f * visibility
}

internal fun Modifier.pageSlideFront(visibility: () -> Float) = graphicsLayer {
    translationX = size.width * PageSlideMotion.frontOffset(visibility())
}

internal fun Modifier.pageSlideBack(visibility: () -> Float) = graphicsLayer {
    translationX = size.width * PageSlideMotion.backOffset(visibility())
}
