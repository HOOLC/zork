@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package ing.zork.android

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.zIndex
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONArray
import org.json.JSONObject
import java.math.BigDecimal

/*
 * Adding or editing a connection's model. Every rule — recognition, preset
 * fill, provenance, validation and when errors show — lives in core's model
 * editor (`NativeBridge.modelEditor`). This sheet keeps the opaque editor state,
 * forwards each edit as an action and renders the returned view. Only UI-local
 * things stay here: which popover is open, typed-but-unsent add inputs, drag
 * state, the recognition debounce and focus.
 */

/** Core's editor state (saved across recreation) and the last view it produced. */
private class EditorSession(state: String?) {
    var state by mutableStateOf(state)
    var view by mutableStateOf<JSONObject?>(null)
    var failure by mutableStateOf<String?>(null)
}

/**
 * The sheet is its own window, so focus and the keyboard must be the ones read
 * inside it; the page's would silently do nothing.
 */
private class SheetWindow {
    var focus: androidx.compose.ui.focus.FocusManager? = null
    var keyboard: androidx.compose.ui.platform.SoftwareKeyboardController? = null
    fun endTyping() { keyboard?.hide(); focus?.clearFocus() }
}

private val SessionSaver = Saver<EditorSession, String>({ it.state ?: "" }, { EditorSession(it.ifEmpty { null }) })

/** The connection as the editor sees it; the rest of a profile (quota, keys) stays out. */
private fun connectionInfo(profile: JSONObject): JSONObject = JSONObject()
    .put("profile_id", profile.text("profile_id")).put("name", profile.opt("name") ?: JSONObject.NULL)
    .put("provider", profile.text("provider")).put("billing", profile.opt("billing") ?: JSONObject.NULL)
    .put("models", profile.optJSONArray("models") ?: JSONArray())

internal fun modelEditorContext(profile: JSONObject, profiles: List<JSONObject>, providers: List<JSONObject>, reported: List<String>) =
    JSONObject().put("profile", connectionInfo(profile)).put("providers", JSONArray(providers))
        .put("profiles", JSONArray(profiles.map(::connectionInfo))).put("reported", JSONArray(reported))

private fun action(type: String, vararg fields: Pair<String, Any?>) =
    JSONObject().put("type", type).apply { fields.forEach { (k, v) -> put(k, v ?: JSONObject.NULL) } }

/** Background behind the controls of a section, so chips stay visible on it. */
private val LocalSectionFill = staticCompositionLocalOf { false }

/**
 * [modelId] null adds a model; otherwise it edits that model of [profile].
 * [finished] gets the toast text after a save or removal; [editOther] opens
 * another model (the duplicate id's “去编辑它”).
 */
