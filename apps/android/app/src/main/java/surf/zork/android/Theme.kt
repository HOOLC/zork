// Approved Zork workbench tokens; custom Compose geometry follows mobile/prototype nav7.
package surf.zork.android

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.LocalContentColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.font.FontVariation

// Mobile nav7 tokens from zork-design/mobile/prototype/style.css and navigation.css.
internal object ZorkColors {
    val Paper = Color(0xFFF6F5F1)
    val Canvas = Color(0xFFFFFFFF)
    val Ink = Color(0xFF24272B)
    val Muted = Color(0xFF646970)
    val Border = Color(0xFFEEEDEA)
    val FieldBorder = Color(0xFFDEDFDF)
    val Composer = Color(0xFFFBFAF7)
    val Bubble = Color(0xFFFAF9F6)
    val Pressed = Color(0xFFEFEEEA)
    val Prompt = Color(0xFFF5F5F5)
    val Selected = Color(0xFFEAE7E1)
    val Online = Color(0xFF006A3F)
    val SendDisabled = Color(0xFFA7A9AA)
    val SendPressed = Color(0xFF41464C)
    val Disabled = Color(0xFFA8AAAA)
    val Warning = Color(0xFF7F5306)
    val Danger = Color(0xFFB42318)
    // Exact timeline accents from zork-gui/src/views/history.rs::history_color.
    val HistoryInput = Color(0xFF2878CE)
    val HistoryModel = Color(0xFF8056C4)
    val HistoryTool = Color(0xFF21865B)
    val HistoryError = Color(0xFFD43D45)
    val HistoryBreak = Color(0xFF9CA3AF)
}

@OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
internal object ZorkFonts {
    val Body = FontFamily(
        Font(R.font.inter, FontWeight.Normal, variationSettings = FontVariation.Settings(FontVariation.weight(400))),
        Font(R.font.inter, FontWeight.Medium, variationSettings = FontVariation.Settings(FontVariation.weight(500))),
        Font(R.font.inter, FontWeight.SemiBold, variationSettings = FontVariation.Settings(FontVariation.weight(600))),
        Font(R.font.inter, FontWeight.Bold, variationSettings = FontVariation.Settings(FontVariation.weight(700))),
    )
    val Mono = FontFamily.Monospace
    private val base = Typography()
    val Styles = Typography(
        bodyLarge = base.bodyLarge.copy(fontFamily = Body),
        bodyMedium = base.bodyMedium.copy(fontFamily = Body),
        bodySmall = base.bodySmall.copy(fontFamily = Body),
        titleLarge = base.titleLarge.copy(fontFamily = Body),
        titleMedium = base.titleMedium.copy(fontFamily = Body),
        titleSmall = base.titleSmall.copy(fontFamily = Body),
        headlineLarge = base.headlineLarge.copy(fontFamily = Body),
        headlineMedium = base.headlineMedium.copy(fontFamily = Body),
        headlineSmall = base.headlineSmall.copy(fontFamily = Body),
        labelLarge = base.labelLarge.copy(fontFamily = Body),
        labelMedium = base.labelMedium.copy(fontFamily = Body),
        labelSmall = base.labelSmall.copy(fontFamily = Body),
    )
}

@Composable
internal fun ZorkTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = lightColorScheme(
        primary = ZorkColors.Ink,
        onPrimary = ZorkColors.Canvas,
        background = ZorkColors.Paper,
        onBackground = ZorkColors.Ink,
        surface = ZorkColors.Canvas,
        onSurface = ZorkColors.Ink,
        surfaceVariant = ZorkColors.Prompt,
        onSurfaceVariant = ZorkColors.Muted,
        outline = ZorkColors.Muted,
        secondaryContainer = ZorkColors.Selected,
        error = ZorkColors.Danger,
    ), typography = ZorkFonts.Styles) {
        CompositionLocalProvider(LocalContentColor provides ZorkColors.Ink, LocalTextStyle provides TextStyle(fontFamily = ZorkFonts.Body, fontSize = 15.sp)) { LiquidHost(content) }
    }
}
