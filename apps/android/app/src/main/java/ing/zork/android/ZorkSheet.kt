package ing.zork.android

import android.view.ViewGroup
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Velocity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import kotlinx.coroutines.launch

/** Close when dragged past this share of the panel's height, or on a fast fling. */
private const val DISMISS_FRACTION = .3f
private val FlingVelocity = 800.dp

/**
 * The app's bottom sheet. The scrim fades in while the panel slides up over the
 * page duration on the enter curve. Dragging (on the panel, or past the top of its
 * scrolled content) follows the finger; on release a spring carries the fling's
 * velocity back into place, or away when dragged past 30% or flung fast.
 * [canDismiss] false keeps it open: no drag, no scrim tap, no back.
 * [onClosed] runs once the exit has finished, so retained content stays until then.
 */
@Composable
internal fun ZorkSheet(
    open: Boolean, title: String, dismiss: () -> Unit, onClosed: () -> Unit = {},
    canDismiss: Boolean = true,
    /** Back is offered here first; returning true means an inner layer closed instead of the sheet. */
    back: (() -> Boolean)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val innerBack by rememberUpdatedState(back)
    val closed by rememberUpdatedState(onClosed)
    val dismissible by rememberUpdatedState(canDismiss)
    val requestDismiss by rememberUpdatedState(dismiss)
    val reduced = LocalReducedMotion.current
    // 0 hidden … 1 fully shown.
    val shown = remember { Animatable(0f) }
    var mounted by remember { mutableStateOf(open) }
    var height by remember { mutableIntStateOf(0) }
    var releaseVelocity by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(open) {
        if (open) { mounted = true; return@LaunchedEffect }
        if (mounted && shown.value > 0f) {
            val velocity = releaseVelocity; releaseVelocity = 0f
            if (reduced) shown.animateTo(0f, tween(ZorkMotion.REDUCED_FADE))
            else if (velocity != 0f) shown.animateTo(0f, ZorkMotion.Release, velocity)
            else shown.animateTo(0f, ZorkMotion.exitSpec(ZorkMotion.PAGE))
        }
        mounted = false
        closed()
    }
    // Enter once the panel has a height to slide from. Re-keying this effect
    // cancels an entry already in flight, so resume from the current value
    // instead of skipping because the cancelled animation's target was 1.
    LaunchedEffect(open, height > 0) {
        if (open && height > 0 && shown.value < 1f)
            shown.animateTo(1f, if (reduced) tween(ZorkMotion.REDUCED_FADE) else ZorkMotion.enter(ZorkMotion.PAGE))
    }
    if (!mounted) return
    val scope = rememberCoroutineScope()
    val fling = with(LocalDensity.current) { FlingVelocity.toPx() }
    fun dragBy(delta: Float) {
        if (height <= 0) return
        scope.launch { shown.snapTo((shown.value - delta / height).coerceIn(0f, 1f)) }
    }
    /** Settles after a drag; [velocity] is in px/s, positive downwards. */
    suspend fun settle(velocity: Float) {
        if (height <= 0) return
        val progressVelocity = -velocity / height
        if (dismissible && (1f - shown.value > DISMISS_FRACTION || velocity > fling)) {
            releaseVelocity = progressVelocity
            requestDismiss()
        } else if (reduced) shown.snapTo(1f)
        else shown.animateTo(1f, ZorkMotion.Release, progressVelocity)
    }
    val nested = remember {
        object : NestedScrollConnection {
            // Finger moving up while the sheet is partly down: raise the sheet first.
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (source != NestedScrollSource.UserInput || available.y >= 0f || shown.value >= 1f || height <= 0) return Offset.Zero
                val before = shown.value
                val next = (before - available.y / height).coerceAtMost(1f)
                scope.launch { shown.snapTo(next) }
                return Offset(0f, -(next - before) * height)
            }
            // Content already at its top and the finger keeps moving down: lower the sheet.
            override fun onPostScroll(consumed: Offset, available: Offset, source: NestedScrollSource): Offset {
                if (!dismissible || source != NestedScrollSource.UserInput || available.y <= 0f || height <= 0) return Offset.Zero
                dragBy(available.y)
                return Offset(0f, available.y)
            }
            override suspend fun onPreFling(available: Velocity): Velocity {
                if (shown.value >= 1f) return Velocity.Zero
                settle(available.y)
                return available
            }
        }
    }
    Dialog(onDismissRequest = { if (innerBack?.invoke() != true && dismissible) requestDismiss() },
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false,
            dismissOnClickOutside = false)) {
        // The sheet draws its own scrim and motion; the window adds neither.
        val window = (LocalView.current.parent as? DialogWindowProvider)?.window
        SideEffect {
            window?.setDimAmount(0f)
            window?.setWindowAnimations(0)
            window?.setLayout(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        }
        Box(Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxSize()
                .graphicsLayer { alpha = shown.value }
                .background(ZorkColors.Scrim)
                .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null,
                    enabled = canDismiss) { requestDismiss() }
                .semantics {
                    contentDescription = "关闭弹层"
                    if (canDismiss) onClick("关闭弹层") { requestDismiss(); true }
                })
            Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.statusBars).padding(top = 24.dp),
                contentAlignment = Alignment.BottomCenter) {
                Column(Modifier.widthIn(max = 640.dp).fillMaxWidth()
                    .onSizeChanged { height = it.height }
                    .graphicsLayer {
                        if (reduced) alpha = shown.value
                        else translationY = (1f - shown.value) * size.height
                    }
                    .background(ZorkColors.Canvas, ZorkShapes.Sheet)
                    .nestedScroll(nested)
                    .draggable(rememberDraggableState { dragBy(it) }, Orientation.Vertical, enabled = canDismiss,
                        onDragStopped = { velocity -> settle(velocity) })
                    .imePadding().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 18.dp)
                    .semantics { paneTitle = title },
                    verticalArrangement = Arrangement.spacedBy(14.dp), content = content)
            }
        }
    }
}

