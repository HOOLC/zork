package ing.zork.android

import android.os.SystemClock
import android.view.ViewTreeObserver
import androidx.compose.ui.semantics.getOrNull
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Explicit device benchmark: opens an existing leader, never sends messages. */
@RunWith(AndroidJUnit4::class)
@OptIn(androidx.compose.ui.ExperimentalComposeUiApi::class)
class ConversationEntryLatencyTest {
    private fun composeRoot(view: android.view.View): androidx.compose.ui.platform.ViewRootForTest? {
        if (view is androidx.compose.ui.platform.ViewRootForTest) return view
        if (view is android.view.ViewGroup) for (i in 0 until view.childCount) composeRoot(view.getChildAt(i))?.let { return it }
        return null
    }
    private fun chatNode(root: androidx.compose.ui.platform.ViewRootForTest): androidx.compose.ui.semantics.SemanticsNode? {
        fun find(node: androidx.compose.ui.semantics.SemanticsNode): androidx.compose.ui.semantics.SemanticsNode? {
            if (node.config.getOrNull(androidx.compose.ui.semantics.SemanticsProperties.ContentDescription)?.contains("聊天页面") == true) return node
            for (child in node.children) find(child)?.let { return it }
            return null
        }
        return find(root.semanticsOwner.unmergedRootSemanticsNode)
    }
    private fun hasMessageView(view: android.view.View): Boolean {
        if (view is android.widget.TextView && view.tag is String && view.text.isNotEmpty() && view.width > 0) return true
        return view is android.view.ViewGroup && (0 until view.childCount).any { hasMessageView(view.getChildAt(it)) }
    }

