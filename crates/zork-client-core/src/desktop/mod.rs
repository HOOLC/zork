//! Native client host capabilities and configuration; independent of UI widgets.
pub mod browser;
pub mod browser_worker;
pub mod data_reset;
pub mod directory;
pub mod node;
pub mod preview;
pub mod startup;
pub mod transport;
pub use zork_browser as browser_engine;
pub use zork_config::startup::mark as trace_startup;
/// Keep Mesh readiness timing visible in installed-client logs.
pub const DEFAULT_LOG_FILTER: &str =
    "warn,zork_mesh::managed=info,zork_client_core::desktop::transport=info";
pub fn automation_token() -> String {
    zork_config::random_token()
}
use std::path::PathBuf;

/// The signed bundle selects its channel even when launched without Launch Services.
/// An absent marker is the legacy release app; a corrupt marker must never open it.
fn app_channel() -> zork_config::channel::Channel {
    zork_config::channel::current().expect("valid signed client channel")
}

pub fn client_root() -> PathBuf {
    std::env::var_os("ZORK_CLIENT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| zork_config::channel::paths(app_channel()).client)
}

/// Prepare the installed app's existing state paths before any threads start.
/// The native entry point only adapts the returned log file to stdout/stderr.
#[cfg(target_os = "macos")]
pub fn prepare_app_environment() -> anyhow::Result<std::fs::File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    for name in ["ZORK_GUI_PREFERENCES_PATH", "ZORK_REGISTRY_DIR"] {
        if let Some(path) = std::env::var_os(name) {
            zork_config::channel::validate_path(&PathBuf::from(path), app_channel())?;
        }
    }
    zork_config::channel::claim(&client_root(), app_channel())?;
    let isolated = app_channel() != zork_config::channel::Channel::Release
        || std::env::var_os("ZORK_CLIENT_DATA").is_some();
    let root = if isolated {
        client_root()
    } else {
        PathBuf::from(std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is unset"))?)
            .join("Library/Application Support/Zork")
    };
    std::fs::create_dir_all(root.join("logs"))?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    if std::env::var_os("ZORK_GUI_PREFERENCES_PATH").is_none() {
        std::env::set_var("ZORK_GUI_PREFERENCES_PATH", root.join("preferences.json"));
    }
    if isolated && std::env::var_os("ZORK_REGISTRY_DIR").is_none() {
        std::env::set_var("ZORK_REGISTRY_DIR", root.join("nodes"));
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(root.join("logs/client.log"))?)
}

pub fn load_services() -> anyhow::Result<zork_config::services::ServicesConfig> {
    zork_config::services::ServicesConfig::load_for_data_root(&client_root())
}

#[cfg(target_os = "macos")]
pub fn reduced_motion() -> bool {
    trace_startup("gui.reduced_motion_begin");
    use std::ffi::{c_char, c_void};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            bytes: *const c_char,
            encoding: u32,
        ) -> *const c_void;
        fn CFPreferencesGetAppBooleanValue(
            key: *const c_void,
            application: *const c_void,
            valid: *mut u8,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    let reduced = unsafe {
        // These CF strings are owned locally; the preferences API only borrows them.
        let key = CFStringCreateWithCString(std::ptr::null(), c"reduceMotion".as_ptr(), 0x08000100);
        let app = CFStringCreateWithCString(
            std::ptr::null(),
            c"com.apple.universalaccess".as_ptr(),
            0x08000100,
        );
        let mut valid = 0;
        let value = !key.is_null()
            && !app.is_null()
            && CFPreferencesGetAppBooleanValue(key, app, &mut valid) != 0;
        if !key.is_null() {
            CFRelease(key);
        }
        if !app.is_null() {
            CFRelease(app);
        }
        valid != 0 && value
    };
    trace_startup("gui.reduced_motion_ready");
    reduced
}
