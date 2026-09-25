package ing.zork.android

import android.app.Application
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.referentialEqualityPolicy
import androidx.compose.runtime.snapshots.Snapshot
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.takeWhile
import kotlinx.coroutines.async
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

internal fun JSONArray?.objects(): List<JSONObject> =
    if (this == null) emptyList() else (0 until length()).mapNotNull { optJSONObject(it) }
internal fun JSONObject.text(key: String, fallback: String = ""): String =
    if (isNull(key)) fallback else optString(key, fallback)

/** A device as the core names it: `name` is the Mesh display name, `machine` the
 * name it registered with when that differs. */
internal data class Peer(val id: String, val name: String, val address: String, val status: DeviceStatusUi = DeviceStatusUi(),
    val machine: String? = null, val colorKey: String? = null) {
    /** Both names for accessibility, e.g. "B（zuozijians-Mac-Studio）". */
    val spokenName: String get() = machine?.let { "$name（$it）" } ?: name
}
internal data class Conversation(val id: String, val title: String, val leaderId: String? = null,
    val canSend: Boolean = true, val canStop: Boolean = canSend)
internal data class ChatMessage(val id: String, val author: String, val content: String,
    val user: Boolean, val pending: Boolean = false, val attempted: Boolean = false,
    val createdAt: String = "", val device: String = "", val model: String = "", val authorAgentId: String = "", val files: List<TextAttachmentUi> = emptyList(), val deliveryStatus: String = "", val requestId: String = "", val deliveryError: String = "", val interaction: InteractionCardUi? = null, val deliveredFiles: List<ChatFileUi> = emptyList(),
    val replyTo: String = "", val source: ObservedRow? = null)

/** The row exactly as the conversation observation sent it, for core
 * presentation (`NativeBridge.messagePresentation`). Equality ignores it: the
 * display fields above already change whenever the row does. */
internal class ObservedRow(val json: JSONObject) {
    override fun equals(other: Any?) = other is ObservedRow
    override fun hashCode() = 0
}

internal fun parseChatMessage(it: JSONObject) = ChatMessage(it.text("id"), it.text("author_name", if (it.text("role") == "user") "用户" else "助手"),
    it.text("display_content", it.text("content")), it.text("role") == "user", pending = it.optBoolean("pending"), attempted = it.optBoolean("attempted"),
    createdAt = it.text("created_at"), device = it.text("device"), model = it.text("model"), authorAgentId = it.text("author_agent_id"),
    files = it.textAttachments(), deliveryStatus = it.text("delivery_status"), requestId = it.text("request_id"),
    deliveryError = it.text("delivery_error"), interaction = it.optJSONObject("interaction_card")?.let(::parseInteractionCard),
    deliveredFiles = it.fileViews(), replyTo = it.text("reply_to"),
    source = ObservedRow(if (it.has("type")) it else it.put("type", "message")))

/** Core's refusal (`state::outbox::EMPTY_COMMENT_REPLY`) of a passage without a reply. */
internal const val EMPTY_COMMENT_REPLY = "每段引用都写一句回复，或者移除它"

internal data class TextAttachmentUi(val id: String, val name: String, val content: String, val caption: String = "文本附件") {
    fun json(): JSONObject = JSONObject().put("id", id).put("name", name).put("content", content)
}
internal fun JSONObject.textAttachments(): List<TextAttachmentUi> = optJSONArray("attachments").objects().map {
    TextAttachmentUi(it.text("id"), it.text("name"), it.text("content"))
}

internal data class DraftCommentUi(val id: String, val session: String, val messageId: String?,
    val author: String, val authorAgentId: String?, val quote: String, val text: String) {
    fun json(): JSONObject = JSONObject().put("id", id).put("comment", text).put("source",
        JSONObject().put("session_id", session).put("message_id", messageId ?: JSONObject.NULL)
            .put("author", author).put("author_agent_id", authorAgentId ?: JSONObject.NULL).put("quote", quote))
}
/** One row of the home list. Order, unread and section come from the core's
 * merged navigation projection; the UI never re-sorts them. */
internal data class HomeChat(val peer: String, val peerName: String, val id: String, val title: String,
    val description: String, val model: String, val unread: Boolean, val archived: Boolean,
    val archivePending: Boolean, val archiveError: String?, val messageCount: Long,
    val updatedAtMs: Long?, val section: String, val canSend: Boolean, val canStop: Boolean,
    val avatar: ChatAvatarUi = ChatAvatarUi(), val deviceLocal: Boolean = false, val deviceMachine: String? = null) {
    fun session(): JSONObject = JSONObject().put("_peer", peer).put("chat_id", id).put("title", title)
        .put("can_send", canSend).put("can_stop", canStop)
}
internal data class HomeNavigation(val chats: List<HomeChat> = emptyList(), val archived: List<HomeChat> = emptyList(),
    val archivedTotal: Int = 0, val loaded: Boolean = false)
internal fun parseHomeChat(it: JSONObject) = HomeChat(it.text("peer"), it.text("peer_name", it.text("peer")),
    it.text("chat_id"), it.text("title", "对话"), it.text("description"), it.text("model"), it.optBoolean("unread"),
    it.optBoolean("archived"), it.optBoolean("archive_pending"), it.text("archive_error").ifBlank { null },
    it.optLong("message_count"), if (it.isNull("updated_at_ms")) null else it.optLong("updated_at_ms"),
    it.text("section", "earlier"), it.optBoolean("can_send", true), it.optBoolean("can_stop"),
    parseChatAvatar(it.optJSONObject("avatar")), it.optJSONObject("device")?.optBoolean("local") ?: false,
    it.optJSONObject("device")?.text("machine")?.ifBlank { null })
internal fun parseHomeNavigation(value: JSONObject) = HomeNavigation(
    value.optJSONArray("chats").objects().map(::parseHomeChat),
    value.optJSONArray("archived").objects().map(::parseHomeChat),
    value.optInt("archived_total"), loaded = true)

internal data class DeviceTree(val leaders: List<JSONObject>, val sessions: List<JSONObject>,
    val tasksByLeader: Map<String, List<JSONObject>>, val online: Boolean = false)

internal class ClientViewModel(app: Application, private val repo: ClientRepository) : AndroidViewModel(app) {
    constructor(app: Application) : this(app, ClientRepository(app))
    var account by mutableStateOf<JSONObject?>(null)
        private set
    var accountError by mutableStateOf<String?>(null)
        private set
    private var accountWatch: Job? = null

        private set
    private var directoryWatch: Job? = null
        private set

