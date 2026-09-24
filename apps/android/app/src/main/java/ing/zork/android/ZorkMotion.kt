package ing.zork.android

import android.content.Context
import android.database.ContentObserver
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.animation.*
import androidx.compose.animation.core.*
import androidx.compose.foundation.IndicationNodeFactory
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.invalidateDraw
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.Modifier.Node
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Motion tokens and recipes (apps/zork-design-pc/docs/07-motion.md). Pages never
 * use bare durations or curves. Motion only animates opacity, translation and
 * scale (plus height inside one expanding container), and never runs while static.
 */
internal object ZorkMotion {
    const val FAST = 120
    const val BASE = 180
    const val SURFACE = 240
    const val PAGE = 280
    /** Exits take two thirds of the matching entry. */
    fun exit(enter: Int) = enter * 2 / 3
    /** Reduced motion keeps only short fades. */
    const val REDUCED_FADE = 150
    const val VALUE = 300
    const val LOADING_DELAY = 300L
    const val LOADING_MIN = 400L
    const val PULSE = 1600

    val Enter = CubicBezierEasing(.2f, .7f, .2f, 1f)
    val Exit = CubicBezierEasing(.4f, 0f, 1f, 1f)
    val Move = CubicBezierEasing(.3f, 0f, .2f, 1f)
    val Release = spring<Float>(dampingRatio = .9f, stiffness = Spring.StiffnessMedium)

    val PageShift = 24.dp
    val RiseShift = 6.dp
    val NoticeShift = 8.dp

    fun <T> enter(duration: Int = BASE) = tween<T>(duration, easing = Enter)
    fun <T> exitSpec(duration: Int = BASE) = tween<T>(exit(duration), easing = Exit)
    fun <T> move(duration: Int = BASE) = tween<T>(duration, easing = Move)
}

/** True when the system asks for reduced motion ("remove animations" sets the animator scale to 0). */
internal val LocalReducedMotion = staticCompositionLocalOf { false }

internal fun systemReducedMotion(context: Context): Boolean =
    Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f

/** Tracks the system setting while composed, without polling. */
@Composable
internal fun rememberReducedMotion(): Boolean {
    val context = LocalContext.current
    var reduced by remember { mutableStateOf(systemReducedMotion(context)) }
    DisposableEffect(context) {
        val uri: Uri = Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE)
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) { reduced = systemReducedMotion(context) }
        }
        context.contentResolver.registerContentObserver(uri, false, observer)
        onDispose { context.contentResolver.unregisterContentObserver(observer) }
    }
    return reduced
}

// ---------- Enter / exit transitions ----------

@Composable
internal fun zorkFadeIn(duration: Int = ZorkMotion.BASE): EnterTransition =
    fadeIn(if (LocalReducedMotion.current) tween(minOf(duration, ZorkMotion.REDUCED_FADE)) else ZorkMotion.enter(duration))

@Composable
internal fun zorkFadeOut(duration: Int = ZorkMotion.BASE): ExitTransition =
    fadeOut(if (LocalReducedMotion.current) tween(minOf(ZorkMotion.exit(duration), ZorkMotion.REDUCED_FADE)) else ZorkMotion.exitSpec(duration))

/** Rows and new messages: fade in while rising [ZorkMotion.RiseShift]. */
@Composable
internal fun zorkRiseIn(): EnterTransition {
    if (LocalReducedMotion.current) return zorkFadeIn()
    val shift = with(LocalDensity.current) { ZorkMotion.RiseShift.roundToPx() }
    return fadeIn(ZorkMotion.enter(ZorkMotion.BASE)) + slideInVertically(ZorkMotion.enter(ZorkMotion.BASE)) { shift }
}

/** List rows added after first display fade in; rows that move slide on the move curve. */
@Composable
internal fun listFade(): FiniteAnimationSpec<Float> =
    if (LocalReducedMotion.current) tween(ZorkMotion.REDUCED_FADE) else ZorkMotion.enter(ZorkMotion.BASE)

@Composable
internal fun listFadeOut(): FiniteAnimationSpec<Float> =
    if (LocalReducedMotion.current) tween(ZorkMotion.REDUCED_FADE) else ZorkMotion.exitSpec(ZorkMotion.BASE)

@Composable
internal fun listMove(): FiniteAnimationSpec<androidx.compose.ui.unit.IntOffset>? =
    if (LocalReducedMotion.current) null else ZorkMotion.move(ZorkMotion.BASE)

/** Status notices come down from their region's top edge. */
@Composable
internal fun zorkNoticeIn(): EnterTransition {
    if (LocalReducedMotion.current) return zorkFadeIn(ZorkMotion.SURFACE)
    val shift = with(LocalDensity.current) { ZorkMotion.NoticeShift.roundToPx() }
    return fadeIn(ZorkMotion.enter(ZorkMotion.SURFACE)) + slideInVertically(ZorkMotion.enter(ZorkMotion.SURFACE)) { -shift }
}

