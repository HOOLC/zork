//! Serialized observation over the same controllers as native UI. Waiting is
//! separate from preparing a batch, and neither requires the command executor.
mod adb;
mod conversation;
mod history;
mod notifications;
mod resources;
mod settings;
mod shared_files;
mod snapshot;
#[cfg(test)]
mod tests;
use crate::{state::Device, store::ClientStore};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use zork_observe::Readiness;

#[derive(Deserialize)]
#[serde(tag = "projection", rename_all = "snake_case", deny_unknown_fields)]
pub enum Key {
    DataReset,
    LocalScripts,
    Adb,
    SharedFiles,
    Notifications,
    Resources {
        peer: Option<String>,
        kind: crate::resources::ResourceKind,
        query: Option<crate::resources::Inspection>,
    },
    Invitation,
    Directory,
    Conversation {
        peer: String,
        session: Option<String>,
    },
    History {
        peer: String,
        session: String,
    },
    Settings {
        peer: String,
    },
}
impl Key {
    pub fn peer(&self) -> &str {
        match self {
            Self::DataReset
            | Self::LocalScripts
            | Self::Adb
            | Self::Invitation
            | Self::Directory
            | Self::Notifications
            | Self::SharedFiles => "",
            Self::Resources { peer, .. } => peer.as_deref().unwrap_or(""),
            Self::Conversation { peer, .. }
            | Self::History { peer, .. }
            | Self::Settings { peer } => peer,
        }
    }
}

