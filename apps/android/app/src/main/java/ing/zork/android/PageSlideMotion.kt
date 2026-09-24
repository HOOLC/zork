package ing.zork.android

import androidx.compose.animation.core.tween
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer

/** Shared stack motion for chat and settings. Visibility is always the fraction
 * of the front page shown: 0 before it arrives, 1 fully in place. The front page
 * enters 24 dp from the right while fading in over the page below, which stays
 * put; popping reverses it. Reduced motion keeps only a short fade. */
internal object PageSlideMotion {
    fun <T> spec(entering: Boolean, reduced: Boolean = false) = when {
        reduced -> tween<T>(ZorkMotion.REDUCED_FADE)
        entering -> tween<T>(ZorkMotion.PAGE, easing = ZorkMotion.Enter)
        else -> tween<T>(ZorkMotion.exit(ZorkMotion.PAGE), easing = ZorkMotion.Exit)
    }
    fun frontOffset(visibility: Float) = 1f - visibility
}

internal fun Modifier.pageSlideFront(visibility: () -> Float, reduced: Boolean) = graphicsLayer {
    val v = visibility()
    if (!reduced) translationX = ZorkMotion.PageShift.toPx() * PageSlideMotion.frontOffset(v)
    alpha = v
}

/** While a back gesture is in progress the current page follows the finger. */
internal fun Modifier.pageSlideGesture(progress: () -> Float, reduced: Boolean) = graphicsLayer {
    val p = progress()
    if (p > 0f) {
        if (!reduced) translationX = ZorkMotion.PageShift.toPx() * p
        alpha = 1f - p
    }
}
