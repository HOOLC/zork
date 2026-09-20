package ing.zork.android

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

internal data class ChatFileUi(val id: String, val name: String, val bytes: Long)
internal data class ChatFilePreviewUi(val key: String, val name: String, val mime: String,
    val loading: Boolean, val error: String?, val text: String?, val truncated: Boolean,
    val contentReady: Boolean, val saving: Boolean, val saveTicket: String?, val saved: Boolean)

internal fun parseChatFilePreview(value: JSONObject) = ChatFilePreviewUi(
    value.getString("key"), value.getJSONObject("file").getString("name"), value.getString("mime"),
    value.getBoolean("loading"), value.text("error").takeIf { it.isNotBlank() },
    value.text("text").takeIf { !value.isNull("text") }, value.getBoolean("truncated"),
    value.getBoolean("content_ready"), value.getBoolean("saving"),
    value.text("save_ticket").takeIf { it.isNotBlank() }, value.getBoolean("saved"))

// Decode/display is a platform capability; core owns file selection and bytes.
internal suspend fun decodeFileImage(bytes: ByteArray): ImageBitmap? = withContext(Dispatchers.Default) {
    if (bytes.isEmpty()) return@withContext null
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0 || bounds.outWidth > 8192 || bounds.outHeight > 8192 || bounds.outWidth.toLong() * bounds.outHeight > 16_777_216) return@withContext null
    val options = BitmapFactory.Options().apply { inSampleSize = 1 }
    while (bounds.outWidth / options.inSampleSize > 2048 || bounds.outHeight / options.inSampleSize > 2048) options.inSampleSize *= 2
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)?.asImageBitmap()
}

@Composable
internal fun DeliveredFileCard(file: ChatFileUi, open: () -> Unit) {
    LiquidCard(Modifier.fillMaxWidth().padding(top = 12.dp), color = ZorkColors.Bubble, radius = 10.dp) {
        Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).liquidPressable(onClick = open)
            .padding(horizontal = 11.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Glyph(R.drawable.ic_result, 22.dp)
            Column(Modifier.weight(1f)) {
                Text(file.name, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                Text("${file.bytes} 字节", fontSize = 11.sp, color = ZorkColors.Muted)
            }
            Glyph(R.drawable.ic_download, 19.dp)
        }
    }
}

@Composable
internal fun ChatFilePreview(file: ChatFilePreviewUi, image: ImageBitmap?, close: () -> Unit, save: () -> Unit) {
    SettingsSheet(file.name, dismiss = close) {
        Column(Modifier.fillMaxWidth().heightIn(max = 560.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(16.dp)) {
            if (file.loading) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            file.error?.let { Text(it, color = ZorkColors.Danger) }
            if (image != null) Image(image, file.name, Modifier.fillMaxWidth().heightIn(max = 400.dp))
            else if (file.text != null) SelectionContainer { Text(file.text, fontSize = 14.sp, lineHeight = 23.sp) }
            else if (!file.loading && file.error == null) Text("保存副本后，可使用本机应用打开此文件", color = ZorkColors.Muted)
            if (file.truncated) Text("预览已截取，保存副本可查看完整内容", color = ZorkColors.Muted)
            LiquidButton(if (file.saving) "正在保存…" else "保存副本", primary = true,
                onClick = save, enabled = !file.saving, modifier = Modifier.fillMaxWidth())
            if (file.saved) Text("副本已保存", color = ZorkColors.Muted)
        }
    }
}
