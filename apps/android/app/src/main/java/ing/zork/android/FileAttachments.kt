package ing.zork.android

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** A picked file that core has not accepted yet: copying, or failed with a reason. */
internal data class PendingFileUi(val key: String, val name: String, val error: String? = null, val session: String = "")

/** Why a message file can or cannot be read right now; core owns the facts. */
internal data class FileAvailability(val deviceName: String = "", val online: Boolean = true,
    val revoked: Boolean = false, val fetching: String? = null)

/**
 * Loads verified image bytes through core and decodes them bounded. `message`
 * is null for a draft file. The ViewModel caches results.
 */
internal val LocalFileImages = staticCompositionLocalOf<suspend (String?, ChatFileUi, Int) -> ImageBitmap?> { { _, _, _ -> null } }

@Composable
private fun rememberFileImage(message: String?, file: ChatFileUi, maxSide: Int): ImageBitmap? {
    val load = LocalFileImages.current
    val image by produceState<ImageBitmap?>(null, message, file.id, maxSide) {
        value = if (file.thumbnail) runCatching { load(message, file, maxSide) }.getOrNull() else null
    }
    return image
}

/** Draft files as a horizontally scrolling row of 48 dp capsules above the editor. */
@Composable
internal fun DraftFileChips(files: List<ChatFileUi>, pending: List<PendingFileUi>,
    open: (ChatFileUi) -> Unit, remove: (String) -> Unit, dismissPending: (String) -> Unit) {
    if (files.isEmpty() && pending.isEmpty()) return
    val scroll = rememberScrollState()
    Row(Modifier.fillMaxWidth()
        .graphicsLayer(compositingStrategy = CompositingStrategy.Offscreen)
        .drawWithContent {
            drawContent()
            // The trailing fade says more files continue off the edge.
            if (scroll.canScrollForward) drawRect(Brush.horizontalGradient(0.85f to Color.Black, 1f to Color.Transparent),
                blendMode = BlendMode.DstIn)
        }
        .horizontalScroll(scroll).padding(start = 4.dp, end = 4.dp, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        files.forEach { file -> key(file.id) { DraftChip(file, open, remove) } }
        pending.forEach { file -> key(file.key) { PendingChip(file, dismissPending) } }
    }
}

@Composable
private fun ChipShell(color: Color, label: String, onClick: (() -> Unit)?, content: @Composable RowScope.() -> Unit) {
    Row(Modifier.height(48.dp).widthIn(max = 260.dp).clip(ZorkShapes.Control).background(color)
        .then(if (onClick != null) Modifier.zorkPressable(onClick = onClick) else Modifier)
        .semantics(mergeDescendants = false) { contentDescription = label }
        .padding(4.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp), content = content)
}

@Composable
private fun ChipText(name: String, meta: String, metaColor: Color = ZorkColors.Muted, modifier: Modifier) {
    Column(modifier) {
        Text(name, fontSize = 14.sp, lineHeight = 18.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(meta, fontSize = 12.sp, lineHeight = 16.sp, color = metaColor, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun ChipRemove(label: String, onClick: () -> Unit) {
    Box(Modifier.size(width = 44.dp, height = 40.dp).clip(CircleShape)
        .zorkPressable(onClick = onClick).semantics { contentDescription = label }, contentAlignment = Alignment.Center) {
        Glyph(R.drawable.ic_x, 16.dp, ZorkColors.Muted)
    }
}

@Composable
private fun DraftChip(file: ChatFileUi, open: (ChatFileUi) -> Unit, remove: (String) -> Unit) {
    ChipShell(ZorkColors.Canvas, "${file.name} · ${file.size}", { open(file) }) {
        // 40 dp inside a 48 dp capsule with 4 dp padding: a concentric circle.
        Box(Modifier.size(40.dp).clip(CircleShape).background(ZorkColors.Prompt), contentAlignment = Alignment.Center) {
            val image = rememberFileImage(null, file, 160)
            if (image != null) Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            else Glyph(if (file.image) R.drawable.ic_image else R.drawable.ic_file, 18.dp, ZorkColors.Muted)
        }
        ChipText(file.name, listOf(file.badge.takeIf { !file.image }, file.size).filterNotNull().filter { it.isNotBlank() }.joinToString(" · "),
            modifier = Modifier.weight(1f, fill = false))
        ChipRemove("移除 ${file.name}") { remove(file.id) }
    }
}

@Composable
private fun PendingChip(file: PendingFileUi, dismiss: (String) -> Unit) {
    val failed = file.error != null
    ChipShell(if (failed) ZorkColors.DangerSoft else ZorkColors.Canvas, file.name, null) {
        Box(Modifier.size(40.dp).clip(CircleShape).background(ZorkColors.Prompt), contentAlignment = Alignment.Center) {
            if (failed) Glyph(R.drawable.ic_attention, 18.dp, ZorkColors.Danger)
            else CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = ZorkColors.Ink, trackColor = ZorkColors.Border)
        }
        ChipText(file.name, file.error ?: "读取中", if (failed) ZorkColors.Danger else ZorkColors.Muted, Modifier.weight(1f, fill = false))
        if (failed) ChipRemove("移除 ${file.name}") { dismiss(file.key) } else Spacer(Modifier.width(8.dp))
    }
}

/** Three system sources; none needs a storage permission. */
@Composable
internal fun AttachSheet(dismiss: () -> Unit, photos: () -> Unit, camera: () -> Unit, files: () -> Unit) {
    SettingsSheet("添加到草稿", dismiss = dismiss) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            AttachTile(R.drawable.ic_image, "照片", Modifier.weight(1f)) { dismiss(); photos() }
            AttachTile(R.drawable.ic_camera, "拍照", Modifier.weight(1f)) { dismiss(); camera() }
            AttachTile(R.drawable.ic_file, "文件", Modifier.weight(1f)) { dismiss(); files() }
        }
        Text("照片用系统照片选择器，不申请相册权限；文件用系统文件选择器。", fontSize = 12.sp, lineHeight = 18.sp,
            color = ZorkColors.Muted, modifier = Modifier.padding(top = 16.dp))
    }
}

@Composable
private fun AttachTile(icon: Int, label: String, modifier: Modifier, onClick: () -> Unit) {
    Column(modifier.clip(ZorkShapes.Container).background(ZorkColors.Prompt).zorkPressable(role = Role.Button, onClick = onClick)
        .padding(vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.size(44.dp).background(ZorkColors.Canvas, CircleShape), contentAlignment = Alignment.Center) { Glyph(icon, 20.dp) }
        Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium)
    }
}

/**
 * Files of one message: images keep their ratio (a 2-column grid when there
 * are several); other files are 16 dp blocks. Tapping opens the preview.
 */
@Composable
internal fun MessageFiles(message: String, files: List<ChatFileUi>, user: Boolean, state: FileAvailability,
    open: (String) -> Unit, save: (String) -> Unit) {
    if (files.isEmpty()) return
    val images = files.filter { it.thumbnail }
    val others = files.filterNot { it.thumbnail }
    Column(Modifier.then(if (user) Modifier.widthIn(max = 280.dp) else Modifier.fillMaxWidth()).padding(top = 8.dp),
        horizontalAlignment = if (user) Alignment.End else Alignment.Start, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (images.size == 1) MessageImage(message, images[0], state, if (user) Modifier.widthIn(max = 220.dp) else Modifier.fillMaxWidth()) { open(images[0].id) }
        else if (images.size > 1) Column(Modifier.then(if (user) Modifier.width(240.dp) else Modifier.fillMaxWidth()).clip(ZorkShapes.Block),
            verticalArrangement = Arrangement.spacedBy(4.dp)) {
            images.chunked(2).forEach { pair ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    pair.forEach { file -> GridImage(message, file, state, Modifier.weight(1f)) { open(file.id) } }
                    if (pair.size == 1) Spacer(Modifier.weight(1f))
                }
            }
        }
        others.forEach { file -> FileBlock(file, state, { open(file.id) }, { save(file.id) },
            if (user) Modifier.widthIn(max = 280.dp) else Modifier.fillMaxWidth()) }
    }
}

