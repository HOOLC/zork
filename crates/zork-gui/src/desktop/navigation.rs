//! Device → Chat navigation, shared by every retained device view.
use super::store::{ClientStore, SavedNode};
use crate::{i18n::Locale, shell::ShellRoute};
use gpui::{prelude::*, Context, Window};
use std::{collections::HashSet, sync::Arc};
use zork_client_core::preferences::{read_view_state, save_view_state, ViewState};

#[derive(Clone)]
pub enum Destination {
    Home,
    Conversation {
        session: String,
        leader: Option<String>,
    },
    Leader(String),
    Manage(usize),
}
#[derive(Clone)]
pub struct Navigate {
    pub node: Option<String>,
    pub destination: Destination,
}
#[derive(Clone)]
pub struct Preview {
    pub node: String,
    pub session: String,
    pub hovered: bool,
}
#[derive(Clone, Default, PartialEq)]
pub struct Selection {
    pub selected_leader: Option<String>,
    pub selected_session: Option<String>,
    pub route: Option<ShellRoute>,
    pub reading_tail: bool,
}
struct Device {
    node: SavedNode,
    data: Arc<zork_client_core::state::NavigationData>,
    selection: Selection,
    core: Option<Arc<zork_client_core::state::Device>>,
    subscription: Option<gpui::Task<()>>,
    notification_subscription: Option<gpui::Task<()>>,
}
pub struct DeviceNavigation {
    store: Arc<ClientStore>,
    view: Option<gpui::Entity<zork_ui::chat_navigation::Navigation>>,
    view_locale: Option<Locale>,
    devices: Vec<Device>,
    active: Option<String>,
    locale: Locale,
    width: f32,
    pub resizing: bool,
    viewing: bool,
}
/// A device's Chat projection changed; hosts showing archived Chats refresh.
pub struct Changed;
impl gpui::EventEmitter<Navigate> for DeviceNavigation {}
impl gpui::EventEmitter<Changed> for DeviceNavigation {}
impl gpui::EventEmitter<Preview> for DeviceNavigation {}
impl DeviceNavigation {
    #[cfg(feature = "headless-bench")]
    pub(crate) fn benchmark_region_counts(
        &self,
        cx: &gpui::App,
    ) -> std::collections::HashMap<String, [usize; 4]> {
        self.view
            .as_ref()
            .map(|view| view.read(cx).counters(cx))
            .unwrap_or_default()
    }
    pub fn new(store: Arc<ClientStore>, nodes: &[SavedNode], _cx: &mut gpui::App) -> Self {
        let width = read_view_state::<f32>(&store, "device", ViewState::SidebarWidth)
            .ok()
            .flatten()
            .unwrap_or(crate::design::DEVICE_SIDEBAR_WIDTH)
            .clamp(200., 420.);
        let mut view = Self {
            store,
            view: None,
            view_locale: None,
            width,
            resizing: false,
            viewing: true,
            devices: Vec::new(),
            active: None,
            locale: crate::i18n::load_locale(
                &crate::i18n::preferences_path(),
                std::env::var("ZORK_GUI_LOCALE").ok().as_deref(),
            ),
        };
        view.set_nodes(nodes);
        view
    }
    pub fn width(&self, available: f32) -> f32 {
        self.width.min((available - 360.).max(200.))
    }
    pub fn resize(&mut self, x: f32, available: f32, cx: &mut Context<Self>) {
        if self.resizing {
            self.width = x.clamp(200., (available - 360.).clamp(200., 420.));
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    pub fn finish_resize(&mut self, cx: &mut Context<Self>) {
        if self.resizing {
            self.resizing = false;
            let _ = save_view_state(&self.store, "device", ViewState::SidebarWidth, &self.width);
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    pub fn set_nodes(&mut self, nodes: &[SavedNode]) {
        self.devices
            .retain(|d| nodes.iter().any(|n| n.id == d.node.id));
        for node in nodes {
            if let Some(device) = self.devices.iter_mut().find(|d| d.node.id == node.id) {
                device.node = node.clone();
                continue;
            }
            self.devices.push(Device {
                node: node.clone(),
                data: Arc::new(Default::default()),
                selection: Selection::default(),
                core: None,
                subscription: None,
                notification_subscription: None,
            });
        }
    }
    pub fn update_nodes(&mut self, nodes: &[SavedNode], cx: &mut Context<Self>) {
        for device in &self.devices {
            if !nodes.iter().any(|node| node.id == device.node.id) {
                if let Some(core) = &device.core {
                    let ledger = core.notifications().snapshot();
                    for tag in ledger.pending.keys().chain(ledger.presented.keys()) {
                        cx.dismiss_system_notification(tag);
                    }
                }
            }
        }
        self.set_nodes(nodes);
        let names = self
            .devices
            .iter()
            .map(|device| format!("device/{}", device.node.id))
            .collect::<HashSet<_>>();
        if names.is_empty() {
            cx.notify();
        } else {
            let names = names.iter().map(String::as_str).collect::<Vec<_>>();
            zork_ui::components::region::invalidate(cx, &names);
        }
    }
    pub fn bind_node(
        &mut self,
        id: &str,
        core: Arc<zork_client_core::state::Device>,
        cx: &mut Context<Self>,
    ) {
        let Some(device) = self.devices.iter_mut().find(|d| d.node.id == id) else {
            return;
        };
        if device
            .core
            .as_ref()
            .is_some_and(|existing| Arc::ptr_eq(existing, &core))
        {
            return;
        }
        let mut subscription = core.navigation();
        device.data = subscription.snapshot();
        device.core = Some(core.clone());
        let notification_core = core.clone();
        let notification_store = self.store.clone();
        let mut notifications = core.notifications();
        device.notification_subscription = Some(cx.spawn(async move |this, cx| {
            let mut previous = notifications
                .snapshot()
                .presented
                .keys()
                .cloned()
                .collect::<HashSet<_>>();
            let mut retries = 0;
            loop {
                // Coalesce a burst of catalog changes before crossing the OS boundary.
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                notification_core.refresh_notifications();
                let mut ledger = notifications.snapshot();
                let mut failed = false;
                let permission = if ledger.pending.is_empty() {
                    super::notifications::Permission::Allowed
                } else {
                    super::notifications::permission(true)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("notification permission: {error}");
                            failed = true;
                            super::notifications::Permission::Unknown
                        })
                };
                // Permission UI may have been open while the user read/muted this chat.
                notification_core.refresh_notifications();
                ledger = notifications.snapshot();
                let Ok(locale) = this.update(cx, |view, cx| {
                    let current = ledger
                        .pending
                        .keys()
                        .chain(ledger.presented.keys())
                        .cloned()
                        .collect::<HashSet<_>>();
                    for tag in previous.difference(&current) {
                        cx.dismiss_system_notification(tag);
                    }
                    previous = current;
                    view.locale
                }) else {
                    return;
                };
                for notice in ledger.pending.values() {
                    if matches!(
                        permission,
                        super::notifications::Permission::Denied
                            | super::notifications::Permission::Unavailable
                    ) {
                        if let Err(error) = notification_core.discard_notification(&notice.id) {
                            eprintln!("notification suppression: {error}");
                            failed = true;
                        }
                        continue;
                    }
                    if permission != super::notifications::Permission::Allowed {
                        continue;
                    }
                    #[cfg(target_os = "macos")]
                    match super::notifications::accepted(&notice.tag(), &notice.id).await {
                        Ok(true) => {
                            if let Err(error) =
                                notification_core.acknowledge_notification(&notice.id)
                            {
                                eprintln!("notification recovery: {error}");
                                failed = true;
                            }
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("notification recovery: {error}");
                            failed = true;
                            continue;
                        }
                    }
                    // Recheck privacy and visibility after all asynchronous OS queries.
                    notification_core.refresh_notifications();
                    if !notification_core
                        .notifications()
                        .snapshot()
                        .pending
                        .values()
                        .any(|n| n.id == notice.id)
                    {
                        continue;
                    }
                    let prefs =
                        match zork_client_core::notifications::preferences(&notification_store) {
                            Ok(prefs) => prefs,
                            Err(error) => {
                                eprintln!("notification preferences: {error}");
                                failed = true;
                                continue;
                            }
                        };
                    let key = match notice.kind {
                        zork_client_core::notifications::Kind::Reply => "notification_reply",
                        zork_client_core::notifications::Kind::Review => "notification_review",
                        zork_client_core::notifications::Kind::Attention => {
                            "notification_attention"
                        }
                    };
                    let notification = gpui::SystemNotification {
                        tag: notice.tag().into(),
                        title: if prefs.preview && !notice.title.is_empty() {
                            notice.title.clone().into()
                        } else {
                            "Zork".into()
                        },
                        body: locale.text(key).into(),
                        actions: vec![],
                    };
                    #[cfg(target_os = "macos")]
                    let result =
                        super::notifications::post(notification, prefs.sound, &notice.id).await;
                    #[cfg(not(target_os = "macos"))]
                    let result = this.update(cx, |_, cx| cx.show_system_notification(notification));
                    match result {
                        Ok(()) => {
                            if let Err(error) =
                                notification_core.acknowledge_notification(&notice.id)
                            {
                                eprintln!("notification receipt: {error}");
                                failed = true;
                            }
                        }
                        Err(error) => {
                            eprintln!("notification delivery: {error}");
                            failed = true;
                        }
                    }
                }
                if failed && retries < 3 {
                    retries += 1;
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1 << retries))
                        .await;
                    continue;
                }
                retries = 0;
                if notifications.changed().await.is_none() {
                    return;
                }
            }
        }));
        let region = format!("device/{id}");
        let id = id.to_owned();
        device.subscription = Some(cx.spawn(async move |this, cx| {
            while let Some(data) = subscription.changed().await {
                if this
                    .update(cx, |view, cx| {
                        if let Some(device) = view.devices.iter_mut().find(|d| d.node.id == id) {
                            device.data = data;
                            zork_ui::components::region::invalidate(cx, &[&format!("device/{id}")]);
                            cx.emit(Changed);
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        }));
        zork_ui::components::region::invalidate(cx, &[&region]);
        cx.emit(Changed);
    }
    /// Archived Chats across every device, newest first.
    pub fn archived_chats(&self) -> Vec<zork_ui::settings::archived::ArchivedChat> {
        let mut chats: Vec<_> = self
            .devices
            .iter()
            .flat_map(|device| {
                device
                    .data
                    .chats
                    .iter()
                    .filter(|chat| chat.archived)
                    .map(|chat| zork_ui::settings::archived::ArchivedChat {
                        node: device.node.id.clone(),
                        device: device.node.name.clone(),
                        chat: chat.clone(),
                    })
            })
            .collect();
        zork_ui::settings::archived::sort(&mut chats);
        chats
    }
    /// The same archive intent the Chat list sends; core reconciles it.
    pub fn set_chat_archived(&self, node: &str, chat: &str, archived: bool, expected: u64) {
        if let Some(core) = self
            .devices
            .iter()
            .find(|d| d.node.id == node)
            .and_then(|d| d.core.as_ref())
        {
            if let Err(error) = core.set_chat_archived(chat, archived, expected) {
                eprintln!("Chat archive: {error}");
            }
        }
    }
    pub fn set_selection(
        &mut self,
        id: &str,
        selection: Selection,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        let Some(device) = self.devices.iter_mut().find(|d| d.node.id == id) else {
            return;
        };
        let changed = device.selection != selection
            || (self.active.as_deref() == Some(id) && self.locale != locale);
        device.selection = selection;
        if self.active.as_deref() == Some(id) {
            self.locale = locale;
        }
        self.mark_viewed();
        if changed {
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    #[cfg(feature = "headless-bench")]
    pub fn set_preview(
        &mut self,
        id: &str,
        data: zork_client_core::state::NavigationData,
        selection: Selection,
        locale: Locale,
        cx: &mut Context<Self>,
    ) {
        if let Some(device) = self.devices.iter_mut().find(|d| d.node.id == id) {
            device.data = Arc::new(data);
        }
        self.set_selection(id, selection, locale, cx);
    }
    pub fn activate(&mut self, id: &str, cx: &mut Context<Self>) {
        self.active = Some(id.to_owned());
        self.mark_viewed();
        zork_ui::components::region::invalidate_all(cx);
    }
    pub fn set_viewing(&mut self, viewing: bool, cx: &mut Context<Self>) {
        if self.viewing != viewing {
            self.viewing = viewing;
            self.mark_viewed();
            zork_ui::components::region::invalidate_all(cx);
        }
    }
    fn mark_viewed(&self) {
        for device in &self.devices {
            if let Some(core) = &device.core {
                core.report_view(
                    device.selection.selected_session.clone(),
                    self.viewing
                        && self.active.as_ref() == Some(&device.node.id)
                        && matches!(device.selection.route, Some(ShellRoute::Task(_))),
                    device.selection.reading_tail,
                );
            }
        }
    }
}

pub(crate) use zork_ui::navigation::TabGroup;
impl Render for DeviceNavigation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use zork_ui::chat_navigation::{Action, Destination as Intent};
        let view = if let Some(view) = &self.view {
            view.clone()
        } else {
            let locale = self.locale;
            let view = cx.new(|cx| {
                zork_ui::chat_navigation::Navigation::new(
                    zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into())),
                    cx,
                )
            });
            cx.subscribe(&view, |v, _, event: &Action, cx| match event {
                Action::Navigate { node, destination } => {
                    let destination = match destination {
                        Intent::NewChat => Destination::Home,
                        Intent::Conversation { session } => Destination::Conversation {
                            session: session.clone(),
                            leader: None,
                        },
                        Intent::Manage(page) => Destination::Manage(*page),
                    };
                    cx.emit(Navigate {
                        node: node.clone(),
                        destination,
                    });
                }
                Action::Archive {
                    node,
                    chat,
                    archived,
                    expected_message_count,
                } => {
                    v.set_chat_archived(node, chat, *archived, *expected_message_count);
                }
                Action::BeginResize => {
                    v.resizing = true;
                }
                Action::Preview {
                    node,
                    session,
                    hovered,
                } => cx.emit(Preview {
                    node: node.clone(),
                    session: session.clone(),
                    hovered: *hovered,
                }),
            })
            .detach();
            self.view_locale = Some(locale);
            self.view = Some(view.clone());
            view
        };
        let devices = self
            .devices
            .iter()
            .map(|device| zork_ui::chat_navigation::Device {
                id: device.node.id.clone(),
                name: device.node.name.clone(),
                machine: device.node.machine_name.clone(),
                online: device.data.online,
                status: device.data.status.clone(),
                direct: device.data.route.direct,
                public: device.data.route.scope == crate::api::ConnectionScope::Public,
                chats: device.data.chats.clone(),
                selected_session: device.selection.selected_session.clone(),
                chatting: matches!(device.selection.route, Some(ShellRoute::Task(_))),
            })
            .collect();
        view.update(cx, |v, cx| {
            v.set_data(devices, self.active.clone(), self.width, cx);
            if self.view_locale != Some(self.locale) {
                let locale = self.locale;
                v.set_text(
                    zork_ui::resources::Text(std::rc::Rc::new(move |key| locale.text(key).into())),
                    cx,
                );
                self.view_locale = Some(locale);
            }
        });
        view
    }
}
