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
    SettingsListRow("清空本机数据", R.drawable.ic_x, subtext = "删除这台手机上的 Zork 数据并退出",
        action = { if (!busy) open = true })
    ZorkRetained(Unit.takeIf { open }) { _, shown, closed ->
        SettingsSheet("清空这台手机上的 Zork 数据？", busy = busy, error = error, dismiss = { open = false },
            open = shown, onClosed = closed) {
            Text("将删除此手机上的消息、草稿、文件缓存、登录信息、配对和设置。电脑上的模型连接、对话和其他数据不会被删除。此操作无法撤销，应用将退出，请重新打开后重新连接设备。",
                fontSize = 14.sp, lineHeight = 22.sp, color = ZorkColors.Muted)
            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                ZorkButton(if (busy) "正在清空…" else "清空并退出", Modifier.fillMaxWidth(), danger = true, enabled = !busy, onClick = actions.clearData)
                ZorkButton("取消", Modifier.fillMaxWidth(), enabled = !busy, onClick = { open = false })
            }
        }
    }
}
