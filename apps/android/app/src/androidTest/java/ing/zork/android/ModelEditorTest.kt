package ing.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The model editor sheet end to end on the settings fixture. Rules come from
 * core's model editor; these tests check the sheet renders and forwards them.
 */
@RunWith(AndroidJUnit4::class)
class ModelEditorTest {
    private val ui = SettingsHarness("model-editor")

    private fun openAdd(page: String = "profile") = ui.launch(page).also {
        ui.settle(); ui.click("手动添加"); ui.await("添加模型")
    }

    /** Reading order of two chips in a wrapping row. */
    private fun before(a: String, b: String): Boolean {
        val x = ui.bounds(a); val y = ui.bounds(b)
        return x.centerY() < y.top || (x.centerY() in y.top..y.bottom && x.left < y.left)
    }

    private fun saved(scenario: androidx.test.core.app.ActivityScenario<Nav7PreviewActivity>): JSONObject {
        var input: JSONObject? = null
        scenario.onActivity {
            assertEquals("save_model", it.lastAction)
            input = it.lastBody!!.getJSONObject("input")
        }
        return input!!
    }

    @Test fun suggestionPickFillsFromPresetAndOneSectionExpands() {
        openAdd().use { scenario ->
            ui.focus("模型 ID")
            ui.type("模型 ID", "o3")
            ui.awaitPrefix("o3-pro，o3 Pro")
            ui.capture("01-suggestions")
            ui.clickPrefix("o3-pro，")
            assertEquals("o3-pro", ui.text("模型 ID"))
            ui.await("参数已按预设填好"); ui.await("换一个来源")
            ui.awaitPrefix("思考：档位")
            ui.awaitPrefix("长度：上下文 200K · 最长输出 100K")
            ui.awaitPrefix("图片：")
            // Suggestions close after a pick.
            assertNull(ui.findPrefix("o3，"))
            ui.capture("02-recognized")
            ui.clickPrefix("长度：")
            ui.await("上下文 单位 K")
            // Only that section opened.
            assertNull(ui.find("按档位调节"))
            ui.capture("03-length-open")
            // Modify, then restore: the row stays open.
            ui.click("最长输出 64K")
            ui.awaitPrefix("预设：上下文 200K · 最长输出 100K")
            ui.capture("04-modified")
            ui.click("恢复长度")
            ui.awaitGone("恢复长度")
            assertNotNull(ui.find("上下文 单位 K"))
            assertEquals("100", ui.text("最长输出"))
            ui.click("保存模型")
            val input = saved(scenario)
            assertEquals("o3-pro", input.getString("id"))
            assertEquals("200K", input.getString("context"))
            assertEquals("100K", input.getString("output"))
            assertEquals("levels", input.getJSONObject("thinking_scheme").getString("kind"))
            ui.await("已添加 o3-pro")
            ui.capture("05-added-toast")
            scenario.onActivity {
                val model = it.lastProfile!!.getJSONArray("models").objects().first { m -> m.text("id") == "o3-pro" }
                assertTrue(model.optBoolean("enabled", true))
            }
            ui.await("编辑 o3-pro")
        }
    }

    @Test fun changingTheIdRefillsUntilTheUserEditsThenOffersRefill() {
        openAdd().use {
            ui.type("模型 ID", "o3"); ui.imeDone("模型 ID")
            ui.awaitPrefix("长度：上下文 200K · 最长输出 100K")
            // Untouched preset: a new id re-fills.
            ui.type("模型 ID", "gpt-4o")
            ui.awaitPrefix("长度：上下文 128K · 最长输出 16,384")
            ui.awaitPrefix("思考：不思考")
            // Edited: a new id no longer overwrites and offers a refill.
            ui.clickPrefix("长度："); ui.click("最长输出 8K")
            ui.type("模型 ID", "o3")
            ui.await("ID 对应 o3，你改过参数所以没有覆盖")
            ui.awaitPrefix("长度：上下文 128K · 最长输出 8K")
            ui.capture("06-refill-offer")
            ui.click("按 o3 预设重新填入")
            ui.awaitPrefix("长度：上下文 200K · 最长输出 100K")
            assertNull(ui.find("ID 对应 o3，你改过参数所以没有覆盖"))
        }
    }

