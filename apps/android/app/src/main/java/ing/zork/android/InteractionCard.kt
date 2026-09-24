package ing.zork.android

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.foundation.background
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import org.json.JSONArray

internal data class InteractionFieldUi(val id: String, val label: String, val kind: String,
    val value: String, val options: List<Pair<String, String>>, val error: String?, val sensitive: Boolean = false, val advanced: Boolean = false)
internal data class InteractionActionUi(val id: String, val label: String, val primary: Boolean, val openUrl: String? = null)
internal data class InteractionCardUi(val id: String, val title: String, val status: String,
    val fields: List<InteractionFieldUi>, val details: List<Pair<String, String>>,
    val actions: List<InteractionActionUi>, val editable: Boolean, val error: String?, val description: String?)

// Presentation strings only. Action availability, validation and result folding
// are supplied by Rust core, never inferred from these labels.
private fun interactionText(key: String) = when (key) {
    "interaction_approved" -> "已批准"
    "interaction_approval_scope" -> "本次批准仅针对卡片所列请求。"
    "interaction_request_owner" -> "操作设备"
    "interaction_request_node" -> "操作由此请求所属的设备执行。"
    "interaction_target_agent" -> "修改对象"
    "interaction_avatar" -> "头像链接"
    "interaction_chat_node" -> "操作将在此对话所属的设备执行。"
    "interaction_public_input" -> "提交的内容会共享到当前对话。"
    "interaction_name" -> "名字"
    "interaction_instructions" -> "职责"
    "interaction_model" -> "使用模型"
    "interaction_grants" -> "允许分配任务的 Agent"
    "interaction_create_agent" -> "创建 Agent"
    "interaction_update_agent" -> "修改 Agent"
    "interaction_confirm_create" -> "创建 Agent"
    "interaction_confirm_update" -> "保存修改"
    "interaction_submit" -> "提交"
    "interaction_login" -> "连接账户"
    "interaction_profile" -> "账户连接"
    "interaction_private_login" -> "登录信息仅用于连接账户，不会出现在对话中。"
    "interaction_continue_login" -> "继续登录"
    "interaction_cancel_login" -> "取消登录"
    "interaction_open_login" -> "打开登录页面"
    "interaction_login_code" -> "登录验证码"
    "interaction_login_callback" -> "浏览器返回的授权码"
    "interaction_approve" -> "批准"
    "interaction_description" -> "请求内容"
    "interaction_processing" -> "处理中"
    "interaction_cancelled" -> "已取消"
    "interaction_expired" -> "已过期"
    "interaction_failed" -> "失败"
    "interaction_decline" -> "暂不执行"
    "interaction_retry" -> "恢复原提交"
    "interaction_completed" -> "已完成"
    "interaction_declined" -> "已拒绝"
    "interaction_submission_failed" -> "提交未被接受"
    "interaction_unconfirmed" -> "结果待确认"
    "interaction_submitting" -> "正在提交…"
    "interaction_confirmation" -> "等待你的确认"
    "local_script_description" -> "说明"
    "local_script_source" -> "脚本"
    "local_script_ready" -> "在这台手机上执行，结果仅在本机显示"
    "local_script_readonly" -> "请在支持此脚本的 Android 客户端操作"
    "local_script_run" -> "在本机执行"
    "interaction_unsupported" -> "当前版本暂不支持此操作"
    "input_required" -> "请填写此项"
    "input_too_large" -> "输入内容过长"
    "invalid_input_choice" -> "请选择有效选项"
    "invalid_input_json" -> "请输入有效的 JSON"
    "unknown_input_field" -> "不支持的输入字段"
    "invalid_agent_name" -> "请填写有效名称"
    "invalid_agent_configuration" -> "请检查配置内容"
    "invalid_agent_resources" -> "请检查资源配置"
    "interaction_role" -> "协作方式"
    "interaction_role_worker" -> "接受其他 Agent 分配的任务"
    "interaction_role_leader" -> "可以给其他 Agent 分配任务"
    "interaction_more_settings" -> "更多设置"
    "interaction_less_settings" -> "收起设置"
    "interaction_none" -> "未设置"
    "interaction_agent_created" -> "已创建"
    "interaction_agent_updated" -> "已保存"
    "interaction_create_description" -> "确认它的职责和使用的模型。"
    "interaction_update_description" -> "确认本次改动；其余设置保持不变。"
    "interaction_no_models" -> "还没有可用的模型，请先连接模型。"
    "interaction_provider" -> "服务"
    "interaction_preparing_login" -> "正在准备登录…"
    "interaction_waiting_login" -> "等待你完成登录"
    "interaction_verifying_login" -> "正在验证授权…"
    "interaction_finish_login" -> "完成连接"
    "interaction_login_browser_steps" -> "打开登录页面并完成授权，再将页面给出的授权码粘贴到这里。授权码不会出现在对话中。"
    "interaction_login_device_steps" -> "打开登录页面，按提示输入验证码并完成授权。此处会自动显示连接结果。"
    else -> key
}

