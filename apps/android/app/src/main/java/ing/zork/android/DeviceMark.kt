package ing.zork.android

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// Same hues and slot rule as `design::device_hue` / `color_slot` on the desktop,
// so a device keeps its color across clients. Hues mark identity only, never status.
private val LightHues = longArrayOf(0xFF56759A, 0xFFA27A2B, 0xFF5E8B6B, 0xFF87618F, 0xFF3E8787, 0xFFA5607A)
private val DarkHues = longArrayOf(0xFF6C8BB0, 0xFFB8903E, 0xFF6E9D7B, 0xFF9D78A6, 0xFF52A0A0, 0xFFBC7890)

/** Palette slot for a core colour key: `seq:N` (Mesh join order) takes slot N, so
 * consecutive devices differ; any other key (identity) hashes with FNV-1a. */
internal fun deviceHueSlot(key: String, slots: Int): Int {
    key.removePrefix("seq:").takeIf { key.startsWith("seq:") }?.toULongOrNull()?.let { return (it % slots.toULong()).toInt() }
    var hash = 0x811C9DC5u
    for (byte in key.encodeToByteArray()) hash = (hash xor (byte.toUInt() and 0xFFu)) * 0x01000193u
    return (hash % slots.toUInt()).toInt()
}

internal fun deviceHue(key: String): Color {
    val hues = if (ZorkColors.dark) DarkHues else LightHues
    return Color(hues[deviceHueSlot(key, hues.size)])
}

/** Colour keys of the directory's devices by the names surfaces print, so a mark
 * that only knows a device name still takes that device's stable hue. */
internal object DeviceColors {
    @Volatile var keys: Map<String, String> = emptyMap()
    fun publish(peers: List<Peer>) {
        keys = peers.flatMap { peer -> val key = peer.colorKey ?: peer.id
            listOfNotNull(peer.name to key, peer.machine?.let { it to key }) }.toMap()
    }
    fun key(name: String): String = keys[name] ?: name
}

/** First letter plus the first digit, if any ("mini1" → "M1"). */
internal fun deviceMonogram(name: String): String {
    val first = name.firstOrNull { it.isLetterOrDigit() } ?: return ""
    val digit = name.drop(1).firstOrNull { it.isDigit() }
    return first.uppercase() + (digit?.toString() ?: "")
}

/** The device's identity mark: its hue with the brand's folded corner. */
@Composable
internal fun DeviceMark(name: String, size: Dp = 18.dp, modifier: Modifier = Modifier, colorKey: String? = null) {
    val hue = deviceHue(colorKey ?: DeviceColors.key(name))
    Box(modifier.size(size), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(size)) {
            val w = this.size.width; val h = this.size.height
            val r = w * .3f; val k = r * .78f; val f = w / 3f
            val body = Path().apply {
                moveTo(r, 0f); lineTo(w - f, 0f); lineTo(w, f); lineTo(w, h - r)
                cubicTo(w, h - r + k, w - r + k, h, w - r, h)
                lineTo(r, h)
                cubicTo(r - k, h, 0f, h - r + k, 0f, h - r)
                lineTo(0f, r)
                cubicTo(0f, r - k, r - k, 0f, r, 0f)
                close()
            }
            drawPath(body, hue)
            val flap = Path().apply { moveTo(w - f, 0f); lineTo(w, f); lineTo(w - f, f); close() }
            drawPath(flap, Color.White.copy(alpha = .45f))
        }
        Text(deviceMonogram(name), color = Color.White, fontSize = (size.value * .52f).sp,
            fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}
