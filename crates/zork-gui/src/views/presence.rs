//! Composer-local presence. Timers exist only for a transition or idle grace period.
use super::*;
use crate::api::ParticipantStatus;
use std::time::Instant;

mod preview;

const MOVE: Duration = Duration::from_millis(500);
const IDLE_GRACE: Duration = Duration::from_millis(1200);
// Logical pixels per second, shared with Android's dp-based animation.
const WIDTH_SPEED: f32 = 1440.;
use zork_ui::components::liquid_composer::{
    DOCK_GAP, EDGE, IMMERSION, RADIUS, ROW_SPACING, SPACING,
};
const AVATAR: f32 = RADIUS * 1.4;
const ROW: f32 = ROW_SPACING;
const FIRST_LIFT: f32 = RADIUS + DOCK_GAP;
const PORTRAIT_INSET: f32 = RADIUS - AVATAR / 2.;
const LABEL_GAP: f32 = 8.;
const LABEL_RIGHT_PADDING: f32 = 12.;

#[derive(Clone, Copy, Default, PartialEq)]
struct Position {
    x: f32,
    y: f32,
    label: f32,
}

struct WidthMotion {
    from: f32,
    to: f32,
    began: Instant,
}
impl WidthMotion {
    fn sample(&self, now: Instant) -> f32 {
        let distance = self.to - self.from;
        let travelled = WIDTH_SPEED * now.duration_since(self.began).as_secs_f32();
        self.from + distance.signum() * travelled.min(distance.abs())
    }
    fn retarget(&mut self, target: f32, now: Instant, snap: bool) {
        if snap {
            self.from = target;
            self.to = target;
            self.began = now;
        } else if self.to != target {
            self.from = self.sample(now);
            self.to = target;
            self.began = now;
        }
    }
    fn moving(&self, now: Instant) -> bool {
        WIDTH_SPEED * now.duration_since(self.began).as_secs_f32() < (self.to - self.from).abs()
    }
}

struct Member {
    info: ParticipantStatus,
    label: String,
    measured_label: String,
    text_width: f32,
    width: WidthMotion,
    failed: bool,
    idle_since: Option<Instant>,
    expanded: bool,
    from: Position,
    velocity: Position,
    to: Position,
    began: Instant,
}

impl Member {
    fn sample(&self, now: Instant) -> (Position, Position) {
        let elapsed = now.duration_since(self.began);
        if elapsed >= MOVE {
            return (self.to, Position::default());
        }
        // Critically damped spring: zero initial speed, continuous position and
        // velocity on retarget, no canned easing restart or decorative bounce.
        let t = elapsed.as_secs_f32();
        let omega = 10.08 / MOVE.as_secs_f32();
        let decay = (-omega * t).exp();
        let axis = |from: f32, to: f32, velocity: f32| {
            let displacement = from - to;
            let coefficient = velocity + omega * displacement;
            (
                to + (displacement + coefficient * t) * decay,
                (velocity - omega * coefficient * t) * decay,
            )
        };
        let (x, vx) = axis(self.from.x, self.to.x, self.velocity.x);
        let (y, vy) = axis(self.from.y, self.to.y, self.velocity.y);
        let (label, vl) = axis(self.from.label, self.to.label, self.velocity.label);
        (
            Position { x, y, label },
            Position {
                x: vx,
                y: vy,
                label: vl,
            },
        )
    }
    fn position(&self, now: Instant) -> Position {
        self.sample(now).0
    }
    fn update_width(&mut self, target: f32, now: Instant, reduced: bool) {
        let position = self.position(now);
        let lifting = self.expanded
            && !reduced
            && ((position.x - self.to.x).abs() > 0.001 || (position.y - self.to.y).abs() > 0.001);
        let target = if lifting {
            self.width.sample(now)
        } else if self.expanded {
            target
        } else {
            0.
        };
        self.width.retarget(target, now, reduced);
    }
    fn moving(&self, now: Instant) -> bool {
        self.width.moving(now)
            || ((self.from != self.to || self.velocity != Position::default())
                && now.duration_since(self.began) < MOVE)
    }
}

