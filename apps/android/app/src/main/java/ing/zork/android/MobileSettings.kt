package ing.zork.android

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.ui.graphics.graphicsLayer
import kotlin.math.roundToInt
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

internal data class MobileSettingsState(
    val page: String = "home", val device: Peer? = null, val fromChat: Boolean = false,
    val online: Boolean = false, val loading: Boolean = false, val info: JSONObject? = null,
    val agents: List<JSONObject> = emptyList(), val profiles: List<JSONObject> = emptyList(),
    val profile: JSONObject? = null, val message: String? = null,
    val background: Boolean? = null, val startup: Boolean? = null,
    val providers: List<JSONObject> = emptyList(),
    val helperLeaders: List<JSONObject> = emptyList(),
    val profilesReady: Boolean = true, val profileMessage: String? = null,
    val authorization: JSONObject? = null, val authorizationBusy: Boolean = false,
    val authorizationError: String? = null, val operation: JSONObject? = null,
    val authorizationComplete: Boolean = false,
    val selectedProfileId: String? = null,
    /** Opened from the global model connections list; back returns there. */
    val fromConnections: Boolean = false,
    /** Open the new-connection editor once this device's page is usable. */
    val addConnection: Boolean = false,
    /** Core's per-device model connections; `null` until first read. */
    val connections: List<JSONObject>? = null,
    val connectionState: String = "connecting",
    val command: JSONObject? = null, val updateCheck: JSONObject? = null, val update: JSONObject? = null,
    val profileRefreshing: Set<String> = emptySet(), val profileFailed: Set<String> = emptySet(),
    val resource: ResourceSelection? = null, val resourceData: JSONObject? = null,
    val resourceDepth: Int = 0,
)
internal class SettingsActions(
    val back: () -> Unit = {}, val device: (Peer) -> Unit = {}, val page: (String) -> Unit = {},
    val profile: (JSONObject) -> Unit = {}, val assist: (JSONObject) -> Unit = {},
    val connection: (String, JSONObject) -> Unit = { _, _ -> },
    val addConnection: (String) -> Unit = {},
    val checkUpdate: () -> Unit = {}, val addDevice: () -> Unit = {},
    val refresh: () -> Unit = {},
    val perform: suspend (String, JSONObject) -> JSONObject = { _,_ -> error("设备未连接") },
    /** Local core commands for the built-in model catalog; returns core's `data`. */
    val catalog: suspend (String, JSONObject) -> Any? = { _, _ -> null },
    val theme: String = "system",
    val saveTheme: suspend (String) -> Unit = {},
    val resource: (ResourceSelection) -> Unit = {},
    val notifications: JSONObject? = null,
    val notificationError: String? = null,
    val notificationTarget: Pair<String, String>? = null,
    val notificationAction: suspend (JSONObject) -> Unit = {},
    val testNotification: suspend () -> Unit = {},
    val notificationRefresh: () -> Unit = {},
    val adb: JSONObject? = null,
    val adbError: String? = null,
    val adbAction: suspend (JSONObject) -> Unit = {},
    val adbRefresh: () -> Unit = {},
    val account: JSONObject? = null,
    val accountError: String? = null,
    val accountAction: (String) -> Unit = {},
    val dataReset: JSONObject? = null,
    val dataResetError: String? = null,
    val clearData: () -> Unit = {},
)