@Composable
internal fun ModelEditor(modelId: String?, profile: JSONObject, state: MobileSettingsState, actions: SettingsActions,
    reported: List<String>, dismiss: () -> Unit, finished: (String) -> Unit, editOther: (String) -> Unit,
    open: Boolean = true, onClosed: () -> Unit = dismiss) {
    val context = remember(profile, state.profiles, state.providers, reported) {
        modelEditorContext(profile, state.profiles, state.providers, reported)
    }
    val currentContext by rememberUpdatedState(context)
    val session = rememberSaveable(saver = SessionSaver) { EditorSession(null) }
    fun call(action: JSONObject): JSONObject? {
        val request = JSONObject().put("context", currentContext)
            .put("state", session.state?.let(::JSONObject) ?: JSONObject.NULL).put("action", action)
        val response = JSONObject(NativeBridge.modelEditor(request.toString()))
        if (!response.optBoolean("ok")) {
            session.failure = response.text("error", "编辑器已过期，请重新打开"); return null
        }
        val data = response.getJSONObject("data")
        session.state = data.getJSONObject("state").toString()
        session.view = data.getJSONObject("view")
        return data
    }
    // First composition (or recreation): open, or re-project the saved state.
    remember(session) {
        if (session.view == null) call(when {
            session.state != null -> action("view")
            modelId == null -> action("open_add")
            else -> action("open_edit", "model" to modelId)
        })
        Unit
    }
    // The connection's models changed elsewhere (a refresh, another device): re-project.
    // Not while closing: a save's refresh would otherwise flash “已经有 …” on the way out.
    LaunchedEffect(context) { if (open && session.view != null && session.failure == null) call(action("view")) }
    LaunchedEffect(session.failure) {
        session.failure?.let { finished(it); dismiss() }
    }
    val view = session.view
    val submit = rememberSettingsSubmission(state, actions) { operation, _ ->
        val id = JSONObject(session.state ?: "{}").let { s ->
            s.optJSONObject("original")?.text("id")?.takeIf { it.isNotBlank() } ?: s.text("id").trim()
        }
        finished(when { operation == "remove_model" -> "已移除 $id"; modelId == null -> "已添加 $id"; else -> "已保存 $id" })
    }
    val busy = submit.busy
    val editable = !busy && state.online

    // ---- UI-local state
    val sheet = remember { SheetWindow() }
    val idFocus = remember { FocusRequester() }
    val contextFocus = remember { FocusRequester() }
    val outputFocus = remember { FocusRequester() }
    val sectionViews = remember { mutableMapOf<String, BringIntoViewRequester>() }
    fun sectionView(key: String) = sectionViews.getOrPut(key) { BringIntoViewRequester() }
    var idValue by rememberSaveable(stateSaver = TextFieldValue.Saver) {
        mutableStateOf(TextFieldValue(view?.optJSONObject("id")?.text("text").orEmpty()))
    }
    var idFocused by remember { mutableStateOf(false) }
    var suggesting by remember { mutableStateOf(false) }
    var highlight by remember { mutableIntStateOf(-1) }
    var recognizeTick by remember { mutableIntStateOf(0) }
    var sourcesOpen by rememberSaveable { mutableStateOf(false) }
    var levelAdding by rememberSaveable { mutableStateOf(false) }
    var levelText by rememberSaveable { mutableStateOf("") }
    var budgetAdding by rememberSaveable { mutableStateOf(false) }
    var budgetText by rememberSaveable { mutableStateOf("") }
    var pendingFocus by remember { mutableStateOf<String?>(null) }

    fun step(type: String, vararg fields: Pair<String, Any?>) = call(action(type, *fields))
    // Recognition runs once typing pauses for 250 ms; the old status stays meanwhile.
    LaunchedEffect(recognizeTick) {
        if (recognizeTick == 0) return@LaunchedEffect
        delay(250)
        step("recognize")
    }
    // The id is decided (picked, or Done): end typing so the form has the screen.
    fun settle(id: String?) {
        suggesting = false; highlight = -1
        if (id != null) idValue = TextFieldValue(id, TextRange(id.length))
        step("settle", "id" to id)
        sheet.endTyping()
    }
    fun save() {
        val saved = JSONObject(session.state ?: "{}")
        // A tap on 保存 does not blur the id field; conclude the id first.
        if (saved.text("mode") == "add" && !saved.optBoolean("settled")) { suggesting = false; step("settle", "id" to null) }
        val data = step("save") ?: return
        val effect = data.optJSONObject("effect")?.optJSONObject("save")
        if (effect != null) {
            sheet.endTyping()
            submit.perform("save_model", JSONObject().put("profile", profile.text("profile_id")).put("input", effect))
        } else data.text("focus").takeIf { it.isNotBlank() }?.let { pendingFocus = it }
    }
    // After a failed save the sections with errors have expanded; bring the first one into view.
    LaunchedEffect(pendingFocus) {
        val target = pendingFocus ?: return@LaunchedEffect
        delay(ZorkMotion.BASE.toLong())
        runCatching {
            when (target) {
                "id" -> idFocus.requestFocus()
                "context" -> { sectionView("length").bringIntoView(); contextFocus.requestFocus() }
                "output" -> { sectionView("length").bringIntoView(); outputFocus.requestFocus() }
                "thinking" -> sectionView("thinking").bringIntoView()
                else -> sectionView("api").bringIntoView()
            }
        }
        pendingFocus = null
    }
    // Back closes the innermost layer first: suggestions → add inputs → the sheet.
    fun back(): Boolean = when {
        suggesting -> { suggesting = false; highlight = -1; true }
        levelAdding -> { levelAdding = false; levelText = ""; step("clear_add_errors"); true }
        budgetAdding -> { budgetAdding = false; budgetText = ""; step("clear_add_errors"); true }
        else -> false
    }

    val mode = view?.text("mode") ?: if (modelId == null) "add" else "edit"
    val title = view?.text("title")?.takeIf { it.isNotBlank() } ?: if (modelId == null) "添加模型" else modelId
    SettingsSheet(title, busy, submit.error, dismiss, open = open, onClosed = onClosed, monoTitle = mode == "edit", back = ::back, footer = {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            if (view?.optJSONObject("footer")?.optBoolean("remove") == true) DangerQuietButton("移除", editable) {
                submit.perform("remove_model", JSONObject().put("profile", profile.text("profile_id"))
                    .put("model", JSONObject(session.state ?: "{}").optJSONObject("original") ?: JSONObject().put("id", modelId)))
            }
            Spacer(Modifier.weight(1f))
            ZorkButton("取消", quiet = true, enabled = !busy, onClick = dismiss)
            ZorkButton(if (busy) "保存中…" else view?.optJSONObject("footer")?.text("save") ?: "保存", primary = true,
                enabled = editable && view != null, onClick = { save() })
        }
    }) {
        sheet.focus = LocalFocusManager.current
        sheet.keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
        if (view == null) return@SettingsSheet
        if (!state.online) Text("设备离线，恢复连接后可修改。", color = ZorkColors.Muted, fontSize = 13.sp)
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            val idView = view.getJSONObject("id")
            if (idView.optBoolean("editable")) {
                IdField(idValue, idView, editable, idFocus,
                    change = { value ->
                        val changed = value.text != idValue.text
                        idValue = value
                        if (changed) {
                            suggesting = true; highlight = -1
                            step("set_id", "id" to value.text)
                            recognizeTick++
                        }
                    },
                    focusChanged = { focused ->
                        if (focused && !idFocused) suggesting = true
                        if (!focused && idFocused) {
                            val wasOpen = suggesting
                            suggesting = false; highlight = -1
                            if (wasOpen || JSONObject(session.state ?: "{}").optBoolean("settled").not()) step("settle", "id" to null)
                        }
                        idFocused = focused
                    },
                    key = { key ->
                        val flat = suggestionIds(idView)
                        when (key) {
                            Key.DirectionDown -> { suggesting = true; highlight = if (flat.isEmpty()) -1 else (highlight + 1) % flat.size; true }
                            Key.DirectionUp -> { highlight = if (flat.isEmpty()) -1 else if (highlight <= 0) flat.lastIndex else highlight - 1; true }
                            Key.Enter, Key.NumPadEnter -> {
                                if (suggesting && highlight in flat.indices) settle(flat[highlight]) else settle(null); true
                            }
                            Key.Escape -> if (suggesting) { suggesting = false; highlight = -1; true } else false
                            else -> false
                        }
                    },
                    done = {
                        val flat = suggestionIds(idView)
                        if (suggesting && highlight in flat.indices) settle(flat[highlight]) else settle(null)
                    })
                AnimatedVisibility(suggesting && idFocused && suggestionIds(idView).isNotEmpty(), enter = zorkRiseIn(), exit = zorkFadeOut(ZorkMotion.FAST)) {
                    Suggestions(idView.optJSONObject("suggestions"), highlight, editable) { settle(it) }
                }
                idView.text("error").takeIf { it.isNotBlank() }?.let { message ->
                    val duplicate = idView.text("duplicate")
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                        Text(message, fontSize = 12.sp, color = ZorkColors.Danger)
                        if (duplicate.isNotBlank()) EditorLink("去编辑它", ZorkColors.Danger) { editOther(duplicate) }
                    }
                }
            }
            view.optJSONObject("status")?.let { status ->
                StatusLine(status, idView.optBoolean("pending"), editable, { sourcesOpen = true }) { step("refill") }
            }
            val full = view.optBoolean("full")
            view.optJSONArray("sections").objects().forEach { section ->
                val key = section.text("section")
                key(key) {
                    EditorSection(section, full, editable, Modifier.bringIntoViewRequester(sectionView(key)),
                        toggle = { step("toggle_section", "section" to key) },
                        restore = { step("restore", "section" to key) }) {
                        when (key) {
                            "thinking" -> ThinkingBody(view.getJSONObject("thinking"), editable, ::step,
                                levelAdding, { levelAdding = it; if (!it) levelText = "" }, levelText, { levelText = it },
                                budgetAdding, { budgetAdding = it; if (!it) budgetText = "" }, budgetText, { budgetText = it })
                            "length" -> LengthBody(view.getJSONObject("length"), editable, contextFocus, outputFocus, ::step)
                            "image" -> view.getJSONObject("image").let { image ->
                                SettingsToggle(image.text("label"), image.optBoolean("on"), editable, image.text("help")) { step("set_image", "on" to it) }
                            }
                            "api" -> view.optJSONObject("api")?.let { ApiBody(it, editable) { api -> step("set_api", "api" to api) } }
                        }
                    }
                }
            }
        }
    }
    ZorkRetained(Unit.takeIf { sourcesOpen && open }) { _, shown, closed ->
        FillSourceSheet(currentContext, session.state, view?.optJSONObject("status")?.text("action") ?: "从相似模型填入",
            shown, { sourcesOpen = false }, closed) { reference ->
            sourcesOpen = false
            step("use_source", "source" to reference)
        }
    }
}

