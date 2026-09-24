package ing.zork.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.viewModels
import androidx.compose.foundation.border
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.core.content.res.ResourcesCompat

class MainActivity : ComponentActivity() {
    private val model: ClientViewModel by viewModels()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        model.localScripts.attach(this)
        intent.getStringExtra("notification_tag")?.let { model.openNotification(it); intent.removeExtra("notification_tag") }
        configureZorkSystemBars()
        setContent { ZorkTheme(model.theme) {
            val lightPage = model.conversation != null || model.settings != null || model.newChat != null
            ZorkPageBackground(lightPage) { ClientScreen(model) }
            LocalScriptPanel(model.localScripts)
        } }
    }
    override fun onDestroy() { model.localScripts.detach(this); super.onDestroy() }
    override fun onStart() { super.onStart(); model.foreground(true) }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        intent.getStringExtra("notification_tag")?.let { model.openNotification(it); intent.removeExtra("notification_tag") }
    }
    override fun onStop() {
        if (!isChangingConfigurations) model.foreground(false)
        super.onStop()
    }
}

@Composable
internal fun ClientScreen(model: ClientViewModel) {
    val context = LocalContext.current
    var openedLogin by rememberSaveable { mutableStateOf<String?>(null) }
    val loginUrl = model.account?.text("login_url")?.takeIf { it.isNotBlank() }
    LaunchedEffect(loginUrl) {
        if (loginUrl != null && openedLogin != loginUrl) {
            openedLogin = loginUrl
            try { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(loginUrl))) }
            catch (_: android.content.ActivityNotFoundException) { model.accountBrowserFailed() }
        }
    }
    LaunchedEffect(model.ready, model.activePeer?.id, model.conversation?.id, model.settings != null) { model.reportVisibleConversation() }
    var addDevice by rememberSaveable { mutableStateOf(false) }

    var editingComment by remember { mutableStateOf<DraftCommentUi?>(null) }
    var attachmentPeer by rememberSaveable { mutableStateOf<String?>(null) }
    var attachmentSession by rememberSaveable { mutableStateOf<String?>(null) }
    var exporting by remember { mutableStateOf<TextAttachmentUi?>(null) }
    val pickFile = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri ->
        val peer = attachmentPeer; val session = attachmentSession
        if (uri != null && peer != null && session != null) model.addTextAttachment(uri, peer, session)
    }
    val saveFile = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        val file = exporting; if (uri != null && file != null) model.exportTextAttachment(uri, file)
        exporting = null
    }
    var chatSaveTicket by rememberSaveable { mutableStateOf<String?>(null) }
    val chatSaveFile = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        chatSaveTicket?.let { model.saveChatFile(uri, it) }
        chatSaveTicket = null
    }
    LaunchedEffect(model.chatFile?.saveTicket) {
        model.chatFile?.let { file ->
            file.saveTicket?.let { ticket ->
                if (chatSaveTicket != ticket) { chatSaveTicket = ticket; chatSaveFile.launch(file.name) }
            }
        }
    }
    model.chatFile?.let { file ->
        ChatFilePreview(file, model.chatFileImage,
            close = { model.chatFileAction("close", "key" to file.key) },
            save = { model.chatFileAction("prepare_save", "key" to file.key) })
    }
    BackHandler(enabled = model.sessionHistory != null || model.settings != null || model.conversation != null || model.newChat != null) {
        if (model.sessionHistory != null) {
            if (model.sessionHistory?.selectedId != null) model.historyDetail(null) else model.closeHistory()
        } else if (model.settings != null) model.backSettings() else model.back()
    }
    val retained = androidx.compose.runtime.saveable.rememberSaveableStateHolder()
    val currentSettings = model.settings
    val workbenchState = WorkbenchState(model.messageRevision, model.peers, model.activePeer, model.conversation, model.leaders, model.sessions,
            model.tasksByLeader, model.messages, model.pending, model.draft, model.olderCursor != null,
            model.busy || model.loadingOlder, model.ready, model.connected, model.notice, model.activity, model.running,
            model.comments, model.participants, model.deviceTrees, model.attachments, model.historyLoading, model.conversationEntry, model.messageActivity, newer = model.hasNewer, home = model.home)
    val history = model.sessionHistory
    val draftChat = model.newChat
    val routeKey = history?.let { "history:${it.peer}:${it.session}" } ?: if (currentSettings == null && draftChat != null) "new-chat:${draftChat.peer.id}" else settingsRouteKey(currentSettings)
    PageSlide(ClientPage(currentSettings, workbenchState, history, draftChat), routeKey, if (history != null) 2 else if (draftChat != null && currentSettings == null) 1 else settingsRouteDepth(currentSettings),
        if (currentSettings != null || model.conversation != null || draftChat != null) ZorkColors.Canvas else ZorkColors.Paper,
        Modifier.safeDrawingPadding().imePadding()) { shown, active ->
    if (shown.history != null) {
        SessionHistoryPage(shown.history, if (!active) HistoryActions() else HistoryActions(
            model::closeHistory, model::olderHistory, model::newerHistory, model::latestHistory, model::retryHistory, model::historyDetail, model::historyAnchor, model::historyNavigate))
    } else if (shown.settings != null) {
        retained.SaveableStateProvider(settingsRouteKey(shown.settings)) {
            MobileSettings(shown.settings, shown.workbench.peers, if (!active) SettingsActions() else SettingsActions(model::backSettings, { model.showDevice(it) },
                model::settingsPage, model::settingsProfile, model::assistSettings, model::openConnection, model::addConnection, model::checkUpdate,
                { addDevice = true }, refresh = model::refreshSettings, perform = model::settingsAction,
                theme = model.theme, saveTheme = model::saveTheme,
                resource = model::inspectResource,
                notifications = model.notificationSettings, notificationError = model.notificationError,
                notificationTarget = model.activePeer?.id?.let { peer -> model.conversation?.id?.let { peer to it } },
                notificationAction = model::notificationAction, testNotification = model::testNotification,
                notificationRefresh = model::refreshNotificationDelivery,
                adb = model.adbSettings, adbError = model.adbError, adbAction = model::adbAction, adbRefresh = model::refreshAdb,
                account = model.account, accountError = model.accountError, accountAction = model::accountAction,
                dataReset = model.dataReset, dataResetError = model.dataResetError, clearData = model::clearData))
        }
    } else if (shown.newChat != null) {
        NewChatPage(shown.newChat, if (active) model::back else ({}),
            if (active) model::newChatAction else ({ _, _ -> }), if (active) model::newChatModels else ({}),
            model.peers, if (active) model::openNewChat else ({}))
    } else retained.SaveableStateProvider("workbench") { Workbench(
        shown.workbench,
        if (!active) WorkbenchActions() else WorkbenchActions(model::selectPeer, model::openLeader, model::openSession, model::back,
            { addDevice = true }, model::showSettings, model::retry, model::editDraft,
            model::send, model::stop, model::older, model::withdraw,
            resend = model::resend, deleteFailed = model::deleteFailed,
            comment = { row, quote -> model.conversation?.let { conversation ->
                editingComment = DraftCommentUi(NativeBridge.newId(), conversation.id, row.id,
                    row.author, row.authorAgentId.ifBlank { null }, quote, "")
            } }, editComment = { editingComment = it }, removeComment = model::removeComment,
            deviceSettings = { model.activePeer?.let { model.showDevice(it, fromChat = true) } },
            attach = { attachmentPeer = model.activePeer?.id; attachmentSession = model.conversation?.id; pickFile.launch(arrayOf("text/*", "application/json")) },
            removeAttachment = model::removeAttachment, file = { exporting = it; saveFile.launch(it.name) }, entered = model::conversationShown,
            newer = model::newer, windowAnchor = model::windowAnchor, interaction = model::respondToInteraction, history = model::openHistory, chatFile = model::openChatFile, newChat = model::openNewChat, archiveChat = model::archiveChat, device = { model.showDevice(it) }),
    ) }
    }
    ZorkRetained(editingComment) { comment, open, closed ->
        CommentDialog(comment, open, closed, { editingComment = null }) { text -> model.saveComment(comment.copy(text = text)); editingComment = null }
    }
    ZorkRetained(Unit.takeIf { addDevice }) { _, open, closed ->
        SettingsSheet("连接设备", dismiss = { addDevice = false }, open = open, onClosed = closed) {
            PhoneConnectActions(model)
        }
    }
}