@Composable
private fun MessageImage(message: String, file: ChatFileUi, state: FileAvailability, modifier: Modifier, open: () -> Unit) {
    val image = rememberFileImage(message, file, 1024)
    val ratio = image?.let { it.width.toFloat() / it.height.coerceAtLeast(1) }?.coerceIn(0.4f, 2.5f) ?: (4f / 3f)
    Box(modifier.aspectRatio(ratio).clip(ZorkShapes.Block).background(ZorkColors.Prompt)
        .zorkPressable(enabled = !state.revoked, onClick = open).semantics { contentDescription = file.name },
        contentAlignment = Alignment.Center) {
        if (image != null) Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        else ImagePlaceholder(file, state)
    }
}

@Composable
private fun GridImage(message: String, file: ChatFileUi, state: FileAvailability, modifier: Modifier, open: () -> Unit) {
    val image = rememberFileImage(message, file, 512)
    Box(modifier.aspectRatio(1f).background(ZorkColors.Prompt)
        .zorkPressable(enabled = !state.revoked, onClick = open).semantics { contentDescription = file.name },
        contentAlignment = Alignment.Center) {
        if (image != null) Image(image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        else ImagePlaceholder(file, state)
    }
}

@Composable
private fun ImagePlaceholder(file: ChatFileUi, state: FileAvailability) {
    Column(Modifier.padding(12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Glyph(R.drawable.ic_image, 20.dp, ZorkColors.Muted)
        val reason = availabilityText(state)
        if (reason != null) Text(reason, fontSize = 12.sp, color = if (state.revoked) ZorkColors.Danger else ZorkColors.Muted, maxLines = 2)
        else Text(file.size, fontSize = 12.sp, color = ZorkColors.Muted)
    }
}

private fun availabilityText(state: FileAvailability): String? = when {
    state.revoked -> "来源设备已吊销，不能再读取"
    !state.online -> "${state.deviceName.ifBlank { "来源设备" }} 离线 · 上线后可以打开"
    else -> null
}

/** Non-image file: type badge, name, size or state, and a separate save button. */
@Composable
internal fun FileBlock(file: ChatFileUi, state: FileAvailability, open: () -> Unit, save: (() -> Unit)?, modifier: Modifier = Modifier) {
    val fetching = state.fetching == file.id
    Row(modifier.heightIn(min = 64.dp).clip(ZorkShapes.Block).background(ZorkColors.Canvas)
        .border(UiTokens.Border, if (state.revoked) ZorkColors.DangerSoft else ZorkColors.Border, ZorkShapes.Block)
        .zorkPressable(enabled = !state.revoked, onClick = open)
        .semantics { contentDescription = listOfNotNull(file.name, file.size, availabilityText(state)).joinToString(" · ") }
        .padding(start = 12.dp, top = 12.dp, bottom = 12.dp, end = if (save != null) 4.dp else 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        FileBadge(file.badge)
        Column(Modifier.weight(1f)) {
            Text(file.name, fontSize = 14.sp, lineHeight = 18.sp, fontWeight = FontWeight.Medium,
                color = if (state.revoked) ZorkColors.Muted else ZorkColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
            val reason = availabilityText(state)
            when {
                fetching -> {
                    Text("正在获取 · ${file.size}", fontSize = 12.sp, color = ZorkColors.Muted)
                    LinearProgressIndicator(Modifier.padding(top = 6.dp).fillMaxWidth().height(3.dp).clip(ZorkShapes.Control),
                        color = ZorkColors.Ink, trackColor = ZorkColors.Border)
                }
                // A short status pill; the full reason is read out with the block.
                reason != null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(file.size, fontSize = 12.sp, color = ZorkColors.Muted)
                    Text(if (state.revoked) "已吊销" else "离线", fontSize = 12.sp, maxLines = 1,
                        color = if (state.revoked) ZorkColors.Danger else ZorkColors.Muted,
                        modifier = Modifier.background(if (state.revoked) ZorkColors.DangerSoft else ZorkColors.Prompt, ZorkShapes.Control)
                            .padding(horizontal = 8.dp, vertical = 1.dp))
                }
                else -> Text(file.size, fontSize = 12.sp, color = ZorkColors.Muted)
            }
        }
        if (save != null && !state.revoked) IconAction(R.drawable.ic_download, "保存 ${file.name}", enabled = state.online,
            glyphSize = 18.dp, onClick = save)
    }
}
