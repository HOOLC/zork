//! Complete browser tabs, address controls, menus and published application list.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{text_input::ComposerInput, tooltip},
    controls as ui,
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, FontWeight, Window};
use std::sync::Arc;
#[derive(Clone)]
pub struct NativePage {
    pub id: String,
    pub title: String,
    pub icon: &'static str,
}
#[derive(Clone, Default)]
pub struct Tab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub loading: bool,
}
#[derive(Clone, Default)]
pub struct Data {
    pub width: f32,
    pub native_pages: Vec<NativePage>,
    pub active_native: Option<String>,
    pub tabs: Vec<Tab>,
    pub active: Option<String>,
    pub blank: bool,
    pub expanded: bool,
    pub error: Option<String>,
    pub granted: bool,
    pub grant_connected: bool,
    pub can_grant: bool,
    pub inspecting: bool,
    pub has_frame: bool,
    pub busy: usize,
    pub applications: Arc<Vec<zork_client_types::pages::ApplicationEntry>>,
}
pub enum Action {
    SelectNative(String),
    CloseNative(String),
    SelectTab(String),
    CloseTab(String),
    ShowWeb,
    CloseBlank,
    NewTab,
    ToggleExpanded,
    Nav(&'static str),
    ClearError,
    FocusAddress,
    SubmitAddress,
    Downloads,
    ToggleGrant,
    ToggleInspect,
    Handoff,
    OpenApplication(String),
}
pub trait Host: Sized + 'static {
    fn browser_action(
        &mut self,
        action: Action,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    );
}
pub struct Chrome {
    pub menu: crate::components::standard_menu::Menu,
    tab_group: crate::components::widgets::navigation::Group,
    address: Entity<ComposerInput>,
    locale: Text,
    data: Data,
}
impl std::ops::Deref for Chrome {
    type Target = Data;
    fn deref(&self) -> &Data {
        &self.data
    }
}
impl Chrome {
    pub fn new(address: Entity<ComposerInput>, locale: Text, cx: &mut gpui::App) -> Self {
        Self {
            menu: Default::default(),
            tab_group: crate::components::widgets::navigation::Group::new(cx)
                .kind(crate::components::widgets::navigation::Kind::Tabs),
            address,
            locale,
            data: Default::default(),
        }
    }
    pub fn configure(&mut self, data: Data, locale: Text) {
        self.data = data;
        self.locale = locale;
    }
    fn active_id(&self) -> Option<String> {
        self.active.clone()
    }
    fn active(&self) -> Option<&Tab> {
        self.tabs
            .iter()
            .find(|tab| Some(&tab.id) == self.active.as_ref())
    }
}
const TAB_WIDTH: f32 = 156.;
const CONTROL_SIZE: f32 = 28.;
const TAB_ROW_HEIGHT: f32 = ZORK_UI.thread.header_height;
const ADDRESS_ROW_HEIGHT: f32 = 40.;

fn icon_button(
    id: impl Into<gpui::ElementId>,
    path: &'static str,
    enabled: bool,
) -> crate::controls::Action {
    ui::icon_button_sized(id, enabled, ui::IconButtonSize::Compact)
        .child(ui::icon(path, 16.).text_color(rgb(ZORK_UI.palette.subtle)))
}
fn hint(
    button: crate::controls::Action,
    id: &'static str,
    label: String,
    enabled: bool,
) -> impl IntoElement {
    tooltip::hint(
        button.automation_enabled(enabled, AutomationRole::Button, label.clone()),
        id,
        label,
    )
}

