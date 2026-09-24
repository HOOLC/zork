package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch

/** Appearance is the theme alone; text size follows the system. The choice is
 * persisted by the client core and applies to this phone only. */
@Composable
internal fun AppearanceSettings(actions: SettingsActions, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    fun choose(value: String) {
        if (saving || value == actions.theme) return
        scope.launch {
            saving = true; error = null
            try { actions.saveTheme(value) }
            catch (e: kotlinx.coroutines.CancellationException) { throw e }
            catch (e: Exception) { error = e.message ?: "外观设置保存失败" }
            finally { saving = false }
        }
    }
    SettingsPageFrame("外观", actions.back, loading = false, refresh = null, modifier = modifier) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("主题", fontSize = 13.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle)
            SettingsListGroup {
                listOf("system" to "跟随系统", "light" to "浅色", "dark" to "深色").forEachIndexed { index, (value, label) ->
                    if (index > 0) SettingsListDivider()
                    SettingsListRow(label, subtext = if (value == "system") "随手机的浅色 / 深色模式切换" else null,
                        trailing = { if (actions.theme == value) Icon(painterResource(R.drawable.ic_check), contentDescription = "已选择", tint = ZorkColors.Ink) },
                        action = { choose(value) })
                }
            }
            Text("字号跟随系统设置，不单独提供字号选项。", fontSize = 13.sp, color = ZorkColors.Subtle)
            error?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger) }
        }
    }
}
