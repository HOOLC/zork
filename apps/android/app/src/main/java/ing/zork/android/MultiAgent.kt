// Multi-agent message presentation (docs/design/interface.md#multi-agent-messages).
// Rules come from core `message_presentation`; this file only draws them.
package ing.zork.android

import android.animation.ValueAnimator
import android.graphics.Canvas
import android.graphics.Paint
import android.text.Spannable
import android.text.Spanned
import android.text.TextPaint
import android.text.style.CharacterStyle
import android.text.style.LineBackgroundSpan
import android.text.style.UpdateAppearance
import android.widget.TextView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicLong

/** Agent identity tints and the jump wash, the same values as
 * `zork_ui::design::AGENT_TINTS` / `JUMP_WASH`. Slots come from core
 * (`agent_tint::slot`, per agent id); never assign them here. */
internal object AgentTints {
    private val light = listOf(0xFFDFE3FA to 0xFF33429A, 0xFFEDE6C4 to 0xFF5A4E0B, 0xFFECE2F8 to 0xFF603B91,
        0xFFDCEBD5 to 0xFF35602A, 0xFFE6E3DD to 0xFF3A3D42)
    private val dark = listOf(0xFF363D68 to 0xFFCBD2FA, 0xFF4A4526 to 0xFFE6DDA6, 0xFF4A3960 to 0xFFDECBF6,
        0xFF34472F to 0xFFC0DDB0, 0xFF3D4044 to 0xFFDDD9D2)
    fun fill(slot: Int?): Color = Color((if (ZorkColors.dark) dark else light)[Math.floorMod(slot ?: 4, 5)].first)
    fun ink(slot: Int?): Color = Color((if (ZorkColors.dark) dark else light)[Math.floorMod(slot ?: 4, 5)].second)
    /** Locates a jumped-to passage; neither persimmon nor a status hue. */
    val Wash get() = if (ZorkColors.dark) Color(0xFF34322D) else Color(0xFFF5EFE3)
}

// ---------------------------------------------------------------- core data

internal data class AuthorUi(val name: String, val user: Boolean, val agentId: String?, val tint: Int?,
    val initial: String?, val maker: String?)
internal data class QuoteUi(val source: String, val text: String, val tooltip: String?, val mark: String?)
internal data class ReplyUi(val targetId: String, val state: String, val targetIndex: Int?, val target: AuthorUi?,
    val content: QuoteUi?, val inHead: Boolean, val ownRun: Boolean)
internal data class CommentPairUi(val quote: String, val reply: String, val source: AuthorUi, val state: String,
    val sourceIndex: Int?, val messageId: String?)
internal data class IdentityUi(val author: AuthorUi, val deviceName: String?, val detail: String)
internal data class TimeUi(val label: String, val full: String, val placement: String)
internal data class RowUi(val index: Int, val groupHead: Boolean, val groupTail: Boolean, val time: TimeUi?,
    val identity: IdentityUi?, val reply: ReplyUi?, val pairs: List<CommentPairUi>?, val extraText: String,
    /** The row's author with disc data (from its group head), for draft quote lines. */
    val author: AuthorUi?)

/** Core presentation of the shown rows, keyed by `ChatMessage.id`. */
internal class TranscriptUi(val rows: List<ChatMessage>, val ids: List<String>, val byId: Map<String, RowUi>,
    val multiDevice: Boolean, val nextChangeMs: Long?, val stamp: Long) {
    operator fun get(id: String) = byId[id]
    fun idAt(index: Int?) = index?.let { rows.getOrNull(it)?.id }
}

private fun JSONObject.author() = AuthorUi(text("name"), optBoolean("user"), text("agent_id").ifBlank { null },
    if (!has("tint") || isNull("tint")) null else optInt("tint"), text("initial").ifBlank { null }, text("maker").ifBlank { null })
private fun JSONObject.quote() = QuoteUi(text("source"), text("text"), text("tooltip").ifBlank { null }, text("mark").ifBlank { null })
private fun JSONObject.index(key: String) = if (!has(key) || isNull(key)) null else optInt(key)

private val presentationStamp = AtomicLong()