impl Chrome {
    pub fn render_tabs<V: Host>(&self, cx: &mut Context<V>) -> impl IntoElement {
        let p = ZORK_UI.palette;
        let locale = self.locale.clone();
        let active = self.active_id();
        let blank_tab = active.is_none() && (self.active_native.is_none() || self.blank);
        let tab_count = self.native_pages.len() + self.tabs.len() + usize::from(blank_tab);
        let tab_width = ((self.width - 168. - 4. * tab_count.saturating_sub(1) as f32)
            / tab_count.max(1) as f32)
            .clamp(96., TAB_WIDTH);
        self.tab_group.surface(
            div()
                .h(px(TAB_ROW_HEIGHT))
                .border_color(rgb(p.border))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .pl_2()
                .pr(px(56.))
                .child(
                    div()
                        .id("browser-tabs")
                        .min_w_0()
                        .max_w(px((self.width - 168.).max(80.)))
                        .flex()
                        .items_center()
                        .gap_1()
                        .overflow_x_scroll()
                        .children(self.native_pages.clone().into_iter().map(|page| {
                            let selected = self.active_native.as_ref() == Some(&page.id);
                            let id = page.id.clone();
                            let close = page.id.clone();
                            self.tab_group
                                .row(format!("page-tab-{id}"), selected, true)
                                .w(px(tab_width))
                                .h(px(CONTROL_SIZE))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .pr_1()
                                .rounded(px(crate::design::RADIUS.control))
                                .text_size(px(13.))
                                .text_color(rgb(if selected { p.text } else { p.muted }))
                                .when(selected, |v| v.font_weight(FontWeight::MEDIUM))
                                .focusable()
                                .tab_stop(true)
                                .cursor_pointer()
                                .on_click(cx.listener(move |v, _, window, cx| {
                                    v.browser_action(
                                        Action::SelectNative(id.clone()),
                                        Some(window),
                                        cx,
                                    )
                                }))
                                .child(ui::icon(page.icon, 16.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(page.title.clone()),
                                )
                                .child(
                                    icon_button(
                                        format!("page-close-{close}"),
                                        "interface/x.svg",
                                        true,
                                    )
                                    .size(px(24.))
                                    .on_click(cx.listener(move |v, _, window, cx| {
                                        cx.stop_propagation();
                                        v.browser_action(
                                            Action::CloseNative(close.clone()),
                                            Some(window),
                                            cx,
                                        );
                                    }))
                                    .automation(
                                        AutomationRole::Button,
                                        format!(
                                            "{} {}",
                                            locale.text("browser_close_tab"),
                                            page.title
                                        ),
                                    ),
                                )
                                .automation(AutomationRole::Button, page.title)
                        }))
                        .children(self.tabs.clone().into_iter().map(|tab| {
                            let id = tab.id.clone();
                            let close = id.clone();
                            let selected =
                                self.active_native.is_none() && active.as_ref() == Some(&id);
                            let title = if tab.title.is_empty() {
                                tab.url
                            } else {
                                tab.title
                            };
                            self.tab_group
                                .row(format!("browser-tab-{id}"), selected, true)
                                .w(px(tab_width))
                                .h(px(CONTROL_SIZE))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .pr_1()
                                .rounded(px(crate::design::RADIUS.control))
                                .text_size(px(13.))
                                .text_color(rgb(if selected { p.text } else { p.muted }))
                                .when(selected, |v| v.font_weight(FontWeight::MEDIUM))
                                .focusable()
                                .tab_stop(true)
                                .cursor_pointer()
                                .on_click(cx.listener(move |v, _, window, cx| {
                                    v.browser_action(
                                        Action::SelectTab(id.clone()),
                                        Some(window),
                                        cx,
                                    )
                                }))
                                .child(
                                    ui::icon("browser/globe.svg", 16.)
                                        .text_color(rgb(if selected { p.text } else { p.muted })),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(title.clone()),
                                )
                                .child(
                                    icon_button(
                                        format!("browser-close-{close}"),
                                        "interface/x.svg",
                                        true,
                                    )
                                    .size(px(24.))
                                    .on_click(cx.listener(move |v, _, window, cx| {
                                        cx.stop_propagation();
                                        v.browser_action(
                                            Action::CloseTab(close.clone()),
                                            Some(window),
                                            cx,
                                        );
                                    }))
                                    .automation(
                                        AutomationRole::Button,
                                        format!("{} {title}", locale.text("browser_close_tab")),
                                    ),
                                )
                                .automation(
                                    AutomationRole::Button,
                                    format!("{} {title}", locale.text("browser_tab")),
                                )
                        }))
                        .when(blank_tab, |v| {
                            v.child(
                                self.tab_group
                                    .row("browser-blank-tab", self.active_native.is_none(), true)
                                    .w(px(tab_width))
                                    .h(px(CONTROL_SIZE))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl_2()
                                    .pr_1()
                                    .rounded(px(crate::design::RADIUS.control))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|v, _, window, cx| {
                                        v.browser_action(Action::ShowWeb, Some(window), cx);
                                    }))
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(
                                        ui::icon("browser/globe.svg", 16.).text_color(rgb(p.text)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(locale.text("browser_new_tab")),
                                    )
                                    .child(
                                        icon_button("browser-close-blank", "interface/x.svg", true)
                                            .size(px(24.))
                                            .on_click(cx.listener(|v, _, window, cx| {
                                                cx.stop_propagation();
                                                v.browser_action(
                                                    Action::CloseBlank,
                                                    Some(window),
                                                    cx,
                                                );
                                            }))
                                            .automation(
                                                AutomationRole::Button,
                                                locale.text("browser_close_tab"),
                                            ),
                                    ),
                            )
                        }),
                )
                .child(hint(
                    icon_button("browser-new-tab", "interface/plus.svg", true).on_click(
                        cx.listener(|v, _, window, cx| {
                            v.browser_action(Action::NewTab, Some(window), cx);
                        }),
                    ),
                    "browser-new-tab",
                    locale.text("browser_new_tab"),
                    true,
                ))
                .child(div().flex_1())
                .child(hint(
                    icon_button(
                        "browser-expand",
                        if self.expanded {
                            "browser/restore.svg"
                        } else {
                            "browser/expand.svg"
                        },
                        true,
                    )
                    .on_click(cx.listener(|v, _, window, cx| {
                        v.browser_action(Action::ToggleExpanded, Some(window), cx);
                    })),
                    "browser-expand",
                    locale.text(if self.expanded {
                        "browser_restore"
                    } else {
                        "browser_expand"
                    }),
                    true,
                )),
        )
    }