private data class ClientPage(val settings: MobileSettingsState?, val workbench: WorkbenchState,
    val history: SessionHistoryState? = null, val newChat: NewChatUi? = null)

@Composable
internal fun PlainMessage(content: String, modifier: Modifier = Modifier, preview: MessagePreviewMeasure? = null, onComment: ((String) -> Unit)? = null) {
    val textPixels = with(androidx.compose.ui.platform.LocalDensity.current) { 15.sp.toPx() }
    AndroidView(modifier = modifier, factory = { ctx ->
        object : MessageTextView(ctx) {
            override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
                super.onMeasure(widthMeasureSpec, heightMeasureSpec)
                val target = kotlin.math.ceil((layout?.lineCount ?: 1) * textSize * 1.7f).toInt()
                if (preview == null && target > measuredHeight) setMeasuredDimension(measuredWidth, resolveSize(target, heightMeasureSpec))
            }
        }.apply {
            gravity = android.view.Gravity.CENTER_VERTICAL
            typeface = ResourcesCompat.getFont(ctx, R.font.inter)
            setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, textPixels); includeFontPadding = false
            setTextIsSelectable(true); setLineHeight((textPixels * 1.7f).toInt())
        }
    }, update = { view ->
        view.preview = preview
        // Colors are read here, not in factory, so a theme switch recolors the view.
        view.setTextColor(ZorkColors.Ink.toArgb()); view.setLinkTextColor(ZorkColors.Ink.toArgb())
        view.highlightColor = ZorkColors.Selected.toArgb()
        if (view.textSize != textPixels) {
            view.invalidatePreview()
            view.setTextSize(android.util.TypedValue.COMPLEX_UNIT_PX, textPixels)
            view.setLineHeight((textPixels * 1.7f).toInt())
        }
        if (view.tag != content) { view.text = content; view.tag = content; view.retainMessageText() }
        view.customSelectionActionModeCallback = if (onComment == null) null else object : android.view.ActionMode.Callback {
            override fun onCreateActionMode(mode: android.view.ActionMode, menu: android.view.Menu): Boolean {
                menu.add(0, 701, 0, "评论").setShowAsAction(android.view.MenuItem.SHOW_AS_ACTION_IF_ROOM)
                return true
            }
            override fun onPrepareActionMode(mode: android.view.ActionMode, menu: android.view.Menu) = false
            override fun onActionItemClicked(mode: android.view.ActionMode, item: android.view.MenuItem): Boolean {
                if (item.itemId != 701) return false
                val start = view.selectionStart; val end = view.selectionEnd
                if (start >= 0 && end > start) onComment(view.text.subSequence(start, end).toString())
                mode.finish(); return true
            }
            override fun onDestroyActionMode(mode: android.view.ActionMode) {}
        }
    })
}

