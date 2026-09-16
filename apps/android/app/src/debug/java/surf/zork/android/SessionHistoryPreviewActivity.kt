package surf.zork.android

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import org.json.JSONArray
import org.json.JSONObject

/** Fixed core-wire presentation fixture. No production identity, network or model. */
class SessionHistoryPreviewActivity : ComponentActivity() {
    internal var history by mutableStateOf<SessionHistoryState?>(null)
    val openedSessions = mutableListOf<String>()
    val frameTimes = mutableListOf<Double>()
    val destinations = mutableListOf<String>()
    private var offset = 99_900
    private var fixture: JSONObject? = null
    fun ready() {
        val current = history ?: return
        fixture?.let { source ->
            val value = JSONObject(source.getJSONObject("history").toString())
                .put("peer", current.peer).put("session", current.session)
                .put("clock_offset_ms", source.getLong("now_ms") - System.currentTimeMillis())
                .put("older", true).put("newer", offset < 99_900).put("window_start", offset)
            current.apply(HistoryFrame.decode(value))
            return
        }
        val entries = JSONArray((offset until offset + 100).map { i -> JSONObject()
            .put("id", "entry-$i").put("action", "shell.run").put("kind", "shell").put("state", "succeeded")
            .put("preview", "检查工作目录 $i · 中英文记录 with a bounded preview")
            .put("start", 1789200000000L + i * 1000).put("end", 1789200000200L + i * 1000) })
        current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session)
            .put("entries", entries).put("loading", false).put("loaded", true).put("older", true)
            .put("newer", offset < 99_900).put("total", 100_000).put("window_start", offset).put("error", JSONObject.NULL)
            .put("overview", JSONObject().put("model", "测试模型").put("context_tokens", 2048).put("context_limit", 32768)
                .put("input", 24000).put("output", 3500).put("total", 27500).put("cached", 12000).put("cache_hit_rate", .5))))
    }
    fun failed() { history?.let { it.status = it.status.copy(loading = false, error = "测试连接暂时中断") } }
    fun prepend() {
        val current = history ?: return
        val frame = fixture?.getJSONObject("prepended") ?: return
        current.apply(HistoryFrame.decode(JSONObject(frame.toString()).put("peer", current.peer).put("session", current.session)))
    }
    fun revoked() {
        history?.let { it.apply(HistoryFrame.decode(JSONObject().put("peer", it.peer).put("session", it.session)
            .put("entries", JSONArray()).put("loading", false).put("loaded", true).put("revoked", true)
            .put("detail", JSONObject.NULL).put("overview", JSONObject.NULL).put("error", "设备访问权限已撤销"))) }
    }
    private fun open(session: String, name: String) {
        openedSessions.add(session)
        offset = 99_900
        history = SessionHistoryState("fixture", session, name)
        ready()
    }
    private fun detail(id: String?) {
        val current = history ?: return
        current.selectedId = id
        current.detail = null
        if (id != null) current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session)
            .put("detail", JSONObject().put("id", id).put("action", "shell.run").put("summary", "pwd")
                .put("outcome", "/workspace/zork").put("raw_json", "{\n  \"tool\": \"shell.run\",\n  \"output\": \"/workspace/zork\"\n}"))))
        if (id != null && fixture != null) {
            val value = JSONObject(fixture!!.getJSONObject("details").getJSONObject(id).toString())
            if (intent.getBooleanExtra("long_detail", false)) value.put("outcome", ("长内容：跨块文本与 emoji 👋，保留每一个字符。\n").repeat(4000))
            current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session).put("detail", value)))
        }
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        fixture = intent.getStringExtra("fixture_file")?.let { JSONObject(java.io.File(it).readText()) }
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.addOnFrameMetricsAvailableListener({ _, frame, _ -> synchronized(frameTimes) {
            frameTimes.add((frame.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION) + frame.getMetric(FrameMetrics.DRAW_DURATION)) / 1_000_000.0)
        } }, Handler(Looper.getMainLooper()))
        val participants = listOf(
            JSONObject().put("id", "agent-a").put("session_id", "session-a").put("name", "阿狸").put("avatar", "fox"),
            JSONObject().put("id", "agent-b").put("session_id", "session-b").put("name", "小熊").put("avatar", "bear"),
        )
        val workbench = WorkbenchState(activePeer = Peer("fixture", "测试设备", ""),
            conversation = Conversation("chat", "测试对话"), participants = participants, connected = true)
        setContent { ZorkTheme {
            Box(Modifier.fillMaxSize().safeDrawingPadding().imePadding()) {
                val current = history
                if (current == null) Workbench(workbench, WorkbenchActions(history = ::open))
                else SessionHistoryPage(current, HistoryActions(back = { history = null },
                    older = { offset -= 100; ready() }, newer = { offset += 100; ready() },
                    latest = { offset = 99_900; ready() }, retry = ::ready, detail = ::detail,
                    navigate = { destinations.add(it.id) }))
            }
        } }
    }
}
