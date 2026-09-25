//! One device-status projection; views never infer Mesh readiness from peer errors.
//!
//! The status answers one question: can this client reach that station? Every
//! surface on every platform reads the value projected here from the device's
//! own connection lifecycle. Node-to-node Mesh membership connectivity is a
//! different fact and never feeds this projection.
use crate::api::ConnectionRoute;
pub use zork_client_types::device::{DeviceStatus, MeshReadiness};

/// The lifecycle of this client's own event connection to one station.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Link {
    /// No connection is open or being attempted (never started, or stopped).
    #[default]
    Idle,
    /// The first attempt of a started connection has not finished yet.
    Connecting,
    /// The event stream is open: the station is reachable right now.
    Connected,
    /// The last attempt failed or the stream dropped; reconnects are pending.
    Lost,
}

pub fn project(
    mesh: Option<&MeshReadiness>,
    link: Link,
    online: Option<bool>,
    route: &ConnectionRoute,
    revoked: bool,
) -> DeviceStatus {
    if revoked {
        return DeviceStatus::Revoked;
    }
    match mesh {
        Some(MeshReadiness::NotStarted) => return DeviceStatus::MeshNotStarted,
        Some(MeshReadiness::Preparing) => return DeviceStatus::MeshPreparing,
        Some(MeshReadiness::Stopping) => return DeviceStatus::MeshStopping,
        Some(MeshReadiness::Stopped) => return DeviceStatus::MeshStopped,
        Some(MeshReadiness::Failed(error)) => return DeviceStatus::MeshFailed(error.clone()),
        Some(MeshReadiness::Ready) | None => {}
    }
    let reachable = match (link, online) {
        // An open stream proves reachability even while a data read fails.
        (Link::Connected, _) => true,
        (Link::Lost, _) | (_, Some(false)) => false,
        (_, Some(true)) => true,
        (Link::Connecting, None) => return DeviceStatus::Connecting,
        (Link::Idle, None) => return DeviceStatus::NotConnected,
    };
    if !reachable {
        DeviceStatus::Offline
    } else if route.direct {
        DeviceStatus::Direct
    } else if route.scope == crate::api::ConnectionScope::Public {
        DeviceStatus::Relay
    } else {
        DeviceStatus::Connected
    }
}

/// Status of a saved station for which this client holds no connection.
pub fn unconnected(mesh: Option<&MeshReadiness>) -> DeviceStatus {
    project(mesh, Link::Idle, None, &ConnectionRoute::default(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ConnectionScope;

    #[test]
    fn local_station_online_does_not_claim_mesh_ready() {
        let route = ConnectionRoute::default();
        let ready = Some(&MeshReadiness::Ready);
        assert_eq!(
            project(
                Some(&MeshReadiness::Preparing),
                Link::Connected,
                Some(true),
                &route,
                false
            ),
            DeviceStatus::MeshPreparing
        );
        assert_eq!(
            project(ready, Link::Lost, Some(false), &route, false),
            DeviceStatus::Offline
        );
        assert_eq!(
            project(
                Some(&MeshReadiness::Failed("startup failed".into())),
                Link::Connected,
                Some(true),
                &route,
                false
            ),
            DeviceStatus::MeshFailed("startup failed".into())
        );
        assert_eq!(
            project(
                Some(&MeshReadiness::Preparing),
                Link::Connected,
                Some(true),
                &route,
                true
            ),
            DeviceStatus::Revoked
        );
    }

    #[test]
    fn connecting_only_while_an_attempt_is_in_progress() {
        let route = ConnectionRoute::default();
        let ready = Some(&MeshReadiness::Ready);
        // Never opened or stopped: an explicit "not connected", not 连接中.
        assert_eq!(
            project(ready, Link::Idle, None, &route, false),
            DeviceStatus::NotConnected
        );
        assert_eq!(unconnected(ready), DeviceStatus::NotConnected);
        assert_eq!(DeviceStatus::default(), DeviceStatus::NotConnected);
        // Client transport state still takes precedence for unopened stations.
        assert_eq!(unconnected(None), DeviceStatus::NotConnected);
        assert_eq!(
            unconnected(Some(&MeshReadiness::Stopped)),
            DeviceStatus::MeshStopped
        );
        assert_eq!(
            project(ready, Link::Connecting, None, &route, false),
            DeviceStatus::Connecting
        );
        // A known outcome ends Connecting, even before the stream opens.
        assert_eq!(
            project(ready, Link::Connecting, Some(true), &route, false),
            DeviceStatus::Connected
        );
        assert_eq!(
            project(ready, Link::Connecting, Some(false), &route, false),
            DeviceStatus::Offline
        );
        // Reconnect attempts after a failure stay Offline, not Connecting.
        assert_eq!(
            project(ready, Link::Lost, None, &route, false),
            DeviceStatus::Offline
        );
        assert_eq!(
            project(ready, Link::Lost, Some(true), &route, false),
            DeviceStatus::Offline
        );
    }

    #[test]
    fn open_stream_is_reachable_before_and_after_data_reads() {
        let ready = Some(&MeshReadiness::Ready);
        let direct = ConnectionRoute {
            scope: ConnectionScope::Lan,
            direct: true,
        };
        // The stream is open but the first catalog read has not completed or failed.
        assert_eq!(
            project(ready, Link::Connected, None, &direct, false),
            DeviceStatus::Direct
        );
        assert_eq!(
            project(ready, Link::Connected, Some(false), &direct, false),
            DeviceStatus::Direct
        );
        assert_eq!(
            project(
                ready,
                Link::Connected,
                None,
                &ConnectionRoute::default(),
                false
            ),
            DeviceStatus::Connected
        );
    }

    #[test]
    fn route_selects_direct_relay_or_connected() {
        let ready = Some(&MeshReadiness::Ready);
        let relay = ConnectionRoute {
            scope: ConnectionScope::Public,
            direct: false,
        };
        let direct = ConnectionRoute {
            scope: ConnectionScope::Public,
            direct: true,
        };
        assert_eq!(
            project(ready, Link::Connected, Some(true), &relay, false),
            DeviceStatus::Relay
        );
        // A relay path upgraded to a direct one is Direct regardless of scope.
        assert_eq!(
            project(ready, Link::Connected, Some(true), &direct, false),
            DeviceStatus::Direct
        );
    }
}
