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
import androidx.compose.foundation.background
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/** Where the add-connection flow is: current step in ink, the others muted, no connectors. */
@Composable
private fun ConnectionSteps(step: Int) {
    Row(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
        listOf("供应商", "登录", "模型").forEachIndexed { index, label ->
            val current = index + 1 == step
            Text("${index + 1} $label", fontSize = 13.sp,
                fontWeight = if (current) androidx.compose.ui.text.font.FontWeight.SemiBold else androidx.compose.ui.text.font.FontWeight.Normal,
                color = if (current) ZorkColors.Ink else ZorkColors.Subtle)
        }
    }
}

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
    // Adding a connection is three steps: provider → sign in or key → models.
    var step by rememberSaveable { mutableStateOf(if (attempt != null) 2 else 1) }
    var discovering by remember { mutableStateOf(false) }
    var discoverError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    fun discover() {
        step = 3
        scope.launch {
            discovering = true; discoverError = null
            try { actions.perform("discover_models", JSONObject().put("profile", profileId)) }
            catch (e: kotlinx.coroutines.CancellationException) { throw e }
            catch (e: Exception) { discoverError = e.message ?: "获取模型失败，可稍后在连接详情里重试" }
            finally { discovering = false; actions.refresh() }
        }
    }
    val submit = rememberSettingsSubmission(state, actions) { action, _ ->
        when (action) {
            "start_authorization", "complete_authorization", "upgrade" -> Unit
            "cancel_authorization" -> dismiss()
            "save_connection" -> { key = ""; discover() }
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
    // Step 1 shows every provider once, noting which access kinds it offers.
    val providerCards = remember(state.providers) {
        val subscription = JSONObject(NativeBridge.connectionChoices(JSONArray(state.providers).toString(), true, "", ""))
            .optJSONArray("providers").objects()
        val api = JSONObject(NativeBridge.connectionChoices(JSONArray(state.providers).toString(), false, "", ""))
            .optJSONArray("providers").objects()
        (subscription + api).distinctBy { it.text("id") }.map { p ->
            val kinds = listOfNotNull("订阅".takeIf { subscription.any { it.text("id") == p.text("id") } },
                "API".takeIf { api.any { it.text("id") == p.text("id") } })
            Triple(p.text("id"), p.text("label", p.text("id")), kinds)
        }
    }
    val newProfile = state.profiles.find { it.text("profile_id") == profileId }
    fun close() {
        if (attempt != null) submit.perform("cancel_authorization", JSONObject()) else dismiss()
    }
    LaunchedEffect(attempt) { if (attempt != null) authorizationId = attempt.text("id") }
    LaunchedEffect(state.authorizationComplete) {
        if (kind == "connection" && authorizationId != null && state.authorizationComplete && step != 3) { key = ""; discover() }
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
            "connection" -> if (step == 3) {
                SettingsButton("完成", true, !discovering) { saved() }
            } else if (step == 1) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("保存到", fontSize = 12.sp, color = ZorkColors.Subtle)
                    DeviceMark(state.device?.name.orEmpty(), 16.dp)
                    Text(state.device?.name.orEmpty(), fontSize = 12.sp, color = ZorkColors.Subtle, modifier = Modifier.weight(1f))
                    DeviceStatusBadge(state.device?.status ?: DeviceStatusUi())
                }
            } else {
                if (attempt != null) SettingsButton("取消登录", enabled = !submit.busy) { close() }
                else ZorkButton("上一步", quiet = true, enabled = !busy, onClick = { step = 1 })
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
                ConnectionSteps(step)
                if (step == 1) {
                    providerCards.forEach { (value, label, kinds) ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 56.dp)
                            .background(if (value == providerId) ZorkColors.Selected else ZorkColors.Prompt, ZorkShapes.Block)
                            .zorkPressable(enabled = editable) {
                                providerId = value; billingId = ""; base = ""; key = ""
                                access = if ("订阅" in kinds) "subscription" else "api"
                                if (profileId.isBlank()) profileId = value
                                step = 2
                            }
                            .semantics { contentDescription = label }
                            .padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            ProviderMark(value, 24)
                            Text(label, fontSize = 15.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium, modifier = Modifier.weight(1f))
                            Text(kinds.joinToString(" · "), fontSize = 12.sp, color = ZorkColors.Subtle)
                        }
                    }
                } else if (step == 3) {
                    val models = newProfile?.optJSONArray("models").objects()
                    Text(when {
                        discovering -> "正在获取模型…"
                        models.isEmpty() -> "还没有模型，可稍后在连接详情里获取或手动添加"
                        else -> "开启的模型会出现在新建 Chat 的模型面板里"
                    }, fontSize = 13.sp, color = ZorkColors.Muted)
                    discoverError?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger) }
                    models.forEach { model ->
                        SettingsToggle(model.text("id"), model.optBoolean("enabled", true), enabled = state.online && model.optJSONObject("limits") != null,
                            detail = modelSummary(model)) { on ->
                            scope.launch {
                                runCatching { actions.perform("enable_model", JSONObject().put("profile", profileId).put("model", model.text("id")).put("enabled", on)) }
                                actions.refresh()
                            }
                        }
                    }
                } else {
                if (providerCards.find { it.first == providerId }?.third?.size == 2)
                    SettingsSegments(listOf("subscription" to "订阅账号", "api" to "API 接入"), access, editable && attempt == null) { access = it; billingId = ""; key = ""; base = "" }
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
            }
            "upgrade" -> {
                Text("升级至 $latest 后设备会重启，连接将暂时中断。", fontSize = 14.sp, lineHeight = 22.sp)
                state.operation?.text("message")?.takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Muted) }
            }
        }
    }
}
