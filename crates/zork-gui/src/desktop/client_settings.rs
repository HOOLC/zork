//! Device-local appearance and account preferences.
use super::{ui, DesktopRoot};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    i18n::Locale,
};
use gpui::{div, prelude::*, px, Context, Div, Entity};
use zork_client_core::preferences::Theme;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Notifications,
    Appearance,
    Account,
    Data,
}

/// The saved preference as a design theme; `None` follows the system.
fn design_theme(theme: Theme) -> Option<zork_ui::design::Theme> {
    match theme {
        Theme::System => None,
        Theme::Light => Some(zork_ui::design::Theme::Light),
        Theme::Dark => Some(zork_ui::design::Theme::Dark),
    }
}
const THEMES: [(Theme, &str, &str); 3] = [
    (Theme::System, "client-theme-system", "client_theme_system"),
    (Theme::Light, "client-theme-light", "client_theme_light"),
    (Theme::Dark, "client-theme-dark", "client_theme_dark"),
];
#[derive(Default)]
pub struct State {
    pub page: Page,
    pub locale: Locale,
    pub notification_permission: super::notifications::Permission,
    pub notification_error: Option<String>,
    pub notification_busy: bool,
    pub(super) data: Option<Entity<zork_ui::settings::data::DataSettings>>,
    pub(super) reset: zork_client_core::data_reset::Snapshot,
    pub(super) reset_updates: Option<gpui::Task<()>>,
}

impl DesktopRoot {
    /// Loads the saved theme before the first window paints, so the window
    /// opens in the chosen appearance. `ZORK_THEME` still overrides it.
    pub fn restore_theme(cx: &mut gpui::App) {
        let theme = cx
            .global::<super::DesktopRuntime>()
            .startup
            .directory
            .snapshot()
            .preferences
            .theme;
        zork_ui::design::set_preferred_theme(design_theme(theme));
    }
    fn save_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        match self.source.save_theme(theme) {
            Ok(()) => {
                self.error = None;
                zork_ui::design::prefer_theme(design_theme(theme), cx);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
        cx.notify();
    }
    fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        use zork_ui::components::widgets::controls::{deferred_segmented, Segment, SegmentKind};
        let locale = self.client_locale();
        let p = zork_ui::design::ZORK_UI.palette;
        let current = self.source.snapshot().preferences.theme;
        let selected = THEMES.iter().position(|(theme, _, _)| *theme == current);
        let env_pinned = std::env::var("ZORK_THEME").is_ok_and(|value| {
            matches!(value.to_ascii_lowercase().as_str(), "light" | "dark")
        });
        let note = |key| {
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(gpui::rgb(p.muted))
                .child(locale.text(key).to_string())
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .max_w(px(480.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(locale.text("client_appearance_theme").to_string()),
            )
            .child(deferred_segmented(
                "client-theme",
                THEMES
                    .iter()
                    .map(|(_, id, key)| Segment {
                        id: (*id).into(),
                        label: locale.text(key).to_string().into(),
                        disabled: false,
                    })
                    .collect(),
                vec![None; THEMES.len()],
                selected,
                SegmentKind::Choice,
                true,
                p.canvas,
                cx.listener(|view, index: &usize, _, cx| {
                    if let Some((theme, _, _)) = THEMES.get(*index) {
                        view.save_theme(*theme, cx);
                    }
                }),
            ))
            .child(note("client_appearance_note"))
            .when(env_pinned, |view| view.child(note("client_appearance_pinned")))
    }
    pub(super) fn watch_data_reset(&mut self, cx: &mut Context<Self>) {
        let mut updates = self.source.data_reset.subscribe();
        self.client_settings.reset = (*updates.snapshot()).clone();
        self.client_settings.reset_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(snapshot) = updates.changed().await {
                if this
                    .update(cx, |view, cx| {
                        view.client_settings.reset = (*snapshot).clone();
                        if snapshot.phase == zork_client_core::data_reset::Phase::Restarting {
                            cx.quit();
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    fn client_locale(&self) -> Locale {
        self.client_settings.locale
    }
    pub(super) fn render_client_settings_navigation(
        &self,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.client_locale();
        let mut pages = vec![(Page::Notifications, "client_notifications")];
        pages.push((Page::Appearance, "client_appearance"));
        pages.push((Page::Account, "client_account"));
        pages.push((Page::Data, "client_data"));
        self.settings_tabs
            .section(
                "client-settings-heading",
                locale.text("client_settings_title"),
            )
            .children(pages.into_iter().map(|(page, key)| {
                self.settings_tabs
                    .tab(key.into(), selected && self.client_settings.page == page)
                    .child(locale.text(key))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.management_tab = 4;
                        view.client_settings.page = page;
                        if page == Page::Notifications {
                            view.refresh_notification_permission(cx);
                        }
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, locale.text(key))
            }))
    }

    pub(super) fn render_client_settings(&mut self, cx: &mut Context<Self>) -> Div {
        let locale = self.client_locale();
        let t = |key| locale.text(key);
        let state = &self.client_settings;
        let title = match state.page {
            Page::Notifications => "client_notifications",
            Page::Appearance => "client_appearance",
            Page::Account => "client_account",
            Page::Data => "client_data",
        };
        let account_page = state.page == Page::Account;
        let content = match state.page {
            Page::Notifications => self.render_notification_settings(cx),
            Page::Appearance => self.render_appearance(cx),
            Page::Account => self.render_account(cx),
            Page::Data => {
                let data = zork_ui::settings::data::Data {
                    busy: state.reset.busy(),
                    error: state.reset.error.clone(),
                };
                let text =
                    zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into()));
                let view = if let Some(view) = &state.data {
                    view.clone()
                } else {
                    let view = cx.new(|cx| {
                        zork_ui::settings::data::DataSettings::new(data.clone(), text.clone(), cx)
                    });
                    cx.subscribe(&view, |v, _, _: &zork_ui::settings::data::Confirmed, _| {
                        v.source.clear_data(true)
                    })
                    .detach();
                    self.client_settings.data = Some(view.clone());
                    view
                };
                view.update(cx, |v, cx| v.configure(data, text, cx));
                div().child(view)
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .when(!account_page, |view| view.child(ui::page_title(t(title))))
            .child(content)
    }
}
