package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.util.Locale

internal fun profileName(profile: JSONObject) = profile.text("name").takeIf { it.isNotBlank() } ?: profile.text("profile_id")
internal fun compactTokens(value: Long): String = when {
    value >= 1_000_000 && value % 1_000_000 == 0L -> "${value / 1_000_000}M"
    value >= 1000 && value % 1000 == 0L -> "${value / 1000}K"
    else -> value.toString()
}

private data class QuotaWindowUi(val label: String, val remaining: Float, val value: String, val reset: String?)
private data class QuotaUi(val failed: Boolean, val windows: List<QuotaWindowUi>, val balance: String?, val checked: String?)

private fun quotaUi(profile: JSONObject): QuotaUi {
    val quota = profile.optJSONObject("quota")
    val windows = quota?.optJSONArray("windows").objects().map { window ->
        val minutes = window.optLong("minutes")
        val duration = when {
            minutes <= 0 -> "额度"
            minutes % 1440 == 0L -> "${minutes / 1440}天"
            minutes % 60 == 0L -> "${minutes / 60}小时"
            else -> "${minutes}分钟"
        }
        val label = listOf(window.text("name"), duration).filter { it.isNotBlank() }.joinToString(" · ")
        val remaining = window.optDouble("remaining", 0.0).coerceIn(0.0, 100.0)
        val percent = if (remaining > 0 && remaining < 1) "<1" else remaining.toInt().toString()
        val reset = window.optLong("resets_at").takeIf { it > 0 }?.let {
            val at = java.time.Instant.ofEpochSecond(it).atZone(java.time.ZoneId.systemDefault())
            at.format(DateTimeFormatter.ofPattern(if (at.toLocalDate() == java.time.LocalDate.now()) "HH:mm" else "M月d日")) + " 重置"
        }
        QuotaWindowUi(label, remaining.toFloat(), "剩余 $percent%", reset)
    }
    val balance = quota?.optJSONArray("balance")?.let {
        val unit = it.optString(1).let { value -> if (value == "credits") "积分" else value }
        "余额 ${String.format(Locale.ROOT, "%.2f", it.optDouble(0))} $unit"
    }
    val checked = profile.text("checkedAt").takeIf { it.isNotBlank() }?.let { time ->
        runCatching { "更新于 " + OffsetDateTime.parse(time).atZoneSameInstant(java.time.ZoneId.systemDefault()).format(DateTimeFormatter.ofPattern("M月d日 HH:mm")) }.getOrNull()
    }
    return QuotaUi(quota?.optBoolean("failed") == true, windows, balance, checked)
}

/** One row per quota window. Ink by default; colour appears only when a window runs low. */
@Composable
internal fun ProfileQuota(profile: JSONObject, summary: Boolean = false) {
    val quota = remember(profile) { quotaUi(profile) }
    if (quota.failed) {
        Text("额度查询失败", fontSize = 12.sp, color = ZorkColors.Warning)
        return
    }
    val windows = if (summary) quota.windows.take(2) else quota.windows
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        windows.forEach { window ->
            val tone = when { window.remaining < 10f -> ZorkColors.Danger; window.remaining < 30f -> ZorkColors.Warning; else -> ZorkColors.Ink }
            Row(Modifier.fillMaxWidth().heightIn(min = 32.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(window.label, fontSize = 13.sp, modifier = Modifier.width(64.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                Box(Modifier.weight(1f).height(6.dp).background(ZorkColors.Border, ZorkShapes.Control)) {
                    Box(Modifier.fillMaxWidth(window.remaining / 100f).fillMaxHeight().background(tone, ZorkShapes.Control))
                }
                Text(window.value, fontSize = 12.sp, color = if (tone == ZorkColors.Ink) ZorkColors.Muted else tone)
                if (!summary) window.reset?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Subtle) }
            }
        }
        quota.balance?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
    }
}

/** The tightest quota window's remaining share (0–100), for a thin list bar; null without quota. */
internal fun quotaRemaining(profile: JSONObject): Float? =
    quotaUi(profile).takeIf { !it.failed }?.windows?.minOfOrNull { it.remaining }

/** "400K · 128K · low–xhigh": a model's limits and thinking range as one muted line. */
internal fun modelSummary(model: JSONObject): String {
    val limits = model.optJSONObject("limits") ?: return "待配置"
    val thinking = model.optJSONArray("thinking")?.let { values -> (0 until values.length()).map { values.optString(it) } }.orEmpty()
        .filter { it.isNotBlank() && it != "off" }
    return listOfNotNull(
        compactTokens(limits.optLong("context_window_tokens")),
        compactTokens(limits.optLong("max_output_tokens")),
        when (thinking.size) { 0 -> null; 1 -> thinking[0]; else -> "${thinking.first()}–${thinking.last()}" },
    ).joinToString(" · ")
}