    var identity by mutableStateOf("")
        private set
    var peers by mutableStateOf(emptyList<Peer>())
    /** This phone as its Mesh names it (core decides which device is local);
     * `null` until it belongs to a Mesh the client knows. */
    var localDevice by mutableStateOf<Peer?>(null)
        private set
    var activePeer by mutableStateOf<Peer?>(null)
        private set
    var conversation by mutableStateOf<Conversation?>(null)
        private set
    var leaders by mutableStateOf(emptyList<JSONObject>())
        private set
    var sessions by mutableStateOf(emptyList<JSONObject>())
        private set
    var tasksByLeader by mutableStateOf(emptyMap<String, List<JSONObject>>())
        private set
    var comments by mutableStateOf(emptyList<DraftCommentUi>())
        private set
    var attachments by mutableStateOf(emptyList<TextAttachmentUi>())
        private set
    /** Files core holds in the current draft. */
    var draftFiles by mutableStateOf(emptyList<ChatFileUi>())
        private set
    /** Picked files still being copied into core, or failed with a reason. */
    var attaching by mutableStateOf(emptyList<PendingFileUi>())
        private set
    var participants by mutableStateOf(emptyList<JSONObject>())
        private set
    var deviceTrees by mutableStateOf(emptyMap<String, DeviceTree>())
        private set
    /** Every device's Chats, merged and ordered by the core. */
    var home by mutableStateOf(HomeNavigation())
        private set
    private var navigationWatch: Job? = null
    var settings by mutableStateOf<MobileSettingsState?>(null)
        private set
    var newChat by mutableStateOf<NewChatUi?>(null)
        private set
    private var newChatWatch: Job? = null
    private var newChatGeneration = 0L
    /** "system", "light" or "dark", persisted by the client core. */
    var theme by mutableStateOf("system")
        private set
    var running by mutableStateOf(false)
        private set
    var conversationEntry by mutableLongStateOf(0L)
        private set
    var historyLoading by mutableStateOf(false)
        private set
    private var pendingConversationLoad: (() -> Unit)? = null
    private data class CachedConversation(
        val peer: String, val id: String, val messages: List<ChatMessage>, val pending: List<ChatMessage>,
        val participants: List<JSONObject>, val draft: String, val comments: List<DraftCommentUi>,
        val attachments: List<TextAttachmentUi>, val olderCursor: String?, val historyReady: Boolean,
    )
    private var cachedConversation: CachedConversation? = null
    private fun rememberConversation() {
        val peer = activePeer ?: return
        val current = conversation ?: return
        if (messages.isEmpty() && pending.isEmpty() && (historyLoading || notice != null)) return
        cachedConversation = CachedConversation(peer.id, current.id, messages, pending, participants,
            draft, comments, attachments, olderCursor, !historyLoading)
    }
    private val messageRows = mutableStateListOf<ChatMessage>()
    var messageRevision by mutableLongStateOf(0L)
        private set
    var messages by mutableStateOf<List<ChatMessage>>(emptyList(), referentialEqualityPolicy())
        private set
    private fun replaceMessages(rows: List<ChatMessage>) {
        loadingOlder = false; hasNewer = false
        messageRows.clear(); messageRows.addAll(rows)
        messages = messageRows.toList()
        messageRevision += 1
    }
    var pending by mutableStateOf(emptyList<ChatMessage>())
        private set
    var draft by mutableStateOf("")
        private set
    var olderCursor by mutableStateOf<String?>(null)
        private set
    var hasNewer by mutableStateOf(false)
        private set
    var loadingOlder by mutableStateOf(false)
        private set
    var busy by mutableStateOf(false)
        private set
    var notice by mutableStateOf<String?>(null)
        private set
    var connected by mutableStateOf(false)
        private set
    var activity by mutableStateOf("")
        private set
    var messageActivity by mutableStateOf(MessageActivity())
        private set
    var directOnly by mutableStateOf(false)
        private set
    var ready by mutableStateOf(false)
        private set
    private var foreground = false
    var chatFile by mutableStateOf<ChatFilePreviewUi?>(null)
        private set
    var chatFileImage by mutableStateOf<androidx.compose.ui.graphics.ImageBitmap?>(null)
        private set
    private var chatFilesWatch: Job? = null
    private var chatImageJob: Job? = null
    private var chatImageKey: String? = null
    private var live: Job? = null
    private var historyWatch: Job? = null
    var sessionHistory by mutableStateOf<SessionHistoryState?>(null)
        private set
    private var activeActions = 0
    var notificationSettings by mutableStateOf<JSONObject?>(null)
        private set
    var notificationError by mutableStateOf<String?>(null)
        private set
    private var notificationsWatch: Job? = null
    var adbSettings by mutableStateOf<JSONObject?>(null)
        private set
    var adbError by mutableStateOf<String?>(null)
        private set
    private var adbWatch: Job? = null
    var dataReset by mutableStateOf<JSONObject?>(null)
        private set
    var dataResetError by mutableStateOf<String?>(null)
        private set
    private var dataResetWatch: Job? = null
    private val adbPlatform = AdbPlatform(app, repo, viewModelScope)
    val localScripts = LocalScriptPlatform(app, repo, viewModelScope)
    private var pendingNotification: String? = null

    init {
        viewModelScope.launch {
            try {
                theme = repo.command("preferences").text("theme", "system")
                notificationSettings = repo.command("notification_settings")
            }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { notice = "外观设置读取失败：${e.message}" }
        }
    }

    suspend fun catalog(op: String, fields: JSONObject): Any? = repo.catalog(op, fields)
    suspend fun saveTheme(value: String) {
        // Keep the committed theme visible even if the user leaves the
        // appearance page while its local write is completing.
        viewModelScope.async {
            theme = repo.command("preferences", "theme" to value).text("theme", "system")
        }.await()
    }

    fun foreground(value: Boolean) {
        if (foreground == value) return
        val hostGeneration = android.os.SystemClock.elapsedRealtimeNanos()
        foreground = value
        live?.cancel()
        historyWatch?.cancel()
        directoryWatch?.cancel()
        navigationWatch?.cancel()
        if (value) { watchDataReset(); watchAccount() }
        if (value) action {
            adbPlatform.start()
            applySnapshot(repo.command("snapshot"))
            applySnapshot(repo.command("host_visibility", "visible" to true, "generation" to hostGeneration))
            ready = true
            refreshNotificationDelivery()
            watchAdb()
            watchDirectory()
            watchNavigation()
            reportVisibleConversation()
            pendingNotification?.let { openNotification(it) }
            settings?.device?.id?.let { watchSettings(it) }
            newChat?.let { watchNewChat(it.peer, newChatGeneration) }
            settings?.resource?.let { watchResources(it); refreshResources(it) }
            if (chatFile != null) watchChatFiles()
            if (foreground) { startLive(); startHistory() }
        } else {
            settingsWatch?.cancel()
            newChatWatch?.cancel()
            resourcesWatch?.cancel()
            chatFilesWatch?.cancel()
            notificationsWatch?.cancel()
            adbWatch?.cancel()
            dataResetWatch?.cancel()
            accountWatch?.cancel()
            adbPlatform.stop()
            NotificationPlatform.releaseHost(repo, hostGeneration)
            connected = false
        }
    }

