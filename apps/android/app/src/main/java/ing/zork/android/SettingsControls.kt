package ing.zork.android

import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// Zork styling on native Compose controls and touch input.
internal object SettingsStyle {
    val Card = ZorkShapes.Container
    val Field = ZorkShapes.Container
    val Pill = ZorkShapes.Control
}
@Composable internal fun SettingsButton(text: String, primary: Boolean = false, enabled: Boolean = true, click: () -> Unit) {
    ZorkButton(text, if (primary) Modifier.fillMaxWidth() else Modifier, primary, enabled, onClick = click)
}
@Composable internal fun SettingsField(label: String, value: String, change: (String) -> Unit, secret: Boolean = false, enabled: Boolean = true,
    error: String? = null, detail: String? = null, singleLine: Boolean = true) {
    ZorkTextField(label, value, change, secret = secret, enabled = enabled, error = error, detail = detail, singleLine = singleLine)
}
@Composable internal fun SettingsSelect(label: String, selected: String, options: List<Pair<String,String>>, enabled: Boolean = true, choose: (String) -> Unit) {
    ZorkChoiceField(label, options.find { it.first == selected }?.second ?: "请选择",
        options, setOf(selected), enabled = enabled, choose = choose)
}
@Composable internal fun SettingsSegments(options: List<Pair<String,String>>, selected: String, enabled: Boolean = true, choose: (String) -> Unit) {
    ZorkSegments(options, selected, choose, enabled = enabled)
}
@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun SettingsSheet(title: String, busy: Boolean = false, error: String? = null, dismiss: () -> Unit,
    footer: (@Composable ColumnScope.() -> Unit)? = null, dismissWhileBusy: Boolean = false,
    open: Boolean? = null, onClosed: () -> Unit = dismiss,
    /** The title is an identifier (a model id): monospace, one line. */
    monoTitle: Boolean = false, back: (() -> Boolean)? = null, content: @Composable ColumnScope.() -> Unit) {
    val scroll=rememberScrollState()
    var localOpen by remember { mutableStateOf(true) }
    val visible = open ?: localOpen
    val close = { if (!busy || dismissWhileBusy) { if (open == null) localOpen = false else dismiss() } }
    LaunchedEffect(error) { if(error!=null) scroll.animateScrollTo(0) }
    ZorkSheet(visible, title, close, onClosed = onClosed, canDismiss = !busy || dismissWhileBusy, back = back) {
        Column(Modifier.fillMaxWidth().heightIn(max = (androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp * .86f).dp)
            .then(if (footer == null) Modifier.verticalScroll(scroll) else Modifier), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, fontSize = if (monoTitle) 16.sp else 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f),
                    fontFamily = if (monoTitle) androidx.compose.ui.text.font.FontFamily.Monospace else null,
                    maxLines = if (monoTitle) 1 else Int.MAX_VALUE, overflow = TextOverflow.Ellipsis)
                ZorkIconButton("关闭", enabled = visible && (!busy || dismissWhileBusy), onClick = close) { Icon(painterResource(R.drawable.ic_x), null, Modifier.size(18.dp)) }
            }
            if (footer == null) {
                if (error != null) Text(error, color = ZorkColors.Danger, fontSize = 13.sp, lineHeight = 19.sp, modifier = Modifier.fillMaxWidth().background(ZorkColors.DangerSoft, ZorkShapes.Container).padding(horizontal = 18.dp, vertical = 12.dp))
                content()
            } else {
                Column(Modifier.weight(1f, fill = false).verticalScroll(scroll), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    if (error != null) Text(error, color = ZorkColors.Danger, fontSize = 13.sp, lineHeight = 19.sp, modifier = Modifier.fillMaxWidth().background(ZorkColors.DangerSoft, ZorkShapes.Container).padding(horizontal = 18.dp, vertical = 12.dp))
                    content()
                }
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp), content = footer)
            }
        }
    }
}
/** Mark of a connection's provider: anything that names a connection or service. */
internal fun providerDrawable(provider: String) = when(provider) { "openai" -> R.drawable.provider_openai; "anthropic" -> R.drawable.provider_anthropic; "github-copilot" -> R.drawable.provider_githubcopilot; "kimi", "kimi-coding" -> R.drawable.provider_kimi; "openrouter" -> R.drawable.provider_openrouter; "xai" -> R.drawable.provider_xai; "opencode-go" -> R.drawable.provider_opencode; else -> R.drawable.provider_compatible }
@Composable internal fun ProviderMark(provider: String, size: Int = 24) {
    Image(painterResource(providerDrawable(provider)), null, Modifier.size(size.dp))
}
/** Mark of a model's maker (core `model_catalog::MAKERS` key): anything that names a model. */
internal fun makerDrawable(maker: String?) = when(maker) { "openai" -> R.drawable.maker_openai; "anthropic" -> R.drawable.maker_anthropic; "google" -> R.drawable.maker_google; "deepseek" -> R.drawable.maker_deepseek; "qwen" -> R.drawable.maker_qwen; "zhipu" -> R.drawable.maker_zhipu; "doubao" -> R.drawable.maker_doubao; "moonshot" -> R.drawable.maker_moonshot; "minimax" -> R.drawable.maker_minimax; "mistral" -> R.drawable.maker_mistral; "meta" -> R.drawable.maker_meta; "xai" -> R.drawable.maker_xai; else -> R.drawable.maker_generic }
@Composable internal fun MakerMark(maker: String?, size: Int = 18, tint: androidx.compose.ui.graphics.Color = ZorkColors.Ink) {
    Glyph(makerDrawable(maker), size.dp, tint)
}

@Composable
internal fun SettingsListGroup(content: @Composable ColumnScope.() -> Unit) {
    // Rows sit directly on the page surface; groups are separated by titles and spacing.
    Column(Modifier.fillMaxWidth(), content = content)
}

@Composable
internal fun SettingsListDivider(@Suppress("UNUSED_PARAMETER") inset: androidx.compose.ui.unit.Dp = 60.dp) {
    // No divider lines: rows are separated by their own height.
}

@Composable
internal fun SettingsListRow(
    label: String,
    icon: Int? = null,
    subtext: String? = null,
    detail: String? = null,
    value: String? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    action: (() -> Unit)? = null,
) {
    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 52.dp), onClick = action) {
        if (leading != null || icon != null) Box(Modifier.size(32.dp), contentAlignment = Alignment.Center) {
            when {
                leading != null -> leading()
                icon != null -> Icon(painterResource(icon), null, Modifier.size(24.dp), tint = ZorkColors.Muted)
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            subtext?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            detail?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
        value?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
        trailing?.invoke()
    }
}

@Composable
internal fun SettingsToggle(label: String, checked: Boolean, enabled: Boolean = true, detail: String? = null, change: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 52.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, fontSize = 15.sp)
            detail?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
        }
        ZorkSwitch(checked, change, enabled = enabled, modifier = Modifier.semantics { contentDescription = label })
    }
}
