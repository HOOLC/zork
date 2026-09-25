package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** One device's copy of an account. */
private data class SourceUi(val peer: String, val device: String, val profile: JSONObject)
/** One account in the global list: core merges the same provider account saved on
 * several devices; [profile] is the source whose quota represents it. */
private data class ConnectionUi(val id: String, val name: String, val customName: String?, val profile: JSONObject, val providerLabel: String,
    val models: Int, val sources: List<SourceUi>)

/** Verification from core's `verification`: verified, failed or pending. */
@Composable
internal fun VerificationPill(profile: JSONObject) {
    val (label, fg, bg) = when (profile.text("verification", if (profile.optBoolean("verified")) "verified" else "pending")) {
        "verified" -> Triple("已验证", ZorkColors.Online, ZorkColors.SuccessSoft)
        "failed" -> Triple("验证失败", ZorkColors.Danger, ZorkColors.DangerSoft)
        else -> Triple("待验证", ZorkColors.Warning, ZorkColors.WarningSoft)
    }
    Row(Modifier.heightIn(min = 24.dp).background(bg, ZorkShapes.Control).padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        if (label == "已验证") Glyph(R.drawable.ic_check, 12.dp, fg)
        Text(label, fontSize = 12.sp, color = fg, maxLines = 1)
    }
}

/** Core writes an API key's title as "access · ···tail"; split so only the access ellipsizes. */
internal fun splitKeyTail(title: String): Pair<String, String?> {
    val at = title.lastIndexOf(" · ···")
    return if (at <= 0) title to null else title.substring(0, at) to title.substring(at + 3)
}

internal fun accessLabel(profile: JSONObject) = if (profile.text("billing") == "subscription") "订阅" else "API Key"

/** Every device's model connections, grouped by provider. A device that could
 * not be read shows why and when its cache is from; it never reads as empty. */
@Composable
internal fun ModelConnectionsPage(state: MobileSettingsState, peers: List<Peer>, actions: SettingsActions, modifier: Modifier = Modifier) {
    val devices = state.connections.orEmpty()
    var choosing by rememberSaveable { mutableStateOf(false) }
    var sourcesOf by rememberSaveable { mutableStateOf<String?>(null) }
    val rows = remember(devices, state.accounts) {
        val labels = devices.flatMap { it.optJSONArray("providers").objects() }
            .associate { it.text("id") to it.text("label") }.filterValues { it.isNotBlank() }
        // Without core's accounts (an older core), each device's profile is its own entry.
        val accounts = state.accounts ?: devices.flatMap { device ->
            device.optJSONArray("profiles").objects().map { profile ->
                JSONObject().put("id", "profile:${device.text("peer")}/${profile.text("profile_id")}")
                    .put("title", connectionTitle(profile)).put("custom_name", customName(profile)).put("provider", profile.text("provider")).put("profile", profile)
                    .put("models", profile.optJSONArray("models")?.length() ?: 0)
                    .put("sources", org.json.JSONArray().put(JSONObject().put("peer", device.text("peer")).put("device", device.text("name")).put("profile", profile)))
            }
        }
        accounts.map { account ->
            val profile = account.optJSONObject("profile") ?: JSONObject()
            // Core titles the account; a Profile name only when the user set one.
            val title = account.text("title").ifBlank { account.text("name").ifBlank { connectionTitle(profile) } }
            ConnectionUi(account.text("id"), title, account.text("custom_name").takeIf { it.isNotBlank() && it != title }, profile,
                labels[account.text("provider")] ?: account.text("provider"), account.optInt("models"),
                account.optJSONArray("sources").objects().map { SourceUi(it.text("peer"), it.text("device"), it.optJSONObject("profile") ?: profile) })
        }
    }
    val groups = remember(rows) { rows.groupBy { it.providerLabel }.toSortedMap(String.CASE_INSENSITIVE_ORDER) }
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回", onClick = actions.back)
            Text("模型连接", fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f), maxLines = 1)
            SettingsRefreshButton(state.loading, actions.refresh)
            ZorkIconButton("添加连接", opensPanel = true, enabled = peers.isNotEmpty(), onClick = { choosing = true }) {
                Icon(painterResource(R.drawable.ic_plus), null, Modifier.size(20.dp))
            }
        }
        LazyColumn(Modifier.fillMaxWidth().weight(1f), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            state.message?.takeIf { it.isNotBlank() }?.let { message ->
                item(key = "message") { ConnectionBanner(message, null) }
            }
            items(devices.filter { it.text("state") in listOf("failed", "revoked") }, key = { "issue:${it.text("peer")}" }) { device ->
                val name = device.text("name")
                // One line: whose connections are stale; the reason stays available to screen readers.
                val text = if (device.text("state") == "revoked") "$name 已撤销访问"
                    else if (device.optBoolean("cached")) "$name 读不到，显示缓存" else "$name 读不到"
                val reason = device.text("error").takeIf { it.isNotBlank() && device.text("state") != "revoked" }
                ConnectionBanner(text, if (device.text("state") == "failed") actions.refresh else null, reason)
            }
            devices.filter { it.text("state") == "loading" }.takeIf { it.isNotEmpty() }?.let { loading ->
                item(key = "loading") {
                    Text("正在读取 ${loading.joinToString("、") { it.text("name") }} 上的连接…", fontSize = 13.sp, color = ZorkColors.Muted,
                        modifier = Modifier.padding(horizontal = 8.dp))
                }
            }
            groups.forEach { (provider, connections) ->
                item(key = "provider:$provider") {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Row(Modifier.padding(start = 8.dp, top = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(provider, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle, maxLines = 1,
                                overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                            Text("${connections.size} 个账号", fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1)
                        }
                        SettingsListGroup {
                            connections.sortedBy { it.name.lowercase() }.forEach { connection ->
                                ConnectionCard(connection) {
                                    // One device opens its connection; several list their sources first.
                                    val only = connection.sources.singleOrNull()
                                    if (only != null) actions.connection(only.peer, only.profile) else sourcesOf = connection.id
                                }
                            }
                        }
                    }
                }
            }
            // Only claim "none" when every device was actually read.
            if (devices.isNotEmpty() && rows.isEmpty() && devices.all { it.text("state") == "ready" }) item(key = "empty") {
                Text("还没有模型连接", fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.padding(horizontal = 8.dp, vertical = 24.dp))
            }
            if (devices.isEmpty() && state.connections != null && !state.loading) item(key = "no-devices") {
                Text("先连接一台设备", fontSize = 13.sp, color = ZorkColors.Muted,
                    modifier = Modifier.padding(horizontal = 8.dp, vertical = 24.dp))
            }
        }
    }
    ZorkRetained(rows.find { it.id == sourcesOf && it.sources.size > 1 }) { account, open, closed ->
        SettingsSheet("${account.name} · 选择设备", dismiss = { sourcesOf = null }, open = open, onClosed = closed) {
            SettingsListGroup {
                account.sources.forEach { source ->
                    val verification = source.profile.text("verification", if (source.profile.optBoolean("verified")) "verified" else "pending")
                    SettingsListRow(source.device, leading = { DeviceMark(source.device, 24.dp) },
                        subtext = customName(source.profile),
                        trailing = { if (verification != "verified") VerificationPill(source.profile) },
                        action = { sourcesOf = null; actions.connection(source.peer, source.profile) })
                }
            }
        }
    }
    ZorkRetained(Unit.takeIf { choosing }) { _, open, closed ->
        SettingsSheet("添加到哪台设备？", dismiss = { choosing = false }, open = open, onClosed = closed) {
            SettingsListGroup {
                val revoked = devices.filter { it.text("state") == "revoked" }.map { it.text("peer") }.toSet()
                peers.filter { it.id !in revoked }.forEachIndexed { index, peer ->
                    SettingsListRow(peer.name, leading = { DeviceMark(peer.name, 24.dp, colorKey = peer.colorKey) },
                        trailing = { DeviceStatusBadge(peer.status) },
                        action = { choosing = false; actions.addConnection(peer.id) })
                }
            }
        }
    }
}