private fun suggestionIds(idView: JSONObject): List<String> {
    val suggestions = idView.optJSONObject("suggestions") ?: return emptyList()
    return suggestions.optJSONArray("groups").objects().flatMap { g -> g.optJSONArray("items").objects().map { it.text("id") } } +
        listOfNotNull(suggestions.optJSONObject("custom")?.text("id")?.takeIf { it.isNotBlank() })
}

@Composable
private fun IdField(value: TextFieldValue, view: JSONObject, enabled: Boolean, focus: FocusRequester,
    change: (TextFieldValue) -> Unit, focusChanged: (Boolean) -> Unit, key: (Key) -> Boolean, done: () -> Unit) {
    val error = view.text("error").isNotBlank()
    OutlinedTextField(value, change, enabled = enabled, singleLine = true,
        placeholder = { Text(view.text("placeholder"), fontSize = 14.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Done, autoCorrectEnabled = false),
        keyboardActions = KeyboardActions(onDone = { done() }),
        isError = error, shape = ZorkShapes.Control,
        textStyle = LocalTextStyle.current.copy(fontSize = 15.sp, fontFamily = FontFamily.Monospace),
        modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp).focusRequester(focus)
            .onFocusChanged { focusChanged(it.isFocused) }
            .onPreviewKeyEvent { event -> event.type == KeyEventType.KeyDown && key(event.key) }
            .semantics { contentDescription = "模型 ID" })
}

