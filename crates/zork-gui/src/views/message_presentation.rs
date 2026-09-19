//! Per-conversation presentation: one-shot arrivals, interruptible tail motion,
//! and full messages hosted in a centered reading dialog.
use super::*;
use std::time::Instant;

#[derive(Default)]
pub(super) struct MessageMotion {
    pub arrivals: HashMap<String, Instant>,
    pub unread: usize,
    pub scroll: Option<Task<()>>,
}

impl RootView {
    pub(super) fn track_message_arrivals(
        &mut self,
        arrivals: &zork_client_core::state::MessageArrivals,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        self.message_motion
            .arrivals
            .retain(|_, time| now.duration_since(*time) < Duration::from_millis(250));
        if arrivals.count == 0 {
            return;
        }
        for id in &arrivals.ids {
            self.message_motion.arrivals.insert(id.clone(), now);
        }
        // Following means: actually following, mid-animation, at the scrollbar end,
        // or sitting on the last message's header (our new tail target for tall messages).
        let is_at_end = self.transcript_list.is_scrolled_to_end().unwrap_or(false);
        let is_on_last_header = !self.lines.is_empty()
            && self.transcript_list.logical_scroll_top().item_ix
                == self.lines.len().saturating_sub(1);
        let following = self.transcript_list.is_following_tail()
            || self.message_motion.scroll.is_some()
            || is_at_end
            || is_on_last_header;
        if following {
            self.animate_message_tail(cx);
        } else {
            self.message_motion.unread = self
                .message_motion
                .unread
                .saturating_add(usize::try_from(arrivals.count).unwrap_or(usize::MAX));
        }
    }

    pub(super) fn animate_message_tail(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.message_motion.unread) > 0 {
            zork_ui::components::region::invalidate(cx, &["composer"]);
        }
        // Determine the first newly arrived message so we can scroll its
        // header into view instead of only bringing the absolute bottom
        // into view (which clips tall headers).
        let first_new = self
            .lines
            .len()
            .saturating_sub(self.message_motion.arrivals.len().max(1));
        let target_offset = gpui::ListOffset {
            item_ix: first_new.min(self.lines.len().saturating_sub(1)),
            offset_in_item: px(0.),
        };
        if cx.reduce_motion() {
            // Reduce-motion: immediate jump to the new message's header.
            // For small messages header==bottom, this matches classic Tail; for tall
            // messages it keeps the header visible without animation.
            self.transcript_list.set_follow_mode(FollowMode::Normal);
            self.transcript_list.scroll_to(target_offset);
            let target_y = -self
                .transcript_list
                .scroll_px_offset_for_scrollbar()
                .y
                .as_f32();
            let max_y = self.transcript_list.max_offset_for_scrollbar().y.as_f32();
            if target_y >= max_y - 1. {
                // Small message: restore Tail semantics so is_following_tail stays true
                self.transcript_list.set_follow_mode(FollowMode::Tail);
                self.transcript_list.scroll_to_end();
            }
            zork_ui::components::region::invalidate(cx, &["transcript"]);
            return;
        }
        if self.message_motion.scroll.is_some() {
            return;
        }
        let start = -self
            .transcript_list
            .scroll_px_offset_for_scrollbar()
            .y
            .as_f32();
        // Compute pixel target for the first new message's top via a
        // temporary scroll_to; this accounts for variable item heights
        // without adding vendor dependencies.
        let start_offset = self.transcript_list.logical_scroll_top();
        self.transcript_list.set_follow_mode(FollowMode::Normal);
        self.transcript_list.scroll_to(target_offset);
        let target = -self
            .transcript_list
            .scroll_px_offset_for_scrollbar()
            .y
            .as_f32();
        self.transcript_list.scroll_to(start_offset);
        self.message_motion.scroll = Some(cx.spawn(async move |this, cx| {
            let began = Instant::now();
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let done = this
                    .update(cx, |v, cx| {
                        let t = (began.elapsed().as_secs_f32() / 0.20).min(1.);
                        let current = -v
                            .transcript_list
                            .scroll_px_offset_for_scrollbar()
                            .y
                            .as_f32();
                        let next = start + (target - start).max(0.) * (1. - (1. - t).powi(3));
                        v.transcript_list.scroll_by(px((next - current).max(0.)));
                        if t >= 1. {
                            let final_ix = v
                                .lines
                                .len()
                                .saturating_sub(v.message_motion.arrivals.len().max(1))
                                .min(v.lines.len().saturating_sub(1));
                            let max_y = v.transcript_list.max_offset_for_scrollbar().y.as_f32();
                            if target >= max_y - 1. {
                                v.transcript_list.set_follow_mode(FollowMode::Tail);
                                v.transcript_list.scroll_to_end();
                            } else {
                                v.transcript_list.set_follow_mode(FollowMode::Normal);
                                v.transcript_list.scroll_to(gpui::ListOffset {
                                    item_ix: final_ix,
                                    offset_in_item: px(0.),
                                });
                            }
                            v.message_motion.scroll = None;
                        }
                        zork_ui::components::region::invalidate(cx, &["transcript"]);
                        t >= 1.
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        }));
    }

    pub(super) fn interrupt_message_scroll(&mut self, cx: &mut Context<Self>) {
        let interrupted = self.message_motion.scroll.take().is_some();
        let root = cx.entity().downgrade();
        // GPUI invokes the wheel callback while borrowing ListState mutably.
        // Cancel immediately, then restore ordinary follow tracking after it exits.
        cx.defer(move |cx| {
            let _ = root.update(cx, |v, cx| {
                if interrupted {
                    let offset = v.transcript_list.logical_scroll_top();
                    v.transcript_list.set_follow_mode(FollowMode::Tail);
                    v.transcript_list.scroll_to(offset);
                }
                if v.transcript_list.is_following_tail() && v.message_motion.unread > 0 {
                    v.message_motion.unread = 0;
                    zork_ui::components::region::invalidate(cx, &["composer"]);
                }
            });
        });
    }

    pub(super) fn open_message_reader(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(TranscriptLine::Message {
            role,
            content,
            metadata,
        }) = self.lines.get(index)
        else {
            return;
        };
        let text = crate::comments::display_text(content);
        let document = crate::components::message::message_document(role, content);
        let source = crate::comments::CommentSource {
            session_id: self.selected_session.clone().unwrap_or_default(),
            message_id: metadata.id.clone(),
            author: metadata.author_name.clone(),
            author_agent_id: metadata.author_agent_id.clone(),
            quote: String::new(),
        };
        let links = self.message_link_handler(cx);
        let title = self.locale.text("message_full_title").into();
        let copy = self.locale.text("message_copy_full").into();
        self.message_reader.update(cx, |reader, cx| {
            reader.configure(title, copy, links);
            reader.open(
                zork_ui::components::message_reader::Content::new(source, text, document),
                cx,
            );
        });
        zork_ui::components::region::invalidate_all(cx);
    }
}
