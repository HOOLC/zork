package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
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
            LiquidIconButton("返回", onClick = actions.back) { Icon(painterResource(R.drawable.ic_arrow_left), null, Modifier.size(22.dp)) }
            Text("Zork 账号", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            AccountContent(actions.account, actions.accountError, actions.accountAction)
        }
    }
}

@Composable
internal fun AccountContent(account: JSONObject?, error: String?, action: (String) -> Unit) {
    val phase = account?.text("phase", "idle") ?: "idle"
    val busy = phase != "idle"
    val signed = !account?.text("subject").isNullOrBlank()
    Text(if (signed) account!!.text("email", "Zork 账号") else "登录 Zork 以使用公网连接", fontSize = 16.sp, fontWeight = FontWeight.Medium)
    Text("使用 Google 账号授权 Zork。局域网无需登录，设备访问权限仍通过邀请授予。", color = ZorkColors.Muted, fontSize = 13.sp, lineHeight = 21.sp)
    if (busy) {
        Text(if (phase == "signing_out") "正在退出…" else "请在浏览器中完成 Google 登录。", color = ZorkColors.Muted, fontSize = 13.sp)
        if (phase != "signing_out") LiquidButton("取消登录", quiet = true, onClick = { action("cancel") })
        account?.text("login_url")?.takeIf { it.isNotBlank() }?.let { url -> SelectionContainer { Text(url, fontSize = 12.sp) } }
    } else if (signed) {
        if (account?.optBoolean("authenticated") != true) Text("正在恢复公网连接…", color = ZorkColors.Muted, fontSize = 13.sp)
        LiquidButton("退出 Zork 账号", quiet = true, onClick = { action("logout") })
    } else LiquidButton("使用 Google 登录", primary = true, onClick = { action("login") }, modifier = Modifier.fillMaxWidth())
    if ((account?.optInt("pending_revocations") ?: 0) > 0) Text("已退出本机，正在等待服务器确认撤销。", color = ZorkColors.Muted, fontSize = 13.sp)
    (error ?: account?.text("error")?.takeIf { it.isNotBlank() })?.let { Text(it, color = ZorkColors.Danger, fontSize = 13.sp) }
}
