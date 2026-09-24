package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

internal data class NewChatUi(val peer: Peer, val snapshot: JSONObject = JSONObject())

@Composable
internal fun NewChatPage(state: NewChatUi, back: () -> Unit, action: (String, String?) -> Unit, configureModels: () -> Unit,
    peers: List<Peer> = emptyList(), selectDevice: (Peer) -> Unit = {}) {
    val data = state.snapshot
    var text by remember(state.peer.id) { mutableStateOf(data.text("text")) }
    // The buffer belongs to the editor. Only restore it on entry or when core
    // freezes the accepted submission; older live echoes never move its caret.
    var restored by remember(state.peer.id) { mutableStateOf(data.has("text")) }
    LaunchedEffect(data.text("text"), data.optBoolean("editable")) {
        if (!restored && data.has("text")) { text = data.text("text"); restored = true }
        if (data.optBoolean("busy") || data.optBoolean("uncertain")) text = data.text("text")
    }
    val editable = data.optBoolean("editable")
    val switching = peers.size > 1
    var picking by remember(state.peer.id) { mutableStateOf(false) }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = back)
            Column(Modifier.weight(1f).padding(start = 8.dp)) {
                Text("新建 Chat", fontSize = 17.sp, fontWeight = FontWeight.Medium)
                // With several devices the tabs above the composer carry the target.
                if (!switching) DeviceName(state.peer.name, state.peer.status)
            }
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val presence = rememberComposerPresence(WorkbenchState(), (maxWidth - 24.dp).value)
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 24.dp),
                verticalArrangement = Arrangement.Center) {
                Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(painterResource(R.drawable.ic_zork), contentDescription = null, Modifier.size(40.dp), tint = ZorkColors.Ink)
                    Text("想让哪台设备开始工作？", fontSize = 20.sp, lineHeight = 28.sp, fontWeight = FontWeight.SemiBold,
                        textAlign = TextAlign.Center, modifier = Modifier.padding(top = 16.dp))
                    Spacer(Modifier.height(24.dp))
                }
                if (switching) NewChatDeviceTabs(peers, state.peer, !data.optBoolean("busy"), selectDevice)
                // The composer's outline covers the tabs' lower edge.
                DraftComposer(text, emptyList(), editable, false, data.optBoolean("can_submit"), presence,
                    if (switching) Modifier.offset(y = (-12).dp) else Modifier, 180.dp,
                    WorkbenchActions(draft = { value -> text = value; action("edit", value) }, send = { action("submit", text) }), showAttach = false)
                val model = data.pickerChoice("model")
                val thinking = data.pickerChoice("thinking")
                val profile = data.pickerChoice("profile")
                Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (model.options.isNotEmpty()) ModelCapsule(model, thinking, editable) { picking = true }
                    when {
                        data.optBoolean("busy") -> "正在创建 Chat…"
                        data.optBoolean("uncertain") -> "创建结果尚未确认，重试会继续同一次创建"
                        data.optBoolean("loading", true) -> "正在加载模型…"
                        else -> null
                    }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
                    if (data.optBoolean("needs_model")) SettingsButton("添加模型连接", primary = true, click = configureModels)
                    data.text("error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger) }
                }
                ZorkRetained(Unit.takeIf { picking }) { _, open, closed ->
                    ModelPickerSheet(open, model, thinking, profile, editable, { field, value -> action(field, value) },
                        { picking = false }, onClosed = closed)
                }
            }
        }
    }
}

/** One tab per device: its mark, name and reachability, chosen with one tap. */
@Composable
private fun NewChatDeviceTabs(peers: List<Peer>, current: Peer, enabled: Boolean, select: (Peer) -> Unit) {
    Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        peers.forEach { peer ->
            val selected = peer.id == current.id
            Row(Modifier.height(48.dp)
                .background(if (selected) ZorkColors.Selected else ZorkColors.Prompt, RoundedCornerShape(topStart = 16.dp, topEnd = 16.dp))
                .selectable(selected, enabled = enabled, role = Role.Tab) { if (!selected) select(peer) }
                .semantics { contentDescription = deviceNameSummary(peer.name, peer.status) }
                .padding(start = 10.dp, end = 14.dp, bottom = 12.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                DeviceMark(peer.name, 18.dp)
                Text(peer.name, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                    color = if (selected) ZorkColors.Ink else ZorkColors.Muted, maxLines = 1)
                DeviceStatusBadge(peer.status)
            }
        }
    }
}
