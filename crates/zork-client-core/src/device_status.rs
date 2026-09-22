//! One device-status projection; views never infer Mesh readiness from peer errors.
use crate::api::ConnectionRoute;
pub use zork_client_types::device::{DeviceStatus, MeshReadiness};

pub fn project(
    mesh: Option<&MeshReadiness>,
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
    match online {
        Some(false) => DeviceStatus::Offline,
        None => DeviceStatus::Connecting,
        Some(true) if route.direct => DeviceStatus::Direct,
        Some(true) if route.scope == crate::api::ConnectionScope::Public => DeviceStatus::Relay,
        Some(true) => DeviceStatus::Connected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_station_online_does_not_claim_mesh_ready() {
        let route = ConnectionRoute::default();
        assert_eq!(
            project(Some(&MeshReadiness::Preparing), Some(true), &route, false),
            DeviceStatus::MeshPreparing
        );
        assert_eq!(
            project(Some(&MeshReadiness::Ready), Some(false), &route, false),
            DeviceStatus::Offline
        );
        assert_eq!(
            project(Some(&MeshReadiness::Ready), None, &route, false),
            DeviceStatus::Connecting
        );
        assert_eq!(
            project(
                Some(&MeshReadiness::Failed("startup failed".into())),
                Some(true),
                &route,
                false
            ),
            DeviceStatus::MeshFailed("startup failed".into())
        );
        assert_eq!(
            project(Some(&MeshReadiness::Preparing), Some(true), &route, true),
            DeviceStatus::Revoked
        );
    }
}
