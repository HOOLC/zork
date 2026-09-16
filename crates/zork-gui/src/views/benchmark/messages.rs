//! Deterministic coverage of every currently supported transcript presentation.
use crate::{
    api::{MessageMetadata, Role},
    comments::{CommentSource, DraftComment, TextAttachment},
    transcript::TranscriptLine,
};

pub const KINDS: &[&str] = &[
    "plain",
    "unicode",
    "emphasis",
    "headings",
    "unordered",
    "ordered",
    "tasks",
    "nested_lists",
    "quote",
    "table",
    "rule",
    "links",
    "inline_code",
    "rust",
    "json",
    "python",
    "javascript",
    "shell",
    "unknown_code",
    "long_code",
    "long_prose",
    "long_token",
    "text_attachment",
    "comment_batch",
    "image_reference",
    "file_link",
    "html_fallback",
    "math_mermaid_fallback",
    "incomplete_markdown",
    "legacy_metadata",
    "empty",
    "interaction_create",
    "interaction_input",
    "interaction_completed",
    "interaction_long",
];

pub fn kinds() -> &'static [&'static str] {
    // Retain an identical input for before/after measurements of the original
    // renderer; the default pressure gate always includes all current kinds.
    if std::env::var_os("ZORK_BENCH_PRE_INTERACTION_FIXTURE").is_some() {
        &KINDS[..KINDS.len() - 4]
    } else {
        KINDS
    }
}

