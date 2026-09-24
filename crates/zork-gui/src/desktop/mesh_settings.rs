use super::ui;
use crate::{
    api::{MeshAction, MeshAdmin, MeshAdminData},
    components::text_input::ComposerInput,
};
use gpui::{prelude::*, Context, Entity, Window};
use std::sync::Arc;
pub struct MeshSettings {
    pub enrollment_only: bool,
    source: Arc<MeshAdmin>,
    updates: Option<gpui::Task<()>>,
    saved: u64,
    config: Option<crate::api::MeshConfig>,
    origin: Option<String>,
    name: Entity<ComposerInput>,
    peer: Entity<ComposerInput>,
    addr: Entity<ComposerInput>,
    client_grant: bool,
    form_open: bool,
    busy: bool,
    message: Option<String>,
    invitation: Option<serde_json::Value>,
    peers: Vec<crate::api::MeshPeer>,
    modal: ui::ModalState,
    switch_focus: [gpui::FocusHandle; 2],
}
impl MeshSettings {
    pub fn new(source: Arc<MeshAdmin>, cx: &mut Context<Self>) -> Self {
        let mut field = |label| {
            let e = cx.new(|cx| ComposerInput::new(label, cx));
            cx.observe(&e, |_, _, cx| cx.notify()).detach();
            e
        };
        let name = field("设备名称");
        let peer = field("key: 设备身份");
        let addr = field("局域网地址（可选）");
        let mut v = Self {
            enrollment_only: false,
            source,
            updates: None,
            saved: 0,
            config: None,
            origin: None,
            name,
            peer,
            addr,
            client_grant: false,
            form_open: false,
            busy: false,
            message: None,
            invitation: None,
            peers: vec![],
            modal: ui::ModalState::new(cx),
            switch_focus: [cx.focus_handle(), cx.focus_handle()],
        };
        v.watch(cx);
        v.refresh(cx);
        v
    }
    pub fn refresh(&mut self, _cx: &mut Context<Self>) {
        self.source.dispatch(MeshAction::Refresh);
    }
    fn watch(&mut self, cx: &mut Context<Self>) {
        let mut updates = self.source.subscribe();
        self.accept(updates.snapshot(), cx);
        self.updates = Some(cx.spawn(async move |this, cx| {
            while let Some(state) = updates.changed().await {
                if this.update(cx, |view, cx| view.accept(state, cx)).is_err() {
                    return;
                }
            }
        }));
    }
    fn accept(&mut self, state: Arc<MeshAdminData>, cx: &mut Context<Self>) {
        self.config = state.config.clone();
        self.origin = state.origin.clone();
        self.peers = state.peers.clone();
        self.invitation = state.invitation.clone();
        self.busy = state.busy;
        self.message = state.message.clone();
        if self.form_open && self.saved != state.saved {
            self.form_open = false;
            self.name.update(cx, |v, cx| v.clear(cx));
            self.peer.update(cx, |v, cx| v.clear(cx));
            self.addr.update(cx, |v, cx| v.clear(cx));
            self.client_grant = false;
        }
        self.saved = state.saved;
        cx.notify();
    }

    fn add_peer(&mut self, _cx: &mut Context<Self>) {
        self.source.dispatch(MeshAction::AddPeer {
            origin: self.peer.read(_cx).value().into(),
            name: self.name.read(_cx).value().into(),
            address: self.addr.read(_cx).value().into(),
            client: self.client_grant,
        });
    }

    fn create_invite(&mut self, _cx: &mut Context<Self>) {
        self.source.dispatch(MeshAction::CreateInvite);
    }

    fn revoke_invite(&mut self, _cx: &mut Context<Self>) {
        self.source.dispatch(MeshAction::RevokeInvite);
    }

