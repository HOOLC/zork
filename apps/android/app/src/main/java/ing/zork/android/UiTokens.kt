package ing.zork.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp

/**
 * Zork's colors and sizes applied to standard Compose controls. Colors are
 * getters so they follow the current theme.
 */
internal object UiTokens {
    val Accent: Color get() = ZorkColors.Accent
    val Outline: Color get() = ZorkColors.FieldBorder
    val Focus: Color get() = ZorkColors.Accent
    val NeutralHover: Color get() = ZorkColors.Pressed
    val NeutralPressed: Color get() = ZorkColors.Selected
    val AccentHover: Color get() = ZorkColors.Accent
    val AccentPressed: Color get() = ZorkColors.AccentPressed
    val Border = 0.5.dp
    /** Cards and menus (`RADIUS.container`). */
    val CardRadius = 24.dp
    /** Embedded blocks (`RADIUS.block`). */
    val CompactRadius = 16.dp
    /** A capsule at the 48 dp field height; taller fields keep a container corner. */
    val FieldRadius = 24.dp
    val PillRadius = 999.dp
    val IconRadius = 999.dp
}
