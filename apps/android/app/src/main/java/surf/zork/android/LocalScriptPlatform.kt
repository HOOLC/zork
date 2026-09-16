package surf.zork.android

import android.annotation.SuppressLint
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.media.AudioManager
import android.net.Uri
import android.os.Bundle
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import java.lang.ref.WeakReference
import kotlin.coroutines.resume

internal data class LocalScriptRun(val id: String, val title: String, val phase: String,
    val error: String?, val logs: List<String>, val canCancel: Boolean)

/** Applies capabilities claimed from core. It never reads message code or executes JavaScript. */
internal class LocalScriptPlatform(private val context: Context, private val repo: ClientRepository, private val scope: CoroutineScope) {
    var run by mutableStateOf<LocalScriptRun?>(null)
        private set
    private data class Binding(val activity: WeakReference<ComponentActivity>, val launch: ActivityResultLauncher<Intent>, val permissions: ActivityResultLauncher<Array<String>>)
    private var binding: Binding? = null
    // These are OS callback tickets, retained across Activity recreation. A
    // cancelled launch cannot donate its late result to a subsequent script.
    private var activityResult: CompletableDeferred<ActivityResult>? = null
    private var permissionResult: CompletableDeferred<Map<String, Boolean>>? = null
    private var observation: Job? = null
    private var applying: Job? = null
    private var applyingRun: String? = null

