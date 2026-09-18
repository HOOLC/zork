package ing.zork.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.drawText
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import org.json.JSONObject
import kotlin.math.*

internal data class ComposerMember(val id: String, val name: String, val avatar: String, val label: String, val failed: Boolean, val session: String = "")
private const val LABEL_GAP = 8f
private const val LABEL_RIGHT_PADDING = 12f
private const val PORTRAIT_INSET = 4.8f
internal data class ComposerBubble(val member: ComposerMember, val x: Float, val lift: Float, val width: Float, val reveal: Float)
internal data class ComposerPresence(val bubbles: List<ComposerBubble>) {
    val extent: Float get() = bubbles.maxOfOrNull { it.lift + 24f }?.coerceAtLeast(0f) ?: 0f
}

@Stable
internal class ComposerMotion(val node: LiquidNode, val members: List<ComposerAnimatedMember>) {
    fun sample() = ComposerPresence(members.map { it.sample() })
    val extent: Float get() = members.maxOfOrNull { it.lift.value + 24f }?.coerceAtLeast(0f) ?: 0f
    fun extentPixels(density: Float) = (extent * density).roundToInt()
    val targetExtent: Float get() = members.maxOfOrNull { it.targetLift + 24f }?.coerceAtLeast(0f) ?: 0f
}
internal data class ComposerAnimatedMember(
    val member: ComposerMember, val x: State<Float>, val lift: State<Float>,
    val progress: State<Float>, val labelWidth: State<Float>, val labelLayout: TextLayoutResult, val targetLift: Float,
) {
    val reveal: Float get() = progress.value
    fun sample() = ComposerBubble(member, x.value, lift.value, 32f + labelWidth.value, reveal)
}

internal fun activityLabel(activity: JSONObject?): String = when (activity?.text("state")) {
    "live" -> activity.optJSONObject("presentation")?.text("label_zh").orEmpty()
    "thinking", "tool_finished" -> "正在思考"
    "tools_started", "tools_waiting" -> if (activity.optBoolean("thinking") && activity.optJSONArray("calls").objects().isNotEmpty()) {
        val calls = activity.optJSONArray("calls").objects()
        val label = "正在思考"
        "$label · ${calls.size} 项操作执行中"
    } else activity.optJSONArray("calls").objects().joinToString(" · ") {
        if (it.text("action").isNotBlank()) return@joinToString it.text("action")
        val labels = it.optJSONObject("labels")
        val action = labels?.optString("zh-CN").orEmpty().ifBlank {
            labels?.optString("en").orEmpty().ifBlank { "执行操作" }
        }
        listOf(action, it.text("detail")).filter(String::isNotBlank).joinToString(" ")
    }.ifBlank { "正在处理" }
    "waiting" -> listOf("等待中", activity.text("reason")).filter(String::isNotBlank).joinToString(" · ")
    "failed" -> listOf("执行失败", activity.text("reason")).filter(String::isNotBlank).joinToString(" · ")
    else -> ""
}

