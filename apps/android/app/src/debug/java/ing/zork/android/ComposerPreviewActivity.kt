package ing.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import org.json.JSONObject

/** Production composer fixture, isolated from all device identities and data. */
class ComposerPreviewActivity : ComponentActivity() {
    var draft by mutableStateOf("")
    var active by mutableStateOf(false)
    var viewportWidth by mutableStateOf(375)
    var fontScale by mutableStateOf(1f)
    lateinit var scroll: LazyListState
    val frames = mutableListOf<Double>()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN)
        window.addOnFrameMetricsAvailableListener({ _, metrics, _ -> synchronized(frames) {
            frames.add((metrics.getMetric(android.view.FrameMetrics.LAYOUT_MEASURE_DURATION) + metrics.getMetric(android.view.FrameMetrics.DRAW_DURATION)) / 1_000_000.0)
        } }, android.os.Handler(mainLooper))
        setContent {
            CompositionLocalProvider(LocalDensity provides Density(1f, fontScale)) { ZorkTheme {
                scroll = rememberLazyListState()
                val members = listOf("fox", "cat", "panda").mapIndexed { i, avatar ->
                    JSONObject().put("id", "member-$i").put("name", listOf("产品伙伴", "工程伙伴", "设计伙伴")[i]).put("avatar", avatar)
                        .put("activity", if (active) JSONObject().put("state", if (i == 2) "failed" else "thinking").put("reason", "等待重试") else JSONObject.NULL)
                }
                val rows = remember { (0 until 20).map { ChatMessage("composer-$it", "伙伴", "消息 $it · 输入框和状态应始终为消息留出空间。", false) } }
                val state = WorkbenchState(conversation = Conversation("composer-preview", "伙伴"), participants = members, messages = rows, connected = true, draft = draft)
                Box(Modifier.requiredSize(viewportWidth.dp, 720.dp)) { ConversationBody(state, WorkbenchActions(draft = { draft = it }), scroll) }
            } }
        }
    }
}
