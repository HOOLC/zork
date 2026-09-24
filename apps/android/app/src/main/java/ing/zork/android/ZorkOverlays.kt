package ing.zork.android

import androidx.compose.animation.*
import androidx.compose.animation.core.MutableTransitionState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SheetValue
import androidx.compose.material3.Surface
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

/** Keep the last value until its standard Compose overlay has closed. */
@Composable
internal fun <T : Any> ZorkRetained(
    value: T?, content: @Composable (T, Boolean, () -> Unit) -> Unit,
) {
    var retained by remember { mutableStateOf<T?>(null) }
    val current by rememberUpdatedState(value)
    SideEffect { if (value != null) retained = value }
    val shown = value ?: retained ?: return
    content(shown, value != null) { if (current == null) retained = null }
}

/**
 * Dialog: the scrim fades in while the panel fades in from 0.96 scale over the
 * surface duration. Closing fades both out without shrinking; content keeps its
 * identity through the transition.
 */
@Composable
internal fun ZorkDialog(
    open: Boolean, title: String, dismiss: () -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    val state = remember { MutableTransitionState(false) }
    state.targetState = open
    if (!state.currentState && !state.targetState && state.isIdle) return
    val reduced = LocalReducedMotion.current
    Dialog(onDismissRequest = dismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        AnimatedVisibility(state,
            enter = if (reduced) zorkFadeIn(ZorkMotion.SURFACE)
                else fadeIn(ZorkMotion.enter(ZorkMotion.SURFACE)) + scaleIn(ZorkMotion.enter(ZorkMotion.SURFACE), initialScale = .96f),
            exit = zorkFadeOut(ZorkMotion.SURFACE)) {
            Surface(Modifier.padding(16.dp).widthIn(max = 480.dp).fillMaxWidth()
                .semantics { paneTitle = title }, shape = ZorkShapes.Surface,
                color = ZorkColors.Canvas, border = BorderStroke(UiTokens.Border, UiTokens.Outline)) {
                Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp),
                    content = content)
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ZorkSheet(
    open: Boolean, title: String, dismiss: () -> Unit, onClosed: () -> Unit = {},
    canDismiss: Boolean = true,
    content: @Composable ColumnScope.() -> Unit,
) {
    val closed by rememberUpdatedState(onClosed)
    val dismissible by rememberUpdatedState(canDismiss)
    LaunchedEffect(open) { if (!open) closed() }
    if (!open) return
    // A busy sheet refuses to hide at the state level. Otherwise Material3 animates
    // it away first and asks afterwards, leaving an invisible sheet that still
    // captures touches.
    val state = rememberModalBottomSheetState(
        skipPartiallyExpanded = true,
        confirmValueChange = { it != SheetValue.Hidden || dismissible },
    )
    ModalBottomSheet(onDismissRequest = { if (dismissible) dismiss() },
        sheetState = state,
        sheetGesturesEnabled = canDismiss,
        shape = ZorkShapes.Sheet,
        containerColor = ZorkColors.Canvas,
        dragHandle = null) {
        Column(Modifier.fillMaxWidth().widthIn(max = 640.dp)
            .imePadding().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 18.dp)
            .semantics { paneTitle = title },
            verticalArrangement = Arrangement.spacedBy(14.dp), content = content)
    }
}

internal object PlainMenuStyle {
    /** Menus are containers; rows stay concentric inside the 8 dp inset. */
    val Radius = UiTokens.CardRadius
    val RowRadius = Radius - 8.dp
}

/**
 * Menus open from their trigger: a fade with a 6 dp shift away from it and a
 * 0.98 → 1 scale with the origin on the trigger side; closing only fades.
 * Placed below the trigger, or above it when there is no room (the shift flips too).
 */
@Composable
internal fun PlainMenu(
    label: String, expanded: Boolean, dismiss: () -> Unit, width: Dp,
    anchor: androidx.compose.ui.geometry.Rect? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val state = remember { MutableTransitionState(false) }
    state.targetState = expanded
    if (!state.currentState && !state.targetState && state.isIdle) return
    var above by remember { mutableStateOf(false) }
    val gap = with(androidx.compose.ui.platform.LocalDensity.current) { 4.dp.roundToPx() }
    val provider = remember(gap) { MenuPosition(gap) { above = it } }
    val reduced = LocalReducedMotion.current
    val shift = with(androidx.compose.ui.platform.LocalDensity.current) { ZorkMotion.RiseShift.roundToPx() }
    androidx.compose.ui.window.Popup(provider, onDismissRequest = dismiss,
        properties = androidx.compose.ui.window.PopupProperties(focusable = true)) {
        AnimatedVisibility(state,
            enter = if (reduced) zorkFadeIn() else fadeIn(ZorkMotion.enter(ZorkMotion.BASE)) +
                slideInVertically(ZorkMotion.enter(ZorkMotion.BASE)) { if (above) shift else -shift } +
                scaleIn(ZorkMotion.enter(ZorkMotion.BASE), initialScale = .98f,
                    transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0f, if (above) 1f else 0f)),
            exit = zorkFadeOut()) {
            Surface(Modifier.widthIn(min = width.coerceAtLeast(160.dp), max = width.coerceAtLeast(320.dp)).heightIn(max = 320.dp)
                .semantics { paneTitle = label },
                shape = RoundedCornerShape(PlainMenuStyle.Radius), color = ZorkColors.Canvas,
                border = BorderStroke(UiTokens.Border, UiTokens.Outline), shadowElevation = 6.dp) {
                Column(Modifier.padding(vertical = 8.dp).verticalScroll(androidx.compose.foundation.rememberScrollState()), content = content)
            }
        }
    }
}

private class MenuPosition(private val gap: Int, private val flipped: (Boolean) -> Unit) :
    androidx.compose.ui.window.PopupPositionProvider {
    override fun calculatePosition(anchorBounds: androidx.compose.ui.unit.IntRect, windowSize: androidx.compose.ui.unit.IntSize,
        layoutDirection: androidx.compose.ui.unit.LayoutDirection, popupContentSize: androidx.compose.ui.unit.IntSize,
    ): androidx.compose.ui.unit.IntOffset {
        val below = anchorBounds.bottom + gap
        val above = below + popupContentSize.height > windowSize.height && anchorBounds.top - gap - popupContentSize.height >= 0
        flipped(above)
        val x = anchorBounds.left.coerceAtMost(windowSize.width - popupContentSize.width).coerceAtLeast(0)
        val y = if (above) anchorBounds.top - gap - popupContentSize.height else below.coerceAtMost((windowSize.height - popupContentSize.height).coerceAtLeast(0))
        return androidx.compose.ui.unit.IntOffset(x, y)
    }
}

@Composable
internal fun ZorkDisclosure(
    title: String, expanded: Boolean, change: (Boolean) -> Unit,
    modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier) {
        ZorkButton(title, Modifier.fillMaxWidth(), onClick = { change(!expanded) })
        AnimatedVisibility(expanded, enter = zorkExpandIn(), exit = zorkCollapseOut()) {
            Column(Modifier.fillMaxWidth().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}
