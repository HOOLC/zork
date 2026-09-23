package ing.zork.android

import androidx.compose.runtime.*
import org.json.JSONArray
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

internal data class HistoryDestination(val id: String, val title: String, val canSend: Boolean, val canStop: Boolean)
internal data class HistoryIdentity(val id: String, val name: String, val role: String,
    val model: String, val profile: String, val thinking: String)
internal data class HistorySubject(val label: String, val agent: HistoryIdentity? = null, val conversation: HistoryDestination? = null) {
    val actionable: Boolean get() = agent != null || conversation != null
}
internal data class HistoryRow(val id: String, val title: String, val preview: String, val kind: String,
    val lane: Int, val visible: Boolean, val start: Long?, val end: Long?, val state: String,
    val subject: HistorySubject?, val model: String, val requestedWait: Long?) {
    val failed get() = state == "failed" || state == "timed_out"
    val status get() = historyStateLabel(state)
    fun duration(now: Long): Long? = start?.let { first -> (end ?: now.takeIf { state == "running" })?.let { (it - first).coerceAtLeast(0) } }
}
internal data class HistoryEdit(val start: Int, val end: Int, val insert: List<HistoryRow>)
internal data class HistoryBlock(val id: String, val members: List<String>, val grouped: Boolean,
    val title: String, val summary: String, val start: Long?, val end: Long?)
internal data class HistoryListRow(val key: String, val entry: HistoryRow? = null, val group: HistoryBlock? = null, val child: Boolean = false)
internal data class HistorySection(val title: String, val text: String, val chunks: List<String>, val code: Boolean = false)
internal data class HistoryDetail(val id: String, val title: String, val sections: List<HistorySection>,
    val state: String, val start: Long?, val end: Long?, val model: String, val usage: String?)
internal data class HistoryQuota(val name: String, val provider: String, val lines: List<String>, val checked: String?)
internal data class HistoryOverview(val model: String, val context: String, val tokens: String, val cache: String,
    val profile: HistoryQuota? = null)
internal data class HistoryStatus(val loading: Boolean = true, val loaded: Boolean = false,
    val older: Boolean = false, val newer: Boolean = false, val total: Int = 0, val start: Int = 0,
    val error: String? = null, val revoked: Boolean = false, val clockOffset: Long = 0)

/** The list is only a presentation mirror of core splices. Group eligibility,
 * identity resolution, pagination and aggregate coverage all arrive from Rust. */
internal class SessionHistoryState(val peer: String, val session: String, val name: String) {
    private val rows = mutableStateListOf<HistoryRow>()
    val entries: List<HistoryRow> get() = rows
    var revision by mutableLongStateOf(0)
        private set
    var blocks by mutableStateOf<List<HistoryBlock>?>(null)
        private set
    val expanded = mutableStateMapOf<String, Boolean>()
    val timeline = HistoryTimelineState()
    var highlightedId by mutableStateOf<String?>(null)
    var status by mutableStateOf(HistoryStatus())
    var overview by mutableStateOf<HistoryOverview?>(null)
    var selectedId by mutableStateOf<String?>(null)
    var detail by mutableStateOf<HistoryDetail?>(null)

    fun apply(frame: HistoryFrame) {
        if (frame.peer != peer || frame.session != session) return
        frame.entries?.let { rows.clear(); rows.addAll(it) }
        frame.edits.forEach { edit ->
            check(edit.start >= 0 && edit.end >= edit.start && edit.end <= rows.size) { "执行记录增量超出已应用范围" }
            rows.subList(edit.start, edit.end).clear()
            rows.addAll(edit.start, edit.insert)
        }
        frame.blocks?.let { incoming ->
            val retained = incoming.filter { block -> block.members.any(expanded::containsKey) }.flatMap { it.members }
            expanded.clear(); retained.forEach { expanded[it] = true }
            blocks = incoming
        }
        if (frame.entries != null || frame.edits.isNotEmpty() || frame.blocks != null) {
            revision += 1
            if (rows.none { it.id == highlightedId }) highlightedId = null
        }
        frame.status?.let { status = it }
        if (frame.hasOverview) overview = frame.overview
        if (frame.hasDetail && (frame.detail == null || frame.detail.id == selectedId)) detail = frame.detail
        if (status.revoked) { selectedId = null; detail = null; expanded.clear(); highlightedId = null }
    }
    fun isExpanded(block: HistoryBlock) = block.members.any(expanded::containsKey)
    fun toggle(block: HistoryBlock) {
        if (isExpanded(block)) block.members.forEach(expanded::remove)
        else block.members.forEach { expanded[it] = true }
    }
    fun reveal(id: String) {
        highlightedId = id
        blocks?.firstOrNull { it.grouped && id in it.members }?.members?.forEach { expanded[it] = true }
    }
    fun visibleRows(): List<HistoryListRow> {
        val byId = rows.associateBy { it.id }
        return blocks?.flatMap { block ->
            if (block.grouped) buildList {
                add(HistoryListRow("group:${block.id}", group = block))
                if (isExpanded(block)) block.members.mapNotNullTo(this) { id -> byId[id]?.let { HistoryListRow("entry:$id", entry = it, child = true) } }
            } else block.members.mapNotNull { id -> byId[id]?.let { HistoryListRow("entry:$id", entry = it) } }
        } ?: rows.filter { it.visible }.map { HistoryListRow("entry:${it.id}", entry = it) }
    }
}