/** The suggestion list under the id: what the provider reported, presets, and the typed id itself. */
@Composable
private fun Suggestions(suggestions: JSONObject?, highlight: Int, enabled: Boolean, pick: (String) -> Unit) {
    if (suggestions == null) return
    var index = 0
    Column(Modifier.fillMaxWidth().heightIn(max = 320.dp).background(ZorkColors.Prompt, ZorkShapes.Block)
        .clip(ZorkShapes.Block).verticalScroll(rememberScrollState()).padding(vertical = 6.dp)
        .semantics { contentDescription = "模型 ID 建议" }) {
        suggestions.optJSONArray("groups").objects().forEach { group ->
            Text(group.text("title"), fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle,
                modifier = Modifier.padding(start = 14.dp, end = 14.dp, top = 8.dp, bottom = 2.dp))
            group.optJSONArray("items").objects().forEach { item ->
                val at = index++
                SuggestionRow(at == highlight, enabled, { pick(item.text("id")) },
                    listOf(item.text("id"), item.text("name"), item.text("meta")).filter(String::isNotBlank).joinToString("，")) {
                    TwoLines(item.text("id"), listOf(item.text("name"), item.text("meta")), mono = true)
                }
            }
        }
        suggestions.optJSONObject("custom")?.let { custom ->
            val at = index
            SuggestionRow(at == highlight, enabled, { pick(custom.text("id")) }, custom.text("label")) {
                Glyph(R.drawable.ic_plus, 14.dp, ZorkColors.Muted)
                Text(custom.text("label"), fontSize = 14.sp, color = ZorkColors.Muted, maxLines = 2, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun SuggestionRow(highlighted: Boolean, enabled: Boolean, click: () -> Unit, description: String,
    content: @Composable RowScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).padding(horizontal = 6.dp)
        .background(if (highlighted) ZorkColors.Selected else Color.Transparent, ZorkShapes.Container)
        .clip(ZorkShapes.Container).zorkPressable(enabled = enabled, onClick = click)
        .semantics { contentDescription = description }
        .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), content = content)
}

/** A suggestion's id (or name) over its muted details. */
@Composable
private fun RowScope.TwoLines(title: String, details: List<String>, mono: Boolean) {
    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
        Text(title, fontSize = 14.sp, fontFamily = if (mono) FontFamily.Monospace else null, maxLines = 1, overflow = TextOverflow.Ellipsis)
        details.filter(String::isNotBlank).joinToString(" · ").takeIf { it.isNotBlank() }?.let {
            Text(it, fontSize = 12.sp, color = ZorkColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** A link-like quiet action that stays a 44 dp touch target inside running text. */
@Composable
private fun EditorLink(text: String, color: Color = ZorkColors.Ink, enabled: Boolean = true, description: String? = null, click: () -> Unit) {
    Box(Modifier.heightIn(min = 44.dp).clip(ZorkShapes.Control).zorkPressable(enabled = enabled, onClick = click)
        .then(if (description != null) Modifier.semantics { contentDescription = description } else Modifier),
        contentAlignment = Alignment.Center) {
        Text(text, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = if (enabled) color else ZorkColors.Disabled, maxLines = 1)
    }
}

@Composable
private fun DangerQuietButton(text: String, enabled: Boolean, click: () -> Unit) {
    Box(Modifier.heightIn(min = 48.dp).clip(ZorkShapes.Control).zorkPressable(enabled = enabled, onClick = click)
        .padding(horizontal = 14.dp), contentAlignment = Alignment.Center) {
        Text(text, fontSize = 14.sp, color = if (enabled) ZorkColors.Danger else ZorkColors.Disabled)
    }
}

/** `✓ DeepSeek Reasoner · 参数已按预设填好 · 换一个来源`, or the no-preset warning. */
@Composable
private fun StatusLine(status: JSONObject, pending: Boolean, enabled: Boolean, sources: () -> Unit, refill: () -> Unit) {
    val warn = status.text("kind") == "no_preset"
    val alpha = animatedValue(if (pending) .55f else 1f, "status")
    val title = status.text("title")
    Row(Modifier.fillMaxWidth().graphicsLayer { this.alpha = alpha }, horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = if (title.isBlank()) Alignment.CenterVertically else Alignment.Top) {
        Box(Modifier.padding(top = if (title.isBlank()) 0.dp else 2.dp)) {
            when {
                // An edited model without a preset: a plain note, not a warning.
                warn && title.isBlank() -> Glyph(R.drawable.ic_attention, 16.dp, ZorkColors.Subtle)
                warn -> Glyph(R.drawable.ic_attention, 16.dp, ZorkColors.Warning)
                else -> Glyph(R.drawable.ic_check, 16.dp, ZorkColors.Online)
            }
        }
        Column(Modifier.weight(1f)) {
            if (title.isNotBlank()) Text(title, fontSize = 14.sp, fontWeight = FontWeight.Medium, lineHeight = 20.sp)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                Text(status.text("detail"), fontSize = 12.sp, lineHeight = 18.sp, color = ZorkColors.Muted)
                EditorLink(status.text("action"), enabled = enabled, click = sources)
            }
            status.optJSONObject("refill")?.let { offer ->
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
                    Text(offer.text("text"), fontSize = 12.sp, lineHeight = 18.sp, color = ZorkColors.Muted)
                    EditorLink(offer.text("action"), enabled = enabled, click = refill)
                }
            }
        }
    }
}

/**
 * One section. Collapsed it is a summary row; open it gains a soft fill. In the
 * numbered form every section is open under its question and cannot collapse.
 * The provenance line and its 恢复 sit beside the row, never inside it.
 */
@Composable
private fun EditorSection(section: JSONObject, full: Boolean, enabled: Boolean, modifier: Modifier,
    toggle: () -> Unit, restore: () -> Unit, body: @Composable () -> Unit) {
    val open = section.optBoolean("open")
    val filled = open && !full
    val fill by animateColorAsState(if (filled) ZorkColors.Prompt else ZorkColors.Canvas, ZorkMotion.move(ZorkMotion.BASE), label = "section")
    Column(modifier.fillMaxWidth().background(fill, ZorkShapes.Block)) {
        if (full) {
            Row(Modifier.fillMaxWidth().heightIn(min = 40.dp).padding(horizontal = 4.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(22.dp).background(ZorkColors.Prompt, CircleShape), contentAlignment = Alignment.Center) {
                    Text(section.optInt("number").toString(), fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Muted)
                }
                Text(section.text("question"), fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                if (section.optBoolean("error")) Box(Modifier.size(6.dp).background(ZorkColors.Danger, CircleShape))
            }
        } else {
            val angle = zorkChevron(open)
            Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(ZorkShapes.Block)
                .zorkPressable(enabled = true, onClick = toggle)
                .semantics {
                    contentDescription = "${section.text("label")}：${section.text("summary")}"
                    stateDescription = if (open) "已展开" else "已收起"
                }
                .padding(start = 12.dp, end = 10.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(section.text("label"), fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.width(40.dp))
                Text(section.text("summary"), fontSize = 14.sp, color = if (section.optBoolean("muted")) ZorkColors.Subtle else ZorkColors.Ink,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                if (section.optBoolean("error") && !open) Box(Modifier.size(6.dp).background(ZorkColors.Danger, CircleShape))
                Box(Modifier.graphicsLayer { rotationZ = angle }) { Glyph(R.drawable.ic_chevron_down, 16.dp, ZorkColors.Subtle) }
            }
        }
        section.optJSONObject("provenance")?.let { provenance ->
            Row(Modifier.fillMaxWidth().padding(start = if (full) 36.dp else 62.dp, end = 4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(provenance.text("text"), fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false))
                RestoreButton(section.text("label"), enabled, restore)
            }
        }
        ZorkExpand(open) {
            Column(Modifier.fillMaxWidth().padding(start = if (full) 4.dp else 12.dp, end = if (full) 4.dp else 12.dp, top = 4.dp, bottom = 12.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp)) {
                CompositionLocalProvider(LocalSectionFill provides filled) { body() }
            }
        }
    }
}

@Composable
private fun RestoreButton(section: String, enabled: Boolean, click: () -> Unit) {
    val fill = if (LocalSectionFill.current) ZorkColors.Canvas else ZorkColors.Prompt
    Box(Modifier.heightIn(min = 44.dp).clip(ZorkShapes.Control).zorkPressable(enabled = enabled, onClick = click)
        .semantics { contentDescription = "恢复$section" }, contentAlignment = Alignment.Center) {
        Row(Modifier.background(fill, ZorkShapes.Control).padding(horizontal = 10.dp, vertical = 5.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            Glyph(R.drawable.ic_reload, 12.dp, ZorkColors.Muted)
            Text("恢复", fontSize = 12.sp, color = ZorkColors.Muted)
        }
    }
}

/** A radio row with a title and an example line; [content] (the chosen kind's editor) opens below it. */
@Composable
private fun RadioOption(title: String, detail: String, selected: Boolean, enabled: Boolean, suffix: String? = null,
    choose: () -> Unit, content: (@Composable ColumnScope.() -> Unit)? = null) {
    val ring by animateDpAsState(if (selected) 5.dp else 1.5.dp, ZorkMotion.move(ZorkMotion.FAST), label = "radio")
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(ZorkShapes.Block)
            .selectable(selected, enabled = enabled, role = Role.RadioButton, onClick = choose)
            .padding(horizontal = 8.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Box(Modifier.padding(top = 1.dp).size(18.dp).border(ring, if (selected) ZorkColors.Ink else ZorkColors.FieldBorder, CircleShape))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(androidx.compose.ui.text.buildAnnotatedString {
                    append(title)
                    suffix?.let {
                        pushStyle(androidx.compose.ui.text.SpanStyle(fontSize = 12.sp, fontWeight = FontWeight.Normal, color = ZorkColors.Subtle))
                        append("  $it"); pop()
                    }
                }, fontSize = 14.sp, fontWeight = FontWeight.Medium, lineHeight = 20.sp)
                if (detail.isNotBlank()) Text(detail, fontSize = 12.sp, lineHeight = 17.sp, color = ZorkColors.Muted)
            }
        }
        if (content != null) ZorkExpand(selected) {
            Column(Modifier.fillMaxWidth().padding(start = 38.dp, end = 4.dp, top = 2.dp, bottom = 8.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
        }
    }
}

/**
 * A 44 dp touch target drawing a shorter capsule: quick picks, defaults and
 * addable names. Pressing tints the capsule instead of the whole target.
 */
@Composable
private fun TouchChip(label: String, selected: Boolean, enabled: Boolean = true, mono: Boolean = false,
    description: String? = null, role: Role = Role.RadioButton, click: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val base = if (LocalSectionFill.current) ZorkColors.Canvas else ZorkColors.Prompt
    val fill by animateColorAsState(when { selected -> ZorkColors.Ink; pressed -> ZorkColors.Pressed; else -> base },
        ZorkMotion.move(ZorkMotion.FAST), label = "chip")
    Box(Modifier.heightIn(min = 44.dp)
        .selectable(selected, enabled = enabled, role = role, interactionSource = interaction, indication = null, onClick = click)
        .semantics {
            description?.let { contentDescription = it }
            if (role == Role.RadioButton) stateDescription = if (selected) "已选中" else "未选中"
        },
        contentAlignment = Alignment.Center) {
        Box(Modifier.height(34.dp).background(fill, ZorkShapes.Control).padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
            Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1,
                fontFamily = if (mono) FontFamily.Monospace else null,
                color = when { selected -> ZorkColors.Canvas; enabled -> ZorkColors.Ink; else -> ZorkColors.Disabled })
        }
    }
}

// ---------------------------------------------------------------- thinking

@Composable
private fun ThinkingBody(thinking: JSONObject, enabled: Boolean, step: (String, Array<out Pair<String, Any?>>) -> JSONObject?,
    levelAdding: Boolean, setLevelAdding: (Boolean) -> Unit, levelText: String, setLevelText: (String) -> Unit,
    budgetAdding: Boolean, setBudgetAdding: (Boolean) -> Unit, budgetText: String, setBudgetText: (String) -> Unit) {
    fun send(type: String, vararg fields: Pair<String, Any?>) = step(type, fields)
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        thinking.optJSONArray("kinds").objects().forEach { kind ->
            val selected = kind.optBoolean("selected")
            RadioOption(kind.text("title"), kind.text("example"), selected, enabled, choose = {
                if (!selected) { setLevelAdding(false); setBudgetAdding(false); send("set_kind", "kind" to kind.text("kind")) }
            }) {
                when (kind.text("kind")) {
                    "toggle" -> thinking.optJSONObject("toggle")?.let { toggle ->
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            Text("默认", fontSize = 13.sp, color = ZorkColors.Muted)
                            ZorkSegments(listOf("off" to "关", "on" to "开"), if (toggle.optBoolean("default_on")) "on" else "off",
                                { send("toggle_default", "on" to (it == "on")) }, Modifier.width(160.dp), enabled)
                        }
                    }
                    "levels" -> thinking.optJSONObject("levels")?.let { levels ->
                        LevelsEditor(levels, enabled, ::send, levelAdding, setLevelAdding, levelText, setLevelText)
                    }
                    "budget" -> thinking.optJSONObject("budget")?.let { budget ->
                        BudgetEditor(budget, enabled, ::send, budgetAdding, setBudgetAdding, budgetText, setBudgetText)
                    }
                }
                thinking.text("note").takeIf { it.isNotBlank() && selected }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Muted) }
            }
        }
    }
    thinking.text("error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
    if (thinking.opt("selected").let { it != null && it != JSONObject.NULL }) PanelPreview(thinking)
}

/** What the composer's model panel will show for this scheme, plus where the value goes in requests. */
@Composable
private fun PanelPreview(thinking: JSONObject) {
    var info by remember { mutableStateOf(false) }
    val panel = thinking.optJSONObject("panel")
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        val shape = panel?.optJSONObject("panel")
        val options = shape?.takeIf { it.text("kind") == "options" }?.optJSONArray("options")
            ?.let { a -> (0 until a.length()).map { a.optString(it) } }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("模型面板里会显示", fontSize = 12.sp, color = ZorkColors.Muted)
            if (options == null) Text(panel?.text("text").orEmpty(), fontSize = 12.sp, color = ZorkColors.Ink, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            Spacer(Modifier.weight(1f))
            Box(Modifier.size(44.dp).clip(CircleShape).zorkPressable(onClick = { info = !info })
                .semantics { contentDescription = "请求字段"; stateDescription = if (info) "已展开" else "已收起" },
                contentAlignment = Alignment.Center) {
                Text("ⓘ", fontSize = 16.sp, color = if (info) ZorkColors.Ink else ZorkColors.Subtle)
            }
        }
        // The composer's own control, shown exactly: it can be wider than the sheet, so it scrolls.
        if (options != null) {
            val chosen = shape.optInt("selected", -1)
            Row(Modifier.horizontalScroll(rememberScrollState()).background(if (LocalSectionFill.current) ZorkColors.Canvas else ZorkColors.Prompt, ZorkShapes.Control)
                .padding(3.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                options.forEachIndexed { i, label ->
                    Box(Modifier.height(28.dp).background(if (i == chosen) ZorkColors.Ink else Color.Transparent, ZorkShapes.Control)
                        .padding(horizontal = 10.dp), contentAlignment = Alignment.Center) {
                        Text(thinkingLabel(label), fontSize = 12.sp, maxLines = 1, color = if (i == chosen) ZorkColors.Canvas else ZorkColors.Muted)
                    }
                }
            }
        }
        ZorkExpand(info) { Text(thinking.text("request_field"), fontSize = 12.sp, color = ZorkColors.Muted, fontFamily = FontFamily.Monospace) }
    }
}

/**
 * Level chips: tap sets the default, × deletes, long-press drags to reorder.
 * Each chip is two sibling touch targets drawn as one capsule.
 */
@Composable
private fun LevelsEditor(levels: JSONObject, enabled: Boolean, send: (String, Array<out Pair<String, Any?>>) -> JSONObject?,
    adding: Boolean, setAdding: (Boolean) -> Unit, text: String, setText: (String) -> Unit) {
    val values = levels.optJSONArray("values").objects()
    val canDelete = levels.optBoolean("can_delete")
    // Chip bounds by name. A drag lifts the chip under the finger and marks the
    // chip it would take the place of; the order changes once, on release, so
    // the gesture never loses its target to a recomposition.
    val bounds = remember { mutableStateMapOf<String, Rect>() }
    val current by rememberUpdatedState(values)
    var dragging by remember { mutableStateOf<String?>(null) }
    var target by remember { mutableStateOf<String?>(null) }
    var pointer by remember { mutableStateOf(Offset.Zero) }
    var grab by remember { mutableStateOf(Offset.Zero) }
    fun indexOf(name: String?) = current.indexOfFirst { it.text("name") == name }
    fun move(from: Int, to: Int) { if (from != to && from in current.indices && to in current.indices) send("level_move", arrayOf("from" to from, "to" to to)) }
    fun drop() {
        val from = indexOf(dragging); val to = indexOf(target)
        dragging = null; target = null
        if (from >= 0 && to >= 0) move(from, to)
    }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
        values.forEachIndexed { index, chip ->
            val name = chip.text("name")
            val default = chip.optBoolean("default")
            val lifted = dragging == name
            key(name) {
                CapsuleChip(name, default, bad = false, enabled = enabled, canDelete = canDelete, marked = target == name,
                    modifier = Modifier
                        .onGloballyPositioned { bounds[name] = it.boundsInRoot() }
                        .zIndex(if (lifted) 1f else 0f)
                        .graphicsLayer {
                            val at = bounds[name]
                            if (lifted && at != null) {
                                translationX = pointer.x - grab.x - at.left
                                translationY = pointer.y - grab.y - at.top
                                scaleX = 1.05f; scaleY = 1.05f; shadowElevation = 6.dp.toPx()
                                shape = ZorkShapes.Control; clip = false
                            }
                        },
                    nameModifier = Modifier
                        .pointerInput(name, enabled) {
                            if (!enabled) return@pointerInput
                            detectDragGesturesAfterLongPress(
                                onDragStart = { offset ->
                                    val at = bounds[name] ?: return@detectDragGesturesAfterLongPress
                                    dragging = name; target = null; grab = offset
                                    pointer = Offset(at.left + offset.x, at.top + offset.y)
                                },
                                onDrag = { change, amount ->
                                    change.consume()
                                    pointer += amount
                                    target = bounds.entries.firstOrNull { (other, r) -> other != name && indexOf(other) >= 0 && r.contains(pointer) }?.key
                                },
                                onDragEnd = { drop() },
                                onDragCancel = { dragging = null; target = null })
                        }
                        .semantics {
                            customActions = listOfNotNull(
                                CustomAccessibilityAction("前移") { move(index, index - 1); true }.takeIf { index > 0 },
                                CustomAccessibilityAction("后移") { move(index, index + 1); true }.takeIf { index < values.lastIndex })
                        },
                    choose = { send("level_default", arrayOf("name" to name)) },
                    delete = { send("level_delete", arrayOf("index" to index)) })
            }
        }
        AddChip("+ 档位", adding, enabled) { setAdding(!adding); if (adding) send("clear_add_errors", emptyArray()) }
    }
    AnimatedVisibility(adding, enter = zorkRiseIn(), exit = zorkFadeOut(ZorkMotion.FAST)) {
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            val addable = levels.optJSONArray("addable")?.let { a -> (0 until a.length()).map { a.optString(it) } }.orEmpty()
            if (addable.isNotEmpty()) FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                addable.forEach { name ->
                    TouchChip(name, false, enabled, mono = true, description = "添加档位 $name", role = Role.Button) {
                        send("level_add", arrayOf("name" to name))?.let { if (levelAdded(it)) { setAdding(false) } }
                    }
                }
            }
            AddInput(text, setText, "其他名称", "新档位名称", KeyboardType.Ascii, enabled) {
                send("level_add", arrayOf("name" to text.trim()))?.let { if (levelAdded(it)) setAdding(false) }
            }
            levels.text("add_error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
        }
    }
    Text(levels.text("help"), fontSize = 12.sp, lineHeight = 17.sp, color = ZorkColors.Muted)
}

