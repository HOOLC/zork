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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<MeshRoutes>,
}

/// Observed transport hints, bound to the enclosing device's authenticated key.
/// These are refreshed by the endpoint; they never select a local NIC or bind.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshRoutes {
    pub direct: Vec<std::net::SocketAddr>,
    pub relays: Vec<String>,
}
impl MeshRoutes {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.direct.len() + self.relays.len() <= 24,
            "too_many_peer_addresses"
        );
        for addr in &self.direct {
            ensure!(
                addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast(),
                "invalid_device_address"
            );
        }
        for relay in &self.relays {
            crate::services::validate_endpoint(relay)?;
        }
        Ok(())
    }
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

/// Binds a leave/switch confirmation to the membership the user reviewed.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeshVersion {
    pub authority: String,
    pub revision: u64,
}
impl MeshVersion {
    pub fn of(group: &MeshGroup) -> Self {
        Self {
            authority: group.authority.clone(),
            revision: group.revision,
        }
    }
    pub fn matches(&self, group: &MeshGroup) -> bool {
        self.authority == group.authority && self.revision == group.revision
    }
}

/// Detaches this device's grants; other members and all local business data stay.
/// The caller preserves the previous directory before committing the change.
pub fn detach(
    config: &mut MeshConfig,
    own_origin: &str,
    expected: &MeshVersion,
) -> Result<MeshGroup> {
    let group = config
        .group
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("mesh_membership_missing"))?;
    ensure!(
        expected.matches(group),
        "mesh_membership_changed_refresh_required"
    );
    ensure!(
        group.authority != own_origin || (group.members.len() == 1 && group.clients.is_empty()),
        "当前设备管理这个 Mesh，请先移交管理职责或移除其它成员"
    );
    let group = group.clone();
    let members: HashSet<_> = group
        .members
        .iter()
        .chain(&group.clients)
        .map(|member| member.origin.as_str())
        .collect();
    config
        .peers
        .retain(|peer| !members.contains(peer.origin.as_str()));
    config.group = None;
    Ok(group)
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
        if let Some(routes) = &self.routes {
            routes.validate()?;
        }
        Ok(())
    }
}

pub fn validate_device_name(name: &str) -> Result<String> {
    zork_client_types::device::validate_name(name).map_err(anyhow::Error::msg)
}

/// Mesh-wide display names, kept by the membership authority beside the
/// group. They are versioned on their own and travel as a sibling of
/// `MeshGroup` (never inside it): older peers parse the group with
/// `deny_unknown_fields`, ignore the sibling and keep working.
///
/// Each named device holds a join sequence number. The default name is the
/// letter for that number (A, B, …, Z, AA, AB, …); `next` only grows, so a
/// letter is never handed out again while this directory exists, and letters
/// never reshuffle when devices leave.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeshNames {
    pub authority: String,
    pub revision: u64,
    #[serde(default)]
    pub next: u64,
    #[serde(default)]
    pub devices: Vec<DisplayName>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplayName {
    pub origin: String,
    /// Join order within this Mesh.
    pub seq: u64,
    pub name: String,
}

/// A device as every surface shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResolvedName {
    pub origin: String,
    pub display: String,
    /// The name the device registered with (`MeshDevice.name`).
    pub machine: String,
    pub station: bool,
    /// Join sequence in this Mesh; `None` for a desktop companion client.
    pub seq: Option<u64>,
}
impl ResolvedName {
    /// The stable key a device's colour follows: its join order when it has
    /// one, so consecutive devices get distinct hues, else its identity.
    /// Renaming never changes it.
    pub fn color_key(&self) -> String {
        match self.seq {
            Some(seq) => format!("seq:{seq}"),
            None => self.origin.clone(),
        }
    }
}

/// Default display name for join sequence `seq`: A…Z, then AA, AB, …
pub fn letter(seq: u64) -> String {
    let mut n = seq + 1;
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii letters")
}

