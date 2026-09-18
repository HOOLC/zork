package ing.zork.android

import android.graphics.Rect
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp

/** Production Markdown, without client storage or a network connection. */
class MarkdownPreviewActivity : ComponentActivity() {
    var contentBounds = Rect()
    var lastQuote = ""
    var captureLinks = false
    val requestedLinks = mutableListOf<String>()
    override fun startActivity(intent: android.content.Intent) {
        if (captureLinks && intent.action == android.content.Intent.ACTION_VIEW) {
            requestedLinks.add(intent.dataString.orEmpty()); return
        }
        super.startActivity(intent)
    }
    var content by mutableStateOf("")
    var width by mutableStateOf(390)
    var fontScale by mutableStateOf(1f)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(android.graphics.Color.WHITE))
        window.insetsController?.hide(android.view.WindowInsets.Type.systemBars())
        window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        content = markdownFixtures.getValue(intent.getStringExtra("case") ?: "typography")
        width = intent.getIntExtra("width", 390)
        fontScale = intent.getFloatExtra("fontScale", 1f)
        setContent { ZorkTheme {
            CompositionLocalProvider(LocalDensity provides Density(1f, fontScale)) {
                Column(Modifier.requiredSize(width.dp, 844.dp).onGloballyPositioned {
                    val b = it.boundsInWindow()
                    contentBounds = Rect(b.left.toInt(), b.top.toInt(), b.right.toInt(), b.bottom.toInt())
                }.verticalScroll(rememberScrollState()).padding(18.dp)) {
                    Markdown(content, Modifier.fillMaxWidth()) { lastQuote = it }
                }
            }
        } }
    }
}

internal val markdownFixtures = linkedMapOf(
    "typography" to """
        # 消息排版
        正文包含 **粗体**、*斜体*、~~删除线~~ 和 `inline_code`，中文 English 🐈。

        ## 列表与引用
        - 第一项
        - 这是一条在手机窄屏上换行的较长列表内容，续行应当与正文对齐。
          - 嵌套条目

        9. 有序列表
        10. 序号使用等宽字体，正文仍保留原来的字体。

        > 引用文字，保持清晰的层次和紧凑的间距。

        [打开链接](https://example.com/markdown) 与 `cargo test --locked`。
    """.trimIndent(),
    "code" to """
        ## 代码与说明
        代码前的正常段落，不应被代码样式影响。

        ```kotlin
        // 中文注释 🐈
        val message = "Hello, Zork"
        fun greet(name: String) {
            println("Hello: " + name)
        }
        ```

        ```rust
        let ready = true;
        fn main() {
            println!("中文 ✓");
        }
        ```

        代码后的段落和 `行内代码`。
    """.trimIndent(),
    "table" to """
        ### 表格
        | 类型 | 状态 | 说明 |
        | :--- | :---: | ---: |
        | 文本 | **就绪** | 中文与 English |
        | 代码 | `ready` | 保留代码字体 |
        | 链接 | [查看](https://example.com) | 可打开 |

        1. **样式组合** 与 `code`
        2. 参考式[链接][docs]

        [docs]: https://example.com/docs

        ---
        分隔线下方的正文。
    """.trimIndent()
)
