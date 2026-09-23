package ing.zork.android

import androidx.compose.animation.core.updateTransition
import androidx.compose.animation.core.animateFloat
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.PointerEventPass
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.Canvas
import androidx.compose.ui.graphics.rememberGraphicsLayer
import androidx.compose.ui.graphics.layer.drawLayer
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.preferredFrameRate
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import org.json.JSONObject
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter

internal data class MessageArrival(val id: String, val sequence: Long, val startedAt: Long)
internal data class MessageActivity(val sequence: Long = 0, val recent: List<MessageArrival> = emptyList())

// App-owned UI state only. Protocol and conversation rules stay in Rust.
internal data class WorkbenchState(
    // Compare presentation invalidation before collection fields. The core's
    // applied cursor lives in the observer; data-class equality never scans a
    // large transcript just to discover its already-known mutation.
    val messageRevision: Long = 0L,
    val peers: List<Peer> = emptyList(), val activePeer: Peer? = null,
    val conversation: Conversation? = null, val leaders: List<JSONObject> = emptyList(),
    val sessions: List<JSONObject> = emptyList(), val tasksByLeader: Map<String, List<JSONObject>> = emptyMap(),
    val messages: List<ChatMessage> = emptyList(), val pending: List<ChatMessage> = emptyList(),
    val draft: String = "", val older: Boolean = false, val busy: Boolean = false,
    val ready: Boolean = true, val connected: Boolean = false, val notice: String? = null,
    val activity: String = "", val running: Boolean = false,
    val comments: List<DraftCommentUi> = emptyList(), val participants: List<JSONObject> = emptyList(),
    val deviceTrees: Map<String, DeviceTree> = emptyMap(),
    val attachments: List<TextAttachmentUi> = emptyList(),
    val historyLoading: Boolean = false,
    val conversationEntry: Long = 0L,
    val messageActivity: MessageActivity = MessageActivity(),
    val newer: Boolean = false,
)
internal class WorkbenchActions(
    val peer: (Peer) -> Unit = {}, val leader: (JSONObject) -> Unit = {},
    val session: (JSONObject) -> Unit = {}, val back: () -> Unit = {},
    val add: () -> Unit = {}, val settings: () -> Unit = {}, val retry: () -> Unit = {},
    val draft: (String) -> Unit = {}, val send: () -> Unit = {}, val stop: () -> Unit = {},
    val older: () -> Unit = {}, val withdraw: (String) -> Unit = {},
    val resend: (String) -> Unit = {}, val deleteFailed: (String) -> Unit = {},
    val comment: (ChatMessage, String) -> Unit = { _, _ -> },
    val editComment: (DraftCommentUi) -> Unit = {}, val removeComment: (String) -> Unit = {},
    val deviceSettings: () -> Unit = settings,
    val attach: () -> Unit = {}, val removeAttachment: (String) -> Unit = {}, val file: (TextAttachmentUi) -> Unit = {},
    val entered: () -> Unit = {},
    val message: (ChatMessage) -> Unit = {},
    val newer: () -> Unit = {},
    val windowAnchor: (String?) -> Unit = {},
    val interaction: (String, String, Map<String, String>) -> Unit = { _, _, _ -> },
    val history: (String, String) -> Unit = { _, _ -> },
    val sharedFiles: () -> Unit = {},
    val chatFile: (String, String) -> Unit = { _, _ -> },
    val newChat: (Peer) -> Unit = {},
    val archiveChat: (String, String, Boolean, Long) -> Unit = { _, _, _, _ -> },
)

private class ConversationPresentation {
    var state: WorkbenchState? = null
    var renderedKey: String? = null
    var renderedMessages = false
}

@Composable
internal fun Workbench(state: WorkbenchState, actions: WorkbenchActions, modifier: Modifier = Modifier) {
    val savedState = rememberSaveableStateHolder()
    val inactiveActions = remember { WorkbenchActions() }
    val presentation = remember { ConversationPresentation() }
    val transition = updateTransition(state.conversation != null, label = "conversation-navigation")
    val pageLayer = rememberGraphicsLayer()
    val focus = LocalFocusManager.current
    LaunchedEffect(transition.targetState) { if (!transition.targetState) focus.clearFocus() }
    if (state.conversation != null) presentation.state = state
    else if (presentation.state?.activePeer?.id !in state.peers.map { it.id }) presentation.state = null
    // Prepare the likely next chat after its navigation entry is available. Keeping
    // this shell mounted avoids constructing the input controls on the click path.
    val preview = state.sessions.firstOrNull()?.let { chat ->
        state.copy(conversation = Conversation(chat.text("chat_id"), chat.text("title")),
            messages = emptyList(), pending = emptyList(), draft = "", comments = emptyList(), attachments = emptyList(), historyLoading = true)
    }
    val chat = presentation.state ?: preview
    val progress = transition.animateFloat(
        transitionSpec = { PageSlideMotion.spec(targetState) },
        label = "conversation-slide",
    ) { if (it) 1f else 0f }
    val entering = !transition.currentState && transition.targetState
    val exiting = transition.currentState && !transition.targetState
    val visible = transition.currentState || transition.targetState
    Box(modifier.fillMaxSize().background(ZorkColors.Canvas).clipToBounds()) {
        if (!transition.currentState || !transition.targetState) {
            Box(Modifier.fillMaxSize().pageSlideBack { progress.value }) {
                savedState.SaveableStateProvider("navigation") { Navigation(state, actions, Modifier.fillMaxWidth()) }
            }
        }
        chat?.let { cached ->
            val key = "${cached.activePeer?.id}:${cached.conversation!!.id}"
            val loadingAtEntry = remember(key, cached.conversationEntry) { cached.historyLoading }
            val replayEntry = remember(key, cached.conversationEntry) {
                cached.messages.isNotEmpty() && presentation.renderedKey == key && presentation.renderedMessages
            }
            val replay = exiting || (entering && replayEntry)
            val shown = if (entering && loadingAtEntry) cached.copy(messages = emptyList(), pending = emptyList(), historyLoading = true) else cached
            val semantics = if (visible && !replay) Modifier.semantics { contentDescription = "聊天页面" } else Modifier.clearAndSetSemantics { }
            Box(Modifier.fillMaxSize().pageSlideFront { if (replay) 1f else progress.value }.graphicsLayer {
                alpha = if (visible && !replay) 1f else 0f
            }.then(semantics)) {
                savedState.SaveableStateProvider("conversation:${cached.activePeer?.id}:${cached.conversation!!.id}") {
                    Column(Modifier.fillMaxSize().drawWithContent {
                        pageLayer.record { this@drawWithContent.drawContent() }
                        presentation.renderedKey = key
                        presentation.renderedMessages = shown.messages.isNotEmpty()
                        drawLayer(pageLayer)
                    }.background(ZorkColors.Canvas)) {
                        ConversationHeader(shown, if (visible) actions else inactiveActions, showBack = true)
                        if (shown.notice != null && visible) Notice(shown.notice, shown.busy, actions.retry)
                        ConversationBody(shown, if (visible) actions else inactiveActions)
                    }
                }
            }
            // Native text views stay in place while a recorded layer slides out.
            // This avoids relaying out long Markdown during the return animation.
            if (replay) Canvas(Modifier.fillMaxSize().pageSlideFront { progress.value }
                .semantics { contentDescription = "聊天页面" }) { drawLayer(pageLayer) }
        }
    }
}

