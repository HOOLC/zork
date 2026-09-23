//! Complete device and Chat sidebar using readonly core projections.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, FontWeight, Window};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};
use zork_client_types::navigation::NavigationChat;
#[derive(Clone)]
pub enum Destination {
    SharedFiles,
    NewChat,
    Conversation { session: String },
    Manage(usize),
}
pub enum Action {
    Archive {
        node: String,
        chat: String,
        archived: bool,
        expected_message_count: u64,
    },
    Navigate {
        node: Option<String>,
        destination: Destination,
    },
    Collapsed(HashSet<String>),
    BeginResize,
    Preview {
        node: String,
        session: String,
        hovered: bool,
    },
}
#[derive(Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub online: Option<bool>,
    pub status: crate::device_name::DeviceStatus,
    pub direct: bool,
    pub public: bool,
    pub chats: Arc<Vec<NavigationChat>>,
    pub selected_session: Option<String>,
    pub chatting: bool,
}
impl Device {
    fn same(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.status == other.status
            && self.online == other.online
            && self.direct == other.direct
            && self.public == other.public
            && self.selected_session == other.selected_session
            && self.chatting == other.chatting
            && Arc::ptr_eq(&self.chats, &other.chats)
    }
}
pub struct Navigation {
    regions: crate::components::region::Regions<Self>,
    devices: Vec<Device>,
    active: Option<String>,
    collapsed: HashSet<String>,
    show_all: HashSet<String>,
    show_archived: bool,
    hovered_row: Option<String>,
    hovered_action: Option<String>,
    focus_subscriptions: RefCell<HashMap<String, Vec<gpui::Subscription>>>,
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
            show_archived: false,
            hovered_row: None,
            hovered_action: None,
            focus_subscriptions: Default::default(),
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
        if !self.focus_subscriptions.get_mut().is_empty() || self.hovered_row.is_some() {
            let show_archived = self.show_archived;
            let visible = self
                .devices
                .iter()
                .flat_map(|device| {
                    device.chats.iter().filter_map(move |chat| {
                        (chat.archived == show_archived)
                            .then(|| format!("chat-{}-{}", device.id, chat.chat_id))
                    })
                })
                .collect::<HashSet<_>>();
            self.focus_subscriptions
                .get_mut()
                .retain(|id, _| visible.contains(id));
            if self
                .hovered_row
                .as_ref()
                .is_some_and(|id| !visible.contains(id))
            {
                self.hovered_row = None;
                self.hovered_action = None;
            }
        }
        self.active = active;
        self.shared_files = shared_files;
        self.width = width;
        if all || removed || !changed.is_empty() {
            crate::components::region::invalidate(cx, &["chats"]);
        }
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
    pub fn counters(&self, cx: &gpui::App) -> std::collections::HashMap<String, [usize; 4]> {
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
                        ZORK_UI.palette.sidebar,
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
        let state = crate::device_name::status_text(&device.status, Some(&self.locale));
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
                            .font_weight(FontWeight::MEDIUM)
                            .child(crate::device_name::label(
                                "device-header-name",
                                node.name.clone(),
                                &device.status,
                                Some(&self.locale),
                            )),
                    )
                    .on_click(cx.listener(move |v, _, _, cx| v.toggle(key.clone(), cx)))
                    .automation(AutomationRole::Button, format!("{} · {state}", node.name)),
            )
            .child({
                let content = fold.mounted(cx).then(|| {
                    self.tabs
                        .column()
                        .child({
                            let node = device.id.clone();
                            self.tabs
                                .tab(
                                    format!("new-chat-{}", device.id),
                                    self.active.as_deref() == Some(&device.id)
                                        && device.selected_session.is_none()
                                        && !self.shared_files,
                                )
                                .tab_stop(interactive)
                                .pl(px(36.))
                                .child(self.locale.text("new_chat"))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.go(Some(node.clone()), Destination::NewChat, cx)
                                }))
                                .automation(AutomationRole::Button, self.locale.text("new_chat"))
                        })
                        .child(self.chat_group(device, &device.chats, interactive, window, cx))
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
    fn new_chat_target(&self) -> Option<String> {
        if let Some(id) = &self.active {
            if self.devices.iter().any(|device| &device.id == id) {
                return Some(id.clone());
            }
        }
        self.devices
            .iter()
            .find(|device| device.online == Some(true))
            .or_else(|| self.devices.first())
            .map(|device| device.id.clone())
    }
    fn on_new_chat(&self) -> bool {
        !self.shared_files
            && self.devices.iter().any(|device| {
                self.active.as_deref() == Some(device.id.as_str())
                    && device.selected_session.is_none()
            })
    }
    fn chat_list(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut chats: Vec<_> = self
            .devices
            .iter()
            .flat_map(|device| device.chats.iter().map(move |chat| (device, chat)))
            .filter(|(_, chat)| chat.archived == self.show_archived)
            .collect();
        chats.sort_by(|a, b| {
            b.1.updated_at
                .cmp(&a.1.updated_at)
                .then_with(|| a.1.chat_id.cmp(&b.1.chat_id))
                .then_with(|| a.0.id.cmp(&b.0.id))
        });
        let target = self.new_chat_target();
        let mut list = self.tabs.column().child({
            let node = target.clone();
            self.tabs
                .tab("new-chat-entry".into(), self.on_new_chat())
                .child(ui::icon("icons/plus.svg", 16.))
                .child(self.locale.text("new_chat"))
                .on_click(
                    cx.listener(move |v, _, _, cx| v.go(node.clone(), Destination::NewChat, cx)),
                )
                .automation(AutomationRole::Button, self.locale.text("new_chat"))
        });
        list = list.child(
            self.tabs
                .tab("chat-archive-filter".into(), self.show_archived)
                .child(self.locale.text(if self.show_archived {
                    "chat_show_active"
                } else {
                    "chat_show_archived"
                }))
                .on_click(cx.listener(|v, _, _, cx| {
                    v.show_archived = !v.show_archived;
                    v.hovered_row = None;
                    v.hovered_action = None;
                    v.focus_subscriptions.borrow_mut().clear();
                    crate::components::region::invalidate(cx, &["chats"]);
                }))
                .automation(
                    AutomationRole::Button,
                    self.locale.text(if self.show_archived {
                        "chat_show_active"
                    } else {
                        "chat_show_archived"
                    }),
                ),
        );
        if chats.is_empty() && self.show_archived {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .child(self.locale.text("chat_archive_empty")),
            );
        }
        let mut previous = String::new();
        for (device, chat) in chats {
            let label = self.day_label(&chat.updated_at);
            if label != previous {
                previous = label.clone();
                list = list.child(self.day_header(label));
            }
            list = list.child(self.chat_row(device, chat, window, cx));
        }
        list
    }
    fn day_header(&self, label: String) -> impl IntoElement {
        div()
            .id(format!("chat-day-{label}"))
            .px(px(8.))
            .pt(px(12.))
            .pb(px(2.))
            .text_size(px(12.))
            .line_height(px(16.))
            .text_color(rgb(ZORK_UI.palette.muted))
            .child(label.clone())
            .automation(AutomationRole::Status, label)
    }
    fn day_label(&self, updated: &str) -> String {
        let Some(day) = calendar_day(updated) else {
            return self.locale.text("chat_day_earlier").into();
        };
        let today = local_today();
        let age = epoch_days(today.0, today.1, today.2) - epoch_days(day.0, day.1, day.2);
        if age == 0 {
            self.locale.text("chat_day_today").into()
        } else if (1..7).contains(&age) {
            self.locale
                .text(&format!("chat_day_{}", weekday(day.0, day.1, day.2)))
                .into()
        } else {
            format!("{:02}-{:02}", day.1, day.2)
        }
    }
    fn chat_group(
        &self,
        device: &Device,
        chats: &[NavigationChat],
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let key = device.id.clone();
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
                    .map(|chat| self.chat_row(device, chat, window, cx)),
            )
            .when(chats.len() > visible.len() || all, |panel| {
                panel.child(
                    self.tabs
                        .tab(format!("chats-more-{key}"), false)
                        .tab_stop(interactive)
                        .pl(px(36.))
                        .text_color(rgb(ZORK_UI.palette.muted))
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
        chat: &NavigationChat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = !self.shared_files
            && self.active.as_deref() == Some(&device.id)
            && device.selected_session.as_deref() == Some(&chat.chat_id);
        let title = if chat.title.is_empty() {
            self.locale.text("device_untitled_task").to_owned()
        } else {
            chat.title.clone()
        };
        let meta = crate::device_name::accessible_summary(
            &device.name,
            &device.status,
            Some(&self.locale),
        );
        let node = device.id.clone();
        let session = chat.chat_id.clone();
        let hover_node = node.clone();
        let hover_session = session.clone();
        let owner = cx.entity().downgrade();
        let mut rows = vec![(self.locale.text("workspace").into(), chat.workspace.clone())];
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
        let archive_node = device.id.clone();
        let archive_chat = chat.chat_id.clone();
        let archived = !chat.archived;
        let expected_message_count = chat.message_count;
        let label = format!("{title}, {meta}");
        let radius = crate::controls::FIELD_RADIUS;
        let row_id = format!("chat-{}-{}", device.id, chat.chat_id);
        let action_id = format!("archive-{row_id}");
        let row_focus =
            crate::components::widgets::controls::action_focus(row_id.clone(), window, cx);
        let action_focus =
            crate::components::widgets::controls::action_focus(action_id.clone(), window, cx);
        {
            let mut subscriptions = self.focus_subscriptions.borrow_mut();
            subscriptions.entry(row_id.clone()).or_insert_with(|| {
                vec![
                    cx.on_focus(&row_focus, window, |_, _, cx| {
                        crate::components::region::invalidate(cx, &["chats"])
                    }),
                    cx.on_blur(&row_focus, window, |_, _, cx| {
                        crate::components::region::invalidate(cx, &["chats"])
                    }),
                    cx.on_focus(&action_focus, window, |_, _, cx| {
                        crate::components::region::invalidate(cx, &["chats"])
                    }),
                    cx.on_blur(&action_focus, window, |_, _, cx| {
                        crate::components::region::invalidate(cx, &["chats"])
                    }),
                ]
            });
        }
        let keyboard_focus = window.last_input_was_keyboard()
            && (row_focus.is_focused(window) || action_focus.is_focused(window));
        let row_hovered = self.hovered_row.as_deref() == Some(row_id.as_str());
        let action_hovered = self.hovered_action.as_deref() == Some(action_id.as_str());
        let show_icon = row_hovered || keyboard_focus || chat.archive_pending;
        let action_label = self.locale.text(if chat.archived {
            "chat_unarchive"
        } else {
            "chat_archive"
        });
        let icon_path = if chat.archived {
            "icons/archive-restore.svg"
        } else {
            "icons/archive.svg"
        };
        let archive_icon =
            ui::icon(icon_path, 16.).text_color(if action_hovered || keyboard_focus {
                rgb(ZORK_UI.palette.text)
            } else {
                gpui::rgba((ZORK_UI.palette.text << 8) | 0x80)
            });
        let archive_action = ui::action_link(action_id.clone(), "", !chat.archive_pending)
            .track_focus(&action_focus)
            .size(px(20.))
            .min_h(px(20.))
            .justify_center()
            .text_color(if action_hovered || keyboard_focus {
                rgb(ZORK_UI.palette.text)
            } else {
                gpui::rgba((ZORK_UI.palette.text << 8) | 0x80)
            })
            .child(archive_icon.opacity(if show_icon { 1. } else { 0. }))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.stop_propagation();
                cx.emit(Action::Archive {
                    node: archive_node.clone(),
                    chat: archive_chat.clone(),
                    archived,
                    expected_message_count,
                });
            }))
            .automation_enabled(
                !chat.archive_pending,
                AutomationRole::Button,
                action_label.clone(),
            );
        let action_hover_id = action_id.clone();
        let archive_action = div()
            .id(format!("hover-shell-{action_id}"))
            .absolute()
            .right(px(6.))
            .top(px(5.))
            .size(px(20.))
            .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                let next = (*hovered).then(|| action_hover_id.clone());
                if v.hovered_action != next {
                    v.hovered_action = next;
                    crate::components::region::invalidate(cx, &["chats"]);
                }
            }))
            .child(
                crate::components::tooltip::hint(archive_action, action_id, action_label)
                    .above()
                    .min_width(80.)
                    .focus_handle(&action_focus),
            );
        let row = div()
            .id(row_id.clone())
            .track_focus(&row_focus)
            .tab_stop(true)
            .relative()
            .w_full()
            .px(px(8.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(1.))
            .cursor_pointer()
            .rounded(px(radius))
            .when(!selected, |row| {
                row.hover(|row| row.bg(rgb(crate::design::INTERACTION.neutral_hover)))
            })
            .when(selected, |row| row.bg(rgb(ZORK_UI.palette.selected)))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .text_ellipsis()
                    .pr(px(24.))
                    .child(title.clone()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(ui::icon("icons/node.svg", 12.))
                    .child(crate::device_name::label(
                        "chat-device-name",
                        device.name.clone(),
                        &device.status,
                        Some(&self.locale),
                    )),
            )
            .child(archive_action)
            .when_some(chat.archive_error.clone(), |row, error| {
                row.child(div().pr(px(24.)).text_size(px(11.)).child(error))
            })
            .when(chat.unread, |row| {
                row.child(
                    div()
                        .id(format!("chat-unread-{}-{}", device.id, chat.chat_id))
                        .opacity(if show_icon { 0. } else { 1. })
                        .absolute()
                        .right(px(8.))
                        .top(px(10.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(rgb(ZORK_UI.palette.text)),
                )
            })
            .on_click(cx.listener(move |v, _, _, cx| {
                v.go(
                    Some(node.clone()),
                    Destination::Conversation {
                        session: session.clone(),
                    },
                    cx,
                );
            }))
            .automation(AutomationRole::Button, label)
            .map(|row| {
                crate::components::tooltip::trigger_with_hover(
                    row,
                    details,
                    self.details_overlay.as_ref().unwrap().clone(),
                    move |hovered, cx| {
                        let _ = owner.update(cx, |_, cx| {
                            cx.emit(Action::Preview {
                                node: hover_node.clone(),
                                session: hover_session.clone(),
                                hovered,
                            });
                        });
                    },
                )
            });
        let row_hover_id = row_id.clone();
        div()
            .id(format!("hover-shell-{row_id}"))
            .w_full()
            .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                let next = (*hovered).then(|| row_hover_id.clone());
                if v.hovered_row != next {
                    v.hovered_row = next;
                    if !*hovered {
                        v.hovered_action = None;
                    }
                    crate::components::region::invalidate(cx, &["chats"]);
                }
            }))
            .child(row)
    }
}
fn calendar_day(value: &str) -> Option<(i32, u32, u32)> {
    let bytes = value.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = std::str::from_utf8(&bytes[0..4]).ok()?.parse().ok()?;
    let month: u32 = std::str::from_utf8(&bytes[5..7]).ok()?.parse().ok()?;
    let day: u32 = std::str::from_utf8(&bytes[8..10]).ok()?.parse().ok()?;
    (1..=12).contains(&month).then_some((year, month, day))
}
fn epoch_days(year: i32, month: u32, day: u32) -> i64 {
    let year = year as i64 - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = (year - era * 400) as u64;
    let month_prime = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_prime as u64 + 2) / 5 + day as u64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146097 + day_of_era as i64 - 719468
}
fn weekday(year: i32, month: u32, day: u32) -> usize {
    let offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let year = if month < 3 { year - 1 } else { year };
    (year + year / 4 - year / 100 + year / 400 + offsets[month as usize - 1] + day as i32)
        .rem_euclid(7) as usize
}
pub(super) fn iso_days_ago(days: i64) -> String {
    let today = local_today();
    let (year, month, day) = ymd_from_epoch(epoch_days(today.0, today.1, today.2) - days);
    format!("{year:04}-{month:02}-{day:02}T12:00:00")
}
fn ymd_from_epoch(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let day_of_era = z.rem_euclid(146097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year as i32, month as u32, day as u32)
}
fn local_today() -> (i32, u32, u32) {
    {
        #[repr(C)]
        struct Tm {
            tm_sec: i32,
            tm_min: i32,
            tm_hour: i32,
            tm_mday: i32,
            tm_mon: i32,
            tm_year: i32,
        }
        unsafe extern "C" {
            fn time(tloc: *mut i64) -> i64;
            fn localtime(timer: *const i64) -> *const Tm;
        }
        unsafe {
            let now = time(std::ptr::null_mut());
            let tm = localtime(&now);
            if !tm.is_null() {
                return (
                    (*tm).tm_year + 1900,
                    (*tm).tm_mon as u32 + 1,
                    (*tm).tm_mday as u32,
                );
            }
        }
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    ymd_from_epoch(seconds.div_euclid(86_400))
}
use crate::navigation::TabGroup;
impl Render for Navigation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let overlay = self
            .details_overlay
            .get_or_insert_with(|| cx.new(|_| Default::default()))
            .clone();
        let width = self.width(window.viewport_size().width.as_f32()) - 16.;
        let mut names = HashSet::<String>::new();
        names.insert("chats".into());
        names.insert("footer".into());
        self.regions.retain(|key| names.contains(key));
        let rows = vec![self
            .regions
            .auto_height("chats", width, cx, |v, window, cx| {
                v.chat_list(window, cx).into_any_element()
            })];
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
        let add_device = self
            .tabs
            .tab("device-add".into(), false)
            .child(ui::icon("icons/plus.svg", 20.))
            .child(self.locale.text("device_add"))
            .on_click(cx.listener(|v, _, _, cx| v.go(None, Destination::Manage(3), cx)));
        let add_device = add_device
            .automation(AutomationRole::Button, self.locale.text("device_add"))
            .into_any_element();
        self.tabs.column().py_2().child(add_device).child(
            self.tabs
                .tab("desktop-manage".into(), false)
                .child(ui::icon("icons/settings.svg", 20.))
                .child(self.locale.text("nav_settings"))
                .on_click(
                    cx.listener(|v, _, _, cx| v.go(v.active.clone(), Destination::Manage(4), cx)),
                )
                .automation(AutomationRole::Button, self.locale.text("nav_settings")),
        )
    }
}

#[cfg(feature = "stories")]
pub mod stories;