pub fn line(index: usize) -> TranscriptLine {
    let kinds = kinds();
    let kind = index % kinds.len();
    let content = match kinds[kind] {
        "plain" => format!("消息 {index}：读取项目配置并记录结果。"),
        "unicode" => {
            format!("消息 {index}：中文 English العربية 日本語 한국어 👨‍👩‍👧‍👦 🐈 e\u{301} café。")
        }
        "emphasis" => format!("消息 {index}：**粗体**、*斜体*、~~删除~~、***组合强调***。"),
        "headings" => format!(
            "# 标题 {index}\n\n## 二级标题\n\n### 三级标题\n\n#### 四级标题\n\n##### 五级标题\n\n###### 六级标题"
        ),
        "unordered" => format!(
            "消息 {index}\n\n- 第一项\n- 第二项有一段会在窄窗口换行的中文说明，验证续行仍与正文对齐。\n- 第三项"
        ),
        "ordered" => format!(
            "消息 {index}\n\n1. 读取\n2. 验证\n3. 执行\n4. 保存\n5. 检查\n6. 重试\n7. 恢复\n8. 比较\n9. 提交\n10. 完成\n11. 归档"
        ),
        "tasks" => format!("消息 {index}\n\n- [x] 已完成\n- [ ] 待处理\n- [x] 复查通过"),
        "nested_lists" => format!(
            "消息 {index}\n\n1. 第一层\n   - 第二层\n     - 第三层\n   - 第二项\n2. 后续段落\n\n   列表中的第二个段落。"
        ),
        "quote" => {
            format!("> 引用 {index}\n>\n> **强调**与 `code`\n>\n>> 嵌套引用\n\n引用后的正文。")
        }
        "table" => format!(
            "| 项目 {index} | 状态 | 数量 |\n| :--- | :---: | ---: |\n| **检查** | 完成 | 12 |\n| `配置` | 待复核 🐈 | 128 |\n| [文档](https://example.com/{index}) | 可用 | 3 |"
        ),
        "rule" => format!("段落 {index}\n\n---\n\n分隔线后的内容。"),
        "links" => format!(
            "消息 {index}：[项目文档](https://example.com/docs/{index})、<https://example.com>、[引用链接][ref]。\n\n[ref]: https://example.com/reference"
        ),
        "inline_code" => format!(
            "消息 {index}：运行 `cargo test --locked`，检查 `REPOS_ROOT` 和 `fn check_{index}()`。"
        ),
        "rust" => format!(
            "```rust\nfn check_{index}() -> Result<&'static str, String> {{\n    // 保留缩进与颜色\n    Ok(\"中文 🐈\")\n}}\n```"
        ),
        "json" => format!(
            "```json\n{{\"task\":{index},\"enabled\":true,\"names\":[\"中文\",null],\"ratio\":0.25}}\n```"
        ),
        "python" => format!(
            "```python\ndef check_{index}(items):\n    \"\"\"A multiline string\n    with 中文.\n    \"\"\"\n    return [x for x in items if x > {index}]\n```"
        ),
        "javascript" => format!(
            "```javascript\nconst task{index} = {{ label: \"中文\", ready: true }};\nasync function run() {{\n  return await Promise.resolve(task{index});\n}}\n```"
        ),
        "shell" => format!(
            "```bash\n# 任务 {index}\nfor name in one two; do\n  printf '%s\\n' \"$name\"\ndone\n```"
        ),
        "unknown_code" => format!(
            "```unknown-language\n    task {index}\n    保留未知语法的缩进、换行和符号 <>&。\n```"
        ),
        "long_code" => format!(
            "```rust\n{}\n```",
            (0..96)
                .map(|n| format!("let value_{index}_{n} = \"line {n}: 中文\";"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        "long_prose" => format!(
            "长消息 {index}\n\n{}",
            "这是长段落，包含中文 English 和标点，用于检查换行、布局及选择。".repeat(96)
        ),
        "long_token" => format!(
            "消息 {index}\n\n```text\n{}\n```",
            format!("token_{index}_").repeat(256)
        ),
        "text_attachment" => crate::comments::compose_document(
            &format!("请检查附件 {index}"),
            &[],
            &[TextAttachment {
                id: format!("text-{index}"),
                name: format!("config-{index}.txt"),
                content: "第一行\n第二行 `inline`\n第三行 🐈".repeat(8),
            }],
        ),
        "comment_batch" => crate::comments::compose(
            &format!("批注说明 {index}"),
            &[DraftComment {
                id: format!("comment-{index}"),
                source: CommentSource {
                    session_id: "render-fixture".into(),
                    message_id: Some(format!("stress-{}", index.saturating_sub(1))),
                    author: Some("产品 Leader".into()),
                    author_agent_id: Some("leader".into()),
                    quote: "需要确认的原文\n包含 `code` 与中文。".into(),
                },
                comment: "请补充说明，并保留这段原文。".into(),
            }],
        ),
        "image_reference" => format!(
            "图片消息 {index}\n\n![图像替代文字](https://example.com/image-{index}.png)\n\n图片 Markdown 当前按链接回退，实际图片由附件预览呈现。"
        ),
        "file_link" => zork_client_core::files::compose(
            &format!("文件消息 {index}"),
            &[zork_client_core::files::FileRef {
                id: format!("file-fixture-{index}"),
                name: format!("report-{index}.pdf"),
                byte_len: 4096,
                content_root: "0".repeat(64),
            }],
        ),
        "html_fallback" => format!("<section data-task=\"{index}\">HTML 以可读文本呈现</section>"),
        "math_mermaid_fallback" => {
            format!("公式 $x_{{{index}}}^2$\n\n```mermaid\ngraph LR\n A-->B\n```")
        }
        "incomplete_markdown" => {
            format!("消息 {index}：**尚未闭合的强调\n\n```rust\nlet partial = \"还在输入")
        }
        "legacy_metadata" => format!("历史消息 {index}：没有发送者元数据。"),
        "empty" => String::new(),
        "interaction_create" | "interaction_input" | "interaction_completed" | "interaction_long" => format!("交互请求 {index}：请确认下面的信息。"),
        _ => unreachable!(),
    };
    let role = if (index / kinds.len()) % 3 == 0 {
        Role::User
    } else {
        Role::Assistant
    };
    let mut metadata = if kinds[kind] == "legacy_metadata" {
        MessageMetadata::default()
    } else {
        MessageMetadata {
            id: Some(format!("stress-{index}")),
            created_at: Some("2026-09-07T08:00:00Z".into()),
            author_agent_id: Some(format!("leader-{}", index % 4)),
            author_name: Some(format!("伙伴 {}", index % 4)),
            author_avatar: Some(zork_ui::controls::AGENT_AVATARS[index % 12].0.into()),
            device: Some(format!("fixture-device-{}", index % 3)),
            ..Default::default()
        }
    };
    if let Some(state) = kinds[kind].strip_prefix("interaction_") {
        let mut card = zork_client_core::interactions::preview::Preview::new(state).card();
        card.message_id = format!("stress-{index}");
        metadata.interaction_view = Some(Box::new(card));
    }
    TranscriptLine::Message {
        role,
        content,
        metadata,
    }
}

pub fn coverage(count: usize) -> serde_json::Value {
    let mut counts = serde_json::Map::new();
    let kinds = kinds();
    for (index, kind) in kinds.iter().enumerate() {
        counts.insert(
            (*kind).into(),
            (count / kinds.len() + usize::from(index < count % kinds.len())).into(),
        );
    }
    serde_json::json!({"message_kinds":counts,"roles":["user","assistant"],"agent_avatars":12,
        "image_markdown":"link fallback; raster image preview is tested separately",
        "separate_surfaces":["file artifact card","PNG image preview","text artifact preview","participant activity","delivery state"]})
}
