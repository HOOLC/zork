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
internal data class PickerChoice(val value: String, val options: List<Pair<String, String>>,
    /** Model value → connections (profile id, name) offering it, from core. */
    val connections: Map<String, List<Pair<String, String>>> = emptyMap(),
    val device: String = "")

internal fun JSONObject?.pickerChoice(field: String): PickerChoice {
    val choice = this?.optJSONObject(field) ?: JSONObject()
    val options = choice.optJSONArray("options").objects()
    return PickerChoice(choice.text("value"), options.map { it.text("value") to it.text("label", it.text("value")) },
        options.associate { o -> o.text("value") to o.optJSONArray("connections").objects().map { it.text("profile") to it.text("name") } },
        options.firstNotNullOfOrNull { it.text("device").takeIf(String::isNotBlank) }.orEmpty())
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
        val visible = model.options.filter { query.isBlank() || it.second.contains(query.trim(), ignoreCase = true) }
        // Group by connection when core names them; a model offered by several
        // connections appears under each, and choosing it there pins that connection.
        val groups: List<Pair<Pair<String, String>?, List<Pair<String, String>>>> =
            if (model.connections.values.all { it.isEmpty() }) listOf(null to visible)
            else visible.flatMap { option -> model.connections[option.first].orEmpty().map { it to option } }
                .groupBy({ it.first }, { it.second }).map { (connection, options) -> connection to options }
        val pinned = profile.value.takeIf { it.isNotBlank() && it != "auto" }
        Column(Modifier.fillMaxWidth().heightIn(max = 400.dp).verticalScroll(rememberScrollState())) {
            groups.forEach { (connection, options) ->
            if (connection != null) Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 2.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(connection.second, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle,
                    modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (model.device.isNotBlank()) {
                    DeviceMark(model.device, 14.dp)
                    Text(model.device, fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1)
                }
            }
            options.forEach { (value, label) ->
                val selected = value == model.value && (connection == null || pinned == null || pinned == connection.first)
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .background(if (selected) ZorkColors.Selected else ZorkColors.Canvas, ZorkShapes.Control)
                    .selectable(selected, enabled = enabled, role = Role.RadioButton) {
                        choose("model", value)
                        // Several connections offer it: pin the one chosen here; one offers it: let core decide.
                        if (connection != null && model.connections[value].orEmpty().size > 1 && profile.options.any { it.first == connection.first })
                            choose("profile", connection.first)
                    }
                    .padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(label, fontSize = 15.sp, fontWeight = if (selected) FontWeight.Medium else FontWeight.Normal,
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    if (selected) Glyph(R.drawable.ic_check, 16.dp, ZorkColors.Ink)
                }
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
