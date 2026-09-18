package ing.zork.android

import android.Manifest
import android.content.Intent
import android.os.Build
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

@Composable
internal fun NotificationSettings(actions: SettingsActions, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val preferences = actions.notifications
    var allowed by remember { mutableStateOf(NotificationPlatform.allowed(context)) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var pendingPermission by rememberSaveable { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    fun run(work: suspend () -> Unit) { scope.launch {
        busy = true; message = null
        try { work() }
        catch (e: CancellationException) { throw e }
        catch (e: Exception) { message = e.message }
        finally { busy = false }
    } }
    fun change(action: String, value: Boolean) = run { actions.notificationAction(JSONObject().put("action", action).put("value", value)) }
    fun test() = run { actions.testNotification(); message = "已发送测试通知" }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        allowed = NotificationPlatform.allowed(context)
        actions.notificationRefresh()
        if (granted) when (pendingPermission) {
            "test" -> test(); "background" -> change("background", true); "enabled" -> change("enabled", true)
        } else message = "系统未允许通知，可在系统通知设置中开启。"
        pendingPermission = ""
    }
    fun request(next: String) {
        if (Build.VERSION.SDK_INT >= 33 && !allowed) { pendingPermission = next; permission.launch(Manifest.permission.POST_NOTIFICATIONS) }
        else when (next) { "test" -> test(); "background" -> change("background", true); "enabled" -> change("enabled", true) }
    }
    DisposableEffect(owner, context) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) allowed = NotificationPlatform.allowed(context) }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    SettingsPageFrame("通知", actions.back, busy, { allowed = NotificationPlatform.allowed(context); actions.notificationRefresh() }, modifier) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            val enabled = preferences?.optBoolean("enabled") == true
            SettingsToggle("接收通知", enabled, !busy && preferences != null, "新回复、待验收和需要处理的任务") {
                if (it) request("enabled") else change("enabled", false)
            }
            SettingsToggle("显示会话名称", preferences?.optBoolean("preview") == true, !busy && preferences != null, "关闭时只显示 Zork 和事件类型") { change("preview", it) }
            SettingsToggle("提示音", preferences?.optBoolean("sound") == true, !busy && preferences != null, "通知声音也可在系统通知设置中调整") { change("sound", it) }
            actions.notificationTarget?.let { (peer, session) ->
                val muted = preferences?.optJSONArray("muted")?.let { rows ->
                    (0 until rows.length()).any { rows.optJSONArray(it)?.let { row -> row.optString(0) == peer && row.optString(1) == session } == true }
                } == true
                SettingsToggle("当前会话免打扰", muted, !busy && preferences != null, "保留未读提示，不发送系统通知") { value ->
                    run { actions.notificationAction(JSONObject().put("action", "mute").put("peer", peer).put("session", session).put("value", value)) }
                }
            }
            SettingsToggle("离开应用后保持连接", preferences?.optBoolean("background") == true, !busy && enabled,
                "保持设备连接以接收通知，会显示常驻通知并增加耗电。系统停止应用后需重新打开。") {
                if (it) request("background") else change("background", false)
            }
            Text(if (allowed) "系统已允许通知" else "系统未允许通知", color = if (allowed) ZorkColors.Muted else ZorkColors.Warning, fontSize = 13.sp)
            if (!allowed && Build.VERSION.SDK_INT >= 33) SettingsButton("允许系统通知", enabled = !busy) { request("permission") }
            SettingsButton("发送测试通知", enabled = enabled && !busy) { request("test") }
            SettingsButton("打开系统通知设置") {
                context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName))
            }
            message?.let { Text(it, color = ZorkColors.Muted, fontSize = 13.sp) }
            actions.notificationError?.let { Text(it, color = ZorkColors.Danger, fontSize = 13.sp) }
        }
    }
}
