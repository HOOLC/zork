package ing.zork.android

import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.FrameMetrics
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp

/** The real conversation and both real message roles, with isolated fixed data. */
class MarkdownStressActivity : ComponentActivity() {
    data class Frame(val uiMs: Double, val totalMs: Double, val vsync: Long)
    lateinit var scroll: LazyListState
    val frames = mutableListOf<Frame>()
    val seen = mutableSetOf<String>()
    var records = 0; private set
    var measuring = false
    var measureStart = 0L
    var coldMaxUiMs = 0.0
    var maxTextViews = 0
    var setupMs = 0.0
    var viewportWidth = 0
    var viewportHeight = 0
    var renderDensity = 1f
    var physical = false
    var deviceDensity = false
    private lateinit var messages: List<ChatMessage>
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState); enableEdgeToEdge()
        physical = intent.getBooleanExtra("physical", false)
        deviceDensity = physical || intent.getBooleanExtra("deviceDensity", false)
        if (!physical) window.insetsController?.hide(android.view.WindowInsets.Type.systemBars())
        renderDensity = if (deviceDensity) resources.displayMetrics.density else 1f
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(android.graphics.Color.WHITE))
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.addOnFrameMetricsAvailableListener({ _, m, _ ->
            val cpu = (m.getMetric(FrameMetrics.ANIMATION_DURATION) + m.getMetric(FrameMetrics.LAYOUT_MEASURE_DURATION) + m.getMetric(FrameMetrics.DRAW_DURATION)) / 1_000_000.0
            if (!measuring) coldMaxUiMs = maxOf(coldMaxUiMs, cpu)
            val vsync = m.getMetric(FrameMetrics.INTENDED_VSYNC_TIMESTAMP)
            if (measuring && vsync >= measureStart) synchronized(frames) { frames.add(Frame(cpu, m.getMetric(FrameMetrics.TOTAL_DURATION) / 1_000_000.0, vsync)) }
        }, Handler(Looper.getMainLooper()))
        val start = System.nanoTime()
        records = intent.getIntExtra("messages", 100000)
        require(records in 100..100000)
        val avatars = listOf("cat", "dog", "owl", "fox", "panda", "penguin", "koala", "rabbit", "bear", "deer", "chick", "octopus")
        messages = List(records - 3) { index ->
            val (kind, body) = markdownStressCases[index % markdownStressCases.size]
            val user = (index / markdownStressCases.size) % 2 == 1
            ChatMessage("stress-$index", if (user) "你" else "小伙伴", if (body.isEmpty()) "" else "消息 $index\n\n" + body.replace("__INDEX__", index.toString()), user,
                avatar = avatars[index % avatars.size], files = if (kind == "text_attachment") listOf(TextAttachmentUi("file-$index", "notes-$index.md", "# 文本附件", "Markdown")) else emptyList())
        }
        val pending = listOf("", "sending", "failed").mapIndexed { index, state ->
            ChatMessage("pending-$index", "你", "队列状态 $index", true, pending = true, deliveryStatus = state)
        }
        setupMs = (System.nanoTime() - start) / 1_000_000.0
        val scale = intent.getFloatExtra("fontScale", 1f)
        setContent { ZorkTheme {
            CompositionLocalProvider(LocalDensity provides Density(renderDensity, scale * if (deviceDensity) resources.configuration.fontScale else 1f)) {
                scroll = rememberLazyListState()
                Box((if (deviceDensity) Modifier.fillMaxSize().safeDrawingPadding() else Modifier.requiredSize(390.dp, 844.dp))
                    .onSizeChanged { viewportWidth = it.width; viewportHeight = it.height }) {
                    ConversationBody(WorkbenchState(conversation = Conversation("markdown-stress", "Markdown 压力验证"),
                        messages = messages, pending = pending, connected = true), WorkbenchActions(), scroll)
                }
            }
        } }
    }
    fun observe() {
        scroll.layoutInfo.visibleItemsInfo.forEach { item ->
            val index = item.key.toString().removePrefix("stress-").toIntOrNull()
            if (index != null && index in messages.indices) {
                seen.add(markdownStressCases[index % markdownStressCases.size].first + if (messages[index].user) ":user" else ":assistant")
            }
        }
        fun count(view: View): Int = when (view) {
            is TextView -> if (view.isShown) 1 else 0
            is ViewGroup -> (0 until view.childCount).sumOf { count(view.getChildAt(it)) }
            else -> 0
        }
        maxTextViews = maxOf(maxTextViews, count(window.decorView))
    }
}

