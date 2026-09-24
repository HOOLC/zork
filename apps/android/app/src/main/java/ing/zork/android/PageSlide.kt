package ing.zork.android

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
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
private class PageSlideFrames<T> { var current: PageSlideEntry<T>? = null }
private data class PageSlideChange<T>(val current: PageSlideEntry<T>, val outgoing: PageSlideEntry<T>?, val forward: Boolean)

/** Keep each page's composition and child layers independent until the slide ends.
 * A recorded parent layer still references mutable child layers and cannot freeze a page. */
@Composable
internal fun <T> PageSlide(
    state: T, routeKey: String, depth: Int, background: Color,
    modifier: Modifier = Modifier, content: @Composable (T, Boolean) -> Unit,
) {
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
        if (change.outgoing != null) {
            progress.snapTo(0f)
            started = true
            progress.animateTo(1f, PageSlideMotion.spec(change.forward))
            finished = true
        }
    }
    fun position() = if (finished) 1f else if (started) progress.value else 0f
    fun frontVisibility() = if (change.forward) position() else 1f - position()
    val entries = if (finished) listOf(change.current) else listOfNotNull(change.outgoing, change.current)
    Box(modifier.fillMaxSize().clipToBounds()) {
        for (entry in entries) key(entry.key) {
            val active = entry === change.current
            val front = active == change.forward
            Box(Modifier.fillMaxSize().zIndex(if (front) 1f else 0f)
                .then(if (front) Modifier.pageSlideFront(::frontVisibility) else Modifier.pageSlideBack(::frontVisibility))
                .background(entry.background)
                .then(if (active) Modifier else Modifier.testTag("page-slide-outgoing").clearAndSetSemantics { })) {
                content(entry.state, active)
            }
        }
    }
}