    pub fn render_navigation<V: Host>(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let p = ZORK_UI.palette;
        let locale = self.locale.clone();
        let has_tab = self.active().is_some();
        let loading = self.active().is_some_and(|tab| tab.loading);
        let invalid = self.error.is_some();
        div()
            .h(px(ADDRESS_ROW_HEIGHT))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .border_color(rgb(p.border))
            .child(hint(
                icon_button("browser-back", "interface/arrow-left.svg", has_tab).when(
                    has_tab,
                    |v| {
                        v.on_click(cx.listener(|v, _, window, cx| {
                            v.browser_action(Action::Nav("back"), Some(window), cx)
                        }))
                    },
                ),
                "browser-back",
                locale.text("browser_back"),
                has_tab,
            ))
            .child(hint(
                icon_button("browser-forward", "interface/arrow-right.svg", has_tab).when(
                    has_tab,
                    |v| {
                        v.on_click(cx.listener(|v, _, window, cx| {
                            v.browser_action(Action::Nav("forward"), Some(window), cx)
                        }))
                    },
                ),
                "browser-forward",
                locale.text("browser_forward"),
                has_tab,
            ))
            .child(hint(
                icon_button(
                    "browser-reload",
                    if loading {
                        "interface/x.svg"
                    } else {
                        "interface/reload.svg"
                    },
                    true,
                )
                .on_click(cx.listener(move |v, _, window, cx| {
                    if has_tab {
                        v.browser_action(
                            Action::Nav(if loading { "stop" } else { "reload" }),
                            Some(window),
                            cx,
                        );
                    } else {
                        v.browser_action(Action::ClearError, Some(window), cx);
                    }
                })),
                "browser-reload",
                locale.text(if loading {
                    "browser_stop"
                } else {
                    "browser_reload"
                }),
                true,
            ))
            .child(
                crate::components::widgets::controls::adaptive_input(
                    "browser-address",
                    &self.address,
                    invalid,
                    p.canvas,
                )
                .flex_1()
                .min_w_0()
                .h(px(CONTROL_SIZE))
                .min_h_0()
                .pl_2()
                .pr_1()
                .gap_1()
                .text_size(px(13.))
                .line_height(px(20.))
                .on_click(cx.listener(|v, _, window, cx| {
                    v.browser_action(Action::FocusAddress, Some(window), cx);
                }))
                .editor_slot(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(20.))
                        .overflow_hidden()
                        .child(self.address.clone()),
                )
                .child(
                    icon_button("browser-go", "browser/go.svg", true)
                        .size(px(22.))
                        .on_click(cx.listener(|v, _, window, cx| {
                            v.browser_action(Action::SubmitAddress, Some(window), cx)
                        }))
                        .automation(AutomationRole::Button, locale.text("browser_go")),
                )
                .automation(AutomationRole::TextInput, locale.text("browser_address")),
            )
            .child(hint(
                icon_button("browser-downloads", "browser/download.svg", true).on_click(
                    cx.listener(|v, _, window, cx| {
                        v.browser_action(Action::Downloads, Some(window), cx);
                    }),
                ),
                "browser-downloads",
                locale.text("browser_downloads"),
                true,
            ))
            .child({
                let focus =
                    crate::components::widgets::controls::action_focus("browser-more", window, cx);
                self.menu
                    .trigger_element(
                        icon_button("browser-more", "browser/more.svg", true),
                        &focus,
                        true,
                        cx,
                    )
                    .automation(AutomationRole::Button, locale.text("browser_more"))
            })
    }

    pub fn render_menu<V: Host>(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<V>,
    ) -> Option<gpui::AnyElement> {
        use crate::components::standard_menu::{Item, ItemKind};
        let locale = self.locale.clone();
        let granted = self.granted;
        let mut grant = Item::new(
            "browser-agent-grant",
            locale.text(if self.grant_connected {
                "browser_granted"
            } else if granted {
                "browser_connecting"
            } else {
                "browser_grant"
            }),
        )
        .icon("icons/permission.svg");
        grant.disabled = !self.can_grant;
        grant.kind = ItemKind::Check(granted);
        let mut inspect = Item::new(
            "browser-inspect",
            locale.text(if self.inspecting {
                "browser_inspect_cancel"
            } else {
                "browser_inspect"
            }),
        )
        .icon("icons/review.svg");
        inspect.disabled = self.active().is_none() || !self.has_frame;
        let mut handoff =
            Item::new("browser-handoff", locale.text("browser_handoff")).icon("browser/go.svg");
        handoff.disabled = self.active().is_none();
        self.menu.render(
            "browser-menu",
            vec![
                grant,
                inspect,
                Item::separator("browser-menu-separator"),
                handoff,
            ],
            window,
            cx,
            |v, key, window, cx| {
                let action = match key.as_str() {
                    "browser-agent-grant" => Action::ToggleGrant,
                    "browser-inspect" => Action::ToggleInspect,
                    "browser-handoff" => Action::Handoff,
                    _ => return,
                };
                v.browser_action(action, Some(window), cx);
            },
        )
    }

    pub fn render_empty<V: Host>(&self, cx: &mut Context<V>) -> impl IntoElement {
        let p = ZORK_UI.palette;
        let loading = self.active().is_some() || self.busy > 0;
        if !loading && !self.applications.is_empty() {
            let applications = self.applications.clone();
            let locale = self.locale.clone();
            return div()
                .absolute()
                .inset_0()
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .child(ui::text_role(
                    locale.text("applications"),
                    crate::design::TextRole::SectionTitle,
                ))
                .child(
                    gpui::uniform_list(
                        "published-applications",
                        applications.len(),
                        cx.processor(move |_: &mut V, range: std::ops::Range<usize>, _, cx| {
                            range
                                .map(|index| {
                                    let app = &applications[index];
                                    let url = app.page.url.clone();
                                    let meta = if app.offline {
                                        format!(
                                            "{} · {}",
                                            crate::device_name::summary(
                                                &app.device_name,
                                                &app.device_status,
                                                Some(&locale)
                                            ),
                                            locale.text("application_offline")
                                        )
                                    } else if app.page.description.is_empty() {
                                        crate::device_name::summary(
                                            &app.device_name,
                                            &app.device_status,
                                            Some(&locale),
                                        )
                                    } else {
                                        app.page.description.clone()
                                    };
                                    div().h(px(56.)).pb_2().child(
                                        crate::components::attachments::content_row(
                                            format!("application-{}", app.page.id),
                                            "browser/globe.svg",
                                            app.page.title.clone(),
                                            meta,
                                            cx,
                                            move |panel, cx| {
                                                panel.browser_action(
                                                    Action::OpenApplication(url.clone()),
                                                    None,
                                                    cx,
                                                )
                                            },
                                        ),
                                    )
                                })
                                .collect()
                        }),
                    )
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                );
        }
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                div()
                    .w_full()
                    .max_w(px(280.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .when(loading, |v| {
                        v.child(crate::components::loading::indicator(
                            "browser-loading",
                            24.,
                        ))
                    })
                    .when(!loading, |v| {
                        v.child(ui::icon("browser/globe.svg", 32.).text_color(rgb(p.muted)))
                    })
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child(self.locale.text(if loading {
                                "browser_loading"
                            } else {
                                "applications"
                            })),
                    )
                    .when(!loading, |v| {
                        v.child(
                            div()
                                .text_size(px(13.))
                                .line_height(px(20.))
                                .text_center()
                                .text_color(rgb(p.muted))
                                .child(self.locale.text("applications_empty")),
                        )
                    }),
            )
    }
}

impl Chrome {
    pub fn render_error(&self) -> Option<gpui::AnyElement> {
        self.error.clone().map(|error| {
            div()
                .id("browser-error")
                .max_h(px(96.))
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .flex()
                .items_start()
                .gap_2()
                .text_size(px(12.))
                .line_height(px(18.))
                .text_color(rgb(ZORK_UI.palette.danger))
                .child(ui::icon("icons/attention.svg", 16.).text_color(rgb(ZORK_UI.palette.danger)))
                .child(div().flex_1().min_w_0().child(error.clone()))
                .automation(AutomationRole::Status, error)
                .into_any_element()
        })
    }
}
#[cfg(feature = "stories")]
pub mod stories;
