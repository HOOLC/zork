package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

internal data class NewChatUi(val peer: Peer, val snapshot: JSONObject = JSONObject())

@Composable
internal fun NewChatPage(state: NewChatUi, back: () -> Unit, action: (String, String?) -> Unit, configureModels: () -> Unit) {
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
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = back)
            Column(Modifier.weight(1f).padding(start = 8.dp)) {
                Text("新建 Chat", fontSize = 17.sp, fontWeight = FontWeight.Medium)
                DeviceName(state.peer.name, state.peer.status)
            }
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val presence = rememberComposerPresence(WorkbenchState(), (maxWidth - 24.dp).value)
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 24.dp),
                verticalArrangement = Arrangement.Center) {
                Text("今天想做些什么？", fontSize = 24.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(start = 12.dp, bottom = 28.dp))
                DraftComposer(text, emptyList(), editable, false, data.optBoolean("can_submit"), presence,
                    Modifier, 180.dp, WorkbenchActions(draft = { value -> text = value; action("edit", value) }, send = { action("submit", text) }), showAttach = false)
                Column(Modifier.padding(horizontal = 12.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    listOf("model" to "模型", "thinking" to "思考深度", "profile" to "Profile · 可选").forEach { (field, label) ->
                        val choice = data.optJSONObject(field) ?: JSONObject()
                        val options = choice.optJSONArray("options").objects().map {
                            it.text("value") to if (field == "profile" && it.text("value") == "auto") "自动分配" else it.text("label")
                        }
                        SettingsSelect(label, choice.text("value"), options, editable && options.isNotEmpty()) { action(field, it) }
                    }
                    Text(when {
                        data.optBoolean("busy") -> "正在创建 Chat…"
                        data.optBoolean("uncertain") -> "创建结果尚未确认，重试会继续同一次创建"
                        data.optBoolean("loading", true) -> "正在加载模型…"
                        else -> "发送第一条消息时创建 Chat 和 Session"
                    }, fontSize = 12.sp, color = ZorkColors.Muted)
                    if (data.optBoolean("needs_model")) SettingsButton("添加模型连接", primary = true, click = configureModels)
                    data.text("error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger) }
                }
            }
        }
    }
}
