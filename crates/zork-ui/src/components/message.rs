//! Small local Markdown message renderer adapted from the block/inline model
//! used by Longbridge GPUI Component's `TextView`.
//!
//! The upstream crate cannot be linked because it owns a different `gpui`
//! package. This module intentionally keeps only the coding-message subset:
//! GFM parsing, styled inline runs, lists, quotes, tables, rules, code blocks,
//! and explicit readable fallbacks.

use super::selection::SelectionContext;
use gpui::StatefulInteractiveElement;
use gpui::{
    div, prelude::FluentBuilder as _, px, rgb, AnyElement, AppContext as _, FontStyle, FontWeight,
    HighlightStyle, InteractiveElement, InteractiveText, IntoElement, ParentElement, SharedString,
    StrikethroughStyle, Styled, StyledText, UnderlineStyle,
};
use markdown::{
    mdast::{self, Node},
    ParseOptions,
};

#[path = "message_code.rs"]
mod code;
#[path = "message_text.rs"]
mod text_cache;

#[allow(non_snake_case)]
fn TEXT() -> u32 {
    crate::design::ZORK_UI.palette.text
}
#[allow(non_snake_case)]
fn MUTED() -> u32 {
    crate::design::ZORK_UI.palette.muted
}
#[allow(non_snake_case)]
fn BORDER() -> u32 {
    crate::design::ZORK_UI.palette.border
}
#[allow(non_snake_case)]
fn CODE_FILL() -> u32 {
    crate::design::ZORK_UI.palette.sidebar
}
#[allow(non_snake_case)]
fn LINK() -> u32 {
    crate::design::ZORK_UI.palette.accent
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub strong: bool,
    pub emphasis: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineSegment {
    pub text: String,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, Default)]
pub struct InlineContent {
    segments: Vec<InlineSegment>,
    layout_cache: text_cache::TextCache,
}
impl PartialEq for InlineContent {
    fn eq(&self, other: &Self) -> bool {
        self.segments == other.segments
    }
}
impl Eq for InlineContent {}

impl InlineContent {
    pub fn segments(&self) -> &[InlineSegment] {
        &self.segments
    }

