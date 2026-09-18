package ing.zork.android

import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.flow.first
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.json.JSONObject

/** Explicit physical-device acceptance against the user's already authorized peer.
 * Reads only: no enrollment, messages, tasks or configuration changes. */
@RunWith(AndroidJUnit4::class)
class SavedConnectionTest {
    @Test fun authorizedPeerReconnects() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val screen = ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java))
        val repository = ClientRepository(context)
        try {
            val state = repository.command("resume")
            val expected = InstrumentationRegistry.getArguments().getString("peer")
            val peer = state.getJSONArray("nodes").objects().first { expected == null || it.text("id") == expected }.text("id")
            val start = android.os.SystemClock.elapsedRealtime()
            val data = repository.command("request", "peer" to peer, "method" to "GET", "path" to "/v1/node/agents", "body" to null)
            val connected = withTimeout(20000) {
                repository.events(peer, null).first { it.value.optJSONObject("state")?.optBoolean("connected") == true }
                true
            }
            assertTrue("Saved peer never established its event stream", connected)
            val report = JSONObject().put("peer",peer).put("connected",connected)
                .put("elapsed_ms",android.os.SystemClock.elapsedRealtime()-start).put("response_received",data.length()>0)
            context.filesDir.resolve("saved-connection-test.json").writeText(report.toString())
        } finally { repository.command("pause"); screen.close() }
    }
    @Test fun foregroundRecoversAfterPeerRestart() = runBlocking<Unit> {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val screen = ActivityScenario.launch<Nav7PreviewActivity>(Intent(context, Nav7PreviewActivity::class.java))
        val repository = ClientRepository(context)
        val ready = context.filesDir.resolve("connection-restart-ready.json")
        val done = context.filesDir.resolve("connection-restart-result.json")
        ready.delete(); done.delete()
        try {
            val snapshot = repository.command("resume")
            val peer = snapshot.getJSONArray("nodes").objects().first().text("id")
            var wasConnected = false; var disconnected = false
            withTimeout(90000) {
              repository.events(peer, null).first { frame ->
                val state = frame.value.optJSONObject("state") ?: return@first false
                if (!state.has("connected")) return@first false
                if (state.optBoolean("connected")) {
                    if (!wasConnected) {
                        wasConnected = true
                        ready.writeText(JSONObject().put("ready",true).put("peer",peer).toString())
                    } else if (disconnected) {
                        repository.command("request", "peer" to peer, "method" to "GET", "path" to "/v1/node/agents", "body" to null)
                        done.writeText(JSONObject().put("reconnected",true).put("identity",snapshot.getString("identity")).toString())
                        return@first true
                    }
                } else if (wasConnected) disconnected = true
                false
              }
            }
        } finally { repository.command("pause"); screen.close() }
    }

}
