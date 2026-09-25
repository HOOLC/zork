//! Presentation of a Chat transcript shared by the desktop and Android UIs.
//!
//! [`present`] projects the loaded transcript rows into what each row shows
//! beyond its body: whether it starts a group, its time label and where it
//! goes, the agent identity at a group head, the reply line, and the pairs of
//! a user comment batch. UIs render from this and keep only layout, hover,
//! scrolling and the "within one screen" measurement.
//!
//! Rules (the approved multi-agent design):
//!
//! - **Groups**: consecutive rows by the same author ([`author_key`]) at most
//!   [`GROUP_WINDOW`] apart form a group. A row without a time joins the
//!   previous row of the same author.
//! - **Time**: one label per group, at the end of the agent identity row
//!   ([`TimePlacement::Head`]) or under the last user bubble
//!   ([`TimePlacement::Tail`]); other rows show theirs on hover
//!   ([`TimePlacement::Hover`]). Labels come from [`crate::message_time`];
//!   re-run [`present`] after [`TranscriptPresentation::next_change_ms`].
//! - **Identity**: at a non-user group head, the agent avatar (its tint disc
//!   with the maker mark of the model that wrote the message, see
//!   [`maker_key`]; the initial when no model is known) and the name. Tints are assigned per Chat in first-appearance order
//!   ([`TintSlots`]). The device name is shown only when
//!   [`TranscriptPresentation::multi_device`] and the UI is not at phone width.
//! - **Reply line**: for rows with `reply_to`, see [`ReplyLine`] and
//!   [`quote_content`]. The omission rule has two parts: core decides
//!   [`ReplyLine::own_run`] (only this author's own messages in between);
//!   the UI decides whether the original starts within one screen above the
//!   reply, measured in the laid-out list (estimated heights when
//!   virtualized), never the scroll position, and re-evaluated on resize.
//!   The line is omitted only when both hold.
//! - **Comment batches**: user messages carrying the comment envelope are
//!   decoded into [`CommentLine`] pairs plus extra text.
//!
//! Tints follow first appearance in the rows given, so loading older history
//! can move them; pass the whole loaded transcript (Android passes its
//! window, where a target outside it reads as not loaded).
use crate::{
    api::{MessageMetadata, Role, TranscriptMessage},
    message_time::{self, TimeLocale},
    transcript::TranscriptLine,
};
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use zork_client_types::{
    chat::{ChatAgent, CHAT_AVATAR_AGENTS},
    comments::{self, CommentPair},
    navigation::{AgentAvatar, ChatAvatar},
};

mod quote;
pub use quote::{fallback_excerpt, quote_content, QuoteContent, QuoteSource};
pub use zork_client_types::agent_tint::{initial, preferred_slot, TintSlots, AGENT_TINT_SLOTS};

/// Longest gap between two messages of one group.
pub const GROUP_WINDOW: Duration = Duration::minutes(5);

/// One transcript row as input.
#[derive(Clone, Copy, Debug)]
pub struct PresentRow<'a> {
    pub role: Role,
    /// Raw payload (comment and file envelopes included).
    pub content: &'a str,
    pub metadata: &'a MessageMetadata,
}
impl<'a> From<&'a TranscriptLine> for PresentRow<'a> {
    fn from(line: &'a TranscriptLine) -> Self {
        let TranscriptLine::Message {
            role,
            content,
            metadata,
        } = line;
        Self {
            role: *role,
            content,
            metadata,
        }
    }
}

/// A device's names for identity details.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLabel {
    pub display: String,
    #[serde(default)]
    pub machine: Option<String>,
}

/// Inputs besides the rows.
#[derive(Clone, Debug)]
pub struct PresentOptions {
    /// The viewer's clock, in the device's local offset.
    pub now: DateTime<FixedOffset>,
    pub locale: TimeLocale,
    /// Older history exists beyond the first row, so a missing reply target
    /// is "not loaded" rather than deleted.
    pub has_older: bool,
    /// Device origin (`metadata.device`) → names.
    pub devices: HashMap<String, DeviceLabel>,
    /// Agent id → current model, used when a row does not carry the model
    /// that wrote it (older Stations, remote authors), e.g. from the Chat's
    /// `agents` or its participants.
    pub agent_models: HashMap<String, String>,
}

