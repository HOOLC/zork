package surf.zork.android

import android.database.ContentObserver
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.View
import android.view.ViewTreeObserver
import androidx.compose.runtime.*
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.layer.GraphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.repeatOnLifecycle
import kotlinx.coroutines.channels.Channel
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** Same-process visual bridge. All scene calls belong to the display thread. */
internal object LiquidNative {
    init { System.loadLibrary("zork_android") }
    external fun create(): Long
    external fun destroy(handle: Long)
    external fun frame(handle: Long, input: ByteBuffer, inputBytes: Int, elapsed: Double, reduced: Boolean, output: ByteBuffer): Int
    external fun palette(): IntArray
    external fun geometry(width: Float, height: Float, radius: Float): FloatArray
}

internal object LiquidTokens {
    private val values = LiquidNative.palette()
    private fun color(index: Int) = Color(values[index] or 0xff000000.toInt())
    val Accent = color(0)
    val Outline = color(1)
    val NeutralHover = color(2)
    val NeutralPressed = color(3)
    val PrimaryHover = color(4)
    val PrimaryPressed = color(5)
    val AccentHover = color(6)
    val AccentPressed = color(7)
    val Focus = color(8)
    val Border = Float.fromBits(values[9]).dp
    val Smoothing = Float.fromBits(values[10])
    val CardRadius = Float.fromBits(values[11]).dp
    val FieldRadius = Float.fromBits(values[12]).dp
    val CompactRadius = Float.fromBits(values[13]).dp
    val PillRadius = Float.fromBits(values[14]).dp
    val IconRadius = Float.fromBits(values[15]).dp
}

/** Native Shape integration is a cold, size-keyed cache; it never runs physics. */
internal class LiquidShape(private val radius: Dp) : Shape {
    override fun createOutline(size: Size, layoutDirection: LayoutDirection, density: Density): Outline {
        if (size.width < 2f * density.density || size.height < 2f * density.density) {
            return Outline.Rectangle(androidx.compose.ui.geometry.Rect(Offset.Zero, size))
        }
        return Outline.Generic(LiquidShapeCache.path(size, density.density, radius.value))
    }
}

private object LiquidShapeCache {
    private data class Key(val width: Float, val height: Float, val density: Float, val radius: Float)
    private val entries = object : LinkedHashMap<Key, Path>(128, .75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Key, Path>) = size > 128
    }
    fun path(size: Size, density: Float, radius: Float): Path {
        val key = Key(size.width, size.height, density, radius)
        return entries.getOrPut(key) {
            val points = LiquidNative.geometry(size.width / density, size.height / density, radius)
            Path().apply {
                moveTo(points[0] * density, points[1] * density)
                var i = 2
                while (i < points.size) {
                    cubicTo(points[i] * density, points[i + 1] * density, points[i + 2] * density,
                        points[i + 3] * density, points[i + 4] * density, points[i + 5] * density)
                    i += 6
                }
                close()
            }
        }
    }
}

internal enum class LiquidKind(val wire: Int) { Static(0), Press(1), Target(2), Segment(3), Toggle(4), Morph(5), Pair(6), Slider(8), Compound(9), Member(10) }
internal data class LiquidPose(val x: Float = 0f, val y: Float = 0f, val width: Float = 2f, val height: Float = 2f, val radius: Float = 0f)
internal data class LiquidTarget(
    val kind: LiquidKind,
    val from: LiquidPose,
    val to: LiquidPose = from,
    val active: Boolean = false,
    val visible: Boolean = true,
    val snap: Boolean = false,
    val anchorX: Float = .5f,
    val anchorY: Float = .5f,
    val count: Int = 1,
    val selected: Int = 0,
    val border: Float = LiquidTokens.Border.value,
    val parent: Int = 0,
)

