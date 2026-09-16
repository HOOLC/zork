package surf.zork.android

import android.content.Intent
import android.os.SystemClock
import android.view.MotionEvent
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SessionHistoryTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation
    private fun nodes(label: String): List<AccessibilityNodeInfo> {
        automation.clearCache()
        val found = mutableListOf<AccessibilityNodeInfo>()
        fun walk(node: AccessibilityNodeInfo?) {
            if (node == null) return
            if (node.isVisibleToUser && (node.contentDescription?.toString()?.let { it == label || it.startsWith("$label ·") } == true || node.text?.toString() == label)) found.add(node)
            for (i in 0 until node.childCount) walk(node.getChild(i))
        }
        walk(automation.rootInActiveWindow)
        return found
    }
    private fun await(label: String): List<AccessibilityNodeInfo> {
        val end = SystemClock.uptimeMillis() + 6000
        while (SystemClock.uptimeMillis() < end) { nodes(label).takeIf { it.isNotEmpty() }?.let { return it }; Thread.sleep(40) }
        capture("failure.png")
        throw AssertionError("Missing $label")
    }
    private fun actionNode(start: AccessibilityNodeInfo): AccessibilityNodeInfo? {
        var node: AccessibilityNodeInfo? = start
        while (node != null && !node.isClickable) node = node.parent
        return node
    }
    private fun screenContains(text: String): Boolean {
        automation.clearCache()
        fun contains(node: AccessibilityNodeInfo?): Boolean {
            if (node == null) return false
            if (node.isVisibleToUser && node.text?.contains(text) == true) return true
            return (0 until node.childCount).any { contains(node.getChild(it)) }
        }
        return contains(automation.rootInActiveWindow)
    }
    private fun click(label: String, last: Boolean = false) {
        if (label in setOf("缩小时间轴", "放大时间轴", "选择时间范围", "适配全部时间", "放大选区") && nodes(label).isEmpty()) click("时间轴操作")
        val matches = await(label)
        var node: AccessibilityNodeInfo? = if (last) matches.last() else matches.first()
        while (node != null && !node.isClickable) node = node.parent
        assertNotNull("Cannot click $label", node)
        val bounds = android.graphics.Rect().also { node!!.getBoundsInScreen(it) }
        // Compose may expose an offscreen parent for a partially visible label.
        // A real touch must stay inside every clipping viewport.
        var parent = node!!.parent
        while (parent != null) {
            val viewport = android.graphics.Rect().also { parent!!.getBoundsInScreen(it) }
            assertTrue("Clipped click target $label", bounds.intersect(viewport))
            parent = parent.parent
        }
        val down = SystemClock.uptimeMillis()
        for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
            MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, bounds.exactCenterX(), bounds.exactCenterY(), 0).let {
                assertTrue(automation.injectInputEvent(it, true)); it.recycle()
            }
            Thread.sleep(40)
        }
        instrumentation.waitForIdleSync(); Thread.sleep(650)
    }
    private fun capture(name: String) {
        val case = InstrumentationRegistry.getArguments().getString("capture_case")
        val folder = instrumentation.targetContext.filesDir.resolve(if (case == null) "session-history" else "session-history-complete/$case").apply { mkdirs() }
        automation.takeScreenshot()?.let { bitmap ->
            folder.resolve(name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }
            bitmap.recycle()
        }
        val tree = StringBuilder()
        fun walk(node: AccessibilityNodeInfo?, depth: Int) {
            if (node == null) return
            val rect = android.graphics.Rect().also { node.getBoundsInScreen(it) }
            tree.append(" ".repeat(depth)).append("${node.text} | ${node.contentDescription} | $rect | click=${node.isClickable}\n")
            for (i in 0 until node.childCount) walk(node.getChild(i), depth + 1)
        }
        walk(automation.rootInActiveWindow, 0)
        folder.resolve(name.removeSuffix(".png") + "-nodes.txt").writeText(tree.toString())
    }
    private fun mixedIntent(long: Boolean = false): Intent {
        val file = instrumentation.targetContext.filesDir.resolve("session-history-fixture.json")
        instrumentation.context.assets.open("session-history-complete.json").use { input -> file.outputStream().use { output -> input.copyTo(output) } }
        return Intent(instrumentation.targetContext, SessionHistoryPreviewActivity::class.java)
            .putExtra("fixture_file", file.absolutePath).putExtra("long_detail", long)
    }
    private fun bounds(label: String) = android.graphics.Rect().also { await(label).first().getBoundsInScreen(it) }
    private fun drag(rect: android.graphics.Rect, x1: Float, y1: Float, x2: Float, y2: Float) {
        val down = SystemClock.uptimeMillis()
        for (i in 0..16) {
            val action = if (i == 0) MotionEvent.ACTION_DOWN else if (i == 16) MotionEvent.ACTION_UP else MotionEvent.ACTION_MOVE
            val x = rect.left + rect.width() * (x1 + (x2 - x1) * i / 16)
            val y = rect.top + rect.height() * (y1 + (y2 - y1) * i / 16)
            MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, x, y, 0).let { automation.injectInputEvent(it, true); it.recycle() }
            Thread.sleep(16)
        }
        Thread.sleep(250)
    }
    private fun pinch(rect: android.graphics.Rect) {
        val down = SystemClock.uptimeMillis()
        val properties = Array(2) { i -> MotionEvent.PointerProperties().apply { id = i; toolType = MotionEvent.TOOL_TYPE_FINGER } }
        fun inject(action: Int, spread: Float, count: Int = 2) {
            val coords = Array(count) { i -> MotionEvent.PointerCoords().apply {
                x = rect.exactCenterX() + (if (i == 0) -1 else 1) * rect.width() * spread
                y = rect.exactCenterY(); pressure = 1f; size = 1f
            } }
            MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, count, properties, coords, 0, 0, 1f, 1f, 0, 0,
                android.view.InputDevice.SOURCE_TOUCHSCREEN, 0).let { automation.injectInputEvent(it, true); it.recycle() }
        }
        inject(MotionEvent.ACTION_DOWN, .12f, 1)
        inject(MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT), .12f)
        for (i in 1..12) { inject(MotionEvent.ACTION_MOVE, .12f + i * .01f); Thread.sleep(18) }
        inject(MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT), .24f)
        inject(MotionEvent.ACTION_UP, .24f, 1)
        Thread.sleep(250)
    }
    private fun scrollToLabel(label: String) {
        repeat(12) {
            val target = nodes(label).firstOrNull()
            if (target != null) {
                var action: AccessibilityNodeInfo = target
                while (!action.isClickable) action = action.parent ?: break
                val rect = android.graphics.Rect().also { action.getBoundsInScreen(it) }
                var parent = action.parent
                var visible = true
                while (parent != null) {
                    val viewport = android.graphics.Rect().also { parent!!.getBoundsInScreen(it) }
                    if (!viewport.contains(rect)) visible = false
                    parent = parent.parent
                }
                if (visible) return
            }
            automation.clearCache()
            fun scrollable(n: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                if (n == null) return null
                if (n.isScrollable && n.isVisibleToUser) return n
                for (i in 0 until n.childCount) scrollable(n.getChild(i))?.let { return it }
                return null
            }
            val node = scrollable(automation.rootInActiveWindow) ?: error("No scrollable surface while finding $label")
            node.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
            // Accessibility scroll animates the viewport; touching during that
            // motion only stops the scroll instead of activating its row.
            Thread.sleep(650)
        }
        await(label)
    }

    @Test fun timelineTimeMappingAndBoundaryGeometry() {
        fun row(id: String, start: Long?, end: Long?, state: String = "succeeded") = HistoryRow(
            id, "模型请求", "", "", 1, false, start, end, state, null, "", null)
        val compressed = HistoryAxis(listOf(row("a", 0, 100), row("b", 1100, 1300)), 1300)
        assertEquals(listOf(100L to 1100L), compressed.gaps)
        assertEquals(300.0, compressed.duration, 0.0)
        assertEquals(0L, compressed.timeAt(0.0))
        assertEquals(1300L, compressed.timeAt(1.0))
        assertEquals(1250L, compressed.timeAt(compressed.position(1250)))
        val points = HistoryAxis(listOf(row("first", 0, 0), row("middle", 1000, 1000), row("last", null, 4000, "failed")), 4000)
        assertTrue(points.gaps.isEmpty())
        assertEquals(.25, points.position(1000), .00001)
        val view = HistoryTimelineState()
        val bars = historyBars(points, view, 300f, listOf(28f, 28f, 28f), 1f)
        assertEquals(3, bars.size)
        assertEquals(300f, bars.last().bounds.right, .001f)
        assertEquals(2f, bars.last().bounds.width, .001f)
        view.transform(4.0, anchor = .25)
        assertEquals(listOf("middle"), historyBars(points, view, 300f, listOf(28f, 28f, 28f), 1f).map { it.span.row.id })
        val running = HistoryAxis(listOf(row("running", 0, null, "running"), row("done", 3000, 3100)), 4000)
        assertTrue(running.gaps.isEmpty())
        assertEquals(4000L, running.spans.first().end)
        assertTrue(historyAxisClock(1125, 2000).endsWith(".125"))
        assertFalse(historyAxisClock(1125, 20000).contains('.'))
    }

    @Test fun timelineUsesDesktopColorsAndCompactTracks() {
        val file = instrumentation.targetContext.filesDir.resolve("session-history-pc.json")
        instrumentation.context.assets.open("session-history-pc.json").use { input -> file.outputStream().use { input.copyTo(it) } }
        val intent = Intent(instrumentation.targetContext, SessionHistoryPreviewActivity::class.java).putExtra("fixture_file", file.absolutePath)
        ActivityScenario.launch<SessionHistoryPreviewActivity>(intent).use { scenario ->
            click("阿狸 · 执行历史")
            val plot = bounds("执行时间轴")
            val density = instrumentation.targetContext.resources.displayMetrics.density
            assertTrue("Desktop timeline tracks must stay compact", plot.height() / density <= 60)
            assertTrue("Touch surface remains accessible", plot.height() / density >= 44)
            scenario.onActivity { assertEquals(14, it.history!!.entries.size) }
            val image = automation.takeScreenshot()!!
            val counts = mutableMapOf(0x2878CE to 0, 0x8056C4 to 0, 0x21865B to 0, 0xD43D45 to 0)
            for (y in plot.top until plot.bottom) for (x in plot.left until plot.right) {
                val color = image.getPixel(x, y) and 0xffffff
                if (color in counts) counts[color] = counts.getValue(color) + 1
            }
            image.recycle()
            counts.forEach { (color, count) -> assertTrue("Missing desktop timeline accent ${color.toString(16)}", count > 20) }
            assertTrue("Model duration must be a long solid bar", counts.getValue(0x8056C4) > plot.width() * 6)
            assertTrue("Wait duration must be a solid tool bar", counts.getValue(0x21865B) > plot.width() * 2)
            capture("desktop-reference.png")
            click("放大时间轴")
            capture("desktop-reference-zoomed.png")
        }
    }

    @Test fun timelineInstantEventsStayVisibleAndSelectable() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent()).use { scenario ->
            click("阿狸 · 执行历史")
            scenario.onActivity { activity ->
                val current = activity.history!!
                val base = 1789200000000L
                val entries = org.json.JSONArray(listOf(0L, 1000L, 4000L).mapIndexed { index, at -> JSONObject()
                    .put("id", "point-$index").put("lane", 1).put("action", "").put("state", "failed")
                    .put("visible", false).put("start", JSONObject.NULL).put("end", base + at) })
                current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session)
                    .put("entries", entries).put("blocks", org.json.JSONArray())))
            }
            Thread.sleep(450)
            capture("instant-events.png")
            val chart = bounds("执行时间轴")
            drag(chart, .998f, .5f, .998f, .5f)
            scenario.onActivity { assertEquals("point-2", it.history!!.highlightedId) }
            capture("last-event-selected.png")
        }
    }

    @Test fun concurrentTracksScrollWithoutPanningTime() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent()).use { scenario ->
            click("阿狸 · 执行历史")
            scenario.onActivity { activity ->
                val current = activity.history!!
                val entries = org.json.JSONArray((0 until 20).map { index -> JSONObject()
                    .put("id", "parallel-$index").put("lane", 1).put("action", "").put("state", "succeeded")
                    .put("visible", false).put("start", 1789200000000L).put("end", 1789200060000L) })
                current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session)
                    .put("entries", entries).put("blocks", org.json.JSONArray())))
            }
            Thread.sleep(400)
            fun scrollActions(): Set<Int> {
                var node: AccessibilityNodeInfo? = await("执行时间轴").first()
                while (node != null && !node.isScrollable) node = node.parent
                assertNotNull("Concurrent tracks need a scrollable viewport", node)
                return node!!.actionList.map { it.id }.toSet()
            }
            val backward = AccessibilityNodeInfo.AccessibilityAction.ACTION_SCROLL_BACKWARD.id
            assertFalse(scrollActions().contains(backward))
            val before = bounds("执行时间轴")
            drag(before, .5f, .4f, .5f, .05f)
            Thread.sleep(350)
            capture("concurrent-tracks.png")
            // Accessibility bounds are clipped to the viewport and therefore
            // stay fixed as the content scrolls. The backward action reflects
            // its actual applied scroll offset.
            assertTrue("Vertical drag must scroll the concurrent tracks", scrollActions().contains(backward))
            scenario.onActivity { assertEquals(0.0, it.history!!.timeline.offset, 0.0) }
        }
    }

    @Test fun groupsTimelineSelectionAndNestedTargets() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent()).use { scenario ->
            click("阿狸 · 执行历史"); await("执行历史")
            var groupLabel = ""
            var firstMember = ""
            scenario.onActivity {
                val history = it.history!!
                val group = history.blocks!!.first { it.grouped }
                groupLabel = "${group.members.size} 项常规操作"
                firstMember = group.members.first()
                assertFalse(history.isExpanded(group))
                assertTrue(history.visibleRows().none { row -> row.entry?.id == firstMember })
            }
            capture("collapsed.png")
            click(groupLabel)
            scenario.onActivity { assertTrue(it.history!!.visibleRows().any { row -> row.entry?.id == firstMember }) }
            // Retarget expansion before the placement spring settles.
            repeat(4) { await(groupLabel).first().performAction(AccessibilityNodeInfo.ACTION_CLICK); Thread.sleep(25) }
            Thread.sleep(650)
            scenario.onActivity { assertTrue(it.history!!.visibleRows().any { row -> row.entry?.id == firstMember }) }
            capture("expanded.png")
            click("选择时间轴记录"); await("时间轴记录"); click("模型请求")
            scenario.onActivity {
                val history = it.history!!
                val chosen = history.entries.first { row -> row.id == history.highlightedId }
                assertEquals(1, chosen.lane)
                assertFalse(chosen.visible)
                assertNull(history.selectedId)
            }
            click("查看时间轴记录详情"); capture("model-detail.png")
            scrollToLabel("开始 JSON · 1"); click("开始 JSON · 1")
            assertTrue("Expanded source JSON is missing", screenContains("step_started"))
            capture("model-source.png")
            click("开始 JSON · 1")
            assertFalse("Collapsed JSON remained visible", screenContains("step_started"))
            click("关闭记录详情")
            click("选择时间轴记录"); click("读取文件")
            scenario.onActivity {
                assertEquals(firstMember, it.history!!.highlightedId)
                assertTrue(it.history!!.visibleRows().any { row -> row.entry?.id == firstMember })
            }
            // A real touch on the input lane's first point selects that input,
            // independently of the model and tool chooser paths.
            val chart = bounds("执行时间轴")
            val dpi = instrumentation.targetContext.resources.displayMetrics.density
            val tap = android.graphics.Rect(chart.left + (2 * dpi).toInt(), chart.top + (8 * dpi).toInt(), chart.left + (4 * dpi).toInt(), chart.top + (10 * dpi).toInt())
            drag(tap, .5f, .5f, .5f, .5f)
            scenario.onActivity { assertEquals(0, it.history!!.entries.first { row -> row.id == it.history!!.highlightedId }.lane) }
            click("放大时间轴")
            scenario.onActivity { assertEquals(2.0, it.history!!.timeline.zoom, .001) }
            val before = doubleArrayOf(0.0)
            scenario.onActivity { before[0] = it.history!!.timeline.offset }
            drag(bounds("执行时间轴"), .75f, .5f, .35f, .5f)
            scenario.onActivity { assertTrue(it.history!!.timeline.offset > before[0]) }
            pinch(bounds("执行时间轴"))
            scenario.onActivity { assertTrue(it.history!!.timeline.zoom > 2) }
            click("选择时间范围")
            drag(bounds("执行时间轴"), .2f, .8f, .8f, .8f)
            scenario.onActivity { assertTrue(it.history!!.timeline.range!!.let { range -> range.first != range.second }) }
            capture("range.png")
            click("适配全部时间")
            scenario.onActivity { assertEquals(1.0, it.history!!.timeline.zoom, .001); assertNull(it.history!!.timeline.range) }
            scenario.onActivity {
                val prior = it.history!!.blocks!!.first { block -> firstMember in block.members }
                assertTrue(it.history!!.isExpanded(prior))
                it.prepend()
                val next = it.history!!.blocks!!.first { block -> firstMember in block.members }
                assertNotEquals(prior.id, next.id)
                assertTrue(it.history!!.isExpanded(next))
            }
            click("选择时间轴记录"); scrollToLabel("分配任务"); click("分配任务")
            click("小熊"); await("身份"); capture("identity.png"); click("关闭记录详情")
            click("选择时间轴记录"); scrollToLabel("发送消息"); click("发送消息")
            click("工作对话")
            scenario.onActivity { assertEquals(listOf("session-a"), it.destinations); assertNull(it.history!!.selectedId) }
            click("模型连接与额度"); await("测试连接"); capture("quota.png"); click("关闭记录详情")
            Thread.sleep(1000)
            scenario.onActivity { synchronized(it.frameTimes) { it.frameTimes.clear() } }
            Thread.sleep(350)
            scenario.onActivity { assertTrue("Settled UI kept drawing: ${it.frameTimes}", synchronized(it.frameTimes) { it.frameTimes.size } <= 2) }
        }
    }

    @Test fun configuredWidthTouchTargetsAndLongContent() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent(long = true)).use { scenario ->
            await("阿狸 · 执行历史"); Thread.sleep(450)
            capture("entry.png")
            val density = instrumentation.targetContext.resources.displayMetrics.density
            val targets = listOf("阿狸 · 执行历史", "小熊 · 执行历史").flatMap { label -> nodes(label).mapNotNull(::actionNode).distinct().map { node ->
                android.graphics.Rect().also { node.getBoundsInScreen(it) }
            } }
            assertEquals("Both header and composer entries must be present", 4, targets.size)
            targets.forEach { assertTrue("Small hit target $it at density $density", it.width() / density >= 43.5f && it.height() / density >= 43.5f) }
            assertFalse("Dock targets overlap", android.graphics.Rect.intersects(targets[1], targets[3]))
            capture("entry.png")
            click("阿狸 · 执行历史", last = true); await("执行历史")
            capture("collapsed.png")
            var label = ""
            scenario.onActivity { label = "${it.history!!.blocks!!.first { group -> group.grouped }.members.size} 项常规操作" }
            click(label); capture("expanded.png")
            if (instrumentation.targetContext.resources.configuration.fontScale > 1.4f) {
                drag(bounds("选择时间轴记录"), .8f, .5f, .8f, -6f)
                Thread.sleep(350)
            }
            val labels = mutableListOf<android.graphics.Rect>()
            fun collectTicks(node: AccessibilityNodeInfo?) {
                if (node == null) return
                if (node.isVisibleToUser && node.text?.toString()?.matches(Regex("\\d{2}:\\d{2}:\\d{2}(\\.\\d{3})?")) == true) {
                    val rect = android.graphics.Rect().also { node.getBoundsInScreen(it) }
                    var parent = node.parent
                    while (parent != null) {
                        val viewport = android.graphics.Rect().also { parent!!.getBoundsInScreen(it) }
                        assertTrue("Time tick is clipped: $rect outside $viewport", viewport.contains(rect))
                        parent = parent.parent
                    }
                    labels.add(rect)
                }
                for (i in 0 until node.childCount) collectTicks(node.getChild(i))
            }
            automation.clearCache(); collectTicks(automation.rootInActiveWindow)
            assertTrue("Time ticks must be visible at this width and font size", labels.size >= 2)
            labels.forEachIndexed { index, rect -> labels.drop(index + 1).forEach {
                assertFalse("Time tick labels overlap: $rect / $it", android.graphics.Rect.intersects(rect, it))
            } }
            capture("timeline.png")
            click("读取文件"); await("复制结果")
            capture("long-detail.png")
            click("复制结果")
            scenario.onActivity { activity ->
                val detail = activity.history!!.detail!!
                val result = detail.sections.first { it.title == "结果" }
                assertTrue(result.text.length > 100_000)
                assertEquals(result.text, result.chunks.joinToString(""))
                val clipboard = activity.getSystemService(android.content.ClipboardManager::class.java)
                assertEquals(result.text, clipboard.primaryClip!!.getItemAt(0).text.toString())
            }
            click("关闭记录详情")
            click("选择时间轴记录"); click("模型请求"); click("查看时间轴记录详情")
            capture("model-detail.png")
            // Actual system Back dismisses the sheet and retains the reader.
            automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            await("执行历史")
            scenario.onActivity { assertNull(it.history!!.selectedId) }
            val report = JSONObject().put("width_px", instrumentation.targetContext.resources.displayMetrics.widthPixels)
                .put("density", density).put("font_scale", instrumentation.targetContext.resources.configuration.fontScale)
                .put("targets", org.json.JSONArray(targets.map { "${it.width() / density}x${it.height() / density}" }))
            val case = InstrumentationRegistry.getArguments().getString("capture_case") ?: "default"
            instrumentation.targetContext.filesDir.resolve("session-history-complete/$case/geometry.json").writeText(report.toString())
        }
    }
    @Test fun keyboardFocusInputDismissalAndEmptyState() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent()).use { scenario ->
            click("消息输入框")
            scenario.onActivity { assertTrue(it.window.decorView.rootWindowInsets.isVisible(android.view.WindowInsets.Type.ime())) }
            capture("keyboard-entry.png")
            click("阿狸 · 执行历史")
            scenario.onActivity { assertFalse(it.window.decorView.rootWindowInsets.isVisible(android.view.WindowInsets.Type.ime())) }
            var label = ""
            scenario.onActivity { label = "${it.history!!.blocks!!.first { group -> group.grouped }.members.size} 项常规操作" }
            var focused = false
            repeat(12) {
                if (!focused) {
                    instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_TAB)
                    Thread.sleep(80)
                    focused = nodes(label).any { actionNode(it)?.isFocused == true }
                }
            }
            if (!focused) capture("focus-failure.png")
            assertTrue("Group must be reachable with Tab", focused)
            instrumentation.sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_ENTER)
            Thread.sleep(650)
            scenario.onActivity { assertTrue(it.history!!.isExpanded(it.history!!.blocks!!.first { block -> block.grouped })) }
            capture("keyboard-focus.png")
            scenario.onActivity {
                val current = it.history!!
                current.apply(HistoryFrame.decode(JSONObject().put("peer", current.peer).put("session", current.session)
                    .put("entries", org.json.JSONArray()).put("blocks", org.json.JSONArray())
                    .put("loaded", true).put("loading", false).put("total", 0)))
            }
            await("暂无执行记录")
            assertTrue(nodes("选择时间轴记录").isEmpty())
            capture("empty.png")
            scenario.onActivity { it.history!!.status = it.history!!.status.copy(loaded = false, loading = true) }
            Thread.sleep(250)
            assertTrue(nodes("暂无执行记录").isEmpty())
            assertFalse(actionNode(await("刷新执行历史").first())!!.isEnabled)
            capture("loading.png")
        }
    }
    @Test fun memberEntrypointsPagingDetailsRetryAndRevocation() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(Intent(instrumentation.targetContext, SessionHistoryPreviewActivity::class.java)).use { scenario ->
            click("阿狸 · 执行历史")
            await("执行历史")
            scenario.onActivity { assertEquals("session-a", it.history?.session) }
            await("总计 27500 · 输入 24K · 输出 3500")
            capture("history.png")
            click("执行命令")
            await("原始 JSON"); await("/workspace/zork")
            capture("detail.png")
            click("关闭记录详情")
            click("更早记录")
            scenario.onActivity { assertEquals(99_800, it.history?.status?.start) }
            click("返回对话")
            // The last matching member action is the composer bubble, not the header.
            click("小熊 · 执行历史", last = true)
            scenario.onActivity {
                assertEquals(listOf("session-a", "session-b"), it.openedSessions)
                assertNull(it.history?.selectedId)
                it.failed()
            }
            await("测试连接暂时中断"); click("重试加载")
            assertTrue(nodes("测试连接暂时中断").isEmpty())
            click("执行命令")
            scenario.onActivity { it.revoked() }
            await("设备访问权限已撤销")
            assertTrue(nodes("原始 JSON").isEmpty())
            assertTrue(nodes("执行命令").isEmpty())
            capture("revoked.png")
        }
    }

    @Test fun historyScrollingKeepsOnlyVisibleRowsComposed() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(Intent(instrumentation.targetContext, SessionHistoryPreviewActivity::class.java)).use { scenario ->
            click("阿狸 · 执行历史")
            await("执行历史")
            scenario.onActivity { synchronized(it.frameTimes) { it.frameTimes.clear() } }
            fun swipe(up: Boolean) {
                val root = android.graphics.Rect().also { automation.rootInActiveWindow.getBoundsInScreen(it) }
                val x = root.centerX().toFloat()
                val start = root.height() * if (up) .48f else .18f
                val end = root.height() * if (up) .18f else .48f
                val down = SystemClock.uptimeMillis()
                fun event(action: Int, y: Float) { MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, x, y, 0).let {
                    automation.injectInputEvent(it, true); it.recycle()
                } }
                event(MotionEvent.ACTION_DOWN, start)
                for (i in 1..20) { Thread.sleep(12); event(MotionEvent.ACTION_MOVE, start + (end - start) * i / 20f) }
                event(MotionEvent.ACTION_UP, end)
                Thread.sleep(150)
            }
            repeat(4) { swipe(true) }
            repeat(4) { swipe(false) }
            val visible = nodes("执行命令").mapNotNull(::actionNode).distinct().size
            assertTrue("Lazy history must bound visible rows: $visible", visible in 1..20)
            scenario.onActivity { activity ->
                assertEquals(100_000, activity.history?.status?.total)
                assertEquals(100, activity.history?.entries?.size)
                val samples = synchronized(activity.frameTimes) { activity.frameTimes.sorted() }
                assertTrue(samples.isNotEmpty())
                val result = JSONObject().put("loaded_total", 100_000).put("wire_window", 100).put("visible_rows", visible)
                    .put("frames", samples.size).put("layout_draw_p95_ms", samples[((samples.size - 1) * .95).toInt()])
                    .put("layout_draw_p99_ms", samples[((samples.size - 1) * .99).toInt()])
                activity.filesDir.resolve("session-history").apply { mkdirs() }.resolve("scroll.json").writeText(result.toString())
            }
            capture("scroll.png")
        }
    }
}