    fun attach(activity: ComponentActivity) {
        val launch = activity.activityResultRegistry.register("local-script-activity", activity, ActivityResultContracts.StartActivityForResult()) { result ->
            activityResult?.complete(result); activityResult = null
        }
        val permissions = activity.activityResultRegistry.register("local-script-permissions", activity, ActivityResultContracts.RequestMultiplePermissions()) { result ->
            permissionResult?.complete(result); permissionResult = null
        }
        binding = Binding(WeakReference(activity), launch, permissions)
        if (observation == null) observation = scope.launch {
            repo.localScriptEvents().collect { frame ->
                val data = frame.value.getJSONObject("snapshot")
                run = data.optJSONObject("run")?.let { value ->
                    val logs = value.getJSONArray("logs")
                    LocalScriptRun(value.getString("id"), value.getString("title"), value.getString("phase"),
                        value.text("error").takeIf(String::isNotBlank), List(logs.length()) { logs.getString(it) }, value.getBoolean("can_cancel"))
                }
                if (applyingRun != null && applyingRun != data.text("active_run")) applying?.cancel()
                data.optJSONObject("request")?.let { request ->
                    // Only core can claim a request; repeated observations and
                    // a recreated Activity cannot repeat its platform effect.
                    scope.launch { apply(request) }
                }
            }
        }
    }
    fun detach(activity: ComponentActivity) {
        if (binding?.activity?.get() === activity) binding = null
    }
    fun action(action: String, runId: String) { scope.launch {
        repo.command("local_script", "operation" to JSONObject().put("action", action).put("run_id", runId))
    } }
    private suspend fun apply(request: JSONObject) {
        val runId = request.getString("run_id"); val requestId = request.getString("id")
        val claim = try { repo.command("local_script", "operation" to JSONObject().put("action", "claim").put("run_id", runId).put("request_id", requestId)) }
            catch (e: Exception) { if (e is CancellationException) throw e; android.util.Log.w("ZorkLocalScript", "Cannot claim native operation", e); return }
        if (!claim.optBoolean("accepted")) return
        val current = kotlinx.coroutines.currentCoroutineContext()[Job]
        applying = current; applyingRun = runId
        val operation = claim.getJSONObject("operation")
        val reply = JSONObject().put("action", "complete").put("run_id", runId).put("request_id", requestId)
        try { reply.put("value", execute(operation) ?: JSONObject.NULL) }
        catch (e: Exception) { reply.put("error", if (e is CancellationException) "Android operation cancelled" else e.message ?: e.javaClass.simpleName) }
        finally {
            if (applying === current) { applying = null; applyingRun = null }
            withContext(NonCancellable + Dispatchers.IO) {
                if (reply.toString().toByteArray(Charsets.UTF_8).size > operation.getInt("max_reply_bytes")) {
                    reply.remove("value"); reply.put("error", "Android result too large")
                }
                runCatching { repo.command("local_script", "operation" to reply) }
            }
        }
    }
    private fun activeBinding(): Binding {
        val current = checkNotNull(binding) { "Return to Zork to perform this action" }
        check(current.activity.get()?.lifecycle?.currentState?.isAtLeast(Lifecycle.State.STARTED) == true) { "Return to Zork to perform this action" }
        return current
    }
    private fun buildIntent(value: JSONObject): Intent {
        val intent = Intent(value.getString("action"))
        val data = value.text("data").takeIf(String::isNotBlank)?.let(Uri::parse)
        val type = value.text("mimeType").takeIf(String::isNotBlank)
        intent.setDataAndType(data, type)
        value.text("package").takeIf(String::isNotBlank)?.let(intent::setPackage)
        value.optJSONArray("categories")?.let { list -> repeat(list.length()) { intent.addCategory(list.getString(it)) } }
        intent.addFlags(value.optInt("flags"))
        val extras = value.optJSONObject("extras") ?: JSONObject()
        for (key in extras.keys()) when (val extra = extras.get(key)) {
            is String -> intent.putExtra(key, extra)
            is Boolean -> intent.putExtra(key, extra)
            is Int -> intent.putExtra(key, extra)
            is Long -> if (extra in Int.MIN_VALUE..Int.MAX_VALUE) intent.putExtra(key, extra.toInt()) else intent.putExtra(key, extra)
            is Number -> intent.putExtra(key, extra.toDouble())
            is JSONArray -> intent.putExtra(key, Array(extra.length()) { extra.getString(it) })
            is JSONObject -> {
                val uri = Uri.parse(extra.getString("uri"))
                intent.putExtra(key, uri)
                // Grant-bearing URIs must also occur in ClipData for the OS
                // to propagate their grants through a chooser/target Activity.
                val clip = intent.clipData
                if (clip == null) intent.clipData = ClipData.newRawUri("", uri) else clip.addItem(ClipData.Item(uri))
            }
        }
        return value.text("chooserTitle").takeIf(String::isNotBlank)?.let { Intent.createChooser(intent, it) } ?: intent
    }
    private fun activityValue(result: ActivityResult): JSONObject {
        val intent = result.data
        val uris = JSONArray()
        intent?.clipData?.let { clip -> repeat(clip.itemCount.coerceAtMost(32)) { index -> clip.getItemAt(index).uri?.let { uris.put(it.toString()) } } }
        val extras = JSONObject()
        intent?.extras?.let { bundle -> for (key in bundle.keySet().take(32)) {
            @Suppress("DEPRECATION") val value = bundle.get(key)
            if (value is String || value is Boolean || value is Number) extras.put(key, value)
        } }
        return JSONObject().put("resultCode", result.resultCode).put("data", intent?.dataString ?: JSONObject.NULL).put("uris", uris).put("extras", extras)
    }
    @SuppressLint("MissingPermission")
    @Suppress("DEPRECATION")
    internal suspend fun execute(op: JSONObject): Any? = when (op.getString("method")) {
        "info" -> JSONObject().put("sdk", android.os.Build.VERSION.SDK_INT).put("packageName", context.packageName)
            .put("manufacturer", android.os.Build.MANUFACTURER).put("model", android.os.Build.MODEL)
        "start_activity" -> {
            val current = activeBinding(); val intent = buildIntent(op.getJSONObject("intent"))
            if (op.optBoolean("result")) {
                check(activityResult == null) { "A previous Android activity is still open" }
                val result = CompletableDeferred<ActivityResult>(); activityResult = result
                try { current.launch.launch(intent) } catch (e: Exception) { activityResult = null; throw e }
                activityValue(result.await())
            } else { checkNotNull(current.activity.get()).startActivity(intent); JSONObject().put("launched", true) }
        }
        "request_permissions" -> {
            val current = activeBinding(); val values = op.getJSONArray("permissions")
            check(permissionResult == null) { "A previous permission dialog is still open" }
            val result = CompletableDeferred<Map<String, Boolean>>(); permissionResult = result
            try { current.permissions.launch(Array(values.length()) { values.getString(it) }) } catch (e: Exception) { permissionResult = null; throw e }
            JSONObject(result.await())
        }
        "has_permission" -> context.checkSelfPermission(op.getString("permission")) == PackageManager.PERMISSION_GRANTED
        "clipboard_write" -> { context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("", op.getString("text"))); null }
        "clipboard_clear" -> { context.getSystemService(ClipboardManager::class.java).clearPrimaryClip(); null }
        "clipboard_read" -> context.getSystemService(ClipboardManager::class.java).primaryClip?.let { clip ->
            if (clip.itemCount == 0) null else clip.getItemAt(0).text?.toString()
        }
        "torch" -> {
            val camera = context.getSystemService(CameraManager::class.java)
            val id = op.text("camera_id").takeIf(String::isNotBlank) ?: camera.cameraIdList.firstOrNull { camera.getCameraCharacteristics(it).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true }
            camera.setTorchMode(checkNotNull(id) { "No flashlight available" }, op.getBoolean("enabled")); null
        }
        "volume_get", "volume_set" -> {
            val audio = context.getSystemService(AudioManager::class.java)
            val stream = when (op.getString("stream")) { "alarm" -> AudioManager.STREAM_ALARM; "ring" -> AudioManager.STREAM_RING; "notification" -> AudioManager.STREAM_NOTIFICATION; else -> AudioManager.STREAM_MUSIC }
            if (op.getString("method") == "volume_set") audio.setStreamVolume(stream, op.getInt("index"), if (op.getBoolean("show_ui")) AudioManager.FLAG_SHOW_UI else 0)
            JSONObject().put("index", audio.getStreamVolume(stream)).put("min", audio.getStreamMinVolume(stream)).put("max", audio.getStreamMaxVolume(stream))
        }
        "vibrate" -> { context.getSystemService(Vibrator::class.java).vibrate(VibrationEffect.createOneShot(op.getLong("milliseconds"), VibrationEffect.DEFAULT_AMPLITUDE)); null }
        "settings_get" -> when (op.getString("namespace")) {
            "global" -> Settings.Global.getString(context.contentResolver, op.getString("key"))
            "secure" -> Settings.Secure.getString(context.contentResolver, op.getString("key"))
            else -> Settings.System.getString(context.contentResolver, op.getString("key"))
        }
        "settings_can_write" -> Settings.System.canWrite(context)
        "settings_put" -> { check(Settings.System.putInt(context.contentResolver, op.getString("key"), op.getInt("value"))) { "System setting was not saved" }; null }
        "content_read_text" -> withContext(Dispatchers.IO) {
            val buffer = repo.localDocumentBytes(Uri.parse(op.getString("uri")), op.getInt("max_bytes"))
            val used = buffer.size
            val digits = "0123456789abcdef"
            JSONObject().put("hex", buildString(used * 2) { repeat(used) { val b = buffer[it].toInt() and 255; append(digits[b ushr 4]); append(digits[b and 15]) } })
        }
        "content_write_text" -> withContext(Dispatchers.IO) {
            val bytes = op.getString("text").toByteArray(Charsets.UTF_8)
            repo.writeLocalDocument(Uri.parse(op.getString("uri")), bytes)
        }
        "location" -> location(op.getLong("timeout_ms"))
        else -> error("Unsupported Android capability")
    }
    @SuppressLint("MissingPermission")
    @Suppress("DEPRECATION")
    private suspend fun location(timeoutMs: Long): JSONObject = withTimeout(timeoutMs) {
        val manager = context.getSystemService(LocationManager::class.java)
        val provider = manager.getProviders(true).let { providers ->
            if (LocationManager.NETWORK_PROVIDER in providers) LocationManager.NETWORK_PROVIDER else LocationManager.GPS_PROVIDER
        }
        val location = suspendCancellableCoroutine<Location> { continuation ->
            val listener = object : LocationListener {
                override fun onLocationChanged(location: Location) { manager.removeUpdates(this); if (continuation.isActive) continuation.resume(location) }
                override fun onProviderEnabled(provider: String) {}
                override fun onProviderDisabled(provider: String) {}
                override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {}
            }
            continuation.invokeOnCancellation { manager.removeUpdates(listener) }
            manager.requestSingleUpdate(provider, listener, Looper.getMainLooper())
        }
        JSONObject().put("latitude", location.latitude).put("longitude", location.longitude).put("accuracy", location.accuracy).put("time", location.time)
    }
}
