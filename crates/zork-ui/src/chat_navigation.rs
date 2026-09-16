//! Complete device, agent and Chat sidebar using readonly core projections.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FontWeight, Window};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zork_client_types::navigation::{NavigationAgent, NavigationChat};
#[derive(Clone)]
pub enum Destination {
    SharedFiles,
    Leader(String),
    Conversation {
        session: String,
        leader: Option<String>,
    },
    Manage(usize),
}
pub enum Action {
    Navigate {
        node: Option<String>,
        destination: Destination,
    },
    Collapsed(HashSet<String>),
    BeginResize,
}
#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub online: Option<bool>,
    pub direct: bool,
    pub public: bool,
    pub agents: Arc<Vec<NavigationAgent>>,
    pub tasks: Arc<HashMap<String, Vec<NavigationChat>>>,
    pub others: Arc<Vec<NavigationChat>>,
    pub selected_session: Option<String>,
    pub chatting: bool,
}
impl Device {
    fn same(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.online == other.online
            && self.direct == other.direct
            && self.public == other.public
            && self.selected_session == other.selected_session
            && self.chatting == other.chatting
            && Arc::ptr_eq(&self.agents, &other.agents)
            && Arc::ptr_eq(&self.tasks, &other.tasks)
            && Arc::ptr_eq(&self.others, &other.others)
    }
}
pub struct Navigation {
    regions: crate::components::region::Regions<Self>,
    devices: Vec<Device>,
    active: Option<String>,
    collapsed: HashSet<String>,
    show_all: HashSet<String>,
    scroll: gpui::ScrollHandle,
    locale: Text,
    width: f32,
    brand: Option<gpui::Entity<crate::components::brand::Brand>>,
    shared_files: bool,
    tabs: TabGroup,
    details_overlay: Option<gpui::Entity<crate::components::tooltip::DetailsOverlay>>,
}
impl gpui::EventEmitter<Action> for Navigation {}
impl Navigation {
    pub fn new(collapsed: HashSet<String>, locale: Text, cx: &mut Context<Self>) -> Self {
        Self {
            regions: Default::default(),
            devices: vec![],
            active: None,
            collapsed,
            show_all: Default::default(),
            scroll: Default::default(),
            locale,
            width: 240.,
            brand: None,
            shared_files: false,
            tabs: TabGroup::new(cx),
            details_overlay: None,
        }
    }
    pub fn set_data(
        &mut self,
        devices: Vec<Device>,
        active: Option<String>,
        shared_files: bool,
        width: f32,
        cx: &mut Context<Self>,
    ) {
        let mut changed = vec![];
        for device in &devices {
            if self
                .devices
                .iter()
                .find(|old| old.id == device.id)
                .is_none_or(|old| !old.same(device))
            {
                changed.push(format!("device/{}", device.id));
            }
        }
        let all = self.active != active || self.shared_files != shared_files || self.width != width;
        let removed = self.devices.len() != devices.len();
        self.devices = devices;
        self.active = active;
        self.shared_files = shared_files;
        self.width = width;
        if all || removed {
            crate::components::region::invalidate_all(cx);
        } else if !changed.is_empty() {
            crate::components::region::invalidate(
                cx,
                &changed.iter().map(String::as_str).collect::<Vec<_>>(),
            );
        }
    }
    pub fn set_text(&mut self, locale: Text, cx: &mut Context<Self>) {
        self.locale = locale;
        crate::components::region::invalidate_all(cx);
    }
    #[cfg(feature = "headless-bench")]
    pub fn counters(&self, cx: &gpui::App) -> HashMap<String, [usize; 4]> {
        self.regions.counters(cx)
    }
    fn width(&self, available: f32) -> f32 {
        self.width.min((available - 360.).max(200.))
    }
    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        cx.emit(Action::Collapsed(self.collapsed.clone()));
        crate::components::region::invalidate_all(cx);
    }
    fn go(&self, node: Option<String>, destination: Destination, cx: &mut Context<Self>) {
        cx.emit(Action::Navigate { node, destination });
    }
    fn brand(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let brand = self
            .brand
            .get_or_insert_with(|| {
                cx.new(|_| {
                    crate::components::brand::Brand::new(
                        crate::components::brand::BrandMotion::Header,
                        CUE_UI.palette.sidebar,
                    )
                })
            })
            .clone();
        div()
            .id("zork-brand")
            .h(px(48.))
            .pl(px(64.))
            .mb_2()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.start_window_move()
            })
            .child(brand)
            .automation(AutomationRole::Status, "Zork")
            .into_any_element()
    }
    fn device(&self, device: &Device, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let node = &device;
        let expanded = !self.collapsed.contains(&node.id);
        let fold = crate::components::collapse::Collapse::new(
            format!("device-fold-{}", node.id),
            expanded,
            self.width(window.viewport_size().width.as_f32()) - 16.,
            window,
            cx,
        );
        let header_focus = fold.header_focus(cx);
        let interactive = fold.interactive(cx);
        let key = node.id.clone();
        let state = self.locale.text(match device.online {
            Some(true) => "device_online",
            Some(false) => "device_offline",
            None => "device_not_connected",
        });
        let leaders = &device.agents;
        self.tabs
            .column()
            .gap_0()
            .pb(px(2.))
            .child(
                self.tabs
                    .tab(format!("device-{}", node.id), false)
                    .track_focus(&header_focus)
                    .child(ui::icon("icons/node.svg", 20.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_ellipsis()
                            .font_weight(FontWeight::MEDIUM)
                            .child(node.name.clone()),
                    )
                    .child(
                        div()
                            .size(px(5.))
                            .rounded_full()
                            .bg(rgb(match device.online {
                                Some(true) if device.direct => CUE_UI.palette.success,
                                Some(true) => 0xD9A023,
                                _ => CUE_UI.palette.subtle,
                            })),
                    )
                    .when(
                        device.online == Some(true) && device.public == true,
                        |row| {
                            row.child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(CUE_UI.palette.muted))
                                    .child(self.locale.text("device_public_network")),
                            )
                        },
                    )
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle(key.clone(), cx)))
                    .automation(AutomationRole::Button, format!("{} · {state}", node.name)),
            )
            .child({
                let content = fold.mounted(cx).then(|| {
                    self.tabs
                        .column()
                        .when(!leaders.is_empty(), |v| {
                            v.pt(px(crate::navigation::TAB_GAP))
                        })
                        .children(
                            leaders
                                .iter()
                                .map(|a| self.leader(device, a, interactive, cx)),
                        )
                        .child(self.chat_group(device, None, &device.others, interactive, cx))
                        .into_any_element()
                });
                let owner = cx.entity().downgrade();
                let region = format!("device/{}", node.id);
                fold.element(
                    content,
                    move |_, cx| {
                        let _ = owner.update(cx, |_, cx| {
                            crate::components::region::invalidate(cx, &[&region]);
                        });
                    },
                    cx,
                )
            })
    }
    fn leader(
        &self,
        device: &Device,
        agent: &NavigationAgent,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = &agent.id;
        let name = &agent.name;
        let key = format!("{}/{id}", device.id);
        let active = !self.shared_files && self.active.as_deref() == Some(&device.id);
        let chatting = active
            && agent.session_id.is_some()
            && agent.session_id == device.selected_session
            && device.chatting;
        let tasks = device.tasks.get(id).map(Vec::as_slice).unwrap_or_default();
        let leader_id = id.clone();
        let node_id = device.id.clone();
        let can_open = agent.can_open;
        let details = crate::components::tooltip::DetailsTooltip {
            key: format!("leader-{key}"),
            title: name.clone(),
            kind: self
                .locale
                .text(if can_open {
                    "navigation_partner"
                } else {
                    "navigation_creator"
                })
                .into(),
            avatar: agent.avatar.clone(),
            description: agent.instructions.clone(),
            rows: vec![
                (
                    self.locale.text("navigation_device").into(),
                    device.name.clone(),
                ),
                (
                    self.locale.text("navigation_connection").into(),
                    agent.profile_id.clone(),
                ),
                (self.locale.text("model").into(), agent.model.clone()),
            ],
        };
        self.tabs
            .column()
            .child(
                self.tabs
                    .tab(format!("leader-{}-{id}", device.id), chatting)
                    .tab_stop(interactive && can_open)
                    .child(ui::agent_avatar(agent.avatar.as_deref(), 20.))
                    .child(div().flex_1().min_w_0().text_ellipsis().child(name.clone()))
                    .when(agent.unread, |v| {
                        v.child(
                            div()
                                .size(px(6.))
                                .rounded_full()
                                .bg(rgb(CUE_UI.palette.text)),
                        )
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if can_open {
                            v.go(
                                Some(node_id.clone()),
                                Destination::Leader(leader_id.clone()),
                                cx,
                            )
                        }
                    }))
                    .automation_enabled(can_open, AutomationRole::Button, name.clone())
                    .map(|row| {
                        crate::components::tooltip::trigger(
                            row,
                            details,
                            self.details_overlay.as_ref().unwrap().clone(),
                        )
                    }),
            )
            .child(self.chat_group(device, Some(agent), tasks, interactive, cx))
    }

    fn chat_group(
        &self,
        device: &Device,
        creator: Option<&NavigationAgent>,
        chats: &[NavigationChat],
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let key = format!(
            "{}/{}",
            device.id,
            creator.map(|a| a.id.as_str()).unwrap_or("unattributed")
        );
        let all = self.show_all.contains(&key);
        let visible: Vec<_> = chats
            .iter()
            .filter(|chat| {
                all || chat.in_preview || device.selected_session.as_deref() == Some(&chat.chat_id)
            })
            .collect();
        self.tabs
            .column()
            .children(
                visible
                    .iter()
                    .map(|chat| self.chat_row(device, creator, chat, interactive, cx)),
            )
            .when(chats.len() > visible.len() || all, |panel| {
                panel.child(
                    self.tabs
                        .tab(format!("leader-more-{key}"), false)
                        .tab_stop(interactive)
                        .pl(px(36.))
                        .text_color(rgb(CUE_UI.palette.muted))
                        .child(self.locale.text(if all {
                            "device_fewer_tasks"
                        } else {
                            "device_more_tasks"
                        }))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if !v.show_all.remove(&key) {
                                v.show_all.insert(key.clone());
                            }
                            crate::components::region::invalidate_all(cx);
                        })),
                )
            })
    }

    fn chat_row(
        &self,
        device: &Device,
        creator: Option<&NavigationAgent>,
        chat: &NavigationChat,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = !self.shared_files
            && self.active.as_deref() == Some(&device.id)
            && device.selected_session.as_deref() == Some(&chat.chat_id)
            && device.chatting;
        let title = if chat.title.is_empty() {
            self.locale.text("device_untitled_task").to_owned()
        } else {
            chat.title.clone()
        };
        let node = device.id.clone();
        let leader = creator.map(|a| a.id.clone());
        let session = chat.chat_id.clone();
        let mut rows = vec![(self.locale.text("workspace").into(), chat.workspace.clone())];
        if let Some(creator) = creator {
            rows.insert(
                0,
                (
                    self.locale.text("navigation_creator").into(),
                    creator.name.clone(),
                ),
            );
        }
        if let Some(executor) = &chat.executor {
            rows.push((self.locale.text("device_executor").into(), executor.clone()));
        }
        let details = crate::components::tooltip::DetailsTooltip {
            key: format!("task-{}-{}", device.id, chat.chat_id),
            title: title.clone(),
            kind: "Chat".into(),
            avatar: None,
            description: chat.description.clone(),
            rows,
        };
        self.tabs
            .tab(
                format!("leader-task-{}-{}", device.id, chat.chat_id),
                selected,
            )
            .pl(px(36.))
            .tab_stop(interactive)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .child(title.clone()),
            )
            .when(chat.unread, |v| {
                v.child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(rgb(CUE_UI.palette.text)),
                )
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                v.go(
                    Some(node.clone()),
                    Destination::Conversation {
                        leader: leader.clone(),
                        session: session.clone(),
                    },
                    cx,
                );
            }))
            .automation(AutomationRole::Button, title)
            .map(|row| {
                crate::components::tooltip::trigger(
                    row,
                    details,
                    self.details_overlay.as_ref().unwrap().clone(),
                )
            })
    }
}
use crate::navigation::TabGroup;
impl Render for Navigation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self
            .details_overlay
            .get_or_insert_with(|| cx.new(|_| Default::default()))
            .clone();
        let width = self.width(window.viewport_size().width.as_f32()) - 16.;
        let ids = self
            .devices
            .iter()
            .map(|d| d.id.clone())
            .collect::<Vec<_>>();
        let mut names = ids
            .iter()
            .map(|id| format!("device/{id}"))
            .collect::<HashSet<_>>();
        names.insert("footer".into());
        self.regions.retain(|key| names.contains(key));
        let rows = ids
            .into_iter()
            .map(|id| {
                self.regions.auto_height(
                    &format!("device/{id}"),
                    width,
                    cx,
                    move |v, window, cx| {
                        v.devices
                            .iter()
                            .find(|d| d.id == id)
                            .map(|d| v.device(d, window, cx).into_any_element())
                            .unwrap_or_else(|| gpui::Empty.into_any_element())
                    },
                )
            })
            .collect::<Vec<_>>();
        let footer = self.regions.auto_height("footer", width, cx, |v, _, cx| {
            v.render_footer(cx).into_any_element()
        });
        let brand = self.brand(cx);
        let tabs = self.tabs.clone();
        tabs.surface(
            div()
                .id("device-sidebar")
                .relative()
                .w(px(self.width(window.viewport_size().width.as_f32())))
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .px_2()
                .child(
                    div()
                        .absolute()
                        .right_0()
                        .top_0()
                        .h_full()
                        .w(px(5.))
                        .id("device-sidebar-resize")
                        .cursor_col_resize()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|_, _, _, cx| {
                                cx.emit(Action::BeginResize);
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(brand)
                .child(
                    self.tabs
                        .tab("navigation-shared-files".into(), self.shared_files)
                        .child(ui::icon("icons/phosphor-folder-simple.svg", 20.))
                        .child(self.locale.text("shared_files"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(Action::Navigate {
                                node: None,
                                destination: Destination::SharedFiles,
                            })
                        }))
                        .automation(AutomationRole::Button, self.locale.text("shared_files")),
                )
                .child(
                    div()
                        .id("device-sidebar-scroll")
                        .track_scroll(&self.scroll)
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .children(rows),
                )
                .child(footer)
                .child(overlay),
        )
    }
}

impl Navigation {
    fn render_footer(&self, cx: &mut Context<Self>) -> Div {
        self.tabs
            .column()
            .py_2()
            .child(
                self.tabs
                    .tab("device-add".into(), false)
                    .child(ui::icon("icons/plus.svg", 20.))
                    .child(self.locale.text("device_add"))
                    .on_click(cx.listener(|v, _, _, cx| v.go(None, Destination::Manage(3), cx)))
                    .automation(AutomationRole::Button, self.locale.text("device_add")),
            )
            .child(
                self.tabs
                    .tab("desktop-manage".into(), false)
                    .child(ui::icon("icons/settings.svg", 20.))
                    .child(self.locale.text("nav_settings"))
                    .on_click(
                        cx.listener(|v, _, _, cx| {
                            v.go(v.active.clone(), Destination::Manage(4), cx)
                        }),
                    )
                    .automation(AutomationRole::Button, self.locale.text("nav_settings")),
            )
    }
}

#[cfg(feature = "stories")]
pub mod stories;
