// Unified Zork tokens, aligned with crates/zork-ui/src/design.rs; see docs/design/interface.md.
package ing.zork.android

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

private class Palette(
    val paper: Color,
    val canvas: Color,
    val ink: Color,
    val muted: Color,
    val subtle: Color,
    val border: Color,
    val fieldBorder: Color,
    val composer: Color,
    val bubble: Color,
    val pressed: Color,
    val prompt: Color,
    val selected: Color,
    val online: Color,
    val sendDisabled: Color,
    val sendPressed: Color,
    val disabled: Color,
    val warning: Color,
    val danger: Color,
    val accent: Color,
    val accentPressed: Color,
    val warningSoft: Color,
    val dangerSoft: Color,
    val successSoft: Color,
)

private val Light = Palette(
    paper = Color(0xFFF4F2ED),
    canvas = Color(0xFFFFFFFF),
    ink = Color(0xFF24272B),
    muted = Color(0xFF575C62),
    subtle = Color(0xFF676C72),
    border = Color(0xFFE7E3DB),
    fieldBorder = Color(0xFFD8D3C9),
    composer = Color(0xFFF4F2ED),
    bubble = Color(0xFFF1EEE8),
    pressed = Color(0xFFEDEAE3),
    prompt = Color(0xFFF1EEE8),
    selected = Color(0xFFE4DFD4),
    online = Color(0xFF1F7A4D),
    sendDisabled = Color(0xFFB9B5AD),
    sendPressed = Color(0xFF3B3F44),
    disabled = Color(0xFFB9B5AD),
    warning = Color(0xFF935800),
    danger = Color(0xFFB42E35),
    accent = Color(0xFFE9643B),
    accentPressed = Color(0xFFC84A27),
    warningSoft = Color(0xFFFBF1DE),
    dangerSoft = Color(0xFFFBEAEA),
    successSoft = Color(0xFFE6F2EA),
)

// Dark states move away from the surface in lightness, never to pure black or white.
private val Dark = Palette(
    paper = Color(0xFF18191B),
    canvas = Color(0xFF1F2023),
    ink = Color(0xFFECE9E3),
    muted = Color(0xFFB7B3AB),
    subtle = Color(0xFF9C988F),
    border = Color(0xFF2E3034),
    fieldBorder = Color(0xFF3A3D42),
    composer = Color(0xFF18191B),
    bubble = Color(0xFF2B2D31),
    pressed = Color(0xFF2A2C2F),
    prompt = Color(0xFF2B2D31),
    selected = Color(0xFF303236),
    online = Color(0xFF5FC08C),
    sendDisabled = Color(0xFF55585D),
    sendPressed = Color(0xFFC8C3BA),
    disabled = Color(0xFF5E6166),
    warning = Color(0xFFE3A84A),
    danger = Color(0xFFF27E83),
    accent = Color(0xFFF0764E),
    accentPressed = Color(0xFFF59C7C),
    warningSoft = Color(0xFF332919),
    dangerSoft = Color(0xFF3A2224),
    successSoft = Color(0xFF1B2D23),
)

/**
 * Colors read through Compose state: any composable that reads a color
 * recomposes when the theme changes, without rebuilding the page or losing
 * drafts and scroll positions.
 */
internal object ZorkColors {
    private var palette by mutableStateOf(Light)
    var dark: Boolean
        get() = palette === Dark
        set(value) { palette = if (value) Dark else Light }

    val Paper get() = palette.paper
    val Canvas get() = palette.canvas
    val Ink get() = palette.ink
    val Muted get() = palette.muted
    val Subtle get() = palette.subtle
    val Border get() = palette.border
    val FieldBorder get() = palette.fieldBorder
    val Composer get() = palette.composer
    val Bubble get() = palette.bubble
    val Pressed get() = palette.pressed
    val Prompt get() = palette.prompt
    val Selected get() = palette.selected
    val Online get() = palette.online
    val SendDisabled get() = palette.sendDisabled
    val SendPressed get() = palette.sendPressed
    val Disabled get() = palette.disabled
    val Warning get() = palette.warning
    val Danger get() = palette.danger
    /** Persimmon: work in progress and sending only. */
    val Accent get() = palette.accent
    val AccentPressed get() = palette.accentPressed
    val WarningSoft get() = palette.warningSoft
    val DangerSoft get() = palette.dangerSoft
    val SuccessSoft get() = palette.successSoft

    // Timeline accents shared with the desktop history view.
    val HistoryInput = Color(0xFF2878CE)
    val HistoryModel = Color(0xFF8056C4)
    val HistoryTool = Color(0xFF21865B)
    val HistoryError = Color(0xFFD43D45)
    val HistoryBreak = Color(0xFF9CA3AF)
}

