//! Several agents sharing one Chat: identity discs with maker marks, groups,
//! relative times, header-line reply quotes with the omission rule, not-loaded
//! originals, the jump wash, sent comment batches and draft quotes.
//!
//! Every rule comes from core: the host injects [`Present`], which runs
//! `zork_client_core::message_presentation::handle` (the same JSON bridge
//! Android uses). The fixture follows the approved prototype's thread.
use super::{
    identity::{self, ReplyText},
    presentation::{self, RowHeights},
    CommentPairView, CommentsView, Decorations, Row,
};
use crate::components::message::{MarkKind, MessageDocument, TextMarks};
use gpui::{div, point, prelude::*, px, rgb, Context, Entity, ScrollHandle, Window};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// Runs core's presentation over a request
/// `{rows, now_ms, utc_offset_minutes, locale, has_older, devices, agent_models}`.
pub type Present = Rc<dyn Fn(Value) -> Value>;

/// Every state, with its story size (width, height).
pub const STATES: [(&str, f32, f32); 10] = [
    ("conversation", 1000., 760.),
    ("comment-batch", 1000., 760.),
    ("run-hover", 1000., 760.),
    ("time-hover", 1000., 760.),
    ("reply-jump", 1000., 760.),
    ("not-loaded", 1000., 560.),
    ("loaded", 1000., 560.),
    ("drafts", 1000., 820.),
    ("seven-agents", 1000., 760.),
    ("phone", 390., 820.),
];

/// The fixture clock: 2026-09-26 14:30:00 +08:00.
const NOW_MS: i64 = 1_790_404_200_000;
const PHONE_WIDTH: f32 = 480.;
const WASH_HOLD: Duration = Duration::from_millis(1600);
const WASH_FADE: Duration = Duration::from_millis(600);

struct Agent {
    id: &'static str,
    name: &'static str,
    device: &'static str,
    model: &'static str,
}
const AGENTS: [Agent; 7] = [
    Agent {
        id: "planner",
        name: "Planner",
        device: "dev-a",
        model: "gpt-6-astra",
    },
    Agent {
        id: "builder",
        name: "Builder",
        device: "dev-b",
        model: "deepseek-flash",
    },
    Agent {
        id: "review",
        name: "审阅助手",
        device: "dev-b",
        model: "claude-sonnet-5",
    },
    Agent {
        id: "tester",
        name: "Tester",
        device: "dev-c",
        model: "glm-5.1",
    },
    Agent {
        id: "docs",
        name: "文档",
        device: "dev-a",
        model: "qwen3.6-plus",
    },
    Agent {
        id: "ops",
        name: "Ops",
        device: "dev-c",
        model: "kimi-k2.6",
    },
    Agent {
        id: "design",
        name: "Designer",
        device: "dev-b",
        model: "gemini-3-pro",
    },
];

fn row(id: &str, who: &str, at: &str, text: &str) -> Value {
    let mut value = json!({"type": "message", "id": id, "created_at": at, "content": text});
    match AGENTS.iter().find(|agent| agent.id == who) {
        Some(agent) => {
            value["role"] = json!("assistant");
            value["author_agent_id"] = json!(agent.id);
            value["author_name"] = json!(agent.name);
            value["device"] = json!(agent.device);
            value["model"] = json!(agent.model);
        }
        None => value["role"] = json!("user"),
    }
    value
}
fn reply(mut value: Value, to: &str, quote: Option<(&str, &str)>) -> Value {
    value["reply_to"] = json!(to);
    if let Some((text, kind)) = quote {
        value["quote"] = json!(text);
        value["quote_kind"] = json!(kind);
    }
    value
}
fn batch(id: &str, at: &str, pairs: &[(&str, &str, &str, &str)], extra: &str) -> Value {
    use zork_client_types::comments::{compose, CommentSource, DraftComment};
    let comments: Vec<DraftComment> = pairs
        .iter()
        .enumerate()
        .map(|(index, (source, agent, quote, reply))| DraftComment {
            id: format!("{id}-{index}"),
            source: CommentSource {
                session_id: "story".into(),
                message_id: Some((*source).into()),
                author: AGENTS
                    .iter()
                    .find(|a| a.id == *agent)
                    .map(|a| a.name.to_owned()),
                author_agent_id: Some((*agent).into()),
                quote: (*quote).into(),
            },
            comment: (*reply).into(),
        })
        .collect();
    row(id, "user", at, &compose(extra, &comments))
}

