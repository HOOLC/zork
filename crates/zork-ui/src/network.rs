//! Device connection and enrollment surfaces shared with the desktop controllers.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
    settings::{row, titled_row},
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FocusHandle, FontWeight};
use std::rc::Rc;
#[derive(Clone, Default)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub status: crate::device_name::DeviceStatus,
    pub permission: String,
}
#[derive(Clone, Default)]
pub struct NetworkData {
    pub enabled: bool,
    pub available: bool,
    pub peers: Vec<Peer>,
    pub identity: Option<String>,
    pub busy: bool,
    pub notice: Option<String>,
}
#[derive(Clone, Debug)]
pub enum NetworkAction {
    Toggle(bool),
    Refresh,
    CopyIdentity,
    Add,
    Remove(String),
}
pub fn network<V: 'static>(
    data: NetworkData,
    focus: &FocusHandle,
    cx: &Context<V>,
    action: impl Fn(&mut V, NetworkAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let add = action.clone();
    let toggle = action.clone();
    let refresh = action.clone();
    let copy = action.clone();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .pb(px(18.))
                .child(ui::page_title("设备连接"))
                .child(
                    ui::page_action("mesh-new-peer", "手动连接")
                        .on_click(cx.listener(move |v, _, _, cx| add(v, NetworkAction::Add, cx)))
                        .automation(AutomationRole::Button, "手动连接设备"),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child("管理已配对设备及其访问权限。"),
        )
        .child(row(
            "允许设备连接",
            if data.enabled {
                "已启用"
            } else {
                "已关闭"
            },
            ui::switch(
                "node-mesh-toggle",
                "允许设备连接",
                data.enabled,
                !data.busy && data.available,
                focus,
                cx,
                move |v, on, cx| toggle(v, NetworkAction::Toggle(on), cx),
            ),
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .py_4()
                .child(
                    ui::button(
                        "copy-node-origin",
                        "复制节点身份",
                        false,
                        data.identity.is_some(),
                    )
                    .on_click(
                        cx.listener(move |v, _, _, cx| copy(v, NetworkAction::CopyIdentity, cx)),
                    )
                    .automation_enabled(
                        data.identity.is_some(),
                        AutomationRole::Button,
                        "复制节点身份",
                    ),
                )
                .child(
                    ui::button(
                        "mesh-settings-refresh",
                        if data.busy {
                            "正在刷新…"
                        } else {
                            "刷新"
                        },
                        false,
                        !data.busy,
                    )
                    .on_click(
                        cx.listener(move |v, _, _, cx| refresh(v, NetworkAction::Refresh, cx)),
                    )
                    .automation_enabled(
                        !data.busy,
                        AutomationRole::Button,
                        "刷新连接信息",
                    ),
                ),
        )
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .pb_3()
                .child("已配对设备"),
        )
        .children(data.peers.iter().map(|peer| {
            let action = action.clone();
            let id = peer.id.clone();
            let (size, line_height, weight) = crate::design::TextRole::SectionTitle.metrics();
            titled_row(
                div()
                    .text_size(px(size))
                    .line_height(px(line_height))
                    .font_weight(FontWeight(weight as f32))
                    .text_color(rgb(ZORK_UI.palette.text))
                    .child(crate::device_name::label(
                        format!("mesh-peer-{id}"),
                        peer.name.clone(),
                        &peer.status,
                        None,
                    )),
                peer.permission.clone(),
                ui::button(format!("revoke-peer-{id}"), "移除", false, !data.busy)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        action(v, NetworkAction::Remove(id.clone()), cx)
                    }))
                    .automation_enabled(!data.busy, AutomationRole::Button, "撤销配对"),
            )
        }))
        .when(data.peers.is_empty(), |v| {
            v.child(
                div()
                    .py_5()
                    .text_size(px(12.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child("还没有配对设备。通过加入命令或手动连接连接设备。"),
            )
        })
        .when_some(data.notice, |v, text| {
            v.child(div().mt_4().child(ui::feedback(text)))
        })
}
#[derive(Clone, Default)]
pub struct EnrollmentData {
    pub available: bool,
    pub busy: bool,
    pub status: String,
    pub command: String,
    pub status_label: String,
    pub notice: Option<String>,
}
#[derive(Clone, Debug)]
pub enum EnrollmentAction {
    Create,
    Copy,
    Revoke,
}
pub fn enrollment<V: 'static>(
    data: EnrollmentData,
    cx: &Context<V>,
    action: impl Fn(&mut V, EnrollmentAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let create = action.clone();
    let copy = action.clone();
    let revoke = action.clone();
    let active =
        matches!(data.status.as_str(), "waiting" | "connecting") && !data.command.is_empty();
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_size(px(12.))
                .child("手机登录同一个 Google 账号后会自动连接。"),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child("连接其它设备：生成安装链接，并在目标设备打开链接完成安装。"),
        )
        .when(!data.status_label.is_empty(), |v| {
            v.child(
                div()
                    .id("mesh-invite-status")
                    .text_size(px(12.))
                    .child(data.status_label.clone())
                    .automation(AutomationRole::Status, data.status_label.clone()),
            )
        })
        .when(!active, |v| {
            v.child(
                ui::button(
                    "mesh-invite-create",
                    if data.busy {
                        "正在生成…"
                    } else {
                        "生成安装链接"
                    },
                    true,
                    !data.busy && data.available,
                )
                .on_click(cx.listener(move |v, _, _, cx| create(v, EnrollmentAction::Create, cx)))
                .automation_enabled(
                    !data.busy && data.available,
                    AutomationRole::Button,
                    "生成安装链接",
                ),
            )
        })
        .when(active, |v| {
            v.child(command_block("mesh-invite-command", data.command.clone()))
                .child(
                    ui::button("mesh-invite-copy", "复制安装链接", true, !data.busy).on_click(
                        cx.listener(move |v, _, _, cx| copy(v, EnrollmentAction::Copy, cx)),
                    ),
                )
                .child(
                    ui::button("mesh-invite-revoke", "撤销链接", false, !data.busy).on_click(
                        cx.listener(move |v, _, _, cx| revoke(v, EnrollmentAction::Revoke, cx)),
                    ),
                )
        })
        .when_some(data.notice, |v, text| v.child(ui::feedback(text)))
}
pub fn command_block(id: impl Into<gpui::ElementId>, command: String) -> impl IntoElement {
    div()
        .id(id)
        .w_full()
        .min_w_0()
        .min_h(px(84.))
        .p_3()
        .bg(rgb(ZORK_UI.palette.sidebar))
        .border(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(ZORK_UI.palette.border))
        .rounded(px(ui::FIELD_RADIUS))
        .overflow_x_scroll()
        .font_family("Menlo")
        .text_size(px(12.))
        .line_height(px(18.))
        .child(command.clone())
        .automation(AutomationRole::Status, command)
}