private fun levelAdded(data: JSONObject) =
    data.optJSONObject("view")?.optJSONObject("thinking")?.optJSONObject("levels")?.text("add_error").isNullOrBlank()

private fun budgetAdded(data: JSONObject) =
    data.optJSONObject("view")?.optJSONObject("thinking")?.optJSONObject("budget")?.text("add_error").isNullOrBlank()

@Composable
private fun BudgetEditor(budget: JSONObject, enabled: Boolean, send: (String, Array<out Pair<String, Any?>>) -> JSONObject?,
    adding: Boolean, setAdding: (Boolean) -> Unit, text: String, setText: (String) -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), itemVerticalAlignment = Alignment.CenterVertically) {
        budget.optJSONArray("presets").objects().forEachIndexed { index, chip ->
            val tokens = chip.optLong("tokens")
            key(tokens) {
                CapsuleChip(chip.text("label"), chip.optBoolean("default"), chip.optBoolean("bad"), enabled, canDelete = true,
                    choose = { send("budget_default", arrayOf("choice" to JSONObject().put("kind", "tokens").put("tokens", tokens))) },
                    delete = { send("budget_delete", arrayOf("index" to index)) })
            }
        }
        AddChip("+ 预算", adding, enabled) { setAdding(!adding); if (adding) send("clear_add_errors", emptyArray()) }
    }
    AnimatedVisibility(adding, enter = zorkRiseIn(), exit = zorkFadeOut(ZorkMotion.FAST)) {
        AddInput(text, setText, "例如 8K", "新预算", KeyboardType.Ascii, enabled) {
            send("budget_add", arrayOf("text" to text.trim()))?.let { if (budgetAdded(it)) { setText(""); setAdding(false) } }
        }
    }
    budget.text("add_error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
    SettingsToggle("可以关闭思考", budget.optBoolean("allow_off"), enabled) { send("budget_allow_off", arrayOf("on" to it)) }
    SettingsToggle("可以让模型自己决定", budget.optBoolean("dynamic"), enabled, budget.text("dynamic_help")) { send("budget_dynamic", arrayOf("on" to it)) }
    val defaults = budget.optJSONArray("defaults").objects()
    if (defaults.isNotEmpty()) Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("默认", fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.padding(top = 12.dp))
        FlowRow(Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            defaults.forEach { option ->
                TouchChip(option.text("label"), option.optBoolean("selected"), enabled, description = "默认预算 ${option.text("label")}") {
                    send("budget_default", arrayOf("choice" to option.getJSONObject("choice")))
                }
            }
        }
    }
}

