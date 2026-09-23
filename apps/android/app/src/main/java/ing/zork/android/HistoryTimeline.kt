package ing.zork.android

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateCentroid
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.*
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionOnScreen
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToLong

internal data class HistorySpan(val row: HistoryRow, val start: Long, val end: Long, val track: Int)

/** Presentation-only clock geometry, matching the native timeline: simultaneous
 * calls occupy separate tracks, and intervals without an event are removed. */
internal class HistoryAxis(entries: List<HistoryRow>, now: Long) {
    val spans: List<HistorySpan>
    val tracks = intArrayOf(1, 1, 1)
    val start: Long
    val end: Long
    val duration: Double
    val gaps: List<Pair<Long, Long>>
    val unknown: Int
    init {
        val timed = entries.mapNotNull { row ->
            (row.start ?: row.end)?.let { first -> HistorySpan(row, first,
                (row.end ?: if (row.state == "running") max(now, first) else first).coerceAtLeast(first), 0) }
        }
        unknown = entries.size - timed.size
        start = timed.minOfOrNull { it.start } ?: 0L
        end = timed.maxOfOrNull { it.end } ?: start
        spans = (0..2).flatMap { lane ->
            val ends = mutableListOf<Pair<Long, Boolean>>()
            timed.filter { it.row.lane == lane }.sortedWith(compareBy({ it.start }, { it.end })).map { span ->
                val found = ends.indexOfFirst { (time, point) -> time < span.start || (time == span.start && !point) }
                val track = if (found < 0) ends.size else found
                val next = span.end to (span.end == span.start)
                if (track == ends.size) ends.add(next) else ends[track] = next
                tracks[lane] = max(1, ends.size)
                span.copy(track = track)
            }
        }
        var covered = start
        val idle = buildList {
            timed.sortedWith(compareBy({ it.start }, { it.end })).forEach { span ->
                if (span.start > covered) add(covered to span.start)
                covered = max(covered, span.end)
            }
        }
        // A page of instantaneous events has no active duration to scale.
        // Keep its real intervals instead of collapsing every point to x = 0.
        gaps = if (timed.any { it.end > it.start }) idle else emptyList()
        duration = max(1L, end - start - gaps.sumOf { it.second - it.first }).toDouble()
    }
    fun position(time: Long): Double = (time - start - gaps.sumOf { (a, b) -> (time - a).coerceIn(0, b - a) }) / duration
    fun timeAt(fraction: Double): Long {
        if (fraction < 0.0) return start
        if (fraction >= 1.0) return end
        val active = (fraction * duration).roundToLong()
        var removed = 0L
        for ((a, b) in gaps) { if (active >= a - start - removed) removed += b - a else break }
        return (start + active + removed).coerceIn(start, end)
    }
}

internal data class HistoryBar(val span: HistorySpan, val bounds: Rect)

/** One clipped geometry for drawing and picking, including a point at x = width. */
internal fun historyBars(axis: HistoryAxis, view: HistoryTimelineState, width: Float,
    heights: List<Float>, scale: Float): List<HistoryBar> {
    if (width <= 0) return emptyList()
    val modelEnds = FloatArray(axis.tracks[1]) { -Float.MAX_VALUE }
    return axis.spans.mapNotNull { span ->
        val left = (view.screen(axis, span.start) * width).toFloat()
        val right = (view.screen(axis, span.end) * width).toFloat()
        if (right < 0 || left > width) return@mapNotNull null
        val lane = span.row.lane
        val trackHeight = heights[lane] * scale / axis.tracks[lane]
        val top = (4f + heights.take(lane).sum()) * scale + span.track * trackHeight + .5f * scale
        val minimum = min(2f * scale, width)
        var x = left.coerceIn(0f, width - minimum)
        var end = max(right, x + minimum).coerceAtMost(width)
        // Match the desktop's one-pixel separation between model calls.
        if (lane == 1) {
            x = max(x + .5f * scale, modelEnds[span.track] + scale)
            end = max(end - .5f * scale, x + minimum)
            modelEnds[span.track] = end
            if (x >= width) return@mapNotNull null
            x = x.coerceAtMost(width - minimum)
            end = end.coerceAtMost(width)
        }
        HistoryBar(span, Rect(x, top, end, top + trackHeight - scale))
    }
}