fn earlier() -> Vec<Value> {
    vec![
        row(
            "old1",
            "user",
            "2026-09-17T12:30:00+08:00",
            "上周的登录埋点口径先定下来：点击和展开分开统计。",
        ),
        row(
            "old2",
            "planner",
            "2026-09-17T12:33:00+08:00",
            "收到，口径写进了 `docs/metrics.md`，后续改版沿用。",
        ),
    ]
}

fn thread() -> Vec<Value> {
    vec![
        row("c1", "user", "2026-09-25T12:30:00+08:00", "@Planner 把登录页改版拆成任务，审阅助手和 Builder 一起跟进。"),
        row("c2", "planner", "2026-09-25T12:34:00+08:00", "好的，拆成三步：\n\n1. 梳理现有登录流程和埋点\n2. 出新版布局与文案\n3. 实现并补测试\n\n我先做第 1 步，Builder 可以先搭页面骨架。"),
        row("criteria", "planner", "2026-09-25T12:35:00+08:00", "验收标准：首屏只保留账号、密码和一个主按钮；错误提示贴在对应字段下方，不弹窗；键盘可以走完全部流程，焦点顺序与视觉顺序一致；深色主题逐项核对对比度。"),
        row("c4", "builder", "2026-09-25T13:30:00+08:00", "骨架已推到 `ud/login-refresh`，表单和按钮先复用现有组件。"),
        reply(row("c5", "review", "2026-09-25T14:30:00+08:00", "看过了：密码框缺少显示/隐藏切换，错误提示的对比度也不够。建议直接用 `field_with_error`，焦点和错误态它都处理好了。"), "c4", Some(("表单和按钮先复用现有组件", "excerpt"))),
        row("c6", "user", "2026-09-25T15:30:00+08:00", "按审阅意见改，改完叫我看。"),
        reply(row("c7", "builder", "2026-09-26T09:30:00+08:00", "已改：加了显示切换，错误提示换成 `field_with_error`。截图放在 Chat 文件里了。"), "c5", Some(("密码框缺少显示/隐藏切换，错误提示对比度不够", "summary"))),
        row("c8", "builder", "2026-09-26T09:32:00+08:00", "顺便把主按钮换成炭墨主操作样式，发送中的状态沿用按钮内的加载反馈。"),
        row("spacing", "review", "2026-09-26T11:30:00+08:00", "布局看过了。「忘记密码」链接离主按钮太近，触控区重叠，建议下移 8 px。"),
        row("c10", "user", "2026-09-26T12:00:00+08:00", "同意，顺便把第三方登录收进「更多方式」。"),
        row("c11", "builder", "2026-09-26T12:30:00+08:00", "已调整间距，第三方登录收进了「更多方式」菜单，默认收起。"),
        reply(row("c12", "planner", "2026-09-26T13:00:00+08:00", "埋点同步更新：登录方式的点击改为在菜单展开后上报，避免把展开误算成选择。"), "old2", Some(("上周定的埋点口径：点击和展开分开统计", "summary"))),
        row("c13", "review", "2026-09-26T13:30:00+08:00", "菜单的键盘操作正常，Esc 能关闭并把焦点还给触发按钮。"),
        row("c14", "builder", "2026-09-26T13:50:00+08:00", "深色主题的截图已更新到 Chat 文件。"),
        batch(
            "comments",
            "2026-09-26T14:10:00+08:00",
            &[
                ("criteria", "planner", "错误提示贴在对应字段下方，不弹窗", "这条保留，但错误文案要写清楚怎么改，不要只说「格式错误」。"),
                ("spacing", "review", "建议下移 8 px", "8 px 还是有点挤，试试 12 px。"),
            ],
            "其他都可以，按这个继续。",
        ),
        row("shortok", "review", "2026-09-26T14:14:00+08:00", "好。"),
        reply(row("c17", "builder", "2026-09-26T14:16:00+08:00", "收到，两处都按你的意见改：错误文案改成具体的修改建议，链接下移 12 px。"), "comments", Some(("错误文案要具体；链接下移 12 px", "summary"))),
        reply(row("c18", "planner", "2026-09-26T14:18:00+08:00", "有一条旧讨论被删了，按删除前的结论执行。"), "gone", None),
        row("c19", "user", "2026-09-26T14:22:00+08:00", "文案这块交给 Planner 再过一遍。"),
        reply(row("c20", "planner", "2026-09-26T14:23:00+08:00", "我来，顺便把审阅的「好」当作文案方向已确认。"), "shortok", None),
        reply(row("c21", "review", "2026-09-26T14:27:00+08:00", "按最初的验收标准复查：焦点顺序已经正确；深色主题下错误提示的对比度只有 3.9:1，还差一点。"), "criteria", Some(("深色主题逐项核对对比度", "excerpt"))),
        reply(row("c22", "builder", "2026-09-26T14:29:20+08:00", "对比度调到 4.8:1 了，其余不变。"), "c21", Some(("对比度只有 3.9:1", "excerpt"))),
        row("notes", "user", "2026-09-26T14:29:30+08:00", "把这轮改动整理成发布说明，按页面分开写。"),
        row("c24", "builder", "2026-09-26T14:29:35+08:00", "发布说明：\n\n1. 登录页：首屏只保留账号、密码和主按钮；第三方登录收进「更多方式」菜单，默认收起。\n2. 表单：密码框加显示/隐藏切换；错误提示贴在字段下方，文案给出具体修改建议。\n3. 间距：「忘记密码」链接下移 12 px，不再和主按钮的触控区重叠。\n4. 深色主题：错误提示对比度从 3.9:1 调到 4.8:1，其余颜色逐项核对通过。"),
        row("c25", "builder", "2026-09-26T14:29:37+08:00", "迁移说明：旧的第三方登录按钮配置仍然有效，菜单会自动读取；埋点口径变化已同步给数据组，看板下周切换。"),
        reply(row("c26", "builder", "2026-09-26T14:29:40+08:00", "以上是按页面整理好的发布说明，需要我直接发到发布频道吗？"), "notes", None),
    ]
}

