package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** A model choice as core presents it: current value plus its native options. */
internal data class PickerChoice(val value: String, val options: List<Pair<String, String>>)

internal fun JSONObject?.pickerChoice(field: String): PickerChoice {
    val choice = this?.optJSONObject(field) ?: JSONObject()
    return PickerChoice(choice.text("value"), choice.optJSONArray("options").objects().map { it.text("value") to it.text("label", it.text("value")) })
}

/** Thinking values keep each provider's own names; only "off" gets a Chinese word. */
internal fun thinkingLabel(value: String) = when (value) { "off", "none", "disabled" -> "关"; else -> value }

/** The composer's model capsule: model and current thinking value, clearly tappable. */
@Composable
internal fun ModelCapsule(model: PickerChoice, thinking: PickerChoice, enabled: Boolean, open: () -> Unit) {
    val modelLabel = model.options.find { it.first == model.value }?.second ?: model.value.ifBlank { "选择模型" }
    Row(Modifier.heightIn(min = 44.dp).clipToCapsule()
        .selectable(false, enabled = enabled, role = Role.Button, onClick = open)
        .semantics { contentDescription = "选择模型" }
        .padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(modelLabel, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
            color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled, modifier = Modifier.widthIn(max = 200.dp))
        if (thinking.value.isNotBlank() && thinking.options.size > 1) Text("· ${thinkingLabel(thinking.value)}", fontSize = 13.sp, color = ZorkColors.Subtle, maxLines = 1)
        Glyph(R.drawable.ic_chevron_down, 14.dp, ZorkColors.Subtle)
    }
}

private fun Modifier.clipToCapsule() = this.then(Modifier.background(ZorkColors.Prompt, ZorkShapes.Control))

/**
 * One sheet chooses everything: connection (when there is a choice), model, and the
 * selected model's own thinking options. No slider and no fixed scale — a model with
 * one or no thinking option shows no thinking row at all.
 */
@Composable
internal fun ModelPickerSheet(open: Boolean, model: PickerChoice, thinking: PickerChoice, profile: PickerChoice,
    enabled: Boolean, choose: (String, String) -> Unit, dismiss: () -> Unit, onClosed: () -> Unit = dismiss) {
    var query by remember { mutableStateOf("") }
    ZorkSheet(open, "模型", dismiss, onClosed = onClosed) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("模型", fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            ZorkButton("完成", quiet = true, onClick = dismiss)
        }
        if (model.options.size > 8) ZorkTextField("", query, { query = it }, placeholder = { Text("搜索模型", fontSize = 14.sp, color = ZorkColors.Subtle) })
        if (profile.options.size > 1) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            profile.options.forEach { (value, label) ->
                PickerChip(if (value == "auto") "自动" else label, value == profile.value, enabled) { choose("profile", value) }
            }
        }
        val visible = model.options.filter { query.isBlank() || it.second.contains(query.trim(), ignoreCase = true) }
        Column(Modifier.fillMaxWidth().heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
            visible.forEach { (value, label) ->
                val selected = value == model.value
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .background(if (selected) ZorkColors.Selected else ZorkColors.Canvas, ZorkShapes.Control)
                    .selectable(selected, enabled = enabled, role = Role.RadioButton) { choose("model", value) }
                    .padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(label, fontSize = 15.sp, fontWeight = if (selected) FontWeight.Medium else FontWeight.Normal,
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    if (selected) Glyph(R.drawable.ic_check, 16.dp, ZorkColors.Ink)
                }
            }
            if (visible.isEmpty()) Text("没有匹配的模型", fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.padding(16.dp))
        }
        if (thinking.options.size > 1) Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("思考", fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle, modifier = Modifier.padding(start = 4.dp))
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                thinking.options.forEach { (value, _) ->
                    PickerChip(thinkingLabel(value), value == thinking.value, enabled) { choose("thinking", value) }
                }
            }
        }
    }
}

@Composable
internal fun PickerChip(label: String, selected: Boolean, enabled: Boolean = true, click: () -> Unit) {
    Box(Modifier.heightIn(min = 44.dp).background(if (selected) ZorkColors.Ink else ZorkColors.Prompt, ZorkShapes.Control)
        .selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = click)
        .padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1,
            color = if (selected) ZorkColors.Canvas else if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}