private fun fence(language: String, code: String) = "```$language\n$code\n```"
internal val markdownStressCases = listOf(
    "plain" to "普通文本，保持轻松阅读。",
    "unicode" to "中文 English العربية 日本語 한국어 🐈 café。",
    "emphasis" to "**粗体**、*斜体*、~~删除线~~、***组合样式***。",
    "headings" to "# 标题\n## 二级标题\n### 三级标题\n#### 四级标题\n##### 五级标题\n###### 六级标题",
    "unordered" to "- 第一项\n- 这是一段窄屏上需要换行的列表正文，检查缩进与连续行对齐。",
    "ordered" to "9. 第一项\n10. 第二项\n11. **粗体** 与 `code`",
    "nested" to "1. 父级\n   - 子级\n     - 第三级\n2. 后续项目",
    "tasks_fallback" to "- [x] 已完成\n- [ ] 待办",
    "quote" to "> 引用文字\n>\n> **带样式的引用**\n> > 嵌套引用",
    "table" to markdownFixtures.getValue("table"),
    "rule" to "上方文字\n\n---\n\n下方文字",
    "link" to "[链接](https://example.com/path) 与 <https://example.com>。",
    "reference" to "[参考链接][r]\n\n[r]: https://example.com/docs",
    "inline_code" to "用 `cargo test --locked` 检查，`中文🐈` 保留原文。",
    "rust" to fence("rust", "// 消息 __INDEX__\nlet ready = true;\nfn main() { println!(\"中文🐈\"); }"),
    "kotlin" to fence("kotlin", "val id = __INDEX__\nfun greet(name: String) { println(name) }"),
    "json" to fence("json", "{\"id\": __INDEX__, \"name\": \"中文🐈\", \"ready\": true}"),
    "python" to fence("python", "# 消息 __INDEX__\ndef greet(name):\n    return \"Hello \" + name"),
    "javascript" to fence("js", "const id = __INDEX__;\nasync function run() { return \"hello\"; }"),
    "typescript" to fence("ts", "interface Item { id: number }\nconst item: Item = { id: __INDEX__ };"),
    "java" to fence("java", "class Example {\n    int id = __INDEX__;\n    String name = \"Hello\";\n}"),
    "shell" to fence("sh", "# 消息 __INDEX__\necho \"hello\"\nif test -f file; then\n  echo ready\nfi"),
    "sql" to fence("sql", "SELECT name FROM messages WHERE id = __INDEX__;"),
    "unknown_code" to fence("unknown", "opaque __INDEX__ <value> 中文🐈"),
    "untagged_code" to fence("", "plain __INDEX__\n中文🐈"),
    "long_code" to fence("kotlin", (0 until 96).joinToString("\n") { "println(\"行 $it · __INDEX__\")" }),
    "long_prose" to "中文 English 的长段落，检查换行与选择。".repeat(100),
    "long_token" to "长内容：" + "x".repeat(2048),
    "text_attachment" to "正文附带一个可预览的文本文件。",
    "image_fallback" to "![图片说明](https://example.com/image.png)",
    "html_fallback" to "<div>安全显示 HTML 原文</div>",
    "math_fallback" to "数学表达式保持文本：\\(a^2+b^2=c^2\\)。",
    "mermaid_fallback" to fence("mermaid", "graph TD; A-->B;"),
    "incomplete" to "**还未结束的强调\n\n```rust\nlet value = \"仍在输入",
    "empty" to ""
)
