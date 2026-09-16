//! Composer actions are product policy. Views render these capabilities.
#[cfg(not(target_family = "wasm"))]
use crate::api::{SessionStatus, SessionSummary};

#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct ComposerState {
    pub editable: bool,
    pub stop: bool,
    pub enabled: bool,
}

pub fn has_content(text: &str, attachments: usize) -> bool {
    !text.trim().is_empty() || attachments > 0
}

#[cfg(not(target_family = "wasm"))]
pub fn can_stop(session: &SessionSummary) -> bool {
    crate::conversation::can_send(session) && session.task.is_none()
}

/// The desktop's primary action interrupts a running direct conversation.
/// Task composers submit comments to their owning Leader instead.
#[cfg(not(target_family = "wasm"))]
pub fn interrupting(
    session: Option<&SessionSummary>,
    text: &str,
    attachments: usize,
    stopping: bool,
    preparing: usize,
) -> ComposerState {
    desktop_state(
        session.map(|s| DesktopSession {
            editable: crate::conversation::can_send(s),
            can_stop: can_stop(s),
            running: s.status == SessionStatus::Working,
        }),
        text,
        attachments,
        stopping,
        preparing,
    )
}

/// Portable input to the desktop policy. Authority and conversation type come
/// from core projections; UI never constructs or evaluates this input.
#[derive(Clone, Copy)]
pub struct DesktopSession {
    pub editable: bool,
    pub can_stop: bool,
    pub running: bool,
}
pub fn desktop_state(
    session: Option<DesktopSession>,
    text: &str,
    attachments: usize,
    stopping: bool,
    preparing: usize,
) -> ComposerState {
    let editable = session.is_some_and(|s| s.editable);
    let working = session.is_some_and(|s| s.editable && s.can_stop && s.running);
    ComposerState {
        editable,
        stop: working || stopping,
        enabled: editable
            && !stopping
            && (working || (has_content(text, attachments) && preparing == 0)),
    }
}

/// Mobile accepts an additional queued draft while a direct conversation runs.
/// It offers Stop only when there is no input and the core grants cancellation.
#[derive(serde::Deserialize)]
pub struct QueuedComposer {
    pub text: String,
    pub attachments: usize,
    pub can_send: bool,
    pub can_stop: bool,
    pub running: bool,
    pub online: bool,
    pub busy: bool,
}
impl QueuedComposer {
    pub fn state(&self) -> ComposerState {
        let content = has_content(&self.text, self.attachments);
        let stop = self.can_stop && self.running && !content;
        ComposerState {
            editable: self.can_send,
            stop,
            enabled: !self.busy
                && if stop {
                    self.online
                } else {
                    self.can_send && content
                },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queued_draft_and_read_only_conversations_do_not_trigger_stop() {
        let mut input = QueuedComposer {
            text: "new request".into(),
            attachments: 0,
            can_send: true,
            can_stop: true,
            running: true,
            online: true,
            busy: false,
        };
        assert!(!input.state().stop);
        assert!(input.state().enabled);
        input.text.clear();
        assert!(input.state().stop);
        input.online = false;
        assert!(!input.state().enabled);
        input.can_stop = false;
        assert!(!input.state().stop);
        input.attachments = 1;
        assert!(input.state().enabled);
        input.can_send = false;
        assert!(!input.state().enabled);
    }
}

/// Offline presentation fixtures; uses the same product policy as desktop.
pub mod fixture;
