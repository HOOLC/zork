package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** A model choice as core presents it: current value plus its native options. */
internal data class PickerChoice(val value: String, val options: List<Pair<String, String>>,
    /** Model value → connections (profile id, name) offering it, from core. */
    val connections: Map<String, List<Pair<String, String>>> = emptyMap(),
    val device: String = "",
    /** Model value → maker key from core (absent: unknown maker, generic mark). */
    val makers: Map<String, String> = emptyMap(),
    /** Connection profile id → provider, for the connection's own mark. */
    val providers: Map<String, String> = emptyMap()) {
    /** The pinned connection's (id, name); null while the connection is automatic. */
    val pinned: Pair<String, String>? get() = value.takeIf { it.isNotBlank() && it != "auto" }
        ?.let { id -> options.find { it.first == id } }
}

internal fun JSONObject?.pickerChoice(field: String): PickerChoice {
    val choice = this?.optJSONObject(field) ?: JSONObject()
    val options = choice.optJSONArray("options").objects()
    return PickerChoice(choice.text("value"), options.map { it.text("value") to it.text("label", it.text("value")) },
        options.associate { o -> o.text("value") to o.optJSONArray("connections").objects().map { it.text("profile") to it.text("name") } },
        options.firstNotNullOfOrNull { it.text("device").takeIf(String::isNotBlank) }.orEmpty(),
        options.mapNotNull { o -> o.text("maker").takeIf(String::isNotBlank)?.let { o.text("value") to it } }.toMap(),
        // Model options name their connections' providers; connection options their own.
        options.flatMap { o -> o.optJSONArray("connections").objects() }.filter { it.text("provider").isNotBlank() }
            .associate { it.text("profile") to it.text("provider") } +
            options.filter { it.text("provider").isNotBlank() }.associate { it.text("value") to it.text("provider") })
}

/** Thinking values keep each provider's own names; only "off" gets a Chinese word. */
internal fun thinkingLabel(value: String) = when (value) { "off", "none", "disabled" -> "关"; else -> value }

/** The composer's model capsule: model and current thinking value, clearly tappable. */
@Composable
internal fun ModelCapsule(model: PickerChoice, thinking: PickerChoice, enabled: Boolean, profile: PickerChoice = PickerChoice("", emptyList()), open: () -> Unit) {
    val modelLabel = model.options.find { it.first == model.value }?.second ?: model.value.ifBlank { "选择模型" }
    Row(Modifier.heightIn(min = 44.dp).clipToCapsule()
        .selectable(false, enabled = enabled, role = Role.Button, onClick = open)
        // Screen readers hear the current choice, the pinned connection included.
        .semantics { contentDescription = "选择模型"; stateDescription = listOfNotNull(modelLabel,
            thinking.value.takeIf { it.isNotBlank() && thinking.options.size > 1 }?.let(::thinkingLabel), profile.pinned?.second).joinToString(" · ") }
        .padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        // The capsule names a model: its maker's mark, not the connection's.
        if (model.options.any { it.first == model.value }) MakerMark(model.makers[model.value], 16, if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
        Text(modelLabel, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
            color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled, modifier = Modifier.widthIn(max = 200.dp))
        if (thinking.value.isNotBlank() && thinking.options.size > 1) Text("· ${thinkingLabel(thinking.value)}", fontSize = 13.sp, color = ZorkColors.Subtle, maxLines = 1)
        // The connection shows only when pinned; automatic is the default.
        profile.pinned?.let { Text("· ${it.second}", fontSize = 13.sp, color = ZorkColors.Subtle, maxLines = 1,
            overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 96.dp)) }
        Glyph(R.drawable.ic_chevron_down, 14.dp, ZorkColors.Subtle)
    }
}

private fun Modifier.clipToCapsule() = this.then(Modifier.background(ZorkColors.Prompt, ZorkShapes.Control))

/**
 * One sheet picks a model (each listed once, whichever connections serve it) and
 * the selected model's own thinking options. The connection is optional: automatic
 * by default, and pinnable to a connection serving the model. No slider and no
 * fixed scale — a model with one or no thinking option shows no thinking row.
 */
