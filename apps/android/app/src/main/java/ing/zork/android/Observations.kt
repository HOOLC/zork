package ing.zork.android

import android.view.Choreographer
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.resume

internal data class MessageEdit(val start: Int, val end: Int, val insert: List<ChatMessage>)
internal data class ObservationFrame(val value: JSONObject, val messages: List<ChatMessage>?, val edits: List<MessageEdit>, val history: HistoryFrame? = null) {
    companion object {
        // Called on IO, including JSON -> display DTO conversion.
        fun decode(value: JSONObject): ObservationFrame {
            val state = value.optJSONObject("state")
            return ObservationFrame(value,
                state?.optJSONArray("messages")?.objects()?.map(::parseChatMessage),
                state?.optJSONArray("message_edits").objects().map {
                    MessageEdit(it.getInt("start"), it.getInt("end"), it.optJSONArray("insert").objects().map(::parseChatMessage))
                }, value.optJSONObject("history")?.let(HistoryFrame::decode))
        }
    }
}

/** Android scheduling only. Each observer has its own handle and applied
 * baseline. Only dirty hints conflate; a prepared frame is emitted and applied
 * synchronously before the next one can be read. */
internal class Observations(private val root: String) {
    private class Lease(val generation: Long, val peer: String, val session: String?) {
        val handle = AtomicLong()
        val frame = Mutex()
    }
    private val next = AtomicLong()
    private var active: Lease? = null
    private var activeHistory: Lease? = null

    private fun call(request: JSONObject): JSONObject {
        val result = JSONObject(NativeBridge.observe(root, request.toString()))
        check(result.optBoolean("ok")) { result.optString("error", "订阅读取失败") }
        return result.optJSONObject("data") ?: JSONObject()
    }
    private fun request(lease: Lease, op: String) = JSONObject().put("op", op)
        .put("handle", lease.handle.get()).put("generation", lease.generation)

    fun conversation(peer: String, session: String?) = frames("conversation", peer, session)
    fun settings(peer: String) = frames("settings", peer, null)
    fun history(peer: String, session: String) = frames("history", peer, session)
    fun invitation() = frames("invitation", "", null)
    fun directory() = frames("directory", "", null)
    fun notifications() = frames("notifications", "", null, frameAligned = false)
    fun localScripts() = frames("local_scripts", "", null, JSONObject().put("projection", "local_scripts"), frameAligned = false)
    fun adb() = frames("adb", "", null, JSONObject().put("projection", "adb"), frameAligned = false)
    fun account() = frames("account", "", null, JSONObject().put("projection", "account"), frameAligned = false)
    fun dataReset() = frames("data_reset", "", null, JSONObject().put("projection", "data_reset"), frameAligned = false)
    fun chatFiles() = frames("chat_files", "", null, JSONObject().put("projection", "chat_files"))
    fun sharedFiles() = frames("shared_files", "", null, JSONObject().put("projection", "shared_files"))
    fun resources(selection: ResourceSelection) = frames("resources", selection.peer.orEmpty(), null,
        JSONObject().put("projection", "resources").put("peer", selection.peer ?: JSONObject.NULL)
            .put("kind", selection.kind).put("query", selection.query?.let(::JSONObject) ?: JSONObject.NULL))

    suspend fun older(peer: String, session: String) {
        changeWindow(peer, session, "older")
    }
    suspend fun newer(peer: String, session: String) { changeWindow(peer, session, "newer") }
    suspend fun windowAnchor(peer: String, session: String, anchor: String?) { changeWindow(peer, session, "window", anchor) }
    suspend fun historyChange(peer: String, session: String, op: String, id: String? = null) {
        changeWindow(peer, session, op, id, history = true)
    }
    private suspend fun changeWindow(peer: String, session: String, op: String, anchor: String? = null, history: Boolean = false) {
        fun current() = if (history) activeHistory else active
        val lease = current()?.takeIf { it.peer == peer && it.session == session } ?: return
        lease.frame.withLock {
            if (current() !== lease) return@withLock
            val result = withContext(Dispatchers.IO) { runCatching {
                val query = request(lease, op)
                if (op == "window") query.put("anchor", anchor ?: JSONObject.NULL)
                if (op == "detail") query.put("id", anchor ?: JSONObject.NULL)
                call(query)
            } }
            if (current() === lease) result.getOrThrow()
        }
    }

