package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import android.os.Bundle
import android.view.accessibility.AccessibilityNodeInfo
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.background
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class LiquidControlsTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation

    @Test fun selectionTriggerWidthFollowsItsText() {
        var label by mutableStateOf("短")
        var width = 0
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            scenario.onActivity { activity -> activity.setContent { ZorkTheme {
                Box(Modifier.width(320.dp)) {
                    LiquidSelectTrigger(label, "选择", Modifier.onSizeChanged { width = it.width }, onClick = {})
                }
            } } }
            instrumentation.waitForIdleSync()
            val shortWidth = width
            scenario.onActivity { label = "较长的模型供应商名称" }
            for (attempt in 0 until 40) {
                instrumentation.waitForIdleSync()
                if (width > shortWidth) break
                Thread.sleep(50)
            }
            assertTrue("short trigger filled its parent", shortWidth < 320 * instrumentation.targetContext.resources.displayMetrics.density)
            assertTrue("trigger width did not follow its text: $shortWidth -> $width", width > shortWidth)
        }
    }

    @Test fun businessDismissalRetainsPaintAndCanReverseFromTheOriginalSource() {
        var scene: LiquidSceneHost? = null
        var payload by mutableStateOf<String?>(null)
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            scenario.onActivity { activity -> activity.setContent { ZorkTheme {
                val shared = LocalLiquidHost.current
                SideEffect { scene = shared }
                Column(Modifier.offset(y = 60.dp)) {
                    LiquidButton("真实来源", onClick = { payload = "业务面板" })
                }
                LiquidRetained(payload) { title, open, closed ->
                    SettingsSheet(title, dismiss = { payload = null }, open = open, onClosed = closed) {
                        Box(Modifier.size(80.dp).background(Color.Green))
                        SettingsField("实际输入", "保留这份内容", {})
                        SettingsButton("保存并关闭", primary = true) { payload = null }
                    }
                }
            } } }
            fun idle() {
                val until = System.nanoTime() + 5_000_000_000L
                var stable = 0
                while (System.nanoTime() < until) {
                    Thread.sleep(40); instrumentation.waitForIdleSync()
                    scenario.onActivity { stable = if (scene?.let { it.frames > 0 && !it.moving && !it.hasPending } == true) stable + 1 else 0 }
                    if (stable >= 3) return
                }
                fail("retained overlay did not settle")
            }
            fun greenPixels(): Int {
                val bitmap = requireNotNull(automation.takeScreenshot())
                var count = 0
                for (y in 0 until bitmap.height step 3) for (x in 0 until bitmap.width step 3) {
                    val pixel = bitmap.getPixel(x, y)
                    if (android.graphics.Color.green(pixel) > 150 && android.graphics.Color.red(pixel) < 50 && android.graphics.Color.blue(pixel) < 50) count++
                }
                bitmap.recycle()
                return count
            }
            idle()
            click("真实来源"); waitFor("保存并关闭"); idle()
            assertTrue("open panel lost its real content drawing", greenPixels() > 40)
            var id = 0
            scenario.onActivity {
                val host = requireNotNull(scene)
                val overlay = host.overlays.single { it.open }
                id = overlay.node.id
                assertEquals(LiquidKind.Pair, overlay.node.target?.kind)
                assertTrue(requireNotNull(overlay.source).relocated)
                host.durationScale = 8f
            }
            click("保存并关闭")
            scenario.onActivity {
                assertNull("business dismissal waited for animation", payload)
                val overlay = requireNotNull(scene).overlays.single()
                assertFalse(overlay.open)
                assertTrue(overlay.node.alive)
            }
            assertNull("retired input remained in the active native window", find("实际输入"))
            assertTrue("closing snapshot lost its child layers", greenPixels() > 40)
            click("真实来源"); waitFor("保存并关闭")
            scenario.onActivity {
                val host = requireNotNull(scene)
                assertEquals("reversal replaced the material identity", id, host.overlays.single().node.id)
                host.durationScale = 0f
            }
            idle()
            click("保存并关闭"); idle()
            assertEquals("closed overlay kept painting", 0, greenPixels())
            assertTrue("focus did not return to the unique source", requireNotNull(find("真实来源")).isFocused)
            var frames = 0L
            scenario.onActivity { frames = requireNotNull(scene).frames; assertEquals(0L, requireNotNull(scene).errors) }
            Thread.sleep(400)
            scenario.onActivity { assertEquals("retired overlay kept scheduling", frames, requireNotNull(scene).frames) }
        }
    }

    @Test fun offscreenControlsStopAndResumeAtTheirCurrentTargets() {
        var scene: LiquidSceneHost? = null
        var offscreen by mutableStateOf(false)
        var selected by mutableStateOf("a")
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            scenario.onActivity { activity -> activity.setContent { ZorkTheme {
                val shared = LocalLiquidHost.current
                SideEffect { scene = shared }
                Column(Modifier.offset(y = if (offscreen) 2000.dp else 48.dp)) {
                    LiquidSegments(listOf("a" to "第一项", "b" to "第二项"), selected, { selected = it })
                    LiquidSlider("离屏滑块", if (selected == "a") .1f else .9f, {})
                    LiquidDisclosure("离屏展开区", selected == "b", {}) {
                        LiquidButton("展开内容", onClick = {})
                    }
                }
            } } }
            fun awaitIdle() {
                val deadline = System.nanoTime() + 5_000_000_000L
                do {
                    Thread.sleep(50); instrumentation.waitForIdleSync()
                    var idle = false
                    scenario.onActivity { idle = scene?.let { it.frames > 0 && !it.moving && !it.hasPending } == true }
                    if (idle) return
                } while (System.nanoTime() < deadline)
                fail("offscreen fixture did not settle")
            }
            awaitIdle()
            scenario.onActivity { selected = "b" }
            Thread.sleep(40)
            scenario.onActivity { assertTrue(requireNotNull(scene).moving); offscreen = true }
            awaitIdle()
            var frames = 0L
            scenario.onActivity { frames = requireNotNull(scene).frames; selected = "a" }
            awaitIdle()
            scenario.onActivity { frames = requireNotNull(scene).frames }
            Thread.sleep(600)
            scenario.onActivity { assertEquals("offscreen liquid frames", frames, requireNotNull(scene).frames); offscreen = false }
            awaitIdle()
            scenario.onActivity {
                assertTrue(requireNotNull(scene).frames > frames)
                assertEquals(0L, requireNotNull(scene).errors)
            }
            waitFor("第一项")
            if (!requireNotNull(find("第一项")).isSelected) dump("offscreen-return")
            assertTrue(requireNotNull(find("第一项")).isSelected)
        }
    }

    @Test fun overlaysKeepNativeInputAndReturnToTheirTriggers() {
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            settle(scenario)
            click("浮层与展开"); waitFor("同步范围"); settle(scenario)
            click("同步范围"); waitFor("所有设备"); settle(scenario)
            screenshot("menu")
            click("所有设备"); settle(scenario)
            click("打开对话框"); waitFor("弹窗名称"); settle(scenario)
            val field = requireNotNull(find("弹窗名称"))
            assertTrue(field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, "中文输入与选择")
            }))
            waitFor("中文输入与选择")
            val updated = requireNotNull(find("弹窗名称"))
            assertTrue(updated.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, 2)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, 4)
            }))
            screenshot("dialog")
            click("完成编辑"); settle(scenario)
            waitFor("打开对话框")
            if (!requireNotNull(find("打开对话框")).isFocused) {
                dump("failed-focus")
            }
            assertTrue("focus did not return to dialog trigger", requireNotNull(find("打开对话框")).isFocused)
            click("打开底部面板"); waitFor("面板名称"); settle(scenario)
            screenshot("sheet")
            click("关闭"); settle(scenario); waitFor("打开底部面板")
            assertTrue("focus did not return to sheet row", requireNotNull(find("打开底部面板")).isFocused)
            click("高级选项"); Thread.sleep(40); click("高级选项"); Thread.sleep(40); click("高级选项")
            settle(scenario); waitFor("备注"); screenshot("disclosure")
            scenario.onActivity { assertEquals(0L, requireNotNull(it.host).errors) }
            // Reduced motion must also stop after applying the final target.
            scenario.onActivity { requireNotNull(it.host).durationScale = 0f }
            click("高级选项"); settle(scenario)
            scenario.onActivity { assertFalse(requireNotNull(it.host).moving) }
        }
    }

    @Test fun controlsUseTheSharedSceneAndStopAtRestAndInBackground() {
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            settle(scenario)
            screenshot("controls")
            click("主操作")
            waitFor("已触发 1 次")
            assertNotNull(find("已触发 1 次"))
            click("月视图")
            Thread.sleep(70)
            screenshot("selection-moving")
            settle(scenario)
            click("保留离线副本")
            settle(scenario)
            assertTrue(find("启用同步")!!.isChecked)
            click("全部设备")
            val slider = requireNotNull(find("缩放比例"))
            assertTrue(slider.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SET_PROGRESS.id, Bundle().apply {
                putFloat(AccessibilityNodeInfo.ACTION_ARGUMENT_PROGRESS_VALUE, .85f)
            }))
            settle(scenario)
            screenshot("selected")
            var frames = 0L
            scenario.onActivity { frames = requireNotNull(it.host).frames }
            Thread.sleep(600)
            scenario.onActivity { assertEquals("idle liquid frames", frames, requireNotNull(it.host).frames) }
            click("日视图")
            scenario.moveToState(Lifecycle.State.CREATED)
            instrumentation.waitForIdleSync()
            scenario.onActivity {
                val host = requireNotNull(it.host)
                assertFalse("background host stayed active", host.active)
                frames = host.frames
            }
            Thread.sleep(400)
            scenario.onActivity { assertEquals("background liquid frames", frames, requireNotNull(it.host).frames) }
            scenario.moveToState(Lifecycle.State.RESUMED)
            settle(scenario)
            scenario.onActivity {
                val host = requireNotNull(it.host)
                assertEquals("contour/border errors", 0L, host.errors)
                File(instrumentation.targetContext.getExternalFilesDir(null), "liquid/host.txt").writeText(
                    "frames=${host.frames}\nnodes=${host.nodeCount}\ngeometry=${host.geometryRecords}\nbytes=${host.bytes}\nerrors=${host.errors}\nnative_ns=${host.nativeNanos}\ndecode_ns=${host.decodeNanos}\n")
            }
        }
    }

    private fun settle(scenario: ActivityScenario<LiquidGalleryActivity>) {
        val deadline = System.nanoTime() + 5_000_000_000L
        var previous = -1L
        var stableSince = System.nanoTime()
        do {
            instrumentation.waitForIdleSync()
            var idle = false
            var frames = 0L
            scenario.onActivity {
                idle = it.host?.let { host -> frames = host.frames; host.frames > 0 && !host.moving && !host.hasPending } == true
            }
            if (!idle || frames != previous) stableSince = System.nanoTime()
            previous = frames
            if (idle && System.nanoTime() - stableSince >= 100_000_000L) return
            Thread.sleep(30)
        } while (System.nanoTime() < deadline)
        dump("unsettled")
        fail("liquid did not settle")
    }
    private fun find(label: String): AccessibilityNodeInfo? {
        automation.clearCache()
        val matches = mutableListOf<AccessibilityNodeInfo>()
        fun walk(node: AccessibilityNodeInfo?) {
            if (node == null) return
            if (node.isVisibleToUser && (node.text?.toString() == label || node.contentDescription?.toString() == label)) matches += node
            for (i in 0 until node.childCount) walk(node.getChild(i))
        }
        walk(automation.rootInActiveWindow)
        return matches.firstOrNull { it.isClickable || it.isEditable || it.isSelected || it.isCheckable || it.rangeInfo != null }
            ?: matches.firstNotNullOfOrNull { matched ->
                var parent = matched.parent
                while (parent != null && !parent.isClickable && !parent.isEditable && !parent.isSelected && !parent.isCheckable && parent.rangeInfo == null) parent = parent.parent
                parent
            } ?: matches.firstOrNull()
    }
    private fun click(label: String) {
        waitFor(label)
        var node = requireNotNull(find(label)) { "missing $label" }
        while (!node.isClickable && node.parent != null) node = node.parent
        val clicked = node.performAction(AccessibilityNodeInfo.ACTION_CLICK)
        if (!clicked) dump("failed-click")
        assertTrue("click $label", clicked)
        instrumentation.waitForIdleSync()
    }
    private fun dump(name: String) {
        automation.clearCache()
        val text = StringBuilder()
        fun walk(node: AccessibilityNodeInfo?, depth: Int) {
            if (node == null) return
            text.append(" ".repeat(depth)).append("text=${node.text}, description=${node.contentDescription}, clickable=${node.isClickable}, editable=${node.isEditable}, enabled=${node.isEnabled}, visible=${node.isVisibleToUser}, selected=${node.isSelected}, checked=${node.isChecked}, focused=${node.isFocused}, actions=${node.actionList}\n")
            for (i in 0 until node.childCount) walk(node.getChild(i), depth + 1)
        }
        walk(automation.rootInActiveWindow, 0)
        val folder = File(instrumentation.targetContext.getExternalFilesDir(null), "liquid").apply { mkdirs() }
        File(folder, "$name.txt").writeText(text.toString())
        screenshot(name)
    }
    private fun waitFor(label: String) {
        val until = System.nanoTime() + 2_000_000_000L
        while (find(label) == null && System.nanoTime() < until) { Thread.sleep(20); instrumentation.waitForIdleSync() }
    }
    private fun screenshot(name: String) {
        val folder = File(instrumentation.targetContext.getExternalFilesDir(null), "liquid").apply { mkdirs() }
        val bitmap = requireNotNull(automation.takeScreenshot())
        File(folder, "$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
        bitmap.recycle()
    }
}
