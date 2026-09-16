package surf.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

@Composable
internal fun ClearDataSettings(actions: SettingsActions) {
    var open by rememberSaveable { mutableStateOf(false) }
    val phase = actions.dataReset?.text("phase").orEmpty()
    val busy = phase == "clearing" || phase == "restarting"
    val error = actions.dataReset?.text("error")?.takeIf { it.isNotBlank() } ?: actions.dataResetError
    SettingsButton("清空数据", enabled = !busy) { open = true }
    LiquidDialog(open, "清空客户端数据？", { if (!busy) open = false }) {
        Text("清空客户端数据？", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        Text("将删除此手机上的消息、草稿、文件缓存、登录信息、配对和设置。电脑上的 Profile、Agent 和其他数据不会被删除。此操作无法撤销，应用将退出，请重新打开后重新连接设备。",
            fontSize = 14.sp, lineHeight = 22.sp)
        error?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger) }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            LiquidButton("取消", Modifier.weight(1f), enabled = !busy, onClick = { open = false })
            LiquidButton(if (busy) "正在清空…" else "确认清空", Modifier.weight(1f), primary = true, enabled = !busy, onClick = actions.clearData)
        }
    }
}