@Composable
internal fun rememberComposerPresence(state: WorkbenchState, availableWidth: Float): ComposerMotion {
    val surface = rememberLiquidNode()
    val members = state.participants.map {
        val activity = it.optJSONObject("activity")
        ComposerMember(it.text("id"), it.text("name"), it.text("avatar"), activityLabel(activity), activity?.text("state") == "failed", it.text("session_id"))
    }
    // Snapshot event order must not shuffle the dock while a member is moving.
    val order = remember(state.conversation?.id) { mutableListOf<String>() }
    order.retainAll(members.map { it.id }.toSet())
    members.forEach { if (it.id !in order) order.add(it.id) }
    val measure = rememberTextMeasurer(cacheSize = 64)
    val density = androidx.compose.ui.platform.LocalDensity.current
    // Match Android's 48dp minimum hit expansion so adjacent cells stay disjoint.
    val dockColumns = (((availableWidth - 62f) / 48f).toInt() + 1).coerceAtLeast(1)
    val dockRows = ((members.size + dockColumns - 1) / dockColumns).coerceAtLeast(1)
    var idle = 0
    var active = 0
    val bubbles = order.mapNotNull { id -> members.find { it.id == id } }.map { member -> key(member.id) {
        var mounted by remember { mutableStateOf(false) }
        LaunchedEffect(Unit) { mounted = true }
        var graceExpanded by remember { mutableStateOf(false) }
        val live = member.label.isNotBlank()
        // An active status retargets all members in this composition. Waiting
        // for one effect per member previously caused a second full UI update.
        val expanded = mounted && (live || graceExpanded)
        LaunchedEffect(live) {
            if (live) graceExpanded = true
            else { delay(1200); graceExpanded = false }
        }
        val label = member.label.ifBlank { "空闲" }
        val maxTextWidth = ((availableWidth - 32f - LABEL_GAP - LABEL_RIGHT_PADDING + PORTRAIT_INSET)
            .coerceAtLeast(0f) * density.density).toInt()
        val labelLayout = measure.measure("${member.name} · $label", TextStyle(fontFamily = ZorkFonts.Body, fontSize = 12.sp),
            maxLines = 1, overflow = TextOverflow.Ellipsis,
            constraints = androidx.compose.ui.unit.Constraints(maxWidth = maxTextWidth))
        val textWidth = with(density) { labelLayout.size.width.toDp().value }
        val dockIndex = if (expanded) 0 else idle++
        val targetX = if (expanded) 8f else 22f + (dockIndex % dockColumns) * 48f
        val targetLift = if (expanded) dockRows * 48f - 8f + active++ * 48f else -8f + (dockIndex / dockColumns) * 48f
        val parcel = rememberLiquidNode()
        val targetWidth = if (expanded) (textWidth + LABEL_GAP + LABEL_RIGHT_PADDING - PORTRAIT_INSET)
            .coerceAtMost((availableWidth - 32f).coerceAtLeast(0f)) else 0f
        val start = remember { LiquidPose(x = targetX, y = -8f, width = 32f, height = 32f, radius = 16f) }
        SideEffect {
            parcel.update(LiquidTarget(LiquidKind.Member, start,
                LiquidPose(x = targetX, y = -targetLift - 16f, width = 32f + targetWidth, height = 32f, radius = 16f),
                parent = surface.id, border = 0f), density.density)
        }
        val x = remember(parcel) { derivedStateOf { parcel.revision; if (parcel.width > 0f) parcel.x else start.x } }
        val lift = remember(parcel) { derivedStateOf { parcel.revision; if (parcel.width > 0f) -parcel.y - 16f else -8f } }
        val progress = remember(parcel) { derivedStateOf { parcel.revision; parcel.progress } }
        val labelWidth = remember(parcel) { derivedStateOf { parcel.revision; (parcel.width - 32f).coerceAtLeast(0f) } }
        ComposerAnimatedMember(member.copy(label = label), x, lift, progress, labelWidth, labelLayout, targetLift)
    } }
    SideEffect {
        if (surface.target == null) surface.update(LiquidTarget(LiquidKind.Compound,
            LiquidPose(width = availableWidth.coerceAtLeast(2f), height = 96f, radius = 24f)), density.density)
    }
    return remember(surface, bubbles) { ComposerMotion(surface, bubbles) }
}

