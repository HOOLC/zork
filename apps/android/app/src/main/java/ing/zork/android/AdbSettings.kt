package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

@Composable
internal fun AdbSettings(actions: SettingsActions, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val snapshot = actions.adb
    val page = snapshot?.optJSONObject("page")
    val settings = snapshot?.optJSONObject("settings")
    val savedPort = settings?.takeIf { it.has("port") }?.getInt("port")
    var port by rememberSaveable(savedPort) { mutableStateOf(savedPort?.toString().orEmpty()) }
    var overlay by rememberSaveable { mutableStateOf<String?>(null) }
    var advanced by rememberSaveable(overlay) { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val stations = snapshot?.optJSONArray("stations")?.let { rows ->
        List(rows.length()) { rows.getJSONObject(it) }
    }.orEmpty()
    val help = when {
        overlay == "activation" -> page?.optJSONObject("activation_help")
        overlay?.startsWith("station:") == true -> stations.find { it.text("peer") == overlay?.removePrefix("station:") }?.optJSONObject("help")
        else -> null
    }
    val details = overlay == "details" && page?.optBoolean("show_details") == true
    LaunchedEffect(overlay, help == null, details) {
        if (overlay != null && help == null && !details) overlay = null
    }
    fun command(operation: JSONObject, saved: () -> Unit = {}) { scope.launch {
        busy = true; error = null
        try { actions.adbAction(operation); saved() }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { error = e.message }
        finally { busy = false }
    } }
    fun perform(action: JSONObject) {
        when (val kind = action.text("kind")) {
            "enable" -> command(JSONObject().put("action", "set_enabled").put("enabled", true))
            "disable" -> command(JSONObject().put("action", "set_enabled").put("enabled", false))
            "open_device_info", "open_developer_options" -> AdbPlatform.openSystemSettings(context, kind)
            "activation_help" -> { error = null; overlay = "activation" }
        }
    }
    LaunchedEffect(Unit) { actions.adbRefresh() }
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            ZorkIconButton("返回设置", onClick = actions.back) {
                Icon(painterResource(R.drawable.ic_arrow_left), null, Modifier.size(22.dp))
            }
            Text("安卓调试", fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            if (page?.optBoolean("show_details") == true) ZorkButton("详情", quiet = true, onClick = { overlay = "details" })
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 24.dp)) {
            if (page == null) {
                Text("正在读取调试状态…", color = ZorkColors.Muted, fontSize = 14.sp)
            } else {
                val tone = adbTone(page.text("tone"))
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                    Box(Modifier.size(6.dp).background(tone, CircleShape))
                    Text(page.text("status").orEmpty(), color = tone, fontSize = 13.sp)
                }
                Spacer(Modifier.height(16.dp))
                Text(page.text("title").orEmpty(), color = ZorkColors.Ink, fontSize = 23.sp, lineHeight = 33.sp, fontWeight = FontWeight.SemiBold)
                Spacer(Modifier.height(12.dp))
                Text(page.text("message").orEmpty(), color = ZorkColors.Muted, fontSize = 15.sp, lineHeight = 24.sp)
                page.optJSONObject("primary_action")?.let { action ->
                    Spacer(Modifier.height(28.dp))
                    AdbButton(action.text("label").orEmpty(), primary = true, enabled = !busy) { perform(action) }
                }
                if (page.optBoolean("show_connections")) {
                    Spacer(Modifier.height(28.dp))
                    if (stations.isEmpty()) Text(page.text("empty_message").orEmpty(), color = ZorkColors.Muted, fontSize = 13.sp)
                    else {
                        Text("Station 连接", color = ZorkColors.Subtle, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                            modifier = Modifier.padding(start = 8.dp, bottom = 8.dp))
                        SettingsListGroup {
                            stations.forEachIndexed { index, station ->
                                if (index > 0) SettingsListDivider()
                                AdbStationRow(station, !busy) { error = null; overlay = "station:${station.text("peer")}" }
                            }
                        }
                    }
                }
                page.text("note")?.let {
                    Spacer(Modifier.height(24.dp))
                    Text(it, color = ZorkColors.Muted, fontSize = 13.sp, lineHeight = 21.sp)
                }
                page.optJSONObject("secondary_action")?.let { action ->
                    Spacer(Modifier.height(28.dp))
                    AdbButton(action.text("label").orEmpty(), enabled = !busy) { perform(action) }
                }
            }
            (actions.adbError ?: error.takeIf { overlay == null })?.let {
                Spacer(Modifier.height(16.dp))
                Text(it, color = ZorkColors.Danger, fontSize = 13.sp)
            }
        }
    }
    ZorkRetained(help) { help, open, closed -> SettingsSheet(help.text("title").orEmpty(), busy = busy, error = error, dismiss = { overlay = null }, open = open, onClosed = closed) {
        help.optJSONArray("paragraphs")?.let { paragraphs ->
            repeat(paragraphs.length()) { Text(paragraphs.getString(it), color = ZorkColors.Muted, fontSize = 15.sp, lineHeight = 24.sp) }
        }
        help.text("command")?.let { activationCommand ->
            Row(Modifier.fillMaxWidth().background(ZorkColors.Paper, SettingsStyle.Field).padding(start = 16.dp, end = 6.dp, top = 8.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SelectionContainer(Modifier.weight(1f)) { Text(activationCommand, color = ZorkColors.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace) }
                ZorkIconButton("复制激活命令", onClick = { AdbPlatform.copyCommand(context, activationCommand) }) {
                    Icon(painterResource(R.drawable.ic_copy), null, Modifier.size(18.dp))
                }
            }
        }
        if (help.text("kind") == "activation") {
            ZorkButton("使用了其他调试端口？", quiet = true, onClick = { advanced = !advanced }, enabled = !busy)
            if (advanced) {
                SettingsField("手机调试端口", port, { port = it }, enabled = !busy)
                AdbButton("保存端口", enabled = !busy) {
                    command(JSONObject().put("action", "set_port").put("port", port)) { advanced = false }
                }
            }
        }
        help.text("note")?.let { Text(it, color = ZorkColors.Muted, fontSize = 13.sp) }
    } }
    ZorkRetained(Unit.takeIf { details }) { _, open, closed -> SettingsSheet("调试详情", dismiss = { overlay = null }, open = open, onClosed = closed) {
        stations.forEach { station ->
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(station.text("name").orEmpty(), color = ZorkColors.Ink, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                Text(station.text("state_label").orEmpty(), color = adbTone(station.text("tone")), fontSize = 13.sp)
                station.text("serial")?.let { serial ->
                    SelectionContainer { Text(serial, color = ZorkColors.Muted, fontSize = 13.sp, fontFamily = FontFamily.Monospace) }
                }
            }
        }
        Spacer(Modifier.height(4.dp))
        Text("连接方式 · Zork Mesh", color = ZorkColors.Muted, fontSize = 14.sp)
        savedPort?.let { Text("手机端口 · $it", color = ZorkColors.Muted, fontSize = 14.sp) }
        Text("各台 Station 可以同时连接，手机和 Station 无需处于同一网络。", color = ZorkColors.Muted, fontSize = 13.sp, lineHeight = 21.sp)
    } }
}

@Composable
private fun AdbButton(label: String, primary: Boolean = false, enabled: Boolean = true, click: () -> Unit) {
    ZorkButton(label, primary = primary, onClick = click, enabled = enabled, modifier = Modifier.fillMaxWidth())
}

@Composable
private fun AdbStationRow(station: JSONObject, enabled: Boolean, help: () -> Unit) {
    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 64.dp)) {
        Box(Modifier.size(32.dp), contentAlignment = Alignment.Center) { DeviceMark(station.text("name").orEmpty(), 28.dp) }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(station.text("name").orEmpty(), color = ZorkColors.Ink, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            Text(station.text("state_label").orEmpty(), color = adbTone(station.text("tone")), fontSize = 13.sp)
        }
        if (station.optJSONObject("help") != null) ZorkButton(station.text("help_label").orEmpty(), quiet = true, onClick = help, enabled = enabled)
    }
}

private fun adbTone(tone: String?): Color = when (tone) {
    "positive" -> ZorkColors.Online
    "warning" -> ZorkColors.Warning
    else -> ZorkColors.Muted
}
