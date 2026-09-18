package ing.zork.android

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

internal val LocalMessagePreviewHeight = staticCompositionLocalOf { 0 }

@Composable
internal fun AppearanceSettings(actions: SettingsActions, modifier: Modifier = Modifier) {
    val savedHeight = actions.messagePreviewHeight
    var height by remember(savedHeight) { mutableFloatStateOf((savedHeight.takeIf { it > 0 } ?: 192).toFloat()) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var attemptedHeight by remember { mutableIntStateOf(savedHeight) }
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current.density
    fun save(value: Int) {
        attemptedHeight = value
        scope.launch {
            saving = true; error = null
            try { actions.saveMessagePreviewHeight(value) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { error = e.message ?: "保存失败，请重试" }
            finally { saving = false }
        }
    }
    Column(modifier.fillMaxSize().background(ZorkColors.Canvas)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconAction(R.drawable.ic_arrow_left, "返回设置", onClick = actions.back)
            Text("外观", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        }
        Column(Modifier.fillMaxWidth().weight(1f).verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("消息折叠高度", fontSize = 16.sp, fontWeight = FontWeight.Medium)
            Text("拖动预览底部的横线，调整长消息默认显示的高度，松手自动保存。", fontSize = 13.sp, lineHeight = 21.sp, color = ZorkColors.Muted)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                Text(if (savedHeight == 0 && height == 192f) "自动 · 192" else height.roundToInt().toString(), fontSize = 13.sp, color = ZorkColors.Muted)
                SettingsButton("恢复自动", enabled = !saving) { height = 192f; save(0) }
            }
            Column(Modifier.fillMaxWidth().clip(SettingsStyle.Field).background(ZorkColors.Paper)) {
                Box(Modifier.fillMaxWidth().height(height.dp).clipToBounds()) {
                    Text(("这是一条示例消息，你可以用它找到舒适的阅读高度。\n\n先浏览主要内容，遇到感兴趣的细节，再打开完整消息。\n\n向下拖动底部横线，显示更多内容；向上拖动，让消息更紧凑。\n\n").repeat(8),
                        modifier = Modifier.padding(16.dp), fontSize = 15.sp, lineHeight = 24.sp)
                }
                HorizontalDivider(color = ZorkColors.FieldBorder)
                Box(Modifier.fillMaxWidth().height(48.dp)
                    .draggable(rememberDraggableState { delta -> height = (height + delta / density).coerceIn(80f, 720f) },
                        Orientation.Vertical, enabled = !saving, onDragStopped = { save(height.roundToInt()) })
                    .semantics {
                        contentDescription = "拖动调整消息高度"
                        progressBarRangeInfo = ProgressBarRangeInfo(height, 80f..720f)
                        setProgress { value ->
                            if (saving) false else { height = value.coerceIn(80f, 720f); save(height.roundToInt()); true }
                        }
                    }, contentAlignment = Alignment.Center) {
                    Box(Modifier.width(40.dp).height(3.dp).background(ZorkColors.Muted, SettingsStyle.Pill))
                }
            }
            Text(if (saving) "正在保存…" else "仅应用于这台手机。", fontSize = 12.sp, color = ZorkColors.Muted)
            error?.let { message ->
                Text(message, color = ZorkColors.Danger, fontSize = 13.sp)
                SettingsButton("重试保存", enabled = !saving) { save(attemptedHeight) }
            }
        }
    }
}
