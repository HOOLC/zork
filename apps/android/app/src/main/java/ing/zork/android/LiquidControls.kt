package ing.zork.android

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.interaction.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.ClipOp
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionOnScreen
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt

private class LiquidMeasurement { var size = Size.Zero; var bounds = Rect.Zero }

/** Layout and native input stay fixed; only the Rust-produced paths change. */
@Composable
internal fun Modifier.liquidSurface(
    node: LiquidNode,
    fill: Color = ZorkColors.Canvas,
    stroke: Color? = LiquidTokens.Outline,
    clipContent: Boolean = true,
    target: (Float, Float) -> LiquidTarget,
): Modifier {
    val scale = LocalDensity.current.density
    val view = LocalView.current
    val scopeVisible = LocalLiquidVisible.current
    val current by rememberUpdatedState(target)
    val measurement = remember(node) { LiquidMeasurement() }
    fun update() {
        val size = measurement.size
        val bounds = measurement.bounds
        if (size.width < 2f * scale || size.height < 2f * scale) {
            node.target?.let { node.update(it.copy(visible = false), scale) }
            return
        }
        val next = current(size.width / scale, size.height / scale)
        val visible = bounds.width > 0f && bounds.height > 0f &&
            bounds.right > 0f && bounds.bottom > 0f && bounds.left < view.width && bounds.top < view.height
        node.update(next.copy(visible = next.visible && visible && scopeVisible, border = if (stroke == null) 0f else next.border), scale)
    }
    SideEffect { update() }
    return onGloballyPositioned { coordinates ->
        node.windowOrigin = coordinates.positionOnScreen()
        measurement.size = Size(coordinates.size.width.toFloat(), coordinates.size.height.toFloat())
        measurement.bounds = coordinates.boundsInWindow()
        node.visibleBounds = measurement.bounds.translate(node.windowOrigin - coordinates.positionInWindow())
        update()
    }.drawWithContent {
        node.revision
        node.prepareFirstDraw()
        if (node.hasPath) {
            if (fill != Color.Transparent) drawPath(node.outline, fill)
            if (clipContent) clipPath(node.outline) { this@drawWithContent.drawContent() } else drawContent()
            if (stroke != null) drawPath(node.border, stroke)
        } else drawContent()
    }
}

@Composable
internal fun LiquidCard(
    modifier: Modifier = Modifier,
    color: Color = ZorkColors.Canvas,
    outlined: Boolean = true,
    radius: Dp = LiquidTokens.CompactRadius,
    content: @Composable BoxScope.() -> Unit,
) {
    val node = rememberLiquidNode()
    Box(modifier.liquidSurface(node, color, if (outlined) LiquidTokens.Outline else null) { width, height ->
        LiquidTarget(LiquidKind.Static, LiquidPose(width = width, height = height, radius = radius.value),
            border = if (outlined) LiquidTokens.Border.value else 0f)
    }, content = content)
}

@Composable
private fun rememberPressure(node: LiquidNode, interaction: MutableInteractionSource, enabled: Boolean): State<Boolean> {
    val down = remember { mutableStateOf(false) }
    val currentEnabled by rememberUpdatedState(enabled)
    LaunchedEffect(node, interaction) {
        interaction.interactions.collect { event ->
            if (event is PressInteraction.Press && event.pressPosition.x.isFinite() && event.pressPosition.y.isFinite()) node.target?.let {
                node.pressAnchorX = (event.pressPosition.x / (it.from.width * node.density)).coerceIn(0f, 1f)
                node.pressAnchorY = (event.pressPosition.y / (it.from.height * node.density)).coerceIn(0f, 1f)
            }
            val pressed = when (event) {
                is PressInteraction.Press -> currentEnabled
                is PressInteraction.Release, is PressInteraction.Cancel -> false
                else -> return@collect
            }
            down.value = pressed
            node.target?.let { node.update(it.copy(active = pressed, anchorX = node.pressAnchorX, anchorY = node.pressAnchorY), tap = event is PressInteraction.Release) }
        }
    }
    LaunchedEffect(enabled) {
        if (!enabled && down.value) {
            down.value = false
            node.target?.let { node.update(it.copy(active = false), tap = true) }
        }
    }
    return down
}