/// The desktop client registers itself next to its own Station as
/// "<host> 客户端". It is part of that computer, not another device, so it
/// takes no letter of its own.
pub const DESKTOP_CLIENT_SUFFIX: &str = " 客户端";
fn companion(device: &MeshDevice) -> bool {
    device.name.ends_with(DESKTOP_CLIENT_SUFFIX)
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl MeshNames {
    pub fn new(authority: &str) -> Self {
        Self {
            authority: authority.into(),
            ..Default::default()
        }
    }

    /// Devices that carry a display name, in join order: Stations in the order
    /// the authority admitted them (the authority first, as it created the
    /// Mesh), then access clients such as phones. This order only matters the
    /// first time a device is named; afterwards its `seq` is fixed.
    pub fn named_devices(group: &MeshGroup) -> impl Iterator<Item = &MeshDevice> {
        let authority = group.members.iter().filter(|m| m.origin == group.authority);
        let members = group.members.iter().filter(|m| m.origin != group.authority);
        authority
            .chain(members)
            .chain(group.clients.iter().filter(|c| !companion(c)))
    }

    pub fn get(&self, origin: &str) -> Option<&DisplayName> {
        self.devices.iter().find(|d| d.origin == origin)
    }

    fn taken(&self, name: &str, except: Option<&str>) -> Option<&DisplayName> {
        self.devices
            .iter()
            .find(|d| Some(d.origin.as_str()) != except && same_name(&d.name, name))
    }

    /// Brings the directory in line with `group`: names new devices with the
    /// next free letter and forgets devices that left. Deterministic for the
    /// same inputs, so a client that has not yet received the authority's
    /// result derives the same letters. Returns whether anything changed; the
    /// authority then bumps `revision` and persists.
    pub fn reconcile(&mut self, group: &MeshGroup) -> bool {
        if self.authority != group.authority {
            *self = Self::new(&group.authority);
        }
        let named: Vec<_> = Self::named_devices(group).collect();
        let before = self.devices.len();
        self.devices
            .retain(|d| named.iter().any(|device| device.origin == d.origin));
        let mut changed = self.devices.len() != before;
        for device in named {
            if self.get(&device.origin).is_some() {
                continue;
            }
            while self.taken(&letter(self.next), None).is_some() {
                self.next += 1;
            }
            self.devices.push(DisplayName {
                origin: device.origin.clone(),
                seq: self.next,
                name: letter(self.next),
            });
            self.next += 1;
            changed = true;
        }
        if changed {
            self.devices.sort_by_key(|d| d.seq);
            self.revision += 1;
        }
        changed
    }

    /// Renames one device. The authority applies renames one at a time, so
    /// concurrent requests resolve in its order and the last one wins; every
    /// peer converges on the highest revision.
    pub fn rename(&mut self, group: &MeshGroup, origin: &str, name: &str) -> Result<bool> {
        let name =
            zork_client_types::device::validate_display_name(name).map_err(anyhow::Error::msg)?;
        self.reconcile(group);
        ensure!(self.get(origin).is_some(), "mesh_device_not_found");
        if self.taken(&name, Some(origin)).is_some() {
            anyhow::bail!("名称“{name}”已被 Mesh 中的另一台设备使用，请换一个名称");
        }
        let entry = self
            .devices
            .iter_mut()
            .find(|d| d.origin == origin)
            .expect("checked above");
        if entry.name == name {
            return Ok(false);
        }
        entry.name = name;
        self.revision += 1;
        Ok(true)
    }

    /// Whether `self` should replace the stored directory `current`.
    pub fn supersedes(&self, current: Option<&MeshNames>) -> bool {
        match current {
            None => true,
            Some(current) => {
                current.authority != self.authority || self.revision > current.revision
            }
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            valid_origin(&self.authority) && self.devices.len() <= 64,
            "invalid_mesh_names"
        );
        for device in &self.devices {
            ensure!(valid_origin(&device.origin), "invalid_device_identity");
            zork_client_types::device::validate_display_name(&device.name)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}

/// Every device of `group` with its display and machine names. `names` is the
/// authority's directory if known; devices it does not cover yet get the same
/// letters the authority will assign. A desktop companion client keeps its
/// machine name.
pub fn resolve(group: &MeshGroup, names: Option<&MeshNames>) -> Vec<ResolvedName> {
    let mut names = names
        .filter(|n| n.authority == group.authority)
        .cloned()
        .unwrap_or_else(|| MeshNames::new(&group.authority));
    names.reconcile(group);
    group
        .members
        .iter()
        .map(|m| (m, true))
        .chain(group.clients.iter().map(|c| (c, false)))
        .map(|(device, station)| ResolvedName {
            origin: device.origin.clone(),
            display: names
                .get(&device.origin)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| device.name.clone()),
            machine: device.name.clone(),
            station,
            seq: names.get(&device.origin).map(|d| d.seq),
        })
        .collect()
}

/// Finds one device by identity, display name or machine name (ignoring case
/// and surrounding spaces). Display names are unique within a Mesh and win
/// over machine names; an ambiguous machine name is an error.
pub fn find_device<'a>(devices: &'a [ResolvedName], query: &str) -> Result<&'a ResolvedName> {
    let query = query.trim();
    if let Some(device) = devices.iter().find(|d| d.origin == query) {
        return Ok(device);
    }
    let display: Vec<_> = devices
        .iter()
        .filter(|d| same_name(&d.display, query))
        .collect();
    if let [device] = display.as_slice() {
        return Ok(device);
    }
    let machine: Vec<_> = devices
        .iter()
        .filter(|d| same_name(&d.machine, query))
        .collect();
    match machine.as_slice() {
        [device] => Ok(device),
        [] => anyhow::bail!("mesh_device_not_found"),
        _ => anyhow::bail!(
            "有多个设备名为 {query}，请改用显示名称或设备身份：{}",
            machine
                .iter()
                .map(|d| format!("{}（{}）", d.display, d.origin))
                .collect::<Vec<_>>()
                .join("、")
        ),
    }
}

