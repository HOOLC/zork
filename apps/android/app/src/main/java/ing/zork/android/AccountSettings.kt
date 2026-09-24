package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

@Composable
internal fun AccountSettings(actions: SettingsActions, modifier: Modifier = Modifier) {
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            ZorkIconButton("返回", onClick = actions.back) { Icon(painterResource(R.drawable.ic_arrow_left), null, Modifier.size(22.dp)) }
            Text("Zork 账号", fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            AccountContent(actions.account, actions.accountError, actions.accountAction)
        }
    }
}

@Composable
internal fun AccountContent(account: JSONObject?, error: String?, action: (String) -> Unit) {
    val phase = account?.text("phase", "idle") ?: "idle"
    val busy = phase != "idle"
    val signed = !account?.text("subject").isNullOrBlank()
    var more by remember { mutableStateOf(false) }
    var technical by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(if (signed) account!!.text("email", "Zork 账号") else "登录 Google 账号连接设备", fontSize = 16.sp, fontWeight = FontWeight.Medium)
            Text(if (signed) "Google 账号" else "与电脑登录同一账号后自动连接", fontSize = 12.sp, color = ZorkColors.Subtle)
        }
        // Rare account actions live in "更多".
        if (signed && !busy) Box {
            IconAction(R.drawable.ic_more, "更多") { more = true }
            PlainMenu("账号操作", more, { more = false }, 180.dp) {
                ZorkMenuItem("退出 Zork 账号", false, onClick = { more = false; action("logout") })
            }
        }
    }
    if (busy) {
        Text(if (phase == "signing_out") "正在退出…" else "请在浏览器中完成 Google 登录。", color = ZorkColors.Muted, fontSize = 13.sp)
        if (phase != "signing_out") ZorkButton("取消登录", quiet = true, onClick = { action("cancel") })
        account?.text("login_url")?.takeIf { it.isNotBlank() }?.let { url -> SelectionContainer { Text(url, fontSize = 12.sp) } }
    } else if (signed) {
        if (account?.optBoolean("authenticated") != true) Text("正在恢复账号连接…", color = ZorkColors.Muted, fontSize = 13.sp)
    } else ZorkButton("使用 Google 登录", primary = true, onClick = { action("login") }, modifier = Modifier.fillMaxWidth())
    if ((account?.optInt("pending_revocations") ?: 0) > 0) Text("已退出本机，正在等待服务器确认撤销。", color = ZorkColors.Muted, fontSize = 13.sp)
    (error ?: account?.text("error")?.takeIf { it.isNotBlank() })?.let { Text(it, color = ZorkColors.Danger, fontSize = 13.sp) }
    // Identity details are for troubleshooting only.
    if (signed || account?.text("origin")?.isNotBlank() == true) {
        ZorkButton(if (technical) "收起技术信息" else "技术信息", quiet = true, onClick = { technical = !technical })
        if (technical) SelectionContainer { Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            account?.text("origin")?.takeIf { it.isNotBlank() }?.let { Text("服务 $it", fontSize = 12.sp, color = ZorkColors.Subtle) }
            account?.text("subject")?.takeIf { it.isNotBlank() }?.let { Text("账号 ID $it", fontSize = 12.sp, color = ZorkColors.Subtle,
                fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace) }
        } }
    }
}