    pub fn text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    fn push(&mut self, text: impl Into<String>, style: InlineStyle) {
        let text = text.into();
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.segments.last_mut() {
            if last.style == style {
                last.text.push_str(&text);
                return;
            }
        }
        self.segments.push(InlineSegment { text, style });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub blocks: Vec<MessageBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageBlock {
    Paragraph(InlineContent),
    Heading {
        level: u8,
        content: InlineContent,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    List {
        ordered: bool,
        items: Vec<ListItem>,
    },
    BlockQuote(Vec<MessageBlock>),
    Rule,
    Table {
        rows: Vec<Vec<InlineContent>>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct MessageDocument {
    truncated: bool,
    blocks: Vec<MessageBlock>,
    plain_text_cache: std::cell::OnceCell<SharedString>,
    code_cache: Vec<std::cell::OnceCell<std::rc::Rc<code::Presentation>>>,
    width_cache: std::cell::RefCell<Option<(gpui::Font, f32, f32, f32)>>,
}
// Keep the most common pathological blocks (single huge paragraphs/code logs)
// bounded in the full reader as well, preserving text and inline styles.
fn reader_blocks(block: &MessageBlock) -> Vec<MessageBlock> {
    fn chunks(text: &str) -> Vec<&str> {
        let mut start = 0;
        let mut count = 0;
        let mut lines = 0;
        let mut chunks = Vec::new();
        for (offset, ch) in text.char_indices() {
            if count >= 4096 || lines >= 64 {
                chunks.push(&text[start..offset]);
                start = offset;
                count = 0;
                lines = 0;
            }
            count += 1;
            if ch == '\n' {
                lines += 1;
            }
        }
        if start < text.len() || chunks.is_empty() {
            chunks.push(&text[start..]);
        }
        chunks
    }
    match block {
        MessageBlock::CodeBlock { language, code } => chunks(code)
            .into_iter()
            .map(|code| MessageBlock::CodeBlock {
                language: language.clone(),
                code: code.into(),
            })
            .collect(),
        MessageBlock::Paragraph(content) => {
            let mut result = Vec::new();
            let mut current = InlineContent::default();
            let mut count = 0;
            for segment in &content.segments {
                for chunk in chunks(&segment.text) {
                    if count + chunk.len() > 8192 && !current.segments.is_empty() {
                        result.push(MessageBlock::Paragraph(std::mem::take(&mut current)));
                        count = 0;
                    }
                    current.push(chunk, segment.style.clone());
                    count += chunk.len();
                }
            }
            if !current.segments.is_empty() {
                result.push(MessageBlock::Paragraph(current));
            }
            result
        }
        MessageBlock::BlockQuote(blocks) => blocks
            .iter()
            .flat_map(reader_blocks)
            .map(|block| MessageBlock::BlockQuote(vec![block]))
            .collect(),
        MessageBlock::Table { rows } if rows.len() > 24 => rows[1..]
            .chunks(23)
            .map(|chunk| MessageBlock::Table {
                rows: std::iter::once(rows[0].clone())
                    .chain(chunk.iter().cloned())
                    .collect(),
            })
            .collect(),
        _ => vec![block.clone()],
    }
}

impl PartialEq for MessageDocument {
    fn eq(&self, other: &Self) -> bool {
        self.blocks == other.blocks
    }
}
impl Eq for MessageDocument {}

impl MessageDocument {
    /// Preserve user-authored text literally, including Markdown syntax and whitespace.
    pub fn plain(source: &str) -> Self {
        Self {
            truncated: false,
            blocks: vec![MessageBlock::Paragraph(InlineContent {
                segments: vec![InlineSegment {
                    text: source.to_owned(),
                    style: InlineStyle::default(),
                }],
                ..Default::default()
            })],
            plain_text_cache: std::cell::OnceCell::from(SharedString::from(source.to_owned())),
            code_cache: Vec::new(),
            width_cache: Default::default(),
        }
    }

    pub fn parse(source: &str) -> Self {
        let blocks = match markdown::to_mdast(source, &ParseOptions::gfm()) {
            Ok(mut root) => {
                let mut definitions = std::collections::HashMap::new();
                collect_definitions(&root, &mut definitions);
                if !definitions.is_empty() {
                    resolve_references(&mut root, &definitions);
                }
                blocks_from_node(root)
            }
            Err(_) => vec![MessageBlock::Paragraph(InlineContent {
                segments: vec![InlineSegment {
                    text: source.to_owned(),
                    style: InlineStyle::default(),
                }],
                ..Default::default()
            })],
        };
        let mut code_cache = Vec::new();
        collect_code_cache(&blocks, &mut code_cache);
        Self {
            truncated: false,
            blocks,
            code_cache,
            plain_text_cache: Default::default(),
            width_cache: Default::default(),
        }
    }

    pub fn with_truncated(mut self, truncated: bool) -> Self {
        self.truncated = truncated;
        self
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Full readers virtualize top-level blocks; code caches remain scoped to
    /// each section and are initialized only when that section is visible.
    pub fn reader_sections(&self) -> Vec<Self> {
        self.blocks
            .iter()
            .flat_map(reader_blocks)
            .map(|block| {
                let blocks = vec![block];
                let mut code_cache = Vec::new();
                collect_code_cache(&blocks, &mut code_cache);
                Self {
                    blocks,
                    code_cache,
                    truncated: false,
                    plain_text_cache: Default::default(),
                    width_cache: Default::default(),
                }
            })
            .collect()
    }

    pub fn blocks(&self) -> &[MessageBlock] {
        &self.blocks
    }

    pub fn plain_text(&self) -> String {
        self.shared_plain_text().to_string()
    }

    /// Bubble sizing only needs to know whether a line reaches its width cap.
    /// Keep that result with the document so offscreen revisits avoid reshaping.
    pub fn bounded_text_width(
        &self,
        font: gpui::Font,
        font_size: f32,
        cap: f32,
        window: &gpui::Window,
    ) -> f32 {
        if let Some((cached_font, size, width_cap, width)) = self.width_cache.borrow().as_ref() {
            if *cached_font == font && *size == font_size && *width_cap == cap {
                return *width;
            }
        }
        let width = bounded_width(&self.shared_plain_text(), cap, |text| {
            let run = gpui::TextRun {
                len: text.len(),
                font: font.clone(),
                color: rgb(TEXT()).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.to_owned().into(), px(font_size), &[run], None)
                .width
                .as_f32()
        });
        *self.width_cache.borrow_mut() = Some((font, font_size, cap, width));
        width
    }

    /// Selection and sizing share immutable message text across scroll frames.
    pub fn shared_plain_text(&self) -> SharedString {
        self.plain_text_cache
            .get_or_init(|| {
                let mut output = String::new();
                append_plain_blocks(&self.blocks, &mut output);
                while output.ends_with('\n') {
                    output.pop();
                }
                output.into()
            })
            .clone()
    }
}

fn bounded_width(text: &str, cap: f32, mut measure: impl FnMut(&str) -> f32) -> f32 {
    let mut widest = 0_f32;
    for line in text.lines() {
        let mut limit = 128;
        loop {
            let end = line
                .char_indices()
                .nth(limit)
                .map_or(line.len(), |(offset, _)| offset);
            let width = measure(&line[..end]);
            if width >= cap {
                return cap;
            }
            if end == line.len() {
                widest = widest.max(width);
                break;
            }
            limit *= 2;
        }
    }
    widest
}

fn collect_code_cache(
    blocks: &[MessageBlock],
    cache: &mut Vec<std::cell::OnceCell<std::rc::Rc<code::Presentation>>>,
) {
    for block in blocks {
        match block {
            MessageBlock::CodeBlock { .. } => cache.push(Default::default()),
            MessageBlock::List { items, .. } => {
                for item in items {
                    collect_code_cache(&item.blocks, cache);
                }
            }
            MessageBlock::BlockQuote(blocks) => collect_code_cache(blocks, cache),
            _ => {}
        }
    }
}

fn append_plain_blocks(blocks: &[MessageBlock], output: &mut String) {
    for (index, block) in blocks.iter().enumerate() {
        if index > 0 && !output.ends_with('\n') {
            output.push('\n');
        }
        match block {
            MessageBlock::Paragraph(content) | MessageBlock::Heading { content, .. } => {
                output.push_str(&content.text());
            }
            MessageBlock::CodeBlock { code, .. } => output.push_str(code),
            MessageBlock::List { items, .. } => {
                for (item_index, item) in items.iter().enumerate() {
                    if item_index > 0 && !output.ends_with('\n') {
                        output.push('\n');
                    }
                    append_plain_blocks(&item.blocks, output);
                }
            }
            MessageBlock::BlockQuote(children) => append_plain_blocks(children, output),
            MessageBlock::Rule => output.push_str("---"),
            MessageBlock::Table { rows } => {
                for (row_index, row) in rows.iter().enumerate() {
                    if row_index > 0 {
                        output.push('\n');
                    }
                    output.push_str(
                        &row.iter()
                            .map(InlineContent::text)
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                }
            }
        }
    }
}

type Definitions = std::collections::HashMap<String, (String, Option<String>)>;
fn reference_key(identifier: &str) -> String {
    identifier
        .split(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
        .to_lowercase()
}
fn collect_definitions(node: &Node, definitions: &mut Definitions) {
    if let Node::Definition(definition) = node {
        definitions
            .entry(reference_key(&definition.identifier))
            .or_insert_with(|| (definition.url.clone(), definition.title.clone()));
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_definitions(child, definitions);
        }
    }
}
fn resolve_references(node: &mut Node, definitions: &Definitions) {
    match node {
        Node::LinkReference(link) => {
            if let Some((url, title)) = definitions.get(&reference_key(&link.identifier)) {
                *node = Node::Link(mdast::Link {
                    children: std::mem::take(&mut link.children),
                    position: link.position.take(),
                    url: url.clone(),
                    title: title.clone(),
                });
            }
        }
        Node::ImageReference(image) => {
            if let Some((url, title)) = definitions.get(&reference_key(&image.identifier)) {
                *node = Node::Image(mdast::Image {
                    alt: std::mem::take(&mut image.alt),
                    position: image.position.take(),
                    url: url.clone(),
                    title: title.clone(),
                });
            }
        }
        _ => {}
    }
    if let Some(children) = node.children_mut() {
        for child in children {
            resolve_references(child, definitions);
        }
    }
}

fn blocks_from_node(node: Node) -> Vec<MessageBlock> {
    match node {
        Node::Root(root) => root
            .children
            .into_iter()
            .flat_map(blocks_from_node)
            .collect(),
        Node::Paragraph(paragraph) => {
            vec![MessageBlock::Paragraph(inline_content(&paragraph.children))]
        }
        Node::Heading(heading) => vec![MessageBlock::Heading {
            level: heading.depth,
            content: inline_content(&heading.children),
        }],
        Node::Code(code) => vec![MessageBlock::CodeBlock {
            language: code.lang,
            code: code.value,
        }],
        Node::Math(math) => vec![MessageBlock::CodeBlock {
            language: None,
            code: math.value,
        }],
        Node::Blockquote(quote) => vec![MessageBlock::BlockQuote(
            quote
                .children
                .into_iter()
                .flat_map(blocks_from_node)
                .collect(),
        )],
        Node::List(list) => vec![MessageBlock::List {
            ordered: list.ordered,
            items: list
                .children
                .into_iter()
                .filter_map(|item| match item {
                    Node::ListItem(item) => Some(ListItem {
                        checked: item.checked,
                        blocks: item
                            .children
                            .into_iter()
                            .flat_map(blocks_from_node)
                            .collect(),
                    }),
                    _ => None,
                })
                .collect(),
        }],
        Node::ThematicBreak(_) => vec![MessageBlock::Rule],
        Node::Table(table) => vec![MessageBlock::Table {
            rows: table
                .children
                .into_iter()
                .filter_map(|row| match row {
                    Node::TableRow(row) => Some(
                        row.children
                            .into_iter()
                            .filter_map(|cell| match cell {
                                Node::TableCell(cell) => Some(inline_content(&cell.children)),
                                _ => None,
                            })
                            .collect(),
                    ),
                    _ => None,
                })
                .collect(),
        }],
        Node::Html(html) => vec![MessageBlock::Paragraph(InlineContent {
            segments: vec![InlineSegment {
                text: html.value,
                style: InlineStyle::default(),
            }],
            ..Default::default()
        })],
        Node::MdxFlowExpression(expression) => vec![MessageBlock::CodeBlock {
            language: Some("mdx".to_owned()),
            code: expression.value,
        }],
        Node::Yaml(yaml) => vec![MessageBlock::CodeBlock {
            language: Some("yaml".to_owned()),
            code: yaml.value,
        }],
        Node::Toml(toml) => vec![MessageBlock::CodeBlock {
            language: Some("toml".to_owned()),
            code: toml.value,
        }],
        Node::Text(text) => vec![MessageBlock::Paragraph(InlineContent {
            segments: vec![InlineSegment {
                text: text.value,
                style: InlineStyle::default(),
            }],
            ..Default::default()
        })],
        Node::Break(_) => vec![MessageBlock::Paragraph(InlineContent {
            segments: vec![InlineSegment {
                text: "\n".to_owned(),
                style: InlineStyle::default(),
            }],
            ..Default::default()
        })],
        _ => Vec::new(),
    }
}

fn inline_content(children: &[Node]) -> InlineContent {
    let mut output = InlineContent::default();
    for child in children {
        append_inline(child, &InlineStyle::default(), &mut output);
    }
    output
}

fn append_inline(node: &Node, inherited: &InlineStyle, output: &mut InlineContent) {
    match node {
        Node::Text(text) => output.push(&text.value, inherited.clone()),
        Node::InlineCode(code) => {
            let mut style = inherited.clone();
            style.code = true;
            output.push(&code.value, style);
        }
        Node::InlineMath(math) => {
            let mut style = inherited.clone();
            style.code = true;
            output.push(&math.value, style);
        }
        Node::Emphasis(emphasis) => {
            let mut style = inherited.clone();
            style.emphasis = true;
            append_inline_children(&emphasis.children, &style, output);
        }
        Node::Strong(strong) => {
            let mut style = inherited.clone();
            style.strong = true;
            append_inline_children(&strong.children, &style, output);
        }
        Node::Delete(delete) => {
            let mut style = inherited.clone();
            style.strikethrough = true;
            append_inline_children(&delete.children, &style, output);
        }
        Node::Link(link) => {
            let mut style = inherited.clone();
            style.link = Some(link.url.clone());
            append_inline_children(&link.children, &style, output);
        }
        Node::LinkReference(link) => {
            append_inline_children(&link.children, inherited, output);
        }
        Node::Image(image) => {
            let label = if image.alt.trim().is_empty() {
                image.url.as_str()
            } else {
                image.alt.as_str()
            };
            let mut style = inherited.clone();
            style.link = Some(image.url.clone());
            output.push(label, style);
        }
        Node::ImageReference(image) => output.push(&image.alt, inherited.clone()),
        Node::Break(_) => output.push("\n", inherited.clone()),
        Node::Html(html) => output.push(&html.value, inherited.clone()),
        Node::FootnoteReference(footnote) => {
            output.push(format!("[{}]", footnote.identifier), inherited.clone());
        }
        Node::MdxTextExpression(expression) => {
            output.push(&expression.value, inherited.clone());
        }
        Node::MdxJsxTextElement(element) => {
            append_inline_children(&element.children, inherited, output);
        }
        Node::Paragraph(paragraph) => {
            append_inline_children(&paragraph.children, inherited, output);
        }
        _ => {}
    }
}

fn append_inline_children(children: &[Node], inherited: &InlineStyle, output: &mut InlineContent) {
    for child in children {
        append_inline(child, inherited, output);
    }
}

/// Parse and render one assistant message with the shared text palette.
pub fn render_markdown(id: &str, source: &str) -> AnyElement {
    let document = MessageDocument::parse(source);
    render_document(id, &document)
}

pub fn render_document(id: &str, document: &MessageDocument) -> AnyElement {
    render_blocks(id, document.blocks(), None, &mut document.code_cache.iter())
}

pub fn render_selectable_document(
    id: &str,
    document: &MessageDocument,
    selection: &SelectionContext,
) -> AnyElement {
    render_blocks(
        id,
        document.blocks(),
        Some(selection),
        &mut document.code_cache.iter(),
    )
}

fn render_blocks<'a>(
    id: &str,
    blocks: &[MessageBlock],
    selection: Option<&SelectionContext>,
    code_cache: &mut impl Iterator<Item = &'a std::cell::OnceCell<std::rc::Rc<code::Presentation>>>,
) -> AnyElement {
    let children = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| render_block(&format!("{id}-{index}"), block, selection, code_cache))
        .collect::<Vec<_>>();
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_3()
        .children(children)
        .into_any_element()
}

fn render_block<'a>(
    id: &str,
    block: &MessageBlock,
    selection: Option<&SelectionContext>,
    code_cache: &mut impl Iterator<Item = &'a std::cell::OnceCell<std::rc::Rc<code::Presentation>>>,
) -> AnyElement {
    match block {
        MessageBlock::Paragraph(content) => div()
            .w_full()
            .min_w_0()
            .whitespace_normal()
            .child(render_inline(id, content, selection))
            .into_any_element(),
        MessageBlock::Heading { level, content } => {
            let (size, weight) = match level {
                1 => (20.0, FontWeight::BOLD),
                2 => (18.0, FontWeight::SEMIBOLD),
                3 => (16.0, FontWeight::SEMIBOLD),
                _ => (14.0, FontWeight::SEMIBOLD),
            };
            div()
                .w_full()
                .min_w_0()
                .text_size(px(size))
                .line_height(px(size + 8.0))
                .font_weight(weight)
                .child(render_inline(id, content, selection))
                .into_any_element()
        }
        MessageBlock::CodeBlock { language, code } => render_code(
            id,
            language.as_deref(),
            code_cache
                .next()
                .expect("code cache follows block order")
                .get_or_init(|| code::prepare(language.as_deref(), code)),
            selection,
        ),
        MessageBlock::List { ordered, items } => {
            let rows = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let marker = match item.checked {
                        Some(true) => "[x]".to_owned(),
                        Some(false) => "[ ]".to_owned(),
                        None if *ordered => format!("{}.", index + 1),
                        None => "•".to_owned(),
                    };
                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .gap(px(6.))
                        .child(
                            div()
                                .w(px(if item.checked.is_some() {
                                    22.
                                } else if *ordered {
                                    24.
                                } else {
                                    10.
                                }))
                                .text_right()
                                .when(*ordered, |marker| {
                                    marker.font_family(crate::assets::CODE_FONT_FAMILY)
                                })
                                .flex_shrink_0()
                                .text_color(rgb(MUTED()))
                                .child(marker),
                        )
                        .child(div().flex_1().min_w_0().child(render_blocks(
                            &format!("{id}-item-{index}"),
                            &item.blocks,
                            selection,
                            code_cache,
                        )))
                })
                .collect::<Vec<_>>();
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_1()
                .children(rows)
                .into_any_element()
        }
        MessageBlock::BlockQuote(children) => div()
            .w_full()
            .border_l(gpui::px(crate::design::BORDER_WIDTH))
            .border_color(rgb(BORDER()))
            .pl_3()
            .text_color(rgb(MUTED()))
            .child(render_blocks(
                &format!("{id}-quote"),
                children,
                selection,
                code_cache,
            ))
            .into_any_element(),
        MessageBlock::Rule => div()
            .w_full()
            .h(px(1.))
            .my_1()
            .bg(rgb(BORDER()))
            .into_any_element(),
        MessageBlock::Table { rows } => {
            let rendered_rows = rows
                .iter()
                .enumerate()
                .map(|(row_index, row)| {
                    let cells = row
                        .iter()
                        .enumerate()
                        .map(|(cell_index, content)| {
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_2()
                                .py_1()
                                .when(cell_index + 1 < row.len(), |cell| {
                                    cell.border_r(gpui::px(crate::design::BORDER_WIDTH))
                                        .border_color(rgb(BORDER()))
                                })
                                .child(render_inline(
                                    &format!("{id}-row-{row_index}-cell-{cell_index}"),
                                    content,
                                    selection,
                                ))
                        })
                        .collect::<Vec<_>>();
                    div()
                        .w_full()
                        .flex()
                        .when(row_index + 1 < rows.len(), |row| {
                            row.border_b(gpui::px(crate::design::BORDER_WIDTH))
                                .border_color(rgb(BORDER()))
                        })
                        .children(cells)
                })
                .collect::<Vec<_>>();
            div()
                .w_full()
                .border(gpui::px(crate::design::BORDER_WIDTH))
                .border_color(rgb(BORDER()))
                .rounded(px(crate::design::RADIUS.block))
                .children(rendered_rows)
                .into_any_element()
        }
    }
}

fn render_inline(
    id: &str,
    content: &InlineContent,
    selection: Option<&SelectionContext>,
) -> AnyElement {
    let text = content.text();
    let mut highlights = Vec::new();
    let mut code_ranges = Vec::new();
    let mut link_ranges = Vec::new();
    let mut link_urls = Vec::new();
    let mut offset = 0;

    for segment in content.segments() {
        let end = offset + segment.text.len();
        let range = offset..end;
        let mut highlight = HighlightStyle::default();
        if segment.style.strong {
            highlight.font_weight = Some(FontWeight::BOLD);
        }
        if segment.style.emphasis {
            highlight.font_style = Some(FontStyle::Italic);
        }
        if segment.style.strikethrough {
            highlight.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(rgb(MUTED()).into()),
            });
        }
        if segment.style.code {
            highlight.color = Some(rgb(*crate::design::CODE_INK).into());
            code_ranges.push((
                range.clone(),
                SharedString::from(crate::assets::CODE_FONT_FAMILY),
            ));
        }
        if let Some(url) = segment.style.link.as_ref() {
            highlight.color = Some(rgb(LINK()).into());
            highlight.underline = Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(rgb(LINK()).into()),
                wavy: false,
            });
            link_ranges.push(range.clone());
            link_urls.push(url.clone());
        }
        if highlight != HighlightStyle::default() {
            highlights.push((range, highlight));
        }
        offset = end;
    }

    let selection_offset = selection.map(|selection| selection.offset(&text));
    if let Some((selection, offset)) = selection.zip(selection_offset) {
        if let Some(highlight) = selection.highlight(offset, text.len()) {
            highlights = merge_selection_highlight(highlights, highlight);
        }
    }
    if link_ranges.is_empty() {
        return content.layout_cache.render(
            format!("{id}-selection"),
            text.clone().into(),
            highlights,
            code_ranges,
            selection
                .zip(selection_offset)
                .map(|(selection, offset)| (selection.clone(), offset)),
        );
    }
    let styled = StyledText::new(text.clone())
        .with_highlights(highlights)
        .with_font_family_overrides(code_ranges);
    if let Some((selection, offset)) = selection.zip(selection_offset) {
        selection.wrap_linked(
            format!("{id}-selection"),
            offset,
            &text,
            styled,
            link_ranges,
            link_urls,
        )
    } else if link_ranges.is_empty() {
        styled.into_any_element()
    } else {
        linked_text(format!("{id}-links"), styled, link_ranges, link_urls, true).into_any_element()
    }
}