/// Where a row's time label is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimePlacement {
    /// End of the agent identity row (agent group head).
    Head,
    /// Under the last bubble of a user group.
    Tail,
    /// Only on hover at the row end.
    Hover,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TimeLabel {
    /// Short relative label ("3 分钟前").
    pub label: String,
    /// Full timestamp for hover / long-press.
    pub full: String,
    pub placement: TimePlacement,
}

/// A message author as shown in identity rows and quote lines.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuthorRef {
    pub name: String,
    pub user: bool,
    pub agent_id: Option<String>,
    /// Tint slot and initial of the small disc; `None` for the user (no disc).
    pub tint: Option<usize>,
    pub initial: Option<String>,
    /// Model maker mark drawn in the disc instead of the initial
    /// ([`maker_key`]); `None` draws the initial.
    pub maker: Option<String>,
}

/// Agent identity at a group head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Identity {
    #[serde(flatten)]
    pub author: AuthorRef,
    /// Device origin and its names, when known.
    pub device: Option<String>,
    pub device_name: Option<String>,
    pub machine: Option<String>,
    pub model: Option<String>,
    /// Hover detail on the name: "名称 · 设备（机器名） · 模型", omitting
    /// unknown parts.
    pub detail: String,
}

/// State of a reply target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    /// In the rows: clicking jumps to it.
    Linked,
    /// Older than the loaded history: "回复 更早的消息 · 尚未加载 · 点击加载";
    /// the first click loads older history keeping the reading position.
    NotLoaded,
    /// Gone: "回复 X · 原消息已删除", not clickable.
    Deleted,
}

/// Where the reply line sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPlacement {
    /// In the identity row after the name, before the time (group head).
    InHead,
    /// On its own line above the body (non-head rows, user bubbles).
    AboveBody,
}

/// The single muted reply line: ↩ 回复 + target author + quote content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplyLine {
    pub target_id: String,
    pub state: TargetState,
    /// Row index of the target within the given rows.
    pub target_index: Option<usize>,
    /// Target author; `None` when unknown (not loaded, or deleted).
    pub target: Option<AuthorRef>,
    /// `None` unless `state` is `Linked`.
    pub content: Option<QuoteContent>,
    pub placement: ReplyPlacement,
    /// Only this author's own messages lie between target and reply. The UI
    /// omits the line when this holds and the target starts within one screen
    /// above the reply; if the line was the only content of a non-head
    /// identity row, that row collapses too.
    pub own_run: bool,
}

/// One pair of a user comment batch: a quote line then the reply text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommentLine {
    #[serde(flatten)]
    pub pair: CommentPair,
    /// Source author; name falls back to "消息"/"message" when unrecorded.
    pub source: AuthorRef,
    /// Whether the source message is in the rows (clicking jumps and marks
    /// `pair.quote`), older than them, or gone.
    pub state: TargetState,
    pub source_index: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CommentsView {
    pub pairs: Vec<CommentLine>,
    /// Optional extra text, rendered after the pairs.
    pub extra_text: String,
}

/// What one row shows besides its body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RowPresentation {
    pub id: Option<String>,
    pub author: String,
    pub user: bool,
    pub group_head: bool,
    /// Last row of its group (user groups put the time here).
    pub group_tail: bool,
    pub time: Option<TimeLabel>,
    /// Present on non-user group heads.
    pub identity: Option<Identity>,
    pub reply: Option<ReplyLine>,
    pub comments: Option<CommentsView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TranscriptPresentation {
    pub rows: Vec<RowPresentation>,
    /// Agents in these rows run on more than one device.
    pub multi_device: bool,
    /// Milliseconds until some label changes; `None` when all are stable.
    pub next_change_ms: Option<i64>,
    /// Tint slots in first-appearance order.
    pub tints: TintSlots,
}

/// Stable author identity used for grouping and the omission rule.
pub fn author_key(role: Role, metadata: &MessageMetadata) -> String {
    metadata.author_key(role)
}

