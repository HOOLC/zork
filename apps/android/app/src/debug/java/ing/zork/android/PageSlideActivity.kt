package ing.zork.android

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import org.json.JSONObject
import androidx.compose.ui.semantics.getOrNull

/** Production settings and page transitions with fixed data; no client or network. */
class PageSlideActivity : ComponentActivity() {
    internal var page by mutableStateOf(MobileSettingsState())
    val positions = mutableListOf<Float>()
    val outgoingPositions = mutableListOf<Float>()
    val pageLifetimes = mutableListOf<String>()
    private fun outgoingPosition(view: android.view.View): Float? {
        if (view is androidx.compose.ui.platform.ViewRootForTest) {
            fun find(node: androidx.compose.ui.semantics.SemanticsNode): Float? {
                if (node.config.getOrNull(androidx.compose.ui.semantics.SemanticsProperties.TestTag) == "page-slide-outgoing") return node.positionInRoot.x
                return node.children.firstNotNullOfOrNull { find(it) }
            }
            return find(view.semanticsOwner.unmergedRootSemanticsNode)
        }
        return if (view is android.view.ViewGroup) (0 until view.childCount).firstNotNullOfOrNull { outgoingPosition(view.getChildAt(it)) } else null
    }
    val frameTimes = mutableListOf<Double>()
    private var coordinates: LayoutCoordinates? = null
    fun go(name: String) {
        page = MobileSettingsState(page = name, device = if (name == "home") null else Peer("fixture", "测试设备", ""),
            profile = if (name == "profile") JSONObject().put("profile_id", "fixture") else null)
    }
    fun refresh() { page = page.copy(loading = !page.loading) }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.decorView.viewTreeObserver.addOnDrawListener {
            coordinates?.takeIf { it.isAttached }?.let { positions.add(it.positionInWindow().x) }
            if (intent.getBooleanExtra("trackMotion", false)) outgoingPosition(window.decorView)?.let { outgoingPositions.add(it) }
        }
        window.addOnFrameMetricsAvailableListener({ _, frame, _ -> synchronized(frameTimes) {
            frameTimes.add((frame.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION) + frame.getMetric(FrameMetrics.DRAW_DURATION)) / 1_000_000.0)
        } }, Handler(Looper.getMainLooper()))
        setContent {
            CompositionLocalProvider(LocalDensity provides Density(1f, 1f)) {
                ZorkTheme {
                    val route = settingsRouteKey(page)
                    val savedState = rememberSaveableStateHolder()
                    val body: @Composable (MobileSettingsState, Boolean) -> Unit = { shown, active ->
                        DisposableEffect(shown.page) {
                            pageLifetimes.add("enter:${shown.page}")
                            onDispose { pageLifetimes.add("exit:${shown.page}") }
                        }
                        savedState.SaveableStateProvider(settingsRouteKey(shown)) {
                            Box(Modifier.onGloballyPositioned { if (active) coordinates = it }) {
                                MobileSettings(shown, listOf(Peer("fixture", "测试设备", "")), if (!active) SettingsActions() else SettingsActions(
                                    back = { go("home") }, device = { go("device") }, page = { go(it) },
                                ))
                            }
                        }
                    }
                    Box(Modifier.requiredSize(390.dp, 844.dp)) {
                        if (intent.getBooleanExtra("animated", true)) PageSlide(page, route, settingsRouteDepth(page), ZorkColors.Canvas, content = body)
                        else body(page, true)
                    }
                }
            }
        }
    }
}