pub(super) fn linked_text(
    id: String,
    styled: StyledText,
    ranges: Vec<std::ops::Range<usize>>,
    urls: Vec<String>,
    open: bool,
) -> InteractiveText {
    let tooltip_ranges = ranges.clone();
    let tooltip_urls = urls.clone();
    InteractiveText::new(id, styled)
        // InteractiveText hit-tests shaped glyphs, so surrounding prose keeps
        // its text cursor. Selectable messages retain their drag-aware click handler.
        .on_click(ranges, move |index, _, cx| {
            if open {
                if let Some(url) = urls.get(index) { cx.open_url(url); }
            }
        })
        .tooltip(move |index, _, cx| {
            tooltip_ranges.iter().position(|range| range.contains(&index))
                .and_then(|index| tooltip_urls.get(index))
                .map(|url| cx.new(|_| super::tooltip::Hint::new("markdown-link-destination", url.clone())).into())
        })
}

/// GPUI text runs must be ordered and disjoint. Overlay selection on the
/// Markdown styles without duplicating ranges or losing bold/link/code styling.
fn merge_selection_highlight(
    existing: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    selection: (std::ops::Range<usize>, HighlightStyle),
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    // Existing runs are ordered and disjoint. Split only where the selection
    // crosses a run; rescanning all runs for every boundary was quadratic.
    let mut output = Vec::with_capacity(existing.len() + 4);
    let mut append = |range: std::ops::Range<usize>, style: HighlightStyle| {
        let left = selection.0.start.clamp(range.start, range.end);
        let right = selection.0.end.clamp(range.start, range.end);
        for (part, selected) in [
            (range.start..left, false),
            (left..right, true),
            (right..range.end, false),
        ] {
            let style = if selected {
                style.highlight(selection.1)
            } else {
                style
            };
            if !part.is_empty() && style != HighlightStyle::default() {
                output.push((part, style));
            }
        }
    };
    let mut cursor = 0;
    for (range, style) in existing {
        if cursor < range.start {
            append(cursor..range.start, HighlightStyle::default());
        }
        cursor = range.end;
        append(range, style);
    }
    if cursor < selection.0.end {
        append(cursor..selection.0.end, HighlightStyle::default());
    }
    output
}

