//! Opt-in startup milestones. No files are touched unless a trace directory is supplied.
#[cfg(unix)]
use std::{
    fs::{File, OpenOptions},
    io::Write,
    sync::{Mutex, OnceLock},
};

/// Record a process milestone against the host's shared monotonic clock.
/// Diagnostic IO is best effort and cannot affect the startup result.
pub fn mark(name: &'static str) {
    #[cfg(unix)]
    {
        static TRACE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
        let trace = TRACE.get_or_init(|| {
            let directory = std::path::PathBuf::from(std::env::var_os("ZORK_STARTUP_TRACE")?);
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(directory.join(format!("startup-{}.jsonl", std::process::id())))
                .ok()
                .map(Mutex::new)
        });
        let Some(trace) = trace else { return };
        let mut clock = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } != 0 {
            return;
        }
        if let Ok(mut file) = trace.lock() {
            let _ = writeln!(
                file,
                "{}",
                serde_json::json!({
                    "pid": std::process::id(), "mark": name,
                    "ns": clock.tv_sec as u64 * 1_000_000_000 + clock.tv_nsec as u64,
                })
            );
        }
    }
    #[cfg(not(unix))]
    let _ = name;
}