@Composable
private fun CommentDialog(comment: DraftCommentUi, open: Boolean, closed: () -> Unit, dismiss: () -> Unit, save: (String) -> Unit) {
    var text by remember(comment.id) { mutableStateOf(comment.text) }
    SettingsSheet(if (comment.text.isBlank()) "评论所选片段" else "编辑评论", dismiss = dismiss, open = open, onClosed = closed) {
        Text(comment.quote, fontSize = 13.sp, lineHeight = 21.sp, modifier = Modifier.fillMaxWidth()
            .background(ZorkColors.Paper, ZorkShapes.Block).padding(horizontal = 16.dp, vertical = 12.dp))
        Text("你的评论", fontSize = 12.sp, color = ZorkColors.Muted)
        FormField(text, { text = it }, modifier = Modifier.fillMaxWidth(), minLines = 3,
            placeholder = { Text("对这段内容有什么想法？", fontSize = 16.sp) })
        ZorkButton("加入待发送评论", primary = true, onClick = { save(text) }, enabled = text.isNotBlank(), modifier = Modifier.fillMaxWidth())
        Text("可以继续添加其他评论，最后和消息一起发送。", color = ZorkColors.Muted, fontSize = 12.sp)
    }
}

@Composable
internal fun FormField(value: String, onValueChange: (String) -> Unit, modifier: Modifier = Modifier,
    label: (@Composable () -> Unit)? = null, placeholder: (@Composable () -> Unit)? = null,
    singleLine: Boolean = false, minLines: Int = 1, maxLines: Int = if (singleLine) 1 else 7) {
    var focused by remember { mutableStateOf(false) }
    Column(modifier) {
        if (label != null) {
            androidx.compose.runtime.CompositionLocalProvider(LocalTextStyle provides androidx.compose.ui.text.TextStyle(
                fontFamily = ZorkFonts.Body, fontSize = 12.sp, color = ZorkColors.Muted)) { label() }
            Spacer(Modifier.height(7.dp))
        }
        androidx.compose.foundation.text.BasicTextField(value, onValueChange,
            // Stable surface; the outline deepens on focus. Single-line fields are capsules.
            modifier = Modifier.fillMaxWidth().heightIn(min = if (minLines > 1) 96.dp else 44.dp)
                .background(ZorkColors.Canvas, if (singleLine || minLines == 1 && maxLines == 1) ZorkShapes.Control else ZorkShapes.Block)
                .border(if (focused) 1.5.dp else 1.dp, if (focused) ZorkColors.Ink else ZorkColors.FieldBorder,
                    if (singleLine || minLines == 1 && maxLines == 1) ZorkShapes.Control else ZorkShapes.Block)
                .onFocusChanged { focused = it.isFocused }.padding(horizontal = 16.dp, vertical = 11.dp),
            singleLine = singleLine, minLines = minLines, maxLines = maxLines,
            textStyle = androidx.compose.ui.text.TextStyle(fontFamily = ZorkFonts.Body, fontSize = 16.sp, lineHeight = 24.sp, color = ZorkColors.Ink),
            cursorBrush = androidx.compose.ui.graphics.SolidColor(ZorkColors.Ink),
            decorationBox = { inner -> Box {
                if (value.isEmpty() && placeholder != null) androidx.compose.runtime.CompositionLocalProvider(LocalTextStyle provides androidx.compose.ui.text.TextStyle(
                    fontFamily = ZorkFonts.Body, fontSize = 16.sp, lineHeight = 24.sp, color = ZorkColors.Muted)) { placeholder() }
                inner()
            } })
    }
}
