package ing.zork.android

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import org.json.JSONArray
import org.json.JSONObject

/**
 * Adding or editing a model. First glance: the id and one line saying what it was
 * recognized as; every parameter is already filled from the built-in catalog and
 * stays collapsed under "调整参数". Expanded, each field can take a value from a
 * popular model ("参照"). Thinking is edited in the model's own scheme.
 */
@Composable
internal fun ModelEditor(source: JSONObject?, state: MobileSettingsState, actions: SettingsActions, dismiss: () -> Unit, saved: () -> Unit,
    open: Boolean = true, onClosed: () -> Unit = dismiss) {
    val initial = remember(source) { JSONObject(NativeBridge.modelForm(JSONObject().put("profile", state.profile ?: JSONObject.NULL)
        .put("providers", JSONArray(state.providers)).put("model", source ?: JSONObject.NULL).toString())) }
    var id by rememberSaveable { mutableStateOf(initial.text("id")) }
    var api by rememberSaveable { mutableStateOf(initial.text("api")) }
    var context by rememberSaveable { mutableStateOf(initial.text("context")) }
    var output by rememberSaveable { mutableStateOf(initial.text("output")) }
    var schemeJson by rememberSaveable { mutableStateOf((initial.optJSONObject("thinking_scheme") ?: JSONObject().put("kind", "unsupported")).toString()) }
    val scheme = JSONObject(schemeJson)
    var images by rememberSaveable { mutableStateOf(initial.optBoolean("images")) }
    var expanded by rememberSaveable { mutableStateOf(false) }
    var copiedJson by rememberSaveable { mutableStateOf<String?>(null) }
    val copied = copiedJson?.let(::JSONObject)
    var attempted by rememberSaveable { mutableStateOf(false) }
    var recognized by remember { mutableStateOf<JSONObject?>(null) }
    var reference by remember { mutableStateOf<String?>(null) }
    var showField by remember { mutableStateOf(false) }
    val submit = rememberSettingsSubmission(state, actions) { _, _ -> saved() }
    val busy = submit.busy
    val templates = remember(state.profiles) {
        state.profiles.flatMap { profile -> profile.optJSONArray("models").objects().filter {
            !(profile.text("profile_id") == state.profile?.text("profile_id") && it.text("id") == source?.text("id")) && NativeBridge.copyableModel(it.toString())
        }.map {
            Triple(profile.text("profile_id") + "/" + it.text("id"), "${profileName(profile)} · ${it.text("id")}", it)
        } }
    }
    val models = state.profile?.optJSONArray("models").objects()
    fun input() = JSONObject().put("previous", source ?: JSONObject.NULL).put("copied", copied ?: JSONObject.NULL)
        .put("id", id).put("api", api).put("context", context).put("output", output)
        .put("thinking", initial.text("thinking")).put("default_thinking", initial.text("default_thinking"))
        .put("thinking_scheme", scheme).put("images", images)
    fun adopt(form: JSONObject) {
        api = form.text("api").ifBlank { api }; context = form.text("context"); output = form.text("output")
        form.optJSONObject("thinking_scheme")?.let { schemeJson = it.toString() }
        images = form.optBoolean("images")
    }
    // Recognize the id against the built-in catalog; core only fills fields that are empty.
    LaunchedEffect(id) {
        delay(250)
        try {
            val result = actions.catalog("model_catalog_recognize", JSONObject().put("id", id.trim())
                .put("provider", state.profile?.text("provider") ?: JSONObject.NULL).put("input", input())) as? JSONObject
            recognized = result?.optJSONObject("entry")
            if (source == null || source.optJSONObject("limits") == null) result?.optJSONObject("input")?.let(::adopt)
        } catch (e: CancellationException) { throw e } catch (_: Exception) { recognized = null }
    }
    val errors = remember(id, api, context, output, schemeJson, images, source, copied, state.profile) {
        JSONArray(NativeBridge.validateModel(input().toString(), JSONArray(models).toString())).objects()
            .associate { it.text("field") to it.text("message") }
    }
    // Collapsed parameters still block saving; open them when a check fails.
    LaunchedEffect(attempted, errors) { if (attempted && errors.keys.any { it != "profile-model" }) expanded = true }
    SettingsSheet(if (source == null) "添加模型" else "编辑模型", busy, submit.error, dismiss, open = open, onClosed = onClosed, footer = {
        SettingsButton(if (busy) "保存中…" else "保存模型", true, !busy && state.online) {
            attempted = true
            if (errors.isEmpty() && state.profile != null) {
                submit.perform("save_model", JSONObject().put("profile", state.profile.text("profile_id")).put("input", input()))
            }
        }
        if (source != null) SettingsButton("移除模型", enabled = !busy && state.online && state.profile != null) {
            submit.perform("remove_model", JSONObject().put("profile", state.profile!!.text("profile_id")).put("model", source))
        }
    }) {
        SettingsField("模型 ID", id, { id = it }, enabled = !busy, error = errors["profile-model"].takeIf { attempted })
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val entry = recognized
            Text(when {
                entry != null -> "${entry.text("name")} · ${entry.text("summary")}"
                id.isBlank() -> "输入模型 ID，常见模型会自动填好参数"
                else -> "未识别，参数需要手动确认"
            }, fontSize = 13.sp, color = if (entry != null) ZorkColors.Ink else ZorkColors.Subtle, maxLines = 2, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f))
            ZorkButton(if (expanded) "收起参数" else "调整参数", quiet = true, onClick = { expanded = !expanded })
        }
        ZorkExpand(expanded) { Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
            if (templates.isNotEmpty()) SettingsSelect("复制已有模型配置", "", templates.map { it.first to it.second }, !busy) { key ->
                val template = templates.first { it.first == key }.third
                copiedJson = template.toString()
                adopt(JSONObject(NativeBridge.modelForm(JSONObject().put("input", input()).put("copy", template).toString())))
            }
            SettingsSelect("接口协议", api, listOf("openai-responses" to "OpenAI Responses", "openai-completions" to "OpenAI Chat Completions",
                "anthropic-messages" to "Anthropic Messages", "openai-codex-responses" to "Codex Responses"), !busy) { api = it }
            ReferenceRow("上下文", { reference = "context" }) {
                SettingsField("上下文", context, { context = it }, enabled = !busy, error = errors["profile-context-limit"].takeIf { attempted }, detail = "例如 128K 或 1M")
            }
            ReferenceRow("最长输出", { reference = "output" }) {
                SettingsField("最长输出", output, { output = it }, enabled = !busy, error = errors["profile-output-limit"].takeIf { attempted })
            }
            ReferenceRow("思考方式", { reference = "thinking" }) {
                ThinkingEditor(scheme, !busy) { schemeJson = it.toString() }
            }
            errors["profile-default-thinking"]?.takeIf { attempted }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
            recognized?.text("thinking_field")?.takeIf { it.isNotBlank() }?.let { field ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    ZorkButton("ⓘ 请求字段", quiet = true, onClick = { showField = !showField })
                    if (showField) Text(field, fontSize = 12.sp, color = ZorkColors.Subtle, fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace)
                }
            }
            ReferenceRow("能力", { reference = "capabilities" }) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) { PickerChip("读取图片", images, !busy) { images = !images } }
            }
        } }
    }
    ZorkRetained(reference) { field, shown, closed ->
        ReferenceSheet(field, id, actions, shown, { reference = null }, closed) { value ->
            when (field) {
                "context" -> context = compactTokens((value as Number).toLong())
                "output" -> output = compactTokens((value as Number).toLong())
                "thinking" -> (value as? JSONObject)?.optJSONObject("scheme")?.let { schemeJson = it.toString() }
                "capabilities" -> images = (value as? JSONObject)?.optBoolean("image") == true
            }
            reference = null
        }
    }
}

