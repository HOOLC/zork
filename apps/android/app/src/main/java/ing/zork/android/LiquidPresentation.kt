package ing.zork.android

import android.view.View
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.layer.GraphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionOnScreen
import androidx.compose.ui.platform.LocalView

/** A visual recording has no event handlers, focus nodes or business state. */
internal class LiquidOverlayPaint(
    val node: LiquidNode,
    val owner: View,
    val content: GraphicsLayer,
    val frame: GraphicsLayer,
    val modal: Boolean,
) {
    var source: LiquidOrigin? = null
    var open = false
    var recorded = false
    var frameRecorded = false
    var windowOrigin by mutableStateOf(Offset.Zero)
    var frameOrigin = Offset.Zero
    var contentOrigin = Offset.Zero
    var closingOrigin: Offset? = null
    var transfer = false

    fun update(open: Boolean, source: LiquidOrigin?) {
        if (this.source !== source) this.source?.relocated = false
        if (this.open != open) {
            transfer = frameRecorded
            closingOrigin = if (open) null else source?.node?.windowOrigin
            node.host.resetFrameClock()
        }
        this.open = open
        this.source = source
        source?.relocated = open || node.alive
    }

    private fun translation(): Offset = closingOrigin?.let { start ->
        source?.node?.takeIf { it.attached }?.let { it.windowOrigin - start }
    } ?: Offset.Zero

    fun DrawScope.source(at: Offset) {
        source?.takeIf { it.node.attached && it.recorded }?.let { origin ->
            val offset = origin.node.windowOrigin - at
            val clip = origin.visibleBounds
            clipRect(clip.left - at.x, clip.top - at.y, clip.right - at.x, clip.bottom - at.y) {
                translate(offset.x, offset.y) { drawLayer(origin.drawing) }
            }
        }
    }

    fun DrawScope.paint(at: Offset, scrim: Boolean = modal) {
        node.revision
        if (!node.hasPath) return
        val delta = translation()
        val position = node.windowOrigin + delta - at
        translate(position.x, position.y) {
            drawPath(node.outline, ZorkColors.Canvas)
            source(node.windowOrigin + delta)
            drawPath(node.border, LiquidTokens.Outline)
        }
        if (scrim) drawRect(Color.Black.copy(alpha = .55f * node.backdropOpacity))
        translate(position.x, position.y) {
            // The complete contour remains authoritative. The body bounds only
            // select which parcel is above the scrim; they never replace its mask.
            clipPath(node.outline) {
                clipRect(node.x * node.density, node.y * node.density,
                    (node.x + node.width) * node.density, (node.y + node.height) * node.density) {
                    drawRect(ZorkColors.Canvas.copy(alpha = node.contentOpacity),
                        topLeft = Offset(node.x * node.density, node.y * node.density),
                        size = Size(node.width * node.density, node.height * node.density))
                    if (recorded) {
                        content.alpha = node.contentOpacity
                        val factor = .96f + .04f * node.contentOpacity
                        scale(factor, factor, Offset(contentOrigin.x + content.size.width / 2f, contentOrigin.y + content.size.height / 2f)) {
                            translate(contentOrigin.x, contentOrigin.y) { drawLayer(content) }
                        }
                    }
                    drawPath(node.border, LiquidTokens.Outline)
                }
            }
        }
    }

    fun DrawScope.retire(at: Offset) {
        node.revision
        if (transfer && frameRecorded && node.host.durationScale > 0f) {
            // Replay the last actual window drawing on the first source-layer
            // frame; subsequent frames continue the same shared material.
            val offset = frameOrigin - at
            translate(offset.x, offset.y) { drawLayer(frame) }
            transfer = false
        } else {
            frame.record { paint(at) }
            frameOrigin = at
            frameRecorded = true
            drawLayer(frame)
        }
    }

    fun DrawScope.releaseDrawing() {
        if (recorded) { content.record { }; recorded = false }
        if (frameRecorded) { frame.record { }; frameRecorded = false }
        transfer = false
    }
}

/** Every native window paints retirements belonging to its actual source view. */
@Composable
internal fun LiquidPaintHost(content: @Composable () -> Unit) {
    val host = LocalLiquidHost.current
    val view = LocalView.current
    var position by remember { mutableStateOf(Offset.Zero) }
    Box(Modifier.onGloballyPositioned { position = it.positionOnScreen() }.drawWithContent {
        drawContent()
        host.overlays.forEach { paint ->
            if (!paint.open && paint.node.alive && (paint.source?.view ?: paint.owner) === view) {
                with(paint) { retire(position) }
            } else if (!paint.open && !paint.node.alive) {
                with(paint) { releaseDrawing() }
            }
        }
    }, propagateMinConstraints = true) { content() }
}

@Composable
internal fun rememberLiquidPaint(node: LiquidNode, modal: Boolean): LiquidOverlayPaint {
    val content = rememberGraphicsLayer()
    val frame = rememberGraphicsLayer()
    val owner = LocalView.current
    val paint = remember(node, owner) { LiquidOverlayPaint(node, owner, content, frame, modal) }
    DisposableEffect(paint) {
        node.host.overlays.add(paint)
        onDispose { paint.source?.relocated = false; node.host.overlays.remove(paint) }
    }
    return paint
}
