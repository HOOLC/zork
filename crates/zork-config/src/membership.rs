//! Versioned membership for a user's own devices. Manual peers retain their grants.
use crate::{MeshConfig, MeshPeer};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshDevice {
    pub origin: String,
    pub name: String,
    pub addr: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshGroup {
    pub authority: String,
    pub revision: u64,
    pub members: Vec<MeshDevice>,
    #[serde(default)]
    pub clients: Vec<MeshDevice>,
}

pub fn valid_origin(origin: &str) -> bool {
    origin.starts_with("key:")
        && origin.len() == 56
        && origin[4..]
            .bytes()
            .all(|b| b"ybndrfg8ejkmcpqxot1uwisza345h769".contains(&b))
}

impl MeshDevice {
    pub fn validate(&self) -> Result<()> {
        ensure!(valid_origin(&self.origin), "invalid_device_identity");
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 128
                && !self.name.chars().any(char::is_control),
            "invalid_device_name"
        );
        if let Some(addr) = &self.addr {
            let addr: std::net::SocketAddr = addr.parse()?;
            ensure!(
                addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast(),
                "invalid_device_address"
            );
        }
        Ok(())
    }
}

pub fn validate_device_name(name: &str) -> Result<String> {
    zork_client_types::device::validate_name(name).map_err(anyhow::Error::msg)
}

impl MeshGroup {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_origin(&self.authority) && self.revision > 0,
            "invalid_mesh_membership"
        );
        ensure!(
            !self.members.is_empty() && self.members.len() <= 16 && self.clients.len() <= 16,
            "mesh_device_limit"
        );
        let mut seen = HashSet::new();
        for member in &self.members {
            member.validate()?;
            ensure!(seen.insert(&member.origin), "duplicate_mesh_device");
        }
        ensure!(seen.contains(&self.authority), "mesh_authority_missing");
        for client in &self.clients {
            client.validate()?;
            ensure!(seen.insert(&client.origin), "duplicate_mesh_device");
        }
        Ok(())
    }

    pub fn contains(&self, origin: &str) -> bool {
        self.members.iter().any(|m| m.origin == origin)
    }

    /// Caller authenticates the authority (or a local invite redemption) first.
    pub fn apply(&self, own_origin: &str, config: &mut MeshConfig) -> Result<bool> {
        self.validate()?;
        if let Some(previous) = &config.group {
            ensure!(
                previous.authority == self.authority,
                "already_in_another_mesh"
            );
            if self.revision < previous.revision {
                return Ok(false);
            }
            if self.revision == previous.revision {
                ensure!(self == previous, "mesh_revision_conflict");
                return Ok(false);
            }
        }
        let previous: HashSet<_> = config
            .group
            .as_ref()
            .into_iter()
            .flat_map(|g| g.members.iter().chain(&g.clients).map(|m| m.origin.clone()))
            .collect();
        config.peers.retain(|p| !previous.contains(&p.origin));
        if self.contains(own_origin) {
            for device in self.members.iter().filter(|m| m.origin != own_origin) {
                // Joining one's own mesh explicitly replaces any narrower manual grant.
                config.peers.retain(|p| p.origin != device.origin);
                config.peers.push(MeshPeer {
                    origin: device.origin.clone(),
                    name: device.name.clone(),
                    addr: device.addr.clone(),
                    execute: vec![],
                    client: true,
                    collaborate: true,
                });
            }
            for device in &self.clients {
                config.peers.retain(|p| p.origin != device.origin);
                config.peers.push(MeshPeer {
                    origin: device.origin.clone(),
                    name: device.name.clone(),
                    addr: device.addr.clone(),
                    execute: vec![],
                    client: true,
                    collaborate: false,
                });
            }
        }
        ensure!(config.peers.len() <= 32, "mesh_peer_limit");
        if let Some(device) = self.members.iter().find(|m| m.origin == own_origin) {
            config.name = device.name.clone();
        }
        config.group = Some(self.clone());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(c: char) -> MeshDevice {
        MeshDevice {
            origin: format!("key:{}", c.to_string().repeat(52)),
            name: c.to_string(),
            addr: None,
        }
    }
    #[test]
    fn renamed_membership_updates_own_name_and_ignores_older_snapshots() {
        let a = device('y');
        let mut b = device('b');
        let initial = MeshGroup {
            authority: a.origin.clone(),
            revision: 1,
            members: vec![a, b.clone()],
            clients: vec![],
        };
        let mut config = MeshConfig::default();
        initial.apply(&b.origin, &mut config).unwrap();
        let mut renamed = initial.clone();
        renamed.revision = 2;
        b.name = "小熊工作室".into();
        renamed.members[1] = b.clone();
        renamed.apply(&b.origin, &mut config).unwrap();
        assert_eq!(config.name, b.name);
        assert!(!initial.apply(&b.origin, &mut config).unwrap());
        assert_eq!(config.name, b.name);
        assert_eq!(validate_device_name("  工作室  ").unwrap(), "工作室");
        for name in ["", "  ", "a\nb"] {
            assert!(validate_device_name(name).is_err());
        }
        assert!(validate_device_name(&"熊".repeat(65)).is_err());
        assert!(validate_device_name(&"熊".repeat(64)).is_ok());
    }
    #[test]
    fn membership_converges_revokes_and_never_widens_manual_peers() {
        let (a, b, c) = (device('y'), device('b'), device('n'));
        let mut config = MeshConfig::default();
        config.peers.push(MeshPeer {
            origin: c.origin.clone(),
            name: c.name.clone(),
            addr: None,
            execute: vec![],
            client: true,
            collaborate: false,
        });
        let mut group = MeshGroup {
            authority: a.origin.clone(),
            revision: 1,
            members: vec![a.clone(), b.clone()],
            clients: vec![],
        };
        assert!(group.apply(&a.origin, &mut config).unwrap());
        assert!(!group.apply(&a.origin, &mut config).unwrap());
        assert!(
            config
                .peers
                .iter()
                .find(|p| p.origin == b.origin)
                .unwrap()
                .collaborate
        );
        assert!(
            !config
                .peers
                .iter()
                .find(|p| p.origin == c.origin)
                .unwrap()
                .collaborate
        );
        let stale = group.clone();
        group.revision = 2;
        group.members.pop();
        group.apply(&a.origin, &mut config).unwrap();
        assert!(!stale.apply(&a.origin, &mut config).unwrap());
        assert!(!config.peers.iter().any(|p| p.origin == b.origin));
        let mut conflict = group.clone();
        conflict.members.push(b);
        assert!(conflict.apply(&a.origin, &mut config).is_err());
    }
}
