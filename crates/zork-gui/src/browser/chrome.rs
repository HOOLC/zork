//! Host intents for the shared browser chrome. Engine work stays in the core adapter.
use super::{BrowserPanel, BrowserVisibility};
use gpui::{prelude::*, Context, Window};
use zork_client_core::desktop::browser_engine::Action as EngineAction;
use zork_ui::browser_chrome::{Action, Host};
impl BrowserPanel {
    pub(super) fn sync_chrome(&mut self) {
        let locale = self.locale;
        self.chrome.configure(zork_ui::browser_chrome::Data {
            width: self.width, native_pages: self.native_pages.clone(), active_native: self.active_native.clone(),
            tabs: self.tabs.iter().map(|tab| zork_ui::browser_chrome::Tab { id: tab.id.clone(), title: tab.title.clone(), url: tab.url.clone(), loading: tab.loading }).collect(),
            active: self.active_id(), blank: self.blank.contains(&self.host), expanded: self.expanded,
            error: self.error.clone(), granted: self.grants.contains_key(&self.host), grant_connected: self.grant_connected,
            can_grant: self.connection.is_some(), inspecting: self.inspecting, has_frame: self.frame.is_some(), busy: self.busy,
            applications: self.applications.clone(),
        }, zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into())));
    }
    pub(super) fn render_tabs(&self, cx: &mut Context<Self>) -> gpui::AnyElement { self.chrome.render_tabs(cx).into_any_element() }
    pub(super) fn render_navigation(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement { self.chrome.render_navigation(window, cx).into_any_element() }
    pub(super) fn render_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> { self.chrome.render_menu(window, cx) }
    pub(super) fn render_empty(&self, cx: &mut Context<Self>) -> gpui::AnyElement { self.chrome.render_empty(cx).into_any_element() }
}
impl Host for BrowserPanel {
    fn browser_action(&mut self, action: Action, window: Option<&mut Window>, cx: &mut Context<Self>) {
        match action {
            Action::SelectNative(id) => self.select_native_page(id, cx),
            Action::CloseNative(id) => { self.close_native_page(&id, cx); cx.emit(super::NativePageClosed(id)); },
            Action::SelectTab(id) => self.select(id, cx),
            Action::CloseTab(id) => self.command(EngineAction::Close { tab_id: id }, cx),
            Action::ShowWeb => { self.show_web_page(cx); cx.notify(); },
            Action::CloseBlank => {
                self.blank.remove(&self.host);
                if let Some(page) = self.native_pages.last() { self.select_native_page(page.id.clone(), cx); }
                else if let Some(tab) = self.tabs.last() { self.select(tab.id.clone(), cx); }
                else { self.toggle(cx); }
            }
            Action::NewTab => {
                self.show_web_page(cx); self.stop_viewport(); self.worker.clear_selection(&self.host);
                self.blank.insert(self.host.clone()); self.selected.remove(&self.host); self.set_frame(None, cx);
                self.sequence = 0; self.generation += 1; self.inspecting = false; self.chrome.menu.dismiss(); self.error = None;
                self.sync_address(cx); window.unwrap().focus(&self.address.read(cx).focus_handle(), cx); cx.notify();
            }
            Action::ToggleExpanded => { self.expanded = !self.expanded; self.chrome.menu.dismiss(); cx.emit(BrowserVisibility); cx.notify(); },
            Action::Nav(direction) => self.nav(direction, cx),
            Action::ClearError => { self.error = None; cx.notify(); },
            Action::FocusAddress => { self.chrome.menu.dismiss(); window.unwrap().focus(&self.address.read(cx).focus_handle(), cx); },
            Action::SubmitAddress => self.submit_address(cx),
            Action::Downloads => {
                self.chrome.menu.dismiss();
                if let Ok(rx) = self.worker.submit(|browser| browser.open_downloads()) { self.await_result(rx, |_, _, _| {}, cx); }
            }
            Action::ToggleGrant => {
                if self.grants.remove(&self.host).is_none() {
                    if let Some((client, session)) = &self.connection {
                        self.grants.insert(self.host.clone(), super::super::bridge::Grant::start(self.worker.clone(), client.clone(), session.clone(), self.host.clone(), cx));
                    }
                }
                cx.notify();
            }
            Action::ToggleInspect => { self.inspecting = !self.inspecting; if self.inspecting { window.unwrap().focus(&self.focus, cx); } cx.notify(); },
            Action::Handoff => { self.grants.remove(&self.host); self.inspecting = false; window.unwrap().focus(&self.focus, cx); cx.notify(); },
            Action::OpenApplication(url) => self.open_shared_link(url, cx),
        }
    }
}