    @Test fun existingLeaderActionToDraw() {
        org.junit.Assume.assumeTrue("Requires an explicitly selected connected device with existing history",
            InstrumentationRegistry.getArguments().getString("live_entry_benchmark") == "true")
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            scenario.onActivity { it.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
            fun waitUntil(predicate: (ClientViewModel) -> Boolean) {
                val deadline = SystemClock.uptimeMillis() + 20000
                while (SystemClock.uptimeMillis() < deadline) {
                    var ready = false
                    scenario.onActivity { ready = predicate(ViewModelProvider(it)[ClientViewModel::class.java]) }
                    if (ready) return
                    Thread.sleep(25)
                }
                fail("Conversation benchmark state did not become ready")
            }
            waitUntil { it.connected && it.leaders.isNotEmpty() && !it.busy }
            val results = JSONArray()
            val frameSamples = mutableListOf<Double>()
            val movingSamples = mutableListOf<Double>()
            val probeSamples = mutableListOf<Double>()
            var observedNode: androidx.compose.ui.semantics.SemanticsNode? = null
            scenario.onActivity { a ->
                val root = requireNotNull(composeRoot(a.window.decorView))
                val model = ViewModelProvider(a)[ClientViewModel::class.java]
                a.window.addOnFrameMetricsAvailableListener({ _, metrics, _ ->
                    val duration = (metrics.getMetric(android.view.FrameMetrics.LAYOUT_MEASURE_DURATION) +
                        metrics.getMetric(android.view.FrameMetrics.DRAW_DURATION)) / 1_000_000.0
                    synchronized(frameSamples) { frameSamples.add(duration) }
                    if (model.conversation == null && (observedNode?.boundsInRoot?.left ?: 0f) <= 0f) observedNode = chatNode(root)
                    observedNode?.boundsInRoot?.let { bounds ->
                        if (bounds.left > 0 && bounds.left < root.view.width) synchronized(movingSamples) { movingSamples.add(duration) }
                    }
                }, android.os.Handler(android.os.Looper.getMainLooper()))
            }
            repeat(5) { iteration ->
                scenario.onActivity { a -> ViewModelProvider(a)[ClientViewModel::class.java].let { if (it.conversation != null) it.back() } }
                waitUntil { it.conversation == null && it.connected && it.leaders.isNotEmpty() && !it.busy }
                Thread.sleep(250)
                val done = CountDownLatch(1)
                var firstDraw = 0L
                var populatedDraw = 0L
                var messageCount = 0
                lateinit var listener: ViewTreeObserver.OnDrawListener
                scenario.onActivity { a ->
                    val model = ViewModelProvider(a)[ClientViewModel::class.java]
                    val leader = model.leaders.first { it.text("session_id").isNotBlank() }
                    val root = requireNotNull(composeRoot(a.window.decorView))
                    observedNode = null
                    val start = SystemClock.elapsedRealtimeNanos()
                    listener = ViewTreeObserver.OnDrawListener {
                        val probeStart = SystemClock.elapsedRealtimeNanos()
                        if (model.conversation?.id == leader.text("session_id")) {
                            if (observedNode == null) observedNode = chatNode(root)
                            val bounds = observedNode?.boundsInRoot
                            if (bounds != null && bounds.width > 0 && bounds.height > 0 && bounds.left < root.view.width && bounds.right > 0) {
                                if (firstDraw == 0L) firstDraw = probeStart - start
                                if (model.messages.isNotEmpty() && populatedDraw == 0L && hasMessageView(a.window.decorView)) {
                                    populatedDraw = probeStart - start
                                    messageCount = model.messages.size
                                    done.countDown()
                                }
                            }
                        }
                        synchronized(probeSamples) { probeSamples.add((SystemClock.elapsedRealtimeNanos() - probeStart) / 1_000_000.0) }
                    }
                    a.window.decorView.viewTreeObserver.addOnDrawListener(listener)
                    model.openLeader(leader)
                    assertEquals("Known conversation must navigate without awaiting network", leader.text("session_id"), model.conversation?.id)
                    if (iteration > 0) {
                        assertFalse("Returning to loaded history must not flash loading", model.historyLoading)
                        assertTrue("Cached messages must be available immediately", model.messages.isNotEmpty())
                    }
                }
                val completed = done.await(5, TimeUnit.SECONDS)
                var diagnostic = ""
                if (!completed) scenario.onActivity { a ->
                    val model = ViewModelProvider(a)[ClientViewModel::class.java]
                    val root = composeRoot(a.window.decorView)
                    diagnostic = "first=$firstDraw messages=${model.messages.size} loading=${model.historyLoading} notice=${model.notice} bounds=${root?.let(::chatNode)?.boundsInRoot} view=${hasMessageView(a.window.decorView)}"
                }
                assertTrue("No populated conversation draw: $diagnostic", completed)
                scenario.onActivity { it.window.decorView.viewTreeObserver.removeOnDrawListener(listener) }
                results.put(JSONObject().put("shell_ms", firstDraw / 1_000_000.0).put("messages_ms", populatedDraw / 1_000_000.0).put("messages", messageCount))
                Thread.sleep(250)
            }
            scenario.onActivity { a ->
                val model = ViewModelProvider(a)[ClientViewModel::class.java]
                val leader = model.leaders.first { it.text("session_id").isNotBlank() }
                model.back()
                model.openLeader(leader)
                model.back()
                assertNull(model.conversation)
            }
            Thread.sleep(500)
            scenario.onActivity { assertNull("A late preparation must not reopen a conversation", ViewModelProvider(it)[ClientViewModel::class.java].conversation) }
            scenario.onActivity { a ->
                val model = ViewModelProvider(a)[ClientViewModel::class.java]
                model.openLeader(model.leaders.first { it.text("session_id").isNotBlank() })
            }
            waitUntil { it.messages.isNotEmpty() && !it.historyLoading }
            Thread.sleep(300)
            scenario.onActivity { ViewModelProvider(it)[ClientViewModel::class.java].back() }
            Thread.sleep(40)
            scenario.onActivity { a ->
                val model = ViewModelProvider(a)[ClientViewModel::class.java]
                model.openLeader(model.leaders.first { it.text("session_id").isNotBlank() })
            }
            waitUntil { it.messages.isNotEmpty() && !it.historyLoading }
            instrumentation.targetContext.filesDir.resolve("conversation-entry-latency.json").writeText(results.toString())
            val frames = synchronized(frameSamples) { frameSamples.sorted() }
            if (frames.isNotEmpty()) {
                val metrics = JSONObject().put("frames", frames.size)
                    .put("layout_draw_p95_ms", frames[((frames.size - 1) * .95).toInt()])
                    .put("layout_draw_p99_ms", frames[((frames.size - 1) * .99).toInt()])
                val moving = synchronized(movingSamples) { movingSamples.sorted() }
                metrics.put("moving_frames", moving.size)
                val probes = synchronized(probeSamples) { probeSamples.sorted() }
                if (probes.isNotEmpty()) metrics.put("probe_p95_ms", probes[((probes.size - 1) * .95).toInt()])
                if (moving.isNotEmpty()) metrics.put("moving_layout_draw_p95_ms", moving[((moving.size - 1) * .95).toInt()])
                    .put("moving_layout_draw_p99_ms", moving[((moving.size - 1) * .99).toInt()])
                instrumentation.targetContext.filesDir.resolve("conversation-transition-performance.json").writeText(metrics.toString())
            }
            val maximum = (0 until results.length()).maxOf { results.getJSONObject(it).getDouble("shell_ms") }
            assertTrue("Conversation first draw exceeds 100 ms: $maximum", maximum < 100.0)
        }
    }
}
