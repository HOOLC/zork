package surf.zork.android

import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.key

@Composable
internal fun LocalScriptPanel(platform: LocalScriptPlatform) {
    LiquidRetained(platform.run) { run, open, closed ->
    val id = run.id
    key(id) {
        SettingsSheet(run.title, error = run.error, dismiss = { platform.action("dismiss", id) }, open = open, onClosed = closed,
            footer = if (run.canCancel) { { SettingsButton("停止执行") { platform.action("cancel", id) } } } else null) {
            Text(when (run.phase) { "running" -> "正在本机执行…"; "succeeded" -> "脚本已完成"; "cancelled" -> "脚本已取消"; else -> "脚本执行失败" })
            if (run.logs.isNotEmpty()) SelectionContainer { Text(run.logs.joinToString("\n")) }
        }
    }
    }
}