@Composable
internal fun LiquidButton(
    text: String,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    enabled: Boolean = true,
    quiet: Boolean = false,
    onClick: () -> Unit,
    leading: (@Composable () -> Unit)? = null,
) {
    LiquidAction(modifier, primary, enabled, quiet, false, LiquidTokens.PillRadius, onClick) {
        Row(Modifier.padding(horizontal = 18.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            leading?.invoke()
            Text(text, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 14.sp)
        }
    }
}

@Composable
internal fun LiquidIconButton(
    label: String, modifier: Modifier = Modifier, enabled: Boolean = true,
    primary: Boolean = false, opensPanel: Boolean = false, onClick: () -> Unit,
    content: @Composable () -> Unit,
) {
    LiquidAction(modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp).semantics { contentDescription = label },
        primary, enabled, true, opensPanel, LiquidTokens.IconRadius, onClick) {
        Box(Modifier.padding(12.dp), contentAlignment = Alignment.Center) { content() }
    }
}

@Composable
internal fun LiquidSelectTrigger(text: String, label: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    LiquidAction(modifier.semantics { contentDescription = label }, false, enabled, false, true, LiquidTokens.FieldRadius, onClick) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(text, Modifier.weight(1f), fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Glyph(R.drawable.ic_chevron_down, 16.dp, ZorkColors.Muted)
        }
    }
}

@Composable
internal fun LiquidMenuItem(text: String, selected: Boolean, enabled: Boolean = true, onClick: () -> Unit) {
    LiquidAction(Modifier.fillMaxWidth(), false, enabled, true, false,
        (LiquidTokens.FieldRadius.value - 6f).coerceAtLeast(0f).dp, onClick) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(text, Modifier.weight(1f), fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Box(Modifier.size(16.dp)) { if (selected) Glyph(R.drawable.ic_check, 16.dp, LiquidTokens.Accent) }
        }
    }
}

@Composable
internal fun LiquidListRow(modifier: Modifier = Modifier, onClick: (() -> Unit)? = null,
    content: @Composable RowScope.() -> Unit) {
    val row: @Composable () -> Unit = {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp), content = content)
    }
    if (onClick == null) Box(modifier, contentAlignment = Alignment.CenterStart) { row() }
    else LiquidAction(modifier, false, true, true, false, 0.dp, onClick, row)
}

@Composable
internal fun LiquidAction(
    modifier: Modifier, primary: Boolean, enabled: Boolean, quiet: Boolean, opensPanel: Boolean,
    radius: Dp, onClick: () -> Unit, content: @Composable () -> Unit,
) {
    val ink = when { !enabled -> ZorkColors.Disabled; primary -> ZorkColors.Canvas; else -> ZorkColors.Ink }
    Box(modifier.liquidPressable(primary, enabled, quiet, opensPanel, radius, onClick = onClick), contentAlignment = Alignment.Center) {
        CompositionLocalProvider(LocalContentColor provides ink, content = content)
    }
}

