//! Local device and saved-device directory; the host owns node operations.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::ComposerInput,
    controls as ui,
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub remote: bool,
}
#[derive(Clone)]
pub struct Data {
    pub nodes: Vec<Node>,
    pub running: bool,
    pub enabled: bool,
    pub busy: bool,
    pub background: bool,
    pub start_at_login: bool,
    pub pairing: bool,
    pub mesh_identity: Option<String>,
    pub remote_name: Entity<ComposerInput>,
    pub remote_origin: Entity<ComposerInput>,
    pub remote_addr: Entity<ComposerInput>,
}
pub enum Action {
    Start,
    Stop,
    Background { enabled: bool, at_login: bool },
    Pair(Option<String>),
    CancelPair,
    Connect,
    Open(String),
    CopyIdentity,
}
pub trait Host: Sized + 'static {
    fn nodes_data(&self) -> Data;
    fn node_action(&mut self, action: Action, cx: &mut Context<Self>);
    fn render_nodes(&self, cx: &mut Context<Self>) -> Div {
        let data = self.nodes_data();
        let p = ZORK_UI.palette;
        let running = data.running;
        let enabled = data.enabled;
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(ui::heading(
                "设备",
                if data.nodes.is_empty() {
                    "开启本机设备，或连接一台已有设备。"
                } else {
                    "选择运行小伙伴的设备。"
                },
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .py_4()
                    .child(
                        div()
                            .size(px(44.))
                            .rounded(px(12.))
                            .bg(rgb(p.selected))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(ui::icon("icons/node.svg", 22.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_weight(FontWeight::MEDIUM).child("本机设备"))
                            .child(div().text_size(px(12.)).text_color(rgb(p.muted)).child(
                                if data.busy {
                                    "处理中…"
                                } else if running {
                                    "运行中"
                                } else if enabled {
                                    "已开启，设备未运行"
                                } else {
                                    "已关闭"
                                },
                            )),
                    )
                    .child(
                        ui::button(
                            "local-node-toggle",
                            if enabled {
                                "关闭设备"
                            } else {
                                "开启本机设备"
                            },
                            !enabled,
                            !data.busy,
                        )
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if enabled {
                                v.node_action(Action::Stop, cx)
                            } else {
                                v.node_action(Action::Start, cx)
                            }
                        }))
                        .automation_enabled(
                            !data.busy,
                            AutomationRole::Button,
                            if enabled {
                                "关闭设备"
                            } else {
                                "开启本机设备"
                            },
                        ),
                    ),
            )
            .when(enabled && !running && !data.busy, |v| {
                v.child(
                    ui::button("local-node-retry", "重试启动", false, true)
                        .on_click(cx.listener(|v, _, _, cx| v.node_action(Action::Start, cx)))
                        .automation(AutomationRole::Button, "重试启动本机设备"),
                )
            })
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .pb_4()
                    .child(if data.background{"Station 由后台服务管理，退出客户端后继续运行。"}else{"由客户端启动的 Station 随客户端退出。连接已有的独立 Station 不改变它的运行方式。"}),
            )
            .when(enabled||running,|v|{
                let background=data.background;let at_login=data.start_at_login;
                v.child(ui::section()
                    .child(div().flex().items_center().justify_between().gap_4()
                        .child(div().flex_1().child("退出客户端后保持 Station 运行"))
                        .child(ui::button("local-node-background",if background{"已开启"}else{"开启"},false,!data.busy)
                            .on_click(cx.listener(move|v,_,_,cx|v.node_action(Action::Background { enabled: !background, at_login }, cx)))
                            .automation_enabled(!data.busy,AutomationRole::Button,if background{"关闭后台运行"}else{"开启后台运行"})))
                    .when(background,|v|v.child(div().flex().items_center().justify_between().gap_4().pt_3()
                        .child("登录系统后自动启动")
                        .child(ui::button("local-node-login",if at_login{"已开启"}else{"开启"},false,!data.busy)
                            .on_click(cx.listener(move|v,_,_,cx|v.node_action(Action::Background { enabled: true, at_login: !at_login }, cx)))
                            .automation_enabled(!data.busy,AutomationRole::Button,"登录系统后自动启动"))))
                    .child(div().pt_3().text_size(px(12.)).text_color(rgb(p.muted)).child("切换后台运行不会重启任务。关闭设备会停止此设备的 Station，其他设备将暂时无法访问它。")))
            })
            .child(
                ui::section()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(ui::label("其他设备"))
                            .child(
                                ui::button("connect-existing-node", "连接设备", false, !data.busy)
                                    .on_click(cx.listener(|v, _, _, cx| v.node_action(Action::Pair(None), cx)))
                                    .automation_enabled(
                                        !data.busy,
                                        AutomationRole::Button,
                                        "连接已有设备",
                                    ),
                            ),
                    )
                    .children(
                        data.nodes
                            .iter()
                            .filter(|n| n.remote)
                            .cloned()
                            .map(|node| {
                                let open = node.clone();
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .py_2()
                                    .child(ui::icon("icons/node.svg", 18.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .child(node.name.clone()),
                                    )
                                    .child(
                                        ui::button(
                                            format!("connect-node-{}", node.id),
                                            "连接",
                                            false,
                                            !data.busy,
                                        )
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            v.node_action(Action::Pair(Some(open.id.clone())), cx)
                                        }))
                                        .automation_enabled(
                                            !data.busy,
                                            AutomationRole::Button,
                                            "连接设备",
                                        ),
                                    )
                            }),
                    )
                    .when(
                        data.nodes.iter().all(|n| !n.remote) && !data.pairing,
                        |v| {
                            v.child(
                                div()
                                    .py_3()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child("还没有连接其他设备。"),
                            )
                        },
                    ),
            )
            .when(data.pairing, |v| {
                v.child(
                    ui::section()
                        .child(ui::label("连接已有设备"))
                        .when_some(data.mesh_identity.clone(), |v, _identity| {
                            v.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_size(px(12.))
                                            .line_height(px(20.))
                                            .text_color(rgb(p.muted))
                                            .child("在目标设备添加此设备，并开启客户端权限。"),
                                    )
                                    .child(
                                        ui::button(
                                            "copy-mesh-identity",
                                            "复制我的身份",
                                            false,
                                            true,
                                        )
                                        .on_click(cx.listener(|v, _, _, cx| v.node_action(Action::CopyIdentity, cx)))
                                        .automation(AutomationRole::Button, "复制设备身份"),
                                    ),
                            )
                        })
                        .child(ui::field("remote-name", "设备名称", &data.remote_name, cx))
                        .child(ui::field(
                            "remote-origin",
                            "目标设备身份",
                            &data.remote_origin, cx))
                        .child(ui::field(
                            "remote-addr",
                            "局域网地址 · 可选",
                            &data.remote_addr, cx))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    ui::button("cancel-pairing", "取消", false, !data.busy)
                                        .on_click(cx.listener(|v, _, _, cx| {
                                            v.node_action(Action::CancelPair, cx);
                                        }))
                                        .automation(AutomationRole::Button, "取消连接"),
                                )
                                .child(
                                    ui::button(
                                        "connect-remote",
                                        "保存并连接",
                                        true,
                                        !data.busy && data.mesh_identity.is_some(),
                                    )
                                    .on_click(cx.listener(|v, _, _, cx| v.node_action(Action::Connect, cx)))
                                    .automation_enabled(
                                        !data.busy && data.mesh_identity.is_some(),
                                        AutomationRole::Button,
                                        "保存并连接",
                                    ),
                                ),
                        ),
                )
            })
            .when(!running && !data.nodes.is_empty(), |v| {
                v.child(ui::section().child(ui::label("本地记录")).children(
                    data.nodes.clone().into_iter().map(|node| {
                        ui::button(
                            format!("browse-node-{}", node.id),
                            format!("浏览 {}", node.name),
                            false,
                            true,
                        )
                        .on_click(cx.listener(move |v, _, _, cx| v.node_action(Action::Open(node.id.clone()), cx)))
                        .automation(AutomationRole::Button, "浏览本地记录")
                    }),
                ))
            })
    }
}