@Composable
private fun ReferenceRow(label: String, open: () -> Unit, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(label, fontSize = 12.sp, color = ZorkColors.Muted, modifier = Modifier.weight(1f))
            ZorkButton("参照", quiet = true, onClick = open)
        }
        content()
    }
}

/** Popular models' values for one field; the recognized model comes first. */
@Composable
private fun ReferenceSheet(field: String, id: String, actions: SettingsActions, open: Boolean, dismiss: () -> Unit, closed: () -> Unit,
    choose: (Any) -> Unit) {
    var rows by remember(field, id) { mutableStateOf<List<JSONObject>?>(null) }
    LaunchedEffect(field, id) {
        rows = try {
            (actions.catalog("model_catalog_references", JSONObject().put("field", field).put("id", id.trim())) as? JSONArray).objects()
        } catch (e: CancellationException) { throw e } catch (_: Exception) { emptyList() }
    }
    val title = when (field) { "context" -> "上下文 · 参照常见模型"; "output" -> "最长输出 · 参照常见模型"; "thinking" -> "思考方式 · 参照常见模型"; else -> "能力 · 参照常见模型" }
    ZorkSheet(open, title, dismiss, onClosed = closed) {
        Text(title, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Column(Modifier.fillMaxWidth().heightIn(max = 420.dp).verticalScrollable()) {
            when (val list = rows) {
                null -> Text("正在读取…", fontSize = 13.sp, color = ZorkColors.Muted)
                else -> list.forEach { row ->
                    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 48.dp), onClick = { choose(row.get("value")) }) {
                        Text(row.text("name"), fontSize = 14.sp, fontWeight = if (row.optBoolean("recognized")) FontWeight.SemiBold else FontWeight.Normal,
                            modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(row.text("label"), fontSize = 13.sp, color = ZorkColors.Muted, maxLines = 1)
                    }
                }
            }
        }
        Text("内置参数以供应商文档为准", fontSize = 12.sp, color = ZorkColors.Subtle)
    }
}

@Composable
private fun Modifier.verticalScrollable() = this.then(Modifier.verticalScroll(rememberScrollState()))

