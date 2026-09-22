package ing.zork.android

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.layout.positionOnScreen
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.hideFromAccessibility
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.*
import androidx.compose.ui.window.*

/** Retain only presentation data through exit; actions still update their owner immediately. */
@Composable
internal fun <T : Any> LiquidRetained(value: T?, content: @Composable (T, Boolean, () -> Unit) -> Unit) {
    var retained by remember { mutableStateOf<T?>(null) }
    val current by rememberUpdatedState(value)
    SideEffect { if (value != null) retained = value }
    val shown = value ?: retained ?: return
    content(shown, value != null) { if (current == null) retained = null }
}

private class OriginBinding { var open = false; var source: LiquidOrigin? = null; var closingEpoch = 0L }
@Composable
private fun rememberOrigin(open: Boolean, node: LiquidNode): OriginBinding {
    val binding = remember { OriginBinding() }
    if (open && !binding.open) binding.source = node.host.lastActivation
    if (!open && binding.open) binding.closingEpoch = node.host.activationEpoch
    binding.open = open
    return binding
}

/** Native windows own focus, Back and IME; the shared scene owns visual motion. */
@Composable
internal fun LiquidDialog(open: Boolean, title: String, dismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    LiquidModal(open, title, dismiss, content)
}

@Composable
internal fun LiquidSheet(open: Boolean, title: String, dismiss: () -> Unit, onClosed: () -> Unit = {}, content: @Composable ColumnScope.() -> Unit) {
    LiquidModal(open, title, dismiss, content, onClosed)
}

@Composable
private fun LiquidModal(open: Boolean, title: String, dismiss: () -> Unit, content: @Composable ColumnScope.() -> Unit, onClosed: () -> Unit = {}) {
    val node = rememberLiquidNode()
    val binding = rememberOrigin(open, node)
    val paint = rememberLiquidPaint(node, true)
    val origin = binding.source
    SideEffect {
        paint.update(open, origin)
        node.target?.let { node.update(it.copy(active = open)) }
        if (!open && !node.alive) origin?.relocated = false
    }
    val closed by rememberUpdatedState(onClosed)
    LaunchedEffect(open, node.alive) {
        if (!open && !node.alive) { origin?.restoreFocus(binding.closingEpoch); closed() }
    }
    // Keep the recorded child layers alive until exit ends. The native window
    // relinquishes input immediately; the source window plays its drawing.
    if (!open && !node.alive) return
    Dialog(onDismissRequest = { if (open) dismiss() }, properties = DialogProperties(
        dismissOnBackPress = open, dismissOnClickOutside = open,
        usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        val view = LocalView.current
        SideEffect { (view.parent as? DialogWindowProvider)?.window?.let { window ->
            window.setDimAmount(0f)
            val flags = android.view.WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or android.view.WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE
            if (open) window.clearFlags(flags) else window.addFlags(flags)
        } }
        CompositionLocalProvider(LocalLiquidInteractive provides (open && node.inputReady)) {
        LiquidPaintHost {
            Box(Modifier.fillMaxSize().semantics { if (!open) hideFromAccessibility() }.liquidOverlayWindow(paint)) {
                Box(Modifier.matchParentSize().clickable(indication = null,
                    interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() }, onClick = dismiss))
                Column(Modifier.align(Alignment.BottomCenter)
                    .windowInsetsPadding(WindowInsets.safeDrawing.union(WindowInsets.ime))
                    .padding(8.dp).widthIn(max = 640.dp).fillMaxWidth()
                    .semantics { paneTitle = title }
                    .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent(PointerEventPass.Final).changes.forEach { it.consume() } } }
                    .liquidOverlayPanel(paint, LiquidTokens.CardRadius)
                    .padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp), content = content)
            }
        }
        }
    }
}