/** One capsule with two sibling targets: the value (tap) and × (delete). */
@Composable
private fun CapsuleChip(label: String, default: Boolean, bad: Boolean, enabled: Boolean, canDelete: Boolean,
    marked: Boolean = false, modifier: Modifier = Modifier, nameModifier: Modifier = Modifier, choose: () -> Unit, delete: () -> Unit) {
    val nameInteraction = remember { MutableInteractionSource() }
    val deleteInteraction = remember { MutableInteractionSource() }
    val pressed = nameInteraction.collectIsPressedAsState().value || deleteInteraction.collectIsPressedAsState().value
    val base = if (LocalSectionFill.current) ZorkColors.Canvas else ZorkColors.Prompt
    val fill by animateColorAsState(when { default -> ZorkColors.Ink; pressed -> ZorkColors.Pressed; else -> base },
        ZorkMotion.move(ZorkMotion.FAST), label = "chip")
    val ink = when { default -> ZorkColors.Canvas; bad -> ZorkColors.Danger; enabled -> ZorkColors.Ink; else -> ZorkColors.Disabled }
    // A drop target shows an ink ring; a bad budget a danger one.
    val outline = if (bad) ZorkColors.Danger else if (marked) ZorkColors.Ink else Color.Transparent
    Row(modifier.height(44.dp).drawBehind {
        val inset = 5.dp.toPx(); val radius = (size.height - 2 * inset) / 2
        drawRoundRect(fill, Offset(0f, inset), Size(size.width, size.height - 2 * inset), CornerRadius(radius))
        if (outline != Color.Transparent) drawRoundRect(outline, Offset(0f, inset), Size(size.width, size.height - 2 * inset), CornerRadius(radius),
            style = Stroke((if (marked) 2 else 1).dp.toPx()))
    }, verticalAlignment = Alignment.CenterVertically) {
        Box(nameModifier.fillMaxHeight()
            .selectable(default, enabled = enabled, role = Role.RadioButton, interactionSource = nameInteraction, indication = null, onClick = choose)
            .semantics { stateDescription = if (default) "默认" else "设为默认" }
            .padding(start = 12.dp, end = 2.dp), contentAlignment = Alignment.Center) {
            Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, fontFamily = FontFamily.Monospace, color = ink, maxLines = 1,
                overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp))
        }
        Box(Modifier.fillMaxHeight().width(34.dp)
            .clickable(enabled = enabled && canDelete, role = Role.Button, interactionSource = deleteInteraction, indication = null, onClick = delete)
            .semantics { contentDescription = "删除 $label" }, contentAlignment = Alignment.Center) {
            Glyph(R.drawable.ic_x, 12.dp, if (enabled && canDelete) ink.copy(alpha = .6f) else ink.copy(alpha = .2f))
        }
    }
}