internal fun historyAxisClock(time: Long, visibleDuration: Long): String =
    historyClock(time) + if (visibleDuration < 10_000) ".%03d".format(java.util.Locale.ROOT, Math.floorMod(time, 1000)) else ""

internal class HistoryTimelineState {
    var zoom by mutableStateOf(1.0)
        private set
    var offset by mutableStateOf(0.0)
        private set
    var selecting by mutableStateOf(false)
    var range by mutableStateOf<Pair<Long, Long>?>(null)
    fun transform(factor: Double, pan: Double = 0.0, anchor: Double = .5) {
        val time = offset + anchor / zoom
        val next = (zoom * factor).coerceIn(1.0, 64.0)
        offset = (time - anchor / next - pan / zoom).coerceIn(0.0, 1.0 - 1.0 / next)
        zoom = next
    }
    fun fit() { zoom = 1.0; offset = 0.0; range = null; selecting = false }
    fun fitRange(axis: HistoryAxis) {
        val (a, b) = range ?: return
        val first = axis.position(min(a, b)).coerceIn(0.0, 1.0)
        val last = axis.position(max(a, b)).coerceIn(0.0, 1.0)
        zoom = (1.0 / (last - first).coerceAtLeast(1.0 / 64)).coerceIn(1.0, 64.0)
        offset = first.coerceIn(0.0, 1.0 - 1.0 / zoom)
        selecting = false
    }
    fun screen(axis: HistoryAxis, time: Long) = (axis.position(time) - offset) * zoom
    fun timeAt(axis: HistoryAxis, position: Double) = axis.timeAt((offset + position.coerceIn(0.0, 1.0) / zoom).coerceIn(0.0, 1.0))
}

internal fun historyTimelineColor(row: HistoryRow) = when {
    row.failed -> ZorkColors.HistoryError
    row.lane == 0 -> ZorkColors.HistoryInput
    row.lane == 1 -> ZorkColors.HistoryModel
    else -> ZorkColors.HistoryTool
}