fn seven() -> Vec<Value> {
    vec![
        row(
            "s1",
            "tester",
            "2026-09-26T13:55:00+08:00",
            "端到端测试补了三条：错误提示、键盘流程、菜单收起。",
        ),
        row(
            "s2",
            "docs",
            "2026-09-26T13:57:00+08:00",
            "更新了登录帮助文档的截图和步骤说明。",
        ),
        row(
            "s3",
            "ops",
            "2026-09-26T14:00:00+08:00",
            "预发环境已部署改版分支，健康检查通过。",
        ),
        row(
            "s4",
            "design",
            "2026-09-26T14:05:00+08:00",
            "「更多方式」菜单的图标换成统一线条风格的版本了。",
        ),
    ]
}

/// Rows of a state, oldest first, and whether older history exists.
fn fixture(state: &str, loaded: bool) -> (Vec<Value>, bool) {
    match state {
        // A short window whose first reply points into history not loaded yet.
        "not-loaded" | "loaded" => {
            let window: Vec<Value> = thread()
                .into_iter()
                .filter(|row| {
                    ["c10", "c11", "c12", "c13", "c14"].contains(&row["id"].as_str().unwrap())
                })
                .collect();
            if loaded {
                (earlier().into_iter().chain(window).collect(), false)
            } else {
                (window, true)
            }
        }
        // The thread up to the reply to the user's comment batch.
        "comment-batch" => {
            let mut rows: Vec<Value> = earlier().into_iter().chain(thread()).collect();
            if let Some(end) = rows.iter().position(|row| row["id"] == "c17") {
                rows.truncate(end + 1);
            }
            (rows, false)
        }
        "seven-agents" => {
            let mut rows: Vec<Value> = earlier().into_iter().chain(thread()).collect();
            let at = rows
                .iter()
                .position(|row| row["id"] == "comments")
                .unwrap_or(rows.len());
            for (offset, extra) in seven().into_iter().enumerate() {
                rows.insert(at + offset, extra);
            }
            (rows, false)
        }
        _ => (earlier().into_iter().chain(thread()).collect(), false),
    }
}

fn request(rows: &[Value], has_older: bool) -> Value {
    json!({
        "rows": rows,
        "now_ms": NOW_MS,
        "utc_offset_minutes": 480,
        "locale": "zh-CN",
        "has_older": has_older,
        "devices": {
            "dev-a": {"display": "A", "machine": "zuozijiandeMacBook-Air"},
            "dev-b": {"display": "B", "machine": "zuozijians-Mac-Studio"},
            "dev-c": {"display": "C", "machine": "mini1"}
        },
        "agent_models": {}
    })
}

fn index_of(state: &str, id: &str) -> usize {
    let (rows, _) = fixture(state, false);
    rows.iter().position(|row| row["id"] == id).unwrap_or(0)
}

