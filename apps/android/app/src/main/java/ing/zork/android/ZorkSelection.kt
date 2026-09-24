package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.triStateToggleable
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
internal fun ZorkChoiceField(
    label: String, text: String, options: List<Pair<String, String>>, selected: Set<String>,
    modifier: Modifier = Modifier, enabled: Boolean = true, closeOnSelect: Boolean = true,
    choose: (String) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    LaunchedEffect(enabled, options.map { it.first }) { expanded = false }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
        Box {
            var width by remember { mutableStateOf(0.dp) }
            val density = androidx.compose.ui.platform.LocalDensity.current
            ZorkSelectTrigger(text, label, Modifier.fillMaxWidth().onGloballyPositioned { width = with(density) { it.size.width.toDp() } },
                enabled && options.isNotEmpty(), expanded = expanded, onClick = { expanded = !expanded })
            PlainMenu(label, expanded, { expanded = false }, width) {
                options.forEach { (id, name) ->
                    ZorkMenuItem(name, id in selected, enabled, choice = true,
                        multiple = !closeOnSelect, onClick = {
                            choose(id)
                            if (closeOnSelect) expanded = false
                        })
                }
            }
        }
    }
}

@Composable
internal fun ZorkCheckbox(
    label: String, checked: Boolean, change: (Boolean) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true,
) = ZorkCheck(label, if (checked) ToggleableState.On else ToggleableState.Off,
    { change(!checked) }, modifier, enabled)

@Composable
internal fun ZorkCheck(
    label: String, state: ToggleableState, change: () -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true,
) {
    Row(modifier.heightIn(min = 48.dp).triStateToggleable(state, enabled = enabled,
        role = Role.Checkbox, onClick = change), verticalAlignment = Alignment.CenterVertically) {
        TriStateCheckbox(state, onClick = null, enabled = enabled)
        Spacer(Modifier.width(6.dp))
        Text(label, fontSize = 14.sp, color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}

@Composable
internal fun ZorkRadioGroup(
    options: List<Pair<String, String>>, selected: String, choose: (String) -> Unit,
    modifier: Modifier = Modifier, enabled: Boolean = true,
) {
    Column(modifier.selectableGroup()) {
        options.forEach { (id, label) ->
            Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                .selectable(id == selected, enabled = enabled, role = Role.RadioButton,
                    onClick = { choose(id) }), verticalAlignment = Alignment.CenterVertically) {
                RadioButton(id == selected, onClick = null, enabled = enabled)
                Spacer(Modifier.width(6.dp))
                Text(label, fontSize = 14.sp,
                    color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
            }
        }
    }
}

@Composable
internal fun ZorkSlider(
    label: String, value: Float, change: (Float) -> Unit, modifier: Modifier = Modifier,
    enabled: Boolean = true, range: ClosedFloatingPointRange<Float> = 0f..1f, steps: Int = 0,
) {
    Slider(value, change, modifier.fillMaxWidth().heightIn(min = 48.dp)
        .semantics { contentDescription = label }, enabled = enabled, valueRange = range,
        steps = steps)
}

@Composable
internal fun ZorkProgress(value: Float, modifier: Modifier = Modifier, label: String = "进度") {
    LinearProgressIndicator(progress = { value.coerceIn(0f, 1f) }, modifier.fillMaxWidth().height(8.dp)
        .semantics { contentDescription = label }, color = UiTokens.Accent,
        trackColor = ZorkColors.Border)
}

@Composable
internal fun ZorkBadge(label: String, modifier: Modifier = Modifier, emphasized: Boolean = false) {
    Badge(modifier, containerColor = if (emphasized) UiTokens.Accent else ZorkColors.Selected,
        contentColor = if (emphasized) ZorkColors.Canvas else ZorkColors.Ink) {
        Text(label, Modifier.padding(horizontal = 6.dp, vertical = 2.dp), fontSize = 12.sp)
    }
}
