package ing.zork.android

import org.json.JSONObject

internal fun settingsFixturePage(state: MobileSettingsState, page: String): MobileSettingsState {
    val selection = when (page) {
        "services" -> ResourceSelection(state.device?.id, "service", title = "服务")
        else -> null
    }
    return state.copy(page = page, resource = selection, resourceData = selection?.let(::settingsResourceFixture))
}

/** Explicit in-memory payloads for production settings composables. */
internal fun settingsResourceFixture(selection: ResourceSelection): JSONObject {
    val data = JSONObject("""{"devices":[{"id":"mini1","name":"工作室电脑","loading":false,"issues":[],"items":[]}]}""")
    val query = selection.query?.let(::JSONObject)
    if (query == null) {
        val item = """{"id":"site","name":"设计预览","status":"running","description":"当前任务的预览服务"}"""
        data.getJSONArray("devices").getJSONObject(0).getJSONArray("items").put(JSONObject(item))
    } else {
        val inspection = JSONObject().put("loading", false)
        when {
            query.has("service") -> inspection.put("details", JSONObject("""{"title":"设计预览","description":"当前任务服务","files":[{"path":"stdout.log","byte_len":16}],"facts":[["status","running"],["port","3000"]]}""").apply {
                if (query.getJSONObject("service").has("log")) put("document", JSONObject("""{"path":"stdout.log","text":"preview server ready","truncated":false}"""))
            })
        }
        data.put("inspection", inspection)
    }
    return data
}
