package ing.zork.android

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
internal fun ZorkCard(
    modifier: Modifier = Modifier,
    color: Color = ZorkColors.Canvas,
    outlined: Boolean = true,
    radius: Dp = UiTokens.CompactRadius,
    shape: androidx.compose.ui.graphics.Shape = RoundedCornerShape(radius),
    content: @Composable BoxScope.() -> Unit,
) {
    Surface(modifier, shape = shape, color = color,
        border = if (outlined) androidx.compose.foundation.BorderStroke(UiTokens.Border, UiTokens.Outline) else null) {
        Box(content = content)
    }
}

@Composable
internal fun ZorkButton(
    text: String,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    enabled: Boolean = true,
    quiet: Boolean = false,
    danger: Boolean = false,
    onClick: () -> Unit,
    leading: (@Composable () -> Unit)? = null,
) {
    val content: @Composable RowScope.() -> Unit = {
        leading?.invoke()
        Text(text, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 14.sp)
    }
    val colors = ButtonDefaults.buttonColors(
        // Primary actions are ink; persimmon is reserved for sending and work in progress.
        containerColor = if (danger) ZorkColors.Danger else if (primary) ZorkColors.Ink else if (quiet) Color.Transparent else ZorkColors.Canvas,
        contentColor = if (primary || danger) ZorkColors.Canvas else ZorkColors.Ink,
        disabledContainerColor = if (primary || danger) ZorkColors.Pressed else Color.Transparent,
        disabledContentColor = ZorkColors.Disabled,
    )
    val shape = RoundedCornerShape(UiTokens.PillRadius)
    if (quiet) TextButton(onClick, modifier.heightIn(min = 48.dp), enabled = enabled, shape = shape,
        colors = colors, content = content)
    else Button(onClick, modifier.heightIn(min = 48.dp), enabled = enabled, shape = shape,
        colors = colors,
        border = if (primary || danger) null else androidx.compose.foundation.BorderStroke(UiTokens.Border, UiTokens.Outline),
        content = content)
}

@Composable
internal fun ZorkIconButton(
    label: String, modifier: Modifier = Modifier, enabled: Boolean = true,
    primary: Boolean = false, opensPanel: Boolean = false, onClick: () -> Unit,
    content: @Composable () -> Unit,
) {
    val shape = RoundedCornerShape(UiTokens.IconRadius)
    val colors = IconButtonDefaults.iconButtonColors(
        containerColor = if (primary) UiTokens.Accent else Color.Transparent,
        contentColor = if (primary) ZorkColors.Canvas else ZorkColors.Ink,
        disabledContentColor = ZorkColors.Disabled)
    IconButton(onClick, modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp)
        .semantics { contentDescription = label }
        .then(if (opensPanel) Modifier.border(UiTokens.Border, UiTokens.Outline, shape) else Modifier),
        enabled = enabled, colors = colors, content = content)
}

@Composable
internal fun ZorkSelectTrigger(
    text: String, label: String, modifier: Modifier = Modifier, enabled: Boolean = true,
    expanded: Boolean = false, onClick: () -> Unit,
) {
    OutlinedButton(onClick, modifier.heightIn(min = 48.dp)
        .semantics { contentDescription = "$label：$text"; stateDescription = if (expanded) "已展开" else "已收起" },
        enabled = enabled, shape = RoundedCornerShape(UiTokens.FieldRadius),
        contentPadding = PaddingValues(horizontal = 14.dp)) {
        Text(text, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 14.sp)
        Glyph(R.drawable.ic_chevron_down, 16.dp, ZorkColors.Muted)
    }
}

@Composable
internal fun ZorkMenuItem(
    text: String, selected: Boolean, enabled: Boolean = true,
    choice: Boolean = false, multiple: Boolean = false, onClick: () -> Unit,
) {
    DropdownMenuItem(
        text = { Text(text, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 14.sp) },
        onClick = onClick, enabled = enabled,
        modifier = Modifier.then(if (choice && selected) Modifier.background(ZorkColors.Selected,
            RoundedCornerShape(PlainMenuStyle.RowRadius)) else Modifier)
            .then(if (choice) Modifier.semantics {
                stateDescription = if (selected) "已选中" else "未选中"
            } else Modifier),
        trailingIcon = { if (selected) Glyph(R.drawable.ic_check, 16.dp, UiTokens.Accent) },
    )
}

@Composable
internal fun ZorkListRow(
    modifier: Modifier = Modifier, onClick: (() -> Unit)? = null,
    content: @Composable RowScope.() -> Unit,
) {
    val body: @Composable () -> Unit = {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp), content = content)
    }
    if (onClick == null) Box(modifier, contentAlignment = Alignment.CenterStart) { body() }
    else Surface(onClick = onClick, modifier = modifier, color = Color.Transparent) { body() }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun Modifier.zorkPressable(
    enabled: Boolean = true, role: Role = Role.Button,
    interactionSource: MutableInteractionSource? = null, onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
): Modifier = this.combinedClickable(
    interactionSource = interactionSource ?: remember { MutableInteractionSource() },
    indication = androidx.compose.foundation.LocalIndication.current, enabled = enabled, role = role,
    onLongClick = onLongClick, onClick = onClick)

