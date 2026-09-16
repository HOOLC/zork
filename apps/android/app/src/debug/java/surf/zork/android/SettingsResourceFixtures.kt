package surf.zork.android

import org.json.JSONObject

internal fun settingsFixturePage(state: MobileSettingsState, page: String): MobileSettingsState {
    val selection = when (page) {
        "connections" -> ResourceSelection(null, "mcp", title = "工具连接")
        "services" -> ResourceSelection(state.device?.id, "service", title = "服务")
        "skills" -> ResourceSelection(state.device?.id, "skill", JSONObject().put("agent_skills", "designer").toString(), "技能")
        else -> null
    }
    return state.copy(page = page, resource = selection, resourceData = selection?.let(::settingsResourceFixture))
}

/** Explicit in-memory payloads for production settings composables. */
internal fun settingsResourceFixture(selection: ResourceSelection): JSONObject {
    val data = JSONObject("""{"devices":[{"id":"mini1","name":"工作室电脑","loading":false,"issues":[],"items":[]}]}""")
    val query = selection.query?.let(::JSONObject)
    if (query == null) {
        val item = if (selection.kind == "mcp")
            """{"id":"search","name":"资料搜索","status":"ready","description":"搜索项目资料"}"""
        else """{"id":"site","name":"设计预览","status":"running","description":"当前任务的预览服务"}"""
        data.getJSONArray("devices").getJSONObject(0).getJSONArray("items").put(JSONObject(item))
    } else {
        val inspection = JSONObject().put("loading", false)
        when {
            query.has("agent_skills") -> inspection.put("skills", JSONObject("""{"skills":[{"id":"design","name":"界面规范","description":"当前队员的设计约定"}],"diagnostics":[]}"""))
            query.has("mcp") -> inspection.put("details", JSONObject("""{"title":"资料搜索","description":"搜索项目中的公开资料","tools":[{"name":"search_docs","description":"按关键词查找","input_schema":{"type":"object","properties":{"query":{"type":"string"}}}}],"files":[],"facts":[["protocol","stdio"]]}"""))
            query.has("skill") -> inspection.put("details", JSONObject("""{"title":"界面规范","description":"队员技能","tools":[],"files":[{"path":"guide.md","byte_len":40}],"document":{"path":"SKILL.md","text":"# 界面规范\n\n复用已有组件与配色。","truncated":false},"facts":[["source","项目"]]}"""))
            query.has("service") -> inspection.put("details", JSONObject("""{"title":"设计预览","description":"当前任务服务","tools":[],"files":[{"path":"stdout.log","byte_len":16}],"facts":[["status","running"],["port","3000"]]}""").apply {
                if (query.getJSONObject("service").has("log")) put("document", JSONObject("""{"path":"stdout.log","text":"preview server ready","truncated":false}"""))
            })
        }
        data.put("inspection", inspection)
    }
    return data
}
