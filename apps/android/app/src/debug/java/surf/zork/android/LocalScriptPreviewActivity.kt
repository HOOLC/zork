package surf.zork.android

import android.app.Application
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

internal object LocalScriptFixtureBridge { external fun seed(root: String, card: String): String }

internal class LocalScriptPreviewModel(app: Application) : AndroidViewModel(app) {
    val repo = ClientRepository(app)
    val platform = LocalScriptPlatform(app, repo, viewModelScope)
    var card by mutableStateOf<InteractionCardUi?>(null)
    var chat = ""
    var error by mutableStateOf("")
    fun load(source: String) { if (card != null || chat.isNotEmpty()) return; viewModelScope.launch {
        try {
            val input = JSONObject().put("kind", "local_script").put("version", 1).put("platform", "android")
                .put("title", "Android 本机脚本").put("description", "验证点击后执行及本机回调").put("source", source)
            val response = withContext(Dispatchers.IO) {
                JSONObject(LocalScriptFixtureBridge.seed(getApplication<Application>().noBackupFilesDir.resolve("client").absolutePath, input.toString()))
            }
            check(response.getBoolean("ok")) { response.text("error") }
            val data = response.getJSONObject("data"); chat = data.getString("chat")
            card = parseInteractionCard(data.getJSONObject("snapshot").getJSONObject("body").getJSONArray("items").getJSONObject(0).getJSONObject("interaction_card"))
        } catch (e: Exception) { error = e.message.orEmpty() }
    } }
    fun activate(action: String, values: Map<String, String>) { viewModelScope.launch {
        try { repo.command("respond_to_interaction", "peer" to "local-script-node", "session" to chat,
            "operation" to JSONObject().put("action", "activate").put("message_id", "local-script-card").put("choice", action).put("values", JSONObject(values))) }
        catch (e: Exception) { error = e.message.orEmpty() }
    } }
}

/** Real cache -> card -> activation -> QuickJS -> Android path, without a network fixture. */
class LocalScriptPreviewActivity : ComponentActivity() {
    internal val model: LocalScriptPreviewModel by viewModels()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        model.platform.attach(this)
        model.load(intent.getStringExtra("source")!!)
        setContent { ZorkTheme {
            Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).verticalScroll(rememberScrollState()).padding(16.dp)) {
                model.card?.let { InteractionCard(it, model::activate) }
                if (model.error.isNotEmpty()) Text(model.error)
            }
            LocalScriptPanel(model.platform)
        } }
    }
    override fun onDestroy() { model.platform.detach(this); super.onDestroy() }
}

class LocalScriptResultActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { ZorkTheme { Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).padding(24.dp)) {
            Text("原生 Activity 回调")
            Button(onClick = { setResult(RESULT_OK, Intent().putExtra("answer", 42)); finish() }) { Text("返回结果") }
        } } }
    }
}
