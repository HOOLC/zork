//! Desktop presentation of the core-owned device directory and host operations.
pub mod client_settings;
#[cfg(feature = "headless-bench")]
mod interaction_story;
pub(crate) mod interaction_view;
mod mesh_settings;
mod model_settings;
pub(crate) mod navigation;
pub mod node;
mod notifications;
pub(crate) mod profile_quota;
mod profiles;
mod resources;
mod startup;
pub mod store;
pub mod transport;
pub(crate) mod ui;
#[cfg(feature = "headless-bench")]
pub use model_settings::ModelSettings as HeadlessModelSettings;
#[cfg(feature = "headless-bench")]
pub use profiles::ProfilesView as HeadlessProfilesView;
#[cfg(feature = "headless-bench")]
pub use resources::ResourcesView as HeadlessResourcesView;

use crate::components::text_input::ComposerInput;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
    views::RootView,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, Entity, Window};
use node::LocalNode;
use std::sync::Arc;
use store::{ClientStore, SavedNode};
use zork_client_core::desktop::directory::{Directory, DirectoryData};

pub use zork_client_core::desktop::{client_root, load_services};

struct DesktopRuntime {
    startup: zork_client_core::desktop::startup::Startup,
}
impl gpui::Global for DesktopRuntime {}

pub struct DesktopRoot {
    source: Arc<Directory>,
    directory_updates: Option<gpui::Task<()>>,
    startup_updates: Option<gpui::Task<()>>,
    startup_state: Arc<zork_client_core::desktop::startup::State>,
    onboarding_models_open: bool,
    store: Arc<ClientStore>,
    local: Arc<LocalNode>,
    local_enabled: bool,
    mesh_identity: Option<String>,
    device_statuses: Arc<std::collections::HashMap<String, zork_ui::device_name::DeviceStatus>>,
    pairing: bool,
    rename_input: Entity<ComposerInput>,
    rename_node_id: Option<String>,
    rename_busy: bool,
    rename_error: Option<String>,
    rename_modal: ui::ModalState,
    remote_name: Entity<ComposerInput>,
    remote_origin: Entity<ComposerInput>,
    remote_addr: Entity<ComposerInput>,
    nodes: Vec<SavedNode>,
    active: Option<Entity<RootView>>,
    preview: Option<(String, String, Entity<RootView>)>,
    active_node_id: Option<String>,
    node_views: std::collections::HashMap<String, (u64, Entity<RootView>)>,
    navigation: Entity<navigation::DeviceNavigation>,
    pending_notification: Option<String>,
    model_settings: Option<Entity<model_settings::ModelSettings>>,
    resource_inspector: Option<Entity<resources::ResourcesView>>,
    service_views: std::collections::HashMap<String, (u64, Entity<resources::ResourcesView>)>,
    applications: Arc<Vec<zork_client_core::pages::ApplicationEntry>>,
    mesh_views: std::collections::HashMap<String, (u64, Entity<mesh_settings::MeshSettings>)>,
    device_info: std::collections::HashMap<String, serde_json::Value>,
    management_tab: usize,
    /// The device page's 服务 entry row expands the services list in place.
    device_services_open: bool,
    mesh_settings: Option<Entity<mesh_settings::MeshSettings>>,
    active_node_name: Option<String>,
    account_state: Arc<zork_client_core::relay_account::controller::Snapshot>,
    account_updates: Option<gpui::Task<()>>,
    client_settings: client_settings::State,
    settings_tabs: navigation::TabGroup,
    opened_account_url: Option<String>,
    managing: bool,
    add_device_open: bool,
    dialog_focus: gpui::FocusHandle,
    device_switch_focus: [gpui::FocusHandle; 2],
    notification_switch_focus: [gpui::FocusHandle; 4],
    add_device_modal: ui::ModalState,
    activation_observed: bool,
    #[cfg(target_os = "macos")]
    settings_action_registered: bool,
    busy: bool,
    error: Option<String>,
}
impl DesktopRoot {
    pub fn install_startup(
        startup: zork_client_core::desktop::startup::Startup,
        cx: &mut gpui::App,
    ) {
        assert!(
            !cx.has_global::<DesktopRuntime>(),
            "one desktop runtime per app"
        );
        cx.set_global(DesktopRuntime { startup });
        cx.on_app_quit(|cx| cx.global::<DesktopRuntime>().startup.shutdown())
            .detach();
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<DesktopRuntime>() {
            Self::install_startup(
                zork_client_core::desktop::startup::Startup::open().expect("open client runtime"),
                cx,
            );
        }
        zork_client_core::desktop::trace_startup("gui.root_begin");
        let startup = &cx.global::<DesktopRuntime>().startup;
        let source = startup.directory.clone();
        let mut startup_updates = startup.subscribe();
        let startup_state = startup_updates.snapshot();
        zork_client_core::desktop::trace_startup("gui.directory_opened");
        let store = source.store.clone();
        let snapshot = source.snapshot();
        let client_settings = client_settings::State {
            locale: crate::i18n::load_locale(
                &crate::i18n::preferences_path(),
                std::env::var("ZORK_GUI_LOCALE").ok().as_deref(),
            ),
            ..Default::default()
        };
        let nodes = snapshot.nodes.as_ref().clone();
        let local_enabled = snapshot.local_enabled;
        let startup_error = snapshot.error.clone();
        let local = source.local.clone();
        let account_state = source.account.snapshot();
        let mut field = |label| {
            let input = cx.new(|cx| ComposerInput::new(label, cx));
            cx.observe(&input, |_, _, cx| cx.notify()).detach();
            input
        };
        let rename_input = field("设备名称");
        let remote_name = field("设备名称");
        let remote_origin = field("目标设备 key: 身份");
        let remote_addr = field("局域网地址（可选），例如 192.168.1.20:43120");
        let mut add_device_modal = ui::ModalState::new(cx);
        add_device_modal.retain("add-device-dialog", None::<()>, cx);
        let navigation = cx.new(|cx| navigation::DeviceNavigation::new(store.clone(), &nodes, cx));
        cx.subscribe(&navigation, |v, _, action: &navigation::Navigate, cx| {
            v.navigate_device(action.clone(), cx);
        })
        .detach();
        cx.subscribe(&navigation, |v, _, event: &navigation::Preview, cx| {
            v.preview_chat(event, cx);
        })
        .detach();
        let notification_root = cx.weak_entity();
        cx.on_system_notification_response(move |response, cx| {
            let _ = notification_root.update(cx, |view, cx| {
                view.open_notification(response.tag.to_string(), cx);
            });
        });
        let mut view = Self {
            source,
            directory_updates: None,
            startup_updates: None,
            startup_state: Arc::new(zork_client_core::desktop::startup::State {
                onboarding: startup_state.onboarding,
                ..Default::default()
            }),
            onboarding_models_open: false,
            store,
            local,
            local_enabled,
            mesh_identity: None,
            device_statuses: Default::default(),
            pairing: false,
            rename_input,
            rename_node_id: None,
            rename_busy: false,
            rename_error: None,
            rename_modal: ui::ModalState::new(cx),
            remote_name,
            remote_origin,
            remote_addr,
            nodes,
            active: None,
            preview: None,
            active_node_id: None,
            node_views: std::collections::HashMap::new(),
            navigation,
            pending_notification: None,
            model_settings: None,
            resource_inspector: None,
            service_views: Default::default(),
            applications: snapshot.applications.clone(),
            mesh_views: Default::default(),
            device_info: Default::default(),
            management_tab: 0,
            device_services_open: false,
            mesh_settings: None,
            active_node_name: None,
            account_state,
            account_updates: None,
            client_settings,
            settings_tabs: navigation::TabGroup::new(cx),
            opened_account_url: None,
            managing: false,
            add_device_open: false,
            dialog_focus: cx.focus_handle(),
            device_switch_focus: [cx.focus_handle(), cx.focus_handle()],
            notification_switch_focus: std::array::from_fn(|_| cx.focus_handle()),
            add_device_modal,
            activation_observed: false,
            #[cfg(target_os = "macos")]
            settings_action_registered: false,
            busy: false,
            error: startup_error,
        };
        view.watch_directory(cx);
        view.watch_data_reset(cx);
        view.apply_startup(startup_state, cx);
        view.startup_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(state) = startup_updates.changed().await {
                if this
                    .update(cx, |view, cx| view.apply_startup(state, cx))
                    .is_err()
                {
                    return;
                }
            }
        }));
        zork_client_core::desktop::trace_startup("gui.root_created");
        view
    }
    fn watch_directory(&mut self, cx: &mut Context<Self>) {
        let mut account = self.source.account.subscribe();
        self.apply_account(account.snapshot(), cx);
        self.account_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(snapshot) = account.changed().await {
                if this
                    .update(cx, |view, cx| view.apply_account(snapshot, cx))
                    .is_err()
                {
                    return;
                }
            }
        }));
        let mut updates = self.source.subscribe();
        self.apply_directory(updates.snapshot(), cx);
        self.directory_updates = Some(cx.spawn(async move |this, cx| {
            while let Some(snapshot) = updates.changed().await {
                if this
                    .update(cx, |view, cx| view.apply_directory(snapshot, cx))
                    .is_err()
                {
                    return;
                }
            }
        }));
    }
    fn apply_account(
        &mut self,
        snapshot: Arc<zork_client_core::relay_account::controller::Snapshot>,
        cx: &mut Context<Self>,
    ) {
        if self.opened_account_url != snapshot.login_url {
            self.opened_account_url = snapshot.login_url.clone();
            if let Some(url) = &snapshot.login_url {
                cx.open_url(url);
            }
        }
        self.account_state = snapshot;
        cx.notify();
    }
    fn apply_directory(&mut self, snapshot: Arc<DirectoryData>, cx: &mut Context<Self>) {
        if !Arc::ptr_eq(&self.applications, &snapshot.applications) {
            self.applications = snapshot.applications.clone();
            for (_, root) in self.node_views.values() {
                root.update(cx, |root, cx| {
                    root.set_applications(self.applications.clone(), cx)
                });
            }
        }
        let nodes_changed = self.nodes != *snapshot.nodes;
        self.nodes = snapshot.nodes.as_ref().clone();
        self.local_enabled = snapshot.local_enabled;
        self.mesh_identity = snapshot.mesh_identity.clone();
        self.device_info = snapshot.info.as_ref().clone();
        let statuses_changed = !Arc::ptr_eq(&self.device_statuses, &snapshot.device_statuses);
        if statuses_changed {
            self.device_statuses = snapshot.device_statuses.clone();
            for (_, mesh) in self.mesh_views.values() {
                mesh.update(cx, |_, cx| cx.notify());
            }
        }
        if let Some(error) = &snapshot.error {
            self.error = Some(error.clone());
        }
        if (nodes_changed || statuses_changed) && self.model_settings.is_some() {
            self.sync_model_settings(cx);
        }
        if nodes_changed {
            self.end_preview(cx);
            self.navigation
                .update(cx, |nav, cx| nav.update_nodes(&self.nodes, cx));
            for node in self.nodes.clone() {
                self.apply_device_name(&node.id, &node.name, cx);
                self.ensure_node_view(&node, cx);
            }
            if let Some(id) = self.active_node_id.clone() {
                if self.node_views.get(&id).is_some_and(|(_, current)| {
                    self.active.as_ref().is_some_and(|active| active != current)
                }) {
                    if let Some(node) = self.source.node(&id) {
                        let managing = self.managing;
                        self.open_node(node, cx);
                        self.managing = managing;
                    }
                }
            }
        }
        if nodes_changed || statuses_changed {
            for (id, (_, root)) in &self.node_views {
                let choice = self.source.new_chat_devices(id);
                root.update(cx, |root, cx| root.set_new_chat_devices(choice, cx));
            }
        }
        cx.notify();
    }
    fn login_account(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if let Err(error) = self.source.login_account() {
            self.error = Some(error.to_string());
        }
        cx.notify();
    }

    fn navigate_device(&mut self, action: navigation::Navigate, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let commits_preview = self.preview.as_ref().is_some_and(|(node, session, _)| {
            action.node.as_ref() == Some(node)
                && matches!(
                    &action.destination,
                    navigation::Destination::Conversation { session: target, .. } if target == session
                )
        });
        if commits_preview {
            if let Some((_, _, view)) = self.preview.take() {
                view.update(cx, |view, cx| view.commit_preview(cx));
            }
        } else {
            self.end_preview(cx);
        }
        if action.node.is_none()
            && matches!(action.destination, navigation::Destination::Manage(3))
            && self.active.is_some()
        {
            self.add_device_open = true;
            if let Some(mesh) = &self.mesh_settings {
                mesh.update(cx, |v, cx| {
                    v.enrollment_only = true;
                    cx.notify();
                });
            }
            cx.notify();
            return;
        }
        if let Some(id) = &action.node {
            let Some(node) = self.nodes.iter().find(|n| &n.id == id).cloned() else {
                return;
            };
            if self.active_node_id.as_ref() != Some(id) {
                self.open_node(node, cx);
            }
        }
        self.apply_navigation(action.destination, cx);
    }
    fn preview_chat(&mut self, event: &navigation::Preview, cx: &mut Context<Self>) {
        if !event.hovered {
            if self
                .preview
                .as_ref()
                .is_some_and(|(node, session, _)| node == &event.node && session == &event.session)
            {
                self.end_preview(cx);
            }
            return;
        }
        if self.busy || self.managing || self.add_device_open {
            return;
        }
        if self
            .preview
            .as_ref()
            .is_some_and(|(node, session, _)| node == &event.node && session == &event.session)
        {
            return;
        }
        self.end_preview(cx);
        let Some(node) = self
            .nodes
            .iter()
            .find(|node| node.id == event.node)
            .cloned()
        else {
            return;
        };
        let Some(view) = self.ensure_node_view(&node, cx) else {
            return;
        };
        let overlay = self.active.as_ref() != Some(&view);
        if view.update(cx, |view, cx| {
            view.preview_session(&event.session, overlay, cx)
        }) {
            self.preview = Some((event.node.clone(), event.session.clone(), view));
            cx.notify();
        }
    }
    fn end_preview(&mut self, cx: &mut Context<Self>) {
        if let Some((_, _, view)) = self.preview.take() {
            view.update(cx, |view, cx| view.restore_preview(cx));
            cx.notify();
        }
    }
    fn apply_navigation(&mut self, destination: navigation::Destination, cx: &mut Context<Self>) {
        self.end_preview(cx);
        if let navigation::Destination::Manage(tab) = destination {
            self.managing = true;
            self.management_tab = if tab == 1 { 0 } else { tab };
            if matches!(tab, 0 | 1) {
                self.sync_model_settings(cx);
            }
        } else if let Some(active) = &self.active {
            self.managing = false;
            active.update(cx, |v, cx| v.navigate_device(&destination, cx));
        }
        cx.notify();
    }
    fn sync_model_settings(&mut self, cx: &mut Context<Self>) {
        let sources = self
            .source
            .snapshot()
            .nodes
            .as_ref()
            .clone()
            .into_iter()
            .filter_map(|node| {
                let view = self.ensure_node_view(&node, cx)?;
                Some((
                    node.id.clone(),
                    node.name,
                    view.read(cx).core_device().profiles(),
                    self.source.device_status(&node.id),
                ))
            })
            .collect();
        let view = self
            .model_settings
            .get_or_insert_with(|| cx.new(model_settings::ModelSettings::new))
            .clone();
        view.update(cx, |v, cx| v.set_sources(sources, cx));
    }
    fn close_add_device(&mut self, cx: &mut Context<Self>) {
        self.add_device_open = false;
        cx.notify();
    }
    fn ensure_node_view(
        &mut self,
        node: &SavedNode,
        cx: &mut Context<Self>,
    ) -> Option<Entity<RootView>> {
        let (binding, client) = self.source.connection(&node.id).ok()?;
        let existing = self
            .node_views
            .get(&node.id)
            .filter(|(version, _)| *version == binding)
            .map(|(_, view)| view.clone());
        Some(existing.unwrap_or_else(|| {
            let active = cx.new(|cx| {
                let mut view =
                    RootView::new_desktop(client.clone(), self.store.clone(), node.id.clone(), cx);
                zork_client_core::desktop::trace_startup("gui.workspace_view_created");
                view.attach_navigation(self.navigation.clone(), node.name.clone());
                view.set_applications(self.applications.clone(), cx);
                view.set_new_chat_devices(self.source.new_chat_devices(&node.id), cx);
                view
            });
            let source_node = node.id.clone();
            cx.subscribe(
                &active,
                move |v, _, action: &crate::views::DesktopAction, cx| {
                    match action {
                        crate::views::DesktopAction::ManageNode => {
                            v.apply_navigation(navigation::Destination::Manage(0), cx);
                        }
                        crate::views::DesktopAction::SelectChatDevice(id) => {
                            match v.source.new_chat_destination(&source_node, id) {
                                Ok(node) => v.navigate_device(
                                    navigation::Navigate {
                                        node: Some(node.id),
                                        destination: navigation::Destination::Home,
                                    },
                                    cx,
                                ),
                                Err(error) => v.error = Some(error.to_string()),
                            }
                        }
                    }
                    cx.notify();
                },
            )
            .detach();
            let resource_node = node.id.clone();
            cx.subscribe(
                &active,
                move |desktop, _, event: &crate::views::InspectResource, cx| {
                    match desktop
                        .source
                        .inspection_node(&resource_node, &event.target)
                    {
                        Ok(node) => {
                            let core = desktop.source.resources();
                            let query = event.target.query.clone();
                            let locale = desktop.client_settings.locale;
                            desktop.resource_inspector = Some(cx.new(|cx| {
                                resources::ResourcesView::inspector(core, node, query, locale, cx)
                            }));
                        }
                        Err(error) => {
                            let core = desktop.source.resources();
                            let locale = desktop.client_settings.locale;
                            desktop.resource_inspector = Some(cx.new(|cx| {
                                resources::ResourcesView::unavailable(
                                    core,
                                    error.to_string(),
                                    locale,
                                    cx,
                                )
                            }));
                        }
                    }
                    cx.notify();
                },
            )
            .detach();
            let sidebar = self.navigation.clone();
            let id = node.id.clone();
            let core = active.read(cx).core_device();
            sidebar.update(cx, |nav, cx| nav.bind_node(&id, core.clone(), cx));
            zork_client_core::desktop::trace_startup("gui.workspace_navigation_bound");
            cx.subscribe(
                &active,
                move |_, _, change: &crate::views::NavigationChanged, cx| {
                    sidebar.update(cx, |nav, cx| {
                        nav.set_selection(&id, change.selection.clone(), change.locale, cx)
                    });
                },
            )
            .detach();
            self.source.bind(node.id.clone(), core, client);
            zork_client_core::desktop::trace_startup("gui.workspace_core_bound");
            self.node_views
                .insert(node.id.clone(), (binding, active.clone()));
            active.update(cx, |v, cx| v.start_device_updates(cx));
            zork_client_core::desktop::trace_startup("gui.workspace_updates_started");
            active
        }))
    }
    fn open_node(&mut self, node: SavedNode, cx: &mut Context<Self>) {
        let Some(node) = self.source.node(&node.id) else {
            return;
        };
        let Ok((binding, _)) = self.source.connection(&node.id) else {
            return;
        };
        zork_client_core::desktop::trace_startup("gui.node_connection_ready");
        if let Err(error) = self.source.select(&node.id) {
            self.error = Some(error.to_string());
        }
        self.active_node_name = Some(node.name.clone());
        self.active_node_id = Some(node.id.clone());
        self.navigation
            .update(cx, |nav, cx| nav.update_nodes(&self.nodes, cx));
        let Some(retained) = self.ensure_node_view(&node, cx) else {
            return;
        };
        zork_client_core::desktop::trace_startup("gui.node_view_ready");
        let profile_source = retained.read(cx).core_device().profiles();
        if let Err(error) = cx
            .global::<DesktopRuntime>()
            .startup
            .observe_onboarding_models(&node.id, profile_source)
        {
            self.error = Some(error.to_string());
        }
        if let Some((_, mesh)) = self
            .mesh_views
            .get(&node.id)
            .filter(|(version, _)| *version == binding)
        {
            self.mesh_settings = Some(mesh.clone());
        } else {
            let mesh = cx.new(|cx| {
                mesh_settings::MeshSettings::new(retained.read(cx).core_device().mesh_admin(), cx)
            });
            self.mesh_views
                .insert(node.id.clone(), (binding, mesh.clone()));
            self.mesh_settings = Some(mesh);
        }
        if !self
            .service_views
            .get(&node.id)
            .is_some_and(|(version, _)| *version == binding)
        {
            let resources = self.source.resources();
            let locale = self.client_settings.locale;
            let services = cx.new(|cx| {
                resources::ResourcesView::services(resources, node.id.clone(), locale, cx)
            });
            self.service_views
                .insert(node.id.clone(), (binding, services));
        }
        let source = self.source.clone();
        let info_node = node.clone();
        if self.startup_state.onboarding.is_some() {
            self.sync_model_settings(cx);
        }
        zork_client_core::desktop::trace_startup("gui.node_management_ready");
        cx.spawn(async move |_, _| {
            let _ = source.refresh_info(&info_node).await;
        })
        .detach();
        for saved in self.nodes.clone() {
            self.ensure_node_view(&saved, cx);
        }
        let active = retained;
        let (selection, locale) = active.read(cx).navigation_selection();
        self.navigation.update(cx, |nav, cx| {
            nav.update_nodes(&self.nodes, cx);
            nav.activate(&node.id, cx);
            nav.set_selection(&node.id, selection, locale, cx);
        });
        if let Some(previous) = &self.active {
            previous.update(cx, |view, cx| view.hide_browser(cx));
        }
        self.active = Some(active);
        self.managing = false;
        cx.notify();
    }
    fn start_pairing(&mut self, node: Option<SavedNode>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        self.pairing = true;
        let source = self.source.clone();
        let work = cx
            .background_executor()
            .spawn(async move { source.pair(node) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(Some(node)) => {
                        view.open_node(node, cx);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        view.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn connect_remote(&mut self, cx: &mut Context<Self>) {
        match Directory::remote_input(
            self.remote_origin.read(cx).value().into(),
            self.remote_name.read(cx).value().into(),
            self.remote_addr.read(cx).value().into(),
        ) {
            Ok(node) => self.start_pairing(Some(node), cx),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }
    fn start_node(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        if self.active.is_none() {
            self.managing = false;
        }
        cx.global::<DesktopRuntime>().startup.start_local();
        cx.notify();
    }
    fn stop_node(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if !cx.global::<DesktopRuntime>().startup.stop_local() {
            return;
        }
        self.error = None;
        if let Some(previous) = &self.active {
            previous.update(cx, |view, cx| view.hide_browser(cx));
        }
        self.active = None;
        self.mesh_settings = None;
        self.active_node_name = None;
        self.active_node_id = None;
        self.management_tab = 3;
        cx.notify();
    }

    fn set_node_background(&mut self, enabled: bool, at_login: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let local = self.local.clone();
        let work = cx
            .background_executor()
            .spawn(async move { local.set_background(enabled, at_login) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                v.error = result.err().map(|e| e.to_string());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
impl DesktopRoot {
    fn apply_device_name(&mut self, id: &str, name: &str, cx: &mut Context<Self>) {
        self.navigation
            .update(cx, |nav, cx| nav.update_nodes(&self.nodes, cx));
        if let Some((_, view)) = self.node_views.get(id) {
            view.update(cx, |v, cx| {
                v.attach_navigation(self.navigation.clone(), name.into());
                cx.notify();
            });
        }
        if self.active_node_id.as_deref() == Some(id) {
            self.active_node_name = Some(name.into());
        }
    }
    fn open_device_rename(&mut self, cx: &mut Context<Self>) {
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| Some(&n.id) == self.active_node_id.as_ref())
        else {
            return;
        };
        self.rename_input
            .update(cx, |v, cx| v.set_value(node.name.clone(), cx));
        self.rename_node_id = Some(node.id.clone());
        self.rename_error = None;
        cx.notify();
    }
    fn save_device_name(&mut self, cx: &mut Context<Self>) {
        if self.rename_busy {
            return;
        }
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| Some(&n.id) == self.rename_node_id.as_ref())
            .cloned()
        else {
            return;
        };
        let name = self.rename_input.read(cx).value().to_owned();
        self.rename_busy = true;
        self.rename_error = None;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.rename(&node, name).await;
            let _ = this.update(cx, |view, cx| {
                view.rename_busy = false;
                match result {
                    Ok(()) => view.rename_node_id = None,
                    Err(e) => view.rename_error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn refresh_device_info(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| Some(&n.id) == self.active_node_id.as_ref())
            .cloned()
        else {
            return;
        };
        self.busy = true;
        self.error = None;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.refresh_info(&node).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.error = result.err().map(|e| e.to_string());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn update_device_release(&mut self, install: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| Some(&n.id) == self.active_node_id.as_ref())
            .cloned()
        else {
            return;
        };
        self.busy = true;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = source.update_release(&node, install).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.error = result.err().map(|e| e.to_string());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn render_device(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use zork_ui::settings::{DeviceAction, DeviceData};
        let Some(node) = self
            .nodes
            .iter()
            .find(|n| Some(&n.id) == self.active_node_id.as_ref())
        else {
            return self.render_nodes(cx);
        };
        let info = self.device_info.get(&node.id);
        let data = DeviceData {
            name: node.name.clone(),
            status: self.source.device_status(&node.id),
            version: info
                .and_then(|i| {
                    i["station"]["release_version"]
                        .as_str()
                        .or(i["station"]["version"].as_str())
                })
                .unwrap_or("尚未获取")
                .into(),
            update_supported: info.is_some_and(|i| i["update"]["supported"] == true),
            update_reason: info
                .and_then(|i| i["update"]["reason"].as_str())
                .map(|reason| {
                    if node.local && !self.local.background() {
                        "开启后台运行后可升级".into()
                    } else {
                        reason.into()
                    }
                }),
            latest_version: info
                .and_then(|i| i["latest_version"].as_str())
                .map(str::to_owned),
            online: self
                .active
                .as_ref()
                .and_then(|v| v.read(cx).core_device().snapshot().online),
            local: node.local,
            running: self.local_enabled,
            background: self.local.background(),
            start_at_login: self.local.start_at_login(),
            busy: self.busy
                || matches!(
                    self.startup_state.dependency(&node),
                    zork_client_core::desktop::startup::Phase::Preparing
                        | zork_client_core::desktop::startup::Phase::Stopping
                ),
            notice: info
                .and_then(|i| {
                    i["error"]
                        .as_str()
                        .or(i["update"]["status"]["message"].as_str())
                })
                .map(str::to_owned),
            services: Some(String::new()),
            connections: Some(String::new()),
        };
        let services_open = self.device_services_open;
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(zork_ui::settings::device(
                data,
                &self.device_switch_focus,
                window,
                cx,
                |v, action, cx| match action {
                    DeviceAction::Rename => v.open_device_rename(cx),
                    DeviceAction::Refresh => v.refresh_device_info(cx),
                    DeviceAction::CheckUpdate => v.update_device_release(false, cx),
                    DeviceAction::Upgrade => v.update_device_release(true, cx),
                    DeviceAction::ToggleRunning => {
                        if v.local_enabled {
                            v.stop_node(cx)
                        } else {
                            v.start_node(cx)
                        }
                    }
                    DeviceAction::Background(on) => v.set_node_background(
                        on,
                        if on { v.local.start_at_login() } else { false },
                        cx,
                    ),
                    DeviceAction::StartAtLogin(on) => v.set_node_background(true, on, cx),
                    DeviceAction::Services => {
                        v.device_services_open = !v.device_services_open;
                        cx.notify();
                    }
                    // Model connections live on their own settings tab.
                    DeviceAction::Connections => {
                        v.management_tab = 0;
                        cx.notify();
                    }
                },
            ))
            .when_some(
                self.service_views
                    .get(&node.id)
                    .filter(|_| services_open)
                    .map(|(_, view)| view.clone()),
                |body, view| body.child(view),
            )
    }
    fn render_account(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use zork_ui::settings::{AccountAction, AccountData};
        zork_ui::settings::account(
            AccountData {
                name: self
                    .account_state
                    .subject
                    .as_ref()
                    .map(|_| "Zork 账号".into()),
                email: self.account_state.email.clone(),
                identity: None,
                busy: self.account_state.busy(),
                signing_out: self.account_state.phase
                    == zork_client_core::relay_account::controller::Phase::SigningOut,
                notice: self.account_state.error.clone().or_else(|| {
                    (self.account_state.pending_revocations > 0)
                        .then(|| "已退出本机，正在等待服务器确认撤销。".into())
                }),
            },
            window,
            cx,
            |v, action, cx| match action {
                AccountAction::Login => v.login_account(cx),
                AccountAction::Cancel => {
                    if let Err(error) = v.source.cancel_account() {
                        v.error = Some(error.to_string());
                    }
                    cx.notify();
                }
                AccountAction::Logout => {
                    if let Err(error) = v.source.logout() {
                        v.error = Some(error.to_string());
                    }
                    cx.notify();
                }
                AccountAction::CopyIdentity => {
                    if let Some(identity) = &v.mesh_identity {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(identity.clone()));
                        v.error = Some("客户端身份已复制。".into());
                        cx.notify();
                    }
                }
            },
        )
    }
}
use zork_ui::node_directory::Host as _;

impl Render for DesktopRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !(self.managing
            && self.management_tab == 4
            && self.client_settings.page == client_settings::Page::Data)
        {
            if let Some(view) = &self.client_settings.data {
                view.update(cx, |view, cx| view.cancel(cx));
            }
        }
        if !self.busy {
            if let Some(tag) = self.pending_notification.take() {
                let view = cx.weak_entity();
                cx.defer(move |cx| {
                    let _ = view.update(cx, |view, cx| view.open_notification(tag, cx));
                });
            }
        }
        let p = ZORK_UI.palette;
        if !self.activation_observed {
            cx.observe_window_activation(window, |_, _, cx| cx.notify())
                .detach();
            self.activation_observed = true;
        }
        self.navigation.update(cx, |nav, cx| {
            nav.set_viewing(
                !self.managing
                    && self.preview.is_none()
                    && !self.add_device_open
                    && window.is_window_active(),
                cx,
            )
        });
        self.rename_modal.sync(
            self.rename_node_id.as_ref().map(|_| "device-rename-dialog"),
            window,
            cx,
        );
        self.add_device_modal.sync(
            self.add_device_open.then_some("add-device-dialog"),
            window,
            cx,
        );
        let rename_visible = self
            .rename_modal
            .retain("device-rename-dialog", self.rename_node_id.clone(), cx)
            .is_some();
        let add_device_visible = self
            .add_device_modal
            .retain("add-device-dialog", self.add_device_open.then_some(()), cx)
            .is_some();
        let width = self
            .navigation
            .read(cx)
            .width(window.viewport_size().width.as_f32());
        if let Some(view) = &self.model_settings {
            let onboarding = matches!(
                self.startup_state.onboarding,
                Some(
                    zork_client_core::desktop::startup::Onboarding::Models
                        | zork_client_core::desktop::startup::Onboarding::Ready
                )
            ) && self.onboarding_models_open;
            let active = onboarding.then(|| self.active_node_id.clone()).flatten();
            let visible = (self.managing && self.management_tab == 0) || onboarding;
            view.update(cx, |v, cx| {
                v.set_onboarding_local(active, cx);
                v.set_visible(visible, cx);
                v.set_locale(self.client_settings.locale, cx);
            });
        }
        let tab = if self.active.is_none() && !matches!(self.management_tab, 0 | 4 | 5) {
            3
        } else {
            self.management_tab
        };
        let shell = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .track_focus(&self.dialog_focus)
            .bg(rgb(p.window))
            .on_key_down(cx.listener(|v, e: &gpui::KeyDownEvent, _, cx| {
                if v.add_device_open && e.keystroke.key == "escape" {
                    v.close_add_device(cx);
                    cx.stop_propagation();
                }
            }))
            .text_color(rgb(p.text))
            .font_family("Inter Variable")
            .text_size(px(13.))
            .on_mouse_move(cx.listener(|v, e: &gpui::MouseMoveEvent, w, cx| {
                v.navigation.update(cx, |n, cx| {
                    n.resize(e.position.x.as_f32(), w.viewport_size().width.as_f32(), cx)
                });
            }))
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    v.navigation.update(cx, |n, cx| n.finish_resize(cx));
                }),
            );
        let content = if self.startup_state.onboarding.is_some() {
            self.render_onboarding(cx)
        } else if !self.managing {
            if let Some(active) = self.active.clone() {
                let preview = self
                    .preview
                    .as_ref()
                    .filter(|(_, _, view)| view != &active)
                    .map(|(_, _, view)| view.clone());
                div()
                    .size_full()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().child(active))
                    .when_some(preview, |panel, view| {
                        panel.child(
                            div()
                                .id("chat-hover-preview")
                                .absolute()
                                .left(px(width))
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .occlude()
                                .child(view),
                        )
                    })
                    .when_some(self.startup_notice(cx), |v, notice| v.child(notice))
            } else {
                self.render_startup(cx)
            }
        } else {
            div()
                .size_full()
                .flex()
                .child(
                    self.settings_tabs.surface(
                        self.settings_tabs
                            .column()
                            .w(px(width))
                            .relative()
                            .flex_shrink_0()
                            .h_full()
                            .px_2()
                            .child(
                                div()
                                    .h(px(48.))
                                    .pl(px(64.))
                                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                                        window.start_window_move()
                                    })
                                    .child(window.use_keyed_state(
                                        "management-header-brand",
                                        cx,
                                        |_, _| {
                                            crate::components::brand::Brand::new(
                                                crate::components::brand::BrandMotion::Header,
                                                p.sidebar,
                                            )
                                        },
                                    )),
                            )
                            .child(
                                self.settings_tabs
                                    .tab("desktop-return".into(), false)
                                    .child(ui::icon("icons/arrow-left.svg", 16.))
                                    .child(self.client_settings.locale.text(
                                        if self.active.is_some() {
                                            "startup_return_chat"
                                        } else {
                                            "startup_return_home"
                                        },
                                    ))
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.managing = false;
                                        cx.notify();
                                    }))
                                    .automation(
                                        AutomationRole::Button,
                                        self.client_settings.locale.text(
                                            if self.active.is_some() {
                                                "startup_return_chat"
                                            } else {
                                                "startup_return_home"
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                self.settings_tabs
                                    .column()
                                    .id("settings-navigation-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .child(self.render_client_settings_navigation(tab == 4, cx))
                                    .child(
                                        self.settings_tabs
                                            .tab("settings-models".into(), tab == 0)
                                            .child(ui::icon("icons/models.svg", 20.))
                                            .child("模型连接")
                                            .on_click(cx.listener(|v, _, _, cx| {
                                                v.apply_navigation(
                                                    navigation::Destination::Manage(0),
                                                    cx,
                                                )
                                            }))
                                            .automation(AutomationRole::Button, "模型连接"),
                                    )
                                    .child(
                                        self.settings_tabs
                                            .section(
                                                "device-settings-heading",
                                                self.client_settings
                                                    .locale
                                                    .text("device_settings_title"),
                                            )
                                            .children(self.nodes.clone().into_iter().map(|node| {
                                                let selected =
                                                    self.active_node_id.as_ref() == Some(&node.id);
                                                let open = node.clone();
                                                self.settings_tabs.column().child(
                                                    self.settings_tabs
                                                        .tab(
                                                            format!("settings-device-{}", node.id),
                                                            selected && tab == 3,
                                                        )
                                                        .child(ui::icon("icons/node.svg", 20.))
                                                        .child(
                                                            div().flex_1().min_w_0().child(
                                                                zork_ui::device_name::label(
                                                                    format!(
                                                                        "settings-name-{}",
                                                                        node.id
                                                                    ),
                                                                    node.name.clone(),
                                                                    &self
                                                                        .source
                                                                        .device_status(&node.id),
                                                                    None,
                                                                ),
                                                            ),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |v, _, _, cx| {
                                                                if !v.busy {
                                                                    v.open_node(open.clone(), cx);
                                                                    v.managing = true;
                                                                    v.management_tab = 3;
                                                                    cx.notify();
                                                                }
                                                            },
                                                        ))
                                                        .automation(
                                                            AutomationRole::Button,
                                                            format!("{} 设备设置", node.name),
                                                        ),
                                                )
                                            }))
                                            .child(
                                                self.settings_tabs
                                                    .tab("settings-add-device".into(), false)
                                                    .child(ui::icon("icons/plus.svg", 20.))
                                                    .child("连接设备")
                                                    .on_click(cx.listener(|v, _, _, cx| {
                                                        v.navigate_device(
                                                            navigation::Navigate {
                                                                node: None,
                                                                destination:
                                                                    navigation::Destination::Manage(
                                                                        3,
                                                                    ),
                                                            },
                                                            cx,
                                                        )
                                                    }))
                                                    .automation(AutomationRole::Button, "连接设备"),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right_0()
                                    .top_0()
                                    .w(px(5.))
                                    .h_full()
                                    .id("settings-sidebar-resize")
                                    .cursor_col_resize()
                                    .on_mouse_down(
                                        gpui::MouseButton::Left,
                                        cx.listener(|v, _, _, cx| {
                                            v.navigation.update(cx, |n, _| n.resizing = true);
                                            cx.stop_propagation();
                                        }),
                                    ),
                            ),
                    ),
                )
                .child(
                    div().flex_1().min_w_0().h_full().bg(rgb(p.canvas)).child(
                        div()
                            .id("desktop-settings-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .child(ui::settings_content(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .when(tab == 3, |v| v.child(self.render_device(window, cx)))
                                    .when(tab == 4, |v| v.child(self.render_client_settings(window, cx)))
                                    .when(tab == 0, |v| {
                                        v.when_some(self.model_settings.clone(), |v, e| v.child(e))
                                    })
                                    .when(tab == 2, |v| {
                                        v.when_some(self.mesh_settings.clone(), |v, e| v.child(e))
                                    })
                                    .when_some(self.error.clone(), |v, e| v.child(ui::feedback(e))),
                            )),
                    ),
                )
        };
        shell
            .child(content)
            .when_some(self.resource_inspector.clone(), |shell, view| {
                shell.child(view)
            })
            .when(rename_visible, |shell| {
                shell.child(zork_ui::settings::rename_device::render(
                    &self.rename_input,
                    self.rename_busy,
                    self.rename_error.clone(),
                    &self.rename_modal,
                    window,
                    cx,
                    |v, action, cx| match action {
                        zork_ui::settings::rename_device::Action::Save => v.save_device_name(cx),
                        zork_ui::settings::rename_device::Action::Cancel => {
                            v.rename_node_id = None;
                            cx.notify();
                        }
                    },
                ))
            })
            .when(add_device_visible, |shell| {
                shell.children(self.mesh_settings.clone().map(|view| {
                    let data = view.read(cx).enrollment_data();
                    zork_ui::network::enrollment_dialog(
                        data,
                        &self.add_device_modal,
                        window,
                        cx,
                        move |_, event, cx| {
                            view.update(cx, |view, cx| view.enrollment_action(event, cx));
                        },
                        |view, cx| view.close_add_device(cx),
                    )
                }))
            })
    }
}

#[cfg(feature = "headless-bench")]
pub use ui::settings_content as headless_settings_content;

#[cfg(feature = "headless-bench")]
pub mod stories;

#[cfg(feature = "headless-bench")]
mod notification_story;
#[cfg(feature = "headless-bench")]
mod onboarding_story;

impl zork_ui::node_directory::Host for DesktopRoot {
    fn nodes_data(&self) -> zork_ui::node_directory::Data {
        zork_ui::node_directory::Data {
            nodes: self
                .nodes
                .iter()
                .map(|n| zork_ui::node_directory::Node {
                    id: n.id.clone(),
                    name: n.name.clone(),
                    status: self.source.device_status(&n.id),
                    remote: n.mesh.is_some(),
                })
                .collect(),
            running: self.local.running(),
            enabled: self.local_enabled,
            busy: self.busy
                || matches!(
                    self.startup_state.local,
                    zork_client_core::desktop::startup::Phase::Preparing
                        | zork_client_core::desktop::startup::Phase::Stopping
                ),
            background: self.local.background(),
            start_at_login: self.local.start_at_login(),
            pairing: self.pairing,
            mesh_identity: self.mesh_identity.clone(),
            remote_name: self.remote_name.clone(),
            remote_origin: self.remote_origin.clone(),
            remote_addr: self.remote_addr.clone(),
        }
    }
    fn node_action(&mut self, action: zork_ui::node_directory::Action, cx: &mut Context<Self>) {
        use zork_ui::node_directory::Action;
        match action {
            Action::Start => self.start_node(cx),
            Action::Stop => self.stop_node(cx),
            Action::Background { enabled, at_login } => {
                self.set_node_background(enabled, at_login, cx)
            }
            Action::Pair(id) => {
                let node = id
                    .and_then(|id| self.nodes.iter().find(|n| n.id == id))
                    .cloned();
                self.start_pairing(node, cx);
            }
            Action::CancelPair => {
                self.pairing = false;
                cx.notify();
            }
            Action::Connect => self.connect_remote(cx),
            Action::Open(id) => {
                if let Some(node) = self.nodes.iter().find(|n| n.id == id).cloned() {
                    self.open_node(node, cx);
                }
            }
            Action::CopyIdentity => {
                if let Some(identity) = &self.mesh_identity {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(identity.clone()));
                }
            }
        }
    }
}
