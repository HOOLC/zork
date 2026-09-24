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
            settle(); click("清空本机数据"); await("清空这台手机上的 Zork 数据？")
            scenario.onActivity { assertEquals("", it.lastAction) }
            click("取消")
            scenario.onActivity { assertEquals("", it.lastAction) }
            click("清空本机数据"); await("清空并退出")
            capture("clear-data-confirmation")
            click("清空并退出")
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
            settle(); click("手动添加"); field("模型 ID","saved-draft-model"); click("调整参数"); field("上下文","256K")
            scenario.recreate(); settle(); await("添加模型")
            assertEquals("saved-draft-model", editable("模型 ID")!!.text.toString())
            assertEquals("256K", editable("上下文")!!.text.toString())
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

    @Test fun appearanceChoosesThemeAndReturnsToSystem() {
        launch("home").use { scenario ->
            settle(); await("外观"); click("外观"); await("跟随系统")
            click("深色"); settle()
            scenario.onActivity { assertEquals("dark", it.previewTheme) }
            capture("appearance-dark")
            click("跟随系统"); settle()
            scenario.onActivity { assertEquals("system", it.previewTheme) }
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
            assertNotNull(reveal("待配置"))
            click("获取模型"); await("模型已是最新")
            scenario.onActivity { assertEquals("discover_models", it.lastAction) }
            click("更多"); click("刷新额度")
            scenario.onActivity { assertEquals("refresh_quota", it.lastAction) }
            click("更多"); click("重命名"); field("名称", "手机可见的工作室账号"); click("保存")
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
            settle(); await("工作室订阅"); click("unconfigured-model"); click("调整参数")
            click("复制已有模型配置"); click("工作室订阅 · fixture-model")
            val idField = editable("模型 ID") ?: error(nodes().joinToString("\n") { "${it.className} text=${it.text} description=${it.contentDescription} editable=${it.isEditable} children=${it.childCount}" })
            assertEquals("unconfigured-model", idField.text.toString())
            field("最长输出", "1M"); click("保存模型")
            scenario.onActivity { assertEquals("Invalid form sent a request: ${it.lastBody}", "", it.lastAction) }
            field("最长输出", "4K")
            click("读取图片"); click("保存模型")
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
            settle(); await("模型连接"); await("这台设备")
            assertNull(find("队员")); assertNull(find("领队")); assertNull(find("管理授权"))
        }
    }

    @Test fun modelConnectionsGroupByProviderAndKeepFailedDevicesVisible() {
        launch("home").use { scenario ->
            settle(); await("Mesh"); assertNull(find("工具连接"))
            click("模型连接"); await("OpenAI"); await("Anthropic")
            // mini2 failed: its cached connection and the reason stay visible, never "none".
            assertNotNull(await("OpenRouter"))
            assertNotNull(nodes().firstOrNull { it.text?.toString()?.startsWith("无法读取 mini2 上的连接") == true })
            assertNotNull(find("重试")); assertNull(find("还没有模型连接"))
            assertNotNull(await("已验证")); assertNotNull(await("待验证")); assertNotNull(await("验证失败"))
            capture("model-connections")
            click("工作室订阅，订阅，mini1")
            scenario.onActivity { assertEquals("open-connection:studio", it.lastAction) }
            await("额度")
            click("返回"); await("OpenRouter")
            click("添加连接"); await("添加到哪台设备？"); click("mini1")
            scenario.onActivity { assertEquals("add-connection:mini1", it.lastAction) }
        }
    }
    @Test fun newChatUsesCoreChoicesAndSubmitsOnlyAfterSending() {
        launch("new-chat").use { scenario ->
            settle(); await("新建 Chat"); await("选择模型")
            scenario.onActivity { assertFalse(it.newChatSnapshot!!.optBoolean("busy"));assertFalse(it.newChatSnapshot!!.optBoolean("can_submit")) }
            click("选择模型"); click("Demo fast"); capture("new-chat-after-model")
            scenario.onActivity { assertEquals(it.newChatSnapshot.toString(),"off",it.newChatSnapshot!!.getJSONObject("thinking").getString("value")) }
            click("完成"); settle()
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
