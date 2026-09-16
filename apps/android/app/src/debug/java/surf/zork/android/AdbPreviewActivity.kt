package surf.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import org.json.JSONObject

/** The tests supply serialized Rust core snapshots, without a live ADB service. */
class AdbPreviewActivity : ComponentActivity() {
    internal var snapshot by mutableStateOf(JSONObject())
    val operations = mutableListOf<JSONObject>()
    var failure: String? = null
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        snapshot = JSONObject(intent.getStringExtra("snapshot")!!)
        val width = intent.getIntExtra("width", 0)
        setContent { ZorkTheme {
            Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing), contentAlignment = Alignment.TopCenter) {
                AdbSettings(SettingsActions(adb = snapshot, adbAction = { operation ->
                    operations.add(operation)
                    failure?.let { error(it) }
                }), if (width > 0) Modifier.width(width.dp).fillMaxHeight() else Modifier.fillMaxSize())
            }
        } }
    }
}
