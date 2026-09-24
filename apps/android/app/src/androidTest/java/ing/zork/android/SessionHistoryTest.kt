package ing.zork.android

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
            click("读取文件"); await("复制结果")
            capture("reopened-detail.png")
            // Actual system Back dismisses the sheet and retains the reader.
            automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            await("执行历史")
            scenario.onActivity { assertNull(it.history!!.selectedId) }
            val report = JSONObject().put("width_px", instrumentation.targetContext.resources.displayMetrics.widthPixels)
                .put("density", density).put("font_scale", instrumentation.targetContext.resources.configuration.fontScale)
                .put("targets", org.json.JSONArray(targets.map { "${it.width() / density}x${it.height() / density}" }))
            val case = InstrumentationRegistry.getArguments().getString("capture_case") ?: "default"
            instrumentation.targetContext.filesDir.resolve("session-history-complete/$case/geometry.json")
                .apply { parentFile?.mkdirs() }.writeText(report.toString())
        }
    }
    @Test fun receivedChatMessageReadsAsAMessageNotJson() {
        ActivityScenario.launch<SessionHistoryPreviewActivity>(mixedIntent()).use { scenario ->
            click("阿狸 · 执行历史", last = true); await("执行历史")
            scrollToLabel("来自 小熊")
            // The sender is the member name, the body is rendered Markdown and
            // the attachment reads by name; the Station envelope never shows.
            assertTrue(screenContains("窄屏截图见附件"))
            assertFalse(screenContains("**窄屏截图**"))
            assertTrue(screenContains("narrow.png"))
            assertFalse(screenContains("\"source\""))
            assertFalse(screenContains("worker-1"))
            scenario.onActivity { activity ->
                val row = activity.history!!.entries.first { it.id == "wake-input" }
                assertEquals("小熊", row.message!!.sender)
                assertEquals(listOf("narrow.png"), row.message!!.files)
            }
            capture("received-message.png")
            // The Markdown body stays selectable; the sender line opens the record.
            val header = android.graphics.Rect().also { rect ->
                automation.clearCache()
                fun find(node: AccessibilityNodeInfo?): AccessibilityNodeInfo? {
                    if (node == null) return null
                    if (node.isVisibleToUser && node.text?.startsWith("来自 小熊") == true) return node
                    return (0 until node.childCount).firstNotNullOfOrNull { find(node.getChild(it)) }
                }
                find(automation.rootInActiveWindow)!!.getBoundsInScreen(rect)
            }
            val down = SystemClock.uptimeMillis()
            for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
                MotionEvent.obtain(down, SystemClock.uptimeMillis(), action, header.exactCenterX(), header.exactCenterY(), 0).let {
                    assertTrue(automation.injectInputEvent(it, true)); it.recycle()
                }
                Thread.sleep(40)
            }
            instrumentation.waitForIdleSync(); Thread.sleep(650)
            await("附件")
            scenario.onActivity { activity ->
                val detail = activity.history!!.detail!!
                assertTrue(detail.sections.first { it.title == "内容" }.text.startsWith("检查完成，继续整理报告。"))
                assertEquals("narrow.png", detail.sections.first { it.title == "附件" }.text)
                // The envelope is only in the raw event section.
                assertTrue(detail.sections.any { it.code && it.text.contains("\\\"source\\\":\\\"chat\\\"") })
            }
            capture("received-message-detail.png")
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
            // The header's one line reads "model · total tokens".
            val tokens = "总计 27500 · 输入 24K · 输出 3500"
            val tokensBy = SystemClock.uptimeMillis() + 6000
            while (!screenContains(tokens) && SystemClock.uptimeMillis() < tokensBy) Thread.sleep(40)
            assertTrue("Missing $tokens in the header", screenContains(tokens))
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
