//! Startup presentation consumes core readiness without gating cached navigation.
use super::*;
use zork_client_core::desktop::startup::{Phase, State};
use zork_ui::{components::loading, design::TextRole};

impl DesktopRoot {
    pub(super) fn apply_startup(&mut self, state: Arc<State>, cx: &mut Context<Self>) {
        let selection_changed =
            self.startup_state.selection_generation != state.selection_generation;
        let entered_first_chat = state.first_chat_welcome && !self.startup_state.first_chat_welcome;
        self.startup_state = state.clone();
        #[cfg(target_os = "macos")]
        if !self.settings_action_registered && state.onboarding.is_none() {
            let view = cx.weak_entity();
            gpui::App::on_action(cx, move |_: &crate::app_menu::OpenSettings, cx| {
                let _ = view.update(cx, |view, cx| {
                    if view.startup_state.onboarding.is_none() {
                        view.apply_navigation(navigation::Destination::Manage(4), cx);
                    }
                });
            });
            self.settings_action_registered = true;
        }
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
        if entered_first_chat {
            if self.active.is_none() {
                if let Some(node) = state
                    .selected
                    .as_deref()
                    .and_then(|id| self.source.node(id))
                {
                    self.open_node(node, cx);
                }
            }
            if let Some(active) = &self.active {
                active.update(cx, |view, cx| view.set_first_chat_welcome(true, cx));
                self.apply_navigation(navigation::Destination::Home, cx);
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
                    "Mesh 连接无需账号；可按需登录云端服务。",
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
                    .window_control_area(gpui::WindowControlArea::Drag),
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

impl DesktopRoot {
    fn finish_onboarding(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = cx.global::<DesktopRuntime>().startup.finish_onboarding() {
            self.error = Some(error.to_string());
        }
        cx.notify();
    }

    pub(super) fn render_onboarding(&mut self, cx: &mut Context<Self>) -> Div {
        use zork_client_core::desktop::startup::Onboarding;
        let phase = self.startup_state.onboarding.expect("onboarding visible");
        let locale = self.client_settings.locale;
        let frame = zork_ui::onboarding::frame();
        if self.onboarding_models_open && matches!(phase, Onboarding::Models | Onboarding::Ready) {
            return frame.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_8()
                    .pb_8()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .child(
                                ui::button(
                                    "onboarding-models-back",
                                    locale.text("onboarding_back"),
                                    false,
                                    true,
                                )
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.onboarding_models_open = false;
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, locale.text("onboarding_back")),
                            )
                            .when(
                                phase == Onboarding::Ready
                                    && self.startup_state.onboarding_error.is_some(),
                                |v| {
                                    v.child(
                                        ui::button(
                                            "onboarding-retry-completion",
                                            locale.text("onboarding_retry"),
                                            false,
                                            true,
                                        )
                                        .on_click(
                                            cx.listener(|v, _, _, cx| v.finish_onboarding(cx)),
                                        )
                                        .automation(
                                            AutomationRole::Button,
                                            locale.text("onboarding_retry"),
                                        ),
                                    )
                                },
                            ),
                    )
                    .when_some(self.startup_state.onboarding_error.clone(), |v, error| {
                        v.child(ui::feedback(error))
                    })
                    .when_some(self.error.clone(), |v, error| v.child(ui::feedback(error)))
                    .child(
                        div()
                            .id("onboarding-model-settings")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .when_some(self.model_settings.clone(), |v, settings| {
                                v.child(ui::settings_content(settings))
                            }),
                    ),
            );
        }
        let (title, description) = match phase {
            Onboarding::Login if self.account_state.busy() => (
                locale.text("onboarding_browser_title"),
                locale.text("onboarding_browser_description"),
            ),
            Onboarding::Login => (
                locale.text("onboarding_welcome"),
                locale.text("onboarding_welcome_description"),
            ),
            Onboarding::Preparing => (
                locale.text("onboarding_preparing_title"),
                locale.text("onboarding_preparing_description"),
            ),
            Onboarding::Models => (
                locale.text("onboarding_models"),
                locale.text("onboarding_models_description"),
            ),
            Onboarding::Ready => (
                locale.text("onboarding_ready"),
                locale.text("onboarding_ready_description"),
            ),
        };
        let mut body = zork_ui::onboarding::hero(title, description);
        match phase {
            Onboarding::Login => {
                let busy = self.account_state.busy();
                body = body
                    .child(
                        div()
                            .mt(px(30.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_3()
                            .child(
                                ui::button(
                                    "desktop-welcome-login",
                                    if busy {
                                        locale.text("onboarding_login_waiting")
                                    } else {
                                        locale.text("onboarding_login")
                                    },
                                    true,
                                    !busy,
                                )
                                .on_click(cx.listener(|v, _, _, cx| v.login_account(cx)))
                                .automation_enabled(
                                    !busy,
                                    AutomationRole::Button,
                                    locale.text("onboarding_login"),
                                ),
                            )
                            .when_some(self.account_state.login_url.clone(), |v, url| {
                                v.child(
                                    ui::button(
                                        "onboarding-reopen-login",
                                        locale.text("onboarding_reopen"),
                                        false,
                                        true,
                                    )
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                    .automation(
                                        AutomationRole::Button,
                                        locale.text("onboarding_reopen"),
                                    ),
                                )
                            })
                            .when(busy, |v| {
                                v.child(
                                    ui::button(
                                        "onboarding-cancel-login",
                                        locale.text("onboarding_cancel"),
                                        false,
                                        true,
                                    )
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        if let Err(error) = v.source.cancel_account() {
                                            v.error = Some(error.to_string());
                                        }
                                        cx.notify();
                                    }))
                                    .automation(
                                        AutomationRole::Button,
                                        locale.text("onboarding_cancel_login"),
                                    ),
                                )
                            }),
                    )
                    .when(!busy, |v| {
                        v.child(div().mt_4().child(ui::text_role(
                            locale.text("onboarding_browser_hint"),
                            TextRole::Metadata,
                        )))
                    })
                    .when_some(self.account_state.error.clone(), |v, error| {
                        v.child(ui::feedback(error))
                    });
            }
            Onboarding::Preparing => {
                let failure = match &self.startup_state.local {
                    Phase::Failed(error) => Some(error.clone()),
                    _ => None,
                };
                body = body
                    .when(failure.is_none(), |v| {
                        v.child(div().mt_6().child(loading::status(
                            "onboarding-preparing",
                            locale.text("onboarding_preparing"),
                        )))
                    })
                    .when_some(failure, |v, error| {
                        v.child(
                            div()
                                .w_full()
                                .mt(px(30.))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_4()
                                .child(zork_ui::onboarding::failure_notice(error))
                                .child(
                                    ui::button(
                                        "desktop-startup-retry",
                                        locale.text("onboarding_retry"),
                                        true,
                                        true,
                                    )
                                    .on_click(cx.listener(|v, _, _, cx| v.retry_startup(cx)))
                                    .automation(
                                        AutomationRole::Button,
                                        locale.text("onboarding_retry"),
                                    ),
                                ),
                        )
                    });
            }
            Onboarding::Models => {
                body = body.child(
                    div().mt(px(30.)).child(
                        ui::button(
                            "onboarding-add-model",
                            locale.text("onboarding_add_model"),
                            true,
                            self.model_settings
                                .as_ref()
                                .zip(self.active_node_id.as_ref())
                                .is_some_and(|(view, id)| view.read(cx).has_device(id)),
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            if let (Some(settings), Some(id)) =
                                (v.model_settings.clone(), v.active_node_id.clone())
                            {
                                if settings.update(cx, |view, cx| view.begin_onboarding(&id, cx)) {
                                    v.onboarding_models_open = true;
                                }
                            }
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, locale.text("onboarding_add_model")),
                    ),
                );
            }
            Onboarding::Ready => {}
        }
        body = body
            .when(
                phase == Onboarding::Ready && self.startup_state.onboarding_error.is_some(),
                |v| {
                    v.child(
                        div().mt(px(30.)).child(
                            ui::button(
                                "onboarding-retry-completion",
                                locale.text("onboarding_retry"),
                                true,
                                true,
                            )
                            .on_click(cx.listener(|v, _, _, cx| v.finish_onboarding(cx)))
                            .automation(AutomationRole::Button, locale.text("onboarding_retry")),
                        ),
                    )
                },
            )
            .when(
                phase == Onboarding::Models && self.startup_state.onboarding_error.is_some(),
                |v| {
                    v.child(
                        ui::button(
                            "onboarding-retry-models",
                            locale.text("onboarding_retry"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.retry_startup(cx)))
                        .automation(AutomationRole::Button, locale.text("onboarding_retry")),
                    )
                },
            )
            .when_some(self.startup_state.onboarding_error.clone(), |v, error| {
                v.child(ui::feedback(error))
            })
            .when_some(self.error.clone(), |v, error| v.child(ui::feedback(error)));
        frame.child(zork_ui::onboarding::center(body))
    }
}
