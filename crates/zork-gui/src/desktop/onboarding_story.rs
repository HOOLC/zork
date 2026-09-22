//! First-use specimen uses the production native composition and model editor.
use super::{model_settings::ModelSettings, stories::new_chat_story, ui};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    i18n::Locale,
};
use gpui::{div, prelude::*, px, Context, Entity, Render, Window};
use serde_json::{json, Value};
use zork_ui::{components::loading, design::TextRole, onboarding};

pub struct OnboardingStory {
    state: String,
    models_open: bool,
    finished: bool,
    models: Entity<ModelSettings>,
    chat: Entity<zork_ui::new_chat::Page>,
}

impl OnboardingStory {
    pub fn new(state: &str, cx: &mut Context<Self>) -> Self {
        let state = state
            .trim_end_matches("-compact")
            .trim_end_matches("-wide")
            .to_owned();
        let models = cx.new(ModelSettings::onboarding_fixture);
        let models_open = state == "model-form";
        if models_open {
            models.update(cx, |view, cx| {
                assert!(view.begin_onboarding("mini1", cx));
            });
        }
        Self {
            state,
            models_open,
            finished: false,
            models,
            chat: new_chat_story("draft", 900., cx),
        }
    }

    pub fn inspect(&self) -> Value {
        json!({"state": self.state, "models_open": self.models_open, "finished": self.finished})
    }
}

impl Render for OnboardingStory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let locale = Locale::ZhCn;
        if self.finished {
            return div().size_full().child(self.chat.clone());
        }
        if self.models_open {
            return onboarding::frame().child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_8()
                    .pb_8()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div().flex().justify_between().child(
                            ui::button(
                                "onboarding-models-back",
                                locale.text("onboarding_back"),
                                false,
                                true,
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.models_open = false;
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, locale.text("onboarding_back")),
                        ),
                    )
                    .child(
                        div()
                            .id("onboarding-model-settings")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(ui::settings_content(self.models.clone())),
                    ),
            );
        }
        let (title, description) = match self.state.as_str() {
            "waiting" => (
                locale.text("onboarding_browser_title"),
                locale.text("onboarding_browser_description"),
            ),
            "preparing" | "failure" => (
                locale.text("onboarding_preparing_title"),
                locale.text("onboarding_preparing_description"),
            ),
            "models" | "model-form" => (
                locale.text("onboarding_models"),
                locale.text("onboarding_models_description"),
            ),
            "ready" => (
                locale.text("onboarding_ready"),
                locale.text("onboarding_ready_description"),
            ),
            _ => (
                locale.text("onboarding_welcome"),
                locale.text("onboarding_welcome_description"),
            ),
        };
        let mut hero = onboarding::hero(title, description);
        match self.state.as_str() {
            "waiting" => {
                hero = hero.child(
                    div()
                        .mt(px(30.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_3()
                        .child(ui::button(
                            "desktop-welcome-login",
                            locale.text("onboarding_login_waiting"),
                            true,
                            false,
                        ))
                        .child(
                            ui::button(
                                "onboarding-cancel-login",
                                locale.text("onboarding_cancel"),
                                false,
                                true,
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.state = "login".into();
                                cx.notify();
                            }))
                            .automation(
                                AutomationRole::Button,
                                locale.text("onboarding_cancel_login"),
                            ),
                        ),
                );
            }
            "preparing" => {
                hero = hero.child(div().mt_6().child(loading::status(
                    "onboarding-preparing",
                    locale.text("onboarding_preparing"),
                )));
            }
            "failure" => {
                hero = hero
                    .child(ui::feedback("本机工作空间暂时无法启动".into()))
                    .child(
                        ui::button(
                            "desktop-startup-retry",
                            locale.text("onboarding_retry"),
                            true,
                            true,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.state = "preparing".into();
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, locale.text("onboarding_retry")),
                    );
            }
            "models" | "model-form" => {
                hero = hero.child(
                    div().mt(px(30.)).child(
                        ui::button(
                            "onboarding-add-model",
                            locale.text("onboarding_add_model"),
                            true,
                            true,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.models_open = true;
                            v.models.update(cx, |models, cx| {
                                assert!(models.begin_onboarding("mini1", cx));
                            });
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, locale.text("onboarding_add_model")),
                    ),
                );
            }
            "ready" => {
                hero = hero.child(
                    div().mt(px(30.)).child(
                        ui::button(
                            "onboarding-finish",
                            locale.text("onboarding_start"),
                            true,
                            true,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.finished = true;
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, locale.text("onboarding_start")),
                    ),
                );
            }
            _ => {
                hero = hero
                    .child(
                        div().mt(px(30.)).child(
                            ui::button(
                                "desktop-welcome-login",
                                locale.text("onboarding_login"),
                                true,
                                true,
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.state = "waiting".into();
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, locale.text("onboarding_login")),
                        ),
                    )
                    .child(div().mt_4().child(ui::text_role(
                        locale.text("onboarding_browser_hint"),
                        TextRole::Metadata,
                    )));
            }
        }
        onboarding::frame().child(onboarding::center(hero))
    }
}