    @Test fun variantIdSaysWhichPresetItFollows() {
        openAdd().use {
            ui.type("模型 ID", "gpt-5-2025-08-07"); ui.imeDone("模型 ID")
            ui.await("按 GPT-5 预设（gpt-5-2025-08-07 是它的变体）")
            ui.capture("07-variant")
        }
    }

    @Test fun unknownIdWaitsWhileTypingThenAsksNumberedQuestions() {
        openAdd().use { scenario ->
            ui.focus("模型 ID")
            ui.type("模型 ID", "my-model")
            ui.await("把 “my-model” 当作自定义模型")
            Thread.sleep(400) // past the recognition debounce
            assertNull(ui.find("没有这个模型的预设"))
            assertNull(ui.find("它会思考吗？"))
            ui.capture("08-typing-unknown")
            ui.imeDone("模型 ID")
            ui.await("没有这个模型的预设"); ui.await("从相似模型填入")
            ui.await("它会思考吗？"); ui.reveal("上下文和最长输出"); ui.reveal("能看图片吗？")
            assertNull(ui.find("接口协议"))
            ui.capture("09-numbered")
            ui.click("保存模型")
            scenario.onActivity { assertEquals("Invalid form sent a request: ${it.lastBody}", "", it.lastAction) }
            ui.reveal("选一种思考方式"); ui.reveal("填写上下文长度"); ui.reveal("填写最长输出")
            // What was typed is kept.
            assertEquals("my-model", ui.text("模型 ID"))
            ui.capture("10-save-errors")
            ui.click("按档位调节")
            ui.await("删除 low"); ui.await("删除 high")
            // Deleting the default moves it to the neighbour.
            ui.click("删除 medium"); ui.awaitGone("删除 medium")
            // Common names go to their canonical place.
            ui.click("+ 档位"); ui.click("添加档位 minimal")
            assertTrue(before("删除 minimal", "删除 low"))
            // Custom names: errors inline, valid ones append.
            ui.click("+ 档位"); ui.type("新档位名称", "bad name")
            ui.imeDone("新档位名称")
            ui.await("只用字母、数字、.、- 和 _（供应商接口里的原名）")
            ui.type("新档位名称", "turbo"); ui.imeDone("新档位名称")
            ui.await("删除 turbo")
            assertNull(ui.editable("新档位名称"))
            ui.capture("11-levels")
            // Long-press drag: turbo moves in front of low.
            // Drag by the chip's name, just left of its × (labels also appear in the panel preview).
            val shift = -(24 * ui.instrumentation.targetContext.resources.displayMetrics.density).toInt()
            ui.longPressDrag("删除 turbo", "删除 low", shift)
            ui.capture("11b-dragged")
            assertTrue("drag did not reorder", before("删除 turbo", "删除 low"))
            ui.type("上下文", "128"); ui.type("最长输出", "8")
            ui.click("保存模型")
            val input = saved(scenario)
            assertEquals("my-model", input.getString("id"))
            assertEquals("128K", input.getString("context")); assertEquals("8K", input.getString("output"))
            val scheme = input.getJSONObject("thinking_scheme")
            val values = scheme.getJSONArray("values").let { a -> (0 until a.length()).map { a.getString(it) } }
            assertEquals(listOf("minimal", "turbo", "low", "high"), values)
            assertEquals("high", scheme.getString("default"))
        }
    }