/// Whether `current` starts a new group after `previous` (same author and at
/// most [`GROUP_WINDOW`] apart continue it).
pub fn starts_group(previous: Option<PresentRow<'_>>, current: PresentRow<'_>) -> bool {
    let Some(previous) = previous else {
        return true;
    };
    if author_key(previous.role, previous.metadata) != author_key(current.role, current.metadata) {
        return true;
    }
    match (time_of(previous.metadata), time_of(current.metadata)) {
        (Some(before), Some(after)) => (after - before).abs() > GROUP_WINDOW,
        _ => false,
    }
}

/// The adjacent part of the omission rule over author keys: the reply at
/// `reply` answers `target` with only its own author's rows in between.
pub fn only_own_between(authors: &[String], target: usize, reply: usize) -> bool {
    target < reply
        && reply < authors.len()
        && authors[target + 1..reply]
            .iter()
            .all(|author| author == &authors[reply])
}

fn time_of(metadata: &MessageMetadata) -> Option<DateTime<Utc>> {
    metadata
        .created_at
        .as_deref()
        .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.with_timezone(&Utc))
}

/// Agents are keyed by id; legacy assistant rows without one by name.
fn tint_key(metadata: &MessageMetadata) -> &str {
    metadata
        .author_agent_id
        .as_deref()
        .or(metadata.author_name.as_deref())
        .unwrap_or("")
}

fn user_name(locale: TimeLocale) -> String {
    if locale == TimeLocale::ZhCn {
        "你"
    } else {
        "You"
    }
    .into()
}

fn agent_name(metadata: &MessageMetadata, locale: TimeLocale) -> String {
    metadata
        .author_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if locale == TimeLocale::ZhCn {
                "助手"
            } else {
                "Assistant"
            }
            .into()
        })
}

/// The maker mark for a model: its `model_catalog::MAKERS` key, `generic`
/// for a model the catalog does not recognize, `None` without a model (the
/// disc then shows the initial).
pub fn maker_key(model: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|model| !model.is_empty())?;
    Some(
        crate::model_catalog::maker(model, None)
            .unwrap_or("generic")
            .to_owned(),
    )
}

/// One Agent's avatar within a Chat's tint assignment.
pub fn agent_avatar(
    agent_id: &str,
    name: Option<&str>,
    model: Option<&str>,
    tints: &TintSlots,
) -> AgentAvatar {
    AgentAvatar {
        agent_id: agent_id.to_owned(),
        maker: maker_key(model),
        tint: tints.slot(agent_id),
        initial: initial(name.filter(|n| !n.trim().is_empty()).unwrap_or(agent_id)),
    }
}

/// A Chat's stacked avatar from its Agents in first-appearance order: the
/// first [`CHAT_AVATAR_AGENTS`] avatars, then `more` for the rest (all
/// `agent_count` Agents, never fewer than listed). Tints follow the same
/// first-appearance assignment as the transcript.
pub fn chat_avatar(agents: &[ChatAgent], agent_count: u64) -> ChatAvatar {
    let tints = TintSlots::assign(agents.iter().map(|agent| agent.id.as_str()));
    let shown: Vec<AgentAvatar> = agents
        .iter()
        .take(CHAT_AVATAR_AGENTS)
        .map(|agent| {
            agent_avatar(
                &agent.id,
                agent.name.as_deref(),
                agent.model.as_deref(),
                &tints,
            )
        })
        .collect();
    let total = agent_count.max(agents.len() as u64);
    ChatAvatar {
        more: total.saturating_sub(shown.len() as u64),
        agents: shown,
    }
}

fn author_ref(row: PresentRow<'_>, tints: &TintSlots, options: &PresentOptions) -> AuthorRef {
    let locale = options.locale;
    if row.role == Role::User {
        return AuthorRef {
            name: user_name(locale),
            user: true,
            agent_id: None,
            tint: None,
            initial: None,
            maker: None,
        };
    }
    let name = agent_name(row.metadata, locale);
    let id = row.metadata.author_agent_id.clone();
    // The model that wrote the row; otherwise the Agent's known current model.
    let model = row
        .metadata
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .or_else(|| {
            id.as_deref()
                .and_then(|id| options.agent_models.get(id))
                .map(String::as_str)
        });
    AuthorRef {
        tint: Some(tints.slot(tint_key(row.metadata))),
        initial: Some(initial(&name)),
        maker: maker_key(model),
        name,
        user: false,
        agent_id: id,
    }
}

