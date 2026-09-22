package ing.zork.android

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONArray
import org.json.JSONObject

@OptIn(ExperimentalLayoutApi::class)
@Composable
internal fun SettingsEditor(kind: String, source: JSONObject?, state: MobileSettingsState, actions: SettingsActions,
    latest: String, dismiss: () -> Unit, saved: () -> Unit,
    open: Boolean = true, onClosed: () -> Unit = dismiss) {
    if (kind == "model") { ModelEditor(source, state, actions, dismiss, saved, open, onClosed); return }
    val context = LocalContext.current
    val attempt = state.authorization.takeIf { kind == "connection" }
    var name by rememberSaveable { mutableStateOf(if (kind == "rename") state.device?.name.orEmpty() else if (kind == "profile-name") source?.let(::profileName).orEmpty() else source?.text("name").orEmpty()) }
    var profileId by rememberSaveable { mutableStateOf(attempt?.text("profile_id") ?: source?.text("profile_id").orEmpty()) }
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
        "rename" -> "修改设备名称"
        "connection" -> "添加模型连接"; "profile-name" -> "重命名连接"; else -> "升级设备"
    }
    SettingsSheet(title, busy, error, ::close, open = open, onClosed = onClosed,
        dismissWhileBusy = attempt != null && !submit.busy, footer = {
        when (kind) {
            "profile-name" -> SettingsButton(if (busy) "保存中…" else "保存", true, editable) {
                submit.perform("rename_profile", JSONObject().put("profile", source!!.text("profile_id")).put("name", name))
            }
            "rename" -> SettingsButton(if (busy) "保存中…" else "保存", true, editable) {
                submit.perform("rename_device", JSONObject().put("name", name))
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
            "profile-name", "rename" -> {
                SettingsField("名称", name, { name = it }, enabled = editable)
                Text(if (kind == "rename") "连接此设备的客户端都会看到新名称。" else "名称可使用中文，原有连接 ID 和 Session 配置保持有效。", fontSize = 12.sp, color = ZorkColors.Muted)
            }
            "connection" -> {
                Text("${deviceNameSummary(state.device?.name.orEmpty(), state.device?.status ?: DeviceStatusUi())} · 连接保存在此设备", fontSize = 12.sp, color = ZorkColors.Muted)
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
