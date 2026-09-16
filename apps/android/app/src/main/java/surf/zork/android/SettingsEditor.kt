package surf.zork.android

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun SettingsEditor(kind: String, source: JSONObject?, state: MobileSettingsState, actions: SettingsActions,
    latest: String, dismiss: () -> Unit, saved: () -> Unit, manageGrants: (JSONObject) -> Unit = {},
    open: Boolean = true, onClosed: () -> Unit = dismiss) {
    if (kind == "model") { ModelEditor(source, state, actions, dismiss, saved, open, onClosed); return }
    val context = LocalContext.current
    val attempt = state.authorization.takeIf { kind == "connection" }
    var name by rememberSaveable { mutableStateOf(if (kind == "rename") state.device?.name.orEmpty() else if (kind == "profile-name") source?.let(::profileName).orEmpty() else source?.text("name").orEmpty()) }
    var avatar by rememberSaveable { mutableStateOf(source?.text("avatar")?.ifBlank { "cat" } ?: "cat") }
    var role by rememberSaveable { mutableStateOf(source?.text("role") ?: "leader") }
    var profileId by rememberSaveable { mutableStateOf(attempt?.text("profile_id") ?: source?.text("profile_id")?.ifBlank { "auto" } ?: if (kind == "agent") "auto" else "") }
    var agentThinking by rememberSaveable { mutableStateOf(source?.text("thinking").orEmpty()) }
    var modelId by rememberSaveable { mutableStateOf(source?.text("model").orEmpty()) }
    var instructions by rememberSaveable { mutableStateOf("") }
    val initialGrants = source?.optJSONArray("allowed_leaders")?.let { a -> (0 until a.length()).map { a.getString(it) } }.orEmpty()
    var selectedGrants by rememberSaveable { mutableStateOf(ArrayList(initialGrants.filter { '/' !in it })) }
    var remoteGrants by rememberSaveable { mutableStateOf(initialGrants.filter { '/' in it }.joinToString("\n")) }
    // Core parses separators and validates these references on both platforms.
    fun grants() = JSONArray(selectedGrants + listOf(remoteGrants))
    var access by rememberSaveable { mutableStateOf("subscription") }
    var providerId by rememberSaveable { mutableStateOf(attempt?.text("provider").orEmpty()) }
    var billingId by rememberSaveable { mutableStateOf(attempt?.text("billing").orEmpty()) }
    var base by rememberSaveable { mutableStateOf("") }
    var key by remember { mutableStateOf("") }
    var callback by remember { mutableStateOf("") }
    var authorizationId by rememberSaveable { mutableStateOf(attempt?.text("id")) }
    val id = rememberSaveable { NativeBridge.newId() }
    val submit = rememberSettingsSubmission(state, actions) { action, _ ->
        when (action) {
            "start_authorization", "complete_authorization", "upgrade" -> Unit
            "cancel_authorization" -> dismiss()
            else -> { key = ""; saved() }
        }
    }
    val busy = submit.busy || (kind == "connection" && state.authorizationBusy)
    val editable = !busy && state.online
    val upgrading = state.operation?.optBoolean("running") == true
    val options = remember(state.profiles, profileId, modelId, agentThinking) {
        JSONObject(NativeBridge.agentChoices(JSONArray(state.profiles).toString(), profileId, modelId, agentThinking))
    }
    val chosenProfile = options.text("profile")
    val chosenThinking = options.text("thinking")
    val connection = remember(state.providers, access, providerId, billingId) {
        JSONObject(NativeBridge.connectionChoices(JSONArray(state.providers).toString(), access == "subscription", providerId, billingId))
    }
    val providers = connection.optJSONArray("providers").objects()
    val provider = connection.optJSONObject("provider")
    val billings = connection.optJSONArray("billings").objects()
    val billing = connection.optJSONObject("billing")
    val deviceCode = billing?.optBoolean("deviceCode") == true || attempt != null
    fun close() {
        if (attempt != null) submit.perform("cancel_authorization", JSONObject()) else dismiss()
    }
    LaunchedEffect(attempt) { if (attempt != null) authorizationId = attempt.text("id") }
    LaunchedEffect(state.authorizationComplete) {
        if (kind == "connection" && authorizationId != null && state.authorizationComplete) { key = ""; saved() }
    }
    LaunchedEffect(state.operation) {
        if (kind == "upgrade" && state.operation?.text("version") == latest && state.operation.optBoolean("completed")) saved()
    }
    val error = submit.error ?: if (kind == "connection") state.authorizationError else if (kind == "upgrade") state.operation?.text("error")?.takeIf { it.isNotBlank() } else null
    val title = when (kind) {
        "rename" -> "修改设备名称"; "agent" -> if (source == null) "添加小伙伴" else "编辑小伙伴"
        "connection" -> "添加模型连接"; "profile-name" -> "重命名连接"; "grants" -> "管理领队授权"; else -> "升级设备"
    }
    SettingsSheet(title, busy, error, ::close, open = open, onClosed = onClosed,
        dismissWhileBusy = attempt != null && !submit.busy, footer = {
        when (kind) {
            "grants" -> SettingsButton(if (busy) "保存中…" else "保存授权", true, editable) {
                submit.perform("agent_grants", JSONObject().put("id", source!!.text("id"))
                    .put("allowed", grants()).put("expected", JSONArray(initialGrants)))
            }
            "profile-name" -> SettingsButton(if (busy) "保存中…" else "保存", true, editable) {
                submit.perform("rename_profile", JSONObject().put("profile", source!!.text("profile_id")).put("name", name))
            }
            "rename" -> SettingsButton(if (busy) "保存中…" else "保存", true, editable) {
                submit.perform("rename_device", JSONObject().put("name", name))
            }
            "agent" -> SettingsButton(if (busy) "保存中…" else if (source == null) "创建" else "保存修改", true, editable && options.optBoolean("valid")) {
                submit.perform("save_agent", JSONObject().put("input", JSONObject()
                    .put("id", source?.text("id") ?: id).put("creating", source == null).put("name", name)
                    .put("role", role).put("avatar", avatar).put("profile", chosenProfile).put("model", modelId)
                    .put("thinking", chosenThinking).put("instructions", instructions).put("allowed", grants())))
            }
            "connection" -> {
                if (attempt != null) SettingsButton("取消登录", enabled = !submit.busy) { close() }
                SettingsButton(if (busy) "处理中…" else if (attempt != null) "完成连接" else if (provider == null) "选择提供商" else if (deviceCode) "登录并连接" else "保存连接",
                    true, editable && ((attempt == null && provider != null && billing != null) || attempt?.text("flow") == "browser_callback")) {
                    when {
                        attempt != null -> submit.perform("complete_authorization", JSONObject().put("callback", callback))
                        deviceCode -> submit.perform("start_authorization", JSONObject().put("profile", profileId).put("provider", providerId).put("billing", billing!!.text("id")))
                        else -> submit.perform("save_connection", JSONObject().put("input", JSONObject().put("id", profileId).put("provider", providerId)
                            .put("billing", billing!!.text("id")).put("base_url", base).put("key", key)))
                    }
                }
            }
            "upgrade" -> SettingsButton(if (upgrading) "正在升级…" else "确认升级", true, editable && !upgrading) {
                submit.perform("upgrade", JSONObject().put("version", latest))
            }
        }
    }) {
        if (!state.online) Text("设备离线，显示已保存的设置。恢复连接后可修改。", color = ZorkColors.Muted, fontSize = 13.sp)
        when (kind) {
            "grants" -> {
                Text(source?.text("name").orEmpty(), fontSize = 16.sp)
                AgentGrantFields(state, selectedGrants.toSet(), remoteGrants, editable, { selectedGrants = ArrayList(it) }, { remoteGrants = it })
            }
            "profile-name", "rename" -> {
                SettingsField("名称", name, { name = it }, enabled = editable)
                Text(if (kind == "rename") "连接此设备的小伙伴都会看到新名称。" else "名称可使用中文，原有连接 ID 和小伙伴配置保持有效。", fontSize = 12.sp, color = ZorkColors.Muted)
            }
            "agent" -> {
                if (source == null) {
                    SettingsSegments(listOf("leader" to "领队", "worker" to "队员"), role, editable) { role = it }
                    SettingsField("名称", name, { name = it }, enabled = editable)
                } else Text(source.text("name"), fontSize = 16.sp)
                val models = options.optJSONArray("models").objects()
                val levels = options.optJSONArray("levels")?.let { a -> (0 until a.length()).map { a.getString(it) } }.orEmpty()
                SettingsSelect("模型", modelId, models.map { it.text("id") to it.text("id") }, editable) { modelId = it }
                SettingsSelect("思考深度", chosenThinking, levels.map { it to it }, editable && levels.isNotEmpty()) { agentThinking = it }
                SettingsSelect("模型连接（可选）", chosenProfile, options.optJSONArray("profiles").objects().map { it.text("profile_id") to profileName(it) }, editable) { profileId = it }
                if (!state.online) Text("模型：$modelId · 思考深度：$agentThinking · 连接：$profileId", fontSize = 12.sp, color = ZorkColors.Muted)
                Text("头像", fontSize = 12.sp, color = ZorkColors.Muted)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    listOf("cat" to "小猫", "bunny" to "小兔", "bear" to "小熊", "fox" to "狐狸", "panda" to "熊猫", "chick" to "小鸡", "dog" to "小狗", "owl" to "猫头鹰", "koala" to "考拉", "penguin" to "企鹅", "deer" to "小鹿", "octopus" to "章鱼").forEach { (value, label) ->
                        LiquidIconButton(label, onClick = { avatar = value }, enabled = editable,
                            modifier = Modifier.semantics { selected = avatar == value }) {
                            Avatar(value, 32.dp)
                        }
                    }
                }
                if (source == null) SettingsField("职责与偏好 · 可选", instructions, { instructions = it }, enabled = editable, singleLine = false)
                if (source == null && role == "worker") AgentGrantFields(state, selectedGrants.toSet(), remoteGrants, editable, { selectedGrants = ArrayList(it) }, { remoteGrants = it })
                if (source?.text("role") == "worker") SettingsButton("管理授权", enabled = !busy) { manageGrants(source) }
                if (source != null) SettingsButton("技能", enabled = !busy) { actions.skills(source) }
                if (state.profiles.isEmpty()) Text("先到「大模型」添加连接与模型。", fontSize = 13.sp, color = ZorkColors.Muted)
            }
            "connection" -> {
                Text("${state.device?.name.orEmpty()} · 连接保存在此设备", fontSize = 12.sp, color = ZorkColors.Muted)
                SettingsSegments(listOf("subscription" to "订阅账号", "api" to "API 接入"), access, editable && attempt == null) { access = it; providerId = ""; billingId = ""; key = ""; base = "" }
                SettingsSelect("提供商", providerId, providers.map { it.text("id") to it.text("label", it.text("id")) }, editable && attempt == null) { providerId = it; billingId = ""; base = ""; key = "" }
                if (billings.size > 1) SettingsSelect("接入方式", billing?.text("id").orEmpty(), billings.map { it.text("id") to it.text("label") }, editable && attempt == null) { billingId = it; key = "" }
                SettingsField("连接名称", profileId, { profileId = it }, enabled = editable && attempt == null)
                if (!deviceCode && provider != null && billing != null) {
                    if (providerId == "openai-compatible") SettingsField("接口地址", base, { base = it }, enabled = editable)
                    SettingsField("API Key", key, { key = it }, secret = true, enabled = editable)
                }
                attempt?.let { pending ->
                    val code = pending.text("user_code")
                    Text(if (code.isNotBlank()) "在浏览器输入：$code，完成后会自动保存。" else "在浏览器完成授权，再粘贴返回内容。", fontSize = 14.sp)
                    SettingsButton("打开登录页面") {
                        val uri = Uri.parse(pending.text("verification_url"))
                        if (uri.scheme == "https") context.startActivity(Intent(Intent.ACTION_VIEW, uri))
                    }
                    if (pending.text("flow") == "browser_callback") SettingsField("浏览器返回内容", callback, { callback = it }, secret = true, enabled = editable)
                }
            }
            "upgrade" -> {
                Text("升级至 $latest 后设备会重启，连接将暂时中断。", fontSize = 14.sp, lineHeight = 22.sp)
                state.operation?.text("message")?.takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Muted) }
            }
        }
    }
}