@Composable
internal fun ModelPickerSheet(open: Boolean, model: PickerChoice, thinking: PickerChoice, profile: PickerChoice,
    enabled: Boolean, choose: (String, String) -> Unit, dismiss: () -> Unit, onClosed: () -> Unit = dismiss) {
    var query by remember { mutableStateOf("") }
    var connections by remember { mutableStateOf(false) }
    ZorkSheet(open, "模型", dismiss, onClosed = onClosed) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("模型", fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            ZorkButton("完成", quiet = true, onClick = dismiss)
        }
        if (model.options.size > 8) ZorkTextField("", query, { query = it }, placeholder = { Text("搜索模型", fontSize = 14.sp, color = ZorkColors.Subtle) })
        val visible = model.options.filter { query.isBlank() || it.second.contains(query.trim(), ignoreCase = true) }
        Column(Modifier.fillMaxWidth().heightIn(max = 320.dp).verticalScroll(rememberScrollState())) {
            visible.forEach { (value, label) ->
                val selected = value == model.value
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                    .background(if (selected) ZorkColors.Selected else ZorkColors.Canvas, ZorkShapes.Control)
                    .selectable(selected, enabled = enabled, role = Role.RadioButton) { choose("model", value) }
                    .padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    // The row names a model: its maker's mark, whichever connection serves it.
                    MakerMark(model.makers[value], 18)
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
        if (model.value.isNotBlank() && profile.options.any { it.first != "auto" })
            ConnectionChoice(profile, connections, enabled, { connections = !connections }) { value ->
                connections = false; choose("profile", value)
            }
    }
}

/** The optional connection: a quiet row naming the choice, unfolding into automatic plus the serving connections. */
@Composable
private fun ConnectionChoice(profile: PickerChoice, expanded: Boolean, enabled: Boolean, toggle: () -> Unit, pin: (String) -> Unit) {
    val pinned = profile.pinned
    val current = pinned?.second ?: "自动"
    Column {
        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(ZorkShapes.Control)
            .selectable(false, enabled = enabled, role = Role.Button, onClick = toggle)
            .semantics { contentDescription = "连接 · $current" }
            .padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("连接", fontSize = 13.sp, color = ZorkColors.Subtle, modifier = Modifier.weight(1f))
            pinned?.let { profile.providers[it.first] }?.let { Glyph(providerDrawable(it), 16.dp, ZorkColors.Ink) }
            Text(current, fontSize = 14.sp, color = if (pinned != null) ZorkColors.Ink else ZorkColors.Subtle,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp))
            val turn by androidx.compose.animation.core.animateFloatAsState(if (expanded) 180f else 0f,
                androidx.compose.animation.core.tween(ZorkMotion.FAST), label = "connection-chevron")
            Box(Modifier.rotate(turn)) { Glyph(R.drawable.ic_chevron_down, 14.dp, ZorkColors.Subtle) }
        }
        ZorkExpand(expanded) {
            Column {
                val selected = profile.value.ifBlank { "auto" }
                profile.options.forEach { (value, label) ->
                    val auto = value == "auto"
                    val checked = value == selected
                    val name = if (auto) "自动" else label
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                        .background(if (checked) ZorkColors.Selected else ZorkColors.Canvas, ZorkShapes.Control)
                        .selectable(checked, enabled = enabled, role = Role.RadioButton) { pin(value) }
                        .semantics { contentDescription = "连接 · $name" }
                        .padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        if (auto) Glyph(R.drawable.ic_sparkles, 18.dp, ZorkColors.Ink)
                        else Glyph(providerDrawable(profile.providers[value].orEmpty()), 18.dp, ZorkColors.Ink)
                        Text(name, fontSize = 15.sp, fontWeight = if (checked) FontWeight.Medium else FontWeight.Normal, maxLines = 1)
                        // Automatic names the pool the device picks from for this model.
                        Text(if (auto) profile.connections[value].orEmpty().joinToString("、") { it.second } else "",
                            fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f))
                        if (checked) Glyph(R.drawable.ic_check, 16.dp, ZorkColors.Ink)
                    }
                }
            }
        }
    }
}

@Composable
internal fun PickerChip(label: String, selected: Boolean, enabled: Boolean = true, toggle: Boolean = false, click: () -> Unit) {
    val fill by androidx.compose.animation.animateColorAsState(if (selected) ZorkColors.Ink else ZorkColors.Prompt,
        androidx.compose.animation.core.tween(ZorkMotion.FAST), label = "chip")
    Box(Modifier.heightIn(min = 44.dp).background(fill, ZorkShapes.Control)
        // A toggle must stay operable when on; a selected radio exposes no click.
        .then(if (toggle) Modifier.toggleable(selected, enabled = enabled, role = Role.Checkbox) { click() }
            else Modifier.selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = click))
        .padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1,
            color = if (selected) ZorkColors.Canvas else if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}