#[cfg(feature = "stories")]
pub struct Story {
    data: Data,
}
#[cfg(feature = "stories")]
impl Story {
    pub fn new(state: &str, cx: &mut Context<Self>) -> Self {
        Self {
            data: Data {
                nodes: if state == "empty" {
                    vec![]
                } else {
                    vec![Node {
                        id: "demo".into(),
                        name: "演示设备".into(),
                        remote: true,
                    }]
                },
                running: state == "running",
                enabled: state != "empty",
                busy: state == "loading",
                background: state == "background",
                start_at_login: false,
                pairing: state == "pairing",
                mesh_identity: Some("key:demo-device-identity".into()),
                remote_name: cx.new(|cx| ComposerInput::new("设备名称", cx)),
                remote_origin: cx.new(|cx| ComposerInput::new("目标设备身份", cx)),
                remote_addr: cx.new(|cx| ComposerInput::new("局域网地址", cx)),
            },
        }
    }
}
#[cfg(feature = "stories")]
impl Host for Story {
    fn nodes_data(&self) -> Data {
        self.data.clone()
    }
    fn node_action(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::Start => {
                self.data.enabled = true;
                self.data.running = true;
            }
            Action::Stop => {
                self.data.enabled = false;
                self.data.running = false;
            }
            Action::Background { enabled, at_login } => {
                self.data.background = enabled;
                self.data.start_at_login = at_login;
            }
            Action::Pair(_) => self.data.pairing = true,
            Action::CancelPair | Action::Connect => self.data.pairing = false,
            Action::Open(_) => {}
            Action::CopyIdentity => {
                if let Some(identity) = &self.data.mesh_identity {
                    cx.write_to_clipboard(ClipboardItem::new_string(identity.clone()));
                }
            }
        }
        cx.notify();
    }
}
#[cfg(feature = "stories")]
impl Render for Story {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_nodes(cx)
    }
}
