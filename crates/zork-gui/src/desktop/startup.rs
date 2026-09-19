//! Startup presentation consumes core readiness without gating cached navigation.
use super::*;
use zork_client_core::desktop::startup::{Phase, State};
use zork_ui::{components::loading, design::TextRole};

impl DesktopRoot {
    pub(super) fn apply_startup(&mut self, state: Arc<State>, cx: &mut Context<Self>) {
        let selection_changed =
            self.startup_state.selection_generation != state.selection_generation;
        self.startup_state = state.clone();
        if selection_changed && self.active.is_none() {
            if let Some(node) = state
                .selected
                .as_deref()
                .and_then(|id| self.source.node(id))
            {
                // A late recovery must not navigate away from settings the user
                // explicitly opened while it was pending.
                let managing = self.managing;
                self.open_node(node, cx);
                self.managing = managing;
            }
        }
        cx.notify();
    }

    fn retry_startup(&mut self, cx: &mut Context<Self>) {
        cx.global::<DesktopRuntime>().startup.retry();
    }

    pub(super) fn startup_notice(&self, cx: &mut Context<Self>) -> Option<Div> {
        let node = self
            .active_node_id
            .as_deref()
            .and_then(|id| self.source.node(id))?;
        let locale = self.client_settings.locale;
        let phase = self.startup_state.dependency(&node);
        let content = match phase {
            Phase::Preparing | Phase::Stopping => div().child(loading::status(
                "desktop-startup-status",
                locale.text(if matches!(phase, Phase::Stopping) {
                    "startup_local_stopping"
                } else if node.local {
                    "startup_local_preparing"
                } else {
                    "startup_mesh_preparing"
                }),
            )),
            Phase::Failed(error) | Phase::StopFailed(error) => div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(ui::text_role(
                            locale.text(if matches!(phase, Phase::StopFailed(_)) {
                                "startup_stop_failed"
                            } else if node.local {
                                "startup_local_failed"
                            } else {
                                "startup_mesh_failed"
                            }),
                            TextRole::Label,
                        ))
                        .child(ui::feedback(error.clone())),
                )
                .child(
                    ui::button(
                        "desktop-startup-retry",
                        locale.text("startup_retry"),
                        false,
                        true,
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.retry_startup(cx)))
                    .automation(AutomationRole::Button, locale.text("startup_retry")),
                ),
            Phase::Ready | Phase::NotRequired => return None,
        };
        Some(
            div()
                .flex_shrink_0()
                .px_4()
                .py_2()
                .bg(rgb(ZORK_UI.palette.canvas))
                .child(content),
        )
    }

    pub(super) fn render_startup(&mut self, cx: &mut Context<Self>) -> Div {
        let locale = self.client_settings.locale;
        let preparing = matches!(self.startup_state.local, Phase::Preparing | Phase::Stopping)
            || matches!(self.startup_state.mesh, Phase::Preparing)
            || self.busy;
        let error = match (&self.startup_state.local, &self.startup_state.mesh) {
            (Phase::Failed(error) | Phase::StopFailed(error), _) | (_, Phase::Failed(error)) => {
                Some(error.clone())
            }
            _ => None,
        };
        let (title, description) = if matches!(self.startup_state.local, Phase::Stopping) {
            ("startup_stopping_title", "startup_stopping_description")
        } else if preparing {
            ("startup_preparing_title", "startup_preparing_description")
        } else if error.is_some() {
            ("startup_failed_title", "startup_failed_description")
        } else {
            ("startup_welcome_title", "startup_welcome_description")
        };
        let body = div()
            .w_full()
            .max_w(px(460.))
            .px_8()
            .child(ui::heading(locale.text(title), locale.text(description)))
            .when(self.account_state.subject.is_none(), |view| {
                view.child(ui::text_role(
                    "公网连接需要登录 Zork；局域网可直接连接。",
                    TextRole::Description,
                ))
                .child(
                    ui::button(
                        "desktop-welcome-login",
                        if self.account_state.busy() {
                            "等待 Google 登录…"
                        } else {
                            "使用 Google 登录"
                        },
                        false,
                        !self.account_state.busy(),
                    )
                    .on_click(cx.listener(|view, _, _, cx| view.login_account(cx)))
                    .automation_enabled(
                        !self.account_state.busy(),
                        AutomationRole::Button,
                        "使用 Google 登录",
                    ),
                )
                .when_some(self.account_state.error.clone(), |view, error| {
                    view.child(ui::feedback(error))
                })
            })
            .when(preparing, |view| {
                view.child(loading::status(
                    "desktop-startup-status",
                    locale.text(if matches!(self.startup_state.local, Phase::Stopping) {
                        "startup_local_stopping"
                    } else if matches!(self.startup_state.local, Phase::Preparing)
                        || (self.busy && !self.pairing)
                    {
                        "startup_local_preparing"
                    } else {
                        "startup_mesh_preparing"
                    }),
                ))
            })
            .when_some(error.clone(), |view, error| view.child(ui::feedback(error)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .mt_4()
                    .when(!preparing && error.is_some(), |view| {
                        view.child(
                            ui::button(
                                "desktop-startup-retry",
                                locale.text("startup_retry"),
                                true,
                                true,
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.retry_startup(cx)))
                            .automation(AutomationRole::Button, locale.text("startup_retry")),
                        )
                    })
                    .when(!preparing && error.is_none(), |view| {
                        view.child(
                            ui::button(
                                "desktop-start-local",
                                locale.text("startup_start_local"),
                                true,
                                true,
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.start_node(cx)))
                            .automation(AutomationRole::Button, locale.text("startup_start_local")),
                        )
                    })
                    .child(
                        ui::button(
                            "desktop-startup-settings",
                            locale.text(if preparing || error.is_some() {
                                "startup_settings"
                            } else {
                                "startup_connect_device"
                            }),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.managing = true;
                            view.management_tab = 3;
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, locale.text("startup_settings")),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                        window.start_window_move()
                    }),
            )
            .child(
                div()
                    .id("desktop-startup-page")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .pb(px(48.))
                    .child(body)
                    .automation(AutomationRole::Status, locale.text(title)),
            )
    }
}