/** The same complete press/focus/source behavior for content-bearing rows. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun Modifier.liquidPressable(
    primary: Boolean = false, enabled: Boolean = true, quiet: Boolean = true,
    opensPanel: Boolean = false, radius: Dp = 6.dp,
    interactionSource: MutableInteractionSource? = null, onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
): Modifier {
    val node = rememberLiquidNode()
    val view = LocalView.current
    val drawing = rememberGraphicsLayer()
    val origin = remember(node, view) { LiquidOrigin(node, view, drawing) }
    DisposableEffect(origin) { onDispose { origin.cancelRestore() } }
    val interaction = interactionSource ?: remember { MutableInteractionSource() }
    val interactive = enabled && LocalLiquidInteractive.current
    val pressed by rememberPressure(node, interaction, enabled)
    val hovered by interaction.collectIsHoveredAsState()
    val focused by interaction.collectIsFocusedAsState()
    val feedback = enabled && (pressed || node.pressure)
    val fill = when {
        !enabled && primary -> LiquidTokens.NeutralPressed
        primary && feedback -> LiquidTokens.AccentPressed
        primary && hovered -> LiquidTokens.AccentHover
        primary -> LiquidTokens.Accent
        quiet && !opensPanel && feedback -> LiquidTokens.NeutralPressed
        quiet && !opensPanel && hovered -> LiquidTokens.NeutralHover
        else -> Color.Transparent
    }
    val border = when {
        focused && enabled -> LiquidTokens.Focus
        !quiet || opensPanel && (hovered || feedback) -> LiquidTokens.Outline
        else -> null
    }
    return defaultMinSize(minWidth = 48.dp, minHeight = 48.dp)
        .drawWithContent {
            if (!origin.relocated || !origin.recorded) {
                drawing.record { this@drawWithContent.drawContent() }
                origin.recorded = true
            }
            if (!origin.relocated) drawLayer(drawing)
        }
        .liquidSurface(node, fill, border) { width, height ->
            LiquidTarget(LiquidKind.Press, LiquidPose(width = width, height = height, radius = radius.value), active = enabled && pressed,
                anchorX = node.pressAnchorX, anchorY = node.pressAnchorY)
        }.focusRequester(origin.focus)
        .focusProperties { canFocus = interactive }
        .combinedClickable(interactionSource = interaction, indication = null, enabled = interactive,
            role = Role.Button, onLongClick = onLongClick?.let { action -> {
                node.host.activate(origin)
                action()
            } }, onClick = {
                node.host.activate(origin)
                node.target?.let { node.update(it, tap = true) }
                onClick()
            })
}

@Composable
internal fun LiquidTextField(
    label: String, value: String, change: (String) -> Unit, modifier: Modifier = Modifier,
    secret: Boolean = false, enabled: Boolean = true, readOnly: Boolean = false,
    error: String? = null, detail: String? = null, singleLine: Boolean = true,
    minLines: Int = 1, maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
    placeholder: (@Composable () -> Unit)? = null,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    keyboardActions: androidx.compose.foundation.text.KeyboardActions = androidx.compose.foundation.text.KeyboardActions.Default,
) {
    val node = rememberLiquidNode()
    var focused by remember { mutableStateOf(false) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (label.isNotEmpty()) Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
        OutlinedTextField(value, change, enabled = enabled && LocalLiquidInteractive.current, readOnly = readOnly, singleLine = singleLine,
            minLines = minLines, maxLines = maxLines,
            placeholder = placeholder, keyboardOptions = keyboardOptions, keyboardActions = keyboardActions,
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label }
                .onFocusChanged { focused = it.isFocused }
                .liquidSurface(node, ZorkColors.Canvas,
                    if (error != null) ZorkColors.Danger else if (focused) LiquidTokens.Focus else LiquidTokens.Outline) { width, height ->
                    LiquidTarget(LiquidKind.Static, LiquidPose(width = width, height = height, radius = LiquidTokens.FieldRadius.value))
                },
            isError = error != null, shape = LiquidShape(LiquidTokens.FieldRadius),
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            textStyle = LocalTextStyle.current.copy(fontSize = 15.sp),
            colors = OutlinedTextFieldDefaults.colors(
                focusedBorderColor = Color.Transparent, unfocusedBorderColor = Color.Transparent,
                disabledBorderColor = Color.Transparent, errorBorderColor = Color.Transparent,
                focusedContainerColor = Color.Transparent, unfocusedContainerColor = Color.Transparent,
                disabledContainerColor = Color.Transparent, errorContainerColor = Color.Transparent))
        (error ?: detail)?.let { Text(it, fontSize = 12.sp, color = if (error != null) ZorkColors.Danger else ZorkColors.Muted) }
    }
}

@Composable
internal fun LiquidSwitch(checked: Boolean, change: (Boolean) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val track = rememberLiquidNode()
    val thumb = rememberLiquidNode()
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    Box(modifier.sizeIn(minWidth = 56.dp, minHeight = 48.dp)
        .toggleable(checked, enabled = enabled && LocalLiquidInteractive.current, role = Role.Switch, interactionSource = interaction, indication = null, onValueChange = change),
        contentAlignment = Alignment.Center) {
        Box(Modifier.size(49.dp, 32.dp)
            .liquidSurface(track, if (checked) ZorkColors.Ink else ZorkColors.Disabled, if (focused) LiquidTokens.Focus else null) { w, h ->
                LiquidTarget(LiquidKind.Static, LiquidPose(width = w, height = h, radius = h / 2f))
            }
            .liquidSurface(thumb, ZorkColors.Canvas, null, clipContent = false) { w, h ->
                LiquidTarget(LiquidKind.Toggle, LiquidPose(width = w, height = h, radius = h / 2f), active = checked, border = 0f)
            })
    }
}

@Composable
internal fun LiquidSegments(
    options: List<Pair<String, String>>, selected: String, choose: (String) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true,
) {
    if (options.isEmpty()) return
    val base = rememberLiquidNode()
    val selection = rememberLiquidNode()
    val selectedIndex = options.indexOfFirst { it.first == selected }
    var focused by remember { mutableStateOf(false) }
    Row(modifier.fillMaxWidth().height(48.dp).selectableGroup()
        .onFocusChanged { focused = it.hasFocus }.focusGroup()
        .liquidSurface(base, ZorkColors.Canvas, if (focused) LiquidTokens.Focus else LiquidTokens.Outline) { w, h ->
            LiquidTarget(LiquidKind.Static, LiquidPose(width = w, height = h, radius = h / 2f))
        }
        .liquidSurface(selection, if (selectedIndex >= 0) ZorkColors.Ink else Color.Transparent, null, clipContent = false) { w, h ->
            LiquidTarget(LiquidKind.Segment, LiquidPose(width = w, height = h, radius = h / 2f),
                count = options.size, selected = selectedIndex.coerceAtLeast(0), border = 0f, visible = selectedIndex >= 0)
        }.padding(horizontal = 4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        options.forEach { (id, label) ->
            val interaction = remember(id) { MutableInteractionSource() }
            Box(Modifier.weight(1f).fillMaxHeight().selectable(id == selected, enabled = enabled && LocalLiquidInteractive.current,
                role = Role.Tab, interactionSource = interaction, indication = null, onClick = { choose(id) }), contentAlignment = Alignment.Center) {
                LiquidContrastLabel(label, selection, selectedIndex >= 0, enabled, Modifier.fillMaxSize())
            }
        }
    }
}

/** Both colors use the same shaped text and the current shared contour. */
@Composable
internal fun LiquidContrastLabel(label: String, selection: LiquidNode, selected: Boolean, enabled: Boolean, modifier: Modifier = Modifier) {
    val measurer = rememberTextMeasurer(cacheSize = 16)
    val style = LocalTextStyle.current.merge(TextStyle(fontSize = 14.sp))
    val position = remember { arrayOf(Offset.Zero) }
    Canvas(modifier.semantics { contentDescription = label }.onGloballyPositioned { position[0] = it.positionOnScreen() }
        .drawWithCache {
            val layout = measurer.measure(label, style, overflow = TextOverflow.Ellipsis, maxLines = 1,
                constraints = Constraints(maxWidth = (size.width - 12.dp.toPx()).roundToInt().coerceAtLeast(0)))
            val text = Offset((size.width - layout.size.width) / 2f, (size.height - layout.size.height) / 2f)
            onDrawBehind {
                selection.revision
                val color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled
                if (!selected || !selection.hasPath) drawText(layout, color, text)
                else {
                    val delta = selection.windowOrigin - position[0]
                    translate(delta.x, delta.y) {
                        clipPath(selection.outline, ClipOp.Difference) { drawText(layout, color, text - delta) }
                        clipPath(selection.outline) { drawText(layout, Color.White, text - delta) }
                    }
                }
            }
        }) { }
}
