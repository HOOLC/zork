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
    val Card = androidx.compose.foundation.shape.RoundedCornerShape(UiTokens.CompactRadius)
    val Field = androidx.compose.foundation.shape.RoundedCornerShape(UiTokens.FieldRadius)
    val Pill = androidx.compose.foundation.shape.RoundedCornerShape(UiTokens.PillRadius)
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
    open: Boolean? = null, onClosed: () -> Unit = dismiss, content: @Composable ColumnScope.() -> Unit) {
    val scroll=rememberScrollState()
    var localOpen by remember { mutableStateOf(true) }
    val visible = open ?: localOpen
    val close = { if (!busy || dismissWhileBusy) { if (open == null) localOpen = false else dismiss() } }
    LaunchedEffect(error) { if(error!=null) scroll.animateScrollTo(0) }
    ZorkSheet(visible, title, close, onClosed = onClosed) {
        Column(Modifier.fillMaxWidth().heightIn(max = (androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp * .86f).dp)
            .then(if (footer == null) Modifier.verticalScroll(scroll) else Modifier), verticalArrangement = Arrangement.spacedBy(20.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                ZorkIconButton("关闭", enabled = visible && (!busy || dismissWhileBusy), onClick = close) { Icon(painterResource(R.drawable.ic_x), null, Modifier.size(18.dp)) }
            }
            if (footer == null) {
                if (error != null) Text(error, color = ZorkColors.Danger, fontSize = 13.sp, modifier = Modifier.fillMaxWidth().background(ZorkColors.Prompt, SettingsStyle.Field).padding(12.dp))
                content()
            } else {
                Column(Modifier.weight(1f, fill = false).verticalScroll(scroll), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                    if (error != null) Text(error, color = ZorkColors.Danger, fontSize = 13.sp, modifier = Modifier.fillMaxWidth().background(ZorkColors.Prompt, SettingsStyle.Field).padding(12.dp))
                    content()
                }
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp), content = footer)
            }
        }
    }
}
@Composable internal fun ProviderMark(provider: String, size: Int = 24) {
    val id = when(provider) { "openai" -> R.drawable.provider_openai; "anthropic" -> R.drawable.provider_anthropic; "github-copilot" -> R.drawable.provider_githubcopilot; "kimi", "kimi-coding" -> R.drawable.provider_kimi; "openrouter" -> R.drawable.provider_openrouter; "xai" -> R.drawable.provider_xai; "opencode-go" -> R.drawable.provider_opencode; else -> R.drawable.provider_compatible }
    Image(painterResource(id), null, Modifier.size(size.dp))
}

@Composable
internal fun SettingsListGroup(content: @Composable ColumnScope.() -> Unit) {
    ZorkCard(Modifier.fillMaxWidth(), color = ZorkColors.Paper, outlined = false) {
        Column(content = content)
    }
}

@Composable
internal fun SettingsListDivider() {
    HorizontalDivider(Modifier.padding(start = 60.dp, end = 16.dp), thickness = 0.5.dp, color = ZorkColors.FieldBorder)
}

@Composable
internal fun SettingsListRow(
    label: String,
    icon: Int? = null,
    avatar: String? = null,
    subtext: String? = null,
    detail: String? = null,
    value: String? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    action: (() -> Unit)? = null,
) {
    ZorkListRow(Modifier.fillMaxWidth().heightIn(min = 56.dp), onClick = action) {
        if (leading != null || avatar != null || icon != null) Box(Modifier.size(32.dp), contentAlignment = Alignment.Center) {
            when {
                leading != null -> leading()
                avatar != null -> Avatar(avatar, 32.dp)
                icon != null -> Icon(painterResource(icon), null, Modifier.size(24.dp), tint = ZorkColors.Muted)
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(label, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            subtext?.let { Text(it, fontSize = 11.sp, color = ZorkColors.Muted, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            detail?.let { Text(it, fontSize = 11.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
        value?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
        trailing?.invoke()
    }
}

@Composable
internal fun SettingsToggle(label: String, checked: Boolean, enabled: Boolean = true, detail: String? = null, change: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(label, fontSize = 14.sp)
            detail?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
        }
        ZorkSwitch(checked, change, enabled = enabled, modifier = Modifier.semantics { contentDescription = label })
    }
}