internal fun parseInteractionCard(value: JSONObject): InteractionCardUi = InteractionCardUi(
    value.text("message_id"), value.text("title").let { if (value.optBoolean("localized_title")) interactionText(it) else it },
    interactionText(value.text("status_key")),
    value.optJSONArray("fields").objects().map { item ->
        val field = item.getJSONObject("field")
        InteractionFieldUi(field.text("id"), field.text("label").let { if (item.optBoolean("localized_label")) interactionText(it) else it },
            field.text("kind", "text"), item.text("value"), field.optJSONArray("options").objects().map { it.text("value") to it.text("label").let { label -> if (item.optBoolean("localized_options")) interactionText(label) else label } },
            item.text("error_key").takeIf(String::isNotBlank)?.let(::interactionText), item.optBoolean("sensitive"), item.optBoolean("advanced"))
    },
    value.optJSONArray("details").objects().map { interactionText(it.text("label_key")) to it.text("value") },
    value.optJSONArray("actions").objects().map { InteractionActionUi(it.text("id"), interactionText(it.text("label_key")), it.optBoolean("primary"), it.text("open_url").takeIf(String::isNotBlank)) },
    value.optBoolean("editable"), value.text("error").takeIf(String::isNotBlank)?.let(::interactionText),
    value.text("description_key").takeIf(String::isNotBlank)?.let(::interactionText),
)

