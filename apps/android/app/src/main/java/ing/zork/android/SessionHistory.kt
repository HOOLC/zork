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
    val latestRows by rememberUpdatedState(rows)
    // Reuse vector drawing caches across rows with the same icon and tint.
    // New visible records do not need a fresh painter for an already drawn glyph.
    val iconResources = remember(state, state.revision) { state.entries.map { historyIcon(it.kind) to it.failed }.distinct() }
    val icons = iconResources.associateWith { (resource, failed) -> key(resource, failed) { painterResource(resource) } }
    val timeline = state.timeline
    var clock by remember(state) { mutableLongStateOf(System.currentTimeMillis()) }
    var reveal by remember(state) { mutableStateOf<Pair<String, Long>?>(null) }
    var identity by remember(state) { mutableStateOf<HistoryIdentity?>(null) }
    var profileOpen by remember(state) { mutableStateOf(false) }
    LaunchedEffect(state) { while (true) { delay(30_000); clock = System.currentTimeMillis() } }
    val now = clock + status.clockOffset
    fun pin() { state.entries.firstOrNull()?.id?.let { latestActions.anchor(it) } }
    fun page(action: () -> Unit) {
        scope.launch { scroll.scrollToItem(0) }
        timeline.fit(); action()
    }
    LaunchedEffect(reveal) {
        val id = reveal?.first ?: return@LaunchedEffect
        withFrameNanos { }
        val index = latestRows.indexOfFirst { it.entry?.id == id }
        if (index >= 0) scroll.animateScrollToItem(index + if (state.status.older) 1 else 0)
    }
    BoxWithConstraints(Modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        val footerLimit = maxHeight * .48f
        Column(Modifier.fillMaxSize()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                HistoryIconAction(R.drawable.ic_arrow_left, "返回对话", onClick = actions.back)
                Column(Modifier.weight(1f).padding(horizontal = 6.dp)) {
                    Text("执行历史", fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(state.name, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                HistoryIconAction(R.drawable.ic_reload, "刷新执行历史", !status.loading && !status.revoked, actions.retry)
            }
            HorizontalDivider(color = ZorkColors.Border, thickness = .5.dp)
            if (status.error != null) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
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
                }, contentPadding = PaddingValues(horizontal = 8.dp, vertical = 4.dp)) {
                    if (status.older) item(key = "older") {
                        Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                            HistoryAction("加载更早记录", !status.loading, label = "更早记录") { page(actions.older) }
                        }
                    }
                    items(rows, key = { it.key }, contentType = { if (it.group != null) "group" else "entry" }) { item ->
                        val motion = Modifier.animateItem(placementSpec = spring<IntOffset>(dampingRatio = 1f, stiffness = 406f))
                        if (item.group != null) HistoryGroupRow(item.group, state.isExpanded(item.group), now, motion) { state.toggle(item.group) }
                        else item.entry?.let { entry -> HistoryEntryRow(entry, item.child, entry.id == state.highlightedId, now, icons.getValue(historyIcon(entry.kind) to entry.failed), motion,
                            open = { actions.detail(entry.id) }, subject = { target ->
                                if (target.agent != null) identity = target.agent
                                else target.conversation?.let(actions.navigate)
                            }) }
                    }
                    if (status.newer) item(key = "newer") {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                            HistoryAction("较新记录", !status.loading) { page(actions.newer) }
                            HistoryAction("最新记录", !status.loading) { page(actions.latest) }
                        }
                    }
                }
                if (rows.isEmpty()) Box(Modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
                    if (status.loading) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = ZorkColors.Muted)
                    else if (status.error == null && status.loaded) Text(if (state.entries.isEmpty()) "暂无执行记录" else "模型请求可在时间轴中查看",
                        color = ZorkColors.Muted, fontSize = 13.sp)
                }
            }
            if (status.loading && state.entries.isNotEmpty()) LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp), color = ZorkColors.Muted)
            if (!status.revoked) {
                HorizontalDivider(color = ZorkColors.Border, thickness = .5.dp)
                // Large accessibility text gets its own scrollable footer while
                // keeping at least half of the available height for records.
                Column(Modifier.fillMaxWidth().heightIn(max = footerLimit).verticalScroll(rememberScrollState())) {
                    state.overview?.let { overview ->
                        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                                Text(overview.model, Modifier.weight(1f), fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                overview.profile?.let { profile -> HistoryAction(profile.name, modifier = Modifier.widthIn(max = 160.dp), label = "模型连接与额度") { profileOpen = true } }
                            }
                            Text(overview.context, fontSize = 12.sp, color = ZorkColors.Muted)
                            Text(overview.tokens, fontSize = 12.sp, color = ZorkColors.Muted)
                            Text(overview.cache, fontSize = 12.sp, color = ZorkColors.Muted)
                        }
                    }
                    if (state.entries.isNotEmpty()) HistoryTimeline(state.entries, state.revision, now, state.highlightedId, timeline,
                        select = { id -> pin(); state.reveal(id); reveal = id to ((reveal?.second ?: 0L) + 1) }, detail = { actions.detail(it) })
                }
            }
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