@Composable
private fun SectionHeader(title: String, meta: String? = null, actions: @Composable RowScope.() -> Unit = {}) {
    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(start = 4.dp, top = 12.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        meta?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f)) }
            ?: Spacer(Modifier.weight(1f))
        actions()
    }
}

@Composable
internal fun ModelSettingsPage(state: MobileSettingsState, actions: SettingsActions, modifier: Modifier = Modifier) {
    val profile = state.profile
    val detail = state.page == "profile"
    val profileId = state.selectedProfileId ?: profile?.text("profile_id").orEmpty()
    var editor by rememberSaveable(state.device?.id, profileId) { mutableStateOf<String?>(null) }
    var editingJson by rememberSaveable(state.device?.id, profileId) { mutableStateOf<String?>(null) }
    val editing = editingJson?.let(::JSONObject)
    var operation by remember(state.device?.id, profileId) { mutableStateOf<String?>(null) }
    var error by remember(state.device?.id, profileId) { mutableStateOf<String?>(null) }
    var updateMessage by remember(state.device?.id, profileId) { mutableStateOf<String?>(null) }
    var more by remember { mutableStateOf(false) }
    // Arriving from the global list's add flow opens the editor once the device is usable.
    var pendingAdd by rememberSaveable(state.device?.id) { mutableStateOf(state.addConnection) }
    val scope = rememberCoroutineScope()
    val enabled = state.online && state.profilesReady && !state.loading && (operation == null || operation == "quota")
    val refreshingQuota = profileId in state.profileRefreshing || operation == "quota"
    fun perform(name: String, work: suspend () -> Unit) {
        scope.launch {
            operation = name; error = null
            try { work(); actions.refresh() }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { error = e.message ?: "操作失败，请重试" }
            finally { operation = null }
        }
    }
    fun editModel(model: JSONObject?) { editingJson = model?.toString(); editor = "model" }
    fun discover() = perform("models") {
        val result = actions.perform("discover_models", JSONObject().put("profile", profileId))
        updateMessage = when {
            result.optInt("added") > 0 -> "已添加 ${result.optInt("added")} 个模型"
            result.optInt("configured") > 0 -> "已配置 ${result.optInt("configured")} 个模型"
            else -> "模型已是最新"
        } + if (result.optBoolean("truncated")) "，仅返回部分结果" else ""
    }
    LaunchedEffect(pendingAdd, enabled) { if (pendingAdd && enabled && !detail) { pendingAdd = false; editingJson = null; editor = "connection" } }
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = actions.back)
            if (!detail) Text("模型连接", fontSize = 18.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            else Spacer(Modifier.weight(1f))
            SettingsRefreshButton(state.loading, actions.refresh)
            if (detail && profile != null) Box {
                IconAction(R.drawable.ic_more, "更多", enabled = enabled) { more = true }
                PlainMenu("连接操作", more, { more = false }, 180.dp) {
                    ZorkMenuItem("重命名", false, onClick = { more = false; editingJson = profile.toString(); editor = "profile-name" })
                    ZorkMenuItem("刷新额度", false, enabled = !refreshingQuota, onClick = {
                        more = false; perform("quota") { actions.perform("refresh_quota", JSONObject().put("profile", profileId)) }
                    })
                }
            }
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 0.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp)) {
            item(key = "notice") {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (!state.online) Text(if (state.connectionState == "connecting") "正在连接设备…" else "设备离线，显示已保存的设置", color = ZorkColors.Subtle, fontSize = 12.sp)
                    (error ?: state.profileMessage ?: state.message)?.takeIf { it.isNotBlank() }?.let {
                        Text(it, color = ZorkColors.Danger, fontSize = 13.sp, lineHeight = 19.sp,
                            modifier = Modifier.fillMaxWidth().background(ZorkColors.DangerSoft, ZorkShapes.Container).padding(horizontal = 16.dp, vertical = 10.dp))
                    }
                }
            }
            if (!detail) {
                if (state.authorization != null) item(key = "authorization") {
                    SettingsButton("继续连接") { editingJson = null; editor = "connection" }
                }
                item(key = "heading") {
                    Row(Modifier.fillMaxWidth().padding(start = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        DeviceMark(state.device?.name.orEmpty(), 18.dp)
                        Text("${state.device?.name.orEmpty()} · ${if (state.profilesReady) state.profiles.size else "—"} 个连接", fontSize = 13.sp, color = ZorkColors.Muted,
                            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        ZorkButton("添加", quiet = true, enabled = enabled, onClick = { editingJson = null; editor = "connection" },
                            leading = { Glyph(R.drawable.ic_plus, 16.dp, ZorkColors.Ink) })
                    }
                }
                items(state.profiles, key = { it.text("profile_id") }) { row ->
                    val provider = state.providers.find { it.text("id") == row.text("provider") }
                    val billing = provider?.optJSONArray("billing").objects().find { it.text("id") == row.text("billing") }
                    Column {
                        SettingsListRow(profileName(row), subtext = "${provider?.text("label") ?: row.text("provider")} · ${billing?.text("label") ?: row.text("billing")}",
                            detail = "${row.optJSONArray("models")?.length() ?: 0} 个模型", leading = { ProviderMark(row.text("provider")) },
                            trailing = { VerificationPill(row) },
                            action = { actions.profile(row) })
                        val quota = remember(row) { quotaUi(row) }
                        if (row.text("profile_id") in state.profileFailed) Text("额度刷新失败，已保留上次结果", color = ZorkColors.Danger, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp))
                        if (quota.failed || quota.windows.isNotEmpty() || quota.balance != null) Column(Modifier.padding(start = 60.dp, end = 16.dp, bottom = 8.dp)) { ProfileQuota(row, true) }
                    }
                }
                if (state.profilesReady && state.profiles.isEmpty()) item {
                    Column(Modifier.padding(horizontal = 4.dp, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("这台设备还没有模型连接", fontSize = 16.sp, fontWeight = FontWeight.Medium)
                        Text("添加订阅账号或 API Key，供新 Chat 选择模型。", fontSize = 13.sp, lineHeight = 21.sp, color = ZorkColors.Muted)
                    }
                }
            } else if (profile == null) {
                item { Text(if (state.loading) "正在读取连接…" else "此连接已不可用，返回模型连接列表查看。", fontSize = 14.sp, color = ZorkColors.Muted) }
            } else {
                val provider = state.providers.find { it.text("id") == profile.text("provider") }
                val verified = profile.text("verification", if (profile.optBoolean("verified")) "verified" else "pending") == "verified"
                item(key = "identity") {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        ProviderMark(profile.text("provider"), 32)
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(profileName(profile), fontSize = 18.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Text("${provider?.text("label") ?: profile.text("provider")} · ${accessLabel(profile)} · 保存在",
                                    fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1)
                                DeviceMark(state.device?.name.orEmpty(), 14.dp)
                                Text(state.device?.name.orEmpty(), fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                        if (!verified) VerificationPill(profile)
                    }
                }
                item(key = "quota") {
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        SectionHeader("额度", remember(profile) { quotaUi(profile).checked }) {
                            if (refreshingQuota) Text("刷新中…", fontSize = 12.sp, color = ZorkColors.Subtle)
                        }
                        ProfileQuota(profile)
                        if (profileId in state.profileFailed) Text("额度刷新失败，已保留上次结果", color = ZorkColors.Danger, fontSize = 12.sp)
                    }
                }
                item(key = "models-heading") {
                    Column {
                        SectionHeader("模型") {
                            ZorkButton(if (operation == "models") "获取中…" else "获取模型", quiet = true, enabled = enabled, onClick = { discover() },
                                leading = { Glyph(R.drawable.ic_reload, 14.dp, ZorkColors.Ink) })
                            ZorkButton("手动添加", quiet = true, enabled = enabled, onClick = { editModel(null) },
                                leading = { Glyph(R.drawable.ic_plus, 14.dp, ZorkColors.Ink) })
                        }
                        updateMessage?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Subtle, modifier = Modifier.padding(start = 4.dp)) }
                    }
                }
                val models = profile.optJSONArray("models").objects()
                items(models, key = { "model:${it.text("id")}" }) { model ->
                    val configured = model.optJSONObject("limits") != null
                    val active = model.optBoolean("enabled", true)
                    SettingsListRow(model.text("id"), subtext = modelSummary(model), action = { editModel(model) },
                        trailing = {
                            ZorkSwitch(active, { on -> perform("enabled") {
                                actions.perform("enable_model", JSONObject().put("profile", profileId).put("model", model.text("id")).put("enabled", on))
                            } }, enabled = enabled && (active || configured),
                                modifier = Modifier.semantics { contentDescription = "模型启用 ${model.text("id")}" })
                        })
                }
                if (models.isEmpty()) item { Text("还没有模型，点“获取模型”从供应商读取", fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.padding(4.dp)) }
            }
        }
    }
    ZorkRetained(editor?.let { it to editing }) { (kind, source), open, closed ->
        key(kind, source?.text("id")) {
            SettingsEditor(kind, source, state, actions, "", { editor = null }, { editor = null; actions.refresh() }, open = open, onClosed = closed)
        }
    }
}
