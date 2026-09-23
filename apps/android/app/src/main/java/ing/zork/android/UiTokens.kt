package ing.zork.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/** Zork's colors and sizes applied to standard Compose controls. */
internal object UiTokens {
    val Accent = Color(0xFFE9643B)
    val Outline = Color(0xFFB6BABD)
    val Focus = ZorkColors.Muted
    val NeutralHover = ZorkColors.Pressed
    val NeutralPressed = ZorkColors.Selected
    val AccentHover = Color(0xFFDB572F)
    val AccentPressed = Color(0xFFC84A27)
    val Border = 0.5.dp
    val CardRadius = 12.dp
    val CompactRadius = 12.dp
    val FieldRadius = 10.dp
    val PillRadius = 999.dp
    val IconRadius = 10.dp
}