internal class LiquidOrigin(val node: LiquidNode, val view: View, val drawing: GraphicsLayer) {
    val focus = FocusRequester()
    var relocated by mutableStateOf(false)
    var recorded = false
    val visibleBounds get() = node.visibleBounds
    private var waiting: ViewTreeObserver.OnWindowFocusChangeListener? = null
    fun relativeTo(panel: LiquidNode, scale: Float): LiquidPose? = node.target?.from?.let { source ->
        LiquidPose((node.windowOrigin.x - panel.windowOrigin.x) / scale,
            (node.windowOrigin.y - panel.windowOrigin.y) / scale,
            source.width * node.density / scale, source.height * node.density / scale, source.radius)
    }
    fun cancelRestore() {
        waiting?.let { if (view.viewTreeObserver.isAlive) view.viewTreeObserver.removeOnWindowFocusChangeListener(it) }
        waiting = null
    }
    fun restoreFocus(epoch: Long) {
        cancelRestore()
        fun request() {
            if (node.attached && node.host.activationEpoch == epoch) {
                focus.requestFocus()
            }
        }
        if (view.hasWindowFocus()) request()
        else {
            val listener = ViewTreeObserver.OnWindowFocusChangeListener { focused ->
                if (focused) { cancelRestore(); view.post { request() } }
            }
            waiting = listener
            view.viewTreeObserver.addOnWindowFocusChangeListener(listener)
        }
    }
}

@Stable
internal class LiquidNode internal constructor(val host: LiquidSceneHost, val id: Int) {
    val fill = Path().apply { fillType = PathFillType.EvenOdd }
    val outline = Path().apply { fillType = PathFillType.EvenOdd }
    val border = Path()
    val activeTrack = Path()
    val track = Path()
    private val localBorder = Path()
    var revision by mutableIntStateOf(0); private set
    var alive by mutableStateOf(false); private set
    var inputReady by mutableStateOf(false); private set
    var pressure by mutableStateOf(false); private set
    var progress = 0f; private set
    var contentOpacity = 0f; private set
    var backdropOpacity = 0f; private set
    var width = 0f; private set
    var height = 0f; private set
    var x = 0f; private set
    var y = 0f; private set
    var density = 1f; private set
    var windowOrigin by mutableStateOf(Offset.Zero)
    var visibleBounds by mutableStateOf(Rect.Zero)
    var pressAnchorX = .5f
    var pressAnchorY = .5f
    var target: LiquidTarget? = null; private set
    var attached = false
    internal var sent = false
    var hasPath = false; private set
    private var resend = false

    fun update(next: LiquidTarget, scale: Float = density, tap: Boolean = false) {
        if (scale != density) { density = scale; resend = true }
        if (next == target && !tap && !resend) return
        target = next
        if (attached) host.enqueue(this, tap, resend)
    }
    internal fun encode(buffer: ByteBuffer, tap: Boolean, full: Boolean) {
        sent = true
        val target = requireNotNull(target)
        buffer.putInt(id).putInt(target.kind.wire)
        buffer.putInt((if (target.visible) 1 else 0) or (if (target.active) 2 else 0) or
            (if (target.snap) 4 else 0) or (if (tap) 8 else 0) or (if (full || resend) 16 else 0)).putInt(target.parent)
        fun pose(p: LiquidPose) { buffer.putFloat(p.x).putFloat(p.y).putFloat(p.width).putFloat(p.height).putFloat(p.radius) }
        pose(target.from); pose(target.to)
        buffer.putFloat(target.anchorX).putFloat(target.anchorY).putInt(target.count).putInt(target.selected)
            .putFloat(LiquidTokens.Smoothing).putFloat(target.border)
        repeat(4) { buffer.putInt(0) }
        resend = false
    }
    internal fun decode(buffer: ByteBuffer, flags: Int, loops: Int, borders: Int, active: Int, tracks: Int, x: Float, y: Float, width: Float, height: Float, progress: Float, contentOpacity: Float, backdropOpacity: Float, presentation: Int) {
        if (flags and 8 != 0) {
            alive = false; inputReady = false; pressure = false; this.progress = 0f
            this.contentOpacity = 0f; this.backdropOpacity = 0f; revision++; return
        }
        if (flags and 1 != 0) {
            fill.rewind(); localBorder.rewind()
            repeat(loops) {
                val curves = buffer.int
                fill.moveTo(buffer.float * density, buffer.float * density)
                repeat(curves) { fill.cubicTo(buffer.float * density, buffer.float * density, buffer.float * density,
                    buffer.float * density, buffer.float * density, buffer.float * density) }
                fill.close()
            }
            repeat(borders) {
                val points = buffer.int
                if (points > 0) {
                    localBorder.moveTo(buffer.float * density, buffer.float * density)
                    repeat(points - 1) { localBorder.lineTo(buffer.float * density, buffer.float * density) }
                    localBorder.close()
                }
            }
            hasPath = true
        }
        fun curves(path: Path, count: Int) {
            if (count == 0) return
            path.rewind()
            repeat(count) {
                val segments = buffer.int
                path.moveTo(buffer.float * density, buffer.float * density)
                repeat(segments) { path.cubicTo(buffer.float * density, buffer.float * density, buffer.float * density,
                    buffer.float * density, buffer.float * density, buffer.float * density) }
                path.close()
            }
        }
        curves(activeTrack, active); curves(track, tracks)
        val origin = Offset(x * density, y * density)
        outline.rewind(); outline.addPath(fill, origin)
        border.rewind(); border.addPath(localBorder, origin)
        this.progress = progress
        this.contentOpacity = contentOpacity
        this.backdropOpacity = backdropOpacity
        this.x = x; this.y = y
        this.width = width; this.height = height
        alive = presentation and 1 != 0
        inputReady = alive && contentOpacity >= .999f
        pressure = flags and 4 != 0
        revision++
    }
    fun prepareFirstDraw() { if (!hasPath) host.flushInitial() }
}

