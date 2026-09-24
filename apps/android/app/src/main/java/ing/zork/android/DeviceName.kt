package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

/** Presentation of the shared Rust DeviceStatus, without readiness inference. */
internal data class DeviceStatusUi(val state: String = "connecting", val error: String? = null)
internal fun JSONObject.deviceStatus() = optJSONObject("status")?.let {
    DeviceStatusUi(it.text("state", "connecting"), it.text("error").ifBlank { null })
} ?: DeviceStatusUi()

internal fun deviceStatusText(status: DeviceStatusUi): String = when (status.state) {
    "mesh_not_started" -> "Mesh 未启动"
    "mesh_preparing" -> "Mesh 准备中"
    "mesh_stopping" -> "Mesh 停止中"
    "mesh_stopped" -> "Mesh 已停止"
    "mesh_failed" -> "Mesh 启动失败"
    "direct" -> "直连"
    "relay" -> "中继"
    "connected" -> "已连接"
    "offline" -> "离线"
    "revoked" -> "访问已撤销"
    else -> "连接中"
}
internal fun deviceNameSummary(name: String, status: DeviceStatusUi) = "$name · ${deviceStatusText(status)}"
internal fun compactDeviceName(name: String, status: DeviceStatusUi) = "$name ${when (status.state) {
    "direct", "connected" -> "●"
    "relay" -> "◉"
    "mesh_preparing", "mesh_stopping", "connecting" -> "◌"
    "mesh_failed", "revoked" -> "×"
    else -> "○"
}}"

/** Status never relies on color alone: online is a filled dot, relay a ring
 * with a center dot, offline a ring, failure a cross; anything but the normal
 * state also carries its short wording. */
@Composable
internal fun DeviceStatusBadge(status: DeviceStatusUi, modifier: Modifier = Modifier) {
    val normal = status.state in listOf("direct", "connected")
    val color = when (status.state) {
        "direct", "connected" -> ZorkColors.Online
        "mesh_failed", "revoked" -> ZorkColors.Danger
        "relay" -> ZorkColors.Warning
        else -> ZorkColors.Subtle
    }
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        when (status.state) {
            "mesh_preparing", "mesh_stopping", "connecting" ->
                CircularProgressIndicator(Modifier.size(12.dp), color = color, strokeWidth = 1.5.dp)
            "direct", "connected" -> Box(Modifier.size(8.dp).background(color, CircleShape))
            "relay" -> Box(Modifier.size(9.dp).border(1.5.dp, color, CircleShape), contentAlignment = Alignment.Center) {
                Box(Modifier.size(3.dp).background(color, CircleShape))
            }
            "mesh_failed", "revoked" -> Glyph(R.drawable.ic_x, 11.dp, color)
            else -> Box(Modifier.size(8.dp).border(1.5.dp, color, CircleShape))
        }
        if (!normal) Text(deviceStatusText(status), color = color, fontSize = 12.sp, maxLines = 1)
    }
}

@Composable
internal fun DeviceName(name: String, status: DeviceStatusUi, modifier: Modifier = Modifier) {
    val description = deviceNameSummary(name, status) + (status.error?.let { "：$it" } ?: "")
    Row(modifier.semantics(mergeDescendants = true) { contentDescription = description },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        DeviceMark(name, 18.dp)
        Text(name, Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis)
        DeviceStatusBadge(status)
    }
}