@Composable
private fun Modifier.liquidOverlayWindow(paint: LiquidOverlayPaint): Modifier =
    onGloballyPositioned { paint.windowOrigin = it.positionOnScreen() }.drawWithContent {
        paint.node.revision
        if (!paint.open) return@drawWithContent
        if (paint.transfer && paint.frameRecorded && paint.node.host.durationScale > 0f) {
            val offset = paint.frameOrigin - paint.windowOrigin
            translate(offset.x, offset.y) { drawLayer(paint.frame) }
            paint.transfer = false
        } else {
            drawContent()
            paint.node.prepareFirstDraw()
            val initial = !paint.frameRecorded && paint.source?.recorded == true && paint.node.host.durationScale > 0f
            paint.frame.record { with(paint) { if (initial) source(windowOrigin) else paint(windowOrigin) } }
            paint.frameRecorded = true
            paint.frameOrigin = paint.windowOrigin
            drawLayer(paint.frame)
        }
    }

@Composable
private fun Modifier.liquidOverlayPanel(paint: LiquidOverlayPaint, radius: Dp): Modifier {
    val scale = LocalDensity.current.density
    val node = paint.node
    // Observe the live source layout independently of its drawing ownership.
    val sourcePosition = paint.source?.node?.windowOrigin
    var size by remember { mutableStateOf(IntSize.Zero) }
    fun update() {
        if (!paint.open) return
        if (size.width < 2 * scale || size.height < 2 * scale) return
        val width = size.width / scale
        val height = size.height / scale
        val source = paint.source?.relativeTo(node, scale)
        node.update(LiquidTarget(if (source == null) LiquidKind.Morph else LiquidKind.Pair,
            source ?: LiquidPose(y = height - 2f, width = width, height = 2f, radius = 1f),
            LiquidPose(width = width, height = height, radius = radius.value), active = paint.open,
            visible = paint.modal || paint.source?.node?.target?.visible != false,
            anchorY = if (paint.modal) 1f else 0f), scale)
    }
    SideEffect { update() }
    return onGloballyPositioned {
        if (paint.open) {
            node.windowOrigin = it.positionOnScreen()
            size = it.size
            update()
        }
    }.drawWithContent {
        if (paint.open) {
            paint.content.record { this@drawWithContent.drawContent() }
            paint.recorded = true
        }
    }
}

private object MenuWindowPosition : PopupPositionProvider {
    override fun calculatePosition(anchorBounds: IntRect, windowSize: IntSize, layoutDirection: LayoutDirection, popupContentSize: IntSize) = IntOffset.Zero
}
internal object PlainMenuStyle {
    val Radius = 16.dp
    val RowRadius = Radius - 6.dp
}

@Composable
internal fun PlainMenu(label: String, expanded: Boolean, dismiss: () -> Unit, width: Dp,
    anchor: androidx.compose.ui.geometry.Rect? = null, content: @Composable ColumnScope.() -> Unit) {
    val node = rememberLiquidNode()
    val binding = rememberOrigin(expanded, node)
    val origin = binding.source
    val bounds = origin?.visibleBounds
    val sourceVisible = origin == null || origin.node.attached && bounds != null && bounds.width > 0f && bounds.height > 0f
    LaunchedEffect(expanded) { if (!expanded) origin?.restoreFocus(binding.closingEpoch) }
    if (!expanded || !sourceVisible) return
    Popup(popupPositionProvider = MenuWindowPosition, onDismissRequest = dismiss,
        properties = PopupProperties(focusable = true, dismissOnBackPress = true, dismissOnClickOutside = true,
            clippingEnabled = false, usePlatformDefaultWidth = false)) {
        val opacity = remember { Animatable(0f) }
        LaunchedEffect(Unit) { opacity.animateTo(1f, tween(120)) }
        CompositionLocalProvider(LocalLiquidInteractive provides true) {
            Box(Modifier.fillMaxSize()) {
                Box(Modifier.matchParentSize().clickable(indication = null,
                    interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() }, onClick = dismiss))
                val density = LocalDensity.current.density
                val sourcePosition = origin?.node?.windowOrigin ?: anchor?.topLeft
                var layoutOrigin by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
                val shape = RoundedCornerShape(PlainMenuStyle.Radius)
                Layout(content = {
                    Column(Modifier.width(width.coerceAtLeast(160.dp)).heightIn(max = 320.dp)
                        .semantics { paneTitle = label }
                        .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent(PointerEventPass.Final).changes.forEach { it.consume() } } }
                        .shadow(8.dp, shape)
                        .background(ZorkColors.Canvas, shape)
                        .border(0.5.dp, ZorkColors.FieldBorder, shape)
                        .graphicsLayer { alpha = opacity.value }
                        .padding(6.dp), content = content)
                }, modifier = Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing.union(WindowInsets.ime))
                    .onGloballyPositioned { layoutOrigin = it.positionOnScreen() }) { children, constraints ->
                    val source = origin?.node
                    val offset = (sourcePosition ?: layoutOrigin) - layoutOrigin
                    val right = offset.x + (source?.target?.from?.width?.times(density) ?: anchor?.width ?: 0f)
                    val bottom = offset.y + (source?.target?.from?.height?.times(density) ?: anchor?.height ?: 0f)
                    val margin = (12f * density).toInt()
                    val gap = (8f * density).toInt()
                    val below = (constraints.maxHeight - bottom.toInt() - margin - gap).coerceAtLeast(0)
                    val above = (offset.y.toInt() - margin - gap).coerceAtLeast(0)
                    val placeAbove = below < (320f * density).toInt() && above > below
                    val available = if (placeAbove) above else below
                    val panel = children.single().measure(constraints.copy(
                        minWidth = 0, minHeight = 0,
                        maxHeight = available.coerceAtMost((320f * density).toInt()).coerceAtLeast(1)))
                    val x = if (layoutDirection == LayoutDirection.Ltr) offset.x else right - panel.width
                    val y = if (placeAbove) offset.y - panel.height - gap else bottom + gap
                    layout(constraints.maxWidth, constraints.maxHeight) {
                        panel.place(x.toInt().coerceIn(0, (constraints.maxWidth - panel.width).coerceAtLeast(0)),
                            y.toInt().coerceIn(0, (constraints.maxHeight - panel.height).coerceAtLeast(0)))
                    }
                }
            }
        }
    }
}