/// Projects rows (oldest first) into their presentation.
pub fn present<'a>(
    rows: impl IntoIterator<Item = PresentRow<'a>>,
    options: &PresentOptions,
) -> TranscriptPresentation {
    let rows: Vec<PresentRow<'a>> = rows.into_iter().collect();
    let locale = options.locale;
    let authors: Vec<String> = rows
        .iter()
        .map(|row| author_key(row.role, row.metadata))
        .collect();
    let index: HashMap<&str, usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(at, row)| row.metadata.id.as_deref().map(|id| (id, at)))
        .collect();
    let tints = TintSlots::assign(
        rows.iter()
            .filter(|row| row.role != Role::User)
            .map(|row| tint_key(row.metadata)),
    );
    let devices: HashSet<&str> = rows
        .iter()
        .filter(|row| row.role != Role::User)
        .filter_map(|row| row.metadata.device.as_deref())
        .collect();
    let heads: Vec<bool> = (0..rows.len())
        .map(|at| starts_group(at.checked_sub(1).map(|p| rows[p]), rows[at]))
        .collect();
    let mut next_change: Option<Duration> = None;
    let mut out = Vec::with_capacity(rows.len());
    for (at, row) in rows.iter().copied().enumerate() {
        let head = heads[at];
        let tail = heads.get(at + 1).copied().unwrap_or(true);
        let user = row.role == Role::User;
        let time = time_of(row.metadata).map(|instant| {
            let formatted = message_time::format(instant, options.now, locale);
            if let Some(wait) = message_time::next_change(instant, options.now) {
                next_change = Some(next_change.map_or(wait, |known| known.min(wait)));
            }
            TimeLabel {
                label: formatted.label,
                full: formatted.full,
                placement: match (user, head, tail) {
                    (false, true, _) => TimePlacement::Head,
                    (true, _, true) => TimePlacement::Tail,
                    _ => TimePlacement::Hover,
                },
            }
        });
        let identity = (!user && head).then(|| identity(row, &tints, options));
        let reply = row.metadata.reply_to.as_deref().map(|target_id| {
            let target_index = index.get(target_id).copied();
            let target = target_index.map(|t| rows[t]);
            ReplyLine {
                target_id: target_id.to_owned(),
                state: match (target_index, options.has_older) {
                    (Some(_), _) => TargetState::Linked,
                    (None, true) => TargetState::NotLoaded,
                    (None, false) => TargetState::Deleted,
                },
                target_index,
                target: target.map(|t| author_ref(t, &tints, options)),
                content: target.map(|t| {
                    quote_content(
                        &comments::quotable_text(t.content),
                        row.metadata.reply_quote().as_ref(),
                    )
                }),
                placement: if head && !user {
                    ReplyPlacement::InHead
                } else {
                    ReplyPlacement::AboveBody
                },
                own_run: target_index.is_some_and(|t| only_own_between(&authors, t, at)),
            }
        });
        let comments = if user {
            comments::decode_batch(row.content).map(|batch| CommentsView {
                pairs: batch
                    .pairs
                    .into_iter()
                    .map(|pair| comment_line(pair, &rows, &index, &tints, options))
                    .collect(),
                extra_text: batch.extra_text,
            })
        } else {
            None
        };
        out.push(RowPresentation {
            id: row.metadata.id.clone(),
            author: authors[at].clone(),
            user,
            group_head: head,
            group_tail: tail,
            time,
            identity,
            reply,
            comments,
        });
    }
    TranscriptPresentation {
        rows: out,
        multi_device: devices.len() > 1,
        next_change_ms: next_change.map(|wait| wait.num_milliseconds().max(0)),
        tints,
    }
}