@Composable
private fun ConnectionBanner(text: String, retry: (() -> Unit)?, reason: String? = null) {
    Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) { reason?.let { contentDescription = "$text，$it" } }.background(ZorkColors.WarningSoft, ZorkShapes.Container)
        .padding(start = 18.dp, end = if (retry != null) 8.dp else 18.dp, top = 8.dp, bottom = 8.dp).heightIn(min = 40.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Glyph(R.drawable.ic_attention, 18.dp, ZorkColors.Warning)
        Text(text, fontSize = 13.sp, lineHeight = 19.sp, color = ZorkColors.Warning, modifier = Modifier.weight(1f))
        retry?.let { ZorkButton("重试", onClick = it) }
    }
}

@Composable
private fun ConnectionCard(connection: ConnectionUi, open: () -> Unit) {
    // One row: name, where it lives, a thin quota bar for subscriptions and the model
    // count. A status pill appears only when verification needs attention.
    val profile = connection.profile
    val models = connection.models
    val verification = profile.text("verification", if (profile.optBoolean("verified")) "verified" else "pending")
    val remaining = if (profile.text("billing") == "subscription") quotaRemaining(profile) else null
    val devices = connection.sources.joinToString("、") { it.device }
    val heading = listOfNotNull(connection.name, connection.customName).joinToString(" · ")
    val description = "$heading，${accessLabel(profile)}，$devices"
    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 52.dp).semantics(mergeDescendants = true) { contentDescription = description }, onClick = open) {
        // Account, custom name and device marks share all the width left of the trailing
        // facts, so the account only ellipsizes when it truly does not fit.
        Row(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            // An API key's tail identifies the account, so the access wording before it
            // is what ellipsizes: "OpenCode G… · ···a1b2".
            val (head, tail) = splitKeyTail(connection.name)
            Row(Modifier.weight(1f, fill = false), verticalAlignment = Alignment.CenterVertically) {
                Text(head, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false))
                tail?.let { Text(" · $it", fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, softWrap = false) }
            }
            // A name the user set: secondary and capped, so the account keeps the width.
            connection.customName?.let {
                Text(it, fontSize = 13.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.widthIn(max = 72.dp))
            }
            // Every device that holds this account, in core's order.
            Row(horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
                connection.sources.forEach { DeviceMark(it.device, 16.dp) }
            }
        }
        if (verification != "verified") VerificationPill(profile)
        else {
            remaining?.let { target ->
                val share = animatedValue(target, "quota")
                val tone = when { share < 10f -> ZorkColors.Danger; share < 30f -> ZorkColors.Warning; else -> ZorkColors.Ink }
                Box(Modifier.width(48.dp).height(4.dp).background(ZorkColors.Border, ZorkShapes.Control)) {
                    Box(Modifier.fillMaxWidth(share / 100f).fillMaxHeight().background(tone, ZorkShapes.Control))
                }
            }
            Text("$models", fontSize = 13.sp, color = ZorkColors.Subtle, modifier = Modifier.semantics { contentDescription = "$models 个模型" })
        }
    }
}

/** Count for the settings home row; `null` while nothing was read yet. */
internal fun connectionCount(state: MobileSettingsState): String {
    val devices = state.connections ?: return "—"
    val total = state.accounts?.size ?: devices.sumOf { it.optJSONArray("profiles")?.length() ?: 0 }
    return if (devices.any { it.text("state") != "ready" } && total == 0) "—" else total.toString()
}
