package ing.zork.android

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.rememberTransformableState
import androidx.compose.foundation.gestures.transformable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** One immutable file as core's `file_views` presents it; badges and sizes come from core. */
internal data class ChatFileUi(val id: String, val name: String, val bytes: Long,
    val mime: String = "application/octet-stream", val kind: String = "file", val badge: String = "FILE",
    val size: String = "", val thumbnail: Boolean = false) {
    val image get() = kind == "image"
}
internal fun parseFileView(file: JSONObject) = ChatFileUi(file.text("id"), file.text("name"), file.optLong("byte_len"),
    file.text("mime", "application/octet-stream"), file.text("kind", "file"), file.text("badge", "FILE"),
    file.text("size"), file.optBoolean("thumbnail"))
/** Prefers core's presentation; older cached rows only carry the raw references. */
internal fun JSONObject.fileViews(): List<ChatFileUi> =
    (optJSONArray("file_views") ?: optJSONArray("files")).objects().map(::parseFileView)

internal data class ChatFilePreviewUi(val key: String, val fileId: String, val name: String, val mime: String,
    val loading: Boolean, val error: String?, val text: String?, val truncated: Boolean,
    val contentReady: Boolean, val saving: Boolean, val saveTicket: String?, val saved: Boolean)

internal fun parseChatFilePreview(value: JSONObject) = ChatFilePreviewUi(
    value.getString("key"), value.getJSONObject("file").text("id"), value.getJSONObject("file").getString("name"), value.getString("mime"),
    value.getBoolean("loading"), value.text("error").takeIf { it.isNotBlank() },
    value.text("text").takeIf { !value.isNull("text") }, value.getBoolean("truncated"),
    value.getBoolean("content_ready"), value.getBoolean("saving"),
    value.text("save_ticket").takeIf { it.isNotBlank() }, value.getBoolean("saved"))

// Decode/display is a platform capability; core owns file selection and bytes.
internal suspend fun decodeFileImage(bytes: ByteArray, maxSide: Int = 2048): ImageBitmap? = withContext(Dispatchers.Default) {
    if (bytes.isEmpty()) return@withContext null
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0 || bounds.outWidth > 8192 || bounds.outHeight > 8192 || bounds.outWidth.toLong() * bounds.outHeight > 16_777_216) return@withContext null
    val options = BitmapFactory.Options().apply { inSampleSize = 1 }
    while (bounds.outWidth / options.inSampleSize > maxSide || bounds.outHeight / options.inSampleSize > maxSide) options.inSampleSize *= 2
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
}

/** A delivered file in the Chat files sheet. */
@Composable
internal fun DeliveredFileCard(file: ChatFileUi, state: FileAvailability = FileAvailability(), open: () -> Unit, save: (() -> Unit)? = null) {
    FileBlock(file, state, open, save, Modifier.padding(top = 12.dp))
}

/**
 * Full-screen preview on a warm-gray canvas. Images zoom and pan; text scrolls;
 * other types offer another app or a saved copy. Saving stays in the overflow.
 */
@Composable
internal fun FilePreviewPage(file: ChatFilePreviewUi, info: ChatFileUi?, image: ImageBitmap?, index: Int, count: Int,
    step: (Int) -> Unit, close: () -> Unit, save: () -> Unit, openExternal: () -> Unit) {
    Dialog(close, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(ZorkColors.Prompt).systemBarsPadding()) {
            var menu by remember { mutableStateOf(false) }
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 20.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(file.name, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val meta = listOfNotNull("${index + 1} / $count".takeIf { count > 1 }, info?.size?.takeIf { it.isNotBlank() })
                    if (meta.isNotEmpty()) Text(meta.joinToString(" · "), fontSize = 12.sp, color = ZorkColors.Muted)
                }
                Box {
                    IconAction(R.drawable.ic_more, "更多", glyphSize = 18.dp) { menu = true }
                    PlainMenu("文件操作", menu, { menu = false }, 200.dp) {
                        PreviewMenuItem(R.drawable.ic_download, if (file.saving) "正在保存…" else "保存到…", !file.saving) { menu = false; save() }
                        PreviewMenuItem(R.drawable.ic_arrow_right, "用其他应用打开", !file.saving) { menu = false; openExternal() }
                    }
                }
                IconAction(R.drawable.ic_x, "关闭预览", glyphSize = 18.dp, onClick = close)
            }
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                when {
                    image != null -> ZoomableImage(image, file.name)
                    file.text != null -> ZorkCard(Modifier.fillMaxSize().padding(16.dp), shape = ZorkShapes.Container) {
                        SelectionContainer(Modifier.verticalScroll(rememberScrollState()).padding(20.dp)) {
                            Text(file.text, fontSize = 14.sp, lineHeight = 23.sp)
                        }
                    }
                    file.loading -> Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp, color = ZorkColors.Ink)
                        Text("正在获取文件…", fontSize = 13.sp, color = ZorkColors.Muted)
                    }
                    else -> Column(Modifier.padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        FileBadge(info?.badge ?: "FILE", 64.dp)
                        Text(file.error ?: "这类文件不在应用内预览", fontSize = 14.sp,
                            color = if (file.error != null) ZorkColors.Danger else ZorkColors.Muted)
                        ZorkButton("用其他应用打开", primary = true, enabled = !file.saving, onClick = openExternal)
                        ZorkButton("保存副本", enabled = !file.saving, onClick = save)
                    }
                }
                if (count > 1) {
                    PreviewStep(R.drawable.ic_arrow_left, "上一个", index > 0, Modifier.align(Alignment.CenterStart)) { step(-1) }
                    PreviewStep(R.drawable.ic_arrow_right, "下一个", index < count - 1, Modifier.align(Alignment.CenterEnd)) { step(1) }
                }
            }
            val note = when {
                file.saving -> "正在准备副本…"
                file.saved -> "副本已保存"
                file.truncated -> "预览已截取，保存副本可查看完整内容"
                else -> null
            }
            if (note != null) Text(note, fontSize = 12.sp, color = ZorkColors.Muted,
                modifier = Modifier.align(Alignment.CenterHorizontally).padding(bottom = 16.dp))
            if (file.loading && image == null && file.text == null) LinearProgressIndicator(Modifier.fillMaxWidth().height(2.dp),
                color = ZorkColors.Ink, trackColor = ZorkColors.Border)
        }
    }
}

