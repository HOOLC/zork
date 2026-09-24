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
    val details = inspection?.optJSONObject("details")
    val context = LocalContext.current
    var factsOpen by rememberSaveable(selection.query) { mutableStateOf(false) }
    val loading = data == null || devices.any { it.optBoolean("loading") } || inspection?.optBoolean("loading") == true
    SettingsPageFrame(selection.title, actions.back, loading, actions.refresh, modifier) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item {
                if (loading) Text("正在读取…", color = ZorkColors.Muted, fontSize = 13.sp)
                state.message?.let { Text(it, color = ZorkColors.Danger) }
                devices.forEach { device ->
                    device.text("error").takeIf { it.isNotBlank() }?.let { Text("${compactDeviceName(device.text("name"), device.deviceStatus())} · $it", color = ZorkColors.Danger) }
                    device.optJSONArray("issues").objects().forEach { Text("${compactDeviceName(device.text("name"), device.deviceStatus())} · ${it.text("error")}", color = ZorkColors.Danger) }
                }
                inspection?.text("error")?.takeIf { it.isNotBlank() }?.let { Text(it, color = ZorkColors.Danger) }
            }
            when {
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
                    items(details.optJSONArray("files").objects(), key = { "file:${it.text("path")}" }) { file ->
                        SettingsListGroup { SettingsListRow(file.text("path"), subtext = "运行日志", action = {
                            val query = JSONObject(selection.query!!)
                            query.getJSONObject("service").put("log", file.text("path"))
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
                selection.query == null -> {
                    val rows = devices.flatMap { device -> device.optJSONArray("items").objects().map { device to it } }
                    if (rows.isEmpty() && !loading && devices.none { it.text("error").isNotBlank() || it.optJSONArray("issues").objects().isNotEmpty() }) {
                        item { Text("暂无已登记的服务", color = ZorkColors.Muted) }
                    }
                    items(rows, key = { (device, row) -> "${device.text("id")}:${row.text("id")}" }) { (device, row) ->
                        SettingsListGroup {
                            SettingsListRow(row.text("name"), subtext = "${compactDeviceName(device.text("name"), device.deviceStatus())} · ${resourceStatus(row.text("status"))}", detail = row.text("description"), action = {
                                val query = JSONObject().put("service", JSONObject().put("id", row.text("id")))
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
internal fun SettingsPageFrame(title: String, back: () -> Unit, loading: Boolean, refresh: (() -> Unit)?,
    modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = back)
            Text(title, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f), maxLines = 2)
            refresh?.let { SettingsRefreshButton(loading, it) }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) { content() }
    }
}