    @Test fun budgetChecksAgainstOutputAndMovesTheDefault() {
        openAdd().use { scenario ->
            ui.type("模型 ID", "my-budget"); ui.imeDone("模型 ID")
            ui.click("按 token 预算")
            ui.await("删除 16K")
            ui.type("最长输出", "8")
            ui.reveal("预算需小于最长输出 8K")
            ui.click("+ 预算")
            ui.type("新预算", "100K"); ui.imeDone("新预算"); ui.await("需小于最长输出 8K")
            ui.type("新预算", "abc"); ui.imeDone("新预算"); ui.await("写成 8K 或 8192 这样的数字")
            ui.type("新预算", "4K"); ui.imeDone("新预算"); ui.await("已经有 4K")
            ui.type("新预算", "2K"); ui.imeDone("新预算"); ui.await("删除 2K")
            ui.capture("12-budget-errors")
            ui.click("删除 16K"); ui.click("删除 32K")
            ui.awaitGone("预算需小于最长输出 8K")
            ui.click("默认预算 4K")
            ui.assertOn("默认预算 4K")
            // Deleting the default preset moves the default to the first option.
            ui.click("删除 4K")
            ui.assertOn("默认预算 关")
            // Turning off the switch behind the default moves it too.
            ui.click("可以关闭思考")
            ui.awaitGone("默认预算 关")
            ui.assertOn("默认预算 2K")
            ui.click("可以让模型自己决定")
            ui.reveal("默认预算 自动")
            ui.capture("13-budget")
            ui.type("上下文", "128")
            ui.click("保存模型")
            val scheme = saved(scenario).getJSONObject("thinking_scheme")
            assertEquals("budget", scheme.getString("kind"))
            assertEquals("[2000]", scheme.getJSONArray("presets").toString())
            assertFalse(scheme.getBoolean("allow_off")); assertTrue(scheme.getBoolean("dynamic"))
        }
    }

    @Test fun lengthUnitsQuickPicksAndConflicts() {
        openAdd().use {
            ui.type("模型 ID", "my-lengths"); ui.imeDone("模型 ID")
            ui.type("上下文", "1.5"); ui.click("上下文 单位 M")
            ui.await("1,500,000")
            ui.type("最长输出", "2")
            ui.click("最长输出 单位 M")
            ui.reveal("需小于上下文 1.5M")
            ui.capture("14-length-conflict")
            ui.click("上下文 128K")
            assertEquals("128", ui.text("上下文"))
            // Output picks at or above the context are disabled.
            assertFalse(ui.isEnabled("最长输出 128K"))
            assertTrue(ui.isEnabled("最长输出 64K"))
            ui.click("最长输出 单位 K")
            ui.awaitGone("需小于上下文 128K")
            // An unparsable number shows its error once the field is left.
            ui.focus("上下文"); ui.type("上下文", "12.3456")
            assertNull(ui.find("写成 128K 或 131072 这样的数字"))
            ui.focus("最长输出")
            ui.reveal("写成 128K 或 131072 这样的数字")
            ui.capture("15-length-invalid")
        }
    }

    @Test fun duplicateIdOffersToEditTheExistingModel() {
        openAdd().use {
            ui.type("模型 ID", "fixture-model")
            ui.await("这个连接里已经有 fixture-model")
            assertNull(ui.findPrefix("思考："))
            ui.capture("16-duplicate")
            ui.click("去编辑它")
            ui.await("移除")
            assertNotNull(ui.find("fixture-model"))
            assertNull(ui.find("添加模型"))
            ui.capture("17-edit-existing")
        }
    }

    @Test fun fillFromSimilarAndProtocolOnCustomConnection() {
        openAdd("profile-custom").use { scenario ->
            ui.type("模型 ID", "my-qwen-awq"); ui.imeDone("模型 ID")
            ui.reveal("接口协议")
            ui.click("从相似模型填入")
            ui.awaitPrefix("fixture-model，工作室订阅")
            ui.capture("18-fill-sources")
            ui.type("搜索来源", "fixture")
            assertNull(ui.findPrefix("qwen3-32b，"))
            ui.clickPrefix("fixture-model，")
            ui.await("参数来自 fixture-model")
            ui.awaitPrefix("长度：上下文 128K · 最长输出 4,096")
            // Thinking, length and image open after a fill; protocol stays as it was.
            ui.reveal("按档位调节")
            ui.clickPrefix("协议：OpenAI Chat Completions · 跟随连接")
            ui.click("Anthropic Messages")
            ui.reveal("这个连接默认用 OpenAI Chat Completions，确认供应商支持再改")
            ui.capture("19-protocol")
            ui.click("保存模型")
            val input = saved(scenario)
            assertEquals("anthropic-messages", input.getString("api"))
            assertEquals("128K", input.getString("context"))
        }
    }