@Composable
private fun HistoryEntryRow(entry: HistoryRow, child: Boolean, selected: Boolean, now: Long,
    icon: Painter, modifier: Modifier, open: () -> Unit, subject: (HistorySubject) -> Unit) {
    val elapsed = entry.duration(now)?.let(::historyDuration)
    val whenText = if (entry.start == null && entry.end == null) "—" else historyRelative(entry.start ?: entry.end, now)
    val caption = entry.preview.takeUnless { it == entry.subject?.label }.orEmpty()
        .ifBlank { listOf(entry.model, entry.status, elapsed.orEmpty()).filter(String::isNotBlank).joinToString(" · ") }
    val subjectBelow = entry.subject != null && androidx.compose.ui.platform.LocalDensity.current.fontScale > 1.4f
    val firstLine = with(androidx.compose.ui.platform.LocalDensity.current) { 20.sp.toDp().coerceAtLeast(24.dp) }
        .coerceAtLeast(if (entry.subject?.actionable == true && !subjectBelow) 44.dp else 24.dp)
    androidx.compose.ui.layout.Layout(modifier = modifier.fillMaxWidth().heightIn(min = 60.dp)
        .historyPress(selected = selected, label = "${entry.title} · ${entry.status} · ${historyRelative(entry.start ?: entry.end, now)} · ${entry.preview}", onClick = open), content = {
        Icon(icon, contentDescription = null, Modifier.size(16.dp), tint = if (entry.failed) ZorkColors.Danger else ZorkColors.Muted)
        Text(entry.title, fontSize = 13.sp, fontWeight = FontWeight.Medium,
            color = if (entry.failed) ZorkColors.Danger else ZorkColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
        entry.subject?.let { target ->
            if (target.actionable) HistorySubjectLink(target) { subject(target) }
            else Text(target.label, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Text(whenText, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1)
        Text(caption, fontSize = 12.sp, lineHeight = 18.sp,
            color = if (entry.failed) ZorkColors.Danger else ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }) { children, constraints ->
        // Each piece is measured once. Scrolling only moves this compact layout,
        // instead of revisiting nested weighted rows for every visible record.
        val start = (if (child) 28.dp else 8.dp).roundToPx()
        val top = 6.dp.roundToPx()
        val textLeft = start + 26.dp.roundToPx()
        val width = (constraints.maxWidth - textLeft - 8.dp.roundToPx()).coerceAtLeast(0)
        val gap = 6.dp.roundToPx()
        val hasSubject = entry.subject != null
        val timeIndex = if (hasSubject) 3 else 2
        fun measure(index: Int, maxWidth: Int, fill: Boolean = false) = children[index].measure(
            androidx.compose.ui.unit.Constraints(minWidth = if (fill) maxWidth else 0, maxWidth = maxWidth))
        val icon = children[0].measure(androidx.compose.ui.unit.Constraints.fixed(16.dp.roundToPx(), 16.dp.roundToPx()))
        val time = measure(timeIndex, width)
        val inlineSubject = hasSubject && !subjectBelow
        val availableTitle = (width - time.width - gap * if (inlineSubject) 2 else 1).coerceAtLeast(0)
        val title = measure(1, if (inlineSubject) minOf(110.dp.roundToPx(), availableTitle) else availableTitle, !inlineSubject)
        val subjectWidth = if (subjectBelow) ((width - gap).coerceAtLeast(0) * .45f).toInt()
            else (width - title.width - time.width - gap * 2).coerceAtLeast(0)
        val target = if (hasSubject) measure(2, subjectWidth, true) else null
        val preview = measure(timeIndex + 1, if (subjectBelow) (width - subjectWidth - gap).coerceAtLeast(0) else width, true)
        val firstHeight = maxOf(firstLine.roundToPx(), title.height, time.height, if (inlineSubject) target!!.height else 0)
        val secondHeight = maxOf(preview.height, if (subjectBelow) target!!.height else 0)
        layout(constraints.maxWidth, constraints.constrainHeight(top * 2 + firstHeight + secondHeight)) {
            icon.place(start + 1.dp.roundToPx(), top + (firstHeight - icon.height) / 2)
            title.place(textLeft, top + (firstHeight - title.height) / 2)
            time.place(textLeft + width - time.width, top + (firstHeight - time.height) / 2)
            target?.let {
                if (subjectBelow) it.place(textLeft, top + firstHeight + (secondHeight - it.height) / 2)
                else it.place(textLeft + title.width + gap, top + (firstHeight - it.height) / 2)
            }
            preview.place(textLeft + if (subjectBelow) subjectWidth + gap else 0,
                top + firstHeight + (secondHeight - preview.height) / 2)
        }
    }
}

@Composable
private fun HistoryGroupRow(block: HistoryBlock, expanded: Boolean, now: Long, modifier: Modifier, toggle: () -> Unit) {
    val firstLine = with(androidx.compose.ui.platform.LocalDensity.current) { 20.sp.toDp().coerceAtLeast(24.dp) }
    Row(modifier.fillMaxWidth().heightIn(min = 60.dp)
        .historyPress(label = "${block.members.size} 项常规操作 · ${block.title}", onClick = toggle)
        .semantics { stateDescription = if (expanded) "已展开" else "已折叠" }
        .padding(horizontal = 8.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
        Box(Modifier.width(18.dp).height(firstLine), contentAlignment = Alignment.Center) { Glyph(R.drawable.history_operations, 16.dp, ZorkColors.Muted) }
        Column(Modifier.weight(1f)) {
            Row(Modifier.fillMaxWidth().heightIn(min = firstLine), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(block.title.ifBlank { "${block.members.size} 项常规操作" }, Modifier.weight(1f), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(if (expanded) "收起" else "展开", fontSize = 12.sp, color = ZorkColors.Muted)
            }
            Text(block.summary.ifBlank { historyRelative(block.start, now) }, fontSize = 12.sp, lineHeight = 18.sp, color = ZorkColors.Muted,
                maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
