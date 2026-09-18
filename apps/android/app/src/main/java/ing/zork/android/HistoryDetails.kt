package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun HistorySheet(title: String, dismiss: () -> Unit, open: Boolean, closed: () -> Unit, content: @Composable ColumnScope.() -> Unit) {
    LiquidSheet(open, title, dismiss, onClosed = closed) {
        Column(Modifier.fillMaxWidth().heightIn(max = (LocalConfiguration.current.screenHeightDp * .90f).dp).navigationBarsPadding()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(start = 20.dp, end = 10.dp, top = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f).semantics { heading() }, fontSize = 17.sp, fontWeight = FontWeight.SemiBold,
                    maxLines = 2, overflow = TextOverflow.Ellipsis)
                HistoryIconAction(R.drawable.ic_x, "关闭记录详情", onClick = dismiss)
            }
            HorizontalDivider(color = ZorkColors.Border, thickness = .5.dp)
            content()
        }
    }
}

@Composable
internal fun HistoryTimelineSelectionSheet(entries: List<HistoryRow>, select: (String) -> Unit, dismiss: () -> Unit, open: Boolean, closed: () -> Unit) {
    HistorySheet("时间轴记录", dismiss, open, closed) {
        LazyColumn(Modifier.fillMaxWidth(), contentPadding = PaddingValues(12.dp)) {
            items(entries, key = { it.id }) { entry ->
                Row(Modifier.fillMaxWidth().heightIn(min = 52.dp)
                    .historyPress(label = "${entry.title} · ${historyClock(entry.start ?: entry.end)} · ${entry.status}") { select(entry.id); dismiss() }
                    .padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Glyph(if (entry.lane == 1) R.drawable.history_operations else historyIcon(entry.kind), 16.dp, if (entry.failed) ZorkColors.Danger else ZorkColors.Muted)
                    Column(Modifier.weight(1f)) {
                        Text(entry.title, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("${historyClock(entry.start ?: entry.end)} · ${entry.status}", fontSize = 11.sp, color = ZorkColors.Muted)
                    }
                }
            }
        }
    }
}

@Composable
internal fun HistoryDetailSheet(state: SessionHistoryState, actions: HistoryActions, open: Boolean, closed: () -> Unit) {
    val detail = state.detail
    val clipboard = LocalClipboard.current
    val scope = rememberCoroutineScope()
    var copied by remember(state.selectedId) { mutableStateOf<String?>(null) }
    val rawOpen = remember(state.selectedId) { mutableStateMapOf<Int, Boolean>() }
    fun copy(section: HistorySection) {
        scope.launch {
            try {
                clipboard.setClipEntry(ClipEntry(android.content.ClipData.newPlainText(section.title, section.text)))
                copied = "已复制${section.title}"
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { copied = "复制失败，可选择需要的片段复制" }
        }
    }
    val title = state.entries.firstOrNull { it.id == state.selectedId }?.title ?: detail?.title ?: "执行记录"
    HistorySheet(title, { actions.detail(null) }, open, closed) {
        if (detail == null) Box(Modifier.fillMaxWidth().heightIn(min = 180.dp).padding(20.dp), contentAlignment = Alignment.Center) {
            if (state.status.error != null) Text(state.status.error!!, color = ZorkColors.Danger, fontSize = 13.sp)
            else CircularProgressIndicator(Modifier.size(22.dp), color = ZorkColors.Muted, strokeWidth = 2.dp)
        } else key(detail.id) {
            // One selection scope spans the visible chunks; full-section copy
            // always uses the original text, including off-screen content.
            SelectionContainer(Modifier.weight(1f, fill = false)) {
                LazyColumn(Modifier.fillMaxWidth(), contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 20.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    item(key = "metadata") {
                        Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(listOf(historyStateLabel(detail.state), detail.model).filter(String::isNotBlank).joinToString(" · "),
                                fontSize = 12.sp, color = if (detail.state == "failed" || detail.state == "timed_out") ZorkColors.Danger else ZorkColors.Muted)
                            Text("开始 ${historyClock(detail.start, true)}", fontSize = 12.sp, color = ZorkColors.Muted)
                            Text("结束 ${historyClock(detail.end, true)}", fontSize = 12.sp, color = ZorkColors.Muted)
                            if (detail.start != null && detail.end != null) Text("耗时 ${historyDuration((detail.end - detail.start).coerceAtLeast(0))}", fontSize = 12.sp, color = ZorkColors.Muted)
                            state.entries.firstOrNull { it.id == detail.id }?.requestedWait?.let { requested ->
                                Text("请求等待 ${historyDuration(requested)}", fontSize = 12.sp, color = ZorkColors.Muted)
                            }
                            detail.usage?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
                        }
                    }
                    detail.sections.forEachIndexed { index, section ->
                        val expanded = !section.code || rawOpen[index] == true
                        item(key = "section:$index") {
                            Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
                                if (section.code) Row(Modifier.weight(1f).heightIn(min = 44.dp)
                                    .historyPress(label = section.title) { rawOpen[index] = !expanded }
                                    .semantics { stateDescription = if (expanded) "已展开" else "已折叠" }, verticalAlignment = Alignment.CenterVertically) {
                                    Text(section.title, Modifier.weight(1f), fontSize = 12.sp, color = ZorkColors.Muted)
                                    Text(if (expanded) "收起" else "展开", fontSize = 11.sp, color = ZorkColors.Muted)
                                } else Text(section.title, Modifier.weight(1f).semantics { heading() }, fontSize = 12.sp, color = ZorkColors.Muted)
                                HistoryIconAction(R.drawable.ic_copy, "复制${section.title}") { copy(section) }
                            }
                        }
                        if (expanded) items(section.chunks.size, key = { "section:$index:$it" }, contentType = { if (section.code) "json" else "text" }) { chunk ->
                            Text(section.chunks[chunk], Modifier.fillMaxWidth().then(if (section.code) Modifier.background(ZorkColors.Paper).padding(8.dp) else Modifier),
                                fontSize = if (section.code) 12.sp else 14.sp, lineHeight = if (section.code) 18.sp else 21.sp,
                                fontFamily = if (section.code) FontFamily.Monospace else ZorkFonts.Body)
                        }
                    }
                }
            }
        }
        copied?.let { Text(it, Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), fontSize = 12.sp, color = ZorkColors.Muted) }
    }
}

