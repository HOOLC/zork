use super::*;
use zork_client_core::preferences::{read_view_state, save_view_state, ViewState};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct ReadingPosition {
    item: usize,
    offset: f32,
    following: bool,
    anchors: Vec<String>,
}
fn marker(role: crate::api::Role, content: &str) -> String {
    zork_client_core::transcript::reading_anchor(role, content)
}
fn anchor_index(markers: &[String], position: &ReadingPosition) -> Option<usize> {
    if position.anchors.is_empty() {
        return None;
    }
    markers
        .windows(position.anchors.len())
        .enumerate()
        .filter(|(_, window)| *window == position.anchors)
        .min_by_key(|(index, _)| index.abs_diff(position.item))
        .map(|(index, _)| index)
}
impl RootView {
    pub(super) fn save_reading_position(&self) {
        let Some(id) = &self.selected_session else {
            return;
        };
        let offset = self.transcript_list.logical_scroll_top();
        let anchors = self
            .lines
            .iter()
            .skip(offset.item_ix)
            .take(2)
            .map(|line| match line {
                TranscriptLine::Message { role, content, .. } => marker(*role, content),
            })
            .collect();
        self.persist_cache(
            ViewState::Reading(id.clone()),
            &ReadingPosition {
                item: offset.item_ix,
                offset: offset.offset_in_item.as_f32(),
                following: self.transcript_list.is_following_tail(),
                anchors,
            },
        );
    }
    pub(super) fn restore_reading_position(&self) {
        let Some(position) = self
            .selected_session
            .as_ref()
            .and_then(|id| self.read_cache::<ReadingPosition>(ViewState::Reading(id.clone())))
        else {
            return;
        };
        if position.following {
            return;
        }
        let markers = self
            .lines
            .iter()
            .map(|line| match line {
                TranscriptLine::Message { role, content, .. } => marker(*role, content),
            })
            .collect::<Vec<_>>();
        if let Some(item_ix) = anchor_index(&markers, &position) {
            self.transcript_list.pause_following_tail();
            self.transcript_list.scroll_to(gpui::ListOffset {
                item_ix,
                offset_in_item: px(position.offset),
            });
        }
    }
    /// Keep the cached reading anchor in the refreshed window, fetching only
    /// the older pages needed to bridge a long disconnect. No session is created.
    pub(super) fn persist_cache<T: serde::Serialize>(&self, key: ViewState, value: &T) {
        if let Some((store, node)) = &self.local_cache {
            if let Err(error) = save_view_state(store, node, key, value) {
                eprintln!("client cache: {error}");
            }
        }
    }
    pub(super) fn read_cache<T: serde::de::DeserializeOwned>(&self, key: ViewState) -> Option<T> {
        self.local_cache
            .as_ref()
            .and_then(|(store, node)| read_view_state(store, node, key).ok().flatten())
    }
    pub(super) fn save_draft(&self, cx: &Context<Self>) {
        if let Some(id) = &self.selected_session {
            if let Err(error) = self
                .core_device
                .edit_draft(id, self.composer_input.read(cx).value().to_owned())
            {
                eprintln!("client draft: {error}");
            }
        }
    }
    pub(super) fn restore_draft(&mut self, cx: &mut Context<Self>) {
        self.file_ui.draft = Default::default();
        self.file_ui.messages.borrow_mut().clear();
        self.comment_popover = None;
        self.comment_editor
            .update(cx, |editor, cx| editor.reset(cx));
        self.composer_surface.scene = Default::default();
        self.close_conversation_artifact();
        self.transcript_selection.borrow_mut().clear();
        self.draft_task = None;
        let mut changes = self
            .selected_session
            .as_ref()
            .map(|id| self.core_device.subscribe_draft(id));
        self.draft_state = changes
            .as_mut()
            .map(|changes| changes.snapshot())
            .unwrap_or_default();
        self.file_ui.draft_files.reset(&self.draft_state.files);
        let text = self.draft_state.text.clone();
        self.composer_input
            .update(cx, |input, cx| input.reset_value(text, cx));
        if let (Some(id), Some(mut changes)) = (self.selected_session.clone(), changes) {
            self.draft_task = Some(cx.spawn(async move |this, cx| {
                while let Some(draft) = changes.changed().await {
                    if this
                        .update(cx, |view, cx| {
                            if view.selected_session.as_ref() != Some(&id) {
                                return;
                            }
                            let text = draft.text.clone();
                            // One authoritative projection for picker, paste,
                            // reuse, removal, and submit. Attachment motion diffs
                            // this snapshot against its last displayed frame.
                            view.draft_state = draft;
                            view.composer_input
                                .update(cx, |input, cx| input.set_value(text, cx));
                            zork_ui::components::region::invalidate(cx, &["composer", "home"]);
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }));
        }
    }
    pub(super) fn refresh_queued(&mut self) {
        let state = self.core_device.recover_outbox();
        self.queued_count = state.items.len();
        #[cfg(feature = "headless-bench")]
        self.benchmark_delivery_snapshot(&state);
    }
    pub(super) fn queue_message(
        &mut self,
        session_id: String,
        text: String,
        cx: &mut Context<Self>,
    ) {
        match self.core_device.submit_draft(&session_id, &text) {
            Ok(_) => {
                // The core draft subscription publishes the cleared text and
                // attachments together. Do not race it with a second UI write.
                self.refresh_queued();
            }
            Err(error) => self.error = Some(format!("保存待发送消息失败：{error}")),
        }
        zork_ui::components::region::invalidate(cx, &["composer", "transcript"]);
    }
    pub(super) fn resend_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(error) = self.core_device.retry_delivery(id) {
            self.error = Some(error.to_string());
        }
        zork_ui::components::region::invalidate(cx, &["composer", "transcript"]);
    }
    pub(super) fn delete_failed_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Err(error) = self.core_device.delete_failed_delivery(id) {
            self.error = Some(error.to_string());
        }
        zork_ui::components::region::invalidate(cx, &["composer", "transcript"]);
    }
    pub(super) fn start_delivery(&mut self, cx: &mut Context<Self>) {
        if self.delivery_task.is_some() || self.local_cache.is_none() {
            return;
        }
        #[cfg(not(feature = "headless-bench"))]
        self.core_device.start_delivery();
        #[cfg(feature = "headless-bench")]
        if !self.benchmark_offline {
            self.core_device.start_delivery();
        }
        let mut updates = self.core_device.subscribe_outbox();
        updates.snapshot();
        self.refresh_queued();
        self.delivery_task = Some(cx.spawn(async move |this, cx| {
            while let Some(state) = updates.changed().await {
                if this
                    .update(cx, |v, cx| {
                        v.queued_count = state.items.len();
                        #[cfg(feature = "headless-bench")]
                        if v.benchmark_offline && v.core_conversation.is_none() {
                            v.benchmark_delivery_snapshot(&state);
                            zork_ui::components::region::invalidate(cx, &["transcript"]);
                        }
                        zork_ui::components::region::invalidate(cx, &["composer"]);
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    pub(super) fn connection_hint(&self) -> String {
        if self.access_revoked {
            return self.locale.text("device_access_revoked").into();
        }
        let mut hint = self.locale.text("device_offline_hint").to_owned();
        if let Some(confirmed) = &self.last_confirmed_at {
            let time = chrono::DateTime::parse_from_rfc3339(confirmed)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%m-%d %H:%M:%S")
                        .to_string()
                })
                .unwrap_or_else(|_| confirmed.clone());
            hint.push_str(
                &self
                    .locale
                    .text("device_last_confirmed")
                    .replace("{time}", &time),
            );
        }
        hint
    }
}