/** Request row: the observed JSON; fixtures without one describe themselves. */
private fun ChatMessage.presentationRow(): JSONObject = source?.json ?: JSONObject().put("type", "message")
    .put("role", if (user) "user" else "assistant").put("content", content).apply {
        if (id.isNotBlank()) put("id", id)
        if (createdAt.isNotBlank()) put("created_at", createdAt)
        if (!user) put("author_name", author)
        if (authorAgentId.isNotBlank()) put("author_agent_id", authorAgentId)
        if (device.isNotBlank()) put("device", device)
        if (model.isNotBlank()) put("model", model)
        if (replyTo.isNotBlank()) put("reply_to", replyTo)
    }

/** Calls core with the rows as observed. `null` if core rejects the request. */
internal fun presentTranscript(rows: List<ChatMessage>, hasOlder: Boolean, devices: Map<String, Peer>,
    nowMs: Long = System.currentTimeMillis()): TranscriptUi? {
    val stamp = presentationStamp.incrementAndGet()
    val request = JSONObject().put("rows", JSONArray().apply { rows.forEach { put(it.presentationRow()) } })
        .put("now_ms", nowMs).put("utc_offset_minutes", java.util.TimeZone.getDefault().getOffset(nowMs) / 60000)
        // The interface is Chinese; its time words follow it, not the system locale.
        .put("locale", "zh-CN").put("has_older", hasOlder)
        .put("devices", JSONObject().apply { devices.forEach { (id, peer) ->
            put(id, JSONObject().put("display", peer.name).put("machine", peer.machine ?: JSONObject.NULL)) } })
    val result = runCatching { JSONObject(NativeBridge.messagePresentation(request.toString())) }.getOrNull() ?: return null
    val data = result.optJSONObject("data")?.takeIf { result.optBoolean("ok") } ?: return null
    val out = data.optJSONArray("rows").objects()
    if (out.size != rows.size) return null
    val byId = HashMap<String, RowUi>(rows.size * 2)
    var groupAuthor: AuthorUi? = null
    out.forEachIndexed { at, row ->
        val identity = row.optJSONObject("identity")?.let { IdentityUi(it.author(), it.text("device_name").ifBlank { null }, it.text("detail")) }
        if (row.optBoolean("group_head")) groupAuthor = identity?.author
            ?: if (row.optBoolean("user")) AuthorUi("你", true, null, null, null, null) else null
        val reply = row.optJSONObject("reply")?.let {
            ReplyUi(it.text("target_id"), it.text("state"), it.index("target_index"), it.optJSONObject("target")?.author(),
                it.optJSONObject("content")?.quote(), it.text("placement") == "in_head", it.optBoolean("own_run"))
        }
        val comments = row.optJSONObject("comments")
        val pairs = comments?.optJSONArray("pairs").objects().takeIf { comments != null }?.map {
            CommentPairUi(it.text("quote"), it.text("reply"), it.optJSONObject("source")?.author()
                ?: AuthorUi("消息", false, null, null, null, null), it.text("state"), it.index("source_index"), it.text("message_id").ifBlank { null })
        }
        val time = row.optJSONObject("time")?.let { TimeUi(it.text("label"), it.text("full"), it.text("placement")) }
        byId[rows[at].id] = RowUi(at, row.optBoolean("group_head"), row.optBoolean("group_tail"), time, identity, reply,
            pairs, comments?.text("extra_text").orEmpty(), groupAuthor)
    }
    return TranscriptUi(rows, rows.map { it.id }, byId, data.optBoolean("multi_device"),
        if (data.isNull("next_change_ms")) null else data.optLong("next_change_ms"), stamp)
}

private class RowIds(val rows: List<ChatMessage>) {
    val ids = rows.map { it.id }
    override fun equals(other: Any?) = other is RowIds && other.ids == ids
    override fun hashCode() = ids.hashCode()
}

/**
 * Presentation of the shown rows. A new or removed row is presented in the same
 * frame (so grouping never flickers); text streaming into existing rows is
 * re-presented off the main thread. Labels refresh at core's `next_change_ms`.
 */
@Composable
internal fun rememberTranscript(rows: List<ChatMessage>, hasOlder: Boolean, devices: Map<String, Peer>): TranscriptUi? {
    var tick by remember { mutableIntStateOf(0) }
    val ids = remember(rows) { RowIds(rows) }
    val structural = remember(ids, hasOlder, devices, tick) { presentTranscript(ids.rows, hasOlder, devices) }
    var refreshed by remember { mutableStateOf<TranscriptUi?>(null) }
    LaunchedEffect(rows, hasOlder, devices, tick) {
        if (structural == null || structural.rows === rows) return@LaunchedEffect
        delay(120)
        refreshed = withContext(Dispatchers.Default) { presentTranscript(rows, hasOlder, devices) }
    }
    val shown = refreshed?.takeIf { structural != null && it.ids == structural.ids && it.stamp > structural.stamp } ?: structural
    LaunchedEffect(shown?.stamp, shown?.nextChangeMs) {
        val wait = shown?.nextChangeMs ?: return@LaunchedEffect
        delay(wait + 50)
        tick++
    }
    return shown
}

