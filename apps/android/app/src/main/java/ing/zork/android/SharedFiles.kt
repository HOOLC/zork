package ing.zork.android

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.zIndex
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

internal data class SharedSourceUi(val id: String, val name: String, val online: Boolean?, val cached: Boolean, val status: DeviceStatusUi = DeviceStatusUi())
internal data class SharedVersionUi(val root: String, val size: Long, val modified: Long, val sources: List<SharedSourceUi>, val canRead: Boolean)
internal data class SharedEntryUi(val id: String, val path: String, val name: String, val directory: Boolean, val sources: List<SharedSourceUi>, val versions: List<SharedVersionUi>)
internal data class SharedSpaceUi(val id: String, val name: String, val sources: List<SharedSourceUi>)
internal data class SharedPreviewUi(val path: String, val name: String, val selected: String, val versions: List<SharedVersionUi>, val loading: Boolean,
    val error: String?, val text: String?, val truncated: Boolean, val mime: String, val cached: Boolean, val canSave: Boolean)
internal data class SharedSaveUi(val busy: Boolean, val ticket: String?, val name: String, val error: String?, val completed: Boolean)
internal data class SharedFilesUi(val active: Boolean, val devices: List<SharedSourceUi>, val spaces: List<SharedSpaceUi>, val entries: List<SharedEntryUi>,
    val space: String?, val path: String, val source: String?, val search: String, val layout: String, val sort: String,
    val preview: SharedPreviewUi?, val save: SharedSaveUi, val loading: Boolean, val more: Boolean, val offline: Boolean, val error: String?,
    val locationName: String = "", val empty: String? = null)

private fun JSONObject.sourceOnline(): Boolean? = if (isNull("online")) null else getBoolean("online")
private fun JSONObject.sources() = optJSONArray("sources").objects().map { SharedSourceUi(it.text("id"), it.text("name"), it.sourceOnline(), it.optBoolean("cached"), it.deviceStatus()) }
private fun JSONObject.versions() = optJSONArray("versions").objects().map { SharedVersionUi(it.text("root"), it.optLong("size"), it.optLong("modified_ns"), it.sources(), it.optBoolean("can_read")) }
internal fun parseSharedFiles(value: JSONObject): SharedFilesUi {
    val location = value.optJSONObject("location")
    val preview = value.optJSONObject("preview")?.let { SharedPreviewUi(it.text("path"), it.text("name"), it.text("selected"), it.versions(), it.optBoolean("loading"),
        it.text("error").ifEmpty { null }, it.text("text").takeIf { _ -> !it.isNull("text") }, it.optBoolean("truncated"), it.text("mime"), it.optBoolean("cached"), it.optBoolean("can_save")) }
    val save = value.optJSONObject("save") ?: JSONObject()
    return SharedFilesUi(value.optBoolean("active"), value.optJSONArray("devices").objects().map { SharedSourceUi(it.text("id"), it.text("name"), it.sourceOnline(), false, it.deviceStatus()) },
        value.optJSONArray("spaces").objects().map { SharedSpaceUi(it.text("id"), it.text("name"), it.sources()) },
        value.optJSONArray("entries").objects().map { SharedEntryUi(it.text("id"), it.text("path"), it.text("name"), it.text("kind") == "directory", it.sources(), it.versions()) },
        location?.text("space"), location?.text("path").orEmpty(), value.text("source").ifEmpty { null }, value.text("search"), value.text("layout", "list"), value.text("sort", "name"),
        preview, SharedSaveUi(save.optBoolean("busy"), save.text("ticket").ifEmpty { null }, save.text("name"), save.text("error").ifEmpty { null }, save.optBoolean("completed")),
        value.optBoolean("loading"), value.optBoolean("more"), value.optBoolean("offline"), value.text("error").ifEmpty { null },
        value.text("location_name"), value.text("empty").ifEmpty { null })
}

internal fun sharedFilesSavedKey(data: SharedFilesUi) = org.json.JSONArray(listOf("shared", data.space, data.path, data.source, data.search, data.layout, data.sort, data.preview?.path)).toString()

internal class SharedFilesActions(
    val back: () -> Unit = {}, val space: (String) -> Unit = {}, val entry: (String) -> Unit = {},
    val source: (String?) -> Unit = {}, val search: (String) -> Unit = {}, val refresh: () -> Unit = {}, val more: () -> Unit = {},
    val layout: (String) -> Unit = {}, val sort: (String) -> Unit = {}, val version: (String) -> Unit = {}, val save: () -> Unit = {},
)

