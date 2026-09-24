//! Presentational settings pages. Hosts own persistence, services and authentication.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FocusHandle, FontWeight};
use std::rc::Rc;
pub mod data;
mod notifications;
pub use notifications::{notifications, NotificationAction, NotificationData};
pub fn row(
    title: impl Into<gpui::SharedString>,
    detail: impl Into<gpui::SharedString>,
    control: impl IntoElement,
) -> Div {
    titled_row(
        ui::text_role(title, crate::design::TextRole::SectionTitle),
        detail,
        control,
    )
}

/// A settings row whose title is an element, such as a device label.
pub fn titled_row(
    title: impl IntoElement,
    detail: impl Into<gpui::SharedString>,
    control: impl IntoElement,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_4()
        .py(px(12.))
        .child(
            div()
                .flex_1()
                .min_w(px(160.))
                .flex_basis(px(160.))
                .flex()
                .flex_col()
                .gap_1()
                .child(title)
                .child(ui::text_role(detail, crate::design::TextRole::Description)),
        )
        .child(div().flex_shrink_0().child(control))
}

fn header(title: impl Into<gpui::SharedString>) -> Div {
    div().pb(px(18.)).child(ui::page_title(title))
}
#[derive(Clone, Debug)]
pub enum AccountAction {
    Login,
    Cancel,
    Logout,
    CopyIdentity,
}
#[derive(Clone, Default)]
pub struct AccountData {
    pub name: Option<String>,
    pub email: Option<String>,
    pub identity: Option<String>,
    pub busy: bool,
    pub signing_out: bool,
    pub notice: Option<String>,
}
/// A row that leads with its value: the primary fact at full contrast, an
/// optional muted line and trailing controls.
fn value_row(primary: impl IntoElement, meta: Option<String>, control: impl IntoElement) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap_4()
        .py(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(primary)
                .when_some(meta, |v, meta| {
                    v.child(crate::components::disclosure::meta(meta))
                }),
        )
        .child(div().flex_shrink_0().flex().items_center().gap_2().child(control))
}