@Composable
private fun Navigation(state: WorkbenchState, actions: WorkbenchActions, modifier: Modifier) {
    var details by remember { mutableStateOf<Pair<String,List<Pair<String,String>>>?>(null) }
    var showArchived by rememberSaveable { mutableStateOf(false) }
    Column(modifier.fillMaxHeight().background(ZorkColors.Paper)) {
        Row(Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 24.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Image(painterResource(R.drawable.ic_zork), null, Modifier.size(26.dp))
            Image(painterResource(R.drawable.zork_wordmark), "Zork", Modifier.width(80.dp).height(26.dp))
        }
        if (state.notice != null && state.conversation == null) Notice(state.notice, state.busy, actions.retry)
        Spacer(Modifier.height(0.5.dp))
        NavRow(onClick = actions.sharedFiles) {
            Glyph(R.drawable.ic_folder, 24.dp)
            Text("共享文件", fontSize = 15.sp, fontWeight = FontWeight.Medium)
        }
        NavRow(onClick = { showArchived = !showArchived }) {
            Text(if (showArchived) "返回 Chat" else "已归档", fontSize = 15.sp)
        }
        val collapsed = remember { mutableStateMapOf<String, Boolean>() }
        val expanded = remember { mutableStateMapOf<String, Boolean>() }
        LazyColumn(state = rememberLazyListState(), modifier = Modifier.weight(1f), contentPadding = PaddingValues(bottom = 10.dp)) {
            if (state.peers.isEmpty()) item {
                Text(if (state.ready) "连接已有设备，开始新的 Chat。" else "正在准备连接…",
                    fontSize = 13.sp, lineHeight = 21.sp, color = ZorkColors.Muted,
                    modifier = Modifier.padding(horizontal = 11.dp, vertical = 20.dp))
            }
            state.peers.forEach { peer ->
                val active = state.activePeer?.id == peer.id
                val tree = state.deviceTrees[peer.id] ?: if (active) DeviceTree(state.leaders, state.sessions, state.tasksByLeader, state.connected) else null
                item(key = "device:${peer.id}") {
                    if (peer != state.peers.firstOrNull()) Spacer(Modifier.height(0.5.dp))
                    Spacer(Modifier.height(16.dp))
                    NavRow(onClick = {
                        collapsed[peer.id] = !(collapsed[peer.id] ?: false)
                        if (!active) { collapsed[peer.id] = false; actions.peer(peer) }
                    }) {
                        Glyph(R.drawable.ic_node, 26.dp)
                        DeviceName(peer.name, peer.status, Modifier.weight(1f))
                    }
                }
                if (tree != null && collapsed[peer.id] != true) {
                    item(key = "new-chat:${peer.id}") {
                        NavRow(indent = 54.dp, onClick = { actions.newChat(peer) }) {
                            Glyph(R.drawable.ic_plus, 20.dp)
                            Text("新建 Chat", fontSize = 15.sp)
                        }
                    }
                    val matching = tree.sessions.filter { it.optBoolean("archived") == showArchived }
                    if (matching.isEmpty()) item {
                        Text(if (showArchived) "没有已归档的 Chat" else if (tree.online) "还没有 Chat" else "等待设备连接…", fontSize = 12.sp,
                            color = ZorkColors.Muted, modifier = Modifier.padding(horizontal = 54.dp, vertical = 10.dp))
                    }
                    val othersKey = "${peer.id}/unattributed"
                    val others = matching.filter { showArchived || expanded[othersKey] == true || it.optBoolean("in_preview", true) || it.text("chat_id") == state.conversation?.id }
                    items(others, key = { "unassigned:${peer.id}:${it.text("chat_id")}" }) { session ->
                        NavRow(indent = 54.dp,
                            onClick = { actions.session(JSONObject(session.toString()).put("_peer", peer.id)) }) {
                            Text(session.text("title", "对话"), fontSize = 15.sp,
                                modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            ZorkIconButton(if (session.optBoolean("archived")) "取消归档" else "归档聊天",
                                enabled = !session.optBoolean("archive_pending"),
                                onClick = { actions.archiveChat(peer.id, session.text("chat_id"), !session.optBoolean("archived"), session.optLong("message_count")) }) {
                                Glyph(if (session.optBoolean("archived")) R.drawable.ic_archive_restore else R.drawable.ic_archive,
                                    16.dp, ZorkColors.Ink.copy(alpha = 0.5f))
                            }
                            if (session.optBoolean("unread")) Box(Modifier.size(6.dp).background(ZorkColors.Ink, CircleShape))
                        }
                        if (!session.isNull("archive_error")) {
                            Text(session.text("archive_error"), color = MaterialTheme.colorScheme.error, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 54.dp))
                        }
                    }
                    if (!showArchived && (others.size < matching.size || expanded[othersKey] == true)) item(key = "more:$othersKey") {
                        NavRow(indent = 54.dp, onClick = { expanded[othersKey] = expanded[othersKey] != true }) {
                            Text(if (expanded[othersKey] == true) "收起" else "显示更多", fontSize = 13.sp, color = ZorkColors.Muted)
                        }
                    }
                }
                item { Spacer(Modifier.height(8.dp)) }
            }
        }
        Spacer(Modifier.height(0.5.dp))
        Row(Modifier.padding(horizontal = 10.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            FooterAction("连接设备", R.drawable.ic_plus, Modifier.weight(1f), state.ready, actions.add)
            FooterAction("设置", R.drawable.ic_settings, Modifier.weight(1f), true, actions.settings)
        }
    }
    ZorkRetained(details) { (title,rows), open, closed -> SettingsSheet(title,dismiss={details=null}, open=open, onClosed=closed) {
        rows.filter{it.second.isNotBlank()}.forEach{(label,value)->Column(verticalArrangement=Arrangement.spacedBy(6.dp)){Text(label,fontSize=12.sp,color=ZorkColors.Muted);Text(value,fontSize=14.sp,lineHeight=22.sp)}}
    }}
}

@Composable
private fun FooterAction(label: String, icon: Int, modifier: Modifier, enabled: Boolean, action: () -> Unit) {
    Row(modifier.height(48.dp).zorkPressable(enabled = enabled, onClick = action),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally)) {
        Glyph(icon, 22.dp); Text(label, fontSize = 14.sp)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NavRow(indent: Dp = 14.dp, enabled: Boolean = true,
    interactions: MutableInteractionSource = remember { MutableInteractionSource() },
    onClick: () -> Unit, onLongClick: (() -> Unit)? = null, content: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp).padding(bottom = 2.dp).height(48.dp)
        .zorkPressable(enabled = enabled, interactionSource = interactions, onClick = onClick, onLongClick = onLongClick)
        .padding(start = indent, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        content()
    }
}

@Composable
internal fun ConversationHeader(state: WorkbenchState, actions: WorkbenchActions, showBack: Boolean) {
    Row(Modifier.fillMaxWidth().height(64.dp).padding(start = 8.dp, end = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        if (showBack) IconAction(R.drawable.ic_arrow_left, "返回对话列表", onClick = actions.back)
        Row(horizontalArrangement = Arrangement.spacedBy(0.dp), verticalAlignment = Alignment.CenterVertically) {
            if (state.participants.isNotEmpty()) state.participants.take(3).forEach { member ->
                Box(Modifier.size(44.dp, 44.dp)
                    .historyPress(enabled = member.text("session_id").isNotBlank(), radius = 12.dp) { actions.history(member.text("session_id"), member.text("name")) }
                    .semantics { contentDescription = "${member.text("name")} · 执行历史" }, contentAlignment = Alignment.Center) {
                    Avatar(member.text("avatar"), 30.dp, member.text("name"))
                }
            } else state.conversation?.let {
                Box(Modifier.size(44.dp, 44.dp).historyPress(radius = 12.dp) { actions.history(it.id, it.title) }
                    .semantics { contentDescription = "${it.title} · 执行历史" }, contentAlignment = Alignment.Center) {
                    Avatar(it.avatar, 30.dp, it.title)
                }
            }
        }
        Spacer(Modifier.weight(1f))
        val deviceInteraction = remember { MutableInteractionSource() }
        val devicePressed by deviceInteraction.collectIsPressedAsState()
        Row(Modifier.clip(RoundedCornerShape(6.dp)).background(if (devicePressed) ZorkColors.Pressed else Color.Transparent)
            .clickable(interactionSource = deviceInteraction, indication = null, onClick = actions.deviceSettings).heightIn(min = 44.dp).padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Glyph(R.drawable.ic_node, 17.dp, tint = ZorkColors.Ink)
            Text(state.activePeer?.name.orEmpty(), fontSize = 13.sp, color = ZorkColors.Ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
    HorizontalDivider(color = ZorkColors.Border, thickness = 0.5.dp)
}

private class MessageTailAnchor(var activityCursor: Long) {
    var appliedRows: List<ChatMessage>? = null
    val arrivals = LinkedHashMap<String, Long>()
}
private class ReferenceKey(private val value: Any) {
    override fun equals(other: Any?) = other is ReferenceKey && value === other.value
    override fun hashCode() = System.identityHashCode(value)
}

@Composable
private fun ConversationRefreshRate() {
    val view = androidx.compose.ui.platform.LocalView.current
    DisposableEffect(view) {
        var context = view.context
        while (context is android.content.ContextWrapper && context !is android.app.Activity) context = context.baseContext
        val window = (context as? android.app.Activity)?.window
        val oldMode = window?.attributes?.preferredDisplayModeId ?: 0
        val oldRate = window?.attributes?.preferredRefreshRate ?: 0f
        var disposed = false
        var applied = false
        fun applyPreference() {
            if (disposed || !view.isAttachedToWindow || window == null) return
            val display = view.display ?: return
            val current = display.mode
            val preferred = display.supportedModes.filter {
                it.physicalWidth == current.physicalWidth && it.physicalHeight == current.physicalHeight && it.refreshRate <= 120.5f
            }.maxByOrNull { it.refreshRate } ?: return
            window.attributes = window.attributes.apply { preferredDisplayModeId = preferred.modeId; preferredRefreshRate = preferred.refreshRate }
            applied = true
        }
        val listener = object : android.view.View.OnAttachStateChangeListener {
            override fun onViewAttachedToWindow(v: android.view.View) { v.post { applyPreference() } }
            override fun onViewDetachedFromWindow(v: android.view.View) { }
        }
        view.addOnAttachStateChangeListener(listener)
        view.post { applyPreference() }
        onDispose {
            disposed = true; view.removeOnAttachStateChangeListener(listener)
            if (applied && window != null) window.attributes = window.attributes.apply { preferredDisplayModeId = oldMode; preferredRefreshRate = oldRate }
        }
    }
}

@Composable
internal fun ConversationBody(state: WorkbenchState, actions: WorkbenchActions, listState: LazyListState = rememberLazyListState()) {
    val messagePreviewHeight = LocalMessagePreviewHeight.current
    ConversationRefreshRate()
    LaunchedEffect(state.conversation?.id, state.conversationEntry) {
        withFrameNanos { }
        actions.entered()
    }
    val rows = if (state.pending.isEmpty()) state.messages else state.messages + state.pending
    val latestActions = rememberUpdatedState(actions)
    val latestFirstId = rememberUpdatedState(rows.firstOrNull()?.id)
    val latestNewer = rememberUpdatedState(state.newer)
    // LazyListState already saves the reading position for this conversation.
    // Only user scrolling changes follow mode; a new layout must not clear it.
    var initialized by rememberSaveable { mutableStateOf(false) }
    var following by rememberSaveable { mutableStateOf(true) }
    val anchor = remember(state.conversation?.id, state.conversationEntry) { MessageTailAnchor(state.messageActivity.sequence) }
    val scope = rememberCoroutineScope()
    var scrollJob by remember { mutableStateOf<Job?>(null) }
    var unread by remember { mutableStateOf(0) }
    var touching by remember { mutableStateOf(false) }
    var userScrollSession by remember { mutableStateOf(false) }
    fun followTail() {
        actions.windowAnchor(null)
        unread = 0
        if (scrollJob?.isActive == true) return
        scrollJob = scope.launch {
            if (!android.animation.ValueAnimator.areAnimatorsEnabled()) {
                listState.scrollToItem(rows.size + 1)
            } else {
                // A burst updates the target read on every frame. There is never
                // a queue of separate animations, and user input cancels this job.
                val started = android.os.SystemClock.uptimeMillis()
                var previousProgress = 0f
                while (true) {
                    withFrameNanos { }
                    val progress = ((android.os.SystemClock.uptimeMillis() - started) / 200f).coerceAtMost(1f)
                    val eased = 1f - (1f - progress) * (1f - progress) * (1f - progress)
                    val info = listState.layoutInfo
                    val last = info.visibleItemsInfo.lastOrNull()
                    if (last != null) {
                        val remaining = if (last.index == info.totalItemsCount - 1) {
                            (last.offset + last.size + info.afterContentPadding - info.viewportEndOffset).coerceAtLeast(0).toFloat()
                        } else info.viewportSize.height.toFloat()
                        val fraction = ((eased - previousProgress) / (1f - previousProgress).coerceAtLeast(.001f)).coerceIn(0f, 1f)
                        listState.scroll { scrollBy(remaining * fraction) }
                    }
                    previousProgress = eased
                    if (progress >= 1f) break
                }
                listState.scrollToItem(listState.layoutInfo.totalItemsCount - 1)
            }
            following = true
            scrollJob = null
        }
    }
    LaunchedEffect(listState) {
        var wasUserScrolling = false
        snapshotFlow { Triple(listState.isScrollInProgress, listState.canScrollForward, scrollJob?.isActive == true) }
            .collect { (scrolling, canScrollForward, programmatic) ->
                if (userScrollSession && !programmatic && (scrolling || wasUserScrolling)) {
                    val follow = !canScrollForward && !latestNewer.value
                    if (following != follow) {
                        following = follow
                        latestActions.value.windowAnchor(if (follow) null else latestFirstId.value)
                    }
                    if (following) unread = 0
                }
                if (wasUserScrolling && !scrolling && !touching) userScrollSession = false
                wasUserScrolling = userScrollSession && scrolling && !programmatic
            }
    }
    val messageActions = remember {
        WorkbenchActions(resend = { latestActions.value.resend(it) }, deleteFailed = { latestActions.value.deleteFailed(it) },
            chatFile = { message, file -> latestActions.value.chatFile(message, file) },
            file = { latestActions.value.file(it) }, message = { latestActions.value.message(it) }, older = { latestActions.value.older() },
            comment = { row, quote -> latestActions.value.comment(row, quote) }, newer = { latestActions.value.newer() },
            interaction = { id, choice, values -> latestActions.value.interaction(id, choice, values) })
    }
    val messageState = remember(state.conversation?.id, ReferenceKey(state.messages), state.connected, state.historyLoading, state.older, state.busy, state.newer) {
        ConversationMessageState(state.conversation?.id, state.messages.firstOrNull()?.createdAt.orEmpty(),
            state.messages.isEmpty(), state.connected, state.historyLoading, state.older, state.busy, state.newer)
    }
    val deviceNames = remember(state.peers, state.activePeer) {
        (state.peers + listOfNotNull(state.activePeer)).associate { it.id to it.name }
    }
    // Measure the floating controls before the list in this same layout pass.
    // No onSizeChanged round trip can leave a frame with obsolete bottom space.
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val presence = rememberComposerPresence(state, maxWidth.value - 24f)
        val reservedExtent = presence.targetExtent
        // Keep the reading anchor if a drag interrupts an active transition.
        val beganAtTail = remember(reservedExtent) { following }
        val availableHeight = maxHeight
        val gutter = 18.dp
        ConversationViewport(
            listState = listState,
            presence = presence,
            trackTail = beganAtTail || following,
            follow = following && !touching && scrollJob?.isActive != true,
            tailIndex = rows.size + 1,
            previewHeight = messagePreviewHeight,
            overlay = {
                Column(Modifier.fillMaxWidth()) {
                    if (unread > 0) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center) {
                        Text("有新消息", fontSize = 12.sp, color = ZorkColors.Ink,
                            modifier = Modifier.background(ZorkColors.Canvas, RoundedCornerShape(20.dp))
                                .clickable { following = true; followTail() }.padding(horizontal = 14.dp, vertical = 8.dp))
                    }
                    if (state.activity.startsWith("已请求停止")) Text(state.activity, color = ZorkColors.Muted, fontSize = 10.sp,
                        modifier = Modifier.padding(start = gutter + 10.dp, end = gutter, bottom = 5.dp))
                    val commentLimit = (availableHeight * .25f).coerceAtMost(160.dp)
                    if (state.comments.isNotEmpty()) CommentTray(state.comments, actions, commentLimit)
                    val composerLimit = (availableHeight - (if (state.comments.isEmpty()) 0.dp else commentLimit + 8.dp) - 40.dp).coerceAtLeast(100.dp)
                    Composer(state, actions, presence, Modifier.padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 12.dp), composerLimit) { actions.send() }
                }
            },
        ) { overlayBaseHeight ->
            val bottomPadding = remember(overlayBaseHeight, reservedExtent) { ComposerMessagePadding(gutter, overlayBaseHeight, reservedExtent) }
            val now = android.os.SystemClock.uptimeMillis()
            val freshCount = (state.messageActivity.sequence - anchor.activityCursor).coerceAtLeast(0)
            state.messageActivity.recent.filter { it.sequence > anchor.activityCursor }.forEach {
                anchor.arrivals[it.id] = it.startedAt
            }
            anchor.arrivals.entries.removeAll { now - it.value > 250 }
            while (anchor.arrivals.size > 32) anchor.arrivals.remove(anchor.arrivals.keys.first())
            SideEffect {
                val rowsChanged = anchor.appliedRows !== rows
                anchor.appliedRows = rows
                anchor.activityCursor = state.messageActivity.sequence
                if (rows.isNotEmpty() && !initialized) {
                    listState.requestScrollToItem(rows.size + 1)
                    initialized = true
                } else if (freshCount > 0) {
                    if (following && !touching) followTail()
                    else unread = (unread.toLong() + freshCount).coerceAtMost(Int.MAX_VALUE.toLong()).toInt()
                } else if (rowsChanged && following && rows.isNotEmpty() && scrollJob?.isActive != true) {
                    listState.requestScrollToItem(rows.size + 1)
                }
            }
            // IME/composer resize changes the viewport, not message geometry.
            // A moving limit retruncates native TextViews on every inset frame.
            val messageLimit = (messagePreviewHeight.takeIf { it in 80..720 } ?: 192).dp
            LazyColumn(state = listState, modifier = Modifier.fillMaxSize().pointerInput(listState) {
                    awaitPointerEventScope {
                        while (true) {
                            val event = awaitPointerEvent(PointerEventPass.Initial)
                            touching = event.changes.any { it.pressed }
                            if (touching) {
                                userScrollSession = true
                                scrollJob?.cancel(); scrollJob = null; following = false
                            } else {
                                following = !listState.canScrollForward
                                if (!listState.isScrollInProgress) userScrollSession = false
                            }
                        }
                    }
                },
                contentPadding = bottomPadding, verticalArrangement = Arrangement.Top) {
                item {
                    val firstDate = messageState.firstDate
                    if (firstDate.isNotBlank()) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(13.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
                        HorizontalDivider(Modifier.width(34.dp), color = ZorkColors.Border, thickness = 0.5.dp)
                        Text(messageDate(firstDate), color = ZorkColors.Muted, fontSize = 12.sp)
                        HorizontalDivider(Modifier.width(34.dp), color = ZorkColors.Border, thickness = 0.5.dp)
                    }
                    if (messageState.older) ZorkButton("加载更早消息", quiet = true, onClick = { following = false; messageActions.older() }, enabled = !messageState.busy)
                }
                items(rows, key = { it.id }) { row ->
                    Column {
                        Spacer(Modifier.height(22.dp))
                        MessageEntry(anchor.arrivals[row.id], row.user) {
                            Column {
                            MessageRow(row, deviceNames[row.device] ?: row.device.takeUnless { it.startsWith("key:") }.orEmpty(), messageActions.resend, messageActions.deleteFailed, messageActions.file, messageActions.chatFile, messageLimit, { messageActions.message(row) }) { quote -> messageActions.comment(row, quote) }
                            row.interaction?.let { card ->
                                Spacer(Modifier.height(8.dp))
                                InteractionCard(card) { choice, values -> messageActions.interaction(row.id, choice, values) }
                            }
                            }
                        }
                    }
                }
                // Padding is a list measure input, so animated clearance is applied
                // immediately rather than waiting for a lazy spacer to recompose.
                item(key = "conversation-bottom") {
                    if (messageState.newer) ZorkButton("加载更新消息", quiet = true, onClick = messageActions.newer, enabled = !messageState.busy)
                    else Spacer(Modifier.height(0.dp))
                }
            }

        if (messageState.historyLoading && messageState.empty) {
            Box(Modifier.fillMaxSize().padding(bottom = overlayBaseHeight), contentAlignment = Alignment.Center) {
                Text("正在加载消息…", color = ZorkColors.Muted, fontSize = 12.sp)
            }
        }
        }
    }
}

private data class ConversationMessageState(val sessionId: String?, val firstDate: String, val empty: Boolean,
    val connected: Boolean, val historyLoading: Boolean, val older: Boolean, val busy: Boolean, val newer: Boolean)

private class ComposerMessagePadding(private val horizontal: Dp, private val base: Dp, private val reservedExtent: Float) : PaddingValues {
    override fun calculateLeftPadding(layoutDirection: androidx.compose.ui.unit.LayoutDirection) = horizontal
    override fun calculateRightPadding(layoutDirection: androidx.compose.ui.unit.LayoutDirection) = horizontal
    override fun calculateTopPadding() = 20.dp
    override fun calculateBottomPadding() = base + reservedExtent.dp + 20.dp
}

private class ConversationBodySlot {
    var base = -1
    var parent: Any? = null
    var content: (@Composable () -> Unit)? = null
}

@Composable
private fun ConversationViewport(listState: LazyListState, presence: ComposerPresence, trackTail: Boolean, follow: Boolean, tailIndex: Int, previewHeight: Int,
    overlay: @Composable () -> Unit, content: @Composable (Dp) -> Unit) {
    val geometry = remember { intArrayOf(-1, -1, -1, -1) }
    val slot = remember { ConversationBodySlot() }
    androidx.compose.ui.layout.SubcomposeLayout(Modifier.fillMaxSize().clipToBounds().preferredFrameRate(120f)) { constraints ->
        val controls = subcompose("controls", overlay).map { it.measure(constraints.copy(minHeight = 0)) }
        val overlayHeight = controls.maxOfOrNull { it.height } ?: 0
        val baseHeight = overlayHeight - presence.extentPixels(density)
        val reservedHeight = baseHeight + (presence.targetExtent * density).toInt()
        if (geometry[0] != constraints.maxWidth || geometry[1] != reservedHeight || geometry[2] != constraints.maxHeight || geometry[3] != previewHeight) {
            if (follow) listState.requestScrollToItem(tailIndex)
            geometry[0] = constraints.maxWidth
            geometry[1] = reservedHeight
            geometry[2] = constraints.maxHeight
            geometry[3] = previewHeight
        }
        if (slot.content == null || slot.base != baseHeight || slot.parent !== content) {
            val base = baseHeight.toDp()
            slot.content = {
                Box(Modifier.fillMaxSize().graphicsLayer()) { content(base) }
            }
            slot.base = baseHeight; slot.parent = content
        }
        // Extra rows above the viewport remain recorded during downward layer
        // translation. The list's geometry changes only at retarget/resize, not
        // on every spring sample; the visible tail still tracks that sample.
        val overscan = ((presence.members.size * 48f + 48f) * density).toInt()
        val body = subcompose("messages", slot.content!!)
            .map { it.measure(androidx.compose.ui.unit.Constraints.fixed(constraints.maxWidth, constraints.maxHeight + overscan)) }
        layout(constraints.maxWidth, constraints.maxHeight) {
            val translation = if (trackTail) ((presence.targetExtent - presence.extent) * density).roundToInt() else 0
            body.forEach { it.place(0, -overscan + translation) }
            controls.forEach { it.place(0, constraints.maxHeight - it.height) }
        }
    }
}

@Composable
private fun MessageRow(row: ChatMessage, device: String, resend: (String) -> Unit, deleteFailed: (String) -> Unit, file: (TextAttachmentUi) -> Unit, chatFile: (String, String) -> Unit, limit: Dp, open: () -> Unit, comment: (String) -> Unit) {
    if (row.user) {
        Column(Modifier.fillMaxWidth().padding(start = 30.dp), horizontalAlignment = Alignment.End) {
            if (row.content.isNotBlank() || row.files.isNotEmpty() || row.deliveredFiles.isNotEmpty()) {
                ZorkCard(color = ZorkColors.Bubble, radius = 20.dp) {
                    Column(Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) {
                        if (row.content.isNotBlank()) MessageBodyPreview(row, limit, open, comment)
                        row.files.forEach { FileCard(it) { file(it) } }
            row.deliveredFiles.forEach { DeliveredFileCard(it) { chatFile(row.id, it.id) } }
                    }
                }
            }
            val time = messageTime(row.createdAt)
            if (time.isNotEmpty()) Text(time, fontSize = 11.sp, color = ZorkColors.Muted, modifier = Modifier.padding(top = 5.dp))
            if (row.pending) Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                if (row.deliveryStatus.isNotBlank()) Column(Modifier.weight(1f, fill = false)) {
                    Text(if (row.deliveryStatus == "failed") "发送失败" else "发送中", fontSize = 10.sp, color = ZorkColors.Muted)
                    if (row.deliveryStatus == "failed" && row.deliveryError.isNotBlank())
                        Text(row.deliveryError, fontSize = 10.sp, color = ZorkColors.Muted)
                }
                if (row.deliveryStatus == "failed") {
                    ZorkButton("重发", quiet = true, onClick = { resend(row.requestId.ifBlank { row.id }) },
                        modifier = Modifier.clearAndSetSemantics {
                            contentDescription = "重发失败消息"
                            onClick { resend(row.requestId.ifBlank { row.id }); true }
                        })
                    ZorkButton("删除", quiet = true, onClick = { deleteFailed(row.requestId.ifBlank { row.id }) },
                        modifier = Modifier.clearAndSetSemantics {
                            contentDescription = "删除失败消息"
                            onClick { deleteFailed(row.requestId.ifBlank { row.id }); true }
                        })
                }
            }
        }
    } else {
        Column(Modifier.fillMaxWidth()) {
            Row(Modifier.padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(device.ifBlank { row.author }, modifier = Modifier.weight(1f, fill = false), fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                val detail = listOf(row.model, messageTime(row.createdAt)).filter { it.isNotBlank() }.joinToString(" · ")
                if (detail.isNotEmpty()) Text(detail, modifier = Modifier.weight(1f, fill = false), fontSize = 11.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (row.content.isNotBlank()) MessageBodyPreview(row, limit, open, comment)
            row.files.forEach { FileCard(it) { file(it) } }
            row.deliveredFiles.forEach { DeliveredFileCard(it) { chatFile(row.id, it.id) } }
        }
    }
}

@Composable
private fun ComposerPlate(presence: ComposerPresence, modifier: Modifier, history: (String, String) -> Unit, body: @Composable () -> Unit) {
    // The editor receives constant constraints during presence motion. In a
    // Column, the changing header consumed its max height and remeasured the
    // text editor on every frame despite its unchanged three-line viewport.
    androidx.compose.ui.layout.Layout(modifier = modifier, content = { ComposerMembers(presence, history); body() }) { measurables, constraints ->
        val controls = measurables[1].measure(constraints.copy(minHeight = 0))
        val header = measurables[0].measure(constraints.copy(minHeight = 0))
        layout(constraints.maxWidth, controls.height + header.height) {
            header.place(0, 0)
            controls.place(0, header.height)
        }
    }
}

@Composable
private fun Composer(state: WorkbenchState, actions: WorkbenchActions, presence: ComposerPresence, modifier: Modifier, heightLimit: Dp, send: () -> Unit) {
    val command = remember(state.draft, state.comments.size, state.attachments.size, state.running, state.connected, state.busy, state.conversation) {
        JSONObject(NativeBridge.composerState(JSONObject().put("text", state.draft)
            .put("attachments", state.comments.size + state.attachments.size).put("can_send", state.conversation?.canSend != false)
            .put("can_stop", state.conversation?.canStop == true).put("running", state.running)
            .put("online", state.connected).put("busy", state.busy).toString()))
    }
    val canSend = command.optBoolean("editable")
    val stop = command.optBoolean("stop")
    val enabled = command.optBoolean("enabled")
    val draft = state.draft
    val attachments = remember(state.attachments) { state.attachments }
    val latest = rememberUpdatedState(actions)
    val latestSend = rememberUpdatedState(send)
    val controls = remember {
        WorkbenchActions(draft = { latest.value.draft(it) }, attach = { latest.value.attach() },
            removeAttachment = { latest.value.removeAttachment(it) }, stop = { latest.value.stop() }, send = { latestSend.value() })
    }
    DraftComposer(draft, attachments, canSend, stop, enabled, presence, modifier, heightLimit, controls, actions.history)
}

@Composable
internal fun DraftComposer(draft: String, attachments: List<TextAttachmentUi>, canEdit: Boolean,
    stop: Boolean, enabled: Boolean, presence: ComposerPresence, modifier: Modifier, heightLimit: Dp,
    actions: WorkbenchActions, history: (String, String) -> Unit = { _, _ -> }, showAttach: Boolean = true) {
    ComposerPlate(presence, modifier.fillMaxWidth().preferredFrameRate(120f), history) {
        ComposerControls(draft, attachments, canEdit, stop, enabled, heightLimit, actions, showAttach)
    }
}

@Composable
private fun ComposerControls(draft: String, attachments: List<TextAttachmentUi>, canSend: Boolean,
    stop: Boolean, enabled: Boolean, heightLimit: Dp, actions: WorkbenchActions, showAttach: Boolean = true) {
    val sendInteractions = remember { MutableInteractionSource() }
    val sendPressed by sendInteractions.collectIsPressedAsState()
        Column(Modifier.graphicsLayer().heightIn(max = heightLimit).padding(start = 8.dp, end = 8.dp, bottom = 4.dp)) {
            Column(Modifier.weight(1f, fill = false)) {
            // BasicTextField owns vertical scrolling and cursor visibility. Do not
            // wrap the editor in another scroller or cap/truncate its draft value.
            BasicTextField(draft, actions.draft, Modifier.fillMaxWidth().padding(start=12.dp,end=12.dp,top=4.dp)
                .semantics { contentDescription = "消息输入框" }, enabled = canSend, maxLines = 3,
                textStyle = TextStyle(fontFamily = ZorkFonts.Body, fontSize = 16.sp, lineHeight = 24.sp, color = ZorkColors.Ink),
                cursorBrush = SolidColor(ZorkColors.Ink), decorationBox = { inner ->
                    Box {
                        if (draft.isEmpty()) Text(if (canSend) "补充想法…" else "此任务暂不可直接发送消息",
                            fontSize = 16.sp, color = ZorkColors.Muted, lineHeight = 24.sp)
                        inner()
                    }
                })
            if (attachments.isNotEmpty()) Column(Modifier.weight(1f, fill = false).heightIn(max=120.dp).verticalScroll(rememberScrollState()),verticalArrangement=Arrangement.spacedBy(8.dp)) {
                attachments.forEach { file -> Row(Modifier.fillMaxWidth().heightIn(min=48.dp).background(ZorkColors.Paper,RoundedCornerShape(20.dp)).padding(start=12.dp,end=4.dp),verticalAlignment=Alignment.CenterVertically) {
                    Glyph(R.drawable.ic_result,16.dp,ZorkColors.Muted)
                    Text(file.name,fontSize=12.sp,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f).padding(horizontal=8.dp))
                    IconAction(R.drawable.ic_x,"移除 ${file.name}",glyphSize=16.dp,onClick={actions.removeAttachment(file.id)})
                } }
            }
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                if (showAttach) IconAction(R.drawable.ic_paperclip, "添加文本附件",enabled=canSend,glyphSize=16.dp,onClick = actions.attach)
                Spacer(Modifier.weight(1f))
                Box(Modifier.size(44.dp).clickable(enabled=enabled,interactionSource=sendInteractions,indication=null,onClick=if(stop)actions.stop else actions.send)
                    .semantics { contentDescription=if(stop) "停止" else "发送" },contentAlignment=Alignment.Center) {
                    Box(Modifier.size(32.dp).background(if(!enabled)ZorkColors.SendDisabled else if(sendPressed)ZorkColors.SendPressed else ZorkColors.Ink,CircleShape),contentAlignment=Alignment.Center) {
                        if(stop) Box(Modifier.size(10.dp).background(ZorkColors.Canvas,RoundedCornerShape(2.dp)))
                        else Glyph(R.drawable.ic_arrow_up,16.dp,tint=ZorkColors.Canvas)
                    }
                }
            }
        }
}

@Composable
internal fun Glyph(resource: Int, size: Dp, tint: Color = ZorkColors.Ink) {
    Icon(painterResource(resource), contentDescription = null, Modifier.size(size), tint = tint)
}
@Composable
internal fun IconAction(resource: Int, description: String, enabled: Boolean = true, glyphSize: Dp = 22.dp, onClick: () -> Unit) {
    ZorkIconButton(description, enabled = enabled, onClick = onClick) {
        Glyph(resource, glyphSize, if (enabled) ZorkColors.Ink else ZorkColors.Ink.copy(alpha = 0.35f))
    }
}

@Composable
internal fun Avatar(name: String?, size: Dp, description: String? = null) {
    val resource = when (name) {
        "fox" -> R.drawable.avatar_fox; "cat" -> R.drawable.avatar_cat; "panda" -> R.drawable.avatar_panda
        "bear" -> R.drawable.avatar_bear; "bunny" -> R.drawable.avatar_bunny; "chick" -> R.drawable.avatar_chick
        "deer" -> R.drawable.avatar_deer; "dog" -> R.drawable.avatar_dog; "koala" -> R.drawable.avatar_koala
        "octopus" -> R.drawable.avatar_octopus; "owl" -> R.drawable.avatar_owl; "penguin" -> R.drawable.avatar_penguin
        else -> R.drawable.avatar_cat
    }
    Image(painterResource(resource), description, Modifier.size(size))
}
@Composable
private fun Notice(message: String, busy: Boolean, retry: () -> Unit) {
    Row(Modifier.fillMaxWidth().background(ZorkColors.Paper).padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(message, fontSize = 11.sp, color = ZorkColors.Muted, maxLines = 3, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        ZorkButton("重试", quiet = true, onClick = retry, enabled = !busy)
    }
}
private fun messageTime(value: String): String = runCatching {
    OffsetDateTime.parse(value).format(DateTimeFormatter.ofPattern("HH:mm"))
}.getOrDefault("")

private fun messageDate(value: String): String = runCatching {
    val date = OffsetDateTime.parse(value).toLocalDate()
    if (date == java.time.LocalDate.now()) "今天" else date.format(DateTimeFormatter.ofPattern("M月d日"))
}.getOrDefault("")

@Composable
private fun CommentTray(comments: List<DraftCommentUi>, actions: WorkbenchActions, heightLimit: Dp) {
    ZorkCard(Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(top = 8.dp).heightIn(max = heightLimit), radius = 20.dp) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(12.dp)) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                Text("待发送评论 · ${comments.size}", fontSize = 11.sp, lineHeight = 16.sp, color = ZorkColors.Muted)
            }
            comments.forEach { comment ->
                HorizontalDivider(Modifier.padding(top = 5.dp), color = ZorkColors.Border, thickness = 0.5.dp)
                Row(Modifier.fillMaxWidth().padding(vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f).zorkPressable(opensPanel = true) { actions.editComment(comment) }) {
                        Row(Modifier.padding(bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Box(Modifier.width(3.dp).height(18.dp).background(ZorkColors.FieldBorder))
                            Text(comment.quote, fontSize = 12.sp, lineHeight = 18.sp, color = ZorkColors.Muted, maxLines = 1,
                                overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 8.dp))
                        }
                        Text(comment.text, fontSize = 13.sp, lineHeight = 19.5.sp)
                    }
                    Spacer(Modifier.width(4.dp))
                    IconAction(R.drawable.ic_x, "移除评论", glyphSize = 16.dp, onClick = { actions.removeComment(comment.id) })
                }
            }
        }
    }
}

@Composable
private fun FileCard(file: TextAttachmentUi, save: () -> Unit) {
    ZorkCard(Modifier.fillMaxWidth().padding(top = 12.dp), color = ZorkColors.Bubble, radius = 10.dp) {
        Row(Modifier.heightIn(min = 64.dp).zorkPressable(onClick = save).padding(horizontal = 11.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Glyph(R.drawable.ic_result, 22.dp)
            Column(Modifier.weight(1f)) {
                Text(file.name, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                Text(file.caption, fontSize = 11.sp, color = ZorkColors.Muted, modifier = Modifier.padding(top = 4.dp))
            }
            Glyph(R.drawable.ic_download, 19.dp)
        }
    }
}
