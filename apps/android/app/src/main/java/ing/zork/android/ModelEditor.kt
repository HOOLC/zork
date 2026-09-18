package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

@Composable
internal fun ModelEditor(source: JSONObject?, state: MobileSettingsState, actions: SettingsActions, dismiss: () -> Unit, saved: () -> Unit,
    open: Boolean = true, onClosed: () -> Unit = dismiss) {
    val initial = remember(source) { JSONObject(NativeBridge.modelForm(JSONObject().put("profile", state.profile ?: JSONObject.NULL)
        .put("providers", JSONArray(state.providers)).put("model", source ?: JSONObject.NULL).toString())) }
    var id by rememberSaveable { mutableStateOf(initial.text("id")) }
    var api by rememberSaveable { mutableStateOf(initial.text("api")) }
    var context by rememberSaveable { mutableStateOf(initial.text("context")) }
    var output by rememberSaveable { mutableStateOf(initial.text("output")) }
    var levels by rememberSaveable { mutableStateOf(initial.text("thinking")) }
    var defaultThinking by rememberSaveable { mutableStateOf(initial.text("default_thinking")) }
    var images by rememberSaveable { mutableStateOf(initial.optBoolean("images")) }
    var copiedJson by rememberSaveable { mutableStateOf<String?>(null) }
    val copied = copiedJson?.let(::JSONObject)
    var copiedKey by rememberSaveable { mutableStateOf("") }
    var attempted by rememberSaveable { mutableStateOf(false) }
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
        .put("thinking", levels).put("default_thinking", defaultThinking).put("images", images)
    val errors = remember(id, api, context, output, levels, defaultThinking, images, source, copied, state.profile) {
        JSONArray(NativeBridge.validateModel(input().toString(), JSONArray(models).toString())).objects()
            .associate { it.text("field") to it.text("message") }
    }
    val idError = errors["profile-model"]
    val contextError = errors["profile-context-limit"]
    val outputError = errors["profile-output-limit"]
    val thinkingError = errors["profile-default-thinking"]
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
        if (templates.isNotEmpty()) SettingsSelect("复制已有模型配置", copiedKey, templates.map { it.first to it.second }, !busy) { key ->
            val template = templates.first { it.first == key }.third
            copiedKey = key; copiedJson = template.toString()
            val form = JSONObject(NativeBridge.modelForm(JSONObject().put("input", input()).put("copy", template).toString()))
            api = form.text("api"); context = form.text("context"); output = form.text("output")
            levels = form.text("thinking"); defaultThinking = form.text("default_thinking"); images = form.optBoolean("images")
        }
        SettingsField("模型 ID", id, { id = it }, enabled = !busy, error = idError.takeIf { attempted })
        SettingsSelect("接口协议", api, listOf("openai-responses" to "OpenAI Responses", "openai-completions" to "OpenAI Chat Completions",
            "anthropic-messages" to "Anthropic Messages", "openai-codex-responses" to "Codex Responses"), !busy) { api = it }
        SettingsField("上下文 token 上限", context, { context = it }, enabled = !busy, error = contextError.takeIf { attempted }, detail = "例如 128K 或 1M")
        SettingsField("输出 token 上限", output, { output = it }, enabled = !busy, error = outputError.takeIf { attempted })
        SettingsField("推理级别，用逗号分隔", levels, { levels = it }, enabled = !busy)
        SettingsField("默认推理级别", defaultThinking, { defaultThinking = it }, enabled = !busy, error = thinkingError.takeIf { attempted })
        Text("填写该模型实际支持的容量和推理级别。", fontSize = 12.sp, color = ZorkColors.Muted)
        SettingsToggle("可读取图片", images, !busy, change = { images = it })
    }
}
