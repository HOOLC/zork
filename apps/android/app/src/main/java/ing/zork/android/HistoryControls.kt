package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Same neutral press token as Android IconAction; no platform-default ripple. */
@Composable
internal fun Modifier.historyPress(enabled: Boolean = true, selected: Boolean = false,
    radius: androidx.compose.ui.unit.Dp = 6.dp, label: String? = null, onClick: () -> Unit): Modifier {
    return then(if (selected) Modifier.background(ZorkColors.Selected, LiquidShape(radius)) else Modifier)
        .liquidPressable(enabled = enabled, radius = radius, onClick = onClick)
        .then(if (label == null) Modifier else Modifier.semantics { contentDescription = label })
}

@Composable
internal fun HistoryAction(text: String, enabled: Boolean = true, selected: Boolean = false,
    modifier: Modifier = Modifier, label: String = text, onClick: () -> Unit) {
    Box(modifier.heightIn(min = 44.dp).widthIn(min = 44.dp)
        .historyPress(enabled, selected, LiquidTokens.PillRadius, label, onClick)
        .padding(horizontal = 10.dp, vertical = 8.dp), contentAlignment = Alignment.Center) {
        Text(text, fontSize = 12.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}

@Composable
internal fun HistoryIconAction(icon: Int, label: String, enabled: Boolean = true, onClick: () -> Unit) {
    LiquidIconButton(label, enabled = enabled, onClick = onClick) {
        Glyph(icon, 18.dp, if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}

@Composable
internal fun HistorySubjectLink(subject: HistorySubject, modifier: Modifier = Modifier, click: () -> Unit) {
    // This is a text link, not another filled action inside the clickable row.
    val interactions = remember { MutableInteractionSource() }
    val pressed by interactions.collectIsPressedAsState()
    val focused by interactions.collectIsFocusedAsState()
    Box(modifier.heightIn(min = 44.dp).clickable(enabled = subject.actionable, interactionSource = interactions,
        indication = null, role = Role.Button, onClick = click), contentAlignment = Alignment.CenterStart) {
        Text(subject.label, fontSize = 12.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            color = if (pressed || focused) ZorkColors.Ink else ZorkColors.Muted,
            textDecoration = if (subject.actionable) TextDecoration.Underline else TextDecoration.None)
    }
}

internal fun historyIcon(kind: String): Int = when (kind) {
    "input", "received" -> R.drawable.history_receive; "output", "send_message" -> R.drawable.history_send
    "send_file" -> R.drawable.history_attachment; "notify" -> R.drawable.history_notify
    "assign", "rework", "tasks" -> R.drawable.history_assign; "workers" -> R.drawable.history_agents
    "read" -> R.drawable.history_file_read; "write" -> R.drawable.history_file_write
    "edit" -> R.drawable.history_file_edit; "shell" -> R.drawable.history_terminal
    "browser" -> R.drawable.history_browser; "wait", "thinking" -> R.drawable.history_clock
    "end" -> R.drawable.history_end; "cancel" -> R.drawable.history_stop
    "help" -> R.drawable.history_help; "history", "chat_history" -> R.drawable.history_history
    "job" -> R.drawable.history_job; "error" -> R.drawable.ic_attention
    else -> R.drawable.history_generic_tool
}
