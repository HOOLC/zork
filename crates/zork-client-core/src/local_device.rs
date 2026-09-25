//! Which Mesh device is this machine ("本机"). One rule for every client: the
//! saved node this client runs itself (`SavedNode::local`, the same flag the
//! chat list treats as local) and this client's own Mesh identity. Every other
//! member is another device, however this client reaches it.
use crate::store::SavedNode;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalDevice {
    /// Mesh identity of the Station this client runs itself (desktop).
    pub station: Option<String>,
    /// This client's own Mesh identity: a phone, or the desktop's companion
    /// client next to its Station.
    pub client: Option<String>,
}

impl LocalDevice {
    /// `origins` maps a Mesh origin to the saved node id it belongs to.
    pub fn new(
        nodes: &[SavedNode],
        origins: &HashMap<String, String>,
        client: Option<&str>,
    ) -> Self {
        let station = origins
            .iter()
            .filter(|(_, id)| nodes.iter().any(|node| &node.id == *id && node.local))
            .map(|(origin, _)| origin.clone())
            .min();
        Self {
            station,
            client: client
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned),
        }
    }

    /// Whether the Mesh member `origin` is this machine.
    pub fn is_local(&self, origin: &str) -> bool {
        !origin.is_empty()
            && (self.station.as_deref() == Some(origin) || self.client.as_deref() == Some(origin))
    }

    /// The member shown as this machine's own row: its Station when it runs
    /// one, else the client itself (a phone has no Station).
    pub fn primary(&self) -> Option<&str> {
        self.station.as_deref().or(self.client.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, local: bool) -> SavedNode {
        SavedNode {
            id: id.into(),
            name: id.into(),
            url: String::new(),
            token: None,
            local,
            mesh: None,
            group: None,
            machine_name: None,
            color_key: None,
        }
    }

    #[test]
    fn the_local_station_and_this_client_are_this_machine() {
        let nodes = [node("desktop", true), node("mini", false)];
        let origins = HashMap::from([
            ("key:desktop".to_owned(), "desktop".to_owned()),
            ("key:mini".to_owned(), "mini".to_owned()),
        ]);
        let local = LocalDevice::new(&nodes, &origins, Some("key:companion"));
        assert!(local.is_local("key:desktop"));
        assert!(local.is_local("key:companion"));
        assert!(!local.is_local("key:mini"));
        assert!(!local.is_local(""));
        assert_eq!(local.primary(), Some("key:desktop"));
    }

    #[test]
    fn a_phone_is_its_own_client_identity() {
        let nodes = [node("mini", false)];
        let origins = HashMap::from([("key:mini".to_owned(), "mini".to_owned())]);
        let local = LocalDevice::new(&nodes, &origins, Some("key:phone"));
        assert_eq!(local.station, None);
        assert!(local.is_local("key:phone"));
        assert!(!local.is_local("key:mini"));
        assert_eq!(local.primary(), Some("key:phone"));
        assert_eq!(LocalDevice::new(&nodes, &origins, Some("")).primary(), None);
    }

    #[test]
    fn a_local_node_without_a_known_origin_is_not_guessed() {
        let nodes = [node("desktop", true)];
        let local = LocalDevice::new(&nodes, &HashMap::new(), None);
        assert_eq!(local, LocalDevice::default());
        assert!(!local.is_local("key:desktop"));
    }
}
