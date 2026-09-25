//! Design concept: several agents sharing one Chat, with reply quotes and
//! relative times. Fixture data only; the live transcript is not wired yet.
use super::{
    identity::{AgentIdentity, Grouping, QuoteAuthor, Reply, ReplyText},
    Decorations, Row,
};
use crate::components::message::MessageDocument;
use gpui::{div, point, prelude::*, px, Context, Entity, ScrollHandle, Window};
use std::rc::Rc;

/// Formats an RFC 3339 instant as (label, full timestamp). The desktop host
/// injects `zork_client_core::message_time` with a fixed clock.
pub type TimeFormat = Rc<dyn Fn(&str) -> (String, String)>;

/// Every state in the concept, with its story size (width, height).
pub const STATES: [(&str, f32, f32); 10] = [
    ("conversation", 900., 700.),
    ("run-hover", 900., 700.),
    ("reply-offscreen", 900., 720.),
    ("reply-highlight", 900., 720.),
    ("reply-edge-cases", 900., 700.),
    ("relative-times", 900., 540.),
    ("time-hover", 900., 540.),
    ("phone", 390., 820.),
    ("phone-reply", 390., 780.),
    ("phone-time-hold", 390., 560.),
];

/// Stories whose preview hovers a time label to show its full timestamp.
pub fn hover_target(state: &str) -> Option<&'static str> {
    match state {
        "time-hover" | "phone-time-hold" => Some("message-3-time"),
        // The second message of 审阅助手's run: its own time appears on hover.
        "run-hover" => Some("message-5-run-time"),
        _ => None,
    }
}

/// Messages that follow within this window join the previous message's group.
const GROUP_WINDOW_SECONDS: i64 = 5 * 60;
/// An original this close above its reply is on screen in practice: it gets
/// the marker. The rule counts messages, not the live viewport, so a row never
/// changes height while the reader scrolls.
const MARKER_DISTANCE: usize = 3;

struct Agent {
    id: &'static str,
    name: &'static str,
    device: &'static str,
    model: &'static str,
}
const AGENTS: [Agent; 3] = [
    Agent {
        id: "agent-planner",
        name: "Planner",
        device: "MacBook Air",
        model: "gpt-6",
    },
    Agent {
        id: "agent-reviewer",
        name: "审阅助手",
        device: "mini1",
        model: "claude-opus-5",
    },
    Agent {
        id: "agent-builder",
        name: "Builder",
        device: "mini1",
        model: "gpt-6-codex",
    },
];
const PLANNER: Option<usize> = Some(0);
const REVIEWER: Option<usize> = Some(1);
const BUILDER: Option<usize> = Some(2);
const USER: Option<usize> = None;