#[derive(Default)]
pub(super) struct Presence {
    #[cfg(feature = "headless-bench")]
    previews: HashMap<String, Arc<zork_client_core::state::HistoryData>>,
    members: Vec<Member>,
    preview: Option<Entity<preview::Overlay>>,
    max_label_width: Option<f32>,
    wake: Option<Task<()>>,
    frame_time: Option<Instant>,
    pub(super) extent: f32,
    pub(super) scene: zork_ui::components::liquid::composer::Scene,
    scene_time: Option<Instant>,
    frame_scheduled: bool,
    anchors: HashMap<String, gpui::Bounds<gpui::Pixels>>,
    hovered: Option<String>,
}

impl Presence {
    fn reconcile(
        &mut self,
        members: Vec<ParticipantStatus>,
        now: Instant,
        reduced: bool,
        locale: Locale,
    ) {
        self.members
            .retain(|old| members.iter().any(|m| m.id == old.info.id));
        for info in members {
            let index = self
                .members
                .iter()
                .position(|m| m.info.id == info.id)
                .unwrap_or_else(|| {
                    let position = Position {
                        x: self.members.len() as f32 * SPACING,
                        y: -IMMERSION,
                        ..Position::default()
                    };
                    self.members.push(Member {
                        info: info.clone(),
                        label: String::new(),
                        measured_label: String::new(),
                        text_width: 0.,
                        width: WidthMotion {
                            from: 0.,
                            to: 0.,
                            began: now,
                        },
                        failed: false,
                        idle_since: None,
                        expanded: false,
                        from: position,
                        velocity: Position::default(),
                        to: position,
                        began: now,
                    });
                    self.members.len() - 1
                });
            let member = &mut self.members[index];
            let status = info
                .activity
                .as_ref()
                .filter(|status| should_render_live_activity(status, None));
            if let Some(status) = status {
                member.label = agent_status_label(status, locale);
                member.failed = matches!(status, AgentStatus::Failed { .. });
                member.idle_since = None;
                member.expanded = true;
            } else if member.expanded {
                let since = *member.idle_since.get_or_insert(now);
                member.label = locale.text("presence_idle").into();
                member.failed = false;
                if now.duration_since(since) >= IDLE_GRACE {
                    member.expanded = false;
                    member.idle_since = None;
                }
            }
            member.info = info;
        }
        // Existing members keep their order even when events reorder the snapshot.
        let mut idle = 0;
        let mut active = 0;
        for member in &mut self.members {
            let target = if member.expanded {
                active += 1;
                Position {
                    x: zork_ui::components::liquid_composer::ACTIVE_EDGE - EDGE,
                    y: FIRST_LIFT + (active - 1) as f32 * ROW,
                    label: 1.,
                }
            } else {
                let x = idle as f32 * SPACING;
                idle += 1;
                Position {
                    x,
                    y: -IMMERSION,
                    label: 0.,
                }
            };
            if target != member.to {
                (member.from, member.velocity) = member.sample(now);
                member.to = target;
                member.began = now;
            }
            if reduced {
                member.from = member.to;
                member.velocity = Position::default();
            }
        }
    }

    fn next_wake(&self, now: Instant) -> Option<Duration> {
        if self.members.iter().any(|m| m.moving(now)) {
            return Some(Duration::from_nanos(8_333_333));
        }
        self.members
            .iter()
            .filter_map(|m| {
                m.idle_since
                    .map(|since| IDLE_GRACE.saturating_sub(now.duration_since(since)))
            })
            .min()
    }
}

impl RootView {
    #[cfg(feature = "headless-bench")]
    pub fn benchmark_presence_history(
        &mut self,
        member: &str,
        data: zork_client_core::state::HistoryData,
        cx: &mut Context<Self>,
    ) {
        self.presence.previews.insert(member.into(), Arc::new(data));
        zork_ui::components::region::invalidate(cx, &["composer"]);
    }
    pub(super) fn conversation_members(&self) -> Vec<ParticipantStatus> {
        self.participants.clone()
    }

