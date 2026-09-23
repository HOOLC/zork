//! Shared reconnecting, authenticated feeds for Mesh business subscriptions.
//! A caller updates the resume request only after committing the received data.
use crate::node::MeshNode;
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, task::JoinHandle};
use zork_notify::retry::Retry;

pub const HEARTBEAT: Duration = Duration::from_secs(10);
const IDLE_TIMEOUT: Duration = Duration::from_secs(35);

/// Read-only subscription requests. Business commands cannot enter the
/// automatic reconnect path, and acknowledgement cannot change its scope.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Watch {
    Peer,
    Membership,
    /// One stream multiplexes all channel messages addressed to the authenticated
    /// receiving node. The publisher's epoch and the committed cursor are durable.
    AgentMessages {
        epoch: Option<String>,
        after: i64,
    },
    Assignment {
        assignment_id: String,
        after: i64,
    },
}
impl std::fmt::Debug for Watch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer => f.write_str("Peer"),
            Self::Membership => f.write_str("Membership"),
            Self::AgentMessages { epoch, after } => f
                .debug_struct("AgentMessages")
                .field("epoch", epoch)
                .field("after", after)
                .finish(),
            Self::Assignment {
                assignment_id,
                after,
            } => f
                .debug_struct("Assignment")
                .field("id", assignment_id)
                .field("after", after)
                .finish(),
        }
    }
}
impl Watch {
    fn wire(&self) -> Value {
        use serde_json::json;
        json!({"v":1,"request":{"kind":"watch","topic":self}})
    }
    fn resume(&mut self, next: Self) -> Result<()> {
        let same = match (&*self, &next) {
            (Self::Peer, Self::Peer) | (Self::Membership, Self::Membership) => true,
            (
                Self::AgentMessages { epoch, after },
                Self::AgentMessages {
                    epoch: next_epoch,
                    after: cursor,
                },
            ) => {
                *cursor >= 0
                    && cursor >= after
                    && next_epoch.as_ref().is_some_and(|next| !next.is_empty())
                    && epoch
                        .as_ref()
                        .is_none_or(|old| Some(old) == next_epoch.as_ref())
            }
            (
                Self::Assignment {
                    assignment_id,
                    after,
                },
                Self::Assignment {
                    assignment_id: id,
                    after: cursor,
                },
            ) => assignment_id == id && *cursor >= 0 && cursor >= after,
            _ => false,
        };
        ensure!(same, "mesh_subscription_scope_or_cursor_changed");
        *self = next;
        Ok(())
    }
}

pub enum Event {
    Data(Value),
    Disconnected { error: String, terminal: bool },
}

pub struct Feed {
    receiver: mpsc::Receiver<Event>,
    request: Arc<Mutex<Watch>>,
    task: JoinHandle<()>,
}
impl Drop for Feed {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Feed {
    pub async fn next(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
    /// No reconnect is triggered by acknowledgement; the next attempt uses
    /// this committed cursor. A lost connection never replays business commands.
    pub fn resume_with(&self, request: Watch) -> Result<()> {
        self.request
            .lock()
            .expect("Mesh subscription request")
            .resume(request)
    }
}
impl MeshNode {
    pub fn follow(&self, origin: String, request: Watch) -> Feed {
        let node = self.clone();
        let request = Arc::new(Mutex::new(request));
        let latest = request.clone();
        let (tx, receiver) = mpsc::channel(8);
        let task = tokio::spawn(async move {
            let mut retry = Retry::default();
            loop {
                let request = latest.lock().expect("Mesh subscription request").wire();
                let result = tokio::time::timeout(
                    Duration::from_secs(15),
                    node.subscribe(&origin, &request),
                )
                .await;
                let (error, terminal) = match result {
                    Ok(Ok(mut source)) => loop {
                        let value = tokio::select! {
                            _ = tx.closed() => return,
                            value = tokio::time::timeout(IDLE_TIMEOUT, source.next()) => value,
                        };
                        match value {
                            Ok(Ok(Some(value))) if value["ok"] == true => {
                                if value["heartbeat"] == true {
                                    continue;
                                }
                                retry.reset();
                                if tx.send(Event::Data(value["data"].clone())).await.is_err() {
                                    return;
                                }
                            }
                            Ok(Ok(Some(value))) => {
                                let error = value["error"]
                                    .as_str()
                                    .unwrap_or("mesh_subscription_rejected");
                                let terminal = matches!(
                                    error,
                                    "mesh_protocol_version"
                                        | "chat_epoch_changed"
                                        | "chat_access_denied"
                                        | "invalid_mesh_cursor"
                                        | "mesh_assignment_unauthorized"
                                        | "invite_expired"
                                        | "invite_expired_or_restart"
                                        | "invite_authority_changed"
                                        | "invite_not_claimed"
                                        | "invalid_invitation_kind"
                                        | "invalid_or_revoked_invite"
                                        | "invite_device_proof_mismatch"
                                        | "device_removed_from_mesh"
                                );
                                break (error.to_owned(), terminal);
                            }
                            Ok(Ok(None)) => break ("mesh_subscription_closed".into(), false),
                            Ok(Err(error)) => break (error.to_string(), false),
                            Err(_) => break ("mesh_subscription_timeout".into(), false),
                        }
                    },
                    Ok(Err(error)) => (error.to_string(), false),
                    Err(_) => ("mesh_subscription_connect_timeout".into(), false),
                };
                if tx
                    .send(Event::Disconnected { error, terminal })
                    .await
                    .is_err()
                    || terminal
                {
                    return;
                }
                tokio::select! {
                    _ = tx.closed() => return,
                    _ = retry.wait() => {}
                }
            }
        });
        Feed {
            receiver,
            request,
            task,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_requests_cannot_execute_commands_or_retarget_acknowledgements() {
        for kind in ["delegate", "cancel", "client", "node_tool"] {
            assert!(serde_json::from_value::<Watch>(serde_json::json!({"type":kind})).is_err());
        }
        let request = |id: &str, after| Watch::Assignment {
            assignment_id: id.into(),
            after,
        };
        let mut current = request("one", 10);
        assert!(current.resume(request("two", 20)).is_err());
        assert!(current.resume(request("one", 9)).is_err());
        assert!(current.resume(Watch::Membership).is_err());
        assert_eq!(current, request("one", 10));
        current.resume(request("one", 11)).unwrap();
        assert_eq!(current.wire()["request"]["topic"]["after"], 11);
    }

    #[test]
    fn agent_message_resume_keeps_publisher_epoch_and_forward_cursor() {
        let mut watch = Watch::AgentMessages {
            epoch: None,
            after: 0,
        };
        watch
            .resume(Watch::AgentMessages {
                epoch: Some("publisher".into()),
                after: 10,
            })
            .unwrap();
        assert!(watch
            .resume(Watch::AgentMessages {
                epoch: Some("publisher".into()),
                after: 9
            })
            .is_err());
        assert!(watch
            .resume(Watch::AgentMessages {
                epoch: Some("other".into()),
                after: 11
            })
            .is_err());
        assert!(watch.resume(Watch::Peer).is_err());
        assert!(serde_json::from_value::<Watch>(serde_json::json!({"type":"agent_messages","after":0,"epoch":null,"agent_id":"somebody"})).is_err());
        watch
            .resume(Watch::AgentMessages {
                epoch: Some("publisher".into()),
                after: 11,
            })
            .unwrap();
        assert_eq!(watch.wire()["request"]["topic"]["after"], 11);
    }
}