pub fn account<V: 'static>(
    data: AccountData,
    window: &mut gpui::Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, AccountAction, &mut Context<V>) + 'static,
) -> Div {
    use crate::components::{disclosure, standard_menu::Item};
    let action = Rc::new(action);
    let login = action.clone();
    let cancel = action.clone();
    let menu = action.clone();
    let copy = action.clone();
    let signed = data.name.is_some();
    let primary = div()
        .text_size(px(14.))
        .font_weight(FontWeight::MEDIUM)
        .child(if signed {
            data.email.clone().or(data.name.clone()).unwrap_or_default()
        } else {
            "未登录".into()
        });
    let control = if signed {
        let items = vec![
            if data.busy {
                Item::new("zork-account-login", "重新验证").disabled()
            } else {
                Item::new("zork-account-login", "重新验证")
            },
            if data.busy {
                Item::new("zork-account-logout", "退出账号").disabled()
            } else {
                Item::new("zork-account-logout", "退出账号")
            },
        ];
        div()
            .flex()
            .gap_2()
            .when(data.busy && !data.signing_out, |v| {
                v.child(
                    ui::button("zork-account-cancel", "取消", false, true)
                        .on_click(
                            cx.listener(move |v, _, _, cx| cancel(v, AccountAction::Cancel, cx)),
                        )
                        .automation(AutomationRole::Button, "取消登录"),
                )
            })
            .child(disclosure::more_menu(
                "zork-account-more",
                items,
                !data.signing_out,
                window,
                cx,
                move |v, key, _, cx| match key.as_str() {
                    "zork-account-login" => menu(v, AccountAction::Login, cx),
                    "zork-account-logout" => menu(v, AccountAction::Logout, cx),
                    _ => {}
                },
            ))
            .into_any_element()
    } else {
        div()
            .flex()
            .gap_2()
            .child(
                ui::button(
                    "zork-account-login",
                    if data.busy {
                        "等待登录…"
                    } else {
                        "使用 Google 登录"
                    },
                    true,
                    !data.busy,
                )
                .on_click(cx.listener(move |v, _, _, cx| login(v, AccountAction::Login, cx)))
                .automation_enabled(!data.busy, AutomationRole::Button, "使用 Google 登录"),
            )
            .when(data.busy, |v| {
                v.child(
                    ui::button("zork-account-cancel", "取消", false, true)
                        .on_click(
                            cx.listener(move |v, _, _, cx| cancel(v, AccountAction::Cancel, cx)),
                        )
                        .automation(AutomationRole::Button, "取消登录"),
                )
            })
            .into_any_element()
    };
    let technical = data.identity.clone().map(|identity| {
        disclosure::expander(
            "client-identity-details",
            "技术信息",
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .font_family(crate::assets::CODE_FONT_FAMILY)
                        .child(format!("客户端身份 {identity}")),
                )
                .child(
                    ui::quiet_button("client-identity-copy", "复制", true, ui::IconButtonSize::Compact)
                        .on_click(
                            cx.listener(move |v, _, _, cx| copy(v, AccountAction::CopyIdentity, cx)),
                        )
                        .automation(AutomationRole::Button, "复制客户端身份"),
                ),
            window,
            cx,
        )
    });
    div()
        .flex()
        .flex_col()
        .child(header("账号"))
        .child(value_row(
            primary,
            signed.then(|| if data.signing_out { "正在退出…" } else { "Google 账号" }.into()),
            control,
        ))
        .children(technical)
        .when_some(data.notice, |v, text| {
            v.child(div().mt_4().child(ui::feedback(text)))
        })
}
#[derive(Clone, Debug)]
pub enum DeviceAction {
    Refresh,
    Rename,
    CheckUpdate,
    Upgrade,
    ToggleRunning,
    Background(bool),
    StartAtLogin(bool),
}
#[derive(Clone)]
pub struct DeviceData {
    pub status: crate::device_name::DeviceStatus,
    pub name: String,
    pub version: String,
    pub update_supported: bool,
    pub update_reason: Option<String>,
    pub latest_version: Option<String>,
    pub online: Option<bool>,
    pub local: bool,
    pub running: bool,
    pub background: bool,
    pub start_at_login: bool,
    pub busy: bool,
    pub notice: Option<String>,
}
pub fn device<V: 'static>(
    data: DeviceData,
    focus: &[FocusHandle; 2],
    window: &mut gpui::Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, DeviceAction, &mut Context<V>) + 'static,
) -> Div {
    use crate::components::{disclosure, standard_menu::Item};
    let action = Rc::new(action);
    let menu_action = action.clone();
    let upgrade = action.clone();
    let background = action.clone();
    let has_update = data
        .latest_version
        .as_ref()
        .is_some_and(|version| version != &data.version);
    let show_updates = !data.local || data.background;
    let can_update = show_updates && data.update_supported && !data.busy;
    let version_label = if data.version.starts_with(|c: char| c.is_ascii_digit()) {
        format!("版本 {}", data.version)
    } else {
        data.version.clone()
    };
    let enabled = |item: Item, on: bool| if on { item } else { item.disabled() };
    // Rare and maintenance actions live in 更多; the page shows state only.
    let mut items = vec![
        enabled(Item::new("device-rename", "重命名"), !data.busy),
        enabled(Item::new("device-refresh", "刷新状态"), !data.busy),
    ];
    if data.local && data.background {
        items.push(enabled(
            Item::new("local-node-login", "登录系统后自动启动").check(data.start_at_login),
            !data.busy,
        ));
    }
    if show_updates {
        let check = enabled(Item::new("device-check-update", "检查更新"), can_update);
        items.push(match (&data.update_reason, data.update_supported) {
            (Some(reason), false) => check.detail(reason.clone()),
            _ => check,
        });
    }
    if data.local {
        items.push(enabled(
            Item::new(
                "local-node-toggle",
                if data.running { "停止设备" } else { "启动设备" },
            ),
            !data.busy,
        ));
    }
    items.push(Item::new("device-version", version_label).disabled());
    let login_on = data.start_at_login;
    let more = disclosure::more_menu(
        "device-more",
        items,
        true,
        window,
        cx,
        move |v, key, _, cx| {
            let event = match key.as_str() {
                "device-rename" => DeviceAction::Rename,
                "device-refresh" => DeviceAction::Refresh,
                "device-check-update" => DeviceAction::CheckUpdate,
                "local-node-toggle" => DeviceAction::ToggleRunning,
                "local-node-login" => DeviceAction::StartAtLogin(!login_on),
                _ => return,
            };
            menu_action(v, event, cx)
        },
    );
    let state = if data.busy {
        Some("正在处理…".to_owned())
    } else if data.local && !data.running {
        Some("已停止".to_owned())
    } else {
        None
    };
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .id("device-runtime-summary")
                .flex()
                .items_center()
                .gap_3()
                .pb(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(crate::device_name::label(
                                    "settings-device-name",
                                    data.name.clone(),
                                    &data.status,
                                    None,
                                )),
                        )
                        .when_some(state, |v, state| v.child(disclosure::meta(state))),
                )
                .child(more),
        )
        .when(data.local, |v| {
            v.child(value_row(
                div().text_size(px(14.)).child("退出客户端后保持运行"),
                None,
                ui::switch(
                    "local-node-background",
                    "退出客户端后保持运行",
                    data.background,
                    !data.busy,
                    &focus[0],
                    cx,
                    move |v, on, cx| background(v, DeviceAction::Background(on), cx),
                ),
            ))
        })
        // The update row appears only when there is something to install.
        .when(has_update && show_updates, |v| {
            v.child(value_row(
                div().text_size(px(14.)).child(format!(
                    "可更新到 {}",
                    data.latest_version.as_deref().unwrap_or_default()
                )),
                Some("更新时会重启设备".into()),
                ui::button("device-upgrade", "更新", true, can_update)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if can_update {
                            upgrade(v, DeviceAction::Upgrade, cx)
                        }
                    }))
                    .automation_enabled(can_update, AutomationRole::Button, "更新并重启"),
            ))
        })
        .when_some(data.notice, |v, text| {
            v.child(div().mt_3().child(ui::feedback(text)))
        })
}