@Composable
internal fun ZorkTextField(
    label: String, value: String, change: (String) -> Unit, modifier: Modifier = Modifier,
    secret: Boolean = false, enabled: Boolean = true, readOnly: Boolean = false,
    error: String? = null, detail: String? = null, singleLine: Boolean = true,
    minLines: Int = 1, maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
    placeholder: (@Composable () -> Unit)? = null,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    keyboardActions: androidx.compose.foundation.text.KeyboardActions = androidx.compose.foundation.text.KeyboardActions.Default,
) {
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (label.isNotEmpty()) Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
        OutlinedTextField(value, change, enabled = enabled, readOnly = readOnly, singleLine = singleLine,
            minLines = minLines, maxLines = maxLines, placeholder = placeholder,
            keyboardOptions = keyboardOptions, keyboardActions = keyboardActions,
            modifier = Modifier.fillMaxWidth().semantics { contentDescription = label },
            isError = error != null, shape = RoundedCornerShape(UiTokens.FieldRadius),
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            textStyle = LocalTextStyle.current.copy(fontSize = 15.sp))
        (error ?: detail)?.let { Text(it, fontSize = 12.sp,
            color = if (error != null) ZorkColors.Danger else ZorkColors.Muted) }
    }
}

/**
 * The switch thumb and track colour move on the move curve; state never animates
 * on first composition. Reduced motion jumps straight to the new position.
 */
@Composable
internal fun ZorkSwitch(
    checked: Boolean, change: (Boolean) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true,
) {
    val reduced = LocalReducedMotion.current
    val position by androidx.compose.animation.core.animateFloatAsState(if (checked) 1f else 0f,
        if (reduced) androidx.compose.animation.core.snap() else ZorkMotion.move(ZorkMotion.FAST), label = "switch")
    val track = androidx.compose.ui.graphics.lerp(ZorkColors.Border, ZorkColors.Ink, position)
    Box(modifier.sizeIn(minWidth = 56.dp, minHeight = 48.dp)
        .toggleableSwitch(checked, enabled, change), contentAlignment = Alignment.Center) {
        Box(Modifier.size(44.dp, 26.dp).background(if (enabled) track else ZorkColors.Border, ZorkShapes.Control)) {
            Box(Modifier.padding(3.dp).size(20.dp)
                .graphicsLayer { translationX = position * 18.dp.toPx() }
                .background(ZorkColors.Canvas, androidx.compose.foundation.shape.CircleShape))
        }
    }
}

private fun Modifier.toggleableSwitch(checked: Boolean, enabled: Boolean, change: (Boolean) -> Unit) =
    this.then(Modifier.toggleable(checked, enabled = enabled, role = Role.Switch, onValueChange = change))

/**
 * Segmented choice: the selected capsule slides between options on the move curve.
 * Options share the width equally, so positions come from the index alone.
 */
@Composable
internal fun ZorkSegments(
    options: List<Pair<String, String>>, selected: String, choose: (String) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true,
) {
    if (options.isEmpty()) return
    val reduced = LocalReducedMotion.current
    val index = options.indexOfFirst { it.first == selected }
    val slot by androidx.compose.animation.core.animateFloatAsState(index.coerceAtLeast(0).toFloat(),
        if (reduced) androidx.compose.animation.core.snap() else ZorkMotion.move(ZorkMotion.BASE), label = "segment")
    BoxWithConstraints(modifier.fillMaxWidth().heightIn(min = 48.dp).background(ZorkColors.Prompt, ZorkShapes.Control).padding(3.dp)) {
        val width = maxWidth / options.size
        if (index >= 0) Box(Modifier.width(width).fillMaxHeight().heightIn(min = 42.dp)
            .graphicsLayer { translationX = slot * width.toPx() }
            .background(ZorkColors.Canvas, ZorkShapes.Control))
        Row(Modifier.fillMaxWidth()) {
            options.forEach { (id, label) ->
                val on = id == selected
                Box(Modifier.weight(1f).heightIn(min = 42.dp).clip(ZorkShapes.Control)
                    .selectable(on, enabled = enabled, role = Role.RadioButton) { if (!on) choose(id) },
                    contentAlignment = Alignment.Center) {
                    Text(label, maxLines = 1, fontSize = 14.sp, fontWeight = if (on) androidx.compose.ui.text.font.FontWeight.Medium else androidx.compose.ui.text.font.FontWeight.Normal,
                        color = if (!enabled) ZorkColors.Disabled else if (on) ZorkColors.Ink else ZorkColors.Muted)
                }
            }
        }
    }
}
