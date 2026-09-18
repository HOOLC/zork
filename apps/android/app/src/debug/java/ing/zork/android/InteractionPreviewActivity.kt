package ing.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import org.json.JSONObject

/** Presentation fixture; the test supplies serialized read-only core snapshots. */
class InteractionPreviewActivity : ComponentActivity() {
    internal var card by mutableStateOf<InteractionCardUi?>(null)
    val activations = mutableListOf<Pair<String, Map<String, String>>>()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        card = parseInteractionCard(JSONObject(intent.getStringExtra("card")!!))
        setContent { ZorkTheme {
            Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing).verticalScroll(rememberScrollState()).padding(16.dp)) {
                card?.let { snapshot -> InteractionCard(snapshot) { action, values -> activations.add(action to values) } }
            }
        } }
    }
}