internal data class HistoryFrame(val peer: String, val session: String, val entries: List<HistoryRow>?,
    val edits: List<HistoryEdit>, val status: HistoryStatus?, val overview: HistoryOverview?,
    val hasOverview: Boolean, val detail: HistoryDetail?, val hasDetail: Boolean, val blocks: List<HistoryBlock>? = null) {
    companion object {
        // JSON, time formatting and text partitioning run on the observation IO lane.
        fun decode(value: JSONObject): HistoryFrame = HistoryFrame(
            value.text("peer"), value.text("session"),
            value.optJSONArray("entries")?.objects()?.map(::historyRow),
            value.optJSONArray("entry_edits").objects().map {
                HistoryEdit(it.getInt("start"), it.getInt("end"), it.optJSONArray("insert").objects().map(::historyRow))
            },
            if (value.has("loading")) HistoryStatus(value.optBoolean("loading"), value.optBoolean("loaded"),
                value.optBoolean("older"), value.optBoolean("newer"), value.optInt("total"), value.optInt("window_start"),
                value.text("error").ifBlank { null }, value.optBoolean("revoked"), value.optLong("clock_offset_ms")) else null,
            value.optJSONObject("overview")?.let(::historyOverview), value.has("overview"),
            value.optJSONObject("detail")?.let(::historyDetail), value.has("detail"),
            value.optJSONArray("blocks")?.objects()?.map { block ->
                val counts = listOf("read" to "读取", "written" to "写入", "edited" to "编辑", "shell" to "命令",
                    "queries" to "查询", "thinking" to "思考", "other" to "其他操作", "failed" to "失败")
                    .mapNotNull { (key, label) -> block.optInt(key).takeIf { it > 0 }?.let { "$label $it" } }
                val ids = block.optJSONArray("members") ?: JSONArray()
                HistoryBlock(block.text("id"), (0 until ids.length()).map(ids::getString), block.optBoolean("grouped"),
                    counts.joinToString(" · "), block.text("summary"), block.longOrNull("start"), block.longOrNull("end"))
            },
        )
    }
}

private fun JSONObject.longOrNull(key: String) = if (isNull(key)) null else optLong(key)
private fun historyRow(value: JSONObject): HistoryRow = HistoryRow(value.text("id"),
    if (value.optInt("lane") == 1 && value.text("kind").isBlank()) "模型请求" else historyTitle(value.text("action"), value.text("kind")),
    value.text("preview"), value.text("kind"), value.optInt("lane", 2), value.optBoolean("visible", true),
    value.longOrNull("start"), value.longOrNull("end"), value.text("state"), value.optJSONObject("subject")?.let { subject ->
        HistorySubject(subject.text("label"), subject.optJSONObject("agent")?.let { a ->
            HistoryIdentity(a.text("id"), a.text("name"), a.text("role"), a.text("model"), a.text("profile"), a.text("thinking"))
        }, subject.optJSONObject("conversation")?.let { c -> HistoryDestination(c.text("id"), c.text("title"), c.optBoolean("can_send"), c.optBoolean("can_stop")) })
    }, value.text("model"), value.longOrNull("requested_wait_ms"))

private fun historyOverview(value: JSONObject): HistoryOverview {
    fun count(key: String) = value.longOrNull(key)?.let(::compactTokens) ?: "—"
    val rate = if (value.isNull("cache_hit_rate")) "—" else "${String.format(Locale.ROOT, "%.0f", value.optDouble("cache_hit_rate") * 100)}%"
    val profile = value.optJSONObject("profile")?.let { p ->
        val quota = p.optJSONObject("quota")
        val lines = buildList {
            if (quota?.optBoolean("failed") == true) add("额度查询失败")
            quota?.optJSONArray("windows").objects().forEach { window ->
                val minutes = window.optLong("minutes")
                val span = when { minutes <= 0 -> "额度"; minutes % 1440 == 0L -> "${minutes / 1440}天"; minutes % 60 == 0L -> "${minutes / 60}小时"; else -> "${minutes}分钟" }
                val remaining = window.optDouble("remaining")
                add("${window.text("name")} $span · 剩余 ${if (remaining > 0 && remaining < 1) "<1" else remaining.toInt()}%".trim())
            }
            quota?.optJSONArray("balance")?.let { balance -> add("余额 ${String.format(Locale.ROOT, "%.2f", balance.optDouble(0))} ${balance.optString(1)}") }
        }
        HistoryQuota(p.text("name"), p.text("provider"), lines, p.text("checked_at").takeIf(String::isNotBlank))
    }
    return HistoryOverview(listOf(value.text("model"), value.text("thinking")).filter(String::isNotBlank).joinToString(" · ").ifBlank { "模型未知" },
        "上下文 ${count("context_tokens")} / ${count("context_limit")}",
        "总计 ${count("total")} · 输入 ${count("input")} · 输出 ${count("output")}",
        "缓存 ${count("cached")} · 命中 $rate${if (value.optBoolean("partial_cache")) "（部分报告）" else ""}", profile)
}