/** Corner radii by role, matching `design::RADIUS` on the desktop. */
internal object ZorkShapes {
    /** Buttons, fields, chips and touch controls: capsules. */
    val Control = RoundedCornerShape(percent = 50)
    /** Embedded blocks that can grow: code, attachments, multi-line fields. */
    val Block = RoundedCornerShape(16.dp)
    /** Cards, menus and banners. */
    val Container = RoundedCornerShape(24.dp)
    /** Bottom sheets and dialogs. */
    val Sheet = RoundedCornerShape(topStart = 32.dp, topEnd = 32.dp)
    val Surface = RoundedCornerShape(32.dp)
    /** Outgoing message bubble; the folded corner echoes the brand mark. */
    val Bubble = RoundedCornerShape(24.dp, 24.dp, 8.dp, 24.dp)
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

/** Every Material3 role is assigned, so no library default (purple) can leak through. */
private fun scheme(dark: Boolean): ColorScheme {
    val p = if (dark) Dark else Light
    val scrim = Color.Black.copy(alpha = if (dark) 0.55f else 0.38f)
    return if (dark) {
        darkColorScheme(
            primary = p.ink, onPrimary = p.canvas, primaryContainer = p.selected, onPrimaryContainer = p.ink,
            inversePrimary = p.canvas,
            secondary = p.muted, onSecondary = p.canvas, secondaryContainer = p.selected, onSecondaryContainer = p.ink,
            tertiary = p.accent, onTertiary = p.canvas, tertiaryContainer = p.warningSoft, onTertiaryContainer = p.ink,
            background = p.paper, onBackground = p.ink, surface = p.canvas, onSurface = p.ink,
            surfaceVariant = p.prompt, onSurfaceVariant = p.muted, surfaceTint = Color.Transparent,
            inverseSurface = p.ink, inverseOnSurface = p.canvas,
            error = p.danger, onError = p.canvas, errorContainer = p.dangerSoft, onErrorContainer = p.danger,
            outline = p.fieldBorder, outlineVariant = p.border, scrim = scrim,
            surfaceBright = p.prompt, surfaceDim = p.paper,
            surfaceContainerLowest = p.paper, surfaceContainerLow = p.canvas, surfaceContainer = p.canvas,
            surfaceContainerHigh = p.canvas, surfaceContainerHighest = p.prompt,
        )
    } else {
        lightColorScheme(
            primary = p.ink, onPrimary = p.canvas, primaryContainer = p.selected, onPrimaryContainer = p.ink,
            inversePrimary = p.canvas,
            secondary = p.muted, onSecondary = p.canvas, secondaryContainer = p.selected, onSecondaryContainer = p.ink,
            tertiary = p.accent, onTertiary = p.canvas, tertiaryContainer = p.warningSoft, onTertiaryContainer = p.ink,
            background = p.paper, onBackground = p.ink, surface = p.canvas, onSurface = p.ink,
            surfaceVariant = p.prompt, onSurfaceVariant = p.muted, surfaceTint = Color.Transparent,
            inverseSurface = p.ink, inverseOnSurface = p.canvas,
            error = p.danger, onError = p.canvas, errorContainer = p.dangerSoft, onErrorContainer = p.danger,
            outline = p.fieldBorder, outlineVariant = p.border, scrim = scrim,
            surfaceBright = p.canvas, surfaceDim = p.paper,
            surfaceContainerLowest = p.canvas, surfaceContainerLow = p.canvas, surfaceContainer = p.canvas,
            surfaceContainerHigh = p.canvas, surfaceContainerHighest = p.prompt,
        )
    }
}

private val PressIndication = ZorkPressIndication { ZorkColors.Pressed }

@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
internal fun ZorkTheme(preference: String = "system", content: @Composable () -> Unit) {
    val system = isSystemInDarkTheme()
    val dark = when (preference) { "light" -> false; "dark" -> true; else -> system }
    // Set before children read colors, so the first frame of a switch is consistent.
    if (ZorkColors.dark != dark) ZorkColors.dark = dark
    MaterialTheme(
        colorScheme = scheme(dark),
        typography = ZorkFonts.Styles,
        shapes = Shapes(
            extraSmall = ZorkShapes.Block,
            small = ZorkShapes.Block,
            medium = ZorkShapes.Container,
            large = ZorkShapes.Container,
            extraLarge = ZorkShapes.Surface,
        ),
    ) {
        // Pressing shows a flat pressed fill everywhere, Material components included: no ripple.
        CompositionLocalProvider(
            LocalContentColor provides ZorkColors.Ink,
            LocalTextStyle provides TextStyle(fontFamily = ZorkFonts.Body, fontSize = 15.sp),
            androidx.compose.foundation.LocalIndication provides PressIndication,
            androidx.compose.material3.LocalRippleConfiguration provides null,
            LocalReducedMotion provides rememberReducedMotion(),
        ) { content() }
    }
}
