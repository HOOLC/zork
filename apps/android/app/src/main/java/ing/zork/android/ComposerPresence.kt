package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.json.JSONObject

internal data class ComposerMember(
    val id: String, val name: String, val label: String,
    val failed: Boolean, val session: String = "", val working: Boolean = false,
    /** Current tool steps as (tool, object), newest last. */
    val steps: List<Pair<String, String>> = emptyList(),
)

/**
 * Live member activity, shown as the last item of the message list. Only members
 * that are working or failed are news; idle members keep their history entry in
 * the header, and a finished round leaves the list with its final message.
 */
internal data class ConversationActivity(val members: List<ComposerMember>) {
    val visible: Boolean get() = members.isNotEmpty()
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

/** Each running call as (tool, object) for the capsule and its step list. */
internal fun activitySteps(activity: JSONObject?): List<Pair<String, String>> =
    activity?.optJSONArray("calls").objects().map { call ->
        val labels = call.optJSONObject("labels")
        val tool = call.text("action").ifBlank {
            labels?.optString("zh-CN").orEmpty().ifBlank { labels?.optString("en").orEmpty().ifBlank { "执行操作" } }
        }
        tool to call.text("detail")
    }

@Composable
internal fun rememberConversationActivity(state: WorkbenchState): ConversationActivity {
    val members = state.participants.map {
        val activity = it.optJSONObject("activity")
        ComposerMember(it.text("id"), it.text("name"), activityLabel(activity),
            activity?.text("state") == "failed", it.text("session_id"),
            activity?.text("state") in setOf("live", "thinking", "tool_finished", "tools_started", "tools_waiting"),
            activitySteps(activity))
    }.filter { it.working || it.failed }
    return remember(members) { ConversationActivity(members) }
}

/**
 * The trailing list item: right after the latest message, at the message column's
 * width, scrolling with the messages. It fades in and out without moving; the list
 * owns tail following, so a reader who scrolled up is never moved by it.
 */
@Composable
internal fun ConversationActivityItem(activity: ConversationActivity, history: (String, String) -> Unit) {
    // Keep the last members on screen while the row fades out.
    val shown = remember { arrayOf(activity) }
    if (activity.visible) shown[0] = activity
    androidx.compose.animation.AnimatedVisibility(activity.visible, enter = zorkFadeIn(), exit = zorkFadeOut()) {
        Row(Modifier.fillMaxWidth().padding(top = 22.dp).horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            shown[0].members.forEach { member -> MemberCapsule(member, history) }
        }
    }
}

/**
 * One line: who, and what they are doing now — the tool in ink, its object muted.
 * "Thinking" alone is not news, so it adds no text. Expanding lists the latest
 * steps in a menu, so the list row keeps its height. Stopping stays on the
 * composer's own stop button, which is always visible while work runs.
 */
@Composable
private fun MemberCapsule(member: ComposerMember, history: (String, String) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    val working = member.working && !member.failed
    val current = member.steps.lastOrNull()
    val thinkingOnly = member.label.startsWith("正在思考") && current == null
    Box {
        Surface(onClick = { history(member.session, member.name) },
            enabled = member.session.isNotBlank(),
            modifier = Modifier.semantics { contentDescription = "${member.name} · 执行历史 · ${member.label.ifBlank { "空闲" }}" },
            shape = ZorkShapes.Control, color = ZorkColors.Prompt) {
            Row(Modifier.height(40.dp).padding(start = 8.dp, end = if (member.steps.size > 1) 2.dp else 12.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                DeviceMark(member.name, 18.dp)
                Text(member.name, fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Ink, maxLines = 1)
                if (working) Box(Modifier.size(6.dp).workingPulse(true).background(ZorkColors.Accent, CircleShape))
                when {
                    member.failed -> Text("执行失败", fontSize = 12.sp, color = ZorkColors.Danger, maxLines = 1)
                    current != null -> androidx.compose.animation.Crossfade(current, label = "step",
                        animationSpec = androidx.compose.animation.core.tween(if (LocalReducedMotion.current) 0 else ZorkMotion.FAST)) { step ->
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(step.first, fontSize = 12.sp, color = ZorkColors.Ink, maxLines = 1)
                            if (step.second.isNotBlank()) Text(step.second, fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1,
                                overflow = TextOverflow.Ellipsis, fontFamily = FontFamily.Monospace, modifier = Modifier.widthIn(max = 140.dp))
                        }
                    }
                    member.label.isNotBlank() && !thinkingOnly -> Text(member.label.substringBefore(" · "), fontSize = 12.sp,
                        color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 140.dp))
                    !working -> Text("空闲", fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1)
                }
                if (member.steps.size > 1) IconAction(R.drawable.ic_chevron_down, "展开步骤", glyphSize = 14.dp) { expanded = true }
            }
        }
        PlainMenu("最近步骤", expanded, { expanded = false }, 240.dp) {
            member.steps.takeLast(3).forEach { (tool, target) ->
                Row(Modifier.fillMaxWidth().heightIn(min = 36.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(tool, fontSize = 13.sp, color = ZorkColors.Ink, maxLines = 1)
                    Text(target, fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        fontFamily = FontFamily.Monospace)
                }
            }
            ZorkMenuItem("完整历史", false, enabled = member.session.isNotBlank(),
                onClick = { expanded = false; history(member.session, member.name) })
        }
    }
}