#[cfg(feature = "stories")]
pub struct NetworkStory {
    validate_peer: fn(&str, &str, &str) -> Result<zork_client_types::device::PeerInput, String>,
    family: String,
    data: NetworkData,
    enrollment: EnrollmentData,
    name: gpui::Entity<crate::components::text_input::ComposerInput>,
    origin: gpui::Entity<crate::components::text_input::ComposerInput>,
    addr: gpui::Entity<crate::components::text_input::ComposerInput>,
    grant: bool,
    focus: [FocusHandle; 2],
    modal: crate::modal::ModalState,
    open: bool,
}
#[cfg(feature = "stories")]
impl NetworkStory {
    pub fn new(
        family: String,
        state: String,
        validate_peer: fn(&str, &str, &str) -> Result<zork_client_types::device::PeerInput, String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let f = crate::stories::page_fixture();
        let mut field = |placeholder| {
            let input =
                cx.new(|cx| crate::components::text_input::ComposerInput::new(placeholder, cx));
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        let name = field("设备名称");
        let origin = field("key: 设备身份");
        let addr = field("局域网地址（可选）");
        let active = state == "command";
        let status = if active {
            "waiting"
        } else if state == "expired" {
            "expired"
        } else {
            ""
        };
        Self {
            validate_peer,
            open: family == "enrollment" || state == "manual",
            family,
            data: NetworkData {
                enabled: true,
                available: true,
                peers: if state == "empty" {
                    vec![]
                } else {
                    vec![Peer {
                        id: "mini2".into(),
                        name: "mini2".into(),
                        status: crate::device_name::DeviceStatus::Connected,
                        permission: "客户端 · 可管理此节点".into(),
                    }]
                },
                identity: Some("device-demo-01".into()),
                busy: false,
                notice: None,
            },
            enrollment: EnrollmentData {
                available: true,
                busy: state == "loading",
                status: status.into(),
                command: if active {
                    f["mesh"]["invitation"].as_str().unwrap().into()
                } else {
                    String::new()
                },
                status_label: if active {
                    "等待目标设备执行 · 10 分 0 秒后过期".into()
                } else if state == "expired" {
                    "加入命令已过期，请重新生成。".into()
                } else {
                    String::new()
                },
                notice: (state == "error").then(|| "获取加入命令失败，请重试。".into()),
            },
            name,
            origin,
            addr,
            grant: false,
            focus: [cx.focus_handle(), cx.focus_handle()],
            modal: crate::modal::ModalState::new(cx),
        }
    }
    fn enrollment_action(&mut self, action: EnrollmentAction, cx: &mut Context<Self>) {
        match action {
            EnrollmentAction::Create => {
                self.enrollment.busy = false;
                self.enrollment.status = "waiting".into();
                self.enrollment.status_label = "等待目标设备执行 · 10 分 0 秒后过期".into();
                self.enrollment.command = crate::stories::page_fixture()["mesh"]["invitation"]
                    .as_str()
                    .unwrap()
                    .into();
                self.enrollment.notice = None;
            }
            EnrollmentAction::Revoke => {
                self.enrollment.status = "revoked".into();
                self.enrollment.status_label = "加入命令已撤销。".into();
                self.enrollment.command.clear();
            }
            EnrollmentAction::Copy => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                    self.enrollment.command.clone(),
                ));
                self.enrollment.notice = Some("加入命令已复制。".into());
            }
        }
        cx.notify();
    }
}
#[cfg(feature = "stories")]
impl gpui::Render for NetworkStory {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enrollment_only = self.family == "enrollment";
        self.modal.sync(
            self.open.then_some(if enrollment_only {
                "add-device-dialog"
            } else {
                "mesh-peer-dialog"
            }),
            window,
            cx,
        );
        let visible = self
            .modal
            .retain(
                if enrollment_only {
                    "add-device-dialog"
                } else {
                    "mesh-peer-dialog"
                },
                self.open.then_some(()),
                cx,
            )
            .is_some();
        if enrollment_only {
            return div()
                .child(
                    ui::page_action("design-pc-add-device", "连接设备").on_click(cx.listener(
                        |v, _, _, cx| {
                            v.open = true;
                            cx.notify();
                        },
                    )),
                )
                .when(visible, |v| {
                    v.child(enrollment_dialog(
                        self.enrollment.clone(),
                        &self.modal,
                        window,
                        cx,
                        |v, event, cx| v.enrollment_action(event, cx),
                        |v, cx| {
                            v.open = false;
                            cx.notify();
                        },
                    ))
                })
                .into_any_element();
        }
        page(
            Page {
                data: self.data.clone(),
                invitation: self.enrollment.clone(),
                focus: &self.focus[0],
                modal: &self.modal,
                peer: visible.then(|| peer::Fields {
                    name: &self.name,
                    origin: &self.origin,
                    address: &self.addr,
                    grant: self.grant,
                    focus: &self.focus[1],
                    busy: self.data.busy,
                    notice: self.data.notice.clone(),
                }),
            },
            window,
            cx,
            |v, event, cx| {
                match event {
                    PageAction::Enrollment(event) => v.enrollment_action(event, cx),
                    PageAction::Network(event) => match event {
                        NetworkAction::Toggle(on) => v.data.enabled = on,
                        NetworkAction::Refresh => v.data.notice = Some("连接信息已刷新。".into()),
                        NetworkAction::CopyIdentity => {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                v.data.identity.clone().unwrap_or_default(),
                            ));
                            v.data.notice = Some("设备身份已复制。".into());
                        }
                        NetworkAction::Add => {
                            v.open = true;
                            v.data.notice = None;
                        }
                        NetworkAction::Remove(id) => v.data.peers.retain(|p| p.id != id),
                    },
                    PageAction::Peer(event) => match event {
                        peer::Action::Grant(on) => v.grant = on,
                        peer::Action::Cancel => {
                            v.open = false;
                            v.data.notice = None;
                        }
                        peer::Action::Save => match (v.validate_peer)(
                            v.name.read(cx).value(),
                            v.origin.read(cx).value(),
                            v.addr.read(cx).value(),
                        ) {
                            Ok(input) => {
                                v.data.peers.push(Peer {
                                    id: input.origin,
                                    name: input.name,
                                    status: crate::device_name::DeviceStatus::Connected,
                                    permission: if v.grant {
                                        "客户端 · 可管理此设备"
                                    } else {
                                        "设备 · 通过 Mesh 授权协作"
                                    }
                                    .into(),
                                });
                                v.open = false;
                                v.data.notice = None;
                            }
                            Err(error) => v.data.notice = Some(error),
                        },
                    },
                }
                cx.notify();
            },
        )
    }
}