/// Pointer actions a state's preview runs before the snapshot.
pub fn actions(state: &str) -> Vec<Value> {
    let target = |id: String| json!({"element_id": id});
    match state {
        // A later message of Builder's run reveals its own time on hover.
        "run-hover" => vec![
            json!({"type": "move", "target": target(format!("message-{}-run-time", index_of(state, "c25")))}),
        ],
        "time-hover" => vec![
            json!({"type": "move", "target": target(format!("message-{}-time", index_of(state, "c21")))}),
        ],
        "reply-jump" => vec![
            json!({"type": "click", "target": target(format!("message-{}-reply", index_of(state, "c21")))}),
        ],
        "loaded" => vec![
            json!({"type": "click", "target": target(format!("message-{}-reply", index_of(state, "c12")))}),
        ],
        _ => vec![],
    }
}

struct Wash {
    row: usize,
    passage: Option<std::ops::Range<usize>>,
    started: std::time::Instant,
}

pub struct Story {
    state: String,
    present: Present,
    loaded: bool,
    rows: Vec<Value>,
    has_older: bool,
    transcript: presentation::Transcript,
    documents: Vec<MessageDocument>,
    texts: Vec<String>,
    comments: Vec<Option<(Vec<Rc<MessageDocument>>, Option<Rc<MessageDocument>>)>>,
    heights: Rc<RefCell<RowHeights>>,
    available_width: f32,
    scroll: ScrollHandle,
    pending_jump: Option<usize>,
    pending_restore: Option<gpui::Pixels>,
    wash: Option<Wash>,
    hint: Option<String>,
    composer: Option<Entity<crate::component_story::ConversationComposer>>,
    drafts: Vec<(String, String)>,
}

impl Story {
    pub fn new(state: &str, present: Present, cx: &mut Context<Self>) -> Self {
        let mut story = Self {
            state: state.into(),
            present,
            loaded: false,
            rows: Vec::new(),
            has_older: false,
            transcript: Default::default(),
            documents: Vec::new(),
            texts: Vec::new(),
            comments: Vec::new(),
            heights: Default::default(),
            available_width: 0.,
            scroll: ScrollHandle::new(),
            pending_jump: None,
            pending_restore: None,
            wash: None,
            hint: None,
            composer: None,
            drafts: Vec::new(),
        };
        story.reload();
        story.scroll.scroll_to_bottom();
        if state == "drafts" {
            story.drafts = vec![
                ("c21".into(), "焦点顺序已经正确".into()),
                ("c24".into(), "错误提示贴在字段下方".into()),
            ];
            let author = |id: &str| author_of(&story.transcript, &story.rows, id);
            let drafts = vec![
                (
                    author("c21"),
                    "焦点顺序已经正确".to_owned(),
                    "菜单里的 Tab 顺序也要一起测。".to_owned(),
                ),
                (
                    author("c24"),
                    "错误提示贴在字段下方".to_owned(),
                    String::new(),
                ),
            ];
            story.composer =
                Some(cx.new(|cx| {
                    crate::component_story::ConversationComposer::with_drafts(drafts, cx)
                }));
        }
        story
    }

    fn reload(&mut self) {
        let (rows, has_older) = fixture(&self.state, self.loaded);
        let transcript = presentation::parse((self.present)(request(&rows, has_older)));
        self.documents = rows
            .iter()
            .map(|row| {
                let content = row["content"].as_str().unwrap_or_default();
                if row["role"] == "user" {
                    MessageDocument::plain(&zork_client_types::comments::display_text(content))
                } else {
                    MessageDocument::parse(content)
                }
            })
            .collect();
        self.texts = rows
            .iter()
            .zip(&self.documents)
            .zip(&transcript.rows)
            .map(|((row, document), presented)| {
                if presented.comments.is_some() {
                    zork_client_types::comments::quotable_text(
                        row["content"].as_str().unwrap_or_default(),
                    )
                } else {
                    document.plain_text()
                }
            })
            .collect();
        self.comments = transcript
            .rows
            .iter()
            .map(|row| {
                row.comments.as_ref().map(|comments| {
                    (
                        comments
                            .pairs
                            .iter()
                            .map(|pair| Rc::new(MessageDocument::plain(&pair.pair.reply)))
                            .collect(),
                        (!comments.extra_text.trim().is_empty())
                            .then(|| Rc::new(MessageDocument::plain(&comments.extra_text))),
                    )
                })
            })
            .collect();
        self.rows = rows;
        self.has_older = has_older;
        self.transcript = transcript;
    }

