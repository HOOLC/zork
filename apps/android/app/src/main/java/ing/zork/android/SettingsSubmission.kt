package ing.zork.android

import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject

internal class SettingsSubmission(val busy: Boolean, val error: String?, val perform: (String, JSONObject) -> Unit)

/** Retains only the ID of a core-owned operation across Activity recreation.
 * The request body (which may contain credentials) is never saveable state. */
@Composable
internal fun rememberSettingsSubmission(state: MobileSettingsState, actions: SettingsActions,
    success: (String, JSONObject) -> Unit): SettingsSubmission {
    var requestId by rememberSaveable { mutableStateOf<String?>(null) }
    var operation by rememberSaveable { mutableStateOf("") }
    var error by rememberSaveable { mutableStateOf<String?>(null) }
    val onSuccess by rememberUpdatedState(success)
    val scope = rememberCoroutineScope()
    fun complete(id: String, result: JSONObject?, failure: String?) {
        if (requestId != id) return
        requestId = null
        error = failure
        if (failure == null) onSuccess(operation, result ?: JSONObject())
    }
    LaunchedEffect(state.command, requestId, state.connectionState) {
        val command = state.command
        val id = requestId
        if (id != null && state.connectionState == "revoked") {
            complete(id, null, "设备访问权限已撤销"); return@LaunchedEffect
        }
        if (id != null && command?.text("id") == id && !command.optBoolean("running")) {
            complete(id, command.optJSONObject("result"), command.text("error").takeIf { it.isNotBlank() })
        }
    }
    return SettingsSubmission(requestId != null, error) { action, fields ->
        if (requestId == null) scope.launch {
            val id = NativeBridge.newId()
            requestId = id; operation = action; error = null
            try { complete(id, actions.perform(action, JSONObject(fields.toString()).put("_request_id", id)), null) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { complete(id, null, e.message ?: "操作未完成，请重试") }
        }
    }
}
