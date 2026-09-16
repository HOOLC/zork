package surf.zork.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import kotlinx.coroutines.*
import org.json.JSONObject

/** OS capabilities only. Core supplies events, privacy, retention and receipts. */
internal object NotificationPlatform {
    private val hostScope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    fun releaseHost(repo: ClientRepository, generation: Long) {
        hostScope.launch { runCatching { repo.command("host_visibility", "visible" to false, "generation" to generation) } }
    }
    private const val MESSAGES = "zork-messages-v1"
    private const val SILENT = "zork-messages-silent-v1"
    private const val CONNECTION = "zork-connection-v1"
    const val SERVICE_ID = 3720
    const val NOTICE_ID = 1
    private fun manager(context: Context) = context.getSystemService(NotificationManager::class.java)
    fun allowed(context: Context) = manager(context).areNotificationsEnabled()
    private fun channels(context: Context) {
        manager(context).createNotificationChannels(listOf(
            NotificationChannel(MESSAGES, "消息与任务", NotificationManager.IMPORTANCE_DEFAULT),
            NotificationChannel(SILENT, "消息与任务（静音）", NotificationManager.IMPORTANCE_DEFAULT).apply { setSound(null, null); enableVibration(false) },
            NotificationChannel(CONNECTION, "后台设备连接", NotificationManager.IMPORTANCE_LOW).apply { setSound(null, null); setShowBadge(false) },
        ))
    }
    private fun open(context: Context, tag: String? = null): PendingIntent {
        val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        if (tag != null) {
            intent.putExtra("notification_tag", tag)
            intent.data = Uri.Builder().scheme("zork").authority("notification").appendQueryParameter("tag", tag).build()
        }
        return PendingIntent.getActivity(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }
    fun connection(context: Context): Notification {
        channels(context)
        return Notification.Builder(context, CONNECTION).setSmallIcon(R.drawable.ic_zork)
            .setContentTitle("Zork 正在保持设备连接").setContentText("接收消息和任务通知；可在 Zork 通知设置中关闭。")
            .setOngoing(true).setContentIntent(open(context)).build()
    }
    fun post(context: Context, value: JSONObject): Boolean {
        channels(context)
        val channel = if (value.optBoolean("sound")) MESSAGES else SILENT
        if (!allowed(context) || manager(context).getNotificationChannel(channel)?.importance == NotificationManager.IMPORTANCE_NONE) return false
        val tag = value.getString("tag")
        val id = value.getString("id")
        if (manager(context).activeNotifications.any { it.tag == tag && it.notification.extras.getString("zork_event") == id }) return true
        val body = when (value.text("kind")) {
            "reply" -> "收到新的回复"; "review" -> "任务已交付，等待验收"; "attention" -> "任务需要你处理"
            else -> "通知已连接，点击可返回 Zork"
        }
        val public = Notification.Builder(context, channel).setSmallIcon(R.drawable.ic_zork).setContentTitle("Zork").setContentText(body).build()
        val notification = Notification.Builder(context, channel).setSmallIcon(R.drawable.ic_zork)
            .setContentTitle(value.getString("title")).setContentText(body).setContentIntent(open(context, tag))
            .setAutoCancel(true).setCategory(Notification.CATEGORY_MESSAGE).setVisibility(Notification.VISIBILITY_PRIVATE).setPublicVersion(public)
            .addExtras(android.os.Bundle().apply { putString("zork_event", id) }).build()
        return try { manager(context).notify(tag, NOTICE_ID, notification); true }
        catch (_: SecurityException) { false }
    }
    suspend fun apply(context: Context, repo: ClientRepository, snapshot: JSONObject) {
        val retained = snapshot.optJSONArray("retained")?.let { a -> (0 until a.length()).map { a.getString(it) }.toSet() }.orEmpty()
        manager(context).activeNotifications.filter {
            it.notification.channelId in listOf(MESSAGES, SILENT) && it.tag != "zork-notification-test" && it.tag !in retained
        }.forEach { manager(context).cancel(it.tag, it.id) }
        for (value in snapshot.optJSONArray("pending").objects()) {
            if (post(context, value)) withContext(NonCancellable) {
                runCatching { repo.command("notification_receipt", "peer" to value.getString("peer"), "id" to value.getString("id")) }
            }
        }
    }
    fun updateService(context: Context, requested: Boolean) {
        if (requested) {
            if (!ZorkNotificationService.running) context.startForegroundService(Intent(context, ZorkNotificationService::class.java))
        } else context.stopService(Intent(context, ZorkNotificationService::class.java))
    }
}

/** A user-enabled remote-messaging host keeps the same native runtime alive. */
class ZorkNotificationService : Service() {
    companion object { @Volatile internal var running = false }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var job: Job? = null
    private val instance = NativeBridge.newId()
    override fun onCreate() {
        super.onCreate()
        val notification = NotificationPlatform.connection(this)
        if (Build.VERSION.SDK_INT >= 34) startForeground(NotificationPlatform.SERVICE_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
        else startForeground(NotificationPlatform.SERVICE_ID, notification)
        running = true
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (job == null) job = scope.launch {
            val repo = ClientRepository(applicationContext)
            try {
                val settings = repo.command("background_service", "running" to true, "instance" to instance)
                if (!settings.optBoolean("service_requested")) { stopSelf(); return@launch }
                repo.notificationEvents().collect { frame ->
                    val data = frame.value.getJSONObject("snapshot")
                    NotificationPlatform.apply(applicationContext, repo, data)
                    if (!data.getJSONObject("settings").optBoolean("service_requested")) stopSelf()
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                android.util.Log.w("ZorkNotifications", "Background connection ended", e)
                stopSelf()
            } finally {
                withContext(NonCancellable) { runCatching { repo.command("background_service", "running" to false, "instance" to instance) } }
            }
        }
        return START_STICKY
    }
    override fun onDestroy() { running = false; scope.cancel(); super.onDestroy() }
    override fun onBind(intent: Intent?): IBinder? = null
}