    private fun frames(projection: String, peer: String, session: String?, keyOverride: JSONObject? = null, frameAligned: Boolean = true) = flow {
        coroutineScope {
            val lease = Lease(next.incrementAndGet(), peer, session)
            val dirty = Channel<Unit>(Channel.CONFLATED)
            val urgent = Channel<Unit>(Channel.CONFLATED)
            val closed = AtomicBoolean(false)
            try {
                withContext(Dispatchers.IO) {
                    val key = JSONObject().put("projection", projection)
                    if (projection !in listOf("invitation", "directory", "notifications")) key.put("peer", peer)
                    if (projection == "conversation" || projection == "history") key.put("session", session ?: JSONObject.NULL)
                    val opened = call(JSONObject().put("op", "open").put("generation", lease.generation).put("key", keyOverride ?: key))
                    // Publish the handle even if cancellation wins dispatch back
                    // to Main, so finally can always close it.
                    lease.handle.set(opened.getLong("handle"))
                }
                if (projection == "conversation") active = lease
                if (projection == "history") activeHistory = lease
                val listener = object : NativeObserver {
                    override fun onReady(handle: Long, generation: Long, urgentHint: Boolean, sourceClosed: Boolean) {
                        if (closed.get() || handle != lease.handle.get() || generation != lease.generation) return
                        if (sourceClosed) dirty.close()
                        else {
                            if (urgentHint) urgent.trySend(Unit)
                            dirty.trySend(Unit)
                        }
                    }
                }
                withContext(Dispatchers.IO) {
                    val result = JSONObject(NativeBridge.watch(root, lease.handle.get(), lease.generation, listener))
                    check(result.optBoolean("ok")) { result.optString("error", "订阅通知未建立") }
                }
                var initial = true
                var applied = 0L
                for (ignored in dirty) {
                    if (frameAligned && !initial && urgent.tryReceive().isFailure) {
                        val frame = async { nextFrame() }
                        try { select<Unit> { frame.onAwait { }; urgent.onReceive { } } }
                        finally { frame.cancel() }
                    }
                    lease.frame.withLock {
                        var batch = 0L
                        var displayed = false
                        try {
                            val prepared = withContext(Dispatchers.IO) {
                                val value = call(request(lease, "prepare"))
                                batch = value.optLong("batch")
                                check(value.getLong("handle") == lease.handle.get() && value.getLong("generation") == lease.generation) { "订阅来源不匹配" }
                                ObservationFrame.decode(value)
                            }
                            if (batch == 0L) return@withLock
                            val valid = withContext(Dispatchers.IO) {
                                call(request(lease, "validate").put("batch", batch)).getBoolean("valid")
                            }
                            if (!valid) return@withLock
                            check(prepared.value.optBoolean("reset") || prepared.value.getLong("from") == applied) { "订阅基线不匹配" }
                            currentCoroutineContext().ensureActive()
                            emit(prepared)
                            displayed = true
                            initial = false
                        } finally {
                            if (batch != 0L) withContext(NonCancellable + Dispatchers.IO) {
                                val accepted = call(request(lease, "finish").put("batch", batch).put("applied", displayed)).getBoolean("accepted")
                                if (displayed && accepted) applied = batch
                            }
                        }
                    }
                }
            } finally {
                closed.set(true)
                if (active === lease) active = null
                if (activeHistory === lease) activeHistory = null
                // A callback already in flight belongs to this closed lease;
                // its identity check cannot target a later A -> B -> A observer.
                withContext(NonCancellable + Dispatchers.IO) {
                    if (lease.handle.get() != 0L) call(request(lease, "close"))
                }
                dirty.cancel(); urgent.cancel()
            }
        }
    }
}

private suspend fun nextFrame() = withContext(Dispatchers.Main.immediate) { suspendCancellableCoroutine<Unit> { continuation ->
    val choreographer = Choreographer.getInstance()
    val callback = Choreographer.FrameCallback { if (continuation.isActive) continuation.resume(Unit) }
    choreographer.postFrameCallback(callback)
    continuation.invokeOnCancellation { choreographer.removeFrameCallback(callback) }
} }
