//! Independently replaceable external-effect boundaries.

mod clock;
mod control;
mod filesystem;
mod identity;
mod model;
mod process;
mod profile;

pub use clock::{Clock, Sleep, SystemClock};
pub(crate) use control::CancelToolRequest;
pub use control::{ToolCancellation, ToolControl};
pub use filesystem::{FileFuture, FilePage, FileSystem, SystemFileSystem};
pub use identity::{IdGenerator, SystemIdGenerator};
pub use model::{ModelExecutor, ModelPort};
pub use process::{
    ProcessHandle, ProcessRequest, ProcessSpawner, ProcessStatus, SpawnedProcess,
    SystemProcessSpawner,
};
pub use profile::{ModelLimits, ProfileExecution, ProfileResolveError, ProfileResolver};