/** Expanders: height on the move curve, content fades in once the height is past halfway. */
@Composable
internal fun zorkExpandIn(): EnterTransition {
    if (LocalReducedMotion.current) return zorkFadeIn()
    return expandVertically(ZorkMotion.move(ZorkMotion.BASE)) +
        fadeIn(tween(ZorkMotion.BASE / 2, delayMillis = ZorkMotion.BASE / 2, easing = ZorkMotion.Enter))
}

/** Collapsing: content fades first, then the height closes. */
@Composable
internal fun zorkCollapseOut(): ExitTransition {
    if (LocalReducedMotion.current) return zorkFadeOut()
    return fadeOut(tween(ZorkMotion.FAST, easing = ZorkMotion.Exit)) +
        shrinkVertically(tween(ZorkMotion.BASE, delayMillis = ZorkMotion.FAST / 2, easing = ZorkMotion.Move))
}

/** An expander body. Nothing animates while it stays open or closed. */
@Composable
internal fun ZorkExpand(visible: Boolean, content: @Composable AnimatedVisibilityScope.() -> Unit) {
    AnimatedVisibility(visible, enter = zorkExpandIn(), exit = zorkCollapseOut(), content = content)
}

/** Chevron rotation that follows an expander. */
@Composable
internal fun zorkChevron(expanded: Boolean): Float {
    val reduced = LocalReducedMotion.current
    val angle by animateFloatAsState(if (expanded) 180f else 0f,
        if (reduced) snap() else ZorkMotion.move(ZorkMotion.BASE), label = "chevron")
    return angle
}

// ---------- Value changes ----------

/** Numbers and bars tween on the move curve, but not on first appearance. */
@Composable
internal fun animatedValue(target: Float, label: String): Float {
    val reduced = LocalReducedMotion.current
    val value = remember { Animatable(target) }
    LaunchedEffect(target) {
        if (value.value == target) return@LaunchedEffect
        if (reduced) value.snapTo(target) else value.animateTo(target, ZorkMotion.move(ZorkMotion.VALUE))
    }
    return value.value
}

/**
 * Loading shows only if it lasts beyond 300 ms, then stays at least 400 ms so it
 * never flickers. Returns whether the loading UI should be visible now.
 */
@Composable
internal fun rememberDeferredLoading(loading: Boolean): Boolean {
    var shown by remember { mutableStateOf(false) }
    var since by remember { mutableLongStateOf(0L) }
    LaunchedEffect(loading) {
        if (loading) {
            delay(ZorkMotion.LOADING_DELAY)
            since = System.currentTimeMillis(); shown = true
        } else if (shown) {
            val left = ZorkMotion.LOADING_MIN - (System.currentTimeMillis() - since)
            if (left > 0) delay(left)
            shown = false
        }
    }
    return shown
}

// ---------- Working pulse ----------

/**
 * The persimmon "working" dot breathes over 1.6 s only while [active]. When the
 * dot leaves composition or stops, the loop ends; reduced motion keeps it static.
 */
@Composable
internal fun Modifier.workingPulse(active: Boolean): Modifier = this.then(
    if (!active) Modifier else Modifier.composedPulse()
)

@Composable
private fun Modifier.composedPulse(): Modifier {
    if (LocalReducedMotion.current) return this
    val transition = rememberInfiniteTransition(label = "working")
    val alpha by transition.animateFloat(1f, .35f,
        infiniteRepeatable(tween(ZorkMotion.PULSE / 2, easing = ZorkMotion.Move), RepeatMode.Reverse), label = "pulse")
    return this.graphicsLayer { this.alpha = alpha }
}

// ---------- Pressed state instead of ripple ----------

/**
 * Pressing shows a flat pressed fill that fades in over [ZorkMotion.FAST] and out
 * on release. No ripple, scale or movement.
 */
internal class ZorkPressIndication(private val color: () -> Color) : IndicationNodeFactory {
    override fun create(interactionSource: InteractionSource): androidx.compose.ui.node.DelegatableNode =
        PressNode(interactionSource, color)
    override fun hashCode() = 1
    override fun equals(other: Any?) = other is ZorkPressIndication
}

private class PressNode(private val source: InteractionSource, private val color: () -> Color) : Node(), DrawModifierNode {
    private val alpha = Animatable(0f)
    override fun onAttach() {
        coroutineScope.launch {
            val pressed = mutableListOf<PressInteraction.Press>()
            source.interactions.collect { interaction ->
                when (interaction) {
                    is PressInteraction.Press -> pressed += interaction
                    is PressInteraction.Release -> pressed -= interaction.press
                    is PressInteraction.Cancel -> pressed -= interaction.press
                }
                val target = if (pressed.isEmpty()) 0f else 1f
                launch {
                    alpha.animateTo(target, tween(if (target > 0f) ZorkMotion.FAST else ZorkMotion.exit(ZorkMotion.FAST))) { invalidateDraw() }
                }
            }
        }
    }
    // The fill sits behind the element's children, above its own background.
    override fun ContentDrawScope.draw() {
        if (alpha.value > 0f) drawRect(color(), alpha = alpha.value)
        drawContent()
    }
}
