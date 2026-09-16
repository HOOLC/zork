package surf.zork.android

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch

/** Fixed core-shaped snapshots exercising the production view and state holder. */
class SharedFilesPreviewActivity : ComponentActivity() {
    private val sources = listOf(SharedSourceUi("studio", "工作室", true, false), SharedSourceUi("laptop", "笔记本", false, true))
    private val versions = listOf(
        SharedVersionUi("first", 120, 1_789_200_000_000_000_000, sources.take(1), true),
        SharedVersionUi("second", 100, 1_789_190_000_000_000_000, sources.drop(1), true))
    private val entries = List(100_000) { index ->
        SharedEntryUi("file:$index", "file-$index.txt", "项目资料 %06d.txt".format(index), false, sources, versions)
    }
    internal var data by mutableStateOf(SharedFilesUi(true, sources, listOf(SharedSpaceUi("space", "项目资料", sources)), entries,
        "space", "", null, "", "list", "name", null, SharedSaveUi(false, null, "", null, false),
        false, false, false, null, locationName = "项目资料"))
    internal var listState: LazyListState? = null
    val commands = mutableListOf<String>()
    val frameTimes = mutableListOf<Double>()
    fun scrollTo(index: Int) { lifecycleScope.launch { listState!!.scrollToItem(index) } }
    fun preview() {
        data = data.copy(preview = SharedPreviewUi("file.txt", "项目说明.txt", "first", versions, false, null,
            "在一个入口浏览各设备明确共享的文件夹。\n\n共享文件保持只读，每个副本保留自己的来源和内容版本。", false, "text/plain", false, true))
    }
    fun closePreview() { data = data.copy(preview = null) }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.addOnFrameMetricsAvailableListener({ _, frame, _ -> synchronized(frameTimes) {
            frameTimes.add((frame.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION) + frame.getMetric(FrameMetrics.DRAW_DURATION)) / 1_000_000.0)
        } }, Handler(Looper.getMainLooper()))
        setContent {
            ZorkTheme {
                val retained = rememberSaveableStateHolder()
                Box(Modifier.fillMaxSize().safeDrawingPadding()) {
                    PageSlide(data, "shared:${data.preview?.path.orEmpty()}", if (data.preview == null) 1 else 2, ZorkColors.Canvas) { shown, active ->
                        retained.SaveableStateProvider(sharedFilesSavedKey(shown)) {
                            val state = rememberLazyListState()
                            SideEffect { if (active && shown.preview == null) listState = state }
                            SharedFilesPage(shown, null, if (!active) SharedFilesActions() else SharedFilesActions(
                                back = { closePreview() }, entry = { commands.add("entry:$it"); preview() },
                                source = { commands.add("source:$it") }, version = { commands.add("version:$it"); data = data.copy(preview = data.preview?.copy(selected = it)) },
                                save = { commands.add("save") }, layout = { data = data.copy(layout = it) }), state)
                        }
                    }
                }
            }
        }
    }
}
