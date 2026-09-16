package surf.zork.android

import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.triStateToggleable
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.ProgressBarRangeInfo
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.progressBarRangeInfo
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** One retained trigger/menu pair for settings and interactive message fields. */
@Composable
internal fun LiquidChoiceField(
    label: String, text: String, options: List<Pair<String, String>>, selected: Set<String>,
    modifier: Modifier = Modifier, enabled: Boolean = true, closeOnSelect: Boolean = true,
    choose: (String) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    var width by remember { mutableStateOf(160.dp) }
    val density = LocalDensity.current
    LaunchedEffect(enabled, options.isEmpty()) { if (!enabled || options.isEmpty()) expanded = false }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
        Box {
            LiquidSelectTrigger(text, label, Modifier.fillMaxWidth().onSizeChanged {
                width = with(density) { it.width.toDp() }
            }, enabled && options.isNotEmpty(), onClick = { expanded = !expanded })
            LiquidMenu(expanded, { expanded = false }, width) {
                LazyColumn(userScrollEnabled = expanded && LocalLiquidInteractive.current) {
                    items(options, key = { it.first }) { (id, name) ->
                        LiquidMenuItem(name, id in selected, enabled && expanded, onClick = {
                            choose(id)
                            if (closeOnSelect) expanded = false
                        })
                    }
                }
            }
        }
    }
}

@Composable
internal fun LiquidCheckbox(label: String, checked: Boolean, change: (Boolean) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) =
    LiquidCheck(label, if (checked) ToggleableState.On else ToggleableState.Off, { change(!checked) }, modifier, enabled)

@Composable
internal fun LiquidCheck(label: String, state: ToggleableState, change: () -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val selected = state != ToggleableState.Off
    Row(modifier.heightIn(min = 48.dp).triStateToggleable(state, enabled = enabled && LocalLiquidInteractive.current, role = Role.Checkbox,
        interactionSource = interaction, indication = null, onClick = change),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        val node = rememberLiquidNode()
        Box(Modifier.padding(3.dp).size(18.dp).liquidSurface(node,
            if (selected) LiquidTokens.Accent else ZorkColors.Canvas,
            if (focused) LiquidTokens.Focus else if (selected) null else LiquidTokens.Outline) { w, h ->
            LiquidTarget(LiquidKind.Static, LiquidPose(width = w, height = h, radius = 5f))
        }, contentAlignment = Alignment.Center) {
            if (selected) Icon(painterResource(if (state == ToggleableState.Indeterminate) R.drawable.ic_minus else R.drawable.ic_check),
                null, Modifier.size(12.dp), tint = ZorkColors.Canvas)
        }
        Text(label, fontSize = 14.sp, color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}

@Composable
internal fun LiquidRadioGroup(options: List<Pair<String, String>>, selected: String, choose: (String) -> Unit, modifier: Modifier = Modifier, enabled: Boolean = true) {
    Column(modifier.selectableGroup()) {
        options.forEach { (id, label) ->
            val interaction = remember(id) { MutableInteractionSource() }
            val focused by interaction.collectIsFocusedAsState()
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).selectable(id == selected, enabled = enabled && LocalLiquidInteractive.current,
                role = Role.RadioButton, interactionSource = interaction, indication = null, onClick = { choose(id) }),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                val ring = rememberLiquidNode()
                Box(Modifier.padding(3.dp).size(18.dp).liquidSurface(ring, ZorkColors.Canvas,
                    if (focused) LiquidTokens.Focus else LiquidTokens.Outline) { w, h ->
                    LiquidTarget(LiquidKind.Static, LiquidPose(width = w, height = h, radius = h / 2f))
                }, contentAlignment = Alignment.Center) {
                    if (id == selected) LiquidCard(Modifier.size(8.dp), LiquidTokens.Accent, false, LiquidTokens.PillRadius) { }
                }
                Text(label, fontSize = 14.sp, color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
            }
        }
    }
}

/** Material3 supplies touch, keyboard and accessibility input; Rust draws it. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun LiquidSlider(
    label: String, value: Float, change: (Float) -> Unit, modifier: Modifier = Modifier,
    enabled: Boolean = true, range: ClosedFloatingPointRange<Float> = 0f..1f, steps: Int = 0,
) {
    val node = rememberLiquidNode()
    val interaction = remember { MutableInteractionSource() }
    val dragged by interaction.collectIsDraggedAsState()
    val pressed by interaction.collectIsPressedAsState()
    val focused by interaction.collectIsFocusedAsState()
    val fraction = if (range.endInclusive > range.start) ((value - range.start) / (range.endInclusive - range.start)).coerceIn(0f, 1f) else 0f
    Slider(value, change, enabled = enabled && LocalLiquidInteractive.current, valueRange = range, steps = steps,
        interactionSource = interaction,
        modifier = modifier.fillMaxWidth().heightIn(min = 48.dp).semantics { contentDescription = label }
            .liquidSurface(node, Color.Transparent, if (focused) LiquidTokens.Focus else null, clipContent = false) { w, h ->
                LiquidTarget(LiquidKind.Slider, LiquidPose(width = w, height = h), anchorX = fraction, active = enabled && (dragged || pressed),
                    border = if (focused) LiquidTokens.Border.value else 0f)
            }.drawWithContent {
                node.revision
                drawPath(node.track, ZorkColors.Border)
                drawPath(node.activeTrack, if (enabled) LiquidTokens.Accent else ZorkColors.Disabled)
                drawPath(node.outline, if (enabled) LiquidTokens.Accent else ZorkColors.Disabled)
                drawContent()
            },
        thumb = { Spacer(Modifier.size(20.dp)) },
        track = { Spacer(Modifier.fillMaxWidth().height(4.dp)) })
}

@Composable
internal fun LiquidProgress(value: Float, modifier: Modifier = Modifier, label: String = "进度") {
    val node = rememberLiquidNode()
    LiquidCard(modifier.fillMaxWidth().height(8.dp).semantics {
        contentDescription = label
        progressBarRangeInfo = ProgressBarRangeInfo(value.coerceIn(0f, 1f), 0f..1f)
    }, ZorkColors.Border, false, LiquidTokens.PillRadius) {
        Box(Modifier.fillMaxSize().liquidSurface(node, if (value > 0f) LiquidTokens.Accent else Color.Transparent, null, clipContent = false) { w, h ->
            LiquidTarget(LiquidKind.Target, LiquidPose(width = (w * value.coerceIn(0f, 1f)).coerceAtLeast(2f), height = h, radius = h / 2f),
                visible = value > 0f, border = 0f)
        })
    }
}

@Composable
internal fun LiquidBadge(label: String, modifier: Modifier = Modifier, emphasized: Boolean = false) {
    LiquidCard(modifier, if (emphasized) LiquidTokens.Accent else ZorkColors.Selected, false, LiquidTokens.PillRadius) {
        Text(label, Modifier.padding(horizontal = 10.dp, vertical = 4.dp), fontSize = 12.sp,
            color = if (emphasized) ZorkColors.Canvas else ZorkColors.Ink)
    }
}