/// One waiter for each underlying source. Cancellation does not consume data.
pub struct Signals(Vec<Readiness>);
impl Signals {
    pub async fn changed(&mut self) -> Result<bool, zork_observe::Closed> {
        futures_util::future::poll_fn(|cx| {
            use std::task::Poll;
            let mut changed = false;
            let mut closed = false;
            for signal in &mut self.0 {
                match signal.poll_changed(cx) {
                    Poll::Ready(Ok(())) => changed = true,
                    Poll::Ready(Err(_)) => closed = true,
                    Poll::Pending => {}
                }
            }
            if changed {
                Poll::Ready(Ok(self
                    .0
                    .iter_mut()
                    .fold(false, |urgent, s| s.take_urgent() || urgent)))
            } else if closed {
                Poll::Ready(Err(zork_observe::Closed))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

enum Projection {
    DataReset(snapshot::SnapshotWire<crate::data_reset::Snapshot>),
    LocalScripts(snapshot::SnapshotWire),
    Adb(adb::AdbWire),
    SharedFiles(shared_files::SharedFilesWire),
    Notifications(notifications::NotificationsWire),
    Resources(resources::ResourcesWire),
    Invitation(snapshot::SnapshotWire),
    Directory(snapshot::SnapshotWire),
    Conversation(conversation::ConversationWire),
    History(history::HistoryWire),
    Settings(settings::SettingsWire),
}
pub struct WireSubscription {
    projection: Projection,
    device: Option<Arc<Device>>,
    prepared: Option<(u64, Arc<Value>, bool)>,
    sequence: u64,
    applied: u64,
}
impl WireSubscription {
    pub(crate) fn from_data_reset(
        source: &zork_observe::ValueSource<crate::data_reset::Snapshot>,
    ) -> Self {
        Self {
            projection: Projection::DataReset(snapshot::SnapshotWire::new(source)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    pub(crate) fn from_local_scripts(source: &zork_observe::ValueSource<Value>) -> Self {
        Self {
            projection: Projection::LocalScripts(snapshot::SnapshotWire::new(source)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    pub(crate) fn from_adb(source: Arc<crate::adb::Controller>) -> Self {
        Self {
            projection: Projection::Adb(adb::AdbWire::new(source)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    pub(crate) fn from_shared_files(
        source: Arc<crate::shared_files::SharedFiles>,
        store: Arc<ClientStore>,
    ) -> Self {
        Self {
            projection: Projection::SharedFiles(shared_files::SharedFilesWire::new(source, store)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    pub(crate) fn from_notifications(store: Arc<ClientStore>) -> Result<Self> {
        Ok(Self {
            projection: Projection::Notifications(notifications::NotificationsWire::new(store)?),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        })
    }
    pub(crate) fn from_resources(
        source: Arc<crate::resources::Resources>,
        store: Arc<ClientStore>,
        peer: Option<String>,
        kind: crate::resources::ResourceKind,
        query: Option<crate::resources::Inspection>,
    ) -> Result<Self> {
        Ok(Self {
            projection: Projection::Resources(resources::ResourcesWire::new(
                source, store, peer, kind, query,
            )?),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        })
    }
    pub(crate) fn from_directory(source: &zork_observe::ValueSource<Value>) -> Self {
        Self {
            projection: Projection::Directory(snapshot::SnapshotWire::new(source)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    pub(crate) fn from_invitation(source: &zork_observe::ValueSource<Value>) -> Self {
        Self {
            projection: Projection::Invitation(snapshot::SnapshotWire::new(source)),
            device: None,
            prepared: None,
            sequence: 0,
            applied: 0,
        }
    }
    /// Adapt an already-owned controller. Its lifecycle is independent of this
    /// read-only observer; the caller activates business operations separately.
    pub fn from_device(key: Key, device: Arc<Device>, store: Arc<ClientStore>) -> Result<Self> {
        anyhow::ensure!(
            device.bound_peer().is_none_or(|peer| peer == key.peer()),
            "projection belongs to another device"
        );
        let projection = match key {
            Key::DataReset
            | Key::LocalScripts
            | Key::Adb
            | Key::Invitation
            | Key::Directory
            | Key::Notifications
            | Key::SharedFiles
            | Key::Resources { .. } => {
                anyhow::bail!("projection uses a client-owned source")
            }
            Key::Conversation { peer, session } => Projection::Conversation(
                conversation::ConversationWire::new(peer, session, device.clone(), store)?,
            ),
            Key::History { peer, session } => {
                Projection::History(history::HistoryWire::new(peer, session, device.clone())?)
            }
            Key::Settings { peer } => {
                Projection::Settings(settings::SettingsWire::new(peer, device.clone(), store)?)
            }
        };
        Ok(Self {
            projection,
            device: Some(device),
            prepared: None,
            sequence: 0,
            applied: 0,
        })
    }
    pub fn signals(&self) -> Signals {
        Signals(match &self.projection {
            Projection::DataReset(p) => p.signals(),
            Projection::Adb(p) => p.wire.signals(),
            Projection::SharedFiles(p) => p.signals(),
            Projection::Notifications(p) => p.wire.signals(),
            Projection::Resources(p) => p.signals(),
            Projection::Invitation(p) | Projection::Directory(p) | Projection::LocalScripts(p) => {
                p.signals()
            }
            Projection::Conversation(p) => p.signals(),
            Projection::History(p) => p.signals(),
            Projection::Settings(p) => p.signals(),
        })
    }
    pub fn valid(&self, batch: u64) -> bool {
        self.prepared.as_ref().is_some_and(|(id, _, revoked)| {
            *id == batch
                && (*revoked
                    || self
                        .device
                        .as_ref()
                        .is_none_or(|device| !device.snapshot().revoked))
        }) && match &self.projection {
            Projection::DataReset(p) => p.valid(),
            Projection::Adb(p) => p.valid(),
            Projection::SharedFiles(p) => p.valid(),
            Projection::Notifications(p) => p.valid(),
            Projection::Resources(p) => p.valid(),
            Projection::Invitation(p) | Projection::Directory(p) | Projection::LocalScripts(p) => {
                p.valid()
            }
            Projection::Conversation(p) => p.valid(),
            Projection::History(p) => p.valid(),
            Projection::Settings(p) => p.valid(),
        }
    }
    pub fn prepare(&mut self) -> Result<Option<Arc<Value>>> {
        if let Some((batch, _, _)) = &self.prepared {
            if !self.valid(*batch) {
                self.finish(*batch, false);
            }
        }
        if let Some((_, value, _)) = &self.prepared {
            return Ok(Some(value.clone()));
        }
        let value = match &mut self.projection {
            Projection::DataReset(p) => p.prepare()?,
            Projection::Adb(p) => p.prepare()?,
            Projection::SharedFiles(p) => p.prepare()?,
            Projection::Notifications(p) => p.prepare()?,
            Projection::Resources(p) => p.prepare()?,
            Projection::Invitation(p) | Projection::Directory(p) | Projection::LocalScripts(p) => {
                p.prepare()?
            }
            Projection::Conversation(p) => p.prepare()?,
            Projection::History(p) => p.prepare()?,
            Projection::Settings(p) => p.prepare()?,
        };
        let Some((mut value, reset, revoked)) = value else {
            return Ok(None);
        };
        self.sequence = self
            .sequence
            .checked_add(1)
            .expect("wire sequence exhausted");
        value["batch"] = json!(self.sequence);
        value["from"] = json!(self.applied);
        value["reset"] = json!(reset || self.applied == 0);
        let value = Arc::new(value);
        self.prepared = Some((self.sequence, value.clone(), revoked));
        Ok(Some(value))
    }
    pub fn finish(&mut self, batch: u64, applied: bool) -> bool {
        if self.prepared.as_ref().is_none_or(|(id, _, _)| *id != batch) {
            return false;
        }
        let valid = self.valid(batch);
        let accepted = match &mut self.projection {
            Projection::DataReset(p) => p.finish(applied && valid),
            Projection::Adb(p) => p.wire.finish(applied && valid),
            Projection::Notifications(p) => p.wire.finish(applied && valid),
            Projection::Resources(p) => p.finish(applied && valid),
            Projection::SharedFiles(p) => p.finish(applied && valid),
            Projection::Invitation(p) | Projection::Directory(p) | Projection::LocalScripts(p) => {
                p.finish(applied && valid)
            }
            Projection::Conversation(p) => p.finish(applied && valid),
            Projection::History(p) => p.finish(applied && valid),
            Projection::Settings(p) => p.finish(applied && valid),
        };
        if applied && valid && accepted {
            self.applied = batch;
        }
        self.prepared = None;
        valid && accepted
    }
    /// Explicitly enlarge this observer's history range; other observers keep
    /// their own bounded window. Loading remains a core business operation.
    pub fn older(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.prepared.is_none(),
            "finish the prepared frame before changing its range"
        );
        if let Projection::Conversation(p) = &mut self.projection {
            p.older();
        }
        if let Projection::History(p) = &mut self.projection {
            p.older();
        }
        Ok(())
    }
    pub fn newer(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.prepared.is_none(),
            "finish the prepared frame before changing its range"
        );
        if let Projection::Conversation(p) = &mut self.projection {
            p.newer();
        }
        if let Projection::History(p) = &mut self.projection {
            p.newer();
        }
        Ok(())
    }
    /// Pin the requested range to a business record while reading history;
    /// None selects the latest tail. Scroll and frame clocks stay in the UI.
    pub fn window_anchor(&mut self, anchor: Option<String>) -> Result<()> {
        anyhow::ensure!(
            self.prepared.is_none(),
            "finish the prepared frame before changing its range"
        );
        match &mut self.projection {
            Projection::Conversation(p) => p.window_anchor(anchor)?,
            Projection::History(p) => p.window_anchor(anchor)?,
            _ => {}
        }
        Ok(())
    }

    pub fn detail(&mut self, id: Option<String>) -> Result<()> {
        anyhow::ensure!(
            self.prepared.is_none(),
            "finish the prepared frame before selecting a record"
        );
        if let Projection::History(p) = &mut self.projection {
            p.detail(id)?;
        }
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.prepared.is_none(),
            "finish the prepared frame before refreshing"
        );
        if let Projection::History(p) = &self.projection {
            p.refresh();
        }
        Ok(())
    }
}
