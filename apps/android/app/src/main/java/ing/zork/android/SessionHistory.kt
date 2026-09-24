package ing.zork.android

import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.painter.Painter
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

internal class HistoryActions(val back: () -> Unit = {}, val older: () -> Unit = {},
    val newer: () -> Unit = {}, val latest: () -> Unit = {}, val retry: () -> Unit = {},
    val detail: (String?) -> Unit = {}, val anchor: (String?) -> Unit = {},
    val navigate: (HistoryDestination) -> Unit = {})

@Composable
internal fun SessionHistoryPage(state: SessionHistoryState, actions: HistoryActions) {
    val status = state.status
    val scroll = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val latestActions by rememberUpdatedState(actions)
    val rows by remember(state) { derivedStateOf { state.visibleRows() } }
    // Reuse vector drawing caches across rows with the same icon and tint.
    val iconResources = remember(state, state.revision) { state.entries.map { historyIcon(it.kind) to it.failed }.distinct() }
    val icons = iconResources.associateWith { (resource, failed) -> key(resource, failed) { painterResource(resource) } }
    var clock by remember(state) { mutableLongStateOf(System.currentTimeMillis()) }
    var identity by remember(state) { mutableStateOf<HistoryIdentity?>(null) }
    var profileOpen by remember(state) { mutableStateOf(false) }
    var usageOpen by remember(state) { mutableStateOf(false) }
    LaunchedEffect(state) { while (true) { delay(30_000); clock = System.currentTimeMillis() } }
    val now = clock + status.clockOffset
    // Loading shows only when it lasts past 300 ms, then stays at least 400 ms.
    val showLoading = rememberDeferredLoading(status.loading)
    fun pin() { state.entries.firstOrNull()?.id?.let { latestActions.anchor(it) } }
    fun page(action: () -> Unit) {
        scope.launch { scroll.scrollToItem(0) }
        action()
    }
    // The page reads like the member's agent session: Markdown output leads,
    // tool calls and messages sit between it. There is no timeline chart.
    Box(Modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Column(Modifier.fillMaxSize()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                HistoryIconAction(R.drawable.ic_arrow_left, "返回对话", onClick = actions.back)
                Column(Modifier.weight(1f).padding(horizontal = 6.dp)
                    .semantics { contentDescription = "执行历史 · ${state.name}" }) {
                    Text(state.name, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val overview = state.overview
                    // One line: model and total tokens. Cache and context are in the usage menu.
                    val line = overview?.let { listOf(it.model, it.tokens).filter(String::isNotBlank).joinToString(" · ") }
                    Text(line ?: "执行历史", fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = if (overview?.profile != null) Modifier.historyPress(label = "模型连接与额度") { profileOpen = true } else Modifier)
                }
                val overview = state.overview
                if (overview != null && (overview.cache.isNotBlank() || overview.context.isNotBlank())) Box {
                    ZorkButton("用量", quiet = true, onClick = { usageOpen = true })
                    PlainMenu("用量", usageOpen, { usageOpen = false }, 200.dp) {
                        listOf("上下文" to overview.context, "Token" to overview.tokens, "缓存" to overview.cache).filter { it.second.isNotBlank() }.forEach { (label, value) ->
                            Row(Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(label, fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.weight(1f))
                                Text(value, fontSize = 13.sp, color = ZorkColors.Ink)
                            }
                        }
                    }
                }
                HistoryIconAction(R.drawable.ic_reload, "刷新执行历史", !status.loading && !status.revoked, actions.retry)
            }
            if (status.error != null) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)
                .background(ZorkColors.DangerSoft, ZorkShapes.Container).padding(start = 16.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Glyph(R.drawable.ic_attention, 16.dp, ZorkColors.Danger)
                Text(status.error, Modifier.weight(1f).padding(horizontal = 8.dp), color = ZorkColors.Danger, fontSize = 13.sp, maxLines = 3, overflow = TextOverflow.Ellipsis)
                if (!status.revoked) HistoryAction("重试", !status.loading, label = "重试加载") { actions.retry() }
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                LazyColumn(state = scroll, modifier = Modifier.fillMaxSize().testTag("history-records").pointerInput(state) {
                    // Pin the current core window when the reader touches the list.
                    // Subsequent live appends cannot discard its reading anchor.
                    awaitPointerEventScope {
                        while (true) {
                            val event = awaitPointerEvent(PointerEventPass.Initial)
                            if (event.changes.any { it.pressed && !it.previousPressed }) pin()
                        }
                    }
                }, contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 12.dp, bottom = 88.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    if (status.older) item(key = "older") {
                        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                            HistoryAction("加载更早记录", !status.loading, label = "更早记录") { page(actions.older) }
                        }
                    }
                    items(rows, key = { it.key }, contentType = { it.group?.let { "group" } ?: it.entry?.kind ?: "entry" }) { item ->
                        val motion = Modifier.animateItem(placementSpec = spring<IntOffset>(dampingRatio = 1f, stiffness = 406f))
                        if (item.group != null) HistoryGroupRow(item.group, state.isExpanded(item.group), now, motion) { state.toggle(item.group) }
                        else item.entry?.let { entry -> HistoryEntryRow(entry, item.child, entry.id == state.highlightedId, now,
                            icons.getValue(historyIcon(entry.kind) to entry.failed), motion,
                            open = { actions.detail(entry.id) }, subject = { target ->
                                if (target.agent != null) identity = target.agent
                                else target.conversation?.let(actions.navigate)
                            }) }
                    }
                }
                if (rows.isEmpty()) Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                    if (showLoading) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = ZorkColors.Muted)
                    else if (status.error == null && status.loaded) Text("暂无执行记录", color = ZorkColors.Muted, fontSize = 13.sp)
                }
                // "回到最新" only fades; nothing scrolls on its own.
                androidx.compose.animation.AnimatedVisibility(status.newer, Modifier.align(Alignment.BottomCenter),
                    enter = zorkFadeIn(), exit = zorkFadeOut()) { Box(Modifier.padding(bottom = 24.dp)) {
                    Row(Modifier.heightIn(min = 44.dp).background(ZorkColors.Ink, ZorkShapes.Control)
                        .historyPress(enabled = !status.loading, radius = 22.dp, label = "回到最新") { page(actions.latest) }
                        .padding(horizontal = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("回到最新", color = ZorkColors.Canvas, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                    }
                } }
            }
            if (showLoading && state.entries.isNotEmpty()) LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp), color = ZorkColors.Muted)
        }
    }
    ZorkRetained(state.takeIf { it.selectedId != null }) { shown, open, closed ->
        HistoryDetailSheet(shown, actions, open, closed)
    }
    ZorkRetained(identity.takeIf { !status.revoked }) { person, open, closed ->
        HistoryIdentitySheet(person, { identity = null }, open, closed)
    }
    ZorkRetained(state.overview?.profile.takeIf { profileOpen && !status.revoked }) { profile, open, closed ->
        HistoryProfileSheet(profile, { profileOpen = false }, open, closed)
    }
    LaunchedEffect(status.revoked) { if (status.revoked) { identity = null; profileOpen = false } }
}

