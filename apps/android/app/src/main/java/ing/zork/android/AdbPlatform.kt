package ing.zork.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.ServiceInfo
import android.database.ContentObserver
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import kotlinx.coroutines.*
import org.json.JSONObject

/** Supplies OS observations and foreground-service lifetime. Core owns ADB. */
internal class AdbPlatform(context: Context, private val repo: ClientRepository, private val scope: CoroutineScope) {
    private val resolver = context.applicationContext.contentResolver
    private val applicationId = context.packageName
    private var observing = false
    private val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
        override fun onChange(selfChange: Boolean) { refresh() }
    }
    fun start() {
        if (observing) return
        observing = true
        listOf(Settings.Global.DEVELOPMENT_SETTINGS_ENABLED, Settings.Global.ADB_ENABLED, "adb_wifi_enabled").forEach {
            runCatching { resolver.registerContentObserver(Settings.Global.getUriFor(it), false, observer) }
        }
        refresh()
    }
    fun stop() { if (observing) resolver.unregisterContentObserver(observer); observing = false }
    fun refresh() { scope.launch(Dispatchers.IO) {
        fun flag(name: String): Any = try { Settings.Global.getInt(resolver, name) != 0 } catch (_: Exception) { JSONObject.NULL }
        val facts = JSONObject().put("supported", true).put("name", Build.MODEL.take(64).filterNot { it.isISOControl() })
            .put("application_id", applicationId)
            .put("developer_enabled", flag(Settings.Global.DEVELOPMENT_SETTINGS_ENABLED))
            .put("usb_enabled", flag(Settings.Global.ADB_ENABLED)).put("wireless_enabled", flag("adb_wifi_enabled"))
        runCatching { repo.command("adb", "operation" to JSONObject().put("action", "facts").put("facts", facts)) }
    } }
    companion object {
        fun openSystemSettings(context: Context, destination: String) {
            val action = when (destination) {
                "open_device_info" -> Settings.ACTION_DEVICE_INFO_SETTINGS
                "open_developer_options" -> Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS
                else -> return
            }
            try { context.startActivity(Intent(action)) }
            catch (_: android.content.ActivityNotFoundException) { context.startActivity(Intent(Settings.ACTION_SETTINGS)) }
        }
        fun copyCommand(context: Context, command: String) {
            context.getSystemService(ClipboardManager::class.java)
                .setPrimaryClip(ClipData.newPlainText("ADB 激活命令", command))
        }
        fun updateService(context: Context, requested: Boolean) {
            if (requested) {
                if (!ZorkAdbService.running) context.startForegroundService(Intent(context, ZorkAdbService::class.java))
            } else context.stopService(Intent(context, ZorkAdbService::class.java))
        }
    }
}

class ZorkAdbService : Service() {
    companion object { @Volatile internal var running = false }
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val instance = NativeBridge.newId()
    private var job: Job? = null
    private var platform: AdbPlatform? = null
    private fun notification(snapshot: JSONObject? = null): Notification {
        val manager = getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("zork-adb", "安卓调试连接", NotificationManager.IMPORTANCE_LOW).apply { setShowBadge(false) })
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, "zork-adb").setSmallIcon(R.drawable.ic_zork)
            .setContentTitle(snapshot?.text("notification_title") ?: "Zork Mesh 调试已开启")
            .setContentText(snapshot?.text("message") ?: "正在读取调试状态…")
            .setOngoing(true).setContentIntent(open).build()
    }
    override fun onCreate() {
        super.onCreate()
        val notification = notification()
        if (Build.VERSION.SDK_INT >= 34) startForeground(3721, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
        else startForeground(3721, notification)
        running = true
    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (job == null) job = scope.launch {
            val repo = ClientRepository(applicationContext)
            platform = AdbPlatform(applicationContext, repo, scope).also { it.start() }
            try {
                val snapshot = repo.command("adb_background_service", "running" to true, "instance" to instance)
                if (!snapshot.optBoolean("service_requested")) { stopSelf(); return@launch }
                repo.adbEvents().collect { frame ->
                    val data = frame.value.getJSONObject("snapshot")
                    if (!data.optBoolean("service_requested")) stopSelf()
                    else getSystemService(NotificationManager::class.java).notify(3721, notification(data))
                }
            } catch (e: CancellationException) { throw e }
            catch (_: Exception) { stopSelf() }
            finally {
                platform?.stop()
                withContext(NonCancellable) { runCatching { repo.command("adb_background_service", "running" to false, "instance" to instance) } }
            }
        }
        return START_STICKY
    }
    override fun onDestroy() { running = false; platform?.stop(); scope.cancel(); super.onDestroy() }
    override fun onBind(intent: Intent?): IBinder? = null
}