    pub(super) fn prepare_presence_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.advance_file_fans(window, cx);
        let now = cx.background_executor().now();
        let file_width = self.file_fan_dimensions().0;
        self.presence.max_label_width = Some(
            (self.composer_surface_width
                - file_width
                - EDGE
                - PORTRAIT_INSET * 2.
                - AVATAR
                - if file_width > 0. { 16. } else { 0. })
            .max(0.),
        );
        self.presence.reconcile(
            self.conversation_members(),
            now,
            cx.reduce_motion(),
            self.locale,
        );
        for member in &mut self.presence.members {
            let label = format!("{} · {}", member.info.name, member.label);
            if label != member.measured_label {
                let run = gpui::TextRun {
                    len: label.len(),
                    font: gpui::font("Inter Variable"),
                    color: rgb(TEXT).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                member.text_width = window
                    .text_system()
                    .shape_line(label.clone().into(), px(12.), &[run], None)
                    .width
                    .as_f32();
                member.measured_label = label;
            }
            let width = (LABEL_GAP + member.text_width + LABEL_RIGHT_PADDING - PORTRAIT_INSET)
                .min(self.presence.max_label_width.unwrap_or(f32::MAX));
            member.update_width(width, now, cx.reduce_motion());
        }
        self.presence.frame_time = Some(now);
        let extent = self
            .presence
            .members
            .iter()
            .map(|m| m.position(now).y + RADIUS)
            .fold(0., f32::max)
            .min(
                (window.viewport_size().height.as_f32() - self.composer_editor_height - 96.)
                    .max(0.),
            );
        if (extent - self.presence.extent).abs() > 0.001 {
            self.presence.extent = extent;
            // The list and bubbles consume the same sample in this frame.
            // Tail mode follows padding; a historical scroll anchor stays put.
            zork_ui::components::region::invalidate(cx, &["transcript"]);
        }
    }

