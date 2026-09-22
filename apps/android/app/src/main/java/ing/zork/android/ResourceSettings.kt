package ing.zork.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** Navigation identity only; resource content belongs to the shared core. */
internal data class ResourceSelection(val peer: String?, val kind: String, val query: String? = null, val title: String)

@Composable
internal fun ResourceSettings(state: MobileSettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val selection = state.resource ?: return
    val data = state.resourceData
    val devices = data?.optJSONArray("devices").objects()
    val inspection = data?.optJSONObject("inspection")
    val skills = inspection?.optJSONObject("skills")
    val details = inspection?.optJSONObject("details")
    val context = LocalContext.current
    var parameters by rememberSaveable(selection.query) { mutableStateOf<String?>(null) }
    var factsOpen by rememberSaveable(selection.query) { mutableStateOf(false) }
    val loading = data == null || devices.any { it.optBoolean("loading") } || inspection?.optBoolean("loading") == true
    BackHandler(parameters != null) { parameters = null }
    SettingsPageFrame(if (parameters != null) "$parameters · 参数" else selection.title,
        { if (parameters != null) parameters = null else actions.back() }, loading, actions.refresh, modifier) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item {
                if (loading) Text("正在读取…", color = ZorkColors.Muted, fontSize = 13.sp)
                state.message?.let { Text(it, color = ZorkColors.Danger) }
                devices.forEach { device ->
                    device.text("error").takeIf { it.isNotBlank() }?.let { Text("${deviceNameSummary(device.text("name"), device.deviceStatus())} · $it", color = ZorkColors.Danger) }
                    device.optJSONArray("issues").objects().forEach { Text("${deviceNameSummary(device.text("name"), device.deviceStatus())} · ${it.text("error")}", color = ZorkColors.Danger) }
                }
                inspection?.text("error")?.takeIf { it.isNotBlank() }?.let { Text(it, color = ZorkColors.Danger) }
            }
            when {
                parameters != null -> item {
                    details?.optJSONArray("tools").objects().find { it.text("name") == parameters }?.let { tool ->
                        Text(tool.text("description"), color = ZorkColors.Muted)
                        SelectionContainer { Text(tool.optJSONObject("input_schema")?.toString(2) ?: tool.opt("input_schema")?.toString().orEmpty(), fontFamily = FontFamily.Monospace, fontSize = 12.sp) }
                    }
                }
                details != null -> {
                    item {
                        Text(details.text("description"), fontSize = 14.sp)
                        details.optJSONArray("facts")?.let { facts ->
                            for (i in 0 until facts.length()) {
                                val fact = facts.optJSONArray(i) ?: continue
                                if (fact.optString(0) in listOf("last_error", "inspection_error")) Text(fact.optString(1), color = ZorkColors.Danger)
                            }
                        }
                    }
                    details.optJSONObject("document")?.let { document -> item {
                        Box(Modifier.fillMaxWidth().heightIn(max = 400.dp).verticalScroll(rememberScrollState())) {
                            if (document.text("path").endsWith(".md") || document.text("path").endsWith(".markdown")) Markdown(document.text("text"))
                            else SelectionContainer { Text(document.text("text"), fontFamily = FontFamily.Monospace, fontSize = 12.sp) }
                        }
                        if (document.optBoolean("truncated")) Text("内容超过显示范围，当前仅显示部分内容", color = ZorkColors.Muted, fontSize = 12.sp)
                    } }
                    items(details.optJSONArray("tools").objects(), key = { "tool:${it.text("name")}" }) { tool ->
                        SettingsListGroup { SettingsListRow(tool.text("name"), subtext = tool.text("description"), action = { parameters = tool.text("name") }) }
                    }
                    items(details.optJSONArray("files").objects(), key = { "file:${it.text("path")}" }) { file ->
                        SettingsListGroup { SettingsListRow(file.text("path"), subtext = if (selection.kind == "service") "运行日志" else "附带文件", action = {
                            val query = JSONObject(selection.query!!)
                            if (query.has("skill")) query.getJSONObject("skill").put("file", file.text("path"))
                            else query.getJSONObject("service").put("log", file.text("path"))
                            actions.resource(selection.copy(query = query.toString(), title = file.text("path")))
                        }) }
                    }
                    item {
                        SettingsButton(if (factsOpen) "收起详细信息" else "详细信息") { factsOpen = !factsOpen }
                        if (factsOpen) details.optJSONArray("facts")?.let { facts ->
                            SelectionContainer { Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                for (i in 0 until facts.length()) {
                                    val fact = facts.optJSONArray(i) ?: continue
                                    if (fact.optString(0) !in listOf("last_error", "inspection_error", "files_truncated")) {
                                        Text("${resourceFactLabel(fact.optString(0))}：${if (fact.optString(0) == "status") resourceStatus(fact.optString(1)) else fact.optString(1)}", fontSize = 13.sp)
                                    }
                                }
                            } }
                        }
                    }
                }
                skills != null -> {
                    val rows = skills.optJSONArray("skills").objects()
                    if (rows.isEmpty()) item { Text("暂无技能", color = ZorkColors.Muted) }
                    items(rows, key = { it.text("id") }) { skill ->
                        SettingsListGroup { SettingsListRow(skill.text("name"), subtext = skill.text("description"), action = {
                            val agent = JSONObject(selection.query!!).getString("agent_skills")
                            actions.resource(selection.copy(query = JSONObject().put("skill", JSONObject().put("agent", agent).put("skill", skill.text("id"))).toString(), title = skill.text("name")))
                        }) }
                    }
                    item { skills.optJSONArray("diagnostics")?.let { messages ->
                        for (i in 0 until messages.length()) Text(messages.getString(i), color = ZorkColors.Warning)
                    } }
                }
                selection.query == null -> {
                    val rows = devices.flatMap { device -> device.optJSONArray("items").objects().map { device to it } }
                    if (rows.isEmpty() && !loading && devices.none { it.text("error").isNotBlank() || it.optJSONArray("issues").objects().isNotEmpty() }) {
                        item { Text(if (selection.kind == "mcp") "尚未添加工具连接" else "暂无已登记的服务", color = ZorkColors.Muted) }
                    }
                    items(rows, key = { (device, row) -> "${device.text("id")}:${row.text("id")}" }) { (device, row) ->
                        SettingsListGroup {
                            SettingsListRow(row.text("name"), subtext = "${deviceNameSummary(device.text("name"), device.deviceStatus())} · ${resourceStatus(row.text("status"))}", detail = row.text("description"), action = {
                                val query = if (selection.kind == "mcp") JSONObject().put("mcp", row.text("id"))
                                    else JSONObject().put("service", JSONObject().put("id", row.text("id")))
                                actions.resource(selection.copy(peer = device.text("id"), query = query.toString(), title = row.text("name")))
                            })
                            row.text("url").takeIf { it.isNotBlank() }?.let { url ->
                                Box(Modifier.padding(start = 16.dp, bottom = 12.dp)) { SettingsButton("打开服务") { openServiceBrowser(context, url) } }
                            }
                        }
                    }
                }
            }
        }
    }
}

internal fun resourceStatus(status: String): String = when (status) {
    "ready" -> "已就绪"; "disabled" -> "已停用"; "running" -> "运行中"; "stopped" -> "已停止"
    "external" -> "外部服务，未探测"; "starting" -> "启动中"; "failed" -> "失败"
    "auth_required" -> "需要认证"; "unprobed" -> "尚未探测"; else -> status
}
private fun resourceFactLabel(key: String) = when (key) {
    "status" -> "状态"; "mode" -> "运行方式"; "port" -> "设备本地端口"; "shared" -> "共享状态"
    "owner_session" -> "所属任务 / 会话"; "last_started_at" -> "最近启动"; "source" -> "来源"
    "path" -> "设备上的路径"; "protocol" -> "接入方式"; else -> key
}

@Composable
internal fun SettingsPageFrame(title: String, back: () -> Unit, loading: Boolean, refresh: () -> Unit,
    modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = back)
            Text(title, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f), maxLines = 2)
            SettingsRefreshButton(loading, refresh)
        }
        Box(Modifier.weight(1f).fillMaxWidth()) { content() }
    }
}
