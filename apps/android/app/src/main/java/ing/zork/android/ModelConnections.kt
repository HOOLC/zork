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

/** One connection in the global list, with the device that keeps it. */
private data class ConnectionUi(val peer: String, val device: String, val profile: JSONObject, val providerLabel: String, val billingLabel: String?)

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

internal fun accessLabel(profile: JSONObject) = if (profile.text("billing") == "subscription") "订阅" else "API Key"

/** Every device's model connections, grouped by provider. A device that could
 * not be read shows why and when its cache is from; it never reads as empty. */
@Composable
internal fun ModelConnectionsPage(state: MobileSettingsState, peers: List<Peer>, actions: SettingsActions, modifier: Modifier = Modifier) {
    val devices = state.connections.orEmpty()
    var choosing by rememberSaveable { mutableStateOf(false) }
    val rows = remember(devices) {
        devices.flatMap { device ->
            val providers = device.optJSONArray("providers").objects()
            device.optJSONArray("profiles").objects().map { profile ->
                val provider = providers.find { it.text("id") == profile.text("provider") }
                val billing = provider?.optJSONArray("billing").objects().find { it.text("id") == profile.text("billing") }
                ConnectionUi(device.text("peer"), device.text("name"), profile,
                    provider?.text("label")?.takeIf { it.isNotBlank() } ?: profile.text("provider"), billing?.text("label"))
            }
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
                        Text(provider, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle,
                            modifier = Modifier.padding(start = 8.dp, top = 8.dp))
                        SettingsListGroup {
                            connections.sortedBy { profileName(it.profile).lowercase() }.forEachIndexed { index, connection ->
                                ConnectionCard(connection) { actions.connection(connection.peer, connection.profile) }
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
    val models = profile.optJSONArray("models")?.length() ?: 0
    val verification = profile.text("verification", if (profile.optBoolean("verified")) "verified" else "pending")
    val remaining = if (profile.text("billing") == "subscription") quotaRemaining(profile) else null
    val description = "${profileName(profile)}，${accessLabel(profile)}，${connection.device}"
    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 52.dp).semantics(mergeDescendants = true) { contentDescription = description }, onClick = open) {
        Text(profileName(profile), fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false))
        DeviceMark(connection.device, 16.dp)
        Spacer(Modifier.weight(1f))
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
    val total = devices.sumOf { it.optJSONArray("profiles")?.length() ?: 0 }
    return if (devices.any { it.text("state") != "ready" } && total == 0) "—" else total.toString()
}