private fun truncatedPreview(text: String) = text.length >= 500 || text.endsWith("…")

@Composable
private fun HistoryEntryRow(entry: HistoryRow, child: Boolean, selected: Boolean, now: Long,
    icon: Painter, modifier: Modifier, open: () -> Unit, subject: (HistorySubject) -> Unit) {
    val elapsed = entry.duration(now)?.let(::historyDuration)
    val whenText = if (entry.start == null && entry.end == null) "" else historyClock(entry.start ?: entry.end)
    val label = "${entry.title} · ${entry.status} · ${historyRelative(entry.start ?: entry.end, now)} · ${entry.preview}"
    when (entry.kind) {
        // Model output is the body of the page: Markdown at reading size.
        "output" -> Column(modifier.fillMaxWidth().padding(vertical = 8.dp)
            .semantics { contentDescription = label }) {
            if (whenText.isNotBlank()) Text(whenText, fontSize = 12.sp, color = ZorkColors.Muted)
            Markdown(entry.preview, Modifier.fillMaxWidth())
            if (truncatedPreview(entry.preview)) Text("查看全文", fontSize = 13.sp, color = ZorkColors.Muted,
                modifier = Modifier.heightIn(min = 44.dp).historyPress(label = "查看全文", onClick = open).wrapContentHeight())
        }
        // Received message: a quote under its sender, the text in Markdown and
        // files by name. A structured payload's source stays in the detail.
        "input", "received" -> {
            val message = entry.message
            val sender = message?.sender ?: entry.subject?.label
            val from = listOfNotNull(sender?.let { "来自 $it" } ?: entry.title,
                message?.device?.takeIf { it != sender }, whenText.ifBlank { null }).joinToString(" · ")
            val text = message?.text ?: entry.preview
            Row(modifier.fillMaxWidth().padding(vertical = 8.dp)
                .historyPress(selected = selected, label = "$from · ${entry.preview}", onClick = open)) {
                Box(Modifier.width(3.dp).heightIn(min = 40.dp).background(ZorkColors.FieldBorder, ZorkShapes.Control))
                Column(Modifier.weight(1f).padding(start = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(if (message?.reply == true) "$from · 回复" else from,
                        fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (text.isNotBlank()) Markdown(text, Modifier.fillMaxWidth())
                    else if (message?.files.isNullOrEmpty()) Text(if (message?.structured == true) "结构化消息，详情中查看原始内容" else "无文字内容",
                        fontSize = 14.sp, color = ZorkColors.Subtle)
                    message?.files?.takeIf { it.isNotEmpty() }?.let { files ->
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Glyph(R.drawable.history_attachment, 14.dp, ZorkColors.Muted)
                            Text(files.joinToString("、"), fontSize = 13.sp, color = ZorkColors.Muted,
                                maxLines = 2, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    if (message?.truncated == true) Text("查看全文", fontSize = 13.sp, color = ZorkColors.Muted,
                        modifier = Modifier.heightIn(min = 44.dp).historyPress(label = "查看全文", onClick = open).wrapContentHeight())
                }
            }
        }
        // A message posted to the Chat: what the member said outside itself.
        "send_message" -> Column(modifier.fillMaxWidth().padding(vertical = 6.dp)
            .background(ZorkColors.Accent.copy(alpha = if (ZorkColors.dark) .16f else .10f), ZorkShapes.Block)
            .historyPress(selected = selected, radius = 16.dp, label = label, onClick = open)
            .padding(horizontal = 14.dp, vertical = 10.dp)) {
            Text(entry.subject?.label?.let { "发送到 Chat「$it」" } ?: entry.title, fontSize = 12.sp, fontWeight = FontWeight.Medium,
                color = ZorkColors.AccentPressed, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(entry.preview, fontSize = 14.sp, lineHeight = 21.sp, color = ZorkColors.Ink, maxLines = 6, overflow = TextOverflow.Ellipsis)
        }
        // Tool calls and other steps: compact, quiet lines between the prose.
        else -> Row(modifier.fillMaxWidth().heightIn(min = 44.dp)
            .historyPress(selected = selected, label = label, onClick = open)
            .padding(start = if (child) 22.dp else 0.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(icon, contentDescription = null, Modifier.size(14.dp), tint = if (entry.failed) ZorkColors.Danger else ZorkColors.Muted)
            Text(entry.title, fontSize = 13.sp, color = if (entry.failed) ZorkColors.Danger else ZorkColors.Muted, maxLines = 1)
            val target = entry.subject
            if (target != null && target.actionable) Box(Modifier.weight(1f)) { HistorySubjectLink(target) { subject(target) } }
            else Text(target?.label ?: entry.preview, Modifier.weight(1f), fontSize = 13.sp,
                color = if (entry.failed) ZorkColors.Danger else ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                fontFamily = if (entry.kind == "shell") ZorkFonts.Mono else null)
            Text(if (entry.state == "running") entry.status else elapsed.orEmpty(), fontSize = 12.sp,
                color = if (entry.failed) ZorkColors.Danger else ZorkColors.Subtle, maxLines = 1)
        }
    }
}

@Composable
private fun HistoryGroupRow(block: HistoryBlock, expanded: Boolean, now: Long, modifier: Modifier, toggle: () -> Unit) {
    Row(modifier.fillMaxWidth().heightIn(min = 44.dp)
        .historyPress(label = "${block.members.size} 项常规操作 · ${block.title}", onClick = toggle)
        .semantics { stateDescription = if (expanded) "已展开" else "已折叠" }
        .padding(end = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Glyph(R.drawable.history_operations, 14.dp, ZorkColors.Muted)
        Text(block.title.ifBlank { "${block.members.size} 项常规操作" }, Modifier.weight(1f), fontSize = 13.sp,
            color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Icon(painterResource(R.drawable.ic_chevron_down), contentDescription = null,
            Modifier.size(14.dp).graphicsLayer { rotationZ = if (expanded) 180f else 0f }, tint = ZorkColors.Subtle)
    }
}
