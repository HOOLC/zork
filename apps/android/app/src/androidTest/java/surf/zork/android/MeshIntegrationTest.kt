package surf.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.activity.compose.setContent
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.flow.first

/** All peers are created by scripts/android/test_mesh.py in a temporary fixture.
 * No production Gateway, model key or account is used. Each test invocation is
 * a fresh Android process sharing the app-private client database.
 */
@RunWith(AndroidJUnit4::class)
class MeshIntegrationTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val root = context.noBackupFilesDir.resolve("integration-client").absolutePath
    private val args = InstrumentationRegistry.getArguments()
    private val report = File(context.filesDir, "android-mesh-lab.json")

    private fun call(op: String, vararg fields: Pair<String, Any?>): JSONObject {
        val request = JSONObject().put("op", op)
        fields.forEach { (key, value) -> request.put(key, value ?: JSONObject.NULL) }
        val result = JSONObject(NativeBridge.call(root, request.toString()))
        assertTrue(result.toString(), result.optBoolean("ok"))
        return result.optJSONObject("data") ?: JSONObject()
    }

    private fun initialize() = NativeBridge.initialize(context.applicationContext)
    private fun request(peer: String, method: String, path: String, body: JSONObject? = null) =
        call("request", "peer" to peer, "method" to method, "path" to path, "body" to body)
    private fun setting(peer: String, action: String, fields: JSONObject = JSONObject()) =
        call("settings_action", "peer" to peer, "operation" to fields.put("action", action))
    private fun fakeMessage(text: String) = JSONObject().put("fake_tools", org.json.JSONArray().put(
        JSONObject().put("name", "chat.post_message").put("input",
            JSONObject().put("kind", "final").put("text", text))))

    @Test fun bootstrap() {
        // The fixture owns only this directory, never the normal client data.
        File(root).deleteRecursively()
        initialize()
        val snapshot = call("network", "network" to JSONObject().put("direct_only", true))
        assertTrue(snapshot.getString("identity").startsWith("key:"))
        report.writeText(snapshot.toString())
        call("pause")
        assertFalse(File(root, "mesh/synch/control.sock").exists())
    }

    @Test fun exchangeAndQueueBeforeProcessDeath() = runBlocking<Unit> {
        initialize()
        val snapshot = call("resume")
        assertEquals(JSONObject(report.readText()).getString("identity"), snapshot.getString("identity"))
        val peer = requireNotNull(args.getString("origin"))
        call("save_peer", "origin" to peer, "name" to "Android 测试设备", "address" to args.getString("address"))
        val agents = request(peer, "GET", "/v1/node/agents")
        assertTrue(agents.getJSONArray("items").length() > 0)
        val opened = request(peer, "POST", "/v1/node/agents/android-leader/open")
        val session = opened.getString("session_id")
        val content = fakeMessage("安卓连接成功。\n\n**消息与身份均通过真实 Mesh 传输。**").toString()
        call("draft", "peer" to peer, "session" to session, "content" to content)
        val queued = call("enqueue", "peer" to peer, "session" to session, "content" to content)
        val body = JSONObject().put("content", content).put("request_id", queued.getString("request_id"))
        // Replay the identical command before flushing the durable outbox. The
        // Gateway must reconcile it to one visible user message and one run.
        request(peer, "POST", "/v1/im/sessions/$session/messages", body)
        request(peer, "POST", "/v1/im/sessions/$session/messages", body)
        assertFalse(call("flush", "peer" to peer).has("error"))
        val observed = observeUntil(peer, session) { state -> state.optJSONArray("messages").objects().any { it.text("role") == "assistant" } }
        val items = observed.getJSONArray("messages").objects()
        assertEquals(1, items.count { it.text("role") == "user" && it.text("content") == content })
        assertEquals(1, items.count { it.text("role") == "assistant" })
        val denied = JSONObject(NativeBridge.call(root, JSONObject().put("op", "request").put("peer", peer)
            .put("method", "GET").put("path", "/v1/tools/context").put("body", JSONObject.NULL).toString()))
        assertFalse("Client escaped Gateway route allowlist", denied.getBoolean("ok"))
        call("pause")
        val cached = call("read", "peer" to peer, "path" to "/v1/im/sessions/$session/messages")
        assertTrue(cached.getBoolean("cached"))
        assertTrue(cached.getJSONObject("snapshot").getJSONObject("body").getJSONArray("items").length() >= 2)
        val pending = call("enqueue", "peer" to peer, "session" to session,
            "content" to fakeMessage("重启后收到待发送消息。身份和请求 ID 保持不变。").toString())
        call("draft", "peer" to peer, "session" to session, "content" to "未发送的中文草稿\n第二行 👋")
        report.writeText(snapshot.put("peer", peer).put("session", session).put("pending_id", pending.getString("request_id")).toString())
    }

    @Test fun settingsManageDevice() {
        initialize();call("resume")
        val peer=JSONObject(report.readText()).getString("peer")
        val before=request(peer,"GET","/v1/node/info").getString("name")
        assertTrue(request(peer,"GET","/v1/node/providers").getJSONArray("providers").length()>0)
        setting(peer, "rename_device", JSONObject().put("name", "移动端测试设备"))
        assertEquals("移动端测试设备",call("snapshot").getJSONArray("nodes").objects().first{it.text("id")==peer}.text("name"))
        setting(peer, "rename_device", JSONObject().put("name", before))
        val agent = JSONObject().put("id", "settings-worker").put("creating", true).put("name", "设置测试队员")
            .put("role", "worker").put("avatar", "bear").put("profile", "fixture").put("model", "fixture-model")
            .put("thinking", "off").put("instructions", "").put("allowed", org.json.JSONArray())
        setting(peer, "save_agent", JSONObject().put("input", agent))
        setting(peer, "save_agent", JSONObject().put("input", agent.put("creating", false).put("avatar", "owl")))
        assertEquals("owl",request(peer,"GET","/v1/node/agents").getJSONArray("items").objects().first{it.text("id")=="settings-worker"}.text("avatar"))
        setting(peer, "save_connection", JSONObject().put("input", JSONObject().put("id", "settings-fixture")
            .put("provider", "openai-compatible").put("billing", "usage").put("base_url", "http://127.0.0.1:9/v1").put("key", "isolated-test-key")))
        val input = JSONObject().put("previous", JSONObject.NULL).put("copied", JSONObject.NULL).put("id", "manual-model")
            .put("api", "openai-completions").put("context", "32K").put("output", "4K")
            .put("thinking", "off").put("default_thinking", "off").put("images", false)
        setting(peer, "save_model", JSONObject().put("profile", "settings-fixture").put("input", input))
        setting(peer, "enable_model", JSONObject().put("profile", "settings-fixture").put("model", "manual-model").put("enabled", false))
        val profile=request(peer,"GET","/v1/node/profiles/settings-fixture")
        val model=profile.getJSONArray("models").getJSONObject(0)
        assertEquals("manual-model",model.getString("id"))
        assertFalse(model.getBoolean("enabled"))
        assertFalse(profile.toString().contains("isolated-test-key"))
        setting(peer, "remove_model", JSONObject().put("profile", "settings-fixture").put("model", model))
        assertEquals(0,request(peer,"GET","/v1/node/profiles/settings-fixture").getJSONArray("models").length())
        setting(peer, "rename_profile", JSONObject().put("profile", "settings-fixture").put("name", "移动端连接"))
        assertEquals("移动端连接", request(peer,"GET","/v1/node/profiles/settings-fixture").getString("name"))
        assertTrue(call("diagnose_connections").getJSONArray("items").getJSONObject(0).getBoolean("reachable"))
        val settings=call("settings","peer" to peer)
        assertTrue(settings.toString(),settings.optBoolean("ready"))
        assertTrue(settings.getJSONArray("providers").length()>0)
        assertTrue(settings.getJSONArray("profiles").objects().any{it.text("profile_id")=="settings-fixture"})
        assertFalse(settings.toString().contains("isolated-test-key"))
        call("pause")
        assertFalse(call("diagnose_connections").getJSONArray("items").getJSONObject(0).getBoolean("reachable"))
        val cached=call("settings","peer" to peer,"cached_only" to true)
        assertEquals(settings.getJSONArray("profiles").toString(),cached.getJSONArray("profiles").toString())
        assertTrue(cached.optBoolean("cached"))
    }

    @Test fun restoreInNewProcess() {
        initialize()
        val saved = JSONObject(report.readText())
        val peer = saved.getString("peer")
        val session = saved.getString("session")
        val state = call("conversation", "peer" to peer, "session" to session)
        assertEquals("未发送的中文草稿\n第二行 👋", state.getString("draft"))
        assertEquals(saved.getString("pending_id"), state.getJSONArray("outbox").getJSONObject(0).getString("request_id"))
        assertEquals(saved.getString("identity"), call("resume").getString("identity"))
        assertEquals("failed", state.getJSONArray("outbox").getJSONObject(0).getString("delivery_status"))
        assertEquals(0, call("flush", "peer" to peer).getJSONArray("delivered").length())
        call("retry", "peer" to peer, "request_id" to saved.getString("pending_id"))
        val flushed = call("flush", "peer" to peer)
        assertFalse(flushed.toString(), flushed.has("error"))
        assertEquals(0, flushed.getJSONArray("outbox").length())
        val replay = call("flush", "peer" to peer)
        assertEquals(0, replay.getJSONArray("delivered").length())
        val page = call("read", "peer" to peer, "path" to "/v1/im/sessions/$session/messages")
            .getJSONObject("snapshot").getJSONObject("body")
        assertEquals(1, page.getJSONArray("items").objects().count {
            it.text("id") == "client-$session-${saved.getString("pending_id")}" })
        call("pause")
    }

    private fun applyFrame(previous: JSONObject, update: JSONObject?): JSONObject {
        if (update == null) return previous
        val messages = (update.optJSONArray("messages") ?: previous.optJSONArray("messages")).objects().toMutableList()
        update.optJSONArray("message_edits").objects().forEach { edit ->
            val start = edit.getInt("start"); val end = edit.getInt("end")
            messages.subList(start, end).clear()
            messages.addAll(start, edit.optJSONArray("insert").objects())
        }
        update.keys().forEach { key -> previous.put(key, update.get(key)) }
        previous.put("messages", org.json.JSONArray(messages))
        return previous
    }

    private suspend fun observeUntil(peer: String, session: String, ready: (JSONObject) -> Boolean): JSONObject {
        var state = JSONObject()
        withTimeout(30000) {
            Observations(root).conversation(peer, session).first { frame ->
                state = applyFrame(state, frame.value.optJSONObject("state")); ready(state)
            }
        }
        return state
    }

    @Test fun sharedStateOwnsDeliveryAndProjection() = runBlocking<Unit> {
        initialize()
        val saved = JSONObject(report.readText())
        val peer = saved.getString("peer")
        val session = saved.getString("session")
        call("resume")
        val reply = "共享 Rust 状态完成自动投递"
        val content = fakeMessage(reply).toString()
        val queued = call("enqueue", "peer" to peer, "session" to session, "content" to content)
        val messageId = "client-$session-${queued.getString("request_id")}"
        val state = observeUntil(peer, session) { state ->
            val rows = state.optJSONArray("messages").objects()
            state.optBoolean("can_send") && rows.none { it.optBoolean("pending") } && rows.any { it.text("content") == reply }
        }
        assertTrue(state.toString(), state.optJSONArray("messages").objects().none { it.optBoolean("pending") })
        assertEquals(1, state.optJSONArray("messages").objects().count { it.text("id") == messageId })
        assertEquals(1, state.optJSONArray("messages").objects().count { it.text("content") == reply })
        assertTrue(state.toString(), state.getBoolean("can_send"))
        // Only observations were used: Rust performs sending, catch-up,
        // stable-ID projection and disk caching without Kotlin orchestration.
        val cached = observeUntil(peer, session) { it.optJSONArray("messages").objects().any { row -> row.text("content") == reply } }
        assertTrue(cached.getJSONArray("messages").objects().any { it.text("content") == reply })
        call("pause")
    }

    @Test fun commentsAndTextFilesUseSharedDraftAndDelivery() = runBlocking<Unit> {
        initialize()
        val saved = JSONObject(report.readText()); val peer = saved.getString("peer"); val session = saved.getString("session")
        call("resume")
        val comments = org.json.JSONArray().put(JSONObject().put("id", "comment-shared")
            .put("source", JSONObject().put("session_id", session).put("message_id", "source-message").put("author", "Leader").put("quote", "原句"))
            .put("comment", "调整这里"))
        val files = org.json.JSONArray().put(JSONObject().put("id", "text-shared").put("name", "notes.md").put("content", "# 文本附件\n保持原文"))
        call("compose", "peer" to peer, "session" to session, "content" to "一起处理", "comments" to comments, "attachments" to files, "send" to false)
        val draft = call("conversation", "peer" to peer, "session" to session)
        assertEquals("一起处理", draft.getString("draft")); assertEquals(1, draft.getJSONArray("comments").length())
        assertEquals("# 文本附件\n保持原文", draft.getJSONArray("attachments").getJSONObject(0).getString("content"))
        val queued = call("compose", "peer" to peer, "session" to session, "content" to "一起处理", "comments" to comments, "attachments" to files, "send" to true)
        val id = "client-$session-${queued.getString("request_id")}"
        val delivered = observeUntil(peer, session) { it.optJSONArray("messages").objects().any { row -> row.text("id") == id && !row.optBoolean("pending") } }
        val found = delivered.optJSONArray("messages").objects().find { it.text("id") == id }
        assertNotNull("Composed message was not delivered", found)
        assertTrue(found!!.getString("display_content").contains("调整这里"))
        assertFalse(found!!.getString("display_content").contains("<zork-message-comments"))
        assertEquals(1, found!!.getJSONArray("attachments").length())
        assertEquals(0, call("conversation", "peer" to peer, "session" to session).getJSONArray("comments").length())
        call("pause")
    }

    @Test fun revocationRejectsClient() {
        initialize()
        val peer = JSONObject(report.readText()).getString("peer")
        call("resume")
        val result = JSONObject(NativeBridge.call(root, JSONObject().put("op", "request")
            .put("peer", peer).put("method", "GET").put("path", "/v1/node/agents").put("body", JSONObject.NULL).toString()))
        assertFalse("Revoked device retained access", result.getBoolean("ok"))
        call("pause")
    }

    @Test fun prepareExecutionHistoryFixture() {
        initialize()
        call("resume")
        val peer = requireNotNull(args.getString("origin"))
        call("save_peer", "origin" to peer, "name" to "执行历史测试设备", "address" to args.getString("address"))
        val session = requireNotNull(args.getString("session"))
        File(context.filesDir, "android-history-lab.json").writeText(JSONObject().put("peer", peer).put("session", session).toString())
        call("pause")
    }

    @Test fun sessionExecutionHistoryUsesNativeObservation() = runBlocking<Unit> {
        val saved = JSONObject(File(context.filesDir, "android-history-lab.json").readText())
        val peer = saved.getString("peer")
        val session = saved.getString("session")
        val repository = ClientRepository(context, File(root))
        val models = androidx.lifecycle.ViewModelStore()
        androidx.test.core.app.ActivityScenario.launch(Nav7PreviewActivity::class.java).use { scenario ->
            lateinit var model: ClientViewModel
            scenario.onActivity { activity ->
                model = ClientViewModel(context.applicationContext as android.app.Application, repository)
                models.put("history", model)
                activity.setContent { ZorkTheme {
                    model.sessionHistory?.let { state -> SessionHistoryPage(state, HistoryActions(
                        model::closeHistory, model::olderHistory, model::newerHistory, model::latestHistory,
                        model::retryHistory, model::historyDetail)) }
                } }
                model.foreground(true)
            }
            suspend fun awaitState(predicate: (ClientViewModel) -> Boolean) = withTimeout(20000) {
                while (true) {
                    var satisfied = false
                    scenario.onActivity { satisfied = predicate(model) }
                    if (satisfied) break
                    delay(20)
                }
            }
            try {
                awaitState { it.ready }
                scenario.onActivity {
                    model.selectPeer(Peer(peer, "历史测试设备", ""))
                    model.openHistory(session, "Android Leader")
                }
                awaitState { it.sessionHistory?.let { history -> history.status.loaded && !history.status.loading && history.entries.any { row -> row.title == "执行命令" } } == true }
                scenario.onActivity {
                    val history = model.sessionHistory!!
                    assertNull(history.status.error)
                    assertTrue(history.entries.size <= 100)
                    val entry = history.entries.first { it.title == "执行命令" }
                    model.historyDetail(entry.id)
                }
                awaitState { it.sessionHistory?.detail != null }
                scenario.onActivity {
                    val history = model.sessionHistory!!
                    assertTrue(history.detail!!.sections.any { it.code && it.chunks.any { chunk -> chunk.contains("shell.run") } })
                    history.detail = null
                    model.foreground(false)
                }
                withTimeout(20000) { while (repository.command("snapshot").optBoolean("running")) delay(20) }
                scenario.onActivity { model.foreground(true) }
                awaitState { it.sessionHistory?.detail != null }
                scenario.onActivity {
                    assertNull(model.sessionHistory!!.status.error)
                    model.historyDetail(null)
                    model.closeHistory()
                    assertNull(model.sessionHistory)
                }
                File(context.filesDir, "session-history-native.json").writeText(JSONObject()
                    .put("session", session).put("jni_history", true).put("record_detail", true).put("foreground_restores_detail", true).toString())
            } finally {
                scenario.onActivity { model.foreground(false); models.clear() }
                repository.command("pause")
            }
        }
    }

    @Test fun localEditsDuringStalledConnection() = runBlocking<Unit> {
        val repository = ClientRepository(context, File(root))
        repository.command("resume")
        val peer = "key:" + "y".repeat(52)
        repository.command("save_peer", "origin" to peer, "name" to "isolated blackhole",
            "address" to "10.0.2.2:9")
        val stalled = async {
            runCatching { repository.command("request", "peer" to peer, "method" to "GET",
                "path" to "/v1/node/info", "body" to null) }
        }
        delay(300)
        assertFalse("Expected an in-flight network request", stalled.isCompleted)
        withTimeout(2000) {
            repository.command("compose", "peer" to peer, "session" to "offline-chat", "content" to "网络等待时也能保存 👋",
                "comments" to org.json.JSONArray(), "attachments" to org.json.JSONArray(), "send" to false)
            val state = repository.command("conversation", "peer" to peer, "session" to "offline-chat")
            assertEquals("网络等待时也能保存 👋", state.getString("draft"))
        }
        assertFalse("Draft waited for network completion", stalled.isCompleted)
        assertTrue(withTimeout(40000) { stalled.await() }.isFailure)
        repository.command("remove_peer", "peer" to peer)
        repository.command("pause")
    }
}