/// Where a Station keeps the Mesh display names: next to `config.json`,
/// outside `MeshConfig`, which older binaries parse strictly.
pub fn names_path(data_root: &std::path::Path) -> std::path::PathBuf {
    data_root.join("mesh").join("names.json")
}

pub fn load_names(data_root: &std::path::Path) -> Result<Option<MeshNames>> {
    match std::fs::read(names_path(data_root)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Atomically replaces the stored directory. Callers serialize writers.
pub fn save_names(data_root: &std::path::Path, names: &MeshNames) -> Result<()> {
    let path = names_path(data_root);
    let parent = path.parent().expect("names directory");
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec_pretty(names)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
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
                    routes: device.routes.clone(),
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
                    routes: device.routes.clone(),
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
            routes: None,
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
            routes: None,
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

    fn named(c: char, name: &str) -> MeshDevice {
        MeshDevice {
            name: name.into(),
            ..device(c)
        }
    }
    fn studio_mesh() -> MeshGroup {
        // The user's Mesh: the MacBook Air created it, the Studio joined, the
        // Air's desktop client and the phone connect as clients.
        MeshGroup {
            authority: device('y').origin,
            revision: 4,
            members: vec![
                named('y', "MacBook-Air"),
                named('b', "zuozijians-Mac-Studio"),
            ],
            clients: vec![
                named('n', "zuozijiandeMacBook-Air 客户端"),
                named('d', "PLP110"),
            ],
        }
    }
    fn display(names: &MeshNames, c: char) -> Option<String> {
        names.get(&device(c).origin).map(|d| d.name.clone())
    }

    #[test]
    fn letters_run_a_to_z_then_double_letters() {
        assert_eq!(letter(0), "A");
        assert_eq!(letter(1), "B");
        assert_eq!(letter(25), "Z");
        assert_eq!(letter(26), "AA");
        assert_eq!(letter(27), "AB");
        assert_eq!(letter(51), "AZ");
        assert_eq!(letter(52), "BA");
        assert_eq!(letter(26 + 26 * 26), "AAA");
    }

    #[test]
    fn existing_mesh_is_backfilled_in_join_order() {
        let group = studio_mesh();
        let mut names = MeshNames::new(&group.authority);
        assert!(names.reconcile(&group));
        assert_eq!(names.revision, 1);
        assert_eq!(display(&names, 'y').as_deref(), Some("A"));
        assert_eq!(display(&names, 'b').as_deref(), Some("B"));
        assert_eq!(display(&names, 'd').as_deref(), Some("C"));
        // The Air's desktop client is part of the Air, not a fourth device.
        assert_eq!(display(&names, 'n'), None);
        assert_eq!(names.next, 3);
        // Backfill is idempotent and deterministic.
        assert!(!names.reconcile(&group));
        let mut again = MeshNames::new(&group.authority);
        again.reconcile(&group);
        assert_eq!(again, names);
        let resolved = resolve(&group, None);
        let air = resolved
            .iter()
            .find(|d| d.origin == device('y').origin)
            .unwrap();
        assert_eq!(
            (air.display.as_str(), air.machine.as_str()),
            ("A", "MacBook-Air")
        );
        let client = resolved
            .iter()
            .find(|d| d.origin == device('n').origin)
            .unwrap();
        assert_eq!(client.display, "zuozijiandeMacBook-Air 客户端");
    }

    #[test]
    fn letters_are_never_reused_and_do_not_reshuffle() {
        let mut group = studio_mesh();
        let mut names = MeshNames::new(&group.authority);
        names.reconcile(&group);
        // The Studio (B) leaves: the phone keeps C, B is not handed out again.
        group.members.retain(|m| m.origin != device('b').origin);
        assert!(names.reconcile(&group));
        assert_eq!(display(&names, 'd').as_deref(), Some("C"));
        group.members.push(named('e', "mini1"));
        names.reconcile(&group);
        assert_eq!(display(&names, 'e').as_deref(), Some("D"));
        // A name the user chose is skipped when it equals the next letter.
        names.rename(&group, &device('d').origin, "E").unwrap();
        group.clients.push(named('j', "Pixel"));
        names.reconcile(&group);
        assert_eq!(display(&names, 'j').as_deref(), Some("F"));
        // Past Z the letters continue with AA.
        let mut long = MeshNames::new(&group.authority);
        long.next = 25;
        long.reconcile(&group);
        let letters: Vec<_> = long.devices.iter().map(|d| d.name.clone()).collect();
        assert_eq!(letters[..3], ["Z".to_string(), "AA".into(), "AB".into()]);
    }

    #[test]
    fn rename_validates_and_rejects_duplicates() {
        let group = studio_mesh();
        let mut names = MeshNames::new(&group.authority);
        names.reconcile(&group);
        let studio = device('b').origin;
        assert!(names.rename(&group, &studio, "  工作室  ").unwrap());
        assert_eq!(display(&names, 'b').as_deref(), Some("工作室"));
        assert!(!names.rename(&group, &studio, "工作室").unwrap());
        let error = names.rename(&group, &studio, "a").unwrap_err().to_string();
        assert!(error.contains("已被 Mesh 中的另一台设备使用"), "{error}");
        assert_eq!(
            names
                .rename(&group, &studio, "   ")
                .unwrap_err()
                .to_string(),
            "请输入显示名称"
        );
        assert!(names
            .rename(&group, &studio, &"长".repeat(25))
            .unwrap_err()
            .to_string()
            .contains("不能超过 24 个字符"));
        assert!(names.rename(&group, &studio, &"长".repeat(24)).is_ok());
        assert!(names.rename(&group, &device('n').origin, "X").is_err());
        assert!(names.validate().is_ok());
    }

    #[test]
    fn concurrent_renames_converge_on_the_authority_order() {
        let group = studio_mesh();
        let mut authority = MeshNames::new(&group.authority);
        authority.reconcile(&group);
        let studio = device('b').origin;
        // Two members ask at the same time; the authority applies them in turn.
        authority.rename(&group, &studio, "Studio").unwrap();
        let first = authority.clone();
        authority.rename(&group, &studio, "工作室").unwrap();
        let second = authority.clone();
        // Peers receive the results in either order and keep the later one.
        for arrivals in [[&first, &second], [&second, &first]] {
            let mut stored: Option<MeshNames> = None;
            for incoming in arrivals {
                if incoming.supersedes(stored.as_ref()) {
                    stored = Some(incoming.clone());
                }
            }
            assert_eq!(stored.as_ref(), Some(&second));
            assert_eq!(
                display(stored.as_ref().unwrap(), 'b').as_deref(),
                Some("工作室")
            );
        }
        // A directory from another Mesh replaces an old one regardless of revision.
        let other = MeshNames::new(&device('j').origin);
        assert!(other.supersedes(Some(&second)));
    }

    #[test]
    fn devices_resolve_by_display_or_machine_name() {
        let group = studio_mesh();
        let mut names = MeshNames::new(&group.authority);
        names.reconcile(&group);
        names.rename(&group, &device('b').origin, "Studio").unwrap();
        let devices = resolve(&group, Some(&names));
        let find = |q: &str| find_device(&devices, q).map(|d| d.origin.clone());
        assert_eq!(find("studio").unwrap(), device('b').origin);
        assert_eq!(find("zuozijians-Mac-Studio").unwrap(), device('b').origin);
        assert_eq!(find(" a ").unwrap(), device('y').origin);
        assert_eq!(find("macbook-air").unwrap(), device('y').origin);
        assert_eq!(find(&device('d').origin).unwrap(), device('d').origin);
        assert_eq!(find("PLP110").unwrap(), device('d').origin);
        assert!(find("nothing").is_err());
        // Display names win over another device's machine name.
        let mut clash = group.clone();
        clash.members[1].name = "C".into();
        let devices = resolve(&clash, Some(&names));
        assert_eq!(
            find_device(&devices, "C").unwrap().origin,
            device('d').origin
        );
        // Machine names can repeat; then the query is ambiguous.
        let mut twins = group.clone();
        twins.members[1].name = "PLP110".into();
        let devices = resolve(&twins, Some(&names));
        assert!(find_device(&devices, "plp110")
            .unwrap_err()
            .to_string()
            .contains("有多个设备"));
    }

    #[test]
    fn names_are_stored_outside_the_strict_config() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_names(dir.path()).unwrap(), None);
        let group = studio_mesh();
        let mut names = MeshNames::new(&group.authority);
        names.reconcile(&group);
        save_names(dir.path(), &names).unwrap();
        assert_eq!(load_names(dir.path()).unwrap(), Some(names.clone()));
        // Older peers parse MeshGroup strictly; the names never enter it.
        let wire = serde_json::json!({"group": group, "names": names});
        let old: MeshGroup = serde_json::from_value(wire["group"].clone()).unwrap();
        assert_eq!(old, group);
        // A newer directory with fields this build does not know still loads.
        let mut future = serde_json::to_value(&names).unwrap();
        future["epoch"] = serde_json::json!(2);
        future["devices"][0]["icon"] = serde_json::json!("laptop");
        assert_eq!(serde_json::from_value::<MeshNames>(future).unwrap(), names);
    }
}
