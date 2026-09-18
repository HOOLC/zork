package ing.zork.android

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.ui.graphics.graphicsLayer
import kotlin.math.roundToInt
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import kotlinx.coroutines.launch

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
    val connectionState: String = "connecting",
    val command: JSONObject? = null, val updateCheck: JSONObject? = null,
    val profileRefreshing: Set<String> = emptySet(), val profileFailed: Set<String> = emptySet(),
    val resource: ResourceSelection? = null, val resourceData: JSONObject? = null,
    val resourceDepth: Int = 0,
)
internal class SettingsActions(
    val back: () -> Unit = {}, val device: (Peer) -> Unit = {}, val page: (String) -> Unit = {},
    val profile: (JSONObject) -> Unit = {}, val assist: (JSONObject) -> Unit = {},
    val checkUpdate: () -> Unit = {}, val addDevice: () -> Unit = {},
    val refresh: () -> Unit = {},
    val perform: suspend (String, JSONObject) -> JSONObject = { _,_ -> error("设备未连接") },
    val messagePreviewHeight: Int = 0,
    val saveMessagePreviewHeight: suspend (Int) -> Unit = {},
    val resource: (ResourceSelection) -> Unit = {},
    val skills: (JSONObject) -> Unit = {},
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
    val dataReset: JSONObject? = null,
    val dataResetError: String? = null,
    val clearData: () -> Unit = {},
)

