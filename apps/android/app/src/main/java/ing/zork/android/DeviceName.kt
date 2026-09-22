package ing.zork.android

import androidx.compose.foundation.background
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

@Composable
internal fun DeviceName(name: String, status: DeviceStatusUi, modifier: Modifier = Modifier) {
    val color = when (status.state) {
        "direct", "connected" -> ZorkColors.Online
        "mesh_failed", "revoked" -> ZorkColors.Danger
        "relay", "mesh_preparing", "connecting" -> ZorkColors.Warning
        else -> ZorkColors.Muted
    }
    val description = deviceNameSummary(name, status) + (status.error?.let { "：$it" } ?: "")
    Row(modifier.semantics(mergeDescendants = true) { contentDescription = description },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(name, Modifier.weight(1f, fill = false), maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (status.state in listOf("mesh_preparing", "mesh_stopping", "connecting")) {
            CircularProgressIndicator(Modifier.size(12.dp), color = color, strokeWidth = 1.dp)
        } else {
            Box(Modifier.size(7.dp).background(color, CircleShape))
        }
    }
}