    private fun watchAccount() {
        accountWatch?.cancel()
        accountWatch = viewModelScope.launch {
            try { repo.accountEvents().collect { frame -> account = frame.value.getJSONObject("snapshot"); accountError = null } }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { accountError = e.message }
        }
    }
    fun accountAction(action: String) {
        viewModelScope.launch {
            try { accountError = null; repo.command("account", "operation" to action) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { accountError = e.message }
        }
    }
    fun accountBrowserFailed() { accountError = "无法打开浏览器，请使用登录链接在浏览器中继续。" }

    private fun watchAdb() {
        if (!foreground) return
        adbWatch?.cancel()
        adbWatch = viewModelScope.launch {
            try {
                repo.adbEvents().collect { frame ->
                    adbSettings = frame.value.getJSONObject("snapshot")
                    AdbPlatform.updateService(getApplication(), adbSettings!!.optBoolean("service_requested"))
                    adbError = null
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { adbError = e.message ?: "调试状态读取失败" }
        }
    }
    private fun watchDataReset() {
        dataResetWatch?.cancel()
        dataResetWatch = viewModelScope.launch {
            try {
                repo.dataResetEvents().collect { frame -> dataReset = frame.value.getJSONObject("snapshot") }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { dataResetError = e.message }
        }
    }
    fun clearData() {
        viewModelScope.launch {
            dataResetError = null
            try { repo.clearData() }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { dataResetError = e.message }
        }
    }
    fun refreshAdb() {
        adbPlatform.refresh()
        viewModelScope.launch {
            try { repo.command("adb", "operation" to JSONObject().put("action", "refresh")); watchAdb() }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { adbError = e.message }
        }
    }
    suspend fun adbAction(operation: JSONObject) {
        viewModelScope.async {
            // The observation applies the current core snapshot and service intent.
            // A command reply may arrive after a newer connection update.
            repo.command("adb", "operation" to operation)
        }.await()
    }
    override fun onCleared() { adbPlatform.stop(); super.onCleared() }

    fun refreshNotificationDelivery() {
        if (!foreground) return
        notificationsWatch?.cancel()
        notificationsWatch = viewModelScope.launch {
            try {
                repo.notificationEvents().collect { frame ->
                    val data = frame.value.getJSONObject("snapshot")
                    notificationSettings = data.getJSONObject("settings")
                    NotificationPlatform.apply(getApplication(), repo, data)
                    NotificationPlatform.updateService(getApplication(), notificationSettings!!.optBoolean("service_requested"))
                    notificationError = null
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { notificationError = e.message ?: "通知未完成，请重新检查" }
        }
    }
    suspend fun notificationAction(operation: JSONObject) {
        viewModelScope.async {
            notificationSettings = repo.command("notification_settings", "operation" to operation)
            NotificationPlatform.updateService(getApplication(), notificationSettings!!.optBoolean("service_requested"))
            refreshNotificationDelivery()
        }.await()
    }
    suspend fun testNotification() {
        check(NotificationPlatform.post(getApplication(), repo.command("test_notification"))) { "系统未允许此类通知，请检查系统通知设置。" }
    }
    fun reportVisibleConversation() {
        if (!ready) return
        val peer = activePeer?.id
        val session = conversation?.id
        val visible = foreground && settings == null && session != null
        viewModelScope.launch { runCatching { repo.command("report_view", "peer" to peer, "session" to session, "visible" to visible) } }
    }
    fun openNotification(tag: String) {
        pendingNotification = tag
        if (!ready || !foreground) return
        pendingNotification = null
        action {
            val target = repo.command("open_notification", "tag" to tag)
            if (target.optBoolean("test")) { showSettings(); settingsPage("notifications"); return@action }
            val peer = peers.find { it.id == target.text("peer") } ?: error("通知对应的设备已不可用")
            repo.command("select_peer", "peer" to peer.id)
            activePeer = peer; settings = null
            openConversation(Conversation(target.getString("session"), target.text("title").ifBlank { "对话" }, target.text("leader")))
        }
    }

    private fun applyDirectory(value: JSONObject) {
        val previous = activePeer?.id
        peers = value.optJSONArray("nodes").objects().map {
            Peer(it.text("id"), it.text("name"), it.optJSONObject("mesh")?.text("addr") ?: "", it.deviceStatus(),
                it.text("machine_name").ifBlank { null }, it.text("color_key").ifBlank { null })
        }
        localDevice = value.optJSONObject("local")?.let {
            Peer(it.text("id"), it.text("name"), "", machine = it.text("machine_name").ifBlank { null }, colorKey = it.text("color_key").ifBlank { null })
        }
        DeviceColors.publish(peers)
        activePeer = peers.find { it.id == (previous ?: value.text("selected_peer")) }
        settings = settings?.let { current -> current.copy(device = peers.find { it.id == current.device?.id }) }
        newChat = newChat?.let { current -> peers.find { it.id == current.peer.id }?.let { current.copy(peer = it) } }
        deviceTrees = deviceTrees.filterKeys { id -> peers.any { it.id == id } }
        if (previous != null && activePeer == null) {
            live?.cancel(); closeHistory(); settingsWatch?.cancel()
            conversation = null; settings = null; connected = false
            replaceMessages(emptyList()); pending = emptyList(); participants = emptyList()
            leaders = emptyList(); sessions = emptyList(); tasksByLeader = emptyMap()
            cachedConversation = null
        }
    }

    private fun watchDirectory() {
        directoryWatch?.cancel()
        directoryWatch = viewModelScope.launch {
            try {
                repo.directoryEvents().collect { frame -> applyDirectory(frame.value.getJSONObject("snapshot")) }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { notice = error.message ?: "设备目录暂时不可用" }
        }
    }

    private fun watchNavigation() {
        navigationWatch?.cancel()
        navigationWatch = viewModelScope.launch {
            try {
                repo.navigationEvents().collect { frame -> home = parseHomeNavigation(frame.value.getJSONObject("snapshot")) }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { notice = error.message ?: "Chat 列表暂时不可用" }
        }
    }

    private fun applySnapshot(value: JSONObject) {
        identity = value.text("identity")
        directOnly = value.optJSONObject("network")?.optBoolean("direct_only") ?: false
        applyDirectory(value)
    }

    private fun action(block: suspend () -> Unit) {
        viewModelScope.launch {
            activeActions += 1
            busy = true
            notice = null
            try { block() }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { historyLoading = false; notice = error.message ?: "操作未完成，请重试" }
            finally { activeActions -= 1; busy = activeActions != 0 }
        }
    }

    /** What the open core file selection is for: a visible preview, a direct save, or another app. */
    private enum class FileIntent { Preview, Save, External }
    private var fileIntent by mutableStateOf(FileIntent.Preview)
    /** File whose selection should continue into a prepared copy once core opens it. */
    private var copyFollowUp: String? = null
    private var copyStarted = false
    private var externalTicket: String? = null
    /** The files of the message whose preview is open; the page steps through them. */
    var previewSequence by mutableStateOf(emptyList<Pair<String, ChatFileUi>>())
        private set
    /** A private copy handed to another app, with its type. MainActivity starts the viewer. */
    var externalFile by mutableStateOf<Pair<android.net.Uri, String>?>(null)
        private set
    var externalCopy by mutableStateOf(false)
        private set
    val chatFileVisible get() = chatFile != null && fileIntent != FileIntent.Save
    /** A prepared copy for the system save dialog; copies for other apps never go there. */
    val chatSaveTicket: String? get() = chatFile?.saveTicket?.takeIf { fileIntent != FileIntent.External }

    fun openChatFile(message: String, file: String) = openMessageFile(message, file, FileIntent.Preview)
    /** Saves without showing the preview; core still fetches and verifies the bytes. */
    fun saveMessageFile(message: String, file: String) = openMessageFile(message, file, FileIntent.Save)
    private fun openMessageFile(message: String, file: String, intent: FileIntent) {
        val peer = activePeer?.id ?: return
        val session = conversation?.id ?: return
        val files = messages.find { it.id == message }?.deliveredFiles.orEmpty()
        previewSequence = files.map { message to it }
        fileIntent = intent
        externalCopy = false
        copyStarted = false
        copyFollowUp = file.takeIf { intent == FileIntent.Save }
        watchChatFiles()
        chatFileAction("open", "peer" to peer, "session" to session, "message" to message, "file" to file)
    }
    fun stepChatFile(delta: Int) {
        val current = chatFile ?: return
        val index = previewSequence.indexOfFirst { it.second.id == current.fileId }
        val next = previewSequence.getOrNull(index + delta) ?: return
        openMessageFile(next.first, next.second.id, FileIntent.Preview)
    }
    fun closeChatFile() {
        chatFile?.let { chatFileAction("close", "key" to it.key) }
        previewSequence = emptyList(); fileIntent = FileIntent.Preview; copyFollowUp = null
    }
    fun saveOpenChatFile() {
        val file = chatFile ?: return
        fileIntent = FileIntent.Preview; externalCopy = false; copyStarted = true
        chatFileAction("prepare_save", "key" to file.key)
    }
    fun openChatFileExternally() {
        val file = chatFile ?: return
        fileIntent = FileIntent.External; externalCopy = true; copyStarted = true
        chatFileAction("prepare_save", "key" to file.key)
    }
    fun externalFileOpened() { externalFile = null }

    /** Continues a save-only or open-elsewhere selection as core's snapshot advances. */
    private fun followChatFile(next: ChatFilePreviewUi?) {
        if (next == null) { copyFollowUp = null; return }
        if (copyFollowUp == next.fileId && next.error != null && fileIntent == FileIntent.Save) {
            notice = next.error; closeChatFile(); return
        }
        if (copyFollowUp == next.fileId && !next.loading && !next.saving && next.saveTicket == null && next.error == null) {
            copyFollowUp = null; copyStarted = true
            chatFileAction("prepare_save", "key" to next.key)
            return
        }
        val ticket = next.saveTicket
        if (fileIntent == FileIntent.External && ticket != null && externalTicket != ticket) {
            externalTicket = ticket
            val mime = previewSequence.firstOrNull { it.second.id == next.fileId }?.second?.mime ?: next.mime
            viewModelScope.launch {
                try { externalFile = repo.openChatFileCopy(ticket, next.name) to mime }
                catch (e: CancellationException) { throw e }
                catch (e: Exception) { notice = e.message; chatFileAction("cancel_save", "ticket" to ticket) }
                finally { fileIntent = FileIntent.Preview }
            }
        }
        // A save-only selection ends once its copy is written, cancelled or failed.
        if (fileIntent == FileIntent.Save && copyStarted && !next.saving && ticket == null) {
            next.error?.let { notice = it }
            if (next.saved) notice = "已保存 ${next.name}"
            closeChatFile()
        }
    }
    fun chatFileAction(action: String, vararg fields: Pair<String, Any?>) {
        val operation = JSONObject().put("action", action)
        fields.forEach { (key, value) -> operation.put(key, value ?: JSONObject.NULL) }
        viewModelScope.launch {
            runCatching { repo.command("chat_files", "operation" to operation) }
                .onFailure { if (it !is CancellationException) notice = it.message }
        }
    }
    private fun watchChatFiles() {
        chatFilesWatch?.cancel()
        chatFilesWatch = viewModelScope.launch {
            try {
                repo.chatFileEvents().collect { frame ->
                    val next = frame.value.getJSONObject("snapshot").optJSONObject("preview")?.let(::parseChatFilePreview)
                    chatFile = next
                    followChatFile(next)
                    val key = next?.key?.takeIf { next.mime.startsWith("image/") && next.contentReady }
                    if (chatImageKey != key) {
                        chatImageKey = key
                        chatImageJob?.cancel(); chatFileImage = null
                        if (key != null) chatImageJob = viewModelScope.launch {
                            val bitmap = decodeFileImage(repo.chatPreviewBytes(key))
                            if (chatImageKey == key) chatFileImage = bitmap
                        }
                    }
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { notice = e.message }
        }
    }
    fun saveChatFile(uri: android.net.Uri?, ticket: String) {
        if (uri == null) { chatFileAction("cancel_save", "ticket" to ticket); return }
        viewModelScope.launch {
            try { repo.saveChatFile(uri, ticket) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { notice = e.message; chatFileAction("cancel_save", "ticket" to ticket) }
        }
    }

    fun changeDirectOnly(enabled: Boolean) = action {
        live?.cancel()
        applySnapshot(repo.command("network", "network" to JSONObject().put("direct_only", enabled)))
        startLive()
    }

    fun selectPeer(peer: Peer) {
        closeNewChat()
        closeHistory()
        rememberConversation()
        pendingConversationLoad = null
        historyLoading = false
        live?.cancel()
        activePeer = peer
        conversation = null
        leaders = emptyList(); sessions = emptyList(); tasksByLeader = emptyMap(); replaceMessages(emptyList()); pending = emptyList()
        connected = false; notice = null; activity = ""
        action { repo.command("select_peer", "peer" to peer.id); startLive() }
    }

    fun back() {
        if (newChat != null) { closeNewChat(); action { startLive() }; return }
        closeHistory()
        rememberConversation()
        pendingConversationLoad = null
        historyLoading = false
        live?.cancel()
        if (conversation != null) {
            conversation = null; replaceMessages(emptyList()); pending = emptyList(); activity = ""
            action { startLive() }
        } else {
            activePeer = null
            action { repo.command("select_peer", "peer" to null) }
        }
    }

    fun openLeader(leader: JSONObject) {
        if (leader.has("can_open") && !leader.optBoolean("can_open")) return
        rememberConversation()
        val peer = peers.find { it.id == leader.text("_peer") } ?: activePeer ?: return
        if (activePeer?.id != peer.id) { live?.cancel(); activePeer = peer; conversation = null; connected = false }
        val sessionId = leader.text("session_id")
        if (sessionId.isNotBlank()) {
            // The list already identifies this conversation. Show it and its local
            // history before waiting for the remote runtime to be prepared.
            openConversation(Conversation(sessionId, leader.text("name"), leader.text("id")),
                prepareLeader = leader.text("id"))
            return
        }
        action {
            val opened = repo.command("settings_action", "peer" to peer.id, "operation" to JSONObject().put("action", "open_agent").put("id", leader.text("id")))
            openConversation(Conversation(opened.text("session_id"), leader.text("name"), leader.text("id")))
        }
    }

    fun archiveChat(peer: String, chat: String, archived: Boolean, expectedMessageCount: Long) {
        // The home list observes every device, so archiving never switches devices.
        action { repo.command("archive_chat", "peer" to peer, "chat" to chat, "archived" to archived, "expected_message_count" to expectedMessageCount) }
    }

    /** Opens a Chat from the archived list in settings, leaving settings. */
    fun openArchivedChat(session: JSONObject) {
        settings = null
        openSession(session)
    }

    fun openSession(session: JSONObject) {
        rememberConversation()
        peers.find { it.id == session.text("_peer") }?.let { peer -> if (activePeer?.id != peer.id) {
            // The home list mixes devices: select the Chat's device first, as
            // the core remembers it as the last-used device.
            live?.cancel(); activePeer = peer; conversation = null; connected = false
            leaders = emptyList(); sessions = emptyList(); tasksByLeader = emptyMap(); participants = emptyList()
            viewModelScope.launch { runCatching { repo.command("select_peer", "peer" to peer.id) } }
        } }
        openConversation(Conversation(session.text("chat_id").ifBlank { session.text("session_id") }, session.text("title", "对话"),
            canSend = session.optBoolean("can_send", true), canStop = session.optBoolean("can_stop")))
    }

    private fun openConversation(value: Conversation, preparedDraft: String? = null, prepareLeader: String? = null) {
        if (value.id.isBlank()) return
        closeNewChat()
        closeHistory()
        rememberConversation()
        val cached = cachedConversation?.takeIf { it.peer == activePeer?.id && it.id == value.id }
        live?.cancel()
        conversation = value
        conversationEntry += 1
        messageActivity = MessageActivity()
        historyLoading = cached?.historyReady != true
        replaceMessages(cached?.messages.orEmpty()); pending = cached?.pending.orEmpty()
        comments = cached?.comments.orEmpty(); attachments = cached?.attachments.orEmpty()
        draftFiles = emptyList(); attaching = emptyList(); previewSequence = emptyList()
        participants = cached?.participants.orEmpty(); draft = cached?.draft.orEmpty()
        olderCursor = cached?.olderCursor; activity = ""
        val peerId = activePeer?.id
        pendingConversationLoad = { action {
            if (conversation !== value || activePeer?.id != peerId) return@action
            loadLocalConversation()
            if (conversation !== value || activePeer?.id != peerId) return@action
            if (preparedDraft != null) editDraft(listOf(draft, preparedDraft).filter { it.isNotBlank() }.joinToString("\n"))
            startLive(prepareLeader)
        } }
    }

    fun conversationShown() {
        val load = pendingConversationLoad ?: return
        pendingConversationLoad = null
        load()
    }

    private suspend fun loadLocalConversation() {
        val peer = activePeer ?: return
        val current = conversation ?: return
        val result = repo.command("conversation", "peer" to peer.id, "session" to current.id)
        if (peer != activePeer || current != conversation) return
        draft = result.text("draft")
        attachments = result.textAttachments()
        draftFiles = result.fileViews()
        comments = result.optJSONArray("comments").objects().map { c ->
            val source = c.getJSONObject("source")
            DraftCommentUi(c.text("id"), source.text("session_id"), source.text("message_id").ifBlank { null },
                source.text("author"), source.text("author_agent_id").ifBlank { null }, source.text("quote"), c.text("comment"))
        }
    }

    fun editDraft(value: String) {
        draft = value
        val peer = activePeer ?: return
        val current = conversation ?: return
        viewModelScope.launch {
            try { repo.command("draft", "peer" to peer.id, "session" to current.id, "content" to value) }
            catch (error: Exception) { if (error !is CancellationException) notice = "草稿尚未保存：${error.message}" }
        }
    }

    fun send() {
        val peer = activePeer ?: return
        val current = conversation ?: return
        val content = draft
        if (busy || !current.canSend) return
        action {
            try { repo.command("submit_draft", "peer" to peer.id, "session" to current.id, "text" to content) }
            catch (error: Exception) {
                // Core refuses a quoted passage without its reply; say so over the
                // composer instead of as a connection notice.
                if (error.message?.contains(EMPTY_COMMENT_REPLY) == true) { toast = EMPTY_COMMENT_REPLY; return@action }
                throw error
            }
            loadLocalConversation()
        }
    }
    /** Short text over the composer; see [EMPTY_COMMENT_REPLY]. */
    var toast by mutableStateOf<String?>(null)
    fun dismissToast() { toast = null }

    fun respondToInteraction(messageId: String, choice: String, values: Map<String, String>) {
        val peer = activePeer ?: return
        val current = conversation ?: return
        action {
            repo.command("respond_to_interaction", "peer" to peer.id, "session" to current.id,
                "operation" to JSONObject().put("action", "activate").put("message_id", messageId)
                    .put("choice", choice).put("values", JSONObject(values)))
        }
    }

    private var settingsWatch: Job? = null
    fun openNewChat(peer: Peer) {
        closeNewChat()
        closeHistory()
        rememberConversation()
        live?.cancel()
        settings = null
        conversation = null
        activePeer = peer
        newChat = NewChatUi(peer)
        val generation = newChatGeneration
        action {
            repo.command("select_peer", "peer" to peer.id)
            repo.command("new_chat", "peer" to peer.id, "operation" to JSONObject().put("action", "begin"))
            if (generation == newChatGeneration && newChat?.peer?.id == peer.id) watchNewChat(peer, generation)
        }
    }
    private fun closeNewChat() {
        newChatGeneration += 1
        newChatWatch?.cancel(); newChatWatch = null
        newChat = null
    }
    private fun watchNewChat(peer: Peer, generation: Long) {
        newChatWatch?.cancel()
        newChatWatch = viewModelScope.launch {
            try {
                repo.newChatEvents(peer.id).takeWhile { foreground && generation == newChatGeneration && newChat?.peer?.id == peer.id }.collect { frame ->
                    val snapshot = frame.value.optJSONObject("snapshot") ?: return@collect
                    if (generation != newChatGeneration) return@collect
                    newChat = NewChatUi(peers.find { it.id == peer.id } ?: peer, snapshot)
                    val chat = snapshot.optJSONObject("created")
                    if (chat != null && settings == null) openSession(JSONObject(chat.toString()).put("_peer", peer.id))
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                if (generation == newChatGeneration) newChat = newChat?.let { it.copy(snapshot = JSONObject(it.snapshot.toString()).put("error", error.message)) }
            }
        }
    }
    fun newChatAction(action: String, value: String?) {
        val current = newChat ?: return
        val generation = newChatGeneration
        val operation = JSONObject().put("action", action)
        if (value != null) operation.put(if (action == "edit" || action == "submit") "text" else "value", value)
        viewModelScope.launch {
            try { repo.command("new_chat", "peer" to current.peer.id, "operation" to operation) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                if (generation == newChatGeneration) newChat = newChat?.let { it.copy(snapshot = JSONObject(it.snapshot.toString()).put("error", error.message)) }
            }
        }
    }
    fun newChatModels() { newChat?.let { showDevice(it.peer); settingsPage("models") } }
    private fun watchSettings(peer: String) {
        settingsWatch?.cancel()
        settingsWatch = viewModelScope.launch {
            try {
                repo.settingsEvents(peer).takeWhile { foreground && settings?.device?.id == peer }.collect { frame ->
                    val result = frame.value
                    if (result.optBoolean("changed")) result.optJSONObject("snapshot")?.let {
                        applySettingsSnapshot(peer, it, settings?.loading == true)
                    }
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { if (settings?.device?.id == peer) settings = settings?.copy(message = e.message) }
        }
    }
    private fun applySettingsSnapshot(peer: String, snapshot: JSONObject, refreshing: Boolean) {
        val current = settings?.takeIf { it.device?.id == peer } ?: return
        if (snapshot.optBoolean("revoked")) {
            settings = current.copy(info = null, agents = emptyList(), profiles = emptyList(), providers = emptyList(), profile = null,
                authorization = null, authorizationBusy = false, authorizationComplete = false, authorizationError = null, operation = null,
                command = null, updateCheck = null, update = null, resourceData = null,
                loading = false, online = false, connectionState = "revoked", profilesReady = false, message = "设备访问权限已撤销")
            return
        }
        if (!snapshot.optBoolean("ready")) {
            settings = current.copy(loading = refreshing, message = snapshot.text("error").takeIf { it.isNotBlank() },
                online = snapshot.optBoolean("online", current.online))
            return
        }
        val info = snapshot.optJSONObject("info") ?: JSONObject()
        val profiles = snapshot.optJSONArray("profiles").objects()
        val selectedId = current.selectedProfileId ?: current.profile?.text("profile_id")
        val name = info.text("name").takeIf { it.isNotBlank() }
        settings = current.copy(authorization = snapshot.optJSONObject("authorization"), authorizationComplete = snapshot.optBoolean("authorization_complete"),
            authorizationBusy = snapshot.optBoolean("authorization_busy"), authorizationError = snapshot.text("authorization_error").takeIf { it.isNotBlank() },
            operation = snapshot.optJSONObject("operation"), info = info, device = name?.let { current.device?.copy(name = it) } ?: current.device,
            command = snapshot.optJSONObject("command"), updateCheck = snapshot.optJSONObject("update_check"), update = snapshot.optJSONObject("update"),
            connectionState = snapshot.text("connection_state", if (snapshot.optBoolean("online")) "online" else "offline"),
            profileRefreshing = snapshot.optJSONArray("profile_refreshing")?.let { a -> (0 until a.length()).map { a.getString(it) }.toSet() }.orEmpty(),
            profileFailed = snapshot.optJSONArray("profile_failed")?.let { a -> (0 until a.length()).map { a.getString(it) }.toSet() }.orEmpty(),
            agents = snapshot.optJSONArray("agents").objects(), profiles = profiles, providers = snapshot.optJSONArray("providers").objects(),
            profilesReady = snapshot.optBoolean("profiles_ready", true),
            profileMessage = snapshot.text("profiles_error").takeIf { it.isNotBlank() }?.let { "模型连接暂时无法刷新，已保留本地内容" },
            profile = profiles.find { it.text("profile_id") == selectedId }, loading = refreshing,
            online = snapshot.optBoolean("online", false), message = snapshot.text("error").takeIf { it.isNotBlank() })
    }
    fun showSettings() { showSettingsHome() }
    private fun showSettingsHome() {
        settingsWatch?.cancel()
        settings = MobileSettingsState(connections = settings?.connections)
        loadModelConnections(refresh = false)
    }
    private fun showModelConnections() {
        settingsWatch?.cancel()
        settings = MobileSettingsState(page = "model-connections", connections = settings?.connections)
        loadModelConnections(refresh = true)
    }
    private var connectionsJob: Job? = null
    /** Every device's connections from core: the cache first, then one read per device. */
    private fun loadModelConnections(refresh: Boolean) {
        connectionsJob?.cancel()
        connectionsJob = viewModelScope.launch {
            fun show(result: JSONObject, loading: Boolean) {
                val current = settings?.takeIf { it.device == null && it.page in listOf("home", "model-connections") } ?: return
                settings = current.copy(connections = result.optJSONArray("devices").objects(), loading = loading)
            }
            if (refresh) settings = settings?.copy(loading = true, message = null)
            try {
                show(repo.command("model_connections", "cached_only" to true), refresh)
                if (refresh) show(repo.command("model_connections"), false)
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                if (settings?.device == null) settings = settings?.copy(loading = false, message = e.message)
            }
        }
    }
    /** Opens one device's connection from the global list; back returns there. */
    fun openConnection(peerId: String, profile: JSONObject) {
        val peer = peers.find { it.id == peerId } ?: return
        showDevice(peer, fromConnections = true)
        settingsProfile(profile)
    }
    /** The add flow picks the device first, then opens that device's editor. */
    fun addConnection(peerId: String) {
        val peer = peers.find { it.id == peerId } ?: return
        showDevice(peer, fromConnections = true)
        settings = settings?.copy(page = "models", addConnection = true)
        if (settings?.online == true) action { settingsAction("open_models", JSONObject()) }
    }
    fun showDevice(peer: Peer, fromChat: Boolean = false, fromConnections: Boolean = false) {
        connectionsJob?.cancel()
        val tree = deviceTrees[peer.id]
        settings = MobileSettingsState(page = "device", device = peer, fromChat = fromChat, fromConnections = fromConnections,
            online = tree?.online ?: false, agents = tree?.leaders.orEmpty(), loading = true, profilesReady = false,
            connections = settings?.connections)
        refreshSettings()
        watchSettings(peer.id)
    }
    suspend fun settingsAction(action: String, fields: JSONObject): JSONObject {
        val peer=settings?.device ?: error("请先选择设备")
        val operation = JSONObject(fields.toString())
        val requestId = operation.remove("_request_id") as? String
        // A page/Activity may disappear while the core operation completes.
        return viewModelScope.async {
            try { repo.command("settings_action", "peer" to peer.id, "request_id" to requestId, "operation" to operation.put("action", action)) }
            finally { applySettingsSnapshot(peer.id, repo.command("settings", "peer" to peer.id, "cached_only" to true), false) }
        }.await()
    }
    fun refreshSettings() {
        val previous = settings ?: return
        if (previous.page == "model-connections") { loadModelConnections(refresh = true); return }
        previous.resource?.let { refreshResources(it); return }
        val peer = previous.device ?: return
        settings = previous.copy(loading = true, message = null)
        action {
            try {
                applySettingsSnapshot(peer.id, repo.command("settings", "peer" to peer.id, "cached_only" to true), true)
                applySettingsSnapshot(peer.id, repo.command("settings", "peer" to peer.id), false)
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                if (settings?.device?.id == peer.id) settings = settings?.copy(loading = false, message = e.message, online = false)
            }
        }
    }
    fun settingsPage(page: String) {
        settings = settings?.copy(page = page, resource = null, resourceData = null, message = null)
        resourceTrail.clear(); resourcesWatch?.cancel()
        when (page) {
            "models" -> if (settings?.online == true) action { settingsAction("open_models", JSONObject()) }
            "model-connections" -> showModelConnections()
            "services" -> showResource(ResourceSelection(settings?.device?.id, "service", title = "服务"))
        }
    }
    fun settingsProfile(profile: JSONObject) {
        settings = settings?.copy(page = "profile", profile = profile, selectedProfileId = profile.text("profile_id"), addConnection = false)
        if (settings?.online == true) action { settingsAction("open_profile", JSONObject().put("profile", profile.text("profile_id"))) }
    }
    private var resourcesWatch: Job? = null
    private val resourceTrail = mutableListOf<ResourceSelection>()
    private fun showResource(selection: ResourceSelection) {
        settings = settings?.copy(resource = selection, resourceData = null, resourceDepth = resourceTrail.size, message = null)
        watchResources(selection); refreshResources(selection)
    }
    private fun watchResources(selection: ResourceSelection) {
        resourcesWatch?.cancel()
        resourcesWatch = viewModelScope.launch {
            try {
                repo.resourceEvents(selection).takeWhile { foreground && settings?.resource == selection }.collect {
                    settings = settings?.copy(resourceData = it.value.optJSONObject("snapshot"))
                }
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) { if (settings?.resource == selection) settings = settings?.copy(message = e.message) }
        }
    }
    private fun refreshResources(selection: ResourceSelection) = action {
        repo.command("resources", "peer" to selection.peer, "query" to selection.query?.let(::JSONObject))
    }
    fun inspectResource(selection: ResourceSelection) {
        settings?.resource?.let { resourceTrail += it }
        showResource(selection)
    }

    fun backSettings() {
        if (settings?.resource != null && resourceTrail.isNotEmpty()) {
            showResource(resourceTrail.removeAt(resourceTrail.lastIndex)); return
        }
        resourcesWatch?.cancel()
        settings = settings?.copy(resource = null, resourceData = null)
        val current = settings
        when {
            current?.page == "home" -> settings = null
            current?.fromConnections == true && current.page in listOf("models", "profile") -> showModelConnections()
            current?.page in listOf("appearance", "notifications", "adb", "account", "model-connections", "archived") -> showSettingsHome()
            current?.page == "device" -> if (current.fromChat) settings = null else showSettingsHome()
            current?.page == "profile" -> settings = current.copy(page = "models")
            else -> settings = current?.copy(page = "device")
        }
    }
    /** Core tracks the check: its running state and outcome arrive as the snapshot's `update`. */
    fun checkUpdate() {
        if (settings?.device == null) return
        viewModelScope.launch {
            try { settingsAction("check_update", JSONObject().put("_request_id", NativeBridge.newId())) }
            catch (e: CancellationException) { throw e }
            catch (_: Exception) { /* Reported through update.error. */ }
        }
    }
    fun assistSettings(leader: JSONObject) = action {
        val targetDevice = settings?.device ?: return@action
        val peer = peers.find { it.id == leader.text("_peer") } ?: targetDevice
        settings = null
        if (activePeer?.id != peer.id) { activePeer = peer; conversation = null }
        val opened = repo.command("settings_action", "peer" to peer.id, "operation" to JSONObject().put("action", "open_agent").put("id", leader.text("id")))
        openConversation(Conversation(opened.text("session_id"), leader.text("name"), leader.text("id")),
            preparedDraft = "帮我看看 ${targetDevice.name} 的设备配置。")
    }

    /**
     * Picked, shot or shared files. Each shows as a reading chip until core has
     * snapshotted it into the draft of the conversation it was picked for.
     */
    fun attachFiles(uris: List<android.net.Uri>, peer: String, session: String) {
        uris.forEach { uri ->
            val pending = PendingFileUi(NativeBridge.newId(), uri.lastPathSegment?.substringAfterLast('/') ?: "文件", session = session)
            attaching = attaching + pending
            viewModelScope.launch {
                try {
                    repo.attachFile(uri, peer, session)
                    attaching = attaching.filterNot { it.key == pending.key }
                } catch (e: CancellationException) { throw e }
                catch (e: Exception) {
                    attaching = attaching.map { if (it.key == pending.key) it.copy(error = e.message ?: "无法添加文件") else it }
                }
            }
        }
    }
    fun dismissPendingFile(key: String) { attaching = attaching.filterNot { it.key == key } }
    fun showNotice(text: String) { notice = text }
    fun removeDraftFile(id: String) = action {
        val peer = activePeer ?: return@action
        val current = conversation ?: return@action
        repo.command("remove_file", "peer" to peer.id, "session" to current.id, "id" to id)
    }
    fun cameraTarget(): android.net.Uri = repo.cameraTarget()

    private val fileImages = android.util.LruCache<String, androidx.compose.ui.graphics.ImageBitmap>(32)
    /** Verified bytes come from core; decoding stays bounded and cached per size. */
    suspend fun fileImage(message: String?, file: ChatFileUi, maxSide: Int): androidx.compose.ui.graphics.ImageBitmap? {
        if (!file.thumbnail) return null
        val peer = activePeer?.id ?: return null
        val session = conversation?.id ?: return null
        val key = "$peer/$session/${message.orEmpty()}/${file.id}/$maxSide"
        fileImages.get(key)?.let { return it }
        val image = decodeFileImage(repo.fileThumbnail(peer, session, message, file.id), maxSide) ?: return null
        fileImages.put(key, image)
        return image
    }

    /** Draft file shown in the preview page; draft bytes never leave core except to decode. */
    var draftPreview by mutableStateOf<ChatFileUi?>(null)
        private set
    fun openDraftFile(file: ChatFileUi?) { draftPreview = file }

    fun removeAttachment(id: String) = draftAction(JSONObject().put("action", "remove_attachment").put("id", id))
    fun exportTextAttachment(uri: android.net.Uri, file: TextAttachmentUi) = action {
        repo.exportText(uri, file.content)
        notice = "已保存 ${file.name}"
    }
    private fun draftAction(operation: JSONObject) = action {
        val peer = activePeer ?: return@action
        val current = conversation ?: return@action
        repo.command("draft_action", "peer" to peer.id, "session" to current.id, "operation" to operation)
    }

    fun assistAddDevice(leader: JSONObject) = action {
        val peer = peers.find { it.id == leader.text("_peer") } ?: activePeer ?: return@action
        settings = null; if (activePeer?.id != peer.id) { live?.cancel(); activePeer = peer; conversation = null }
        val opened = repo.command("settings_action", "peer" to peer.id, "operation" to JSONObject().put("action", "open_agent").put("id", leader.text("id")))
        openConversation(Conversation(opened.text("session_id"), leader.text("name"), leader.text("id")),
            preparedDraft = "我想添加一台新设备，请帮我准备接入步骤。")
    }

    fun saveComment(comment: DraftCommentUi) = draftAction(JSONObject().put("action", "put_comment").put("comment", comment.json()))
    /** Selected text → a draft passage with an empty reply, written in the composer. */
    fun quoteReply(row: ChatMessage, quote: String) {
        val current = conversation ?: return
        saveComment(DraftCommentUi(NativeBridge.newId(), current.id, row.id, row.author,
            row.authorAgentId.ifBlank { null }, quote, ""))
    }
    /** Typing a passage's reply saves it like the main draft: no busy state. */
    fun editCommentText(comment: DraftCommentUi) {
        val peer = activePeer ?: return
        val current = conversation ?: return
        comments = comments.map { if (it.id == comment.id) comment else it }
        viewModelScope.launch {
            try { repo.command("draft_action", "peer" to peer.id, "session" to current.id,
                "operation" to JSONObject().put("action", "put_comment").put("comment", comment.json())) }
            catch (error: Exception) { if (error !is CancellationException) notice = "草稿尚未保存：${error.message}" }
        }
    }
    fun removeComment(id: String) = draftAction(JSONObject().put("action", "remove_comment").put("id", id))

    fun withdraw(id: String) = action {
        val peer = activePeer ?: return@action
        repo.command("withdraw", "peer" to peer.id, "request_id" to id)
        loadLocalConversation()
    }

    fun resend(id: String) = action {
        val peer = activePeer ?: return@action
        repo.command("retry", "peer" to peer.id, "request_id" to id)
        loadLocalConversation()
    }
    fun deleteFailed(id: String) = action {
        val peer = activePeer ?: return@action
        repo.command("delete_failed", "peer" to peer.id, "request_id" to id)
        loadLocalConversation()
    }

    fun retry() = action { repo.command("resume"); startLive() }

    fun openHistory(session: String, name: String) {
        val peer = activePeer ?: return
        if (session.isBlank()) return
        historyWatch?.cancel()
        sessionHistory = SessionHistoryState(peer.id, session, name)
        startHistory()
    }
    fun closeHistory() {
        historyWatch?.cancel()
        historyWatch = null
        sessionHistory = null
    }
    private fun startHistory() {
        historyWatch?.cancel()
        val current = sessionHistory ?: return
        if (!foreground) return
        historyWatch = viewModelScope.launch {
            try {
                var restoringDetail = current.selectedId
                repo.historyEvents(current.peer, current.session).collect { frame ->
                    if (sessionHistory !== current || !foreground) return@collect
                    frame.history?.let { history -> Snapshot.withMutableSnapshot { current.apply(history) } }
                    val id = restoringDetail
                    if (id != null && current.selectedId != id) restoringDetail = null
                    else if (id != null && current.status.loaded && !current.status.loading) {
                        restoringDetail = null
                        launch {
                            // The collection callback returns before taking the
                            // same observer's frame gate to restore its detail.
                            try {
                                if (sessionHistory === current && current.selectedId == id)
                                    repo.historyDetail(current.peer, current.session, id)
                            }
                            catch (cancelled: CancellationException) { throw cancelled }
                            catch (error: Exception) {
                                if (sessionHistory === current) current.status = current.status.copy(loading = false, error = error.message)
                            }
                        }
                    }
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                if (sessionHistory === current) current.status = current.status.copy(loading = false, error = error.message ?: "执行历史加载失败")
            }
        }
    }
    private fun historyAction(block: suspend (SessionHistoryState) -> Unit) {
        val current = sessionHistory ?: return
        viewModelScope.launch {
            try { block(current) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                if (sessionHistory === current) current.status = current.status.copy(loading = false, error = error.message ?: "执行历史操作失败")
            }
        }
    }
    fun historyAnchor(id: String?) = historyAction { repo.historyAnchor(it.peer, it.session, id) }
    fun historyNavigate(target: HistoryDestination) {
        openConversation(Conversation(target.id, target.title, canSend = target.canSend, canStop = target.canStop))
    }
    fun olderHistory() = historyAction { repo.historyOlder(it.peer, it.session) }
    fun newerHistory() = historyAction { repo.historyNewer(it.peer, it.session) }
    fun latestHistory() = historyAction { repo.historyLatest(it.peer, it.session) }
    fun retryHistory() {
        if (historyWatch?.isActive != true) startHistory()
        else historyAction { repo.historyRefresh(it.peer, it.session) }
    }
    fun historyDetail(id: String?) {
        val current = sessionHistory ?: return
        current.selectedId = id
        current.detail = null
        historyAction { repo.historyDetail(it.peer, it.session, id) }
    }

    fun stop() = action {
        val peer = activePeer ?: return@action
        val current = conversation ?: return@action
        repo.command("settings_action", "peer" to peer.id, "operation" to JSONObject().put("action", "stop_conversation").put("session", current.id))
    }

    fun older() = action {
        val peer = activePeer ?: return@action
        val current = conversation ?: return@action
        repo.older(peer.id, current.id)
    }
    fun newer() = action {
        val peer = activePeer ?: return@action
        val current = conversation ?: return@action
        repo.newer(peer.id, current.id)
    }
    fun windowAnchor(anchor: String?) {
        val peer = activePeer ?: return
        val current = conversation ?: return
        viewModelScope.launch {
            try { repo.windowAnchor(peer.id, current.id, anchor) }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { if (activePeer?.id == peer.id && conversation?.id == current.id) notice = error.message }
        }
    }

    private fun startLive(prepareLeader: String? = null) {
        live?.cancel()
        val peer = activePeer ?: return
        if (!foreground) return
        val current = conversation
        live = viewModelScope.launch {
            try {
                var initial = true
                repo.events(peer.id, current?.id).takeWhile { foreground && activePeer?.id == peer.id && conversation?.id == current?.id }.collect { frame ->
                    Snapshot.withMutableSnapshot { applyState(frame) }
                    if (initial) {
                        initial = false
                        if (prepareLeader != null) launch { repo.command("settings_action", "peer" to peer.id,
                            "operation" to JSONObject().put("action", "prepare_agent").put("id", prepareLeader)) }
                    }
                }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) {
                connected = false
                historyLoading = false
                notice = error.message ?: "客户端连接未建立，请重试"
            }
        }
    }

    // Rust owns connection recovery, history, message identity, permissions and
    // outbox delivery. Kotlin maps the snapshot to display models only.
    private fun applyState(frame: ObservationFrame) {
        val state = frame.value.optJSONObject("state")
        if (state == null || state.text("peer") != activePeer?.id ||
            state.text("session") != (conversation?.id ?: "")) return
        if(state.has("nodes")) {
            val names=state.optJSONArray("nodes").objects().associate { it.text("id") to it.text("name") }
            peers=peers.map { p -> names[p.id]?.takeIf{it.isNotBlank()}?.let{p.copy(name=it)} ?: p }
            activePeer=activePeer?.let{p -> names[p.id]?.takeIf{it.isNotBlank()}?.let{p.copy(name=it)} ?: p}
            settings=settings?.let{current -> current.copy(device=current.device?.let{p -> names[p.id]?.takeIf{it.isNotBlank()}?.let{p.copy(name=it)} ?: p})}
        }
        state.optJSONObject("message_arrivals")?.let { arrivals ->
            val count = arrivals.optLong("count")
            if (count > 0) {
                val sequence = messageActivity.sequence + count
                val ids = arrivals.optJSONArray("ids")
                val size = ids?.length() ?: 0
                val now = android.os.SystemClock.uptimeMillis()
                val recent = messageActivity.recent.filter { now - it.startedAt < 250 } +
                    (0 until size).map { MessageArrival(ids!!.getString(it), sequence - size + it + 1, now) }
                messageActivity = MessageActivity(sequence, recent.takeLast(32))
            }
        }
        if (state.has("connected")) connected = state.optBoolean("connected")
        if (state.has("error")) notice = state.text("error").ifBlank { null }
        state.optJSONObject("navigation")?.let { navigation ->
            leaders = navigation.optJSONArray("agents").objects()
            sessions = navigation.optJSONArray("chats").objects()
            val groups = navigation.optJSONObject("tasks")
            tasksByLeader = groups?.keys()?.asSequence()?.associateWith { groups.optJSONArray(it).objects() } ?: emptyMap()
        }
        frame.messages?.let(::replaceMessages)
        for (edit in frame.edits) {
            check(edit.start >= 0 && edit.end >= edit.start && edit.end <= messageRows.size) { "消息增量超出已应用范围" }
            messageRows.subList(edit.start, edit.end).clear()
            messageRows.addAll(edit.start, edit.insert)
        }
        if (frame.edits.isNotEmpty()) { messages = messageRows.toList(); messageRevision += 1 }
        if (state.optBoolean("unified_transcript")) pending = emptyList()
        if (messages.isNotEmpty() || state.optBoolean("loaded") || notice != null) historyLoading = false
        if (state.has("loading_older")) loadingOlder = state.optBoolean("loading_older")
        if (state.has("newer_available")) hasNewer = state.optBoolean("newer_available")
        state.optJSONObject("draft_document")?.let { document ->
            attachments = document.textAttachments()
            draftFiles = document.fileViews()
            comments = document.optJSONArray("comments").objects().map { c ->
                val source = c.getJSONObject("source")
                DraftCommentUi(c.text("id"), source.text("session_id"), source.text("message_id").ifBlank { null },
                    source.text("author"), source.text("author_agent_id").ifBlank { null }, source.text("quote"), c.text("comment"))
            }
        }
        if (state.has("participants")) participants = state.optJSONArray("participants").objects()
        if (state.has("navigation") || state.has("connected"))
            activePeer?.let { deviceTrees = deviceTrees + (it.id to DeviceTree(leaders, sessions, tasksByLeader, connected)) }
        if (state.has("running")) running = state.optBoolean("running")
        if (state.has("older_cursor")) olderCursor = state.text("older_cursor").ifBlank { null }
        if (state.has("can_send")) conversation = conversation?.copy(canSend = state.optBoolean("can_send"), canStop = state.optBoolean("can_stop"))
        if (state.has("activity")) activity = if (state.optBoolean("stop_pending")) "已请求停止，等待设备确认" else
            activityLabel(state.optJSONObject("activity"))
    }
}