#[cfg(feature = "stories")]
pub struct SettingsStory {
    validate_name: fn(&str) -> Result<String, String>,
    rename_input: gpui::Entity<crate::components::text_input::ComposerInput>,
    rename_open: bool,
    rename_error: Option<String>,
    rename_modal: ui::ModalState,
    family: String,
    data: DeviceData,
    account: AccountData,
    focus: [FocusHandle; 2],
}
#[cfg(feature = "stories")]
impl SettingsStory {
    pub fn new(
        family: String,
        state: String,
        validate_name: fn(&str) -> Result<String, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let f = crate::stories::page_fixture();
        let text = |k: &str| f["account"][k].as_str().unwrap_or_default().to_owned();
        let rename_input =
            cx.new(|cx| crate::components::text_input::ComposerInput::new("设备名称", cx));
        cx.observe(&rename_input, |_, _, cx| cx.notify()).detach();
        Self {
            validate_name,
            rename_input,
            rename_open: false,
            rename_error: None,
            rename_modal: ui::ModalState::new(cx),
            family,
            data: DeviceData {
                status: if state == "stopped" {
                    crate::device_name::DeviceStatus::Offline
                } else {
                    crate::device_name::DeviceStatus::Direct
                },
                name: "mini1".into(),
                update_supported: true,
                update_reason: None,
                latest_version: None,
                version: f["device"]["version"].as_str().unwrap_or("0.1.30").into(),
                online: Some(state != "stopped"),
                local: true,
                running: state != "stopped",
                background: true,
                start_at_login: false,
                busy: state == "loading",
                notice: (state == "error").then(|| "暂时无法读取设备状态，请重试。".into()),
            },
            account: AccountData {
                name: (state == "signed-in").then(|| text("name")),
                email: (state == "signed-in").then(|| text("email")),
                identity: Some(text("identity")),
                busy: state == "loading",
                signing_out: false,
                notice: (state == "error").then(|| "登录未完成。请检查连接后重试。".into()),
            },
            focus: [cx.focus_handle(), cx.focus_handle()],
        }
    }
}
#[cfg(feature = "stories")]
impl gpui::Render for SettingsStory {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.rename_modal.sync(
            self.rename_open.then_some("device-rename-dialog"),
            window,
            cx,
        );
        let rename_visible = self
            .rename_modal
            .retain("device-rename-dialog", self.rename_open.then_some(()), cx)
            .is_some();
        let page = if self.family == "client" {
            account(self.account.clone(), window, cx, |v, event, cx| {
                match event {
                    AccountAction::Login => {
                        v.account.name = Some("Zork 用户".into());
                        v.account.email = Some("design@example.test".into());
                        v.account.busy = false;
                        v.account.notice = None
                    }
                    AccountAction::Cancel => v.account.busy = false,
                    AccountAction::Logout => {
                        v.account.name = None;
                        v.account.email = None
                    }
                    AccountAction::CopyIdentity => {
                        if let Some(id) = &v.account.identity {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(id.clone()));
                            v.account.notice = Some("客户端身份已复制。".into())
                        }
                    }
                }
                cx.notify();
            })
        } else {
            device(self.data.clone(), &self.focus, window, cx, |v, event, cx| {
                match event {
                    DeviceAction::Rename => {
                        v.rename_input
                            .update(cx, |i, cx| i.set_value(v.data.name.clone(), cx));
                        v.rename_open = true;
                        v.rename_error = None;
                    }
                    DeviceAction::CheckUpdate => {
                        v.data.latest_version = Some("0.1.31".into());
                    }
                    DeviceAction::Upgrade => {
                        v.data.version = v
                            .data
                            .latest_version
                            .take()
                            .unwrap_or(v.data.version.clone());
                        v.data.notice = Some("版本升级完成".into());
                    }
                    DeviceAction::Refresh => {
                        v.data.busy = false;
                        v.data.notice = Some("设备状态已刷新。".into())
                    }
                    DeviceAction::ToggleRunning => {
                        v.data.running = !v.data.running;
                        v.data.online = Some(v.data.running);
                        v.data.status = if v.data.running {
                            crate::device_name::DeviceStatus::Direct
                        } else {
                            crate::device_name::DeviceStatus::Offline
                        }
                    }
                    DeviceAction::Background(on) => {
                        v.data.background = on;
                        if !on {
                            v.data.start_at_login = false
                        }
                    }
                    DeviceAction::StartAtLogin(on) => v.data.start_at_login = on,
                }
                cx.notify();
            })
        };
        page.when(rename_visible, |page| {
            page.child(rename_device::render(
                &self.rename_input,
                false,
                self.rename_error.clone(),
                &self.rename_modal,
                window,
                cx,
                |v, action, cx| {
                    match action {
                        rename_device::Action::Cancel => v.rename_open = false,
                        rename_device::Action::Save => {
                            match (v.validate_name)(v.rename_input.read(cx).value()) {
                                Ok(name) => {
                                    v.data.name = name;
                                    v.rename_open = false;
                                }
                                Err(error) => v.rename_error = Some(error),
                            }
                        }
                    }
                    cx.notify();
                },
            ))
        })
        .into_any_element()
    }
}

pub mod rename_device;