    pub(super) fn render_shared_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use component::{Capabilities, Member as MemberView, Snapshot};
        use zork_ui::components::liquid::{composer as component, Pose};
        let now = self
            .presence
            .frame_time
            .unwrap_or_else(|| cx.background_executor().now());
        self.presence.wake = None;
        let preview = self
            .presence
            .preview
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    zork_ui::components::region::forget_on_release(cx);
                    preview::Overlay::default()
                })
            })
            .clone();
        preview.update(cx, |v, cx| {
            v.retain(self.presence.members.iter().map(|m| &m.info.id), cx)
        });
        self.presence
            .anchors
            .retain(|id, _| self.presence.members.iter().any(|m| &m.info.id == id));
        let extent = self.presence.extent.max(self.file_fan_dimensions().1);
        let height = self.composer_editor_height
            + zork_ui::components::liquid_composer::TOP_EXTENSION
            + zork_ui::components::liquid_composer::COMPOSER_CHROME;
        let body = Pose::rect(
            0.,
            extent as f64,
            self.composer_surface_width.max(2.) as f64,
            height as f64,
            zork_ui::components::liquid_composer::SURFACE_RADIUS as f64,
        );
        let state = zork_client_core::composer::interrupting(
            self.sessions
                .iter()
                .find(|s| Some(&s.session_id) == self.selected_session.as_ref()),
            self.composer_input.read(cx).value(),
            self.draft_state.comments.len()
                + self.draft_state.attachments.len()
                + self.draft_state.files.len(),
            self.canceling,
            self.preparing_files,
        );
        let targets: Vec<_> = self
            .presence
            .members
            .iter()
            .filter_map(|member| {
                let position = member.position(now);
                let width = (AVATAR + member.width.sample(now))
                    .min(self.composer_surface_width - EDGE - PORTRAIT_INSET * 2.)
                    + PORTRAIT_INSET * 2.;
                let pose = Pose::rect(
                    (EDGE + position.x) as f64,
                    body.top() - position.y as f64 - RADIUS as f64,
                    width.max(2.) as f64,
                    32.,
                    16.,
                );
                // Membership remains in core. Offscreen portraits need neither a
                // material parcel nor a control; retain a small motion gutter.
                (pose.left() < body.w + 8.
                    && pose.left() + pose.w > -8.
                    && pose.top() + pose.h > -8.)
                    .then_some((member, pose))
            })
            .collect();
        let snapshot = Snapshot {
            capabilities: Capabilities {
                editable: state.editable,
                stop: state.stop,
                enabled: state.enabled,
            },
            text: self.composer_input.read(cx).value().to_owned(),
            members: targets
                .iter()
                .map(|(member, _)| MemberView {
                    id: member.info.id.clone(),
                    avatar: member.info.avatar.clone().unwrap_or_else(|| "cat".into()),
                    label: format!("{} · {}", member.info.name, member.label),
                    active: member.expanded,
                })
                .collect(),
            ..Default::default()
        };
        let member_colors = targets
            .iter()
            .map(|(m, _)| {
                if m.failed {
                    ZORK_UI.palette.danger
                } else {
                    TEXT
                }
            })
            .collect();
        let member_names = targets.iter().map(|(m, _)| m.info.name.clone()).collect();
        let targets: Vec<_> = targets
            .into_iter()
            .map(|(member, pose)| (member.info.id.clone(), pose))
            .collect();
        let opening = self.draft_opening();
        let fan = opening.map(|_| {
            div()
                .absolute()
                .top_0()
                .w_full()
                .h(px(
                    extent + zork_ui::components::liquid_composer::TOP_EXTENSION
                ))
                .child(self.render_draft_fan(cx))
                .into_any_element()
        });
        let elapsed = self
            .presence
            .scene_time
            .replace(now)
            .map_or(0., |previous| {
                now.saturating_duration_since(previous).as_secs_f64()
            });
        let prior_revision = self
            .presence
            .scene
            .surface
            .as_ref()
            .map(|surface| surface.simulation.revision);
        let moving = self.presence.scene.frame(
            body,
            &targets,
            opening.map(|opening| opening.translated(gpui::point(0., body.top() as f32))),
            elapsed,
            cx.reduce_motion(),
        );
        let settled = !moving
            && prior_revision
                != self
                    .presence
                    .scene
                    .surface
                    .as_ref()
                    .map(|surface| surface.simulation.revision);
        if moving
            || settled
            || self
                .presence
                .members
                .iter()
                .any(|member| member.moving(now))
        {
            if !self.presence.frame_scheduled {
                self.presence.frame_scheduled = true;
                let root = cx.entity().downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = root.update(cx, |view, cx| {
                        view.presence.frame_scheduled = false;
                        let now = cx.background_executor().now();
                        if view.presence.scene.moving()
                            || view
                                .presence
                                .members
                                .iter()
                                .any(|member| member.moving(now))
                        {
                            zork_ui::components::region::invalidate(cx, &["composer"]);
                        } else {
                            // Commit the intrinsic region's cached layout at rest;
                            // a later unrelated redraw must not rebuild the composer.
                            cx.notify();
                        }
                    });
                });
            }
        } else if let Some(delay) = self.presence.next_wake(now) {
            self.presence.wake = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |_, cx| {
                    zork_ui::components::region::invalidate(cx, &["composer"])
                });
            }));
        }
        let indices = self
            .presence
            .scene
            .indices(snapshot.members.iter().map(|m| &m.id));
        let bubbles = self.presence.scene.bubbles();
        let root = cx.entity().downgrade();
        let handler = Rc::new(move |action, w: &mut Window, cx: &mut gpui::App| {
            let _ = root.update(cx, |view, cx| view.composer_action(action, w, cx));
        });
        let surface = self
            .presence
            .scene
            .surface
            .as_ref()
            .expect("valid composer material");
        let component = component::render(
            component::Props {
                id: "composer",
                surface,
                width: self.composer_surface_width,
                height: extent + height,
                editor: &self.composer_input,
                snapshot: &snapshot,
                fan_progress: self.file_ui.draft.progress,
                fan_pinned: self.file_ui.draft.pinned,
                bubbles: &bubbles,
                handler,
                presentation: Some(component::Presentation {
                    editor_id: "composer-input".into(),
                    attach_id: "composer-options".into(),
                    primary_id: "send-button".into(),
                    member_groups: indices,
                    member_colors,
                    member_names,
                    fan,
                    busy: self.canceling,
                    editor_label: self.locale.text("composer_placeholder").into(),
                    attach_label: self.locale.text("add_files").into(),
                    primary_label: self
                        .locale
                        .text(if state.stop {
                            "stop_task"
                        } else {
                            "send_message"
                        })
                        .into(),
                }),
            },
            window,
            cx,
        );
        div()
            .relative()
            .w(px(self.composer_surface_width))
            .h(px(extent + height))
            .child(component)
            .child(zork_ui::components::region::tracked_view(preview))
            .into_any_element()
    }

    fn composer_action(
        &mut self,
        action: zork_ui::components::liquid::composer::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zork_ui::components::liquid::{composer::Action, departure::Origin};
        match action {
            Action::FocusEditor => self.focus_composer(window, cx),
            Action::Primary => {
                let state = zork_client_core::composer::interrupting(
                    self.sessions
                        .iter()
                        .find(|s| Some(&s.session_id) == self.selected_session.as_ref()),
                    self.composer_input.read(cx).value(),
                    self.draft_state.comments.len()
                        + self.draft_state.attachments.len()
                        + self.draft_state.files.len(),
                    self.canceling,
                    self.preparing_files,
                );
                if state.enabled {
                    if state.stop {
                        self.cancel_session(cx);
                    } else {
                        self.send_composer(Origin::Button, cx);
                    }
                }
            }
            Action::ChooseFiles => {
                self.focus_composer(window, cx);
                self.choose_files(cx);
            }
            Action::Member(id) => {
                if let Some(member) = self
                    .presence
                    .members
                    .iter()
                    .find(|member| member.info.id == id)
                {
                    self.toggle_history(&member.info.session_id.clone(), cx);
                }
            }
            Action::MemberAnchor(id, bounds) => {
                self.presence.anchors.insert(id.clone(), bounds);
                if let Some(preview) = &self.presence.preview {
                    preview.update(cx, |v, cx| v.anchor(&id, bounds, cx));
                }
            }
            Action::MemberHover(id) => self.hover_composer_member(id, cx),
            // The rich fan supplies its own typed file references and callback.
            Action::FanHover(_)
            | Action::ToggleFan
            | Action::OpenFile(_)
            | Action::RemoveFile(_) => {}
        }
    }
    fn hover_composer_member(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let Some(preview) = self.presence.preview.clone() else {
            return;
        };
        if let Some(previous) = self.presence.hovered.take() {
            if id.as_ref() != Some(&previous) {
                preview.update(cx, |v, cx| v.leave(&previous, cx));
            }
        }
        self.presence.hovered = id.clone();
        let Some(id) = id else {
            return;
        };
        let Some(member) = self
            .presence
            .members
            .iter()
            .find(|m| m.info.id == id)
            .map(|m| m.info.clone())
        else {
            return;
        };
        let bounds = self.presence.anchors.get(&id).copied().unwrap_or_default();
        let device = self.core_device.clone();
        let conversation = self.core_conversation.clone();
        let locale = self.locale;
        let is_selected = self.selected_session.as_deref() == Some(&member.session_id);
        #[cfg(feature = "headless-bench")]
        let offline = self.benchmark_offline;
        #[cfg(not(feature = "headless-bench"))]
        let offline = false;
        #[cfg(feature = "headless-bench")]
        let fixture = self.presence.previews.get(&id).cloned();
        preview.update(cx, |v, cx| {
            v.show(
                &id,
                bounds,
                |cx| {
                    #[cfg(feature = "headless-bench")]
                    if let Some(state) = &fixture {
                        return cx.new(|_| {
                            preview::Preview::fixture(member.clone(), locale, state.clone())
                        });
                    }
                    let source = (!offline && !member.session_id.is_empty())
                        .then(|| device.conversation(&member.session_id));
                    cx.new(|cx| {
                        preview::Preview::new(member, locale, source, conversation, is_selected, cx)
                    })
                },
                cx,
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_moves_at_fixed_speed_and_retargets_from_its_current_position() {
        let now = Instant::now();
        let mut width = WidthMotion {
            from: 0.,
            to: 960.,
            began: now,
        };
        assert_eq!(width.sample(now + Duration::from_millis(100)), 144.);
        assert_eq!(width.sample(now + Duration::from_millis(200)), 288.);
        assert!(width.moving(now + MOVE)); // Large changes take longer than the lift animation.
        let turn = now + Duration::from_millis(200);
        width.retarget(0., turn, false);
        assert_eq!(width.sample(turn), 288.);
        assert_eq!(width.sample(turn + Duration::from_millis(100)), 144.);
        assert_eq!(width.sample(turn + Duration::from_millis(200)), 0.);
        assert!(!width.moving(turn + Duration::from_millis(200)));
        width.retarget(240., turn, true);
        assert_eq!(width.sample(turn), 240.);
        assert!(!width.moving(turn));
    }
    fn member(id: &str, activity: Option<AgentStatus>) -> ParticipantStatus {
        ParticipantStatus {
            subscribed: false,
            assigned: false,
            id: id.into(),
            name: id.into(),
            avatar: None,
            session_id: "s".into(),
            activity,
        }
    }

    #[test]
    fn active_member_lifts_before_it_extends() {
        let now = Instant::now();
        let mut dock = Presence::default();
        dock.reconcile(vec![member("a", None)], now, false, Locale::default());
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Thinking))],
            now,
            false,
            Locale::default(),
        );
        let member = &mut dock.members[0];
        member.update_width(240., now, false);
        member.update_width(240., now + MOVE / 2, false);
        assert_eq!(member.width.sample(now + MOVE / 2), 0.);
        assert!(member.position(now + MOVE / 2).y < member.to.y);
        member.update_width(240., now + MOVE, false);
        assert_eq!(member.position(now + MOVE).y, member.to.y);
        assert_eq!(member.width.sample(now + MOVE), 0.);
        assert!(member.width.sample(now + MOVE + Duration::from_millis(50)) > 0.);
    }
    #[test]
    fn idle_grace_reentry_and_quiescence() {
        let mut dock = Presence::default();
        let now = Instant::now();
        let locale = Locale::default();
        dock.reconcile(vec![member("a", None)], now, false, locale);
        assert!(dock.next_wake(now).is_none());
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Thinking))],
            now,
            false,
            locale,
        );
        let middle = now + MOVE / 2;
        assert!(dock.members[0].position(middle).y > 0.);
        assert!(dock.members[0].position(middle).y < ROW);
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Finished))],
            now + MOVE,
            false,
            locale,
        );
        assert!(dock.members[0].expanded);
        assert_eq!(dock.next_wake(now + MOVE), Some(IDLE_GRACE));
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Thinking))],
            now + MOVE + IDLE_GRACE / 2,
            false,
            locale,
        );
        assert!(dock.members[0].idle_since.is_none());
        let idle = now + MOVE + IDLE_GRACE;
        dock.reconcile(vec![member("a", None)], idle, false, locale);
        dock.reconcile(vec![member("a", None)], idle + IDLE_GRACE, false, locale);
        assert!(!dock.members[0].expanded);
        assert!(dock.next_wake(idle + IDLE_GRACE + MOVE).is_none());
        assert_eq!(
            dock.members[0].position(idle + IDLE_GRACE + MOVE).y,
            -IMMERSION
        );
    }

    #[test]
    fn resumed_work_preserves_motion_velocity() {
        let mut dock = Presence::default();
        let now = Instant::now();
        let locale = Locale::default();
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Thinking))],
            now,
            false,
            locale,
        );
        dock.reconcile(vec![member("a", None)], now + MOVE, false, locale);
        let returning = now + MOVE + IDLE_GRACE;
        dock.reconcile(vec![member("a", None)], returning, false, locale);
        let interrupted = returning + Duration::from_millis(40);
        let (before, velocity) = dock.members[0].sample(interrupted);
        assert!(velocity.y < 0.);
        dock.reconcile(
            vec![member("a", Some(AgentStatus::Thinking))],
            interrupted,
            false,
            locale,
        );
        let (after, resumed_velocity) = dock.members[0].sample(interrupted);
        assert!((before.y - after.y).abs() < 0.0001);
        assert!((velocity.y - resumed_velocity.y).abs() < 0.0001);
        assert_eq!(dock.members[0].position(interrupted + MOVE).y, FIRST_LIFT);
    }

    #[test]
    fn stable_members_waiting_failure_and_reduced_motion() {
        let mut dock = Presence::default();
        let now = Instant::now();
        let a = member(
            "a",
            Some(AgentStatus::Waiting {
                reason: "approval".into(),
                deadline_ms: 0,
            }),
        );
        let b = member(
            "b",
            Some(AgentStatus::Failed {
                reason: "error".into(),
            }),
        );
        dock.reconcile(vec![a.clone(), b.clone()], now, true, Locale::default());
        dock.reconcile(vec![b, a], now + IDLE_GRACE * 2, true, Locale::default());
        assert_eq!(dock.members[0].info.id, "a");
        assert!(dock.members.iter().all(|m| m.expanded));
        assert!(dock.members[1].failed);
        assert_eq!(dock.members[1].position(now).y, FIRST_LIFT + ROW);
        assert!(dock.next_wake(now + IDLE_GRACE * 2).is_none());
        dock.reconcile(vec![], now + IDLE_GRACE * 3, false, Locale::default());
        assert!(dock.members.is_empty());
    }
}
