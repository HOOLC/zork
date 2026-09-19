package ing.zork.android

import android.app.Application
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Invoked only with isolated nodes by scripts/android/test_directory.py. */
@RunWith(AndroidJUnit4::class)
class MeshDirectoryTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val args = InstrumentationRegistry.getArguments()
    private val root = context.noBackupFilesDir.resolve("mesh-directory-fixture-" + requireNotNull(args.getString("fixture")))
    private val report = context.filesDir.resolve("mesh-directory-report.json")
    private fun call(op: String, vararg values: Pair<String, Any?>): JSONObject {
        val request = JSONObject().put("op", op)
        values.forEach { (key, value) -> request.put(key, value ?: JSONObject.NULL) }
        val response = JSONObject(NativeBridge.call(root.absolutePath, request.toString()))
        assertTrue(response.toString(), response.getBoolean("ok"))
        return response.getJSONObject("data")
    }
    private fun stage(value: String, identity: String) {
        report.writeText(JSONObject().put("stage", value).put("identity", identity).toString())
    }
    private suspend fun invitation(predicate: (JSONObject) -> Boolean): JSONObject = withTimeout(60000) {
        Observations(root.absolutePath).invitation().first { predicate(it.value.getJSONObject("snapshot")) }
            .value.getJSONObject("snapshot")
    }
    private suspend fun directory(vararg peers: String): JSONObject = withTimeout(60000) {
        Observations(root.absolutePath).directory().first {
            val snapshot = it.value.getJSONObject("snapshot")
            snapshot.optBoolean("running") && snapshot.optJSONArray("nodes").objects().map { node -> node.text("id") }.toSet() == peers.toSet()
        }.value.getJSONObject("snapshot")
    }
    private suspend fun read(peer: String) = withTimeout(60000) {
        while (true) {
            val value = call("read", "peer" to peer, "path" to "/v1/node/info")
            if (!value.optBoolean("cached", true)) {
                assertTrue(value.getJSONObject("snapshot").getJSONObject("body").getString("name").isNotBlank())
                break
            }
            delay(150)
        }
    }
    @Test fun approvalDynamicMembersAndRecovery() = runBlocking<Unit> {
        require(args.getString("isolated") == "true")
        val a = requireNotNull(args.getString("a")); val b = requireNotNull(args.getString("b")); val c = requireNotNull(args.getString("c"))
        val ticket = requireNotNull(args.getString("ticket"))
        NativeBridge.initialize(context)
        // This named test root belongs to this fixture, never the real app root.
        assertEquals(0, call("resume").getJSONArray("nodes").length())
        call("begin_invitation", "ticket" to ticket, "name" to "Android Mesh directory")
        val waiting = invitation { it.optJSONObject("invitation")?.optString("status") == "awaiting_approval" }
        val identity = waiting.getString("identity")
        stage("awaiting_approval", identity)
        invitation { it.optBoolean("done") && it.isNull("invitation") }
        directory(a, b); read(a); read(b)
        assertEquals("dev", root.resolve("channel").readText().trim())

        val store = ViewModelStore()
        lateinit var model: ClientViewModel
        instrumentation.runOnMainSync {
            model = ClientViewModel(context.applicationContext as Application, ClientRepository(context, root))
            store.put("fixture", model)
            model.foreground(true)
        }
        suspend fun uiPeers(vararg expected: String) = withTimeout(60000) {
            while (true) {
                var values = emptySet<String>()
                instrumentation.runOnMainSync { values = model.peers.map { it.id }.toSet() }
                if (values == expected.toSet()) break
                delay(20)
            }
        }
        try {
            uiPeers(a, b); stage("initial_members", identity)
            directory(a, b, c); uiPeers(a, b, c); read(c); stage("member_added", identity)
            directory(a, b); uiPeers(a, b); stage("member_removed", identity)
            withTimeout(60000) {
                val signal = context.filesDir.resolve("mesh-directory-step")
                while (!signal.isFile || signal.readText().trim() != "restart") delay(50)
            }
            call("pause"); assertEquals(identity, call("resume").getString("identity"))
            directory(a, b); uiPeers(a, b); read(b)

            // Channel rejection happens before a network or membership change.
            val flags = android.util.Base64.URL_SAFE or android.util.Base64.NO_PADDING or android.util.Base64.NO_WRAP
            val raw = android.util.Base64.decode(ticket.substring(4), flags)
            raw[48] = (raw[48].toInt() and 16.inv()).toByte()
            val otherChannel = "zc1_" + android.util.Base64.encodeToString(raw, flags)
            val rejected = JSONObject(NativeBridge.call(root.absolutePath, JSONObject().put("op", "begin_invitation")
                .put("ticket", otherChannel).put("name", "wrong channel").toString()))
            assertFalse(rejected.toString(), rejected.getBoolean("ok"))
            assertTrue(rejected.getString("error").contains("另一环境"))
            directory(a, b); read(b); stage("recovered", identity)
        } finally {
            instrumentation.runOnMainSync { model.foreground(false); store.clear() }
            call("pause")
        }
    }
    @Test fun reopenAfterProcessRestart() = runBlocking<Unit> {
        require(args.getString("isolated") == "true")
        NativeBridge.initialize(context)
        val old = JSONObject(report.readText()).getString("identity")
        assertEquals(old, call("resume").getString("identity"))
        directory(requireNotNull(args.getString("a")), requireNotNull(args.getString("b")))
        read(requireNotNull(args.getString("b")))
        stage("reopened", old)
        call("pause")
    }
}
