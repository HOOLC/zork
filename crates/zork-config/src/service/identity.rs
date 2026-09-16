//! A packaged native background executable checks itself into Launch Services.
//! Standalone CLI binaries keep their ordinary process identity.
use anyhow::{ensure, Context, Result};
use std::{
    ffi::CStr,
    os::unix::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn native_helper(path: &Path) -> bool {
    path.ends_with("ZorkSupervisor.app/Contents/MacOS/zork")
        || path.ends_with("ZorkStation.app/Contents/MacOS/zork-station")
}

#[repr(C)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn GetCurrentProcess(process: *mut ProcessSerialNumber) -> i16;
}

unsafe extern "C" {
    fn _NSGetExecutablePath(buffer: *mut libc::c_char, size: *mut u32) -> libc::c_int;
}

pub(super) fn enter_bundle() -> Result<bool> {
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    let mut size = buffer.len() as u32;
    if unsafe { _NSGetExecutablePath(buffer.as_mut_ptr().cast(), &mut size) } != 0 {
        buffer.resize(size as usize, 0);
        ensure!(
            unsafe { _NSGetExecutablePath(buffer.as_mut_ptr().cast(), &mut size) } == 0,
            "Cannot read background executable path"
        );
    }
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(
        CStr::from_bytes_until_nul(&buffer)?.to_bytes(),
    ));
    let resolved = path
        .canonicalize()
        .context("Resolve background executable")?;
    if !native_helper(&resolved) {
        return Ok(false);
    }
    // CLI aliases must enter the canonical bundle before Launch Services caches
    // its identity. Normal core/supervisor launches already use this exact path.
    if path != resolved {
        return Err(Command::new(resolved)
            .args(std::env::args_os().skip(1))
            .exec()
            .into());
    }
    Ok(true)
}

pub(super) fn register(native: bool) -> Result<()> {
    if !native {
        return Ok(());
    }
    crate::startup::mark("helper.before_registration");
    let mut current = ProcessSerialNumber { high: 0, low: 0 };
    // The package declares LSBackgroundOnly. GetCurrentProcess registers the
    // name, icon and background activation policy without an NSApplication.
    let status = unsafe { GetCurrentProcess(&mut current) };
    ensure!(
        status == 0,
        "Register background process identity: {status}"
    );
    crate::startup::mark("helper.identity_ready");
    Ok(())
}