/** A draft file before sending: images show at full size; other files show what will be sent. */
@Composable
internal fun DraftFilePreview(file: ChatFileUi, close: () -> Unit) {
    val load = LocalFileImages.current
    var image by remember(file.id) { mutableStateOf<ImageBitmap?>(null) }
    var loading by remember(file.id) { mutableStateOf(file.thumbnail) }
    LaunchedEffect(file.id) {
        if (file.thumbnail) { image = runCatching { load(null, file, 2048) }.getOrNull(); loading = false }
    }
    Dialog(close, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(ZorkColors.Prompt).systemBarsPadding()) {
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(start = 20.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(file.name, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text("草稿 · ${file.size}", fontSize = 12.sp, color = ZorkColors.Muted)
                }
                IconAction(R.drawable.ic_x, "关闭预览", glyphSize = 18.dp, onClick = close)
            }
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                val shown = image
                if (shown != null) ZoomableImage(shown, file.name)
                else Column(Modifier.padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    if (loading) CircularProgressIndicator(Modifier.size(28.dp), strokeWidth = 2.dp, color = ZorkColors.Ink)
                    else FileBadge(file.badge, 64.dp)
                    Text("发送后，成员可以在对话中打开这个文件", fontSize = 13.sp, color = ZorkColors.Muted)
                }
            }
        }
    }
}

@Composable
private fun PreviewMenuItem(icon: Int, text: String, enabled: Boolean, onClick: () -> Unit) {
    Row(Modifier.padding(horizontal = 8.dp).fillMaxWidth().heightIn(min = 44.dp)
        .historyPress(enabled = enabled, radius = PlainMenuStyle.RowRadius, onClick = onClick).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Glyph(icon, 18.dp, if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
        Text(text, fontSize = 14.sp, color = if (enabled) ZorkColors.Ink else ZorkColors.Disabled)
    }
}

@Composable
private fun PreviewStep(icon: Int, label: String, enabled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    if (!enabled) return
    Box(modifier.padding(12.dp).size(44.dp).background(ZorkColors.Canvas, CircleShape)
        .zorkPressable(onClick = onClick).semantics { contentDescription = label }, contentAlignment = Alignment.Center) {
        Glyph(icon, 18.dp)
    }
}

@Composable
private fun ZoomableImage(image: ImageBitmap, name: String) {
    var scale by remember(image) { mutableFloatStateOf(1f) }
    var offset by remember(image) { mutableStateOf(Offset.Zero) }
    val transform = rememberTransformableState { zoom, pan, _ ->
        scale = (scale * zoom).coerceIn(1f, 5f)
        offset = if (scale == 1f) Offset.Zero else offset + pan
    }
    Image(image, name, Modifier.fillMaxSize().padding(horizontal = 16.dp)
        .pointerInput(image) { detectTapGestures(onDoubleTap = { scale = if (scale > 1f) 1f else 2.5f; if (scale == 1f) offset = Offset.Zero }) }
        .transformable(transform)
        .graphicsLayer { scaleX = scale; scaleY = scale; translationX = offset.x; translationY = offset.y },
        contentScale = ContentScale.Fit)
}

/** Type badge; the shape stays concentric inside 16 dp file blocks. */
@Composable
internal fun FileBadge(badge: String, size: androidx.compose.ui.unit.Dp = 40.dp) {
    val pdf = badge == "PDF"
    Box(Modifier.size(size).background(if (pdf) ZorkColors.DangerSoft else ZorkColors.Prompt, RoundedCornerShape(size / 4)),
        contentAlignment = Alignment.Center) {
        Text(badge, fontSize = if (size > 48.dp) 16.sp else 12.sp, fontWeight = FontWeight.SemiBold,
            color = if (pdf) ZorkColors.Danger else ZorkColors.Muted, maxLines = 1)
    }
}