@Composable
internal fun LiquidDisclosure(title: String, expanded: Boolean, change: (Boolean) -> Unit, modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit) {
    val node = rememberLiquidNode()
    val density = LocalDensity.current.density
    val view = LocalView.current
    val scopeVisible = LocalLiquidVisible.current
    Column(modifier) {
        LiquidButton(title, Modifier.fillMaxWidth(), onClick = { change(!expanded) })
        Layout(content = {
            CompositionLocalProvider(LocalLiquidInteractive provides (expanded && node.inputReady),
                LocalLiquidVisible provides (scopeVisible && (expanded || node.alive))) {
            Column(Modifier.padding(16.dp).graphicsLayer { node.revision; alpha = node.contentOpacity }
                .semantics { if (!expanded) hideFromAccessibility() }.focusProperties { canFocus = expanded },
                verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
            }
        }, modifier = Modifier.fillMaxWidth().onGloballyPositioned { coordinates ->
            node.target?.let { target ->
                val origin = coordinates.positionInWindow()
                // Use expanded bounds even at zero height, so opening a visible
                // collapsed row can animate without an offscreen frame chain.
                val visible = scopeVisible && origin.x < view.width && origin.y < view.height &&
                    origin.x + coordinates.size.width > 0f && origin.y + target.to.height * density > 0f
                node.update(target.copy(visible = visible), density)
            }
        }.drawWithContent {
            node.revision
            node.prepareFirstDraw()
            if (node.alive || expanded) {
                drawPath(node.outline, ZorkColors.Canvas)
                clipPath(node.outline) { this@drawWithContent.drawContent() }
                drawPath(node.border, LiquidTokens.Outline)
            }
        }) { children, constraints ->
            val child = children.single().measure(constraints.copy(minHeight = 0, maxHeight = Constraints.Infinity))
            val w = (child.width / density).coerceAtLeast(2f)
            val h = (child.height / density).coerceAtLeast(2f)
            node.update(LiquidTarget(LiquidKind.Morph, LiquidPose(width = w, height = 2f, radius = 1f),
                LiquidPose(width = w, height = h, radius = LiquidTokens.FieldRadius.value), active = expanded,
                visible = scopeVisible && (node.target?.visible ?: true), anchorY = 0f), density)
            node.revision
            val visible = if (!expanded && !node.alive) 0 else (node.height * density).toInt().coerceAtLeast(0)
            layout(child.width, visible) { child.place(0, 0) }
        }
    }
}
