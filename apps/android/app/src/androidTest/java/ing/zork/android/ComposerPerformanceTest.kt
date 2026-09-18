package ing.zork.android

import android.content.Intent
import android.view.FrameMetrics
import android.view.Window
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Uses the device's real pixel density and refresh budget, never Density(1f). */
@RunWith(AndroidJUnit4::class)
class ComposerPerformanceTest {
    @Test fun presenceFrames() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        var scene: LiquidSceneHost? = null
        val samples = mutableListOf<List<Long>>()
        var active by mutableStateOf(false)
        var observedRefreshRate = 0f
        val listener = Window.OnFrameMetricsAvailableListener { _, frame, _ -> synchronized(samples) {
            samples.add(listOf(frame.getMetric(FrameMetrics.TOTAL_DURATION), frame.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION),
                frame.getMetric(FrameMetrics.DRAW_DURATION), frame.getMetric(FrameMetrics.GPU_DURATION), frame.getMetric(FrameMetrics.DEADLINE), frame.getMetric(FrameMetrics.ANIMATION_DURATION), frame.getMetric(FrameMetrics.INTENDED_VSYNC_TIMESTAMP), frame.getMetric(FrameMetrics.VSYNC_TIMESTAMP)))
        } }
        val rows = (0 until 40).map { ChatMessage("perf-$it", "伙伴", "消息 $it · 为输入区和伙伴状态保留空间。", false) }
        fun pause(ms: Long) { Thread.sleep(ms); instrumentation.waitForIdleSync() }
        ActivityScenario.launch<ComposerPreviewActivity>(Intent(instrumentation.targetContext, ComposerPreviewActivity::class.java)).use { scenario ->
            scenario.onActivity { activity ->
                activity.setContent { ZorkTheme {
                    val shared = LocalLiquidHost.current
                    SideEffect { scene = shared }
                    val members = listOf("fox", "cat", "panda").mapIndexed { i, avatar -> JSONObject()
                        .put("id", "member-$i").put("name", listOf("产品伙伴", "工程伙伴", "设计伙伴")[i]).put("avatar", avatar)
                        .put("activity", if (active) JSONObject().put("state", "thinking") else JSONObject.NULL) }
                    androidx.compose.foundation.layout.Box(Modifier.fillMaxSize().background(ZorkColors.Canvas)) {
                        ConversationBody(WorkbenchState(conversation = Conversation("composer-perf", "伙伴"), participants = members,
                            messages = rows, connected = true, draft = "第一行\n第二行\n第三行"), WorkbenchActions())
                    }
                } }
                activity.window.addOnFrameMetricsAvailableListener(listener, android.os.Handler(activity.mainLooper))
            }
            pause(1000)
            scenario.onActivity { active = true }; pause(200)
            val diagnostic = File(instrumentation.targetContext.getExternalFilesDir(null), "composer-performance").apply { mkdirs() }
            val fd = instrumentation.uiAutomation.executeShellCommand("dumpsys display")
            android.os.ParcelFileDescriptor.AutoCloseInputStream(fd).use { input -> File(diagnostic, "display.txt").writeBytes(input.readBytes()) }
            scenario.onActivity { activity ->
                observedRefreshRate = activity.display?.refreshRate ?: 0f
                val attrs = activity.window.attributes
                File(diagnostic, "window.txt").writeText("mode=${attrs.preferredDisplayModeId}, rate=${attrs.preferredRefreshRate}, display=${activity.display?.refreshRate}")
            }
            pause(600)
            scenario.onActivity { active = false }; pause(2000)
            synchronized(samples) { samples.clear() }
            instrumentation.uiAutomation.executeShellCommand("dumpsys gfxinfo ing.zork.android.debug reset").close()
            repeat(4) {
                scenario.onActivity { active = true }; pause(800)
                scenario.onActivity { active = false }; pause(2000)
            }
            val folder = File(instrumentation.targetContext.getExternalFilesDir(null), "composer-performance").apply { mkdirs() }
            val saved = synchronized(samples) { samples.toList() }
            File(folder, "frames.csv").writeText("total_ns,layout_ns,draw_ns,gpu_ns,deadline_ns,animation_ns,intended_vsync_ns,vsync_ns\n" + saved.joinToString("\n") { it.joinToString(",") })
            scenario.onActivity { it.window.removeOnFrameMetricsAvailableListener(listener) }
            scenario.onActivity {
                val shared = requireNotNull(scene)
                org.junit.Assert.assertTrue("shared Rust scene was not rendered", shared.geometryRecords > 0)
                org.junit.Assert.assertEquals("shared contour errors", 0L, shared.errors)
                File(folder, "shared-scene.txt").writeText("frames=${shared.frames}, bytes=${shared.bytes}, native_ns=${shared.nativeNanos}, decode_ns=${shared.decodeNanos}")
            }
            if (InstrumentationRegistry.getArguments().getString("require120") == "true") {
                org.junit.Assert.assertTrue("Display is not rendering at 120Hz: $observedRefreshRate", observedRefreshRate >= 119.5f)
                org.junit.Assert.assertTrue("Insufficient animated frames: ${saved.size}", saved.size >= 400)
                val ui = saved.map { (it[1] + it[2] + it[5]) / 1_000_000.0 }.sorted()
                val p95 = ui[((ui.size - 1) * .95).toInt()]
                org.junit.Assert.assertTrue("120Hz CPU UI budget exceeded: $p95 ms", p95 <= 1000.0 / 120.0)
            }
        }
    }
}
