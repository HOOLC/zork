//! Composer material and attachment fan. Member activity belongs to the transcript.
use super::*;
use crate::api::ParticipantStatus;
use std::time::Instant;

#[derive(Default)]
pub(super) struct ComposerSurface {
    pub(super) scene: zork_ui::components::liquid::composer::Scene,
    scene_time: Option<Instant>,
    frame_scheduled: bool,
}

impl RootView {
    pub(super) fn conversation_members(&self) -> Vec<ParticipantStatus> {
        self.participants.clone()
    }

    pub(super) fn prepare_composer_frame(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.advance_file_fans(window, cx);
    }

    pub(super) fn render_shared_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use zork_ui::components::liquid::{composer as component, Pose};
        let now = cx.background_executor().now();
        let extent = self.file_fan_dimensions().1;
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
        let snapshot = component::Snapshot {
            capabilities: component::Capabilities {
                editable: state.editable,
                stop: state.stop,
                enabled: state.enabled,
            },
            text: self.composer_input.read(cx).value().to_owned(),
            ..Default::default()
        };
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
            .composer_surface
            .scene_time
            .replace(now)
            .map_or(0., |previous| {
                now.saturating_duration_since(previous).as_secs_f64()
            });
        let prior_revision = self
            .composer_surface
            .scene
            .surface
            .as_ref()
            .map(|surface| surface.simulation.revision);
        let moving = self.composer_surface.scene.frame(
            body,
            &[],
            opening.map(|opening| opening.translated(gpui::point(0., body.top() as f32))),
            elapsed,
            cx.reduce_motion(),
        );
        let settled = !moving
            && prior_revision
                != self
                    .composer_surface
                    .scene
                    .surface
                    .as_ref()
                    .map(|surface| surface.simulation.revision);
        if (moving || settled) && !self.composer_surface.frame_scheduled {
            self.composer_surface.frame_scheduled = true;
            let root = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = root.update(cx, |view, cx| {
                    view.composer_surface.frame_scheduled = false;
                    if view.composer_surface.scene.moving() {
                        zork_ui::components::region::invalidate(cx, &["composer"]);
                    } else {
                        cx.notify();
                    }
                });
            });
        }
        let bubbles = self.composer_surface.scene.bubbles();
        let root = cx.entity().downgrade();
        let handler = Rc::new(move |action, w: &mut Window, cx: &mut gpui::App| {
            let _ = root.update(cx, |view, cx| view.composer_action(action, w, cx));
        });
        let surface = self
            .composer_surface
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
                header: None,
                header_fill: None,
                header_height: 0.,
                action_size: 24.,
                accessory_band: 0.,
                accessories: vec![],
                presentation: Some(component::Presentation {
                    editor_id: "composer-input".into(),
                    attach_id: "composer-options".into(),
                    show_attach: true,
                    primary_id: "send-button".into(),
                    member_groups: vec![],
                    member_colors: vec![],
                    member_names: vec![],
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
            // This composer has no member controls. File fan actions are handled
            // by the draft fan itself.
            Action::Member(_)
            | Action::MemberAnchor(_, _)
            | Action::MemberHover(_)
            | Action::FanHover(_)
            | Action::ToggleFan
            | Action::OpenFile(_)
            | Action::RemoveFile(_) => {}
        }
    }
}