    @Test fun editRemovesAndUnconfiguredOpensTheQuestions() {
        ui.launch("profile").use { scenario ->
            ui.settle()
            ui.reveal("待配置")
            assertFalse(ui.isEnabled("模型启用 unconfigured-model"))
            ui.capture("20-list")
            ui.click("编辑 unconfigured-model")
            ui.await("它会思考吗？")
            ui.capture("21-edit-unconfigured")
            ui.click("取消"); ui.awaitGone("它会思考吗？")
            ui.click("编辑 fixture-model")
            ui.await("移除"); ui.await("这个模型没有预设")
            ui.capture("22-edit")
            ui.click("移除")
            scenario.onActivity { assertEquals("remove_model", it.lastAction) }
            ui.await("已移除 fixture-model")
            ui.awaitGone("编辑 fixture-model")
        }
    }

    @Test fun fetchFillsPresetsAndOffersRemovedModelsAgain() {
        ui.launch("profile").use { scenario ->
            ui.settle(); ui.click("获取模型")
            ui.await("获取到 2 个新模型：1 个已按预设填好，1 个待配置")
            ui.await("200K · 档位 · 按预设")
            // Recognized but off until turned on; the unknown one cannot be turned on yet.
            assertTrue(ui.isEnabled("模型启用 o3")); assertFalse(ui.isEnabled("模型启用 vendor-x-preview"))
            ui.capture("23-fetched")
            ui.click("模型启用 o3")
            scenario.onActivity { assertEquals("enable_model", it.lastAction); assertTrue(it.lastBody!!.getBoolean("enabled")) }
            // A reported model the user removed comes back as a suggestion.
            ui.click("编辑 o3"); ui.await("与预设一致"); ui.click("移除"); ui.await("已移除 o3")
            ui.click("手动添加"); ui.focus("模型 ID")
            ui.await("工作室订阅 上可用")
            ui.awaitPrefix("o3，o3")
            ui.capture("24-reported-suggestion")
        }
    }

    @Test fun cancelAndReopenStartsClean() {
        openAdd().use {
            ui.type("模型 ID", "o3"); ui.imeDone("模型 ID"); ui.await("参数已按预设填好")
            ui.click("取消"); ui.awaitGone("参数已按预设填好")
            ui.click("手动添加"); ui.await("添加模型")
            assertEquals("", ui.editable("模型 ID")?.text?.toString().orEmpty())
            assertNull(ui.find("参数已按预设填好"))
            assertNull(ui.findPrefix("思考："))
        }
    }

    @Test fun backClosesSuggestionsBeforeTheSheet() {
        openAdd().use {
            ui.focus("模型 ID")
            ui.type("模型 ID", "o3"); ui.awaitPrefix("o3-pro，")
            ui.back()
            // The keyboard may take the first back.
            if (ui.findPrefix("o3-pro，") != null) ui.back()
            assertNull(ui.findPrefix("o3-pro，"))
            ui.await("添加模型")
            ui.back()
            ui.awaitGone("添加模型")
        }
    }

    @Test fun darkModeStates() {
        openAdd().use { scenario ->
            scenario.onActivity { it.previewTheme = "dark" }
            ui.settle()
            ui.type("模型 ID", "o3"); ui.imeDone("模型 ID")
            ui.clickPrefix("思考：")
            ui.capture("30-dark-thinking")
            ui.clickPrefix("长度：")
            ui.click("最长输出 64K")
            ui.capture("31-dark-modified")
            ui.type("模型 ID", "my-dark"); ui.imeDone("模型 ID")
            ui.click("按 token 预算")
            ui.capture("32-dark-numbered")
        }
    }

    @Test fun veryLongIdStaysOnOneLine() {
        openAdd().use {
            val long = "vendor-" + "x".repeat(150)
            ui.type("模型 ID", long); ui.imeDone("模型 ID")
            ui.await("没有这个模型的预设")
            ui.capture("33-long-id")
        }
    }
}