pub mod peer;

pub struct Page<'a> {
    pub data: NetworkData,
    pub invitation: EnrollmentData,
    pub focus: &'a FocusHandle,
    pub peer: Option<peer::Fields<'a>>,
    pub modal: &'a crate::modal::ModalState,
}
pub enum PageAction {
    Network(NetworkAction),
    Enrollment(EnrollmentAction),
    Peer(peer::Action),
}
pub fn page<V: 'static>(
    props: Page<'_>,
    window: &mut gpui::Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, PageAction, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let action = Rc::new(action);
    let network_action = action.clone();
    let enrollment_action = action.clone();
    // The last peer row already ends with a divider.
    let divided = props.data.peers.is_empty() || props.data.notice.is_some();
    div()
        .flex()
        .flex_col()
        .child(network(props.data, props.focus, cx, move |v, event, cx| {
            network_action(v, PageAction::Network(event), cx)
        }))
        .child(
            div()
                .mt_6()
                .when(divided, |v| {
                    v.pt_5()
                })
                .child(enrollment(props.invitation, cx, move |v, event, cx| {
                    enrollment_action(v, PageAction::Enrollment(event), cx)
                })),
        )
        .when_some(props.peer, |v, fields| {
            v.child(peer::render(
                fields,
                props.modal,
                window,
                cx,
                move |v, event, cx| action(v, PageAction::Peer(event), cx),
            ))
        })
        .into_any_element()
}
pub fn enrollment_dialog<V: 'static>(
    data: EnrollmentData,
    modal: &crate::modal::ModalState,
    window: &mut gpui::Window,
    cx: &mut Context<V>,
    event: impl Fn(&mut V, EnrollmentAction, &mut Context<V>) + 'static,
    close: impl Fn(&mut V, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let content = enrollment(data, cx, event);
    ui::detail_modal(
        "add-device-dialog",
        "连接设备",
        content,
        None,
        modal,
        window,
        cx,
        true,
        move |v, _, cx| close(v, cx),
    )
    .into_any_element()
}