@Composable
internal fun ComposerMembers(presence: ComposerMotion, history: (String, String) -> Unit = { _, _ -> }) {
    val ellipsisMeasurer = rememberTextMeasurer(cacheSize = 64)
    val labelDensity = androidx.compose.ui.platform.LocalDensity.current
    // Sample animation in layout/draw, never in composition. Text is measured
    // at its settled width; the reveal only changes the clipping boundary.
    androidx.compose.ui.layout.Layout(modifier = Modifier.fillMaxWidth(), content = {
        presence.members.forEach { animated -> key(animated.member.id) {
            androidx.compose.ui.layout.Layout(modifier = Modifier.clip(LiquidShape(LiquidTokens.PillRadius))
                .historyPress(enabled = animated.member.session.isNotBlank(),
                radius = LiquidTokens.PillRadius,
                label = "${animated.member.name} · 执行历史 · ${animated.member.label}") {
                history(animated.member.session, animated.member.name)
            }, content = {
                Row(Modifier.clearAndSetSemantics { }.drawWithContent {
                    val revealedWidth = (22.4f + animated.labelWidth.value) * density
                    clipRect(right = min(size.width, revealedWidth)) { this@drawWithContent.drawContent() }
                }, verticalAlignment = Alignment.CenterVertically) {
                    ComposerPortrait(animated.member.avatar, animated.member.name)
                    Spacer(Modifier.width(LABEL_GAP.dp))
                    val label = "${animated.member.name} · ${animated.member.label}"
                    Canvas(Modifier.width(with(labelDensity) { animated.labelLayout.size.width.toDp() }).fillMaxHeight()
                        .semantics { contentDescription = label }) {
                        val layout = if (animated.labelLayout.size.width <= size.width) animated.labelLayout else
                            ellipsisMeasurer.measure(label, animated.labelLayout.layoutInput.style,
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                                constraints = androidx.compose.ui.unit.Constraints(maxWidth = size.width.toInt().coerceAtLeast(1)))
                        val visibleWidth = ((animated.labelWidth.value - LABEL_GAP - LABEL_RIGHT_PADDING + PORTRAIT_INSET)
                            .coerceAtLeast(0f) * density).coerceAtMost(size.width)
                        clipRect(right = visibleWidth) {
                            drawText(layout, color = if (animated.member.failed) ZorkColors.Danger else ZorkColors.Ink,
                                topLeft = androidx.compose.ui.geometry.Offset(0f, (size.height - layout.size.height) * .5f), alpha = animated.reveal)
                        }
                    }
                }
            }) { children, constraints ->
                // Keep the text's constraints constant while the enclosing hit
                // surface follows the visible bubble. One semantic surface owns
                // both portrait and label; decorative siblings cannot occlude it.
                val width = 30.4.dp.roundToPx() + animated.labelLayout.size.width
                val visual = children.single().measure(androidx.compose.ui.unit.Constraints.fixed(width, 22.4.dp.roundToPx()))
                layout(constraints.maxWidth, constraints.maxHeight) {
                    visual.place(12.8.dp.roundToPx(), 12.8.dp.roundToPx())
                }
            }
        } }
    }) { measurables, constraints ->
        val extent = presence.extentPixels(density)
        val placeables = measurables.mapIndexed { index, measurable ->
            val member = presence.members[index]
            val width = ((48f + member.labelWidth.value) * density).roundToInt()
                .coerceIn(1, (constraints.maxWidth - ((member.x.value - 8f) * density).roundToInt()).coerceAtLeast(1))
            measurable.measure(androidx.compose.ui.unit.Constraints.fixed(width, 48.dp.roundToPx()))
        }
        // The text editor expands its short first line to Android's minimum hit
        // height. Leave 8dp below the member cells so those targets cannot overlap.
        layout(constraints.maxWidth, extent + 40.dp.roundToPx()) {
            placeables.forEachIndexed { i, placeable ->
                val member = presence.members[i]
                placeable.place(((member.x.value - 8f) * density).roundToInt(), extent - ((member.lift.value + 24f) * density).roundToInt())
            }
        }
    }
}

@Composable
private fun ComposerPortrait(avatar: String, name: String) {
    val resource = when (avatar) {
        "fox" -> R.drawable.portrait_fox; "panda" -> R.drawable.portrait_panda
        "bear" -> R.drawable.portrait_bear; "bunny" -> R.drawable.portrait_bunny
        "chick" -> R.drawable.portrait_chick; "deer" -> R.drawable.portrait_deer
        "dog" -> R.drawable.portrait_dog; "koala" -> R.drawable.portrait_koala
        "octopus" -> R.drawable.portrait_octopus; "owl" -> R.drawable.portrait_owl
        "penguin" -> R.drawable.portrait_penguin; else -> R.drawable.portrait_cat
    }
    androidx.compose.foundation.Image(androidx.compose.ui.res.painterResource(resource), name, Modifier.size(22.4.dp))
}

// The shared scene advances every parcel and produces the fused path in one
// batch. Android only places text/portraits and submits that path to Canvas.
@Composable
internal fun Modifier.liquidComposer(presence: ComposerMotion): Modifier {
    val node = presence.node
    val factor = androidx.compose.ui.platform.LocalDensity.current.density
    val view = androidx.compose.ui.platform.LocalView.current
    val latest by rememberUpdatedState(presence)
    return onGloballyPositioned { coordinates ->
        val bounds = coordinates.boundsInWindow()
        val top = latest.extentPixels(factor) / factor
        node.update(LiquidTarget(LiquidKind.Compound,
            LiquidPose(width = (coordinates.size.width / factor).coerceAtLeast(2f),
                height = (coordinates.size.height / factor - top).coerceAtLeast(2f), radius = 24f),
            visible = bounds.width > 0f && bounds.height > 0f && bounds.bottom > 0f && bounds.top < view.height), factor)
    }.drawWithContent {
        node.revision
        node.prepareFirstDraw()
        translate(top = latest.extentPixels(factor).toFloat()) {
            drawPath(node.outline, ZorkColors.Paper)
            drawPath(node.border, ZorkColors.FieldBorder)
        }
        drawContent()
    }
}
