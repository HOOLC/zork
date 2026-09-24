package ing.zork.android

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import kotlinx.coroutines.launch
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.zIndex
import androidx.compose.ui.semantics.clearAndSetSemantics

internal fun settingsRouteKey(state: MobileSettingsState?): String = state?.let {
    "settings:${it.device?.id.orEmpty()}:${it.page}:${if (it.page == "profile") it.selectedProfileId ?: it.profile?.text("profile_id").orEmpty() else ""}:${it.resource?.query.orEmpty()}"
} ?: "workbench"

internal fun settingsRouteDepth(state: MobileSettingsState?): Int {
    if (state == null) return 0
    val deviceDepth = if (state.fromChat) 1 else 2
    // A connection opened from the global list sits one level above that list.
    if (state.fromConnections && state.page in listOf("models", "profile")) return 3 + state.resourceDepth
    return when (state.page) {
        "home" -> 1
        "appearance", "notifications", "account", "adb", "model-connections" -> 2
        "device" -> deviceDepth
        "profile" -> deviceDepth + 2
        else -> deviceDepth + 1
    } + state.resourceDepth
}

private class PageSlideEntry<T>(val key: String, var state: T, val depth: Int, val background: Color)
private class PageSlideFrames<T> {
    var current: PageSlideEntry<T>? = null
    /** Each composed page's last drawing, for snapshots. */
    val layers = HashMap<String, androidx.compose.ui.graphics.layer.GraphicsLayer>()
    /** Rendered pages left behind by forward navigation, newest last. */
    val below = mutableStateListOf<Pair<String, androidx.compose.ui.graphics.ImageBitmap>>()
}
/** Snapshots kept for the back gesture; older pages fall back to the plain background. */
private const val SNAPSHOT_DEPTH = 3
private data class PageSlideChange<T>(val current: PageSlideEntry<T>, val outgoing: PageSlideEntry<T>?, val forward: Boolean)

/** Keep each page's composition and child layers independent until the slide ends.
 * A recorded parent layer still references mutable child layers and cannot freeze a page. */
@Composable
internal fun <T> PageSlide(
    state: T, routeKey: String, depth: Int, background: Color,
    modifier: Modifier = Modifier,
    /** Progress of an in-flight predictive back gesture, 0 when there is none. */
    backProgress: () -> Float = { 0f },
    content: @Composable (T, Boolean) -> Unit,
) {
    val reduced = LocalReducedMotion.current
    val frames = remember { PageSlideFrames<T>() }
    val change = remember(routeKey) {
        val previous = frames.current
        val current = PageSlideEntry(routeKey, state, depth, background)
        frames.current = current
        PageSlideChange(current, previous, previous == null || depth >= previous.depth)
    }
    change.current.state = state
    val progress = remember { Animatable(1f) }
    var started by remember(routeKey) { mutableStateOf(change.outgoing == null) }
    var finished by remember(routeKey) { mutableStateOf(change.outgoing == null) }
    LaunchedEffect(routeKey) {
        val outgoing = change.outgoing
        if (outgoing != null && change.forward) {
            // Keep a picture of the page being covered, to draw under a later back gesture.
            launch {
                val picture = frames.layers[outgoing.key]?.let { runCatching { it.toImageBitmap() }.getOrNull() }
                frames.below.removeAll { it.first == outgoing.key }
                if (picture != null) frames.below.add(outgoing.key to picture)
                while (frames.below.size > SNAPSHOT_DEPTH) frames.below.removeAt(0)
            }
        } else if (outgoing != null) {
            // Returning to a page: its picture (and any above it) is live again.
            val index = frames.below.indexOfLast { it.first == change.current.key }
            if (index >= 0) frames.below.removeRange(index, frames.below.size)
            else if (frames.below.isNotEmpty()) frames.below.removeAt(frames.below.lastIndex)
        }
        if (change.outgoing != null) {
            // A committed back gesture continues from where the finger let go.
            progress.snapTo(if (change.forward) 0f else backProgress().coerceIn(0f, 1f))
            started = true
            progress.animateTo(1f, PageSlideMotion.spec(change.forward, reduced))
            finished = true
        }
    }
    fun position() = if (finished) 1f else if (started) progress.value else 0f
    fun frontVisibility() = if (change.forward) position() else 1f - position()
    val entries = if (finished) listOf(change.current) else listOfNotNull(change.outgoing, change.current)
    Box(modifier.fillMaxSize().clipToBounds()) {
        // During a back gesture the page below shows at its settled place, slightly
        // faded, under the page following the finger. It is a picture: composing a
        // second live page for every screen would double the cost of each frame.
        if (finished) frames.below.lastOrNull()?.let { (_, picture) ->
            androidx.compose.foundation.Canvas(Modifier.fillMaxSize().graphicsLayer {
                val p = backProgress()
                alpha = if (p > 0f) .9f + .1f * p else 0f
            }) { drawImage(picture) }
        }
        for (entry in entries) key(entry.key) {
            val layer = androidx.compose.ui.graphics.rememberGraphicsLayer()
            DisposableEffect(entry.key, layer) {
                frames.layers[entry.key] = layer
                onDispose { if (frames.layers[entry.key] === layer) frames.layers.remove(entry.key) }
            }
            val active = entry === change.current
            val front = active == change.forward
            Box(Modifier.fillMaxSize().zIndex(if (front) 1f else 0f)
                .then(when {
                    finished -> Modifier.pageSlideGesture(backProgress, reduced)
                    front -> Modifier.pageSlideFront(::frontVisibility, reduced)
                    else -> Modifier
                })
                .drawWithContent {
                    layer.record { this@drawWithContent.drawContent() }
                    drawLayer(layer)
                }
                .background(entry.background)
                .then(if (active) Modifier else Modifier.testTag("page-slide-outgoing").clearAndSetSemantics { })) {
                content(entry.state, active)
            }
        }
    }
}