@Composable
internal fun MobileSettings(state: MobileSettingsState, peers: List<Peer>, actions: SettingsActions, modifier: Modifier = Modifier) {
    if (state.page == "account") { AccountSettings(actions, modifier); return }
    if (state.page == "adb") { AdbSettings(actions, modifier); return }
    if (state.page == "notifications") {
        NotificationSettings(actions, modifier)
        return
    }
    if (state.resource != null) {
        ResourceSettings(state, actions, modifier)
        return
    }
    if (state.page == "model-connections") {
        ModelConnectionsPage(state, peers, actions, modifier)
        return
    }
    if (state.page in listOf("models", "profile")) {
        ModelSettingsPage(state, actions, modifier)
        return
    }
    if (state.page == "appearance") {
        AppearanceSettings(actions, modifier)
        return
    }
    var editor by rememberSaveable(state.device?.id) { mutableStateOf<String?>(null) }
    var editingJson by rememberSaveable(state.device?.id) { mutableStateOf<String?>(null) }
    val editing = editingJson?.let(::JSONObject)
    // Version comparison and the running check come from core's `update`.
    val update = state.update
    val latest = update?.text("latest").orEmpty()
    val upgrading = state.operation?.optBoolean("running") == true
    val title = if (state.page == "home") "设置" else "设备"
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            ZorkIconButton(if (state.fromChat && state.page == "device") "返回对话" else "返回", onClick = actions.back) { Icon(painterResource(R.drawable.ic_arrow_left), null, Modifier.size(22.dp)) }
            Text(title, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            if (state.page != "home") SettingsRefreshButton(state.loading, actions.refresh)
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (state.page == "home") {
                SectionTitle("客户端")
                SettingsListGroup {
                    val account = actions.account
                    SettingsListRow("Zork 账号", R.drawable.ic_settings,
                        value = account?.text("email")?.takeIf { it.isNotBlank() && !account.text("subject").isNullOrBlank() } ?: "未登录",
                        action = { actions.page("account") })
                    SettingsListRow("外观", R.drawable.ic_settings_three,
                        value = when (actions.theme) { "light" -> "浅色"; "dark" -> "深色"; else -> "跟随系统" },
                        action = { actions.page("appearance") })
                    SettingsListRow("通知", R.drawable.ic_attention,
                        value = actions.notifications?.let { if (it.optBoolean("enabled")) "已开启" else "已关闭" },
                        action = { actions.page("notifications") })
                }
                SectionTitle("Mesh")
                SettingsListGroup {
                    SettingsListRow("模型连接", R.drawable.ic_mesh, value = connectionCount(state), action = { actions.page("model-connections") })
                    peers.forEach { peer ->
                        SettingsListRow(peer.name, leading = { DeviceMark(peer.name, 24.dp) },
                            trailing = { if (peer.status.state !in listOf("direct", "connected")) DeviceStatusBadge(peer.status) },
                            action = { actions.device(peer) })
                    }
                    SettingsListRow("连接设备", R.drawable.ic_plus, action = actions.addDevice)
                }
                SectionTitle("高级")
                SettingsListGroup {
                    SettingsListRow("安卓调试", R.drawable.ic_node, action = { actions.page("adb") })
                    ClearDataSettings(actions)
                }
            } else if (state.page == "device") {
                val device = state.device
                val status = device?.status ?: DeviceStatusUi()
                val current = update?.text("current")?.takeIf { it.isNotBlank() }
                    ?: state.info?.optJSONObject("station")?.let { it.text("release_version", it.text("version")) }?.takeIf { it.isNotBlank() }
                val checking = update?.optBoolean("checking") == true
                val available = update?.optBoolean("available") == true && state.online
                var more by remember { mutableStateOf(false) }
                // First glance: who and whether it is reachable. Name, version and checks live in "更多".
                Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    DeviceMark(device?.name.orEmpty(), 40.dp)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(device?.name.orEmpty(), fontSize = 18.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            DeviceStatusBadge(status)
                            if (status.state in listOf("direct", "connected")) Text(deviceStatusText(status), fontSize = 12.sp, color = ZorkColors.Muted)
                        }
                    }
                    Box {
                        IconAction(R.drawable.ic_more, "更多", enabled = !state.loading) { more = true }
                        PlainMenu("设备操作", more, { more = false }, 200.dp) {
                            ZorkMenuItem("重命名", false, enabled = state.online, onClick = { more = false; editor = "rename" })
                            if (update?.optBoolean("supported") == true && state.online && !available)
                                ZorkMenuItem(if (checking) "正在检查…" else "检查更新", false, enabled = !checking && !upgrading,
                                    onClick = { more = false; actions.checkUpdate() })
                            Text("版本 ${current?.let { "v$it" } ?: "—"}", fontSize = 12.sp, color = ZorkColors.Subtle,
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp))
                        }
                    }
                }
                // The update row exists only when there is something to install.
                if (available) SettingsListRow("可更新到 $latest", R.drawable.ic_reload,
                    trailing = { ZorkButton("更新", primary = true, enabled = !checking && !upgrading, onClick = { editor = "upgrade" }) })
                SettingsListGroup {
                    SettingsListRow("服务", R.drawable.ic_node, action = { actions.page("services") })
                    SettingsListRow("模型连接", R.drawable.ic_mesh, value = if (state.profilesReady) state.profiles.size.toString() else "—",
                        action = { actions.page("models") })
                }
                update?.text("error")?.takeIf { it.isNotBlank() }?.let { Text(it, color = ZorkColors.Danger, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp)) }
                state.operation?.let { operation ->
                    val message = operation.text("error").ifBlank { operation.text("message").ifBlank {
                        if (operation.optBoolean("completed")) "设备升级完成" else if (operation.optBoolean("running")) "正在升级，等待设备恢复连接…" else ""
                    } }
                    if (message.isNotBlank()) Text(message, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp),
                        color = if (operation.text("error").isNotBlank()) ZorkColors.Danger else ZorkColors.Muted)
                }
            }
            state.message?.takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger, modifier = Modifier.padding(horizontal = 8.dp)) }
        }
    }
    ZorkRetained(editor?.let { it to editing }) { (type, source), open, closed -> key(type, source?.text("id")) {
        SettingsEditor(type, source, state, actions, latest, { editor = null }, { editor = null; actions.refresh() },
            open = open, onClosed = closed)
    } }
}
@Composable
internal fun SettingsRefreshButton(rawLoading: Boolean, refresh: () -> Unit) {
    val loading = rememberDeferredLoading(rawLoading)
    val angle = remember { Animatable(0f) }
    val reduced = LocalReducedMotion.current
    LaunchedEffect(loading, reduced) {
        // Reduced motion: no loops, the icon stays still.
        if (reduced) { angle.snapTo(0f); return@LaunchedEffect }
        if (!loading && angle.value == 0f) return@LaunchedEffect
        // Finish the current turn when loading ends; never jump back to zero.
        do {
            val remaining = 360f - angle.value
            angle.animateTo(360f, tween((720f * remaining / 360f).roundToInt().coerceAtLeast(1), easing = LinearEasing))
            angle.snapTo(0f) // 360° and 0° render identically.
        } while (loading)
    }
    ZorkIconButton(if (rawLoading) "正在刷新" else "刷新", onClick = refresh, enabled = !rawLoading) {
        Icon(painterResource(R.drawable.ic_reload), null,
            Modifier.size(18.dp).graphicsLayer { rotationZ = angle.value }, tint = ZorkColors.Ink)
    }
}

@Composable private fun SectionTitle(title: String) {
    Text(title, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle,
        modifier = Modifier.padding(start = 16.dp, top = 20.dp, bottom = 2.dp))
}
