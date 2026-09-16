use super::*;
use crate::components::{text_input::ComposerSubmit, workbench as wb};
pub struct Story {
    chrome: Chrome,
    data: Data,
    locale: Text,
    sequence: usize,
}
impl Story {
    pub fn new(state: &str, locale: Text, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            ComposerInput::new(locale.text("browser_address_placeholder"), cx).single_line()
        });
        cx.subscribe(&address, |v, _, _: &ComposerSubmit, cx| {
            v.browser_action(Action::SubmitAddress, None, cx)
        })
        .detach();
        let mut data = Data {
            width: 640.,
            blank: true,
            can_grant: true,
            ..Default::default()
        };
        if matches!(state, "tabs" | "loading" | "error") {
            data.tabs = vec![Tab {
                id: "demo-web".into(),
                title: "产品原型".into(),
                url: "https://example.invalid".into(),
                loading: state == "loading",
            }];
            data.active = Some("demo-web".into());
            data.blank = false;
            data.native_pages = vec![NativePage {
                id: "history".into(),
                title: "执行历史".into(),
                icon: "icons/history.svg",
            }];
            address.update(cx, |input, cx| {
                input.set_value("https://example.invalid", cx)
            });
        }
        if state == "error" {
            data.error = Some("页面暂时无法连接".into());
        }
        if state == "applications" {
            data.applications = Arc::new(vec![zork_client_types::pages::ApplicationEntry {
                page: zork_client_types::pages::PageLink {
                    id: "demo-app".into(),
                    title: "项目资料".into(),
                    url: "https://example.invalid/materials".into(),
                    description: "团队发布的资料与页面".into(),
                },
                device_id: "demo-device".into(),
                device_name: "工作设备".into(),
                offline: false,
            }]);
        }
        let chrome = Chrome::new(address, locale.clone(), cx);
        Self {
            chrome,
            data,
            locale,
            sequence: 0,
        }
    }
}
impl Host for Story {
    fn browser_action(
        &mut self,
        action: Action,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::SelectNative(id) => self.data.active_native = Some(id),
            Action::CloseNative(id) => {
                self.data.native_pages.retain(|page| page.id != id);
                if self.data.active_native.as_ref() == Some(&id) {
                    self.data.active_native = None;
                }
            }
            Action::SelectTab(id) => {
                self.data.active = Some(id);
                self.data.active_native = None;
            }
            Action::CloseTab(id) => {
                self.data.tabs.retain(|tab| tab.id != id);
                if self.data.active.as_ref() == Some(&id) {
                    self.data.active = self.data.tabs.last().map(|tab| tab.id.clone());
                }
            }
            Action::ShowWeb => self.data.active_native = None,
            Action::CloseBlank => self.data.blank = false,
            Action::NewTab => {
                self.data.active = None;
                self.data.active_native = None;
                self.data.blank = true;
                self.chrome
                    .address
                    .update(cx, |input, cx| input.set_value("", cx));
            }
            Action::ToggleExpanded => self.data.expanded = !self.data.expanded,
            Action::ClearError => self.data.error = None,
            Action::FocusAddress => {
                self.chrome.menu.dismiss();
                if let Some(window) = window {
                    window.focus(&self.chrome.address.read(cx).focus_handle(), cx);
                }
            }
            Action::SubmitAddress => {
                let url = self.chrome.address.read(cx).value().to_owned();
                self.browser_action(Action::OpenApplication(url), None, cx);
            }
            Action::OpenApplication(url) => {
                self.sequence += 1;
                let id = format!("demo-{}", self.sequence);
                self.data.tabs.push(Tab {
                    id: id.clone(),
                    title: "演示页面".into(),
                    url: url.clone(),
                    loading: false,
                });
                self.data.active = Some(id);
                self.data.active_native = None;
                self.data.blank = false;
                self.chrome
                    .address
                    .update(cx, |input, cx| input.set_value(url, cx));
            }
            Action::ToggleGrant => self.data.granted = !self.data.granted,
            Action::ToggleInspect => self.data.inspecting = !self.data.inspecting,
            Action::Handoff => {
                self.data.granted = false;
                self.data.inspecting = false;
            }
            Action::Nav(_) => {
                for tab in &mut self.data.tabs {
                    tab.loading = false;
                }
                self.data.error = None;
            }
            Action::Downloads => {}
        }
        cx.notify();
    }
}
impl Render for Story {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.data.width = (window.viewport_size().width.as_f32() - 48.).min(640.);
        self.chrome
            .configure(self.data.clone(), self.locale.clone());
        wb::column(0.)
            .size_full()
            .child(self.chrome.render_tabs(cx))
            .when(self.data.active_native.is_none(), |v| {
                v.child(self.chrome.render_navigation(window, cx))
            })
            .children(self.chrome.render_error())
            .child(
                wb::column(0.)
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(self.chrome.render_empty(cx)),
            )
            .children(self.chrome.render_menu(window, cx))
    }
}