@Composable
private fun AddChip(label: String, open: Boolean, enabled: Boolean, click: () -> Unit) {
    Box(Modifier.heightIn(min = 44.dp).clip(ZorkShapes.Control).zorkPressable(enabled = enabled, onClick = click)
        .semantics { stateDescription = if (open) "已展开" else "已收起" }, contentAlignment = Alignment.Center) {
        Box(Modifier.height(34.dp).border(1.dp, ZorkColors.FieldBorder, ZorkShapes.Control).padding(horizontal = 12.dp),
            contentAlignment = Alignment.Center) {
            Text(label, fontSize = 13.sp, color = if (enabled) ZorkColors.Muted else ZorkColors.Disabled)
        }
    }
}

/** The inline `+ 档位` / `+ 预算` input; Done submits and core reports any error. */
@Composable
private fun AddInput(value: String, change: (String) -> Unit, placeholder: String, description: String, type: KeyboardType,
    enabled: Boolean, submit: () -> Unit) {
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(value, change, enabled = enabled, singleLine = true,
            placeholder = { Text(placeholder, fontSize = 14.sp, color = ZorkColors.Subtle) },
            keyboardOptions = KeyboardOptions(keyboardType = type, imeAction = ImeAction.Done, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onDone = { submit() }), shape = ZorkShapes.Control,
            textStyle = LocalTextStyle.current.copy(fontSize = 14.sp, fontFamily = FontFamily.Monospace),
            modifier = Modifier.weight(1f).heightIn(min = 48.dp).focusRequester(focus).semantics { contentDescription = description })
        ZorkButton("添加", enabled = enabled && value.isNotBlank(), onClick = submit)
    }
}

// ---------------------------------------------------------------- length

/** `128K` → ("128", "K"); a plain count is shown in K (131072 → 131.072 K). */
internal fun splitTokens(text: String): Pair<String, String> {
    val t = text.trim().replace(",", "").replace(" ", "")
    if (t.isEmpty()) return "" to "K"
    val unit = t.last().uppercaseChar()
    if (unit == 'K' || unit == 'M') return t.dropLast(1) to unit.toString()
    val count = t.toLongOrNull() ?: return t to "K"
    return BigDecimal(count).movePointLeft(3).stripTrailingZeros().toPlainString() to "K"
}

internal fun joinTokens(number: String, unit: String) = number.trim().let { if (it.isEmpty()) "" else it + unit }

@Composable
private fun LengthBody(length: JSONObject, enabled: Boolean, contextFocus: FocusRequester, outputFocus: FocusRequester,
    step: (String, Array<out Pair<String, Any?>>) -> JSONObject?) {
    TokenInput("context", length.getJSONObject("context"), enabled, contextFocus, step)
    TokenInput("output", length.getJSONObject("output"), enabled, outputFocus, step)
    Text(length.text("help"), fontSize = 12.sp, lineHeight = 17.sp, color = ZorkColors.Muted)
}

/**
 * A number, a K|M unit and the always-visible quick picks. What the user typed
 * stays on screen while it is what core holds; any other change (a pick, a
 * restore, normalizing on blur) shows core's value instead.
 */
@Composable
private fun TokenInput(field: String, view: JSONObject, enabled: Boolean, focus: FocusRequester,
    step: (String, Array<out Pair<String, Any?>>) -> JSONObject?) {
    val label = view.text("label")
    val coreText = view.text("text")
    var draft by remember { mutableStateOf<Pair<String, String>?>(null) }
    val shown = draft?.takeIf { joinTokens(it.first, it.second) == coreText } ?: splitTokens(coreText)
    var focused by remember { mutableStateOf(false) }
    val focusManager = LocalFocusManager.current
    fun send(number: String, unit: String) {
        draft = number to unit
        step("set_length", arrayOf("field" to field, "text" to joinTokens(number, unit)))
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label, fontSize = 12.sp, color = ZorkColors.Muted)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(shown.first, { send(it, shown.second) }, enabled = enabled, singleLine = true,
                placeholder = { Text(view.text("placeholder").removePrefix("例如 ").dropLast(1).let { "例如 $it" }, fontSize = 14.sp, color = ZorkColors.Subtle) },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { focusManager.clearFocus() }),
                // The exact count sits inside the field, right-aligned and muted.
                suffix = view.text("exact").takeIf { it.isNotBlank() }?.let { exact -> { Text(exact, fontSize = 12.sp, color = ZorkColors.Subtle, maxLines = 1) } },
                isError = view.text("error").isNotBlank(), shape = ZorkShapes.Control,
                textStyle = LocalTextStyle.current.copy(fontSize = 15.sp),
                modifier = Modifier.weight(1f).widthIn(min = 96.dp).heightIn(min = 52.dp).focusRequester(focus)
                    .onFocusChanged {
                        if (focused && !it.isFocused) { draft = null; step("blur_length", arrayOf("field" to field)) }
                        focused = it.isFocused
                    }
                    .onPreviewKeyEvent { event ->
                        val up = event.key == Key.DirectionUp
                        if (event.type == KeyEventType.KeyDown && (up || event.key == Key.DirectionDown)) {
                            draft = null; step("step_length", arrayOf("field" to field, "up" to up)); true
                        } else false
                    }
                    .semantics { contentDescription = label })
            UnitToggle(label, shown.second, enabled) { unit -> send(shown.first, unit) }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            view.optJSONArray("picks").objects().forEach { pick ->
                TouchChip(pick.text("label"), pick.optBoolean("selected"), enabled && !pick.optBoolean("disabled"),
                    description = "$label ${pick.text("label")}") {
                    draft = null
                    step("pick_length", arrayOf("field" to field, "value" to pick.optLong("value")))
                }
            }
        }
        view.text("error").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Danger) }
    }
}