/** One coroutine and one retained buffer pair for a complete visible host. */
@Stable
internal class LiquidSceneHost {
    val overlays = mutableStateListOf<LiquidOverlayPaint>()
    var lastActivation: LiquidOrigin? = null
    var activationEpoch = 0L; private set
    fun activate(origin: LiquidOrigin) { origin.cancelRestore(); lastActivation = origin; activationEpoch++ }
    var probe: LiquidFrameProbe? = null
    private data class Pending(val node: LiquidNode?, var tap: Boolean = false, var full: Boolean = false)
    private val nodes = LinkedHashMap<Int, LiquidNode>()
    private val pending = LinkedHashMap<Int, Pending>()
    private var input = direct(96 * 32)
    private var output = direct(16 * 1024)
    private var handle = 0L
    private var sequence = 0
    private var closed = false
    private var lastNanos: Long? = null
    internal val wake = Channel<Unit>(Channel.CONFLATED)
    var active = false; private set
    var moving = false; private set
    var durationScale = 1f
        set(value) { if (field != value) { field = value; wake.trySend(Unit) } }
    // Bounded counters are also used by instrumentation; no per-frame log/JSON.
    var frames = 0L; private set
    var geometryRecords = 0L; private set
    var bytes = 0L; private set
    var errors = 0L; private set
    var nativeNanos = 0L; private set
    var decodeNanos = 0L; private set
    val nodeCount get() = nodes.size
    val hasPending get() = pending.isNotEmpty()
    fun node() = LiquidNode(this, ++sequence)
    fun attach(node: LiquidNode) {
        check(!closed); nodes[node.id] = node; node.attached = true
        if (node.target != null) enqueue(node, false, true)
    }
    fun detach(node: LiquidNode) {
        node.attached = false; nodes.remove(node.id)
        if (!closed) { pending[node.id] = Pending(null); wake.trySend(Unit) }
    }
    internal fun enqueue(node: LiquidNode, tap: Boolean, full: Boolean) {
        if (closed) return
        // Unseen rows need no native allocation. Lazy containers dispose rows
        // as they leave, and previously drawn nodes still receive terminal state.
        if (node.target?.visible == false && !node.sent) { pending.remove(node.id); return }
        val change = pending.getOrPut(node.id) { Pending(node) }
        change.tap = change.tap || tap; change.full = change.full || full
        wake.trySend(Unit)
    }
    fun resume() { active = true; lastNanos = null; if (moving || hasPending) wake.trySend(Unit) }
    fun pause() { active = false; lastNanos = null }
    fun resetFrameClock() { lastNanos = null }
    fun tick(nanos: Long) {
        val elapsed = lastNanos?.let { ((nanos - it).coerceAtLeast(0L) / 1e9).coerceAtMost(.05) } ?: 0.0
        flush(if (durationScale > 0f) elapsed / durationScale else 0.0)
        lastNanos = if (moving || hasPending) nanos else null
    }
    internal fun flushInitial() { if (active && hasPending) flush(0.0) }
    private fun flush(elapsed: Double) {
        if (!active || closed) return
        val encodingAt = System.nanoTime()
        if (handle == 0L) handle = LiquidNative.create()
        if (input.capacity() < pending.size * 96) input = direct(grown(pending.size * 96))
        input.clear()
        for ((id, change) in pending) {
            if (change.node == null) { input.putInt(id).putInt(7); repeat(22) { input.putInt(0) } }
            else change.node.encode(input, change.tap, change.full)
        }
        val inputBytes = input.position()
        pending.clear()
        val begin = System.nanoTime()
        var used = LiquidNative.frame(handle, input, inputBytes, elapsed, durationScale <= 0f, output)
        if (used < 0) {
            output = direct(grown(-used))
            used = LiquidNative.frame(handle, input, -1, 0.0, false, output)
        }
        val decodedAt = System.nanoTime()
        check(used >= 16 && used <= output.capacity())
        output.position(0); output.limit(used)
        check(output.int == 3) { "Liquid protocol mismatch" }
        val records = output.int
        moving = output.int > 0
        errors += output.int
        repeat(records) {
            val start = output.position()
            val id = output.int; val flags = output.int; val length = output.int
            val loops = output.int; val borders = output.int; val active = output.int; val tracks = output.int
            val x = output.float; val y = output.float; val width = output.float; val height = output.float; val progress = output.float
            val contentOpacity = output.float; val backdropOpacity = output.float
            output.float // Expansion is diagnostic; the shared scene owns its entry cue.
            val presentation = output.int
            nodes[id]?.decode(output, flags, loops, borders, active, tracks, x, y, width, height, progress, contentOpacity, backdropOpacity, presentation)
            output.position(start + length)
            if (flags and 1 != 0) geometryRecords++
        }
        val finishedAt = System.nanoTime()
        nativeNanos += decodedAt - begin; decodeNanos += finishedAt - decodedAt
        probe?.record(begin - encodingAt, decodedAt - begin, finishedAt - decodedAt, used, records)
        frames++; bytes += used
    }
    fun close() {
        if (closed) return
        closed = true; active = false; wake.close(); pending.clear(); nodes.clear(); lastActivation = null
        if (handle != 0L) { LiquidNative.destroy(handle); handle = 0L }
    }
    private fun grown(bytes: Int): Int {
        require(bytes in 1..8 * 1024 * 1024)
        return Integer.highestOneBit(bytes - 1).coerceAtLeast(1) shl 1
    }
    private fun direct(bytes: Int) = ByteBuffer.allocateDirect(bytes).order(ByteOrder.nativeOrder())
}