@Composable
internal fun InteractionCard(card: InteractionCardUi, activate: (String, Map<String, String>) -> Unit) {
    val values = remember(card.id) { mutableStateMapOf<String, String>() }
    val uriHandler = LocalUriHandler.current
    var advancedOpen by remember(card.id) { mutableStateOf(false) }
    var detailsOpen by remember(card.id) { mutableStateOf(false) }
    LaunchedEffect(card.editable) { if (!card.editable) advancedOpen = false }
    LaunchedEffect(card.fields.map { it.id }) { values.keys.retainAll(card.fields.map { it.id }.toSet()) }
    ZorkCard(Modifier.fillMaxWidth().widthIn(max = 620.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            // Status is a small pill beside the title; the description is the body.
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(card.title, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = ZorkColors.Ink, modifier = Modifier.weight(1f, fill = false))
                if (card.status.isNotBlank()) Text(card.status, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1,
                    modifier = Modifier.background(ZorkColors.Prompt, ZorkShapes.Control).padding(horizontal = 10.dp, vertical = 3.dp))
            }
            card.description?.let { Text(it, fontSize = 14.sp, lineHeight = 21.sp, color = ZorkColors.Ink) }
            card.fields.filter { !it.advanced || advancedOpen || it.error != null }.forEach { field -> key(card.id, field.id) {
                val draftState = if (field.sensitive) remember(card.id, field.id) { mutableStateOf(values[field.id] ?: field.value) }
                    else rememberSaveable(card.id, field.id) { mutableStateOf(values[field.id] ?: field.value) }
                var draft by draftState
                LaunchedEffect(card.editable, field.value) {
                    if (!card.editable) draft = field.value
                    values[field.id] = draft
                }
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (!card.editable) {
                        Text(field.label, fontSize = 12.sp, color = ZorkColors.Muted)
                        InteractionValue(if (field.kind == "multi_choice") choiceLabel(field, field.value) else field.options.find { it.first == field.value }?.second ?: field.value)
                    } else if (field.kind == "choice" || field.kind == "multi_choice") {
                        val multiple = field.kind == "multi_choice"
                        ZorkChoiceField(field.label,
                            if (multiple) choiceLabel(field, draft) else field.options.find { it.first == draft }?.second ?: "请选择",
                            field.options, if (multiple) choiceValues(draft).toSet() else setOf(draft),
                            closeOnSelect = !multiple) { value ->
                            if (multiple) {
                                val selected = choiceValues(draft).toMutableList()
                                if (selected.contains(value)) selected.remove(value) else selected.add(value)
                                draft = JSONArray(selected).toString()
                            } else draft = value
                            values[field.id] = draft
                        }
                    } else {
                        ZorkTextField(field.label, draft, { draft = it; values[field.id] = it },
                            modifier = Modifier.fillMaxWidth(), secret = field.sensitive,
                            singleLine = field.kind != "multiline" && field.kind != "json", minLines = if (field.kind == "multiline" || field.kind == "json") 3 else 1,
                            maxLines = if (field.kind == "multiline" || field.kind == "json") 5 else 1, error = field.error)
                    }
                    if (!card.editable || field.kind == "choice" || field.kind == "multi_choice")
                        field.error?.let { Text(it, color = ZorkColors.Danger, fontSize = 12.sp) }
                }
            } }
            if (card.fields.any { it.advanced }) ZorkButton(
                interactionText(if (advancedOpen) "interaction_less_settings" else "interaction_more_settings"),
                quiet = true, onClick = { advancedOpen = !advancedOpen })
            // Request details stay available behind one expander.
            if (card.details.isNotEmpty()) {
                ZorkButton(if (detailsOpen) "收起详情" else "详情", quiet = true, onClick = { detailsOpen = !detailsOpen })
                if (detailsOpen) card.details.forEach { (label, value) -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
                    InteractionValue(value)
                } }
            }
            card.error?.let { Text(it, color = MaterialTheme.colorScheme.error, fontSize = 12.sp) }
            card.actions.forEach { action ->
                val click = { if (action.openUrl != null) uriHandler.openUri(action.openUrl) else activate(action.id, values.toMap()) }
                ZorkButton(action.label, Modifier.fillMaxWidth(), primary = action.primary, quiet = !action.primary, onClick = click)
            }
        }
    }
}

private fun choiceValues(value: String): List<String> = runCatching {
    val items = JSONArray(value)
    List(items.length()) { items.getString(it) }
}.getOrDefault(emptyList())
private fun choiceLabel(field: InteractionFieldUi, value: String): String {
    val selected = choiceValues(value)
    return field.options.filter { selected.contains(it.first) }.joinToString("、") { it.second }.ifBlank { interactionText("interaction_none") }
}

// Expansion is transient presentation state; the complete core value is retained.
@Composable
private fun InteractionValue(value: String) {
    var expanded by remember(value) { mutableStateOf(false) }
    val long = value.length > 384 || value.count { it == '\n' } > 6
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (expanded) Box(Modifier.heightIn(max = 220.dp).verticalScroll(rememberScrollState())) {
            Text(value, fontSize = 13.sp, color = ZorkColors.Ink)
        } else Text(value, maxLines = 6, overflow = TextOverflow.Ellipsis, fontSize = 13.sp, color = ZorkColors.Ink)
        if (long) ZorkButton(if (expanded) "收起" else "展开完整内容", quiet = true, onClick = { expanded = !expanded })
    }
}