@Composable
internal fun HistoryTimeline(entries: List<HistoryRow>, revision: Long, now: Long, selected: String?,
    view: HistoryTimelineState, select: (String) -> Unit, detail: (String) -> Unit) {
    val axis = remember(view, revision, now) { HistoryAxis(entries, now) }
    val density = LocalDensity.current
    val rowHeight = max(11f, with(density) { 10.sp.toDp().value } * 1.1f)
    val labelWidth = max(36f, with(density) { 10.sp.toDp().value } * 2.3f + 8f)
    val heights = remember(axis, rowHeight) { axis.tracks.map { it * rowHeight } }
    val totalHeight = max(44f, 4f + heights.sum())
    var chartWidth by remember { mutableIntStateOf(0) }
    val bars = remember(axis, view.zoom, view.offset, chartWidth, rowHeight, density.density) {
        historyBars(axis, view, chartWidth.toFloat(), heights, density.density)
    }
    val latestBars by rememberUpdatedState(bars)
    val latestSelect by rememberUpdatedState(select)
    val visibleDuration = view.timeAt(axis, 1.0) - view.timeAt(axis, 0.0)
    val tickWidth = (if (visibleDuration < 10_000) 82 else 58) * density.density * density.fontScale
    val tickSpacing = tickWidth * 1.5f + 8f * density.density * density.fontScale
    val tickCount = (1 + (chartWidth / tickSpacing).toInt()).coerceIn(2, 5)
    val tickLabels = remember(axis, view.zoom, view.offset, tickCount) {
        List(tickCount) { tick ->
            if (axis.spans.isEmpty()) "—" else historyAxisClock(view.timeAt(axis, tick.toDouble() / (tickCount - 1)), visibleDuration)
        }
    }
    val breakMarks = remember(axis, view.zoom, view.offset, chartWidth) {
        axis.gaps.map { (from, _) -> (view.screen(axis, from) * chartWidth).toFloat() }
            .filter { it in 0f..chartWidth.toFloat() }
    }
    val chosen = entries.firstOrNull { it.id == selected }
    var choosing by remember { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp).graphicsLayer()) {
        HorizontalDivider(color = ZorkColors.Border, thickness = .5.dp)
        // Desktop order: clock labels, idle dots, then three compact solid lanes.
        val stackedTicks = chartWidth < 2 * tickWidth
        val ticks: @Composable () -> Unit = {
            repeat(tickCount) { tick ->
                Text(tickLabels[tick],
                    if (stackedTicks) Modifier.fillMaxWidth().wrapContentWidth(if (tick == 0) Alignment.Start else Alignment.End) else Modifier,
                    fontSize = 10.sp, fontFamily = ZorkFonts.Mono, color = ZorkColors.Muted, maxLines = 1)
            }
        }
        val tickModifier = Modifier.fillMaxWidth().padding(start = labelWidth.dp, top = 4.dp)
        if (stackedTicks) Column(tickModifier) { ticks() }
        else Layout(content = ticks, modifier = tickModifier) { measurables, constraints ->
            val labels = measurables.map { it.measure(constraints.copy(minWidth = 0, minHeight = 0)) }
            layout(constraints.maxWidth, labels.maxOfOrNull { it.height } ?: 0) {
                labels.forEachIndexed { index, label ->
                    val center = constraints.maxWidth * index / (labels.size - 1)
                    label.placeRelative((center - label.width / 2).coerceIn(0, constraints.maxWidth - label.width), 0)
                }
            }
        }
        Row(Modifier.fillMaxWidth().heightIn(max = 128.dp).verticalScroll(rememberScrollState())) {
            Column(Modifier.width(labelWidth.dp).padding(top = 4.dp)) {
                listOf("输入", "模型", "工具").forEachIndexed { index, name ->
                    Box(Modifier.height(heights[index].dp).fillMaxWidth().padding(end = 6.dp), contentAlignment = Alignment.TopEnd) {
                        Text(name, fontSize = 10.sp, lineHeight = 11.sp, color = ZorkColors.Muted, maxLines = 1)
                    }
                }
            }
            val transform = if (view.selecting) Modifier else Modifier.pointerInput(axis) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    var total = Offset.Zero
                    var transforming = false
                    while (true) {
                        val event = awaitPointerEvent()
                        if (event.changes.none { it.pressed } || event.changes.any { it.isConsumed }) break
                        val pan = event.calculatePan()
                        total += pan
                        if (!transforming) {
                            transforming = event.changes.count { it.pressed } > 1 ||
                                (abs(total.x) > viewConfiguration.touchSlop && abs(total.x) > abs(total.y))
                            // Leave vertical drags to the track/footer scroller.
                            if (!transforming && abs(total.y) > viewConfiguration.touchSlop) break
                        }
                        if (transforming) {
                            val center = event.calculateCentroid(useCurrent = false)
                            val width = size.width.toDouble().coerceAtLeast(1.0)
                            view.transform(event.calculateZoom().toDouble(), pan.x / width, center.x / width)
                            event.changes.forEach { it.consume() }
                        }
                    }
                }
            }
            Canvas(Modifier.weight(1f).height(totalHeight.dp).onSizeChanged { chartWidth = it.width }.testTag("history-timeline")
                .semantics {
                    contentDescription = "执行时间轴"
                    stateDescription = "缩放 ${view.zoom.toInt()} 倍，${axis.spans.size} 条记录${if (view.selecting) "，拖动选择时间范围" else "，双指缩放，拖动平移"}"
                    customActions = listOf(
                        CustomAccessibilityAction("选择时间轴记录") { choosing = true; true },
                        CustomAccessibilityAction("上一条记录") {
                            val at = entries.indexOfFirst { it.id == selected }
                            if (at > 0) { latestSelect(entries[at - 1].id); true } else false
                        },
                        CustomAccessibilityAction("下一条记录") {
                            val at = entries.indexOfFirst { it.id == selected } + 1
                            if (at in entries.indices) { latestSelect(entries[at].id); true } else false
                        },
                    )
                }.then(transform)
                .pointerInput(axis, view.selecting) {
                    fun time(x: Float) = view.timeAt(axis, x / size.width.toDouble().coerceAtLeast(1.0))
                    if (view.selecting) detectDragGestures(onDragStart = { point -> view.range = time(point.x) to time(point.x) }) { change, _ ->
                        change.consume(); view.range = (view.range?.first ?: time(change.position.x)) to time(change.position.x)
                    } else detectTapGestures(onTap = { point ->
                        val scale = size.height / totalHeight
                        var best: HistorySpan? = null
                        var distance = Float.MAX_VALUE
                        for ((span, bounds) in latestBars) {
                            val dx = max(0f, max(bounds.left - point.x, point.x - bounds.right))
                            val dy = abs(bounds.center.y - point.y)
                            val d = dx * dx + dy * dy
                            if (dx <= 22f * scale && dy <= max(6f * scale, bounds.height / 2) && d <= distance) { best = span; distance = d }
                        }
                        best?.let { latestSelect(it.row.id) }
                    })
                }) {
                val factor = size.height / totalHeight
                view.range?.let { (a, b) ->
                    val x1 = (view.screen(axis, min(a, b)) * size.width).toFloat().coerceIn(0f, size.width)
                    val x2 = (view.screen(axis, max(a, b)) * size.width).toFloat().coerceIn(0f, size.width)
                    drawRect(ZorkColors.HistoryInput.copy(alpha = .10f), Offset(x1, 4f * factor), Size((x2 - x1).coerceAtLeast(1f), size.height - 4f * factor))
                }
                bars.forEach { (span, bounds) ->
                    val color = historyTimelineColor(span.row)
                    if (span.row.id == selected) {
                        drawRoundRect(color.copy(alpha = .10f), bounds.topLeft - Offset(3f, 3f) * factor,
                            Size(bounds.width + 6f * factor, bounds.height + 6f * factor), CornerRadius(4f * factor))
                        drawRoundRect(color.copy(alpha = .75f), bounds.topLeft - Offset(factor, factor),
                            Size(bounds.width + 2f * factor, bounds.height + 2f * factor), CornerRadius(2f * factor))
                    }
                    drawRoundRect(color, bounds.topLeft, bounds.size, CornerRadius(factor))
                }
                breakMarks.forEach { x -> drawCircle(ZorkColors.HistoryBreak, 1.5f * factor, Offset(x, 1.5f * factor)) }
            }
        }
        // Touch and keyboard alternatives stay in a single footer row; the
        // plot keeps the desktop's density instead of reserving blank toolbars.
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f).heightIn(min = 44.dp).historyPress(label = "选择时间轴记录") { choosing = true }
                .padding(horizontal = 4.dp, vertical = 4.dp), contentAlignment = Alignment.CenterStart) {
                val label = when {
                    view.selecting -> view.range?.let { (a, b) -> "选区 ${historyDuration(abs(b - a))}" } ?: "拖动选择时间范围"
                    chosen != null -> "${chosen.title} · ${chosen.status}"
                    else -> "已加载 ${entries.size} 条记录 · 双指缩放"
                }
                Text(label, fontSize = 10.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                    color = if (chosen?.failed == true) ZorkColors.HistoryError else ZorkColors.Muted)
            }
            if (chosen != null) HistoryIconAction(R.drawable.ic_result, "查看时间轴记录详情") { detail(chosen.id) }
            var menuAnchor by remember { mutableStateOf<Rect?>(null) }
            Box(Modifier.onGloballyPositioned {
                val topLeft = it.positionOnScreen()
                menuAnchor = Rect(topLeft, Size(it.size.width.toFloat(), it.size.height.toFloat()))
            }) {
                HistoryIconAction(R.drawable.ic_settings_three, "时间轴操作") { menu = true }
                PlainMenu("时间轴操作", menu, { menu = false }, 200.dp, menuAnchor) {
                    ZorkMenuItem("缩小时间轴", false, enabled = view.zoom > 1, onClick = { view.transform(.5); menu = false })
                    ZorkMenuItem("放大时间轴", false, enabled = view.zoom < 64, onClick = { view.transform(2.0); menu = false })
                    ZorkMenuItem("选择时间范围", view.selecting, onClick = { view.selecting = !view.selecting; view.range = null; menu = false })
                    ZorkMenuItem("适配全部时间", false, onClick = { view.fit(); menu = false })
                    if (view.range != null) ZorkMenuItem("放大选区", false, enabled = view.range?.let { it.first != it.second } == true,
                        onClick = { view.fitRange(axis); menu = false })
                }
            }
        }
    }
    ZorkRetained(entries.takeIf { choosing }) { shown, open, closed ->
        HistoryTimelineSelectionSheet(shown, select, { choosing = false }, open, closed)
    }
}
