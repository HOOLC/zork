package surf.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp

/** Fixed-input production conversation fixture; never opens the client's database. */
class ConversationScrollActivity : ComponentActivity() {
    data class DrawnPosition(val messages: Int, val first: Int, val canScrollForward: Boolean)
    val drawnPositions = mutableListOf<DrawnPosition>()
    val messageBounds = android.graphics.Rect()
    var loading by mutableStateOf(false)
    private var older by mutableStateOf(false)
    var olderLoads = 0
        private set
    private var members by mutableStateOf(emptyList<org.json.JSONObject>())
    fun completeMembers() { members = listOf(org.json.JSONObject().put("id","leader").put("name","滚动验证").put("avatar","cat")) }
    fun completeMetadata() { rows = rows.map { it.copy(author="同一个小伙伴",createdAt="2026-09-07T18:00:00+08:00") } }
    val initialFrames = mutableListOf<Double>()
    lateinit var scroll: LazyListState
    private var rows by mutableStateOf(emptyList<ChatMessage>())
    fun load(count: Int) { rows = (0 until count).map(::message); loading = false }
    fun append() { rows = rows + message(rows.size) }
    fun growTail() { rows = rows.dropLast(1) + rows.last().copy(content = rows.last().content + "\n\n" + "新增内容\n\n".repeat(45)) }
    private fun message(index: Int) = ChatMessage("row-$index", "小伙伴", "消息 $index\n\n这是一段用于验证滚动的内容。\n\n```kotlin\nval item$index = $index\nprintln(item$index)\n```\n\n消息 $index 结束。", false)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); enableEdgeToEdge()
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.addOnFrameMetricsAvailableListener({_,m,_-> synchronized(initialFrames) { initialFrames.add((m.getMetric(android.view.FrameMetrics.LAYOUT_MEASURE_DURATION)+m.getMetric(android.view.FrameMetrics.DRAW_DURATION))/1_000_000.0) } },android.os.Handler(android.os.Looper.getMainLooper()))
        loading = intent.getBooleanExtra("loading",false)
        older = intent.getBooleanExtra("older",false)
        if (!intent.getBooleanExtra("empty",false)) load(80)
        setContent {
            CompositionLocalProvider(LocalDensity provides Density(1f,1f)) {
                ZorkTheme {
                    scroll = rememberLazyListState()
                    val state = WorkbenchState(conversation=Conversation("scroll","滚动验证",avatar="cat"),messages=rows,participants=members,historyLoading=loading,older=older)
                    val actions = WorkbenchActions(older = { olderLoads++; older = false; load(1) })
                    Column(Modifier.requiredSize(390.dp,844.dp)) {
                        if (intent.getBooleanExtra("header",false)) ConversationHeader(state,WorkbenchActions(),true)
                        Box(Modifier.weight(1f).onGloballyPositioned {
                            val bounds = it.boundsInWindow()
                            messageBounds.set(bounds.left.toInt(), bounds.top.toInt(), bounds.right.toInt(), bounds.bottom.toInt())
                        }.drawWithContent {
                            drawContent()
                            if (rows.isNotEmpty()) drawnPositions.add(DrawnPosition(rows.size,scroll.firstVisibleItemIndex,scroll.canScrollForward))
                        }) { ConversationBody(state,actions,scroll) }
                    }
                }
            }
        }
    }
}