@Composable
internal fun SharedFilesPage(data: SharedFilesUi, image: ImageBitmap?, actions: SharedFilesActions, listState: LazyListState = rememberLazyListState()) {
    var sheet by remember { mutableStateOf<String?>(null) }
    var searching by remember { mutableStateOf(false) }
    var query by remember(data.search) { mutableStateOf(data.search) }
    val preview = data.preview
    var detailsOpen by remember(preview?.path) { mutableStateOf(false) }
    val title = preview?.name ?: if (data.space == null) "共享文件" else if (data.path.isNotEmpty()) data.path.substringAfterLast('/') else "共享文件"
    Column(Modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(56.dp).zIndex(1f).padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            SharedIcon("返回", R.drawable.ic_arrow_left, actions.back)
            if (searching && preview == null) {
                ZorkTextField("", query, { query = it }, modifier = Modifier.weight(1f).semantics { contentDescription = "搜索当前目录" }, placeholder = { Text("搜索当前目录", fontSize = 13.sp) },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { actions.search(query) }))
            } else Text(title, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 16.sp, fontWeight = FontWeight.Medium)
            if (preview == null) {
                SharedIcon(if (searching) "完成搜索" else "搜索", R.drawable.ic_search) {
                    if (searching) { actions.search(query); searching = false } else searching = true
                }
                ZorkIconButton("更多", opensPanel = true, onClick = { sheet = "more" }) { Glyph(R.drawable.ic_settings_three, 20.dp, ZorkColors.Muted) }
            }
        }
        if (preview != null) {
            val selected = preview.versions.find { it.root == preview.selected }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp), verticalArrangement = Arrangement.spacedBy(18.dp)) {
                ZorkButton(selected?.let { sharedSize(it.size) }.orEmpty(), quiet = true, onClick = { sheet = "versions" })
                ZorkButton("详细信息", quiet = true, onClick = { detailsOpen = !detailsOpen })
                if (detailsOpen) Text(selected?.sources?.joinToString(" · ") { compactDeviceName(it.name, it.status) }.orEmpty(), fontSize = 12.sp, color = ZorkColors.Muted)
                if (preview.loading) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                preview.error?.let { Text(it, color = ZorkColors.Muted, fontSize = 14.sp, lineHeight = 23.sp) }
                if (preview.cached) Text("正在查看已缓存的副本", color = ZorkColors.Muted, fontSize = 12.sp)
                if (image != null) Image(image, preview.name, Modifier.fillMaxWidth().heightIn(max = 600.dp))
                else if (preview.text != null) SelectionContainer { Text(preview.text, fontSize = 14.sp, lineHeight = 23.sp, color = ZorkColors.Ink) }
                else if (!preview.loading && preview.error == null) Text("保存副本后，可使用本机应用打开此文件", color = ZorkColors.Muted, fontSize = 14.sp, lineHeight = 23.sp)
                if (preview.truncated) Text("预览已截取，保存副本可查看完整内容", color = ZorkColors.Muted, fontSize = 12.sp)
                Spacer(Modifier.height(20.dp))
            }
            Column(Modifier.fillMaxWidth().padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ZorkButton(if (data.save.busy) "正在保存…" else "保存副本", primary = true,
                    onClick = actions.save, enabled = preview.canSave && !data.save.busy, modifier = Modifier.fillMaxWidth())
                data.save.error?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
                if (data.save.completed) Text("副本已保存", fontSize = 12.sp, color = ZorkColors.Muted)
            }
        } else {
            if (data.offline) Text("文件网络离线，显示已同步的目录", fontSize = 12.sp, color = ZorkColors.Muted, modifier = Modifier.padding(horizontal = 24.dp, vertical = 10.dp))
            data.error?.let { Text(it, fontSize = 13.sp, color = ZorkColors.Danger, modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp)) }
            val empty = if (data.space == null) data.spaces.isEmpty() else data.entries.isEmpty()
            if (empty) Box(Modifier.weight(1f).fillMaxWidth().padding(32.dp), contentAlignment = Alignment.Center) {
                if (data.loading) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
                else Text(when(data.empty) {
                    "no_results" -> "没有符合名称的文件"
                    "no_spaces" -> "已连接 Station 的文件会自动出现在这里。"
                    "unavailable" -> "所选来源暂时无法提供此目录。\n可切换来源或稍后重试。"
                    else -> "此目录为空"
                }, color = ZorkColors.Muted, fontSize = 14.sp, lineHeight = 24.sp)
            } else if (data.layout == "grid") {
                LazyVerticalGrid(GridCells.Adaptive(150.dp), modifier = Modifier.weight(1f).clipToBounds(), contentPadding = PaddingValues(12.dp)) {
                    if (data.space == null) items(data.spaces, key = { it.id }) { space -> SharedFileRow(space.name, true, space.sources, 0, true) { actions.space(space.id) } }
                    else items(data.entries, key = { it.id }) { entry -> SharedFileRow(entry.name, entry.directory, entry.sources, entry.versions.size, true) { actions.entry(entry.id) } }
                }
            } else LazyColumn(state = listState, modifier = Modifier.weight(1f).clipToBounds(), contentPadding = PaddingValues(horizontal = 12.dp, vertical = 8.dp)) {
                if (data.space == null) items(data.spaces, key = { it.id }) { space -> SharedFileRow(space.name, true, space.sources, 0, false) { actions.space(space.id) } }
                else items(data.entries, key = { it.id }) { entry -> SharedFileRow(entry.name, entry.directory, entry.sources, entry.versions.size, false) { actions.entry(entry.id) } }
            }
            if (data.more) ZorkButton(if (data.loading) "正在载入…" else "载入更多", quiet = true, onClick = actions.more, enabled = !data.loading, modifier = Modifier.fillMaxWidth())
        }
    }
    ZorkRetained(sheet) { shown, open, closed -> when (shown) {
        "sources", "settings" -> SettingsSheet(if (shown == "sources") "文件来源" else "共享来源", dismiss = { sheet = null }, open = open, onClosed = closed) {
            SharedChoice("所有设备", data.source == null) { actions.source(null); sheet = null }
            data.devices.forEach { source -> SharedChoice(compactDeviceName(source.name, source.status), data.source == source.id) { actions.source(source.id); sheet = null } }
        }
        "versions" -> SettingsSheet("文件版本", dismiss = { sheet = null }, open = open, onClosed = closed) {
            preview?.versions?.forEach { version ->
                val modified = remember(version.modified) { DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm").withZone(ZoneId.systemDefault()).format(Instant.ofEpochSecond(version.modified / 1_000_000_000)) }
                SharedChoice("${version.sources.joinToString(" · ") { compactDeviceName(it.name, it.status) }} · ${sharedSize(version.size)}\n$modified${if (!version.canRead) " · 暂不可读取" else ""}", preview.selected == version.root) { actions.version(version.root); sheet = null }
            }
        }
        "more" -> SettingsSheet("更多", dismiss = { sheet = null }, open = open, onClosed = closed) {
            SharedChoice("刷新", false) { actions.refresh(); sheet = null }
            SharedChoice(if (data.layout == "list") "图标布局" else "列表布局", false) { actions.layout(if (data.layout == "list") "grid" else "list"); sheet = null }
            SharedChoice(if (data.sort == "name") "按名称降序" else "按名称升序", false) { actions.sort(if (data.sort == "name") "name_descending" else "name"); sheet = null }
            SharedChoice("共享来源", false) { sheet = "settings" }
        }
    } }
}

