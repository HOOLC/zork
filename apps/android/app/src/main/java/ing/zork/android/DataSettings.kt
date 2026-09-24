package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** A danger row in Settings; the confirmation comes up from the bottom like every app modal. */
@Composable
internal fun ClearDataSettings(actions: SettingsActions) {
    var open by rememberSaveable { mutableStateOf(false) }
    val phase = actions.dataReset?.text("phase").orEmpty()
    val busy = phase == "clearing" || phase == "restarting"
    val error = actions.dataReset?.text("error")?.takeIf { it.isNotBlank() } ?: actions.dataResetError
    SettingsListRow("清空本机数据", R.drawable.ic_x,
        action = { if (!busy) open = true })
    ZorkRetained(Unit.takeIf { open }) { _, shown, closed ->
        SettingsSheet("清空这台手机上的 Zork 数据？", busy = busy, error = error, dismiss = { open = false },
            open = shown, onClosed = closed) {
            Text("本机的缓存、草稿和设置会删除并退出，无法撤销。", fontSize = 14.sp, lineHeight = 22.sp)
            var details by remember { mutableStateOf(false) }
            ZorkButton("电脑上的数据会受影响吗？", quiet = true, onClick = { details = !details })
            ZorkExpand(details) { Text("只删除这台手机上的消息、草稿、文件缓存、登录信息、配对和设置。电脑上的模型连接、对话和其他数据不受影响；重新打开后需要重新连接设备。",
                fontSize = 13.sp, lineHeight = 20.sp, color = ZorkColors.Muted) }
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                ZorkButton(if (busy) "正在清空…" else "清空并退出", Modifier.fillMaxWidth(), danger = true, enabled = !busy, onClick = actions.clearData)
                ZorkButton("取消", Modifier.fillMaxWidth(), enabled = !busy, onClick = { open = false })
            }
        }
    }
}