private fun tokensLabel(tokens: Long) = compactTokens(tokens).let { if (tokens % 1024 == 0L && tokens >= 1024 && !it.endsWith("K") && !it.endsWith("M")) "${tokens / 1024}K" else it }

private fun parseTokens(text: String): Long? {
    val t = text.trim().uppercase()
    return when {
        t.endsWith("M") -> t.dropLast(1).toDoubleOrNull()?.let { (it * 1_000_000).toLong() }
        t.endsWith("K") -> t.dropLast(1).toDoubleOrNull()?.let { (it * 1024).toLong() }
        else -> t.toLongOrNull()
    }
}

/**
 * The model's thinking in its own terms: not supported, always on, an on/off
 * toggle, native effort levels, or a token budget. The kind picks the control.
 */
@Composable
private fun ThinkingEditor(scheme: JSONObject, enabled: Boolean, change: (JSONObject) -> Unit) {
    val kind = scheme.text("kind", "unsupported")
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("unsupported" to "不支持", "always" to "固定开启", "toggle" to "开关", "levels" to "档位", "budget" to "预算").forEach { (value, label) ->
                PickerChip(label, kind == value, enabled) {
                    if (value != kind) change(when (value) {
                        "always" -> JSONObject().put("kind", "always").put("value", "high")
                        "toggle" -> JSONObject().put("kind", "toggle").put("on", "high").put("default_on", true)
                        "levels" -> JSONObject().put("kind", "levels").put("values", JSONArray(listOf("low", "medium", "high"))).put("default", "medium")
                        "budget" -> JSONObject().put("kind", "budget").put("presets", JSONArray(listOf(4096, 16384, 32768)))
                            .put("default", JSONObject().put("kind", "tokens").put("tokens", 16384)).put("dynamic", false).put("allow_off", true)
                        else -> JSONObject().put("kind", "unsupported")
                    })
                }
            }
        }
        when (kind) {
            "always" -> SettingsField("发送的值", scheme.text("value"), { change(JSONObject(scheme.toString()).put("value", it.trim())) }, enabled = enabled)
            "toggle" -> SettingsToggle("默认开启", scheme.optBoolean("default_on"), enabled) { change(JSONObject(scheme.toString()).put("default_on", it)) }
            "levels" -> {
                val values = scheme.optJSONArray("values")?.let { a -> (0 until a.length()).map { a.optString(it) } }.orEmpty()
                var text by remember(scheme.toString()) { mutableStateOf(values.joinToString(", ")) }
                SettingsField("档位（供应商原名，逗号分隔）", text, { value ->
                    text = value
                    val list = value.split(',', '，').map(String::trim).filter(String::isNotEmpty).distinct()
                    change(JSONObject(scheme.toString()).put("values", JSONArray(list))
                        .put("default", scheme.text("default").takeIf { it in list } ?: list.firstOrNull().orEmpty()))
                }, enabled = enabled)
                if (values.isNotEmpty()) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text("默认", fontSize = 12.sp, color = ZorkColors.Muted)
                    values.forEach { v -> PickerChip(v, v == scheme.text("default"), enabled) { change(JSONObject(scheme.toString()).put("default", v)) } }
                }
            }
            "budget" -> {
                val presets = scheme.optJSONArray("presets")?.let { a -> (0 until a.length()).map { a.optLong(it) } }.orEmpty()
                var text by remember(scheme.toString()) { mutableStateOf(presets.joinToString(", ") { tokensLabel(it) }) }
                SettingsField("预算档位（例如 4K, 16K, 32K）", text, { value ->
                    text = value
                    val list = value.split(',', '，').mapNotNull(::parseTokens).filter { it > 0 }.distinct()
                    change(JSONObject(scheme.toString()).put("presets", JSONArray(list)))
                }, enabled = enabled)
                val default = scheme.optJSONObject("default") ?: JSONObject().put("kind", "off")
                Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text("默认", fontSize = 12.sp, color = ZorkColors.Muted)
                    if (scheme.optBoolean("allow_off")) PickerChip("关", default.text("kind") == "off", enabled) {
                        change(JSONObject(scheme.toString()).put("default", JSONObject().put("kind", "off"))) }
                    if (scheme.optBoolean("dynamic")) PickerChip("动态", default.text("kind") == "dynamic", enabled) {
                        change(JSONObject(scheme.toString()).put("default", JSONObject().put("kind", "dynamic"))) }
                    presets.forEach { tokens ->
                        PickerChip(tokensLabel(tokens), default.text("kind") == "tokens" && default.optLong("tokens") == tokens, enabled) {
                            change(JSONObject(scheme.toString()).put("default", JSONObject().put("kind", "tokens").put("tokens", tokens)))
                        }
                    }
                }
                SettingsToggle("允许关闭", scheme.optBoolean("allow_off"), enabled) { change(JSONObject(scheme.toString()).put("allow_off", it)) }
                SettingsToggle("允许动态", scheme.optBoolean("dynamic"), enabled) { change(JSONObject(scheme.toString()).put("dynamic", it)) }
            }
        }
    }
}