private fun historyDetail(value: JSONObject): HistoryDetail {
    fun section(title: String, text: String, code: Boolean = false) = HistorySection(title, text, historyTextChunks(text), code)
    val sections = buildList {
        value.text("summary").takeIf(String::isNotBlank)?.let { add(section("内容", it)) }
        value.text("outcome").takeIf { it.isNotBlank() && it != value.text("summary") }?.let { add(section("结果", it)) }
        val stages = value.optJSONArray("stages").objects()
        stages.forEachIndexed { index, stage ->
            val title = when (stage.text("kind")) { "request" -> "请求"; "result" -> "结果"; "start" -> "开始"; "end" -> "结束"; else -> "事件" }
            add(section("$title JSON · ${index + 1}", stage.text("json"), true))
        }
        if (stages.isEmpty()) value.text("raw_json").takeIf(String::isNotBlank)?.let { add(section("原始 JSON", it, true)) }
    }
    val usage = value.optJSONObject("usage")?.let { u ->
        fun count(key: String) = u.longOrNull(key)?.toString() ?: "—"
        "输入 ${count("input_tokens")} · 输出 ${count("output_tokens")} · 缓存 ${count("cached_input_tokens")}"
    }
    return HistoryDetail(value.text("id"), if (value.optInt("lane") == 1) "模型请求" else historyTitle(value.text("action"), ""), sections, value.text("state"),
        value.longOrNull("start"), value.longOrNull("end"), value.text("model"), usage)
}

/** Preserve every character, including surrogate pairs, while bounding Text layout. */
internal fun historyTextChunks(text: String): List<String> = buildList {
    var start = 0
    while (start < text.length) {
        var end = (start + 1024).coerceAtMost(text.length)
        if (end < text.length && Character.isHighSurrogate(text[end - 1])) end -= 1
        add(text.substring(start, end)); start = end
    }
}
internal fun historyStateLabel(state: String) = when (state) {
    "running" -> "进行中"; "failed" -> "失败"; "timed_out" -> "超时"; "succeeded", "completed" -> "完成"
    "received" -> "已接收"; "cancelled", "canceled", "interrupted" -> "已取消"; else -> state
}
internal fun historyTitle(action: String, kind: String): String = when (kind) {
    "input", "received" -> "收到输入"; "output" -> "模型回复"; "thinking" -> "思考中"
    "send_message" -> "发送消息"; "send_file" -> "发送文件"; "notify" -> "发送通知"
    "assign" -> "分配任务"; "rework" -> "要求返工"; "workers" -> "查询队员"; "tasks" -> "查询任务"
    "read" -> "读取文件"; "write" -> "写入文件"; "edit" -> "编辑文件"; "shell" -> "执行命令"
    "browser" -> "浏览器操作"; "wait" -> "等待"; "end" -> "结束执行"; "cancel" -> "取消任务"
    "help" -> "查询工具"; "history" -> "查询执行历史"; "chat_history" -> "查询聊天历史"; "job" -> "后台任务"
    "error" -> "执行错误"; "notice" -> "通知"
    else -> when (action) { "input" -> "收到输入"; "model", "model_call" -> "模型请求"; else -> action.ifBlank { "执行记录" } }
}
internal fun historyDuration(ms: Long): String = when {
    ms < 1000 -> "${ms}ms"
    ms < 60_000 -> "${String.format(Locale.ROOT, "%.1f", ms / 1000.0)}s"
    else -> "${ms / 60_000}m ${ms % 60_000 / 1000}s"
}
private val historyClockFormat = DateTimeFormatter.ofPattern("HH:mm:ss")
private val historyDateFormat = DateTimeFormatter.ofPattern("MM-dd HH:mm:ss")
internal fun historyClock(time: Long?, date: Boolean = false): String = time?.let {
    (if (date) historyDateFormat else historyClockFormat).format(Instant.ofEpochMilli(it).atZone(ZoneId.systemDefault()))
} ?: "—"
internal fun historyRelative(time: Long?, now: Long): String = time?.let {
    val elapsed = (now - it).coerceAtLeast(0)
    when { elapsed < 60_000 -> "刚刚"; elapsed < 3_600_000 -> "${elapsed / 60_000}分钟前"; elapsed < 86_400_000 -> "${elapsed / 3_600_000}小时前"
        elapsed < 2_592_000_000L -> "${elapsed / 86_400_000}天前"; elapsed < 31_536_000_000L -> "${elapsed / 2_592_000_000L}个月前"; else -> "${elapsed / 31_536_000_000L}年前" }
} ?: "时间未知"