@Composable
internal fun MobileSettings(state: MobileSettingsState, peers: List<Peer>, actions: SettingsActions, modifier: Modifier = Modifier) {
    if (state.page == "adb") { AdbSettings(actions, modifier); return }
    if (state.page == "notifications") {
        NotificationSettings(actions, modifier)
        return
    }
    if (state.resource != null) {
        ResourceSettings(state, actions, modifier)
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
    val scope = rememberCoroutineScope()
    var updateBusy by remember { mutableStateOf(false) }
    var updateError by remember { mutableStateOf<String?>(null) }
    val latest = state.updateCheck?.text("latest_version").orEmpty()
    val upgrading = state.operation?.optBoolean("running") == true
    val info = state.info
    val supported = info?.optJSONObject("update")?.optBoolean("supported") == true
    val title = when(state.page) { "home" -> "设置"; "device" -> "设备"; "agents" -> "队员"; "models" -> "大模型"; else -> "连接详情" }
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            LiquidIconButton(if (state.fromChat && state.page == "device") "返回对话" else "返回", onClick = actions.back) { Icon(painterResource(R.drawable.ic_arrow_left), null, Modifier.size(22.dp)) }
            Text(title, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            if (state.page != "home") SettingsRefreshButton(state.loading, actions.refresh)
        }
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
            if (state.page in listOf("models","profile")) state.profileMessage?.let { Text(it,color=ZorkColors.Danger,fontSize=13.sp) }
            if (state.page == "home") {
                SettingsListGroup {
                    SettingsListRow("工具连接", R.drawable.ic_mesh, subtext = "查看设备上的工具及连接状态", action = { actions.page("connections") })
                }
                SectionTitle("客户端")
                SettingsListGroup {
                    SettingsListRow("外观", R.drawable.ic_settings, subtext = "消息折叠高度", action = { actions.page("appearance") })
                    SettingsListDivider()
                    SettingsListRow("通知", R.drawable.ic_settings, subtext = "消息提醒、免打扰与后台连接", action = { actions.page("notifications") })
                    SettingsListDivider()
                    SettingsListRow("安卓调试", R.drawable.ic_settings, subtext = "通过 Mesh 安装应用和调试", action = { actions.page("adb") })
                }
                SectionTitle("设备", peers.size.toString())
                if (peers.isNotEmpty()) SettingsListGroup {
                    peers.forEachIndexed { index, peer ->
                        if (index > 0) SettingsListDivider()
                        SettingsListRow(peer.name, R.drawable.ic_node, action = { actions.device(peer) })
                    }
                }
                SettingsButton("连接设备", click = actions.addDevice)
                SectionTitle("数据")
                ClearDataSettings(actions)
            } else if (state.page == "device") {
                Column(Modifier.fillMaxWidth().background(ZorkColors.Paper, SettingsStyle.Card).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Box(Modifier.size(44.dp).background(ZorkColors.Canvas, SettingsStyle.Field), contentAlignment = Alignment.Center) { Glyph(R.drawable.ic_node, 25.dp, ZorkColors.Muted) }
                        Text(state.device?.name.orEmpty(), modifier = Modifier.weight(1f), fontSize = 18.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        if (state.online) LiquidIconButton("修改设备名称", opensPanel = true, onClick = { editor = "rename" }, enabled = !state.loading) { Icon(painterResource(R.drawable.ic_edit), null, Modifier.size(18.dp)) }
                    }
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(Modifier.size(6.dp).background(if (state.online) ZorkColors.Online else ZorkColors.Muted, androidx.compose.foundation.shape.CircleShape))
                        Text(if (state.online) "在线" else if (state.connectionState == "connecting") "连接中" else "离线", fontSize = 12.sp, color = ZorkColors.Muted)
                        Text("·", color = ZorkColors.Muted)
                        Text(info?.optJSONObject("station")?.let { "v"+it.text("release_version",it.text("version","—")) } ?: "版本待获取", fontSize = 12.sp, color = ZorkColors.Muted)
                    }
                }
                SectionTitle("管理")
                SettingsListGroup {
                    SettingsListRow("队员", avatar = "cat", value = state.agents.size.toString(), subtext = "领队与队员的分工、头像和模型", action = { actions.page("agents") })
                    SettingsListDivider()
                    SettingsListRow("大模型", R.drawable.ic_mesh, value = if(state.profilesReady) state.profiles.size.toString() else "—", subtext = "连接账号，管理可用模型", action = { actions.page("models") })
                    SettingsListDivider()
                    SettingsListRow("服务", R.drawable.ic_node, subtext = "运行状态、共享信息与日志", action = { actions.page("services") })
                }
                if (supported && state.online) {
                SectionTitle("版本更新")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    SettingsButton(if(updateBusy) "正在处理…" else "检查更新", enabled = supported && state.online && !updateBusy && !upgrading) {
                        scope.launch { updateBusy=true; updateError=null; try { actions.perform("check_update", JSONObject()) } catch(e:kotlinx.coroutines.CancellationException) { throw e } catch(e:Exception) { updateError=e.message } finally { updateBusy=false } }
                    }
                }
                if (latest.isNotBlank()) {
                    val version=info?.optJSONObject("station")?.let { it.text("release_version",it.text("version")) }
                    if (latest == version) Text("已是最新版本", color=ZorkColors.Muted, fontSize=13.sp)
                    else { Text("可升级至 $latest",fontSize=13.sp); SettingsButton("升级并重启",primary=true,enabled=supported && !updateBusy && !upgrading) { editor="upgrade" } }
                }
                info?.optJSONObject("update")?.optJSONObject("status")?.text("message")?.takeIf { it.isNotBlank() }?.let { Text(it,fontSize=13.sp,color=ZorkColors.Muted) }
                updateError?.let { Text(it,color=ZorkColors.Danger,fontSize=13.sp) }
                }
                state.operation?.let { operation ->
                    val message = operation.text("error").ifBlank { operation.text("message").ifBlank {
                        if (operation.optBoolean("completed")) "设备升级完成" else if (operation.optBoolean("running")) "正在升级，等待设备恢复连接…" else ""
                    } }
                    if (message.isNotBlank()) Text(message, fontSize = 13.sp,
                        color = if (operation.text("error").isNotBlank()) ZorkColors.Danger else ZorkColors.Muted)
                }
            } else if (state.page == "agents") {
                PageHeading("队员", "${state.device?.name.orEmpty()} · ${state.agents.size} 位小伙伴", "添加", !state.loading && state.online) { editingJson=null;editor="agent" }
                listOf("leader" to "领队", "worker" to "队员").forEach { (role,label) ->
                    val members=state.agents.filter { it.text("role")==role }
                    if(members.isNotEmpty()) {
                        SectionTitle(label,members.size.toString())
                        Text(if(role=="leader") "与你沟通，安排任务与队员" else "接受领队安排，专注完成任务",fontSize=12.sp,color=ZorkColors.Muted)
                        SettingsListGroup {
                            members.forEachIndexed { index, agent ->
                                if (index > 0) SettingsListDivider()
                                SettingsListRow(agent.text("name"), avatar = agent.text("avatar"), subtext = "${agent.text("model","未配置模型")} · ${agent.text("profile_id","未配置连接").let { if(it=="auto" || it.isBlank()) "自动分配" else it }}", action = {editingJson=agent.toString();editor="agent"})
                            }
                        }
                    }
                }
                if(state.agents.isEmpty()) EmptySettings("让第一位领队加入", "添加领队，开始对话并安排任务。")
            }
            state.message?.takeIf{it.isNotBlank()}?.let { Text(it,fontSize=13.sp,color=ZorkColors.Danger) }
        }
    }
    LiquidRetained(editor?.let { it to editing }) { (type, source), open, closed -> key(type, source?.text("id")) {
        SettingsEditor(type,source,state,actions,latest,{editor=null},{editor=null;actions.refresh()},
            manageGrants={editingJson=it.toString();editor="grants"}, open=open, onClosed=closed)
    } }
}
@Composable
internal fun SettingsRefreshButton(loading: Boolean, refresh: () -> Unit) {
    val angle = remember { Animatable(0f) }
    LaunchedEffect(loading) {
        if (!loading && angle.value == 0f) return@LaunchedEffect
        // Finish the current turn when loading ends; never jump back to zero.
        do {
            val remaining = 360f - angle.value
            angle.animateTo(360f, tween((720f * remaining / 360f).roundToInt().coerceAtLeast(1), easing = LinearEasing))
            angle.snapTo(0f) // 360° and 0° render identically.
        } while (loading)
    }
    LiquidIconButton(if (loading) "正在刷新" else "刷新", onClick = refresh, enabled = !loading) {
        Icon(painterResource(R.drawable.ic_reload), null,
            Modifier.size(18.dp).graphicsLayer { rotationZ = angle.value }, tint = ZorkColors.Ink)
    }
}

@Composable private fun PageHeading(title:String, detail:String, action:String, enabled:Boolean, click:()->Unit) {
    Row(verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f),verticalArrangement=Arrangement.spacedBy(6.dp)){if(title !in listOf("队员","大模型")) Text(title,fontSize=24.sp,fontWeight=FontWeight.SemiBold,maxLines=2,overflow=TextOverflow.Ellipsis);Text(detail,fontSize=12.sp,color=ZorkColors.Muted)}
        if(enabled) SettingsButton(action,click=click)
    }
}
@Composable private fun SectionTitle(title:String,count:String?=null) { Row(horizontalArrangement=Arrangement.spacedBy(8.dp),verticalAlignment=Alignment.CenterVertically){Text(title,fontSize=13.sp,fontWeight=FontWeight.Medium);count?.let{Text(it,fontSize=12.sp,color=ZorkColors.Muted)}} }
@Composable private fun EmptySettings(title:String,detail:String){Column(Modifier.fillMaxWidth().padding(vertical=24.dp),verticalArrangement=Arrangement.spacedBy(8.dp)){Text(title,fontSize=16.sp,fontWeight=FontWeight.Medium);Text(detail,fontSize=13.sp,lineHeight=21.sp,color=ZorkColors.Muted)}}