/** K|M: the selected capsule slides on the move curve. */
@Composable
private fun UnitToggle(label: String, unit: String, enabled: Boolean, choose: (String) -> Unit) {
    val base = if (LocalSectionFill.current) ZorkColors.Canvas else ZorkColors.Prompt
    val thumb = if (LocalSectionFill.current) ZorkColors.Prompt else ZorkColors.Canvas
    val slot = animatedValue(if (unit == "M") 1f else 0f, "unit")
    Box(Modifier.width(92.dp).height(48.dp).background(base, ZorkShapes.Control).padding(3.dp)) {
        Box(Modifier.width(43.dp).fillMaxHeight().graphicsLayer { translationX = slot * 43.dp.toPx() }
            .background(thumb, ZorkShapes.Control).border(UiTokens.Border, UiTokens.Outline, ZorkShapes.Control))
        Row(Modifier.fillMaxSize()) {
            listOf("K", "M").forEach { option ->
                val on = option == unit
                Box(Modifier.weight(1f).fillMaxHeight().clip(ZorkShapes.Control)
                    .selectable(on, enabled = enabled, role = Role.RadioButton) { if (!on) choose(option) }
                    .semantics { contentDescription = "$label 单位 $option" }, contentAlignment = Alignment.Center) {
                    Text(option, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                        color = if (!enabled) ZorkColors.Disabled else if (on) ZorkColors.Ink else ZorkColors.Muted)
                }
            }
        }
    }
}

// ---------------------------------------------------------------- protocol

@Composable
private fun ApiBody(api: JSONObject, enabled: Boolean, choose: (String) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        api.optJSONArray("options").objects().forEach { option ->
            RadioOption(option.text("title"), option.text("description"), option.optBoolean("selected"), enabled,
                suffix = "· 连接默认".takeIf { option.optBoolean("default") }, choose = { choose(option.text("api")) })
        }
    }
    api.text("warning").takeIf { it.isNotBlank() }?.let { Text(it, fontSize = 12.sp, color = ZorkColors.Warning) }
}

// ---------------------------------------------------------------- fill sources

/** “从相似模型填入 / 换一个来源”: your configured models and presets, searchable. */
@Composable
private fun FillSourceSheet(context: JSONObject, editorState: String?, title: String, open: Boolean, dismiss: () -> Unit,
    closed: () -> Unit, choose: (JSONObject) -> Unit) {
    var query by remember { mutableStateOf("") }
    // Nothing is highlighted until ↑/↓; Done takes the highlighted row or the first.
    var highlight by remember { mutableIntStateOf(-1) }
    val groups = remember(query, context, editorState) {
        val response = JSONObject(NativeBridge.modelEditorSources(JSONObject().put("context", context)
            .put("state", editorState?.let(::JSONObject) ?: JSONObject.NULL).put("query", query).toString()))
        response.optJSONArray("data").objects()
    }
    val flat = groups.flatMap { it.optJSONArray("items").objects() }
    val focus = remember { FocusRequester() }
    LaunchedEffect(open) { if (open) { delay(ZorkMotion.PAGE.toLong()); runCatching { focus.requestFocus() } } }
    ZorkSheet(open, title, dismiss, onClosed = closed) {
        Text(title, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        OutlinedTextField(query, { query = it; highlight = -1 }, singleLine = true,
            placeholder = { Text("搜索模型或预设", fontSize = 14.sp, color = ZorkColors.Subtle) },
            leadingIcon = { Glyph(R.drawable.ic_search, 16.dp, ZorkColors.Subtle) },
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onDone = { (flat.getOrNull(highlight) ?: flat.firstOrNull())?.let { choose(it.getJSONObject("ref")) } }),
            shape = ZorkShapes.Control, textStyle = LocalTextStyle.current.copy(fontSize = 15.sp),
            modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp).focusRequester(focus)
                .onPreviewKeyEvent { event ->
                    if (event.type != KeyEventType.KeyDown || flat.isEmpty()) return@onPreviewKeyEvent false
                    when (event.key) {
                        Key.DirectionDown -> { highlight = (highlight + 1) % flat.size; true }
                        Key.DirectionUp -> { highlight = if (highlight <= 0) flat.lastIndex else highlight - 1; true }
                        else -> false
                    }
                }
                .semantics { contentDescription = "搜索来源" })
        Column(Modifier.fillMaxWidth().heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
            var index = 0
            groups.forEach { group ->
                Text(group.text("title"), fontSize = 12.sp, fontWeight = FontWeight.Medium, color = ZorkColors.Subtle,
                    modifier = Modifier.padding(start = 12.dp, top = 10.dp, bottom = 2.dp))
                group.optJSONArray("items").objects().forEach { item ->
                    val at = index++
                    SuggestionRow(at == highlight, true, { choose(item.getJSONObject("ref")) },
                        listOf(item.text("label"), item.text("sub"), item.text("meta")).filter(String::isNotBlank).joinToString("，")) {
                        TwoLines(item.text("label"), listOf(item.text("sub"), item.text("meta")), mono = group.text("kind") == "models")
                    }
                }
            }
            if (flat.isEmpty()) Text("没有匹配的模型", fontSize = 13.sp, color = ZorkColors.Muted, modifier = Modifier.padding(12.dp))
        }
    }
}
