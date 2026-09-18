use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::connections::ConnectionManager;
use crate::db::StationDb;
use crate::im_entry::ImEntryStation;
use crate::jobs::JobSupervisor;
use crate::status_projection::AgentStatusProjector;

/// Admin-plane attachments; None only during early construction.
#[derive(Clone)]
pub struct AdminPlane {
    pub db: Arc<crate::control_db::ControlDb>,
    pub admin_token: Option<String>,
    pub started_at: String,
    pub reload_sock: std::path::PathBuf,
}

#[derive(Clone)]
pub struct AppState {
    pub login_cards: Arc<crate::provider_login::Hub>,
    pub interaction_handlers: Arc<crate::interaction_registry::Handlers>,
    pub provider_auth: Arc<crate::node::auth::Hub>,
    pub node_tools: Arc<crate::node_tools::Hub>,
    pub mcp: Arc<crate::mcp::Hub>,
    pub browser: Arc<crate::browser::Hub>,
    pub config: RuntimeConfig,
    pub agent: zork_agent::Agent,
    pub draining: Arc<std::sync::atomic::AtomicBool>,
    pub db: Arc<StationDb>,
    pub files: Arc<crate::files::Files>,
    pub connections: Arc<ConnectionManager>,
    pub entries: ImEntryStation,
    pub status_projection: AgentStatusProjector,

    pub jobs: Arc<JobSupervisor>,
    pub admin: AdminPlane,
    /// Local APIs are available while the owned Mesh startup checks peers.
    pub mesh: Arc<std::sync::OnceLock<Arc<crate::mesh::MeshService>>>,
}
