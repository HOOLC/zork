//! Device-local appearance and account preferences.
use super::{ui, DesktopRoot};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    i18n::Locale,
};
use gpui::{div, prelude::*, Context, Div, Entity};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Appearance,
    Notifications,
    Account,
    Data,
}
#[derive(Default)]
pub struct State {
    pub page: Page,
    pub locale: Locale,
    pub message_preview_height: u32,
    pub(super) appearance: Option<Entity<zork_ui::settings::appearance::Appearance>>,
    pub account_available: bool,
    pub notification_permission: super::notifications::Permission,
    pub notification_error: Option<String>,
    pub notification_busy: bool,
    pub(super) data: Option<Entity<zork_ui::settings::data::DataSettings>>,
    pub(super) reset: zork_client_core::data_reset::Snapshot,
    pub(super) reset_updates: Option<gpui::Task<()>>,
}

pub(crate) use zork_client_core::preferences::{MESSAGE_PREVIEW_MAX, MESSAGE_PREVIEW_MIN};

#[cfg(test)]
fn dragged_preview_height(start: u32, delta: f32) -> u32 {
    zork_ui::settings::appearance::dragged_height(start, delta, MESSAGE_PREVIEW_MIN, MESSAGE_PREVIEW_MAX)
}

pub(crate) fn load_message_preview_height(store: &super::store::ClientStore) -> u32 {
    zork_client_core::preferences::read(store).message_preview_height
}

pub(crate) fn message_preview_limit(height: u32, available: f32) -> f32 {
    if height == 0 {
        (available * 0.45).clamp(80., 240.)
    } else {
        height as f32
    }
}

impl DesktopRoot {
    pub(super) fn watch_data_reset(&mut self, cx: &mut Context<Self>) {
        let mut updates = self.source.data_reset.subscribe();
        self.client_settings.reset = (*updates.snapshot()).clone();
        self.client_settings.reset_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(snapshot) = updates.changed().await {
                if this.update(cx, |view, cx| {
                    view.client_settings.reset = (*snapshot).clone();
                    if snapshot.phase == zork_client_core::data_reset::Phase::Restarting { cx.quit(); }
                    cx.notify();
                }).is_err() { return; }
            }
        }));
    }
    fn save_message_preview_height(&mut self, height: u32, cx: &mut Context<Self>) {
        if let Err(error) = self.source.save_message_preview_height(height) {
            self.error = Some(error.to_string());
        }
        cx.notify();
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
        let mut pages = vec![
            (Page::Appearance, "client_appearance"),
            (Page::Notifications, "client_notifications"),
        ];
        if self.client_settings.account_available || self.identity.is_some() {
            pages.push((Page::Account, "client_account"));
        }
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
            Page::Appearance => "client_appearance",
            Page::Notifications => "client_notifications",
            Page::Account => "client_account",
            Page::Data => "client_data",
        };
        let content = match state.page {
            Page::Appearance => {
                let data = zork_ui::settings::appearance::Data { height: state.message_preview_height,
                    automatic: 240, minimum: MESSAGE_PREVIEW_MIN, maximum: MESSAGE_PREVIEW_MAX };
                let text = zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into()));
                let view = if let Some(view) = &state.appearance { view.clone() } else {
                    let view = cx.new(|cx| zork_ui::settings::appearance::Appearance::new(data, text.clone(), cx));
                    cx.subscribe(&view, |v, _, event: &zork_ui::settings::appearance::Changed, cx| v.save_message_preview_height(event.0, cx)).detach();
                    self.client_settings.appearance = Some(view.clone()); view
                };
                view.update(cx, |v, cx| v.configure(data, text, cx));
                div().child(view)
            }
            Page::Notifications => self.render_notification_settings(cx),
            Page::Account => self.render_account(cx),
            Page::Data => {
                let data = zork_ui::settings::data::Data { busy: state.reset.busy(), error: state.reset.error.clone() };
                let text = zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into()));
                let view = if let Some(view) = &state.data { view.clone() } else {
                    let view = cx.new(|cx| zork_ui::settings::data::DataSettings::new(data.clone(), text.clone(), cx));
                    cx.subscribe(&view, |v, _, _: &zork_ui::settings::data::Confirmed, _| v.source.clear_data(true)).detach();
                    self.client_settings.data = Some(view.clone()); view
                };
                view.update(cx, |v, cx| v.configure(data, text, cx));
                div().child(view)
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(ui::page_title(t(title)))
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_client_core::preferences::MESSAGE_PREVIEW_HEIGHT_KEY;
    #[test]
    fn message_preview_preferences_survive_reopening_and_reject_invalid_values() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = super::super::store::ClientStore::open(directory.path()).unwrap();
            assert_eq!(load_message_preview_height(&store), 0);
            store
                .put("device", MESSAGE_PREVIEW_HEIGHT_KEY, &277_u32)
                .unwrap();
        }
        let store = super::super::store::ClientStore::open(directory.path()).unwrap();
        assert_eq!(load_message_preview_height(&store), 277);
        store
            .put("device", MESSAGE_PREVIEW_HEIGHT_KEY, &99999_u32)
            .unwrap();
        assert_eq!(load_message_preview_height(&store), 0);
        store
            .put("device", MESSAGE_PREVIEW_HEIGHT_KEY, &"invalid")
            .unwrap();
        assert_eq!(load_message_preview_height(&store), 0);
    }

    #[test]
    fn message_preview_default_preserves_adaptive_height() {
        assert_eq!(message_preview_limit(0, 100.), 80.);
        assert_eq!(message_preview_limit(0, 400.), 180.);
        assert_eq!(message_preview_limit(0, 1000.), 240.);
        assert_eq!(message_preview_limit(480, 1000.), 480.);
    }

    #[test]
    fn preview_drag_is_continuous_and_bounded() {
        assert_eq!(dragged_preview_height(240, 37.), 277);
        assert_eq!(dragged_preview_height(240, -1000.), 80);
        assert_eq!(dragged_preview_height(240, 1000.), 720);
    }
}