fn identity(row: PresentRow<'_>, tints: &TintSlots, options: &PresentOptions) -> Identity {
    let author = author_ref(row, tints, options);
    let device = row.metadata.device.clone();
    let label = device.as_deref().and_then(|d| options.devices.get(d));
    let model = row
        .metadata
        .model
        .clone()
        .filter(|model| !model.trim().is_empty());
    let mut detail = author.name.clone();
    if let Some(label) = label {
        detail.push_str(" · ");
        detail.push_str(&label.display);
        if let Some(machine) = label.machine.as_deref().filter(|m| *m != label.display) {
            detail.push_str(&format!("（{machine}）"));
        }
    }
    if let Some(model) = &model {
        detail.push_str(" · ");
        detail.push_str(model);
    }
    Identity {
        author,
        device,
        device_name: label.map(|l| l.display.clone()),
        machine: label.and_then(|l| l.machine.clone()),
        model,
        detail,
    }
}

fn comment_line(
    pair: CommentPair,
    rows: &[PresentRow<'_>],
    index: &HashMap<&str, usize>,
    tints: &TintSlots,
    options: &PresentOptions,
) -> CommentLine {
    let source_index = pair
        .message_id
        .as_deref()
        .and_then(|id| index.get(id).copied());
    // The loaded source knows its author best; otherwise use what the draft
    // recorded.
    let source = match source_index {
        Some(at) => author_ref(rows[at], tints, options),
        None => match pair.author_agent_id.as_deref() {
            Some(agent) => {
                let name = pair.author.clone().unwrap_or_else(|| agent.to_owned());
                AuthorRef {
                    tint: Some(tints.slot(agent)),
                    initial: Some(initial(&name)),
                    maker: maker_key(options.agent_models.get(agent).map(String::as_str)),
                    name,
                    user: false,
                    agent_id: Some(agent.to_owned()),
                }
            }
            None => AuthorRef {
                name: pair.author.clone().unwrap_or_else(|| {
                    if options.locale == TimeLocale::ZhCn {
                        "消息"
                    } else {
                        "message"
                    }
                    .into()
                }),
                user: false,
                agent_id: None,
                tint: None,
                initial: None,
                maker: None,
            },
        },
    };
    CommentLine {
        state: match (source_index, options.has_older) {
            (Some(_), _) => TargetState::Linked,
            (None, true) if pair.message_id.is_some() => TargetState::NotLoaded,
            _ => TargetState::Deleted,
        },
        source_index,
        source,
        pair,
    }
}

/// JSON request of the Android bridge (`NativeBridge.messagePresentation`).
#[derive(Clone, Debug, Deserialize)]
pub struct PresentRequest {
    /// Conversation rows exactly as the conversation observation sends them.
    pub rows: Vec<TranscriptMessage>,
    /// Viewer clock in Unix milliseconds.
    pub now_ms: i64,
    /// The device's current UTC offset in minutes.
    #[serde(default)]
    pub utc_offset_minutes: i32,
    /// BCP 47 tag; `zh…` selects Chinese.
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub has_older: bool,
    #[serde(default)]
    pub devices: HashMap<String, DeviceLabel>,
    /// Agent id → current model (fallback for rows without one).
    #[serde(default)]
    pub agent_models: HashMap<String, String>,
}

/// Pure JSON entry point for platform bridges.
pub fn handle(request: PresentRequest) -> anyhow::Result<serde_json::Value> {
    let offset = FixedOffset::east_opt(request.utc_offset_minutes * 60)
        .ok_or_else(|| anyhow::anyhow!("invalid_utc_offset"))?;
    let now = offset
        .timestamp_millis_opt(request.now_ms)
        .single()
        .ok_or_else(|| anyhow::anyhow!("invalid_now"))?;
    let options = PresentOptions {
        now,
        locale: TimeLocale::from_tag(&request.locale),
        has_older: request.has_older,
        devices: request.devices,
        agent_models: request.agent_models,
    };
    let rows = request.rows.iter().map(
        |TranscriptMessage::Message {
             role,
             content,
             metadata,
         }| PresentRow {
            role: *role,
            content,
            metadata,
        },
    );
    Ok(serde_json::to_value(present(rows, &options))?)
}

#[cfg(test)]
mod tests;
