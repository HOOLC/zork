package surf.zork.android

import android.content.Intent
import android.view.FrameMetrics
import android.view.Window
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Real-density Canvas rendering, plus separate batched JNI/encoding costs. */
@RunWith(AndroidJUnit4::class)
class LiquidPerformanceTest {
    @Test fun sixteenControlsStayWithinTheDisplayBudgetAndSleepAtRest() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val metrics = LongArray(4096 * 8)
        var count = 0
        var phase by mutableIntStateOf(0)
        var scene: LiquidSceneHost? = null
        var refresh = 0f
        val fields = intArrayOf(FrameMetrics.TOTAL_DURATION, FrameMetrics.LAYOUT_MEASURE_DURATION,
            FrameMetrics.DRAW_DURATION, FrameMetrics.GPU_DURATION, FrameMetrics.DEADLINE,
            FrameMetrics.ANIMATION_DURATION, FrameMetrics.INTENDED_VSYNC_TIMESTAMP, FrameMetrics.VSYNC_TIMESTAMP)
        val listener = Window.OnFrameMetricsAvailableListener { _, frame, _ ->
            if (count < 4096) { for (i in fields.indices) metrics[count * 8 + i] = frame.getMetric(fields[i]); count++ }
        }
        fun pause(ms: Long) { Thread.sleep(ms); instrumentation.waitForIdleSync() }
        ActivityScenario.launch<LiquidGalleryActivity>(Intent(instrumentation.targetContext, LiquidGalleryActivity::class.java)).use { scenario ->
            scenario.onActivity { activity ->
                activity.setContent { ZorkTheme {
                    val host = LocalLiquidHost.current
                    SideEffect { scene = host }
                    Column(Modifier.fillMaxSize().background(ZorkColors.Paper).systemBarsPadding().padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(16.dp)) {
                        repeat(4) { row ->
                            Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                LiquidSegments(listOf("0" to "日", "1" to "周", "2" to "月"), ((phase + row) % 3).toString(), {})
                                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    Box(Modifier.weight(1f)) { LiquidSlider("滑块 $row", if (phase % 2 == 0) .15f else .85f, {}) }
                                    LiquidSwitch(phase % 2 == 0, {})
                                }
                                LiquidProgress(if (phase % 2 == 0) .2f else .9f)
                            }
                        }
                    }
                } }
            }
            // Warm the compiled app, Android paths and native contour workspaces.
            repeat(8) { scenario.onActivity { phase++ }; pause(180) }
            scenario.onActivity { activity ->
                requireNotNull(scene).probe = LiquidFrameProbe()
                refresh = activity.display?.refreshRate ?: 0f
                activity.window.addOnFrameMetricsAvailableListener(listener, android.os.Handler(activity.mainLooper))
            }
            repeat(60) { scenario.onActivity { phase++ }; pause(180) }
            scenario.onActivity { it.window.removeOnFrameMetricsAvailableListener(listener) }
            pause(2000)
            var idleFrames = 0L
            scenario.onActivity { idleFrames = requireNotNull(scene).frames; assertFalse(requireNotNull(scene).moving) }
            pause(700)
            scenario.onActivity { assertEquals("idle native frame callbacks", idleFrames, requireNotNull(scene).frames) }
            val host = requireNotNull(scene)
            val probe = requireNotNull(host.probe)
            assertEquals("contour errors", 0L, host.errors)
            assertTrue("not enough real display samples: $count", count >= 400)
            assertTrue("not enough native samples: ${probe.count}", probe.count >= 400)
            fun p95(values: List<Double>) = values.sorted()[((values.size - 1) * .95).toInt()]
            val native = (0 until probe.count).map { i -> (probe.data[i * 5] + probe.data[i * 5 + 1] + probe.data[i * 5 + 2]) / 1e6 }
            val ui = (0 until count).map { i -> (metrics[i * 8 + 1] + metrics[i * 8 + 2] + metrics[i * 8 + 5]) / 1e6 }
            val folder = File(instrumentation.targetContext.getExternalFilesDir(null), "liquid-performance").apply { mkdirs() }
            File(folder, "native.csv").writeText("encode_ns,native_ns,decode_ns,bytes,records\n" +
                (0 until probe.count).joinToString("\n") { i -> (0..4).joinToString(",") { probe.data[i * 5 + it].toString() } })
            File(folder, "frames.csv").writeText("total_ns,layout_ns,draw_ns,gpu_ns,deadline_ns,animation_ns,intended_vsync_ns,vsync_ns\n" +
                (0 until count).joinToString("\n") { i -> (0..7).joinToString(",") { metrics[i * 8 + it].toString() } })
            File(folder, "summary.json").writeText(JSONObject().put("controls", 16).put("refresh_hz", refresh)
                .put("display_frames", count).put("native_frames", probe.count).put("bridge_p95_ms", p95(native))
                .put("ui_p95_ms", p95(ui)).put("idle_frames", host.frames - idleFrames).put("geometry_errors", host.errors).toString(2))
            val budget = 1000.0 / refresh.coerceAtLeast(60f)
            assertTrue("batched JNI/geometry CPU exceeded display budget: ${p95(native)} ms", p95(native) <= budget)
            assertTrue("CPU UI exceeded display budget: ${p95(ui)} ms", p95(ui) <= budget)
            if (InstrumentationRegistry.getArguments().getString("require120") == "true") {
                assertTrue("display is not 120Hz: $refresh", refresh >= 119.5f)
            }
        }
    }
}
