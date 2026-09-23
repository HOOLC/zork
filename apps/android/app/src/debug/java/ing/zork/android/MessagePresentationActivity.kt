package ing.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.unit.dp

/** Disconnected production-component fixture for the message presentation contract. */
class MessagePresentationActivity : ComponentActivity() {
    data class Frame(val count: Int, val first: Int, val offset: Int, val atTail: Boolean)
    val frames = mutableListOf<Frame>()
    lateinit var scroll: LazyListState
    var keyboardInset by mutableStateOf(0)
    var previewHeight by mutableIntStateOf(0)
    private var rows by mutableStateOf((0..12).map { ChatMessage("row-$it", "产品 Leader", "消息 $it：保留现有风格和阅读位置。", false, device = "studio-dev", model = "gpt-6") })
    private var messageActivity by mutableStateOf(MessageActivity())
    private fun deliver(row: ChatMessage) {
        rows = rows + row
        val sequence = messageActivity.sequence + 1
        messageActivity = MessageActivity(sequence, listOf(MessageArrival(row.id, sequence, android.os.SystemClock.uptimeMillis())))
    }
    val fullSource = "# 完整方案\n\n" + "长消息只显示预览，全文在独立页面阅读。\n\n```kotlin\nval message = \"中文🐈\"\n```\n\n".repeat(80) + "FULL-MESSAGE-END"
    fun append() { deliver(ChatMessage("row-${rows.size}", "产品 Leader", "这是一条新到达的消息。", false, device = "studio-dev", model = "gpt-6")) }
    fun appendLong() { deliver(ChatMessage("long", "产品 Leader", fullSource, false, device = "studio-dev", model = "gpt-6")) }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); enableEdgeToEdge()
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        setContent { ZorkTheme {
            var full by remember { mutableStateOf<ChatMessage?>(null) }
            scroll = rememberLazyListState()
            val state = WorkbenchState(conversation = Conversation("presentation", "产品 Leader"), messages = rows, connected = true, messageActivity = messageActivity)
            CompositionLocalProvider(LocalMessagePreviewHeight provides previewHeight) {
            Box(Modifier.fillMaxSize().safeDrawingPadding().imePadding().padding(bottom = keyboardInset.dp)) {
                PageSlide(full, full?.id ?: "chat", if (full == null) 0 else 1, ZorkColors.Canvas) { shown, active ->
                    if (shown != null) FullMessagePage(shown, { full = null }, {})
                    else Column(Modifier.fillMaxSize()) {
                        ConversationHeader(state, WorkbenchActions(), true)
                        Box(Modifier.weight(1f).drawWithContent {
                            drawContent(); frames.add(Frame(rows.size, scroll.firstVisibleItemIndex, scroll.firstVisibleItemScrollOffset, !scroll.canScrollForward))
                        }) { ConversationBody(state, WorkbenchActions(message = { if (active) full = it }), scroll) }
                    }
                }
            }
            }
        } }
    }
}
