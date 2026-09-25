package ing.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import org.json.JSONArray
import org.json.JSONObject
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/**
 * Visual fixture of the multi-agent transcript and chat list (the approved
 * prototype's thread). Production composables with raw observed-row JSON, so
 * core `message_presentation` does the grouping, times and quotes. Extras:
 * `screen` = chat | list | drafts, `theme` = light | dark.
 */
class MultiAgentPreviewActivity : ComponentActivity() {
    var theme by mutableStateOf("light")
    private var comments by mutableStateOf(emptyList<DraftCommentUi>())

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); configureZorkSystemBars()
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        theme = intent.getStringExtra("theme") ?: "light"
        val screen = intent.getStringExtra("screen") ?: "chat"
        val rows = multiAgentFixtureRows(System.currentTimeMillis())
        if (screen == "drafts") comments = listOf(
            DraftCommentUi("d1", "chat-login", "criteria", "Planner", "planner", "键盘可以走完全部流程", "菜单里的 Tab 顺序也要一起测。"),
            DraftCommentUi("d2", "chat-login", "spacing", "审阅助手", "review", "触控区重叠", ""))
        setContent {
            ZorkTheme(theme) {
                Box(Modifier.fillMaxSize().background(ZorkColors.Canvas).safeDrawingPadding()) {
                    val now = System.currentTimeMillis()
                    val home = HomeNavigation(multiAgentFixtureChats(now), loaded = true)
                    val peers = listOf(Peer("peer-a", "A", "", machine = "zuozijiandeMacBook-Air"), Peer("peer-b", "B", "", machine = "zuozijians-Mac-Studio"))
                    val state = WorkbenchState(peers = peers, activePeer = peers[0],
                        conversation = if (screen == "list") null else Conversation("chat-login", "登录页改版"),
                        messages = rows, connected = true, older = true, comments = comments, home = home,
                        sessions = listOf(JSONObject().put("chat_id", "chat-login").put("title", "登录页改版")
                            .put("avatar", JSONObject().put("agents", JSONArray()
                                .put(JSONObject().put("agent_id", "planner").put("maker", "openai").put("tint", 0).put("initial", "P"))
                                .put(JSONObject().put("agent_id", "builder").put("maker", "deepseek").put("tint", 3).put("initial", "B"))
                                .put(JSONObject().put("agent_id", "review").put("maker", "anthropic").put("tint", 2).put("initial", "审"))))))
                    val actions = WorkbenchActions(
                        comment = { row, quote -> comments = comments + DraftCommentUi("q${comments.size}", "chat-login", row.id, row.author,
                            row.authorAgentId.ifBlank { null }, quote, "") },
                        editComment = { edited -> comments = comments.map { if (it.id == edited.id) edited else it } },
                        removeComment = { id -> comments = comments.filterNot { it.id == id } })
                    Workbench(state, actions)
                }
            }
        }
    }
}

private val iso = DateTimeFormatter.ISO_OFFSET_DATE_TIME.withZone(ZoneId.systemDefault())
private const val M = 60_000L
private const val H = 60 * M

private fun row(id: String, author: String, ago: Long, now: Long, text: String, reply: String? = null,
    quote: String? = null, kind: String? = null): JSONObject {
    val agents = mapOf("planner" to ("Planner" to "gpt-6-astra"), "builder" to ("Builder" to "deepseek-flash"),
        "review" to ("审阅助手" to "claude-sonnet-5"))
    val devices = mapOf("planner" to "peer-a", "builder" to "peer-b", "review" to "peer-b")
    val json = JSONObject().put("type", "message").put("id", id).put("created_at", iso.format(Instant.ofEpochMilli(now - ago)))
        .put("role", if (author == "user") "user" else "assistant").put("content", text)
        // Projection extras the conversation observation adds (core ignores them).
        .put("file_views", JSONArray()).put("files", JSONArray()).put("source_epoch", "fixture")
    agents[author]?.let { (name, model) ->
        json.put("author_agent_id", author).put("author_name", name).put("model", model).put("device", devices[author])
    }
    reply?.let { json.put("reply_to", it) }
    quote?.let { json.put("quote", it) }
    kind?.let { json.put("quote_kind", it) }
    return json
}

private fun commentBatch(pairs: List<Triple<String, String, String>>, extra: String): String {
    val authors = mapOf("criteria" to ("Planner" to "planner"), "spacing" to ("审阅助手" to "review"))
    val comments = JSONArray()
    pairs.forEachIndexed { at, (source, quote, reply) ->
        val (name, agent) = authors.getValue(source)
        comments.put(JSONObject().put("id", "c$at").put("comment", reply).put("source", JSONObject().put("session_id", "chat-login")
            .put("message_id", source).put("author", name).put("author_agent_id", agent).put("quote", quote)))
    }
    return "<zork-message-comments version=\"1\">\n" + JSONObject().put("comments", comments).put("text", extra) + "\n</zork-message-comments>"
}