/** Chat-list time labels from core, refreshed when one changes. */
@Composable
internal fun rememberChatTimes(times: List<Long?>): List<String?> {
    var tick by remember { mutableIntStateOf(0) }
    val result = remember(times, tick) {
        val now = System.currentTimeMillis()
        val request = JSONObject().put("times_ms", JSONArray().apply { times.forEach { put(it ?: JSONObject.NULL) } })
            .put("now_ms", now).put("utc_offset_minutes", java.util.TimeZone.getDefault().getOffset(now) / 60000).put("locale", "zh-CN")
        runCatching { JSONObject(NativeBridge.messageTimes(request.toString())).optJSONObject("data") }.getOrNull()
    }
    LaunchedEffect(result) {
        val data = result ?: return@LaunchedEffect
        if (data.isNull("next_change_ms") || !data.has("next_change_ms")) return@LaunchedEffect
        delay(data.optLong("next_change_ms") + 50)
        tick++
    }
    val labels = result?.optJSONArray("labels")
    return times.indices.map { at -> labels?.optJSONObject(at)?.text("label")?.ifBlank { null } }
}

// ---------------------------------------------------------------- avatars

internal data class AgentAvatarUi(val agentId: String, val maker: String?, val tint: Int, val initial: String)
internal data class ChatAvatarUi(val agents: List<AgentAvatarUi> = emptyList(), val more: Long = 0)

internal fun parseChatAvatar(value: JSONObject?): ChatAvatarUi = if (value == null) ChatAvatarUi() else ChatAvatarUi(
    value.optJSONArray("agents").objects().map {
        AgentAvatarUi(it.text("agent_id"), it.text("maker").ifBlank { null }, it.optInt("tint"), it.text("initial"))
    }, value.optLong("more"))