/** Optional bounded instrumentation storage; the production path allocates none. */
internal class LiquidFrameProbe(capacity: Int = 4096) {
    val data = LongArray(capacity.coerceIn(1, 16384) * 5)
    var count = 0; private set
    fun record(encode: Long, native: Long, decode: Long, bytes: Int, records: Int) {
        if (count * 5 == data.size) return
        val offset = count++ * 5
        data[offset] = encode; data[offset + 1] = native; data[offset + 2] = decode
        data[offset + 3] = bytes.toLong(); data[offset + 4] = records.toLong()
    }
}

internal val LocalLiquidHost = staticCompositionLocalOf<LiquidSceneHost> { error("Liquid controls require ZorkTheme or LiquidHost") }
internal val LocalLiquidInteractive = staticCompositionLocalOf { true }
internal val LocalLiquidVisible = staticCompositionLocalOf { true }

@Composable
internal fun LiquidHost(content: @Composable () -> Unit) {
    val host = remember { LiquidSceneHost() }
    val owner = LocalLifecycleOwner.current
    val resolver = LocalContext.current.contentResolver
    DisposableEffect(host, resolver) {
        fun refresh() { host.durationScale = Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f).coerceAtLeast(0f) }
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) { override fun onChange(selfChange: Boolean) = refresh() }
        resolver.registerContentObserver(Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, observer)
        refresh()
        onDispose { resolver.unregisterContentObserver(observer); host.close() }
    }
    LaunchedEffect(host, owner) {
        owner.lifecycle.repeatOnLifecycle(Lifecycle.State.STARTED) {
            host.resume()
            try {
                for (signal in host.wake) {
                    do {
                        withFrameNanos { host.tick(it) }
                        // Consume already represented input notifications; leave
                        // the coroutine suspended once the material has settled.
                        while (host.wake.tryReceive().isSuccess) { }
                    } while (host.moving || host.hasPending)
                }
            } finally { host.pause() }
        }
    }
    CompositionLocalProvider(LocalLiquidHost provides host) { LiquidPaintHost(content) }
}

@Composable
internal fun rememberLiquidNode(): LiquidNode {
    val host = LocalLiquidHost.current
    val node = remember(host) { host.node() }
    DisposableEffect(node) { host.attach(node); onDispose { host.detach(node) } }
    return node
}
