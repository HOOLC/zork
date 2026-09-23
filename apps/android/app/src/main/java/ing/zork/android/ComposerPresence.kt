package ing.zork.android

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject
import kotlin.math.roundToInt

internal data class ComposerMember(
    val id: String, val name: String, val label: String,
    val failed: Boolean, val session: String = "",
)

/** Presence has a fixed layout slot; updates never animate the composer outline. */
internal data class ComposerPresence(val members: List<ComposerMember>) {
    val targetExtent: Float get() = if (members.isEmpty()) 0f else 48f
    val extent: Float get() = targetExtent
    fun extentPixels(density: Float): Int = (extent * density).roundToInt()
}

internal fun activityLabel(activity: JSONObject?): String = when (activity?.text("state")) {
    "live" -> activity.optJSONObject("presentation")?.text("label_zh").orEmpty()
    "thinking", "tool_finished" -> "正在思考"
    "tools_started", "tools_waiting" -> if (activity.optBoolean("thinking") && activity.optJSONArray("calls").objects().isNotEmpty()) {
        "正在思考 · ${activity.optJSONArray("calls").objects().size} 项操作执行中"
    } else activity.optJSONArray("calls").objects().joinToString(" · ") {
        if (it.text("action").isNotBlank()) return@joinToString it.text("action")
        val labels = it.optJSONObject("labels")
        val action = labels?.optString("zh-CN").orEmpty().ifBlank {
            labels?.optString("en").orEmpty().ifBlank { "执行操作" }
        }
        listOf(action, it.text("detail")).filter(String::isNotBlank).joinToString(" ")
    }.ifBlank { "正在处理" }
    "waiting" -> listOf("等待中", activity.text("reason")).filter(String::isNotBlank).joinToString(" · ")
    "failed" -> listOf("执行失败", activity.text("reason")).filter(String::isNotBlank).joinToString(" · ")
    else -> ""
}

@Composable
internal fun rememberComposerPresence(state: WorkbenchState, availableWidth: Float): ComposerPresence {
    val members = state.participants.map {
        val activity = it.optJSONObject("activity")
        ComposerMember(it.text("id"), it.text("name"), activityLabel(activity),
            activity?.text("state") == "failed", it.text("session_id"))
    }
    return remember(members) { ComposerPresence(members) }
}

@Composable
internal fun ComposerMembers(
    presence: ComposerPresence, history: (String, String) -> Unit = { _, _ -> },
) {
    Box(Modifier.fillMaxWidth().height((presence.extent + 40f).dp),
        contentAlignment = Alignment.TopStart) {
        if (presence.members.isNotEmpty()) {
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())
                .padding(horizontal = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                presence.members.forEach { member ->
                    Surface(onClick = { history(member.session, member.name) },
                        enabled = member.session.isNotBlank(),
                        shape = RoundedCornerShape(UiTokens.PillRadius),
                        color = ZorkColors.Canvas) {
                        Row(Modifier.height(40.dp).padding(horizontal = 8.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text("${member.name} · ${member.label.ifBlank { "空闲" }}", fontSize = 12.sp,
                                color = if (member.failed) ZorkColors.Danger else ZorkColors.Ink)
                        }
                    }
                }
            }
        }
    }
}