/** An agent's round tint disc with its model maker's mark, or the initial. */
@Composable
internal fun AgentDisc(tint: Int?, maker: String?, initial: String?, size: Dp, modifier: Modifier = Modifier, markScale: Float = .68f) {
    Box(modifier.size(size).background(AgentTints.fill(tint), CircleShape), contentAlignment = Alignment.Center) {
        if (maker != null) Icon(painterResource(makerDrawable(maker)), null, Modifier.size(size * markScale), tint = AgentTints.ink(tint))
        else Text(initial.orEmpty().ifBlank { "?" }, color = AgentTints.ink(tint), fontSize = (size.value * .5f).sp,
            fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

@Composable
internal fun AgentDisc(author: AuthorUi, size: Dp, modifier: Modifier = Modifier) =
    AgentDisc(author.tint, author.maker, author.initial, size, modifier)

/** A Chat's avatar: up to three overlapping agent discs, then "+N"; the plain
 * Chat mark when the Chat has no agent authors. Each disc has a [ring] of the
 * surface colour so overlapping discs stay apart. */
@Composable
internal fun AvatarStack(avatar: ChatAvatarUi, size: Dp, overlap: Dp, ring: Color, modifier: Modifier = Modifier) {
    if (avatar.agents.isEmpty()) {
        Box(modifier.size(size), contentAlignment = Alignment.Center) { Glyph(R.drawable.ic_message_square, size * .9f, ZorkColors.Muted) }
        return
    }
    val ringed = Modifier.drawBehind { drawCircle(ring, radius = this.size.minDimension / 2 + 1.5.dp.toPx()) }
    androidx.compose.ui.layout.Layout(content = {
        avatar.agents.forEach { agent -> AgentDisc(agent.tint, agent.maker, agent.initial, size, ringed, markScale = .62f) }
        if (avatar.more > 0) Box(ringed.size(size).background(ZorkColors.Pressed, CircleShape), contentAlignment = Alignment.Center) {
            Text("+${avatar.more}", fontSize = (size.value * .48f).sp, color = ZorkColors.Muted, maxLines = 1)
        }
    }, modifier = modifier.semantics { contentDescription = "${avatar.agents.size + avatar.more} 个 Agent" }) { measurables, _ ->
        val placeables = measurables.map { it.measure(androidx.compose.ui.unit.Constraints()) }
        val step = (size - overlap).roundToPx()
        val width = (placeables.firstOrNull()?.width ?: 0) + step * (placeables.size - 1).coerceAtLeast(0)
        layout(width, placeables.maxOfOrNull { it.height } ?: 0) { placeables.forEachIndexed { at, p -> p.place(at * step, 0) } }
    }
}

// ---------------------------------------------------------------- tips

/** Long press shows [tip] above the content (the phone's hover detail). */
@OptIn(ExperimentalMaterial3Api::class, androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
internal fun LongPressTip(tip: String?, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null,
    content: @Composable () -> Unit) {
    if (tip.isNullOrBlank() && onClick == null) { Box(modifier) { content() }; return }
    val state = rememberTooltipState()
    val scope = rememberCoroutineScope()
    // TooltipBox keeps its modifier off its outer node; this Box carries row weights.
    Box(modifier) {
        TooltipBox(TooltipDefaults.rememberPlainTooltipPositionProvider(), tooltip = {
            PlainTooltip(containerColor = ZorkColors.Ink, contentColor = ZorkColors.Paper) { Text(tip.orEmpty(), fontSize = 12.sp) }
        }, state = state, enableUserInput = false) {
            Box(Modifier.combinedClickable(interactionSource = remember { MutableInteractionSource() }, indication = null,
                enabled = true, onLongClick = tip?.takeIf { it.isNotBlank() }?.let { { scope.launch { state.show() } } },
                onClick = onClick ?: {})) { content() }
        }
    }
}

// ---------------------------------------------------------------- quote lines

private val QuoteSize = 12.5.sp

@Composable
private fun QuoteWho(author: AuthorUi?) {
    if (author == null) return
    if (!author.user) AgentDisc(author, 16.dp, Modifier.padding(start = 2.dp))
    Text(author.name, fontSize = QuoteSize, fontWeight = FontWeight.Medium, color = ZorkColors.Ink, maxLines = 1,
        overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 120.dp))
}

@Composable
private fun SummaryPill() {
    Text("大意", fontSize = 10.5.sp, lineHeight = 16.sp, color = ZorkColors.Muted, maxLines = 1,
        modifier = Modifier.background(ZorkColors.Pressed, ZorkShapes.Control).padding(horizontal = 6.dp))
}

/** The single muted reply line: ↩ 回复, target disc + name, then the quote. */
@Composable
internal fun ReplyLine(reply: ReplyUi, modifier: Modifier = Modifier, open: () -> Unit, load: () -> Unit) {
    val content = reply.content
    val tip = when {
        reply.state != "linked" -> null
        content == null || content.tooltip == null -> null
        content.source == "summary" -> "大意：${content.tooltip}"
        else -> content.tooltip
    }
    val click: (() -> Unit)? = when (reply.state) { "linked" -> open; "not_loaded" -> load; else -> null }
    LongPressTip(tip, modifier, onClick = click) {
        Row(Modifier.heightIn(min = 22.dp).semantics(mergeDescendants = true) {}, verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            Glyph(R.drawable.ic_reply, 12.dp, ZorkColors.Muted)
            Text("回复", fontSize = QuoteSize, color = ZorkColors.Muted, maxLines = 1)
            when (reply.state) {
                "not_loaded" -> Text("更早的消息 · 尚未加载 · 点击加载", fontSize = QuoteSize, color = ZorkColors.Muted, maxLines = 1,
                    overflow = TextOverflow.Ellipsis)
                "deleted" -> {
                    QuoteWho(reply.target)
                    Text("· 原消息已删除", fontSize = QuoteSize, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                else -> {
                    QuoteWho(reply.target)
                    if (content != null) QuoteText(content.source, content.text)
                }
            }
        }
    }
}

@Composable
private fun RowScope.QuoteText(source: String, text: String) {
    if (source == "summary") SummaryPill()
    val shown = if (source == "original" || source == "excerpt") "「$text」" else text
    Text(shown, fontSize = QuoteSize, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis,
        modifier = Modifier.weight(1f, fill = false).padding(start = 2.dp))
}

/** A user reply's quote line (sent pairs and composer drafts): ↩, source disc + name, 「passage」. */
@Composable
internal fun PassageLine(author: AuthorUi?, passage: String, modifier: Modifier = Modifier, open: (() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null) {
    Row(modifier.heightIn(min = 22.dp), verticalAlignment = Alignment.CenterVertically) {
        LongPressTip("${author?.name ?: "消息"}：$passage", Modifier.weight(1f, fill = trailing != null), onClick = open) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Glyph(R.drawable.ic_reply, 12.dp, ZorkColors.Muted)
                QuoteWho(author ?: AuthorUi("消息", true, null, null, null, null))
                Text("「$passage」", fontSize = QuoteSize, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false))
            }
        }
        trailing?.invoke()
    }
}

@Composable
internal fun TimeText(time: TimeUi, fontSize: TextUnit = 12.sp, modifier: Modifier = Modifier) {
    LongPressTip(time.full, modifier) {
        Text(time.label, fontSize = fontSize, color = ZorkColors.Subtle, maxLines = 1)
    }
}

// ---------------------------------------------------------------- text marks

/** Marks drawn inside a message body: draft passages (dotted underline) and a
 * jumped-to passage (wash that fades). The caller washes the whole message
 * instead when the passage is not in its text. */
internal data class MessageMarks(val drafts: List<String> = emptyList(), val wash: String? = null, val washToken: Long = 0)

/** Quiet dotted underline under a passage already in the reply draft. */
internal class DraftUnderlineSpan(private val view: TextView, private val start: Int, private val end: Int,
    private val color: Int) : LineBackgroundSpan {
    private val dot = Paint(Paint.ANTI_ALIAS_FLAG)
    override fun drawBackground(canvas: Canvas, paint: Paint, left: Int, right: Int, top: Int, baseline: Int, bottom: Int,
        text: CharSequence, lineStart: Int, lineEnd: Int, lineNumber: Int) {
        val layout = view.layout ?: return
        val from = maxOf(start, lineStart); val to = minOf(end, lineEnd)
        if (from >= to) return
        val x1 = layout.getPrimaryHorizontal(from)
        val x2 = if (to >= lineEnd) layout.getLineRight(lineNumber) else layout.getPrimaryHorizontal(to)
        val density = view.resources.displayMetrics.density
        val y = baseline + 4f * density
        dot.color = color
        val step = 3f * density; val radius = .75f * density
        var x = minOf(x1, x2) + radius
        while (x <= maxOf(x1, x2)) { canvas.drawCircle(x, y, radius, dot); x += step }
    }
}

/** Background behind a jumped-to passage; its colour animates out. */
internal class WashSpan(var color: Int) : CharacterStyle(), UpdateAppearance {
    override fun updateDrawState(paint: TextPaint) { paint.bgColor = color }
}

internal fun applyMessageMarks(view: MessageTextView, marks: MessageMarks?) {
    val text = view.text as? Spannable ?: return
    text.getSpans(0, text.length, DraftUnderlineSpan::class.java).forEach(text::removeSpan)
    val subtle = ZorkColors.Subtle.toArgb()
    marks?.drafts?.forEach { quote ->
        val at = text.indexOf(quote)
        if (quote.isNotEmpty() && at >= 0) text.setSpan(DraftUnderlineSpan(view, at, at + quote.length, subtle), at, at + quote.length,
            Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    }
    val wash = marks?.wash
    if (wash == null || marks.washToken == view.appliedWash) return
    view.appliedWash = marks.washToken
    val at = text.indexOf(wash)
    if (wash.isEmpty() || at < 0) return
    val base = AgentTints.Wash.toArgb()
    val span = WashSpan(base)
    text.setSpan(span, at, at + wash.length, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
    // Hold ~1.6 s, then fade. Re-setting the span makes the text view redraw it.
    ValueAnimator.ofFloat(1f, 0f).apply {
        startDelay = 1600; duration = if (ValueAnimator.areAnimatorsEnabled()) 600 else 0
        addUpdateListener {
            val current = view.text as? Spannable ?: return@addUpdateListener
            val s = current.getSpanStart(span); val e = current.getSpanEnd(span)
            if (s < 0) return@addUpdateListener
            span.color = android.graphics.Color.argb((android.graphics.Color.alpha(base) * (it.animatedValue as Float)).toInt(),
                android.graphics.Color.red(base), android.graphics.Color.green(base), android.graphics.Color.blue(base))
            current.removeSpan(span)
            if (it.animatedFraction < 1f) current.setSpan(span, s, e, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
        start()
    }
}
