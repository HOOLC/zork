//! Composer and attachment preview presentation. Session activity stays in the transcript.
use super::*;
use crate::api::ParticipantStatus;
use zork_ui::design::{INTERACTION, ZORK_UI};

/// Group on the composer frame whose file drags reveal the drop target.
pub(super) const COMPOSER_DROP_GROUP: &str = "composer-drop";

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
        // Draft quotes sit in the composer in the sent-message shape; the main
        // input then reads "补充说明（可选）".
        let drafts = self.render_drafts(window, cx);
        let extra = drafts.is_some();
        if extra != self.extra_placeholder {
            self.extra_placeholder = extra;
            let placeholder = self
                .locale
                .text(if extra {
                    "draft_extra_placeholder"
                } else {
                    "composer_placeholder"
                })
                .to_owned();
            self.composer_input
                .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        }
        let drafts_band = drafts.as_ref().map_or(0., |(_, height)| *height);
        let height = self.composer_editor_height
            + drafts_band
            + self.draft_files_band()
            + zork_ui::components::composer_layout::TOP_EXTENSION
            + zork_ui::components::composer_layout::COMPOSER_CHROME;
        let body = Pose::rect(
            0.,
            0.,
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
        let row_width = (self.composer_surface_width
            - 2. * zork_ui::components::composer_layout::ACTION_INSET)
            .max(2.);
        let files = self.render_draft_files(row_width, cx);
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
                height,
                editor: &self.composer_input,
                snapshot: &snapshot,
                handler,
                action_size: 24.,
                accessory_band: 0.,
                accessories: vec![],
                presentation: Some(component::Presentation {
                    editor_id: "composer-input".into(),
                    attach_id: "composer-options".into(),
                    show_attach: true,
                    primary_id: "send-button".into(),
                    files,
                    drafts,
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
        // Files dragged over the composer show where they will land; the drop
        // itself is handled by the surrounding frame (see `render_composer_frame`).
        let p = ZORK_UI.palette;
        let accent = INTERACTION.accent;
        div()
            .relative()
            .w(px(self.composer_surface_width))
            .h(px(height))
            .child(component)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded(px(zork_ui::components::composer_layout::SURFACE_RADIUS))
                    .border(px(1.5))
                    .border_dashed()
                    .border_color(rgb(accent))
                    .bg(gpui::rgba((accent << 8) | 0x1F))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .text_color(rgb(accent))
                    .opacity(0.)
                    .group_drag_over::<gpui::ExternalPaths>(COMPOSER_DROP_GROUP, |style| {
                        style.opacity(1.)
                    })
                    .child(zork_ui::controls::icon("icons/paperclip.svg", 18.))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.locale.text("files_drop")),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(p.muted))
                            .child(self.locale.text("files_drop_hint")),
                    ),
            )
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
            Action::OpenFile(_) | Action::RemoveFile(_) => {}
        }
    }
}