    fn content_width(&self) -> f32 {
        let gutter = if self.available_width < PHONE_WIDTH {
            32.
        } else {
            48.
        };
        (self.available_width - gutter).min(744.).max(1.)
    }

    /// Scrolls the original to ~24 px below the top and washes the passage
    /// (or the whole message) for about 1.6 s.
    fn jump(&mut self, target: usize, mark: Option<String>, cx: &mut Context<Self>) {
        let passage = mark
            .as_deref()
            .and_then(|mark| presentation::find_passage(&self.texts[target], mark));
        self.wash = Some(Wash {
            row: target,
            passage,
            started: std::time::Instant::now(),
        });
        self.pending_jump = Some(target);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(WASH_HOLD).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// First click on a not-loaded original: load older history only, keep
    /// the distance from the bottom, and say how to jump.
    fn load_earlier(&mut self, cx: &mut Context<Self>) {
        let offset = self.scroll.offset();
        let max = self.scroll.max_offset();
        self.pending_restore = Some(max.y + offset.y);
        self.loaded = true;
        self.reload();
        self.hint = Some("已加载，再点引用可以跳转到原消息".into());
        cx.notify();
    }

    fn apply_pending_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(from_bottom) = self.pending_restore {
            let max = self.scroll.max_offset();
            if max.y > from_bottom {
                let offset = self.scroll.offset();
                self.scroll
                    .set_offset(point(offset.x, -(max.y - from_bottom)));
                self.pending_restore = None;
            } else {
                cx.on_next_frame(window, |_, _, cx| cx.notify());
            }
        }
        let Some(index) = self.pending_jump else {
            return;
        };
        match self.scroll.bounds_for_item(index) {
            Some(item) => {
                let view = self.scroll.bounds();
                let offset = self.scroll.offset();
                let max = self.scroll.max_offset();
                // Child bounds are in unscrolled content coordinates.
                let target = (-(item.top() - view.top()) + px(24.))
                    .min(px(0.))
                    .max(-max.y);
                self.scroll.set_offset(point(offset.x, target));
                self.pending_jump = None;
                cx.notify();
            }
            None => cx.on_next_frame(window, |_, _, cx| cx.notify()),
        }
    }

    fn wash_alpha(&self, window: &mut Window) -> f32 {
        let Some(wash) = &self.wash else {
            return 0.;
        };
        let elapsed = wash.started.elapsed();
        if elapsed < WASH_HOLD {
            return 1.;
        }
        let fade = (elapsed - WASH_HOLD).as_secs_f32() / WASH_FADE.as_secs_f32();
        if fade >= 1. {
            return 0.;
        }
        window.request_animation_frame();
        1. - fade
    }
}

/// The quote author of fixture row `id` as the transcript shows it.
fn author_of(
    transcript: &presentation::Transcript,
    rows: &[Value],
    id: &str,
) -> identity::QuoteAuthor {
    let agent = rows
        .iter()
        .find(|row| row["id"] == id)
        .and_then(|row| row["author_agent_id"].as_str().map(str::to_owned));
    transcript
        .rows
        .iter()
        .filter_map(|row| row.identity.as_ref())
        .find(|identity| identity.author.agent_id == agent)
        .map(|identity| identity.author.quote_author())
        .unwrap_or(identity::QuoteAuthor {
            name: String::new(),
            disc: None,
        })
}

impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_scroll(window, cx);
        let content_width = self.content_width();
        let phone = self.available_width < PHONE_WIDTH;
        let show_device = self.transcript.multi_device && !phone;
        let screen = self.scroll.bounds().size.height.as_f32();
        let wash_alpha = self.wash_alpha(window);
        if wash_alpha <= 0. {
            self.wash = None;
        }
        let weak = cx.entity().downgrade();
        let heights = self.heights.clone();
        let height_of = |index: usize| {
            let row = &self.transcript.rows[index];
            let id = row.id.clone().unwrap_or_default();
            heights.borrow().get(&id, content_width).unwrap_or_else(|| {
                presentation::estimate_height(
                    &self.texts[index],
                    content_width,
                    row.group_head,
                    row.user,
                )
            })
        };
        let mut rows = Vec::with_capacity(self.transcript.rows.len());
        for (index, presented) in self.transcript.rows.iter().enumerate() {
            let omit =
                presentation::omit_row_reply(&self.transcript.rows, index, &height_of, screen);
            let mut decor = Decorations::from_presentation(
                presented,
                show_device,
                omit,
                false,
                ReplyText::default(),
            );
            if let Some(line) = presented.reply.as_ref() {
                let weak = weak.clone();
                let mark = line.mark();
                decor.on_reply = match (line.state, line.target_index) {
                    (presentation::TargetState::Linked, Some(target)) => {
                        Some(Rc::new(move |_, cx| {
                            let mark = mark.clone();
                            let _ = weak.update(cx, |v, cx| v.jump(target, mark, cx));
                        }))
                    }
                    (presentation::TargetState::NotLoaded, _) => Some(Rc::new(move |_, cx| {
                        let _ = weak.update(cx, |v, cx| v.load_earlier(cx));
                    })),
                    _ => None,
                };
            }
            // In-place marks: the jump wash on the passage, draft underlines.
            let mut marks = Vec::new();
            let mut whole = 0.;
            if let Some(wash) = self.wash.as_ref().filter(|wash| wash.row == index) {
                match &wash.passage {
                    Some(range) => marks.push((range.clone(), MarkKind::Wash(wash_alpha))),
                    None => whole = wash_alpha,
                }
            }
            for (source, quote) in &self.drafts {
                if presented.id.as_deref() == Some(source.as_str()) {
                    if let Some(range) = presentation::find_passage(&self.texts[index], quote) {
                        marks.push((range, MarkKind::Draft));
                    }
                }
            }
            decor.wash = whole;
            decor.marks =
                (!marks.is_empty()).then(|| TextMarks::new(self.texts[index].clone(), marks));
            decor.quote_action = Some(("引用".into(), Rc::new(|_, _| {})));
            if let (Some(view), Some((docs, extra))) = (&presented.comments, &self.comments[index])
            {
                decor.comments = Some(CommentsView {
                    pairs: view
                        .pairs
                        .iter()
                        .zip(docs)
                        .map(|(pair, document)| {
                            let weak = weak.clone();
                            let passage = pair.pair.quote.clone();
                            let target = pair.source_index;
                            CommentPairView {
                                author: pair.source.quote_author(),
                                passage: pair.pair.quote.clone(),
                                reply: document.clone(),
                                on_click: target.map(|target| {
                                    Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                        let passage = passage.clone();
                                        let _ = weak
                                            .update(cx, |v, cx| v.jump(target, Some(passage), cx));
                                    }) as super::Callback
                                }),
                            }
                        })
                        .collect(),
                    extra: extra.clone(),
                });
            }
            let element = Row {
                index,
                user: presented.user,
                document: &self.documents[index],
                content_width,
                selection: None,
                author_name: None,
                device: None,
                model: None,
                time: None,
            }
            .render_with(window, decor);
            let id = presented.id.clone().unwrap_or_default();
            let heights = self.heights.clone();
            let owner = weak.clone();
            rows.push(
                div()
                    .relative()
                    .child(element)
                    .child(
                        gpui::canvas(
                            move |bounds, _, cx| {
                                let changed = heights.borrow_mut().record(
                                    &id,
                                    content_width,
                                    bounds.size.height.as_f32(),
                                );
                                if changed {
                                    let _ = owner.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .into_any_element(),
            );
        }
        let (discs, more) = presentation::agent_stack(&self.transcript);
        let p = crate::design::ZORK_UI.palette;
        let owner = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(p.canvas))
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            let next = bounds.size.width.as_f32();
                            if (v.available_width - next).abs() > 0.1 {
                                v.available_width = next;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .h(px(44.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(identity::chat_title(
                        "chat-header",
                        &discs,
                        more,
                        "登录页改版",
                        p.canvas,
                    )),
            )
            .child(
                div()
                    .id("multi-agent-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows)
                    .child(div().h(px(16.))),
            )
            .when_some(self.composer.clone(), |v, composer| {
                v.child(div().flex_shrink_0().h(px(210.)).child(composer))
            })
            .when_some(self.hint.clone(), |v, hint| {
                v.child(
                    div()
                        .absolute()
                        .bottom(px(28.))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(identity::dark_pill("multi-agent-hint", hint)),
                )
            })
    }
}

/// Creates the story entity for `state`.
pub fn create(state: &str, present: Present, cx: &mut gpui::App) -> Entity<Story> {
    cx.new(|cx| Story::new(state, present, cx))
}
