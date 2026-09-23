//! Per-conversation presentation: one-shot arrivals and interruptible tail motion.
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
        following: bool,
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
        if following {
            let target = arrivals
                .ids
                .iter()
                .filter_map(|id| self.transcript_lookup.index_of(id))
                .min()
                .unwrap_or_else(|| {
                    self.lines.len().saturating_sub(
                        usize::try_from(arrivals.count).unwrap_or(usize::MAX).max(1),
                    )
                });
            self.animate_message_tail(Some(target), cx);
        } else {
            self.message_motion.unread = self
                .message_motion
                .unread
                .saturating_add(usize::try_from(arrivals.count).unwrap_or(usize::MAX));
        }
    }

    pub(super) fn animate_message_tail(&mut self, target: Option<usize>, cx: &mut Context<Self>) {
        let unread = std::mem::take(&mut self.message_motion.unread);
        if unread > 0 {
            zork_ui::components::region::invalidate(cx, &["composer"]);
        }
        let first_new = target.unwrap_or_else(|| self.lines.len().saturating_sub(unread.max(1)));
        let target_offset = gpui::ListOffset {
            item_ix: first_new.min(self.lines.len().saturating_sub(1)),
            offset_in_item: px(0.),
        };
        self.message_motion.scroll.take();
        if cx.reduce_motion() {
            // Stay in normal mode so a newly inserted tall row cannot restore
            // tail mode from its still-estimated height and jump to the bottom.
            self.transcript_list.set_follow_mode(FollowMode::Normal);
            self.transcript_list.scroll_to(target_offset);
            self.message_motion.scroll = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(32))
                    .await;
                let _ = this.update(cx, |v, cx| {
                    if v.transcript_list.is_scrolled_to_end().unwrap_or(false) {
                        v.transcript_list.set_follow_mode(FollowMode::Tail);
                        v.transcript_list.scroll_to_end();
                    }
                    v.message_motion.scroll = None;
                    zork_ui::components::region::invalidate(cx, &["transcript"]);
                });
            }));
            zork_ui::components::region::invalidate(cx, &["transcript"]);
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
                            v.transcript_list.set_follow_mode(FollowMode::Normal);
                            v.transcript_list.scroll_to(gpui::ListOffset {
                                item_ix: first_new.min(v.lines.len().saturating_sub(1)),
                                offset_in_item: px(0.),
                            });
                            if v.transcript_list.is_scrolled_to_end().unwrap_or(false) {
                                v.transcript_list.set_follow_mode(FollowMode::Tail);
                                v.transcript_list.scroll_to_end();
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
}