internal fun multiAgentFixtureRows(now: Long): List<ChatMessage> = listOf(
    row("u1", "user", 26 * H, now, "@Planner 把登录页改版拆成任务，审阅助手和 Builder 一起跟进。"),
    row("p1", "planner", 26 * H - 4 * M, now, "好的，拆成三步：\n1. 梳理现有登录流程和埋点\n2. 出新版布局与文案\n3. 实现并补测试\n我先做第 1 步，Builder 可以先搭页面骨架。"),
    row("criteria", "planner", 26 * H - 5 * M, now, "验收标准：首屏只保留账号、密码和一个主按钮；错误提示贴在对应字段下方，不弹窗；键盘可以走完全部流程，焦点顺序与视觉顺序一致；深色主题逐项核对对比度。"),
    row("b1", "builder", 25 * H, now, "骨架已推到 `ud/login-refresh`，表单和按钮先复用现有组件。"),
    row("r1", "review", 24 * H, now, "看过了：密码框缺少显示/隐藏切换，错误提示的对比度也不够。建议直接用 `field_with_error`，焦点和错误态它都处理好了。", "b1", "表单和按钮先复用现有组件", "excerpt"),
    row("u2", "user", 23 * H, now, "按审阅意见改，改完叫我看。"),
    row("b2", "builder", 5 * H, now, "已改：加了显示切换，错误提示换成 `field_with_error`。截图放在 Chat 文件里了。", "r1", "密码框缺少显示/隐藏切换，错误提示对比度不够", "summary"),
    row("b3", "builder", 5 * H - 2 * M, now, "顺便把主按钮换成炭墨主操作样式，发送中的状态沿用按钮内的加载反馈。"),
    row("spacing", "review", 3 * H, now, "布局看过了。「忘记密码」链接离主按钮太近，触控区重叠，建议下移 8 px。"),
    row("u3", "user", 2 * H + 30 * M, now, "同意，顺便把第三方登录收进「更多方式」。"),
    row("b4", "builder", 2 * H, now, "已调整间距，第三方登录收进了「更多方式」菜单，默认收起。"),
    row("p2", "planner", 90 * M, now, "埋点同步更新：登录方式的点击改为在菜单展开后上报，避免把展开误算成选择。", "old2", "上周定的埋点口径：点击和展开分开统计", "summary"),
    row("r2", "review", 60 * M, now, "菜单的键盘操作正常，Esc 能关闭并把焦点还给触发按钮。"),
    row("b5", "builder", 40 * M, now, "深色主题的截图已更新到 Chat 文件。"),
    row("usercomments", "user", 20 * M, now, commentBatch(listOf(
        Triple("criteria", "错误提示贴在对应字段下方，不弹窗", "这条保留，但错误文案要写清楚怎么改，不要只说「格式错误」。"),
        Triple("spacing", "建议下移 8 px", "8 px 还是有点挤，试试 12 px。")), "其他都可以，按这个继续。")),
    row("shortok", "review", 16 * M, now, "好。"),
    row("b6", "builder", 14 * M, now, "收到，两处都按你的意见改：错误文案改成具体的修改建议，链接下移 12 px。", "usercomments", "错误文案要具体；链接下移 12 px", "summary"),
    row("u4", "user", 8 * M, now, "文案这块交给 Planner 再过一遍。"),
    row("p3", "planner", 7 * M, now, "我来，顺便把审阅的「好」当作文案方向已确认。", "shortok"),
    row("r3", "review", 3 * M, now, "按最初的验收标准复查：焦点顺序已经正确；深色主题下错误提示的对比度只有 3.9:1，还差一点。", "criteria", "深色主题逐项核对对比度", "excerpt"),
    row("b7", "builder", 40_000, now, "对比度调到 4.8:1 了，其余不变。", "r3", "对比度只有 3.9:1", "excerpt"),
    row("notesreq", "user", 30_000, now, "把这轮改动整理成发布说明，按页面分开写。"),
    row("b8", "builder", 25_000, now, "发布说明：\n1. 登录页：首屏只保留账号、密码和主按钮；第三方登录收进「更多方式」菜单，默认收起。\n2. 表单：密码框加显示/隐藏切换；错误提示贴在字段下方，文案给出具体修改建议。\n3. 间距：「忘记密码」链接下移 12 px，不再和主按钮的触控区重叠。"),
    row("b9", "builder", 20_000, now, "以上是按页面整理好的发布说明，需要我直接发到发布频道吗？", "notesreq"),
).map(::parseChatMessage)

internal fun multiAgentFixtureChats(now: Long): List<HomeChat> {
    fun avatar(vararg agents: Pair<String, Int>, more: Long = 0) = ChatAvatarUi(agents.map { (maker, tint) ->
        AgentAvatarUi(maker, maker.takeIf { it != "none" }, tint, maker.first().uppercase()) }, more)
    fun chat(id: String, title: String, peer: String, local: Boolean, ago: Long, avatar: ChatAvatarUi, unread: Boolean = false) =
        HomeChat(peer, peer, id, title, "", "", unread, false, false, null, 0, now - ago, "today", true, false,
            avatar, deviceLocal = local)
    return listOf(
        chat("chat-login", "登录页改版", "A", false, 20_000, avatar("openai" to 0, "deepseek" to 3, "anthropic" to 2)),
        chat("chat-notes", "发布说明", "B", true, 10 * M, avatar("deepseek" to 3), unread = true),
        chat("chat-metrics", "数据看板迁移到新的指标体系，按页面拆分并补齐埋点", "C", false, 70 * M,
            avatar("qwen" to 1, "moonshot" to 4, "zhipu" to 0, more = 1)),
        chat("chat-weekly", "周报", "B", true, 26 * H, avatar("openai" to 0, "qwen" to 1)),
        chat("chat-plain", "没有 Agent 的旧对话", "A", false, 3 * 24 * H, ChatAvatarUi()),
    )
}