#[derive(Clone)]
enum ReplySpec {
    To(&'static str),
    Deleted(Option<usize>),
    NotLoaded,
    Loading,
    Long(usize, &'static str),
}

#[derive(Clone)]
struct Message {
    id: &'static str,
    author: Option<usize>,
    at: &'static str,
    body: &'static str,
    reply: Option<ReplySpec>,
}
fn m(id: &'static str, author: Option<usize>, at: &'static str, body: &'static str) -> Message {
    Message {
        id,
        author,
        at,
        body,
        reply: None,
    }
}
impl Message {
    fn reply(mut self, reply: ReplySpec) -> Self {
        self.reply = Some(reply);
        self
    }
}

fn conversation() -> Vec<Message> {
    vec![
        m(
            "c0",
            USER,
            "2026-09-26T14:02:00+08:00",
            "@Planner 把登录页改版拆成任务，审阅助手和 Builder 一起跟进。",
        ),
        m(
            "c1",
            PLANNER,
            "2026-09-26T14:03:00+08:00",
            "好的，拆成三步：\n\n1. 梳理现有登录流程和埋点\n2. 出新版布局与文案\n3. 实现并补测试",
        ),
        m(
            "c2",
            PLANNER,
            "2026-09-26T14:04:00+08:00",
            "我先做第 1 步，Builder 可以先搭页面骨架。",
        ),
        m(
            "c3",
            BUILDER,
            "2026-09-26T14:11:00+08:00",
            "骨架已推到 `ud/login-refresh`，表单和按钮先复用现有组件。",
        ),
        m(
            "c4",
            REVIEWER,
            "2026-09-26T14:18:00+08:00",
            "看过了：密码框缺少显示/隐藏切换，错误提示的对比度也不够。",
        )
        .reply(ReplySpec::To("c3")),
        m(
            "c5",
            REVIEWER,
            "2026-09-26T14:19:00+08:00",
            "建议直接用 `field_with_error`，焦点和错误态它都处理好了。",
        ),
        m(
            "c6",
            USER,
            "2026-09-26T14:24:00+08:00",
            "按审阅意见改，改完叫我看。",
        ),
        m(
            "c7",
            BUILDER,
            "2026-09-26T14:27:00+08:00",
            "已改：加了显示切换，错误提示换成 `field_with_error`。",
        )
        .reply(ReplySpec::To("c4")),
        m(
            "c8",
            BUILDER,
            "2026-09-26T14:28:00+08:00",
            "截图放在 Chat 文件里了。",
        ),
        m(
            "c9",
            PLANNER,
            "2026-09-26T14:29:40+08:00",
            "流程梳理完成：共 4 个入口、7 个埋点，清单在 `docs/login-flow.md`。",
        ),
    ]
}

fn long_thread() -> Vec<Message> {
    vec![
        m("t0", PLANNER, "2026-09-26T13:05:00+08:00", "登录页改版的验收标准：\n\n- 首屏只保留账号、密码和一个主按钮\n- 错误提示贴在对应字段下方，不弹窗\n- 键盘可以走完全部流程，焦点顺序与视觉顺序一致\n- 深色主题逐项核对对比度"),
        m("t1", USER, "2026-09-26T13:06:00+08:00", "可以，按这个标准做。"),
        m("t2", BUILDER, "2026-09-26T13:20:00+08:00", "开始实现表单布局，先把账号和密码字段换成共享组件。"),
        m("t3", BUILDER, "2026-09-26T13:22:00+08:00", "主按钮用炭墨主操作样式，发送中的状态沿用按钮内的加载反馈，不另加遮罩。"),
        m("t4", REVIEWER, "2026-09-26T13:40:00+08:00", "布局看过了。「忘记密码」链接离主按钮太近，触控区重叠，建议下移 8 px。"),
        m("t5", USER, "2026-09-26T13:45:00+08:00", "同意，顺便把第三方登录收进「更多方式」。"),
        m("t6", BUILDER, "2026-09-26T13:58:00+08:00", "已调整间距，第三方登录收进了「更多方式」菜单，默认收起。"),
        m("t7", PLANNER, "2026-09-26T14:05:00+08:00", "埋点同步更新：登录方式的点击改为在菜单展开后上报，避免把展开误算成选择。"),
        m("t8", REVIEWER, "2026-09-26T14:12:00+08:00", "菜单的键盘操作正常，Esc 能关闭并把焦点还给触发按钮。"),
        m("t9", BUILDER, "2026-09-26T14:20:00+08:00", "深色主题的截图已更新到 Chat 文件。"),
        m("t10", USER, "2026-09-26T14:26:00+08:00", "间距现在可以了，就按这个。").reply(ReplySpec::To("t4")),
        m("t11", REVIEWER, "2026-09-26T14:29:00+08:00", "按最初的验收标准复查：焦点顺序已经正确；深色主题下错误提示的对比度只有 3.9:1，还差一点。").reply(ReplySpec::To("t0")),
    ]
}

const LONG_ORIGINAL: &str = "关于登录页的整体方案，我整理了三部分：第一部分是入口，保留账号密码为主路径，第三方登录收进「更多方式」；第二部分是错误处理，所有错误都贴在字段下方并给出下一步；第三部分是埋点，展开菜单与选择方式分开上报，并补充失败原因的维度，方便之后按原因看转化。";

fn edge_cases() -> Vec<Message> {
    vec![
        m(
            "e0",
            REVIEWER,
            "2026-09-26T14:08:00+08:00",
            "那条说明撤回了也没关系，我按现在的分支来审。",
        )
        .reply(ReplySpec::Deleted(BUILDER)),
        m(
            "e1",
            PLANNER,
            "2026-09-26T14:12:00+08:00",
            "这是上周定下的方案，我按它继续拆任务。",
        )
        .reply(ReplySpec::NotLoaded),
        m(
            "e2",
            BUILDER,
            "2026-09-26T14:16:00+08:00",
            "收到，照这个改。",
        )
        .reply(ReplySpec::Loading),
        m(
            "e3",
            REVIEWER,
            "2026-09-26T14:21:00+08:00",
            "第二部分我同意；第三部分的失败原因维度需要先和数据那边确认口径。",
        )
        .reply(ReplySpec::Long(0, LONG_ORIGINAL)),
        m("e4", USER, "2026-09-26T14:25:00+08:00", "好，就按这个来。")
            .reply(ReplySpec::Long(0, LONG_ORIGINAL)),
        m(
            "e5",
            BUILDER,
            "2026-09-26T14:28:00+08:00",
            "原消息删了，我先按当前分支继续。",
        )
        .reply(ReplySpec::Deleted(None)),
    ]
}

fn relative_times() -> Vec<Message> {
    vec![
        m(
            "r0",
            PLANNER,
            "2026-08-21T10:15:00+08:00",
            "上个月的迭代复盘：登录转化率 61%，主要流失在第三方授权页。",
        ),
        m(
            "r1",
            USER,
            "2026-09-03T16:20:00+08:00",
            "这个月把登录页改版排进来。",
        ),
        m(
            "r2",
            BUILDER,
            "2026-09-23T09:15:00+08:00",
            "登录页骨架开始搭建。",
        ),
        m(
            "r3",
            REVIEWER,
            "2026-09-25T18:40:00+08:00",
            "骨架审完，意见写在 PR 里了。",
        ),
        m("r4", BUILDER, "2026-09-26T12:05:00+08:00", "按意见改完了。"),
        m("r5", USER, "2026-09-26T14:27:00+08:00", "我看一下。"),
        m(
            "r6",
            PLANNER,
            "2026-09-26T14:29:50+08:00",
            "验收清单已更新。",
        ),
    ]
}

pub struct Story {
    state: String,
    messages: Vec<Message>,
    documents: Vec<MessageDocument>,
    tints: Vec<(String, usize)>,
    available_width: f32,
    scroll: ScrollHandle,
    highlighted: Option<usize>,
    pending_jump: Option<usize>,
    time: TimeFormat,
    text: ReplyText,
}

impl Story {
    pub fn new(state: &str, time: TimeFormat, cx: &mut Context<Self>) -> Self {
        let _ = cx;
        let messages = match state {
            "reply-offscreen" | "reply-highlight" | "phone-reply" => long_thread(),
            "reply-edge-cases" => edge_cases(),
            "relative-times" | "time-hover" | "phone-time-hold" => relative_times(),
            _ => conversation(),
        };
        let documents = messages
            .iter()
            .map(|message| match message.author {
                None => MessageDocument::plain(message.body),
                Some(_) => MessageDocument::parse(message.body),
            })
            .collect();
        let tints = crate::design::agent_tint_slots(
            messages
                .iter()
                .filter_map(|message| message.author.map(|a| AGENTS[a].id)),
        );
        let scroll = ScrollHandle::new();
        if matches!(state, "reply-offscreen" | "phone-reply") {
            scroll.scroll_to_bottom();
        }
        let highlight = (state == "reply-highlight").then_some(0);
        Self {
            state: state.into(),
            messages,
            documents,
            tints,
            available_width: 0.,
            scroll,
            highlighted: highlight,
            pending_jump: highlight,
            time,
            text: ReplyText::default(),
        }
    }

    fn tint(&self, agent: usize) -> usize {
        self.tints
            .iter()
            .find(|(id, _)| id == AGENTS[agent].id)
            .map(|(_, slot)| *slot)
            .unwrap_or(0)
    }

    fn quote_author(&self, author: Option<usize>) -> QuoteAuthor {
        match author {
            None => QuoteAuthor::User,
            Some(agent) => QuoteAuthor::Agent {
                name: AGENTS[agent].name.into(),
                tint: self.tint(agent),
            },
        }
    }

    /// Scrolls the original to the upper part of the view and washes it briefly.
    fn jump(&mut self, index: usize, cx: &mut Context<Self>) {
        self.highlighted = Some(index);
        self.pending_jump = Some(index);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1600))
                .await;
            let _ = this.update(cx, |v, cx| {
                if v.highlighted == Some(index) {
                    v.highlighted = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn apply_pending_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.pending_jump else {
            return;
        };
        // Row children follow a leading spacer, so message i is child i + 1.
        match self.scroll.bounds_for_item(index + 1) {
            Some(item) => {
                let view = self.scroll.bounds();
                let offset = self.scroll.offset();
                let max = self.scroll.max_offset();
                let target = (offset.y - (item.top() - view.top()) + px(24.))
                    .min(px(0.))
                    .max(-max.y);
                self.scroll.set_offset(point(offset.x, target));
                self.pending_jump = None;
                cx.notify();
            }
            None => cx.on_next_frame(window, |_, _, cx| cx.notify()),
        }
    }

    fn reply_for(&self, index: usize) -> Option<Reply> {
        let spec = self.messages[index].reply.clone()?;
        Some(match spec {
            ReplySpec::To(id) => match self.messages.iter().position(|m| m.id == id) {
                Some(original) if index.saturating_sub(original) <= MARKER_DISTANCE => {
                    Reply::Marker {
                        author: self.quote_author(self.messages[original].author),
                    }
                }
                Some(original) => Reply::Quote {
                    author: self.quote_author(self.messages[original].author),
                    excerpt: super::identity::excerpt(&self.documents[original].plain_text()),
                },
                None => Reply::NotLoaded,
            },
            ReplySpec::Deleted(author) => Reply::Deleted {
                author: author.map(|a| self.quote_author(Some(a))),
            },
            ReplySpec::NotLoaded => Reply::NotLoaded,
            ReplySpec::Loading => Reply::Loading,
            ReplySpec::Long(agent, text) => Reply::Quote {
                author: self.quote_author(Some(agent)),
                excerpt: super::identity::excerpt(text),
            },
        })
    }

    fn joins_previous(&self, index: usize) -> bool {
        if index == 0 || self.state == "reply-edge-cases" {
            return false;
        }
        let (previous, current) = (&self.messages[index - 1], &self.messages[index]);
        let seconds = |at: &str| {
            // Fixture instants share one offset; compare wall-clock seconds.
            let time = &at[11..19];
            let day: i64 = at[8..10].parse().unwrap_or(0);
            let parts: Vec<i64> = time.split(':').map(|p| p.parse().unwrap_or(0)).collect();
            day * 86_400 + parts[0] * 3600 + parts[1] * 60 + parts[2]
        };
        previous.author == current.author
            && previous.at[..7] == current.at[..7]
            && seconds(current.at) - seconds(previous.at) <= GROUP_WINDOW_SECONDS
    }
}

impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_pending_jump(window, cx);
        let content_width = (self.available_width - 48.).min(744.).max(1.);
        let devices: Vec<&str> = {
            let mut devices: Vec<&str> = self
                .messages
                .iter()
                .filter_map(|m| m.author.map(|a| AGENTS[a].device))
                .collect();
            devices.sort();
            devices.dedup();
            devices
        };
        let weak: gpui::WeakEntity<Self> = cx.entity().downgrade();
        let count = self.messages.len();
        let mut rows = Vec::with_capacity(count + 2);
        rows.push(div().h(px(12.)).into_any_element());
        for index in 0..count {
            let message = self.messages[index].clone();
            let starts = !self.joins_previous(index);
            let ends = index + 1 == count || !self.joins_previous(index + 1);
            let (label, full) = (self.time)(message.at);
            let reply = self.reply_for(index);
            let target = match &message.reply {
                Some(ReplySpec::To(id)) => self.messages.iter().position(|m| m.id == *id),
                _ => None,
            };
            let on_reply: Option<super::identity::Callback> = match (&reply, target) {
                (Some(_), Some(original)) => {
                    let weak = weak.clone();
                    Some(Rc::new(move |_, cx| {
                        let _ = weak.update(cx, |v, cx| v.jump(original, cx));
                    }))
                }
                (Some(Reply::NotLoaded), None) => {
                    let weak = weak.clone();
                    Some(Rc::new(move |_, cx| {
                        let _ = weak.update(cx, |v, cx| {
                            v.messages[index].reply = Some(ReplySpec::Loading);
                            cx.notify();
                        });
                    }))
                }
                _ => None,
            };
            let identity = message.author.map(|agent| AgentIdentity {
                name: AGENTS[agent].name.into(),
                tint: self.tint(agent),
                device: (devices.len() > 1).then(|| AGENTS[agent].device.into()),
                model: Some(AGENTS[agent].model.into()),
            });
            let row = Row {
                index,
                user: message.author.is_none(),
                document: &self.documents[index],
                content_width,
                selection: None,
                author_name: None,
                device: None,
                model: None,
                time: Some(label),
            }
            .render_with(
                window,
                Decorations {
                    identity,
                    grouping: Some(Grouping { starts, ends }),
                    reply,
                    reply_text: self.text.clone(),
                    on_reply,
                    time_full: Some(full),
                    highlighted: self.highlighted == Some(index),
                },
            );
            rows.push(row);
        }
        rows.push(div().h(px(16.)).into_any_element());
        let owner = cx.entity().downgrade();
        div()
            .relative()
            .size_full()
            .min_w_0()
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
                    .id("multi-agent-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
    }
}

/// Creates the story entity for `state`.
pub fn create(state: &str, time: TimeFormat, cx: &mut gpui::App) -> Entity<Story> {
    cx.new(|cx| Story::new(state, time, cx))
}