    fn remove_peer(&mut self, origin: String, _cx: &mut Context<Self>) {
        self.source.dispatch(MeshAction::RemovePeer(origin));
    }
}
impl MeshSettings {
    pub(super) fn enrollment_data(&self) -> zork_ui::network::EnrollmentData {
        let invitation = self.invitation.as_ref();
        let status = invitation.and_then(|i| i["status"].as_str()).unwrap_or("");
        let remaining = self.source.remaining().unwrap_or_default();
        let label = match status {
            "joined" => format!(
                "{} 已加入，任务协作已开启",
                invitation
                    .and_then(|i| i["device"]["name"].as_str())
                    .unwrap_or("设备")
            ),
            "expired" => "安装链接已过期，请重新生成。".into(),
            "revoked" => "安装链接已撤销。".into(),
            "connecting" => "正在确认设备身份…".into(),
            "waiting" => format!(
                "{} 分 {} 秒内有效",
                remaining / 60,
                remaining % 60
            ),
            _ => String::new(),
        };
        zork_ui::network::EnrollmentData {
            available: self.config.is_some(),
            busy: self.busy,
            status: status.into(),
            command: invitation
                .and_then(|i| i["install_url"].as_str())
                .unwrap_or_default()
                .into(),
            status_label: label,
            notice: self.message.clone(),
        }
    }
    pub(super) fn enrollment_action(
        &mut self,
        action: zork_ui::network::EnrollmentAction,
        cx: &mut Context<Self>,
    ) {
        match action {
            zork_ui::network::EnrollmentAction::Create => self.create_invite(cx),
            zork_ui::network::EnrollmentAction::Revoke => self.revoke_invite(cx),
            zork_ui::network::EnrollmentAction::Copy => {
                if let Some(command) = self
                    .invitation
                    .as_ref()
                    .and_then(|i| i["install_url"].as_str())
                {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(command.into()));
                    self.message = Some("连接邀请已复制。".into());
                    cx.notify();
                }
            }
        }
    }
}
impl Render for MeshSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.modal.sync(
            (self.form_open && !self.enrollment_only).then_some("mesh-peer-dialog"),
            window,
            cx,
        );
        let peer_visible = self
            .modal
            .retain(
                "mesh-peer-dialog",
                (self.form_open && !self.enrollment_only).then_some(()),
                cx,
            )
            .is_some();
        if self.enrollment_only {
            return zork_ui::network::enrollment(self.enrollment_data(), cx, |v, event, cx| {
                v.enrollment_action(event, cx)
            })
            .into_any_element();
        }
        let data = zork_ui::network::NetworkData {
            enabled: self.config.as_ref().is_some_and(|c| c.enabled),
            available: self.config.is_some(),
            busy: self.busy,
            identity: self.origin.clone(),
            notice: if self.form_open {
                None
            } else {
                self.message.clone()
            },
            peers: self
                .config
                .as_ref()
                .map(|c| {
                    c.peers
                        .iter()
                        .map(|peer| zork_ui::network::Peer {
                            id: peer.origin.clone(),
                            name: peer.name.clone(),
                            status: self.source.peer_status(&peer.origin),
                            permission: if peer.collaborate {
                                "同一 mesh · 任务协作已开启"
                            } else if peer.client {
                                "客户端 · 可管理此设备"
                            } else {
                                "设备 · 协作节点"
                            }
                            .into(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        };
        zork_ui::network::page(
            zork_ui::network::Page {
                data,
                invitation: self.enrollment_data(),
                focus: &self.switch_focus[0],
                modal: &self.modal,
                peer: peer_visible.then(|| zork_ui::network::peer::Fields {
                    name: &self.name,
                    origin: &self.peer,
                    address: &self.addr,
                    grant: self.client_grant,
                    focus: &self.switch_focus[1],
                    busy: self.busy,
                    notice: self.message.clone(),
                }),
            },
            window,
            cx,
            |v, event, cx| {
                use zork_ui::network::{peer, NetworkAction, PageAction};
                match event {
                    PageAction::Enrollment(event) => v.enrollment_action(event, cx),
                    PageAction::Network(event) => match event {
                        NetworkAction::Toggle(on) => v.source.dispatch(MeshAction::Enable(on)),
                        NetworkAction::Refresh => v.refresh(cx),
                        NetworkAction::CopyIdentity => {
                            if let Some(origin) = &v.origin {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    origin.clone(),
                                ));
                                v.message = Some("设备身份已复制。".into());
                            }
                        }
                        NetworkAction::Add => {
                            v.form_open = true;
                            v.message = None;
                        }
                        NetworkAction::Remove(id) => v.remove_peer(id, cx),
                    },
                    PageAction::Peer(event) => match event {
                        peer::Action::Grant(on) => v.client_grant = on,
                        peer::Action::Save => v.add_peer(cx),
                        peer::Action::Cancel => {
                            v.form_open = false;
                            v.message = None;
                        }
                    },
                }
                cx.notify();
            },
        )
    }
}