@Composable private fun SharedIcon(label: String, icon: Int, click: () -> Unit) {
    ZorkIconButton(label, onClick = click) { Icon(painterResource(icon), null, Modifier.size(20.dp), tint = ZorkColors.Muted) }
}
@Composable private fun SharedChoice(text: String, selected: Boolean, click: () -> Unit) {
    ZorkListRow(onClick = click) {
        Text(text, modifier = Modifier.weight(1f), fontSize = 14.sp, lineHeight = 23.sp)
        if (selected) Glyph(R.drawable.ic_check, 16.dp, UiTokens.Accent)
    }
}
@Composable private fun SharedFileRow(name: String, directory: Boolean, sources: List<SharedSourceUi>, versions: Int, grid: Boolean, click: () -> Unit) {
    val hint = if (versions > 1) "$versions 个版本" else if (sources.isNotEmpty() && sources.all { it.online == false }) "来源离线" else null
    Surface(onClick = click,
        modifier = Modifier.fillMaxWidth().heightIn(min = if (grid) 136.dp else 68.dp),
        shape = androidx.compose.foundation.shape.RoundedCornerShape(UiTokens.FieldRadius),
        color = androidx.compose.ui.graphics.Color.Transparent) {
        if (grid) Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Glyph(if (directory) R.drawable.ic_folder else R.drawable.ic_file, 28.dp)
            Text(name, maxLines = 2, overflow = TextOverflow.Ellipsis, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            hint?.let { Text(it, fontSize = 11.sp, color = ZorkColors.Muted) }
        } else Row(Modifier.padding(horizontal = 12.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            Glyph(if (directory) R.drawable.ic_folder else R.drawable.ic_file, 24.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(name, maxLines = 1, overflow = TextOverflow.Ellipsis, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            }
            hint?.let { Text(it, fontSize = 10.sp, color = ZorkColors.Muted) }
        }
    }
}
private fun sharedSize(bytes: Long) = when {
    bytes >= 1024 * 1024 -> String.format("%.1f MB", bytes.toDouble() / (1024 * 1024))
    bytes >= 1024 -> "${bytes / 1024} KB"
    else -> "$bytes B"
}
