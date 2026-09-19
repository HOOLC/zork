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
    div()
        .flex()
        .items_center()
        .gap_5()
        .py(px(14.))
        .border_b(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .child(detail.into()),
                ),
        )
        .child(control)
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
pub fn account<V: 'static>(
    data: AccountData,
    cx: &Context<V>,
    action: impl Fn(&mut V, AccountAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let login = action.clone();
    let cancel = action.clone();
    let logout = action.clone();
    let copy = action.clone();
    let signed = data.name.is_some();
    div()
        .flex()
        .flex_col()
        .child(header("账号"))
        .child(
            div()
                .text_size(px(11.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child("Zork 账号用于公网连接；局域网连接无需登录。"),
        )
        .child(row(
            "账号",
            data.name
                .clone()
                .map(|name| {
                    format!(
                        "{}{}",
                        name,
                        data.email
                            .as_ref()
                            .map(|e| format!(" · {e}"))
                            .unwrap_or_default()
                    )
                })
                .unwrap_or("使用 Google 账号登录 Zork，启用跨网络连接。".into()),
            div()
                .flex()
                .gap_2()
                .child(
                    ui::button(
                        "zork-account-login",
                        if data.signing_out {
                            "正在退出…"
                        } else if data.busy {
                            "等待登录…"
                        } else if signed {
                            "重新验证"
                        } else {
                            "使用 Google 登录"
                        },
                        false,
                        !data.busy,
                    )
                    .on_click(cx.listener(move |v, _, _, cx| login(v, AccountAction::Login, cx)))
                    .automation_enabled(
                        !data.busy,
                        AutomationRole::Button,
                        "使用 Google 登录",
                    ),
                )
                .when(data.busy && !data.signing_out, |v| {
                    v.child(
                        ui::button("zork-account-cancel", "取消", false, true)
                            .on_click(
                                cx.listener(move |v, _, _, cx| {
                                    cancel(v, AccountAction::Cancel, cx)
                                }),
                            )
                            .automation(AutomationRole::Button, "取消登录"),
                    )
                })
                .when(signed, |v| {
                    v.child(
                        ui::button("zork-account-logout", "退出账号", false, !data.busy)
                            .on_click(
                                cx.listener(move |v, _, _, cx| {
                                    logout(v, AccountAction::Logout, cx)
                                }),
                            )
                            .automation_enabled(!data.busy, AutomationRole::Button, "退出账号"),
                    )
                }),
        ))
        .when(data.identity.is_some(), |view| {
            view.child(row(
                "客户端身份",
                data.identity
                    .clone()
                    .unwrap_or("正在准备客户端身份…".into()),
                ui::button(
                    "client-identity-copy",
                    "复制身份",
                    false,
                    data.identity.is_some(),
                )
                .on_click(cx.listener(move |v, _, _, cx| copy(v, AccountAction::CopyIdentity, cx)))
                .automation_enabled(
                    data.identity.is_some(),
                    AutomationRole::Button,
                    "复制客户端身份",
                ),
            ))
        })
        .child(row(
            "配置归属",
            "小伙伴 和模型连接分别保存在所属设备中。",
            div(),
        ))
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
    rename_source: crate::components::liquid::overlay::SourceBinding,
    cx: &Context<V>,
    action: impl Fn(&mut V, DeviceAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let refresh = action.clone();
    let rename = action.clone();
    let check_update = action.clone();
    let upgrade = action.clone();
    let toggle = action.clone();
    let background = action.clone();
    let login = action.clone();
    let p = ZORK_UI.palette;
    let has_update = data
        .latest_version
        .as_ref()
        .is_some_and(|version| version != &data.version);
    let online_ink = match data.online {
        Some(true) => p.success,
        Some(false) => p.muted,
        None => p.warning,
    };
    let show_updates = !data.local || data.background;
    let can_update = show_updates && data.update_supported && !data.busy;
    let version_label = if data.version.starts_with(|c: char| c.is_ascii_digit()) {
        format!("v{}", data.version)
    } else {
        data.version.clone()
    };
    let update_button = ui::button("device-check-update", "检查更新", false, can_update)
        .on_click(cx.listener(move |v, _, _, cx| {
            if can_update {
                check_update(v, DeviceAction::CheckUpdate, cx)
            }
        }))
        .automation_enabled(can_update, AutomationRole::Button, "检查更新");
    let update_button = if !data.update_supported {
        crate::components::tooltip::hint(
            update_button,
            "node-update",
            data.update_reason
                .clone()
                .unwrap_or_else(|| "当前设备不支持在线更新。".into()),
        )
        .into_any_element()
    } else {
        update_button.into_any_element()
    };
    // Hallmark · settings hierarchy: operational state → run mode → maintenance.
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .id("device-runtime-summary")
                .flex()
                .items_center()
                .gap_4()
                .p_5()
                .rounded(px(ui::CARD_RADIUS))
                .bg(rgb(p.sidebar))
                .child(
                    div()
                        .size(px(44.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(ui::FIELD_RADIUS))
                        .bg(rgb(p.canvas))
                        .child(ui::icon("icons/node.svg", 24.).text_color(rgb(p.muted))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(px(16.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(data.name.clone()),
                                )
                                .child(crate::components::tooltip::hint(
                                    ui::icon_button("device-rename", !data.busy)
                                        .flex_shrink_0()
                                        .child(
                                            ui::icon("icons/edit.svg", 14.)
                                                .text_color(rgb(p.muted)),
                                        )
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            rename(v, DeviceAction::Rename, cx)
                                        }))
                                        .map(|button| {
                                            rename_source.bind(
                                                button,
                                                "",
                                                ui::ActionStyle {
                                                    quiet: true,
                                                    icon: Some("icons/edit.svg"),
                                                    disabled: data.busy,
                                                    ..Default::default()
                                                },
                                            )
                                        })
                                        .automation_enabled(
                                            !data.busy,
                                            AutomationRole::Button,
                                            "修改设备名称",
                                        ),
                                    "device-rename",
                                    "修改设备名称",
                                )),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div().size(px(5.)).rounded_full().bg(rgb(online_ink)),
                                        )
                                        .child(match data.online {
                                            Some(true) => "在线",
                                            Some(false) => "离线",
                                            None => "连接中",
                                        }),
                                )
                                .when(data.local, |v| {
                                    v.child("·").child(if data.busy {
                                        "正在处理…"
                                    } else if data.running {
                                        "运行中"
                                    } else {
                                        "已停止"
                                    })
                                })
                                .child("·")
                                .child(version_label),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .child(crate::components::tooltip::hint(
                            ui::icon_button("device-refresh", !data.busy)
                                .child(ui::icon("icons/reload.svg", 14.))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    refresh(v, DeviceAction::Refresh, cx)
                                }))
                                .automation_enabled(
                                    !data.busy,
                                    AutomationRole::Button,
                                    "刷新设备状态",
                                ),
                            "device-refresh",
                            "刷新设备状态",
                        ))
                        .when(data.local, |v| {
                            v.child(
                                ui::button(
                                    "local-node-toggle",
                                    if data.running {
                                        "停止设备"
                                    } else {
                                        "启动设备"
                                    },
                                    !data.running,
                                    !data.busy,
                                )
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    toggle(v, DeviceAction::ToggleRunning, cx)
                                }))
                                .automation_enabled(
                                    !data.busy,
                                    AutomationRole::Button,
                                    "切换本机设备运行",
                                ),
                            )
                        }),
                ),
        )
        .when(data.local, |v| {
            v.child(
                div()
                    .mt_6()
                    .mb_3()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("运行方式"),
            )
            .child(crate::components::liquid::controls::deferred_segmented(
                "local-node-mode",
                [
                    ("local-node-foreground", "随客户端"),
                    ("local-node-background", "后台运行"),
                ]
                .into_iter()
                .map(|(id, label)| crate::components::liquid::controls::Segment {
                    id: id.into(),
                    label: label.into(),
                    disabled: false,
                })
                .collect(),
                vec![
                    Some("关闭客户端时，设备一同停止。".into()),
                    Some("退出客户端后，设备继续运行。".into()),
                ],
                Some(usize::from(data.background)),
                crate::components::liquid::controls::SegmentKind::Choice,
                !data.busy,
                p.canvas,
                cx.listener(move |v, index: &usize, _, cx| {
                    background(v, DeviceAction::Background(*index == 1), cx)
                }),
            ))
            .when(data.background, |v| {
                v.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .mt_3()
                        .min_h(px(40.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(13.))
                                .child("登录系统后自动启动"),
                        )
                        .child(ui::switch(
                            "local-node-login",
                            "登录系统后自动启动",
                            data.start_at_login,
                            !data.busy,
                            &focus[1],
                            cx,
                            move |v, on, cx| login(v, DeviceAction::StartAtLogin(on), cx),
                        )),
                )
            })
        })
        .when(show_updates, |v| {
            v.child(
                div()
                    .mt_6()
                    .pt_4()
                    .border_t(gpui::px(crate::design::BORDER_WIDTH))
                    .border_color(rgb(p.border))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_3()
                    .when(has_update, |v| {
                        v.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(rgb(p.warning))
                                .child(format!(
                                    "可升级至 {} · 将重启设备",
                                    data.latest_version.as_deref().unwrap_or_default()
                                )),
                        )
                    })
                    .child(update_button)
                    .when(has_update, |v| {
                        v.child(
                            ui::button("device-upgrade", "升级并重启", true, can_update)
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    if can_update {
                                        upgrade(v, DeviceAction::Upgrade, cx)
                                    }
                                }))
                                .automation_enabled(
                                    can_update,
                                    AutomationRole::Button,
                                    "升级并重启",
                                ),
                        )
                    }),
            )
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
            account(self.account.clone(), cx, |v, event, cx| {
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
            device(
                self.data.clone(),
                &self.focus,
                self.rename_modal.source("device-rename-dialog"),
                cx,
                |v, event, cx| {
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
                            v.data.online = Some(v.data.running)
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
                },
            )
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

pub mod appearance;

pub mod rename_device;
