//! Composer and attachment preview presentation. Session activity stays in the transcript.
use super::*;
use crate::api::ParticipantStatus;

#[derive(Default)]
pub(super) struct ComposerSurface {
    pub(super) scene: zork_ui::components::widgets::composer::Scene,
}

impl RootView {
    pub(super) fn conversation_members(&self) -> Vec<ParticipantStatus> {
        self.participants.clone()
    }

    pub(super) fn render_shared_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use zork_ui::components::widgets::{composer as component, Pose};
        let extent = self.file_fan_dimensions().1;
        let height = self.composer_editor_height
            + zork_ui::components::composer_layout::TOP_EXTENSION
            + zork_ui::components::composer_layout::COMPOSER_CHROME;
        let body = Pose::rect(
            0.,
            extent as f64,
            self.composer_surface_width.max(2.) as f64,
            height as f64,
            zork_ui::components::composer_layout::SURFACE_RADIUS as f64,
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
        let fan = (!self.draft_file_frame().files.is_empty()).then(|| {
            div()
                .absolute()
                .top_0()
                .w_full()
                .h(px(
                    extent + zork_ui::components::composer_layout::TOP_EXTENSION
                ))
                .child(self.render_draft_fan(cx))
                .into_any_element()
        });
        self.composer_surface.scene.frame(body);
        let root = cx.entity().downgrade();
        let handler = Rc::new(move |action, w: &mut Window, cx: &mut gpui::App| {
            let _ = root.update(cx, |view, cx| view.composer_action(action, w, cx));
        });
        let component = component::render(
            component::Props {
                id: "composer",
                scene: &self.composer_surface.scene,
                width: self.composer_surface_width,
                height: extent + height,
                editor: &self.composer_input,
                snapshot: &snapshot,
                fan_expanded: self.file_ui.draft.open(),
                fan_pinned: self.file_ui.draft.pinned,
                handler,
                action_size: 24.,
                accessory_band: 0.,
                accessories: vec![],
                presentation: Some(component::Presentation {
                    editor_id: "composer-input".into(),
                    attach_id: "composer-options".into(),
                    show_attach: true,
                    primary_id: "send-button".into(),
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
        action: zork_ui::components::widgets::composer::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zork_ui::components::widgets::composer::Action;
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
                        self.send_composer(cx);
                    }
                }
            }
            Action::ChooseFiles => {
                self.focus_composer(window, cx);
                self.choose_files(cx);
            }
            Action::FanHover(_)
            | Action::ToggleFan
            | Action::OpenFile(_)
            | Action::RemoveFile(_) => {}
        }
    }
}