fn render_code(
    id: &str,
    language: Option<&str>,
    prepared: &std::rc::Rc<code::Presentation>,
    selection: Option<&SelectionContext>,
) -> AnyElement {
    CodeBlockView {
        id: id.to_owned(),
        language: language.map(str::to_owned),
        prepared: prepared.clone(),
        selection: selection.cloned(),
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct CodeBlockView {
    id: String,
    language: Option<String>,
    prepared: std::rc::Rc<code::Presentation>,
    selection: Option<SelectionContext>,
}
impl gpui::RenderOnce for CodeBlockView {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let highlights = self.prepared.highlights(window, cx);
        render_code_ready(
            &self.id,
            self.language.as_deref(),
            &self.prepared,
            self.selection.as_ref(),
            highlights,
        )
    }
}

fn render_code_ready(
    id: &str,
    language: Option<&str>,
    prepared: &code::Presentation,
    selection: Option<&SelectionContext>,
    mut highlights: code::Runs,
) -> AnyElement {
    let code = &prepared.text;
    let offset = selection.map(|selection| selection.offset(code));
    if let Some((selection, offset)) = selection.zip(offset) {
        if let Some(selected) = selection.highlight(offset, code.len()) {
            highlights = merge_selection_highlight(highlights, selected);
        }
    }
    let body = prepared.layout_cache.render(
        format!("{id}-code-selection"),
        code.clone(),
        highlights,
        Vec::new(),
        selection
            .zip(offset)
            .map(|(selection, offset)| (selection.clone(), offset)),
    );
    div()
        .w_full()
        .min_w_0()
        .rounded(px(crate::design::RADIUS.block))
        .overflow_hidden()
        .border(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(BORDER()))
        .bg(rgb(CODE_FILL()))
        .when_some(language.filter(|s| !s.is_empty()), |block, language| {
            block.child(
                div()
                    .px_3()
                    .py_1()
                    .border_b(gpui::px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(BORDER()))
                    .text_size(px(12.))
                    .line_height(px(18.))
                    .text_color(rgb(MUTED()))
                    .child(language.to_owned()),
            )
        })
        .child(
            div()
                .id(format!("{id}-code-scroll"))
                .w_full()
                .min_w_0()
                .flex()
                .overflow_x_scroll()
                .child(
                    div()
                        .flex_shrink_0()
                        .p_3()
                        .min_w_full()
                        .whitespace_nowrap()
                        .font_family(crate::assets::CODE_FONT_FAMILY)
                        .text_size(px(12.))
                        .line_height(px(20.))
                        .text_color(rgb(TEXT()))
                        .child(body),
                ),
        )
        .into_any_element()
}

/// Share message text between frames and parse each visible Markdown row once.
#[cfg(test)]
mod selection_style_tests {
    use super::*;
    use gpui::rgba;
    #[test]
    fn intrinsic_width_stops_at_the_bubble_cap_without_sizing_a_long_document() {
        let source = format!("{}\n{}", "中文🐈".repeat(10000), "later".repeat(10000));
        let mut lengths = Vec::new();
        let width = bounded_width(&source, 480., |sample| {
            lengths.push(sample.chars().count());
            sample.chars().count() as f32 * 13.
        });
        assert_eq!(width, 480.);
        assert_eq!(lengths, [128]);
        assert_eq!(bounded_width("abc\n12345\nx", 480., |s| s.len() as f32), 5.);
    }
    #[test]
    fn reference_links_keep_destinations_and_inline_styles() {
        let document = MessageDocument::parse("[**粗体**][DOC] 与 ![图片][image]\n\n[doc]: https://example.com/docs\n[image]: https://example.com/image.png");
        let MessageBlock::Paragraph(content) = &document.blocks()[0] else {
            panic!("paragraph")
        };
        assert!(content
            .segments()
            .iter()
            .any(|segment| segment.text == "粗体"
                && segment.style.strong
                && segment.style.link.as_deref() == Some("https://example.com/docs")));
        assert!(content
            .segments()
            .iter()
            .any(|segment| segment.text == "图片"
                && segment.style.link.as_deref() == Some("https://example.com/image.png")));
    }

    #[test]
    fn preparing_and_revisiting_history_does_not_parse_code_synchronously() {
        let before = code::parse_count();
        let documents = (0..64).map(|index| MessageDocument::parse(&format!(
            "正文 {index}\n\n```rust\nlet task_{index} = \"中文\";\n```\n\n> ```json\n> {{\"task\":{index}}}\n> ```"
        ))).collect::<Vec<_>>();
        for (index, document) in documents.iter().enumerate() {
            let _ = render_document(&format!("cache-{index}"), document);
            assert_eq!(document.code_cache.len(), 2);
            assert!(document
                .code_cache
                .iter()
                .all(|cache| cache.get().is_some()));
        }
        let parses = code::parse_count();
        assert_eq!(
            parses, before,
            "preparing code parsed on the rendering thread"
        );
        for (index, document) in documents.iter().enumerate() {
            let _ = render_document(&format!("cache-{index}"), document);
            let first = document.shared_plain_text();
            let second = document.shared_plain_text();
            assert_eq!(first.as_ptr(), second.as_ptr(), "message text was copied");
        }
        assert_eq!(
            code::parse_count(),
            parses,
            "scrolling reparsed unchanged code"
        );
    }

    #[test]
    fn selection_overlay_covers_unstyled_gaps_without_overlapping_code_runs() {
        let styled = HighlightStyle {
            color: Some(rgb(*crate::design::CODE_INK).into()),
            ..Default::default()
        };
        let selected = HighlightStyle {
            background_color: Some(rgba(*crate::design::TEXT_SELECTION).into()),
            ..Default::default()
        };
        let runs = (0..4096)
            .map(|index| (index * 4..index * 4 + 2, styled))
            .collect();
        let overlaid = merge_selection_highlight(runs, (1..16383, selected));
        assert!(overlaid
            .windows(2)
            .all(|pair| pair[0].0.end <= pair[1].0.start));
        assert_eq!(overlaid.first().unwrap().0, 0..1);
        assert_eq!(overlaid.last().unwrap().0.end, 16383);
        for (range, style) in &overlaid {
            assert_eq!(
                style.background_color.is_some(),
                range.start >= 1 && range.end <= 16383
            );
        }
    }

    #[test]
    fn partial_selection_keeps_markdown_styles_with_disjoint_utf8_runs() {
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        let selected = HighlightStyle {
            background_color: Some(rgba(*crate::design::TEXT_SELECTION).into()),
            ..Default::default()
        };
        let ranges = merge_selection_highlight(vec![(0..6, bold)], (3..9, selected));
        assert_eq!(
            ranges.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(),
            vec![0..3, 3..6, 6..9]
        );
        assert_eq!(ranges[0].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(ranges[1].1.font_weight, Some(FontWeight::BOLD));
        assert_eq!(ranges[1].1.background_color, selected.background_color);
        assert_eq!(ranges[2].1.font_weight, None);
    }
}

/// Current layout-cache occupancy; glyph/run storage is bounded independently of history length.
#[cfg(feature = "headless-bench")]
pub fn text_layout_cache_stats() -> (usize, usize, u64, u64) {
    text_cache::stats()
}
