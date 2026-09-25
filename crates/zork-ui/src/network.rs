//! Device connection and enrollment surfaces shared with the desktop controllers.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FocusHandle};
use std::rc::Rc;
#[derive(Clone, Default)]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// Registered machine name, when it differs from the display `name`.
    pub machine: Option<String>,
    /// Stable colour key from core (join order or identity).
    pub color: Option<String>,
    /// Whether this client can reach the peer: the same core status shown next
    /// to the device everywhere else. `None` when the peer is not a saved device
    /// of this client (for example another client), so no dot is shown.
    pub status: Option<crate::device_name::DeviceStatus>,
    /// Whether the station owning this list currently has a Mesh link to the
    /// peer. Node-to-node connectivity is worded, never drawn as a status dot.
    pub linked: Option<bool>,
    pub permission: String,
}
#[derive(Clone, Default)]
pub struct NetworkData {
    /// Name of the station whose Mesh membership is listed.
    pub station: String,
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
/// Wording for a station's Mesh link to one member.
pub fn link_text(station: &str, linked: bool) -> String {
    match (station.is_empty(), linked) {
        (true, true) => "Mesh 已互通".into(),
        (true, false) => "Mesh 未互通".into(),
        (false, true) => format!("与 {station} 已互通"),
        (false, false) => format!("与 {station} 未互通"),
    }
}
pub fn network<V: 'static>(
    data: NetworkData,
    focus: &FocusHandle,
    connect: Option<gpui::AnyElement>,
    window: &mut gpui::Window,
    cx: &mut Context<V>,
    action: impl Fn(&mut V, NetworkAction, &mut Context<V>) + 'static,
) -> Div {
    use crate::components::{disclosure, standard_menu::Item};
    let _ = focus;
    let action = Rc::new(action);
    let more_action = action.clone();
    let enabled = data.enabled;
    let mut items = vec![Item::new("mesh-new-peer", "手动连接"), {
        let item = Item::new("node-mesh-toggle", "允许设备连接").check(enabled);
        if data.busy || !data.available {
            item.disabled()
        } else {
            item
        }
    }];
    items.push(if data.identity.is_some() {
        Item::new("copy-node-origin", "复制节点身份")
    } else {
        Item::new("copy-node-origin", "复制节点身份").disabled()
    });
    items.push(if data.busy {
        Item::new("mesh-settings-refresh", "正在刷新…").disabled()
    } else {
        Item::new("mesh-settings-refresh", "刷新")
    });
    let more = disclosure::more_menu(
        "mesh-more",
        items,
        true,
        window,
        cx,
        move |v, key, _, cx| {
            let event = match key.as_str() {
                "mesh-new-peer" => NetworkAction::Add,
                "node-mesh-toggle" => NetworkAction::Toggle(!enabled),
                "copy-node-origin" => NetworkAction::CopyIdentity,
                "mesh-settings-refresh" => NetworkAction::Refresh,
                _ => return,
            };
            more_action(v, event, cx)
        },
    );
    let mut list = div().flex().flex_col();
    for peer in &data.peers {
        let action = action.clone();
        let id = peer.id.clone();
        let busy = data.busy;
        let row_menu = disclosure::more_menu(
            format!("mesh-peer-more-{id}"),
            vec![if busy {
                Item::new("remove", "移除").disabled()
            } else {
                Item::new("remove", "移除")
            }],
            true,
            window,
            cx,
            move |v, key, _, cx| {
                if key == "remove" {
                    action(v, NetworkAction::Remove(id.clone()), cx)
                }
            },
        );
        let group = format!("mesh-peer-row-{}", peer.id);
        list = list.child(
            div()
                .group(group.clone())
                .flex()
                .items_center()
                .gap_2()
                .min_h(px(40.))
                .child(div().min_w_0().text_size(px(14.)).child(match &peer.status {
                    Some(status) => crate::device_name::label(
                        format!("mesh-peer-{}", peer.id),
                        crate::device_name::DeviceName::new(peer.name.clone(), peer.machine.clone())
                            .with_color(peer.color.clone()),
                        status,
                        None,
                    )
                    .into_any_element(),
                    None => {
                        let id: gpui::ElementId = format!("mesh-peer-{}", peer.id).into();
                        let name = crate::device_name::DeviceName::new(
                            peer.name.clone(),
                            peer.machine.clone(),
                        );
                        div()
                            .id(id.clone())
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(crate::device_name::mark_keyed(
                                &peer.name,
                                &peer
                                    .color
                                    .clone()
                                    .unwrap_or_else(|| crate::device_name::color_key(&peer.name)),
                                18.,
                            ))
                            .child(crate::device_name::name_text(&id, &name))
                            .automation(AutomationRole::Status, name.accessible())
                            .into_any_element()
                    }
                }))
                // Membership link of the listed station, in words: it is not
                // this client's reachability, which the dot above shows.
                .when_some(peer.linked, |v, linked| {
                    let text = link_text(&data.station, linked);
                    v.child(
                        disclosure::meta(text.clone())
                            .id(format!("mesh-peer-link-{}", peer.id))
                            .automation(AutomationRole::Status, text),
                    )
                })
                .when(!peer.permission.is_empty(), |v| {
                    v.child(disclosure::info(
                        format!("mesh-peer-permission-{}", peer.id),
                        peer.permission.clone(),
                    ))
                })
                .child(div().flex_1())
                // Rare row actions stay out of sight until the row is hovered.
                .child(
                    div()
                        .opacity(0.)
                        .group_hover(group, |s| s.opacity(1.))
                        .child(row_menu),
                ),
        );
    }
    let count = data.peers.len();
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pb(px(12.))
                .child(ui::page_title("设备"))
                .when(count > 0, |v| {
                    v.child(disclosure::meta(format!("{count} 台")))
                })
                .child(div().flex_1())
                .children(connect)
                .child(more),
        )
        .child(list)
        .when(data.peers.is_empty(), |v| {
            v.child(disclosure::meta("还没有其他设备。").py_2())
        })
        .child(
            div()
                .pt(px(12.))
                .child(disclosure::meta("同一 Google 账号的手机会自动出现在这里")),
        )
        .when(!enabled, |v| {
            v.child(
                div()
                    .pt_2()
                    .child(disclosure::meta("设备连接已关闭，可在“更多”中开启。")),
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
    let p = ZORK_UI.palette;
    let active =
        matches!(data.status.as_str(), "waiting" | "connecting") && !data.command.is_empty();
    let generating = data.busy || (data.status.is_empty() && data.notice.is_none());
    div()
        .flex()
        .flex_col()
        .items_start()
        .gap_3()
        // Opening "连接设备" already asks for a command, so the first state is
        // progress, never a lone button. A button only returns to start over
        // after expiry, revocation, a join or a failure.
        .when(!active && generating, |v| {
            v.child(
                div()
                    .id("mesh-invite-generating")
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(13.))
                    .text_color(rgb(p.muted))
                    .child(
                        crate::components::loading::indicator("mesh-invite-loading", 14.)
                            .without_delay(),
                    )
                    .child("正在生成连接命令…")
                    .automation(AutomationRole::Status, "正在生成连接命令"),
            )
        })
        .when(!active && !generating, |v| {
            v.when(!data.status_label.is_empty(), |v| {
                v.child(
                    div()
                        .id("mesh-invite-status")
                        .text_size(px(13.))
                        .text_color(rgb(p.muted))
                        .child(data.status_label.clone())
                        .automation(AutomationRole::Status, data.status_label.clone()),
                )
            })
            .child(
                ui::button("mesh-invite-create", "重新生成", true, data.available)
                    .on_click(
                        cx.listener(move |v, _, _, cx| create(v, EnrollmentAction::Create, cx)),
                    )
                    .automation_enabled(data.available, AutomationRole::Button, "重新生成连接命令"),
            )
        })
        .when(active, |v| {
            v.child(div().text_size(px(14.)).child("在另一台电脑上运行："))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .p(px(4.))
                        .pl(px(14.))
                        .rounded(px(crate::design::RADIUS.block))
                        .bg(rgb(p.prompt))
                        .child(
                            div()
                                .id("mesh-invite-command")
                                .flex_1()
                                .min_w_0()
                                .overflow_x_scroll()
                                .font_family(crate::assets::CODE_FONT_FAMILY)
                                .text_size(px(12.))
                                .line_height(px(18.))
                                .whitespace_nowrap()
                                .child(data.command.clone())
                                .automation(AutomationRole::Status, data.command.clone()),
                        )
                        .child(
                            ui::quiet_button("mesh-invite-copy", "复制", !data.busy, ui::IconButtonSize::Compact)
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    copy(v, EnrollmentAction::Copy, cx)
                                }))
                                .automation_enabled(!data.busy, AutomationRole::Button, "复制安装链接"),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .id("mesh-invite-status")
                                .text_size(px(12.))
                                .text_color(rgb(p.muted))
                                .child(data.status_label.clone())
                                .automation(AutomationRole::Status, data.status_label.clone()),
                        )
                        .child(div().text_size(px(12.)).text_color(rgb(p.subtle)).child("·"))
                        .child(
                            ui::quiet_button("mesh-invite-revoke", "撤销", !data.busy, ui::IconButtonSize::Small)
                                .px(px(6.))
                                .text_size(px(12.))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    revoke(v, EnrollmentAction::Revoke, cx)
                                }))
                                .automation_enabled(!data.busy, AutomationRole::Button, "撤销链接"),
                        ),
                )
        })
        .when_some(data.notice, |v, text| v.child(ui::feedback(text)))
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
                station: "mini1".into(),
                enabled: true,
                available: true,
                peers: if state == "empty" {
                    vec![]
                } else {
                    vec![
                        Peer {
                            id: "mini2".into(),
                            name: "B".into(),
                            machine: Some("mini2".into()),
                            color: Some("seq:1".into()),
                            status: Some(crate::device_name::DeviceStatus::Direct),
                            linked: Some(true),
                            permission: "设备 · 协作节点".into(),
                        },
                        // Reachable from mini1, but not from this client.
                        Peer {
                            id: "studio".into(),
                            name: "C".into(),
                            machine: Some("zuozijians-Mac-Studio".into()),
                            color: Some("seq:2".into()),
                            status: Some(crate::device_name::DeviceStatus::Offline),
                            linked: Some(true),
                            permission: "设备 · 协作节点".into(),
                        },
                        Peer {
                            id: "pixel".into(),
                            name: "D".into(),
                            machine: Some("Pixel 手机".into()),
                            color: Some("seq:3".into()),
                            status: None,
                            linked: Some(false),
                            permission: "客户端 · 可管理此设备".into(),
                        },
                    ]
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
                    "10 分钟内有效".into()
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
                self.enrollment.status_label = "10 分钟内有效".into();
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
                            v.enrollment_action(EnrollmentAction::Create, cx);
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
                                    machine: None,
                                    color: None,
                                    status: None,
                                    linked: Some(false),
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
    let create = action.clone();
    let invitation = props.invitation.clone();
    let active = matches!(invitation.status.as_str(), "waiting" | "connecting")
        && !invitation.command.is_empty();
    let connect = (!active).then(|| {
        let enabled = !invitation.busy && invitation.available;
        ui::button("mesh-invite-create", "连接设备", true, enabled)
            .on_click(cx.listener(move |v, _, _, cx| {
                create(v, PageAction::Enrollment(EnrollmentAction::Create), cx)
            }))
            .automation_enabled(enabled, AutomationRole::Button, "连接设备")
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .child(network(
            props.data,
            props.focus,
            connect,
            window,
            cx,
            move |v, event, cx| network_action(v, PageAction::Network(event), cx),
        ))
        // The command appears only while an invitation is open.
        .when(active || invitation.notice.is_some(), |v| {
            v.child(
                div()
                    .mt_6()
                    .child(enrollment(invitation, cx, move |v, event, cx| {
                        enrollment_action(v, PageAction::Enrollment(event), cx)
                    })),
            )
        })
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
    let done = Rc::new(close);
    let finish = done.clone();
    let active =
        matches!(data.status.as_str(), "waiting" | "connecting") && !data.command.is_empty();
    let content = div()
        .flex()
        .flex_col()
        .gap_4()
        .child(enrollment(data, cx, event))
        .when(active, |v| {
            v.child(
                div().flex().justify_end().child(
                    ui::button("add-device-done", "完成", false, true)
                        .on_click(cx.listener(move |v, _, _, cx| finish(v, cx)))
                        .automation(AutomationRole::Button, "完成"),
                ),
            )
        });
    ui::detail_modal_sized(
        "add-device-dialog",
        "连接设备",
        content,
        None,
        modal,
        ui::DIALOG_FORM_WIDTH,
        window,
        cx,
        true,
        move |v, _, cx| done(v, cx),
    )
    .into_any_element()
}
