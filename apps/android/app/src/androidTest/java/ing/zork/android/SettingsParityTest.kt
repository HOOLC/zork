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
            settle(); click("手动添加"); field("模型 ID","saved-draft-model")
            assertTrue(editable("模型 ID")!!.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)); settle()
            await("它会思考吗？"); field("上下文","256"); click("按档位调节")
            scenario.recreate(); settle(); await("添加模型")
            assertEquals("256", revealEditable("上下文").text.toString())
            assertEquals("saved-draft-model", revealEditable("模型 ID").text.toString())
            assertNotNull(reveal("删除 medium"))
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
    private fun revealEditable(label: String): AccessibilityNodeInfo {
        repeat(18) { attempt ->
            editable(label)?.takeIf { it.isVisibleToUser }?.let { return it }
            nodes().lastOrNull { it.isScrollable }?.performAction(
                if (attempt < 8) AccessibilityNodeInfo.ACTION_SCROLL_FORWARD else AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD); settle()
        }
        return editable(label) ?: error("Missing editable $label")
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
        // Restore the sheet to its top before locating fields above the footer;
        // fields further down (expanded parameters on a short screen) come after.
        repeat(16) { attempt ->
            val field = editable(label)
            if (field != null) {
                assertTrue(field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, Bundle().apply {
                    putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value)
                })); settle(); assertEquals("Edited the wrong field: $label", value, editable(label)?.text?.toString()); return
            }
            nodes().lastOrNull { it.isScrollable }?.performAction(
                if (attempt < 8) AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD else AccessibilityNodeInfo.ACTION_SCROLL_FORWARD)
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
            click("获取模型"); await("获取到 2 个新模型：1 个已按预设填好，1 个待配置")
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
            settle(); await("工作室订阅"); click("编辑 unconfigured-model"); await("它会思考吗？")
            click("从相似模型填入"); click("fixture-model，工作室订阅，128K · 不思考 · 输出 4,096")
            await("参数来自 fixture-model")
            // Editing keeps the id: it is the title, not a field.
            assertNull(editable("模型 ID")); assertNotNull(find("unconfigured-model"))
            field("最长输出", "1"); click("最长输出 单位 M"); click("保存")
            scenario.onActivity { assertEquals("Invalid form sent a request: ${it.lastBody}", "", it.lastAction) }
            reveal("需小于上下文 128K")
            click("最长输出 单位 K"); field("最长输出", "4")
            // The switch (its label repeats the section summary, so bring the switch itself on screen).
            reveal("打开后，这个模型可以接收对话里的图片；关闭时给它发图会在发送前提醒"); click("能看图片")
            assertNotNull(reveal("不能看图片")); click("保存")
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
            await("已保存 unconfigured-model")
        }
    }

    @Test fun deviceSettingsDoNotExposeRoleOrGrantConfiguration() {
        launch("device").use {
            settle(); await("模型连接"); await("服务")
            assertNull(find("队员")); assertNull(find("领队")); assertNull(find("管理授权"))
        }
    }

    @Test fun modelConnectionsGroupByProviderAndKeepFailedDevicesVisible() {
        launch("home").use { scenario ->
            settle(); await("Mesh"); assertNull(find("工具连接"))
            click("模型连接"); await("OpenAI"); await("Anthropic")
            // mini2 failed: its cached connection and the reason stay visible, never "none".
            assertNotNull(await("OpenRouter"))
            assertNotNull(nodes().firstOrNull { (it.text ?: it.contentDescription)?.toString()?.startsWith("mini2 读不到，显示缓存") == true })
            assertNotNull(find("重试")); assertNull(find("还没有模型连接"))
            assertNull(find("已验证")); assertNotNull(await("待验证")); assertNotNull(await("验证失败"))
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

    /** Models carry their maker's mark, connections their provider's: OpenCode Go serves DeepSeek and GLM. */
    @Test fun modelPickerMarksFollowTheMakerNotTheConnection() {
        assertEquals(R.drawable.maker_deepseek, makerDrawable("deepseek"))
        assertEquals(R.drawable.maker_zhipu, makerDrawable("zhipu"))
        assertEquals(R.drawable.maker_generic, makerDrawable(null))
        assertEquals(R.drawable.maker_generic, makerDrawable("someone-new"))
        assertEquals(R.drawable.provider_opencode, providerDrawable("opencode-go"))
        ActivityScenario.launch<Nav7PreviewActivity>(Intent(instrumentation.targetContext, Nav7PreviewActivity::class.java)
            .putExtra("screen", "new-chat").putExtra("width", 0).putExtra("scenario", "picker")).use { scenario ->
            settle(); await("选择模型")
            scenario.onActivity {
                val choice = it.newChatSnapshot.pickerChoice("model")
                assertEquals("deepseek", choice.makers["deepseek-flash"])
                assertEquals("zhipu", choice.makers["glm-5.1"])
                assertNull(choice.makers["muse-spark"])
                assertEquals("opencode-go", choice.providers["opencode"])
                assertEquals("openai", choice.providers["personal"])
            }
            click("选择模型"); reveal("deepseek-flash"); capture("model-picker-makers")
            scenario.onActivity { it.previewTheme = "dark" }; settle(); capture("model-picker-makers-dark")
            scenario.onActivity { it.previewTheme = "light" }; settle()
            click("deepseek-flash")
            // The connection list names OpenCode Go, the only connection serving it.
            click("连接 · 自动"); await("连接 · OpenCode Go"); assertNull(find("连接 · API"))
            click("完成"); settle()
            scenario.onActivity { assertEquals("deepseek-flash", it.newChatSnapshot!!.getJSONObject("model").getString("value")) }
            capture("model-capsule-maker")
        }
        launch("profile").use { settle(); await("fixture-model"); capture("profile-model-makers") }
        launch("profile-custom").use { settle(); await("qwen3-32b"); capture("profile-custom-model-makers") }
    }

    /** Model and thinking are the choice; the connection is optional and automatic by default. */
    @Test fun modelPickerListsEachModelOnceAndPinsAConnectionOptionally() {
        ActivityScenario.launch<Nav7PreviewActivity>(Intent(instrumentation.targetContext, Nav7PreviewActivity::class.java)
            .putExtra("screen", "new-chat").putExtra("width", 0).putExtra("scenario", "picker")).use { scenario ->
            fun selection(): Triple<String, String, String> {
                var result = Triple("", "", "")
                scenario.onActivity { val s = it.newChatSnapshot!!
                    result = Triple(s.getJSONObject("model").getString("value"), s.getJSONObject("thinking").getString("value"), s.getJSONObject("profile").getString("value")) }
                return result
            }
            settle(); click("选择模型"); await("Demo fast")
            // Offered by 个人账号 and API, Demo model is still one row.
            assertEquals(1, nodes().count { it.isVisibleToUser && it.text?.toString() == "Demo model" })
            assertEquals(Triple("Demo model", "high", "auto"), selection())
            click("连接 · 自动"); await("连接 · 个人账号"); await("连接 · API"); assertNull(find("连接 · OpenCode Go"))
            capture("model-picker-connections")
            scenario.onActivity { it.previewTheme = "dark" }; settle(); capture("model-picker-connections-dark")
            scenario.onActivity { it.previewTheme = "light" }; settle()
            click("连接 · API"); settle()
            assertEquals(Triple("Demo model", "high", "api"), selection())
            await("连接 · API"); capture("model-picker-pinned")
            // API does not serve Demo fast: the connection returns to automatic.
            click("Demo fast"); settle()
            assertEquals(Triple("Demo fast", "off", "auto"), selection())
            await("连接 · 自动")
            click("Demo model"); click("连接 · 自动"); click("连接 · API"); click("完成"); settle()
            capture("model-capsule-pinned")
            // The capsule names the pinned connection after model and thinking.
            val states = generateSequence(find("选择模型")) { it.parent }.mapNotNull { it.stateDescription?.toString() }.toList()
            assertTrue(states.toString(), "Demo model · high · API" in states)
        }
    }

    @Test fun failedRenameKeepsTheDraftForRetry() {
        launch("profile").use { scenario ->
            settle(); await("工作室订阅"); click("更多"); click("重命名"); await("重命名连接"); field("名称", "重试后的名称")
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