@Composable
internal fun HistoryIdentitySheet(identity: HistoryIdentity, dismiss: () -> Unit, open: Boolean, closed: () -> Unit) {
    HistorySheet(identity.name, dismiss, open, closed) {
        LazyColumn(Modifier.fillMaxWidth(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item { Avatar(identity.avatar, 44.dp, identity.name) }
            items(listOf("身份" to identity.id, "角色" to when (identity.role) { "leader" -> "领队"; "worker" -> "队员"; else -> identity.role },
                "模型" to identity.model, "模型连接" to identity.profile, "思考深度" to identity.thinking).filter { it.second.isNotBlank() }) { (label, text) ->
                Column { Text(label, fontSize = 12.sp, color = ZorkColors.Muted); SelectionContainer { Text(text, fontSize = 14.sp) } }
            }
        }
    }
}

@Composable
internal fun HistoryProfileSheet(profile: HistoryQuota, dismiss: () -> Unit, open: Boolean, closed: () -> Unit) {
    HistorySheet(profile.name, dismiss, open, closed) {
        LazyColumn(Modifier.fillMaxWidth(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item { Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                ProviderMark(profile.provider); Text(profile.provider, fontSize = 14.sp)
            } }
            items(profile.lines) { line -> Text(line, fontSize = 13.sp, color = ZorkColors.Muted) }
            if (profile.lines.isEmpty()) item { Text("提供商未报告额度", fontSize = 13.sp, color = ZorkColors.Muted) }
            profile.checked?.let { checked -> item { Text("额度更新于 $checked", fontSize = 12.sp, color = ZorkColors.Muted) } }
        }
    }
}
