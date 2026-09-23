package ing.zork.android

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

/** Keep the last value until its standard Compose overlay has closed. */
@Composable
internal fun <T : Any> ZorkRetained(
    value: T?, content: @Composable (T, Boolean, () -> Unit) -> Unit,
) {
    var retained by remember { mutableStateOf<T?>(null) }
    val current by rememberUpdatedState(value)
    SideEffect { if (value != null) retained = value }
    val shown = value ?: retained ?: return
    content(shown, value != null) { if (current == null) retained = null }
}

@Composable
internal fun ZorkDialog(
    open: Boolean, title: String, dismiss: () -> Unit,
    content: @Composable ColumnScope.() -> Unit,
) {
    if (!open) return
    Dialog(onDismissRequest = dismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.padding(16.dp).widthIn(max = 640.dp).fillMaxWidth()
            .semantics { paneTitle = title }, shape = RoundedCornerShape(UiTokens.CardRadius),
            color = ZorkColors.Canvas, border = BorderStroke(UiTokens.Border, UiTokens.Outline)) {
            Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(16.dp),
                content = content)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ZorkSheet(
    open: Boolean, title: String, dismiss: () -> Unit, onClosed: () -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val closed by rememberUpdatedState(onClosed)
    LaunchedEffect(open) { if (!open) closed() }
    if (!open) return
    ModalBottomSheet(onDismissRequest = dismiss,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RoundedCornerShape(topStart = UiTokens.CardRadius, topEnd = UiTokens.CardRadius),
        containerColor = ZorkColors.Canvas,
        dragHandle = null) {
        Column(Modifier.fillMaxWidth().widthIn(max = 640.dp)
            .imePadding().navigationBarsPadding().padding(24.dp)
            .semantics { paneTitle = title },
            verticalArrangement = Arrangement.spacedBy(16.dp), content = content)
    }
}

internal object PlainMenuStyle {
    val Radius = 16.dp
    val RowRadius = Radius - 6.dp
}

@Composable
internal fun PlainMenu(
    label: String, expanded: Boolean, dismiss: () -> Unit, width: Dp,
    anchor: androidx.compose.ui.geometry.Rect? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    DropdownMenu(expanded, onDismissRequest = dismiss,
        modifier = Modifier.widthIn(min = width.coerceAtLeast(160.dp)).heightIn(max = 320.dp)
            .semantics { paneTitle = label }, content = content)
}

@Composable
internal fun ZorkDisclosure(
    title: String, expanded: Boolean, change: (Boolean) -> Unit,
    modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier) {
        ZorkButton(title, Modifier.fillMaxWidth(), onClick = { change(!expanded) })
        AnimatedVisibility(expanded) {
            Column(Modifier.fillMaxWidth().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
        }
    }
}
