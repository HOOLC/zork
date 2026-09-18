package ing.zork.android

import android.content.Intent
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.runtime.withFrameNanos
import androidx.lifecycle.lifecycleScope
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.math.abs

@RunWith(AndroidJUnit4::class)
class MarkdownStressTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private fun settle() { instrumentation.waitForIdleSync(); Thread.sleep(80); instrumentation.waitForIdleSync() }
    private fun move(scenario: ActivityScenario<MarkdownStressActivity>, index: Int) {
        val done = CountDownLatch(1)
        scenario.onActivity { a -> a.lifecycleScope.launch(androidx.compose.ui.platform.AndroidUiDispatcher.Main) {
            try { a.scroll.scrollToItem(index + 1) } finally { done.countDown() }
        } }
        assertTrue("Scroll did not complete", done.await(10, TimeUnit.SECONDS)); settle()
        scenario.onActivity { it.observe() }
    }
    @Test fun oneHundredThousandMixedMessagesStayVirtualized() {
        val context = instrumentation.targetContext
        val arguments = InstrumentationRegistry.getArguments()
        val physical = arguments.getString("physical") == "true"
        val deviceDensity = physical || arguments.getString("deviceDensity") == "true"
        val variant = arguments.getString("variant") ?: "current"
        val output = context.filesDir.resolve("markdown-performance").apply { mkdirs() }
        val budgetFailures = mutableListOf<String>()
        for (scale in listOf(1f, 1.4f)) {
            ActivityScenario.launch<MarkdownStressActivity>(Intent(context, MarkdownStressActivity::class.java)
                .putExtra("fontScale", scale).putExtra("messages", 100000).putExtra("physical", physical).putExtra("deviceDensity", deviceDensity)).use { scenario ->
                settle()
                for (index in 0 until markdownStressCases.size * 2) move(scenario, index)
                scenario.onActivity {
                    assertEquals(100000, it.records)
                    assertEquals("Every body kind and both roles must render", markdownStressCases.size * 2, it.seen.size)
                }
                for (anchor in listOf(0, 50000, 99940)) {
                    move(scenario, anchor)
                    val done = CountDownLatch(1)
                    var consumed = 0f
                    var elapsed = 0.0
                    var failure: Throwable? = null
                    val positions = mutableListOf<List<Int>>()
                    scenario.onActivity { a -> a.lifecycleScope.launch(androidx.compose.ui.platform.AndroidUiDispatcher.Main) {
                        try {
                            synchronized(a.frames) { a.frames.clear() }
                            a.measureStart = System.nanoTime(); a.measuring = true
                            val start = withFrameNanos { it }; var last = start; var observed = start
                            while (true) {
                                val now = withFrameNanos { it }
                                if (now - start >= 5_000_000_000L) { elapsed = (now - start) / 1e9; break }
                                val turn = (now - start) / 1_000_000_000L
                                val direction = (if (turn % 2 == 0L) 1 else -1) * if (anchor == 0) 1 else -1
                                consumed += abs(a.scroll.scrollBy(6000f * (now - last) / 1e9f * direction))
                                last = now
                                if (now - observed >= 200_000_000L) {
                                    observed = now; a.observe()
                                    positions.add(listOf(a.scroll.firstVisibleItemIndex, a.scroll.firstVisibleItemScrollOffset))
                                }
                            }
                        } catch (error: Throwable) { failure = error }
                        finally { a.measuring = false; done.countDown() }
                    } }
                    assertTrue("Frame-driven scrolling timed out", done.await(30, TimeUnit.SECONDS))
                    failure?.let { throw it }; settle()
                    scenario.onActivity { a ->
                        val frames = synchronized(a.frames) { a.frames.toList() }
                        val ui = frames.map { it.uiMs }.sorted()
                        val total = frames.map { it.totalMs }.sorted()
                        fun percentile(values: List<Double>, p: Double) = values.getOrElse(((values.size - 1) * p).toInt().coerceAtLeast(0)) { Double.POSITIVE_INFINITY }
                        val memory = android.os.Debug.MemoryInfo().also { android.os.Debug.getMemoryInfo(it) }
                        val result = JSONObject().put("records", a.records).put("body_kinds", JSONArray(markdownStressCases.map { it.first }))
                            .put("roles", JSONArray(listOf("user", "assistant"))).put("seen", JSONArray(a.seen.sorted())).put("font_scale", scale)
                            .put("viewport_dp", JSONArray(listOf(a.viewportWidth / a.renderDensity, a.viewportHeight / a.renderDensity)))
                            .put("viewport_pixels", JSONArray(listOf(a.viewportWidth, a.viewportHeight))).put("density", a.renderDensity)
                            .put("variant", variant).put("physical_device", physical).put("device_density", deviceDensity).put("device_model", android.os.Build.MODEL)
                            .put("display_hz", a.window.decorView.display.refreshRate).put("anchor", anchor)
                            .put("frames", frames.size).put("elapsed_seconds", elapsed).put("scroll_pixels", consumed)
                            .put("pixels_per_second", 6000).put("scroll_positions", JSONArray(positions))
                            .put("max_attached_text_views", a.maxTextViews).put("fixture_setup_ms", a.setupMs)
                            .put("unmeasured_max_ui_ms", a.coldMaxUiMs).put("ui_p95_ms", percentile(ui, .95)).put("ui_p99_ms", percentile(ui, .99))
                            .put("frame_total_p95_ms", percentile(total, .95)).put("frame_total_p99_ms", percentile(total, .99))
                            .put("process_pss_mib", memory.totalPss / 1024.0)
                            .put("parsed_documents", MessageMarkdownCache.parses).put("ast_cache_entries", MessageMarkdownCache.entries)
                            .put("ast_cache_estimated_bytes", MessageMarkdownCache.estimatedBytes)
                            .put("code_tokenizations", MessageCodeColors.tokenizations).put("code_cache_entries", MessageCodeColors.cachedEntries)
                            .put("code_cache_estimated_bytes", MessageCodeColors.estimatedBytes)
                            .put("measurement", "Android FrameMetrics: animation + layout/measure + draw; frame totals also reported")
                        output.resolve("scale-$scale-anchor-$anchor.json").writeText(result.toString(2))
                        instrumentation.sendStatus(0, android.os.Bundle().apply {
                            putString("stream", "ZORK_MARKDOWN_PERF ${result}\n")
                        })
                        assertTrue("Frame samples missing: ${frames.size}", frames.size >= 180)
                        assertTrue("The list did not actually scroll: $consumed", consumed >= 10000f && positions.distinct().size >= 10)
                        assertTrue("Too many attached message views: ${a.maxTextViews}", a.maxTextViews <= 40)
                        assertTrue("Too much eager Markdown parsing", MessageMarkdownCache.parses <= 2000)
                        assertTrue(MessageMarkdownCache.entries <= 384 && MessageMarkdownCache.estimatedBytes <= 8 * 1024 * 1024)
                        assertTrue(MessageCodeColors.cachedEntries <= 96 && MessageCodeColors.estimatedBytes <= 4 * 1024 * 1024)
                        if (percentile(ui, .95) > 1000.0 / 120.0) {
                            budgetFailures += "scale=$scale anchor=$anchor: ${percentile(ui, .95)} ms"
                        }
                    }
                }
            }
        }
        assertTrue("CPU UI budget exceeded: ${budgetFailures.joinToString()}", budgetFailures.isEmpty())
    }
}
