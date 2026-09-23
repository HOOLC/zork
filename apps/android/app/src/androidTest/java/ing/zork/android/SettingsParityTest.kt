package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import android.os.Bundle
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SettingsParityTest {
    @Test fun clearingDataRequiresConfirmationAndCancellationKeepsData() {
        launch("home").use { scenario ->
            settle(); click("清空数据"); await("清空客户端数据？")
            scenario.onActivity { assertEquals("", it.lastAction) }
            click("取消")
            scenario.onActivity { assertEquals("", it.lastAction) }
            click("清空数据"); await("确认清空")
            capture("clear-data-confirmation")
            click("确认清空")
            scenario.onActivity { assertEquals("clear-data", it.lastAction) }
        }
    }
    @Test fun resourceSettingsExposeServiceLogs() {
        launch("services").use {
            settle(); click("设计预览"); click("stdout.log"); await("preview server ready")
            capture("service-log")
        }
    }
    @Test fun modelDraftSurvivesActivityRecreation() {
        launch("profile").use { scenario ->
            settle(); click("手动添加"); field("模型 ID","saved-draft-model"); field("上下文 token 上限","256K")
            scenario.recreate(); settle(); await("添加模型")
            assertEquals("saved-draft-model", editable("模型 ID")!!.text.toString())
            assertEquals("256K", editable("上下文 token 上限")!!.text.toString())
            capture("model-recreated")
        }
    }
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.uiAutomation
    private fun settle() { instrumentation.waitForIdleSync(); Thread.sleep(250) }
    private fun nodes(): List<AccessibilityNodeInfo> {
        automation.clearCache()
        val result = mutableListOf<AccessibilityNodeInfo>()
        fun walk(node: AccessibilityNodeInfo?) {
            if (node == null) return
            result += node
            for (i in 0 until node.childCount) walk(node.getChild(i))
        }
        walk(automation.rootInActiveWindow)
        return result
    }
    private fun find(label: String) = nodes().filter { it.isVisibleToUser &&
        (it.contentDescription?.toString() == label || it.text?.toString() == label) }.let { matches ->
        matches.firstOrNull { it.isClickable } ?: matches.firstOrNull { it.contentDescription?.toString() == label } ?: matches.firstOrNull()
    }
    private fun descendants(node: AccessibilityNodeInfo): List<AccessibilityNodeInfo> =
        listOf(node) + (0 until node.childCount).flatMap { node.getChild(it)?.let(::descendants).orEmpty() }
    private fun editable(label: String): AccessibilityNodeInfo? {
        for (node in nodes().filter { it.contentDescription?.toString() == label }) {
            var parent: AccessibilityNodeInfo? = node
            while (parent != null) {
                if (parent.isEditable) return parent
                parent = parent.parent
            }
        }
        return null
    }
    private fun await(label: String): AccessibilityNodeInfo {
        val deadline = SystemClock.uptimeMillis() + 5000
        while (SystemClock.uptimeMillis() < deadline) { find(label)?.let { return it }; Thread.sleep(80) }
        error("Missing $label")
    }
    private fun reveal(label: String): AccessibilityNodeInfo {
        repeat(9) {
            find(label)?.let { return it }
            nodes().lastOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
            settle()
        }
        return await(label)
    }
    private fun click(label: String) {
        reveal(label)
        val deadline = SystemClock.uptimeMillis() + 5000
        while (SystemClock.uptimeMillis() < deadline) {
            var node = find(label)
            while (node != null) {
                if (node.isClickable) {
                    if (node.isEnabled) {
                        assertTrue("Cannot click $label", node.performAction(AccessibilityNodeInfo.ACTION_CLICK))
                        settle(); return
                    }
                    break
                }
                node = node.parent
            }
            settle()
        }
        error("Cannot click $label")
    }
    private fun field(label: String, value: String) {
        // Restore the sheet to its top before locating fields above the footer.
        repeat(8) {
            val field = editable(label)
            if (field != null) {
                assertTrue(field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                    putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value)
                })); settle(); assertEquals("Edited the wrong field: $label", value, editable(label)?.text?.toString()); return
            }
            nodes().lastOrNull { it.isScrollable }?.performAction(AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD)
            settle()
        }
        error("Missing editable $label")
    }
    private fun launch(page: String) = ActivityScenario.launch<Nav7PreviewActivity>(
        Intent(instrumentation.targetContext, Nav7PreviewActivity::class.java).putExtra("screen", page).putExtra("width", 0))
    private fun capture(name: String) {
        val folder = instrumentation.targetContext.filesDir.resolve("settings-parity").apply { mkdirs() }
        automation.takeScreenshot()?.let { bitmap ->
            folder.resolve("$name.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle()
        }
    }

    @Test fun appearanceSavesHeightAndCanRestoreAutomatic() {
        launch("home").use { scenario ->
            settle(); await("外观"); click("外观")
            val handle = await("拖动调整消息高度")
            assertTrue(handle.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_SET_PROGRESS.id, Bundle().apply {
                putFloat(AccessibilityNodeInfo.ACTION_ARGUMENT_PROGRESS_VALUE, 360f)
            })); settle()
            scenario.onActivity { assertEquals(360, it.previewHeight) }
            capture("appearance-360")
            click("恢复自动"); scenario.onActivity { assertEquals(0, it.previewHeight) }
            val bounds = android.graphics.Rect().also { await("拖动调整消息高度").getBoundsInScreen(it) }
            var density = 1f
            scenario.onActivity { density = it.resources.displayMetrics.density }
            val down = SystemClock.uptimeMillis()
            for (step in 0..16) {
                val action = when (step) { 0 -> android.view.MotionEvent.ACTION_DOWN; 16 -> android.view.MotionEvent.ACTION_UP; else -> android.view.MotionEvent.ACTION_MOVE }
                val event = android.view.MotionEvent.obtain(down, SystemClock.uptimeMillis(), action,
                    bounds.exactCenterX(), bounds.exactCenterY() + 72f * density * step / 16f, 0)
                assertTrue(automation.injectInputEvent(event, true)); event.recycle(); Thread.sleep(16)
            }
            settle(); scenario.onActivity { assertTrue("Drag did not resize continuously: ${it.previewHeight}", it.previewHeight in 240..272) }
        }
    }

    @Test fun connectionNameQuotaAndModelControlsUseCurrentEndpoints() {
        launch("profile").use { scenario ->
            settle(); await("工作室订阅"); assertNotNull(await("剩余 72%"))
            assertNotNull(await("余额 0.00 USD"))
            click("模型启用 fixture-model")
            scenario.onActivity {
                assertEquals("enable_model", it.lastAction)
                assertFalse(it.lastBody!!.getBoolean("enabled"))
            }
            assertNotNull(reveal("待配置上下文与输出上限"))
            click("更新模型"); await("模型已是最新")
            scenario.onActivity { assertEquals("discover_models", it.lastAction) }
            click("刷新额度")
            scenario.onActivity { assertEquals("refresh_quota", it.lastAction) }
            click("重命名连接"); field("名称", "手机可见的工作室账号"); click("保存")
            await("手机可见的工作室账号")
            scenario.onActivity {
                assertEquals("rename_profile", it.lastAction)
                assertEquals("手机可见的工作室账号", it.lastBody!!.getString("name"))
            }
            capture("connection")
        }
    }

    @Test fun copyingModelSettingsPreservesIdentityAndEnforcesLimits() {
        launch("profile").use { scenario ->
            settle(); await("工作室订阅"); click("unconfigured-model")
            click("复制已有模型配置"); click("工作室订阅 · fixture-model")
            val idField = editable("模型 ID") ?: error(nodes().joinToString("\n") { "${it.className} text=${it.text} description=${it.contentDescription} editable=${it.isEditable} children=${it.childCount}" })
            assertEquals("unconfigured-model", idField.text.toString())
            field("输出 token 上限", "1M"); click("保存模型")
            scenario.onActivity { assertEquals("Invalid form sent a request: ${it.lastBody}", "", it.lastAction) }
            field("输出 token 上限", "4K")
            click("可读取图片"); click("保存模型")
            scenario.onActivity {
                assertEquals("save_model", it.lastAction)
                val input = it.lastBody!!.getJSONObject("input")
                assertEquals("unconfigured-model", input.getString("id"))
                val models = it.lastProfile!!.getJSONArray("models")
                assertEquals(2, models.length())
                assertEquals("fixture-model", models.getJSONObject(0).getString("id"))
                val target = models.getJSONObject(1)
                assertEquals("unconfigured-model", target.getString("id"))
                assertFalse(target.getBoolean("enabled")); assertFalse(target.getBoolean("default"))
                assertEquals(128000L, target.getJSONObject("limits").getLong("context_window_tokens"))
                assertEquals(4000L, target.getJSONObject("limits").getLong("max_output_tokens"))
                assertEquals("[\"text\"]", target.getJSONObject("capabilities").getJSONArray("input").toString())
            }
            await("工作室订阅")
        }
    }

    @Test fun deviceSettingsDoNotExposeRoleOrGrantConfiguration() {
        launch("device").use {
            settle(); await("大模型")
            assertNull(find("队员")); assertNull(find("领队")); assertNull(find("管理授权"))
        }
    }
    @Test fun newChatUsesCoreChoicesAndSubmitsOnlyAfterSending() {
        launch("new-chat").use { scenario ->
            settle(); await("新建 Chat"); await("Demo model")
            scenario.onActivity { assertFalse(it.newChatSnapshot!!.optBoolean("busy"));assertFalse(it.newChatSnapshot!!.optBoolean("can_submit")) }
            click("Demo model"); click("Demo fast"); capture("new-chat-after-model")
            scenario.onActivity { assertEquals(it.newChatSnapshot.toString(),"off",it.newChatSnapshot!!.getJSONObject("thinking").getString("value")) }
            reveal("off")
            val input=nodes().first { it.isEditable }
            assertTrue(input.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE,"新建一个 Chat 🦊")}))
            settle(); capture("new-chat-input");click("发送")
            scenario.onActivity {
                assertEquals("submit",it.lastAction)
                assertTrue(it.newChatSnapshot!!.optBoolean("busy"))
                assertFalse(it.newChatSnapshot!!.optBoolean("can_submit"))
                assertEquals("Demo fast",it.newChatSnapshot!!.getJSONObject("model").getString("value"))
            }
        }
    }

    @Test fun failedRenameKeepsTheDraftForRetry() {
        launch("profile").use { scenario ->
            settle(); await("工作室订阅"); click("重命名连接"); field("名称", "重试后的名称")
            scenario.onActivity { it.failNextRequest = true }
            click("保存"); await("fixture request failed")
            assertEquals("重试后的名称", editable("名称")!!.text.toString())
            click("保存"); await("重试后的名称")
        }
    }

    @Test fun modelFooterRemainsVisibleAboveKeyboard() {
        launch("profile").use { scenario ->
            settle(); await("工作室订阅"); click("手动添加")
            assertTrue(editable("模型 ID")!!.performAction(AccessibilityNodeInfo.ACTION_CLICK))
            scenario.onActivity { it.window.insetsController!!.show(android.view.WindowInsets.Type.ime()) }
            var ime = 0; var height = 0
            val deadline = SystemClock.uptimeMillis() + 5000
            while (ime == 0 && SystemClock.uptimeMillis() < deadline) {
                settle(); scenario.onActivity {
                    ime = it.window.decorView.rootWindowInsets.getInsets(android.view.WindowInsets.Type.ime()).bottom
                    height = it.window.decorView.height
                }
            }
            assertTrue("Keyboard did not open", ime > 0)
            val bounds = android.graphics.Rect().also { await("保存模型").getBoundsInScreen(it) }
            assertTrue("Save action hidden behind keyboard", bounds.bottom <= height - ime)
            assertNotNull(await("关闭")); capture("model-keyboard")
            scenario.onActivity { it.window.insetsController!!.hide(android.view.WindowInsets.Type.ime()) }
        }
    }
}
