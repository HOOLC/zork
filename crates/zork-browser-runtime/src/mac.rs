// Cocoa CEF protocol integration follows cef-rs; see ../LICENSE.cef-rs.
use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{define_class, msg_send, rc::Retained, runtime::Bool, DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent};
use std::cell::Cell;

pub fn configure_runtime(command_line: &mut cef::CommandLine) {
    use cef::{CefString, ImplCommandLine};
    // Chromium's clone preserves its identity during an in-place auto-update.
    // Zork stops the bundle before replacing it; CEF's helper cannot run the
    // Chrome-only clone cleanup entry, which otherwise crashes on every exit.
    let key: CefString = "disable-features".into();
    let disabled = CefString::from(&command_line.switch_value(Some(&key))).to_string();
    if !disabled
        .split(',')
        .any(|name| name == "MacAppCodeSignClone")
    {
        let value = if disabled.is_empty() {
            "MacAppCodeSignClone".to_owned()
        } else {
            format!("{disabled},MacAppCodeSignClone")
        };
        command_line.remove_switch(Some(&key));
        command_line.append_switch_with_value(Some(&key), Some(&value.as_str().into()));
    }
}

/// Preserve CEF's sandbox arguments while selecting a role-specific app icon.
pub fn select_helper(command_line: &mut cef::CommandLine) {
    use cef::{CefString, ImplCommandLine};
    let kind = CefString::from(&command_line.switch_value(Some(&"type".into()))).to_string();
    let service =
        CefString::from(&command_line.switch_value(Some(&"utility-sub-type".into()))).to_string();
    let role = match (kind.as_str(), service.as_str()) {
        ("gpu-process", _) => "GPU",
        ("utility", "network.mojom.NetworkService") => "Network",
        ("utility", "storage.mojom.StorageService") => "Storage",
        _ => return,
    };
    let program = std::path::PathBuf::from(CefString::from(&command_line.program()).to_string());
    let Some(frameworks) = program
        .ancestors()
        .find(|p| p.extension().is_some_and(|s| s == "app"))
        .and_then(|p| p.parent())
    else {
        return;
    };
    let name = format!("ZorkBrowser Helper ({role})");
    let executable = frameworks.join(format!("{name}.app/Contents/MacOS/{name}"));
    if executable.is_file() {
        command_line.set_program(Some(&executable.to_string_lossy().as_ref().into()));
    }
}

#[derive(Default)]
pub struct Ivars {
    sending: Cell<Bool>,
}
define_class!(
    #[unsafe(super(NSApplication))]
    #[ivars = Ivars]
    pub struct BrowserApplication;
    impl BrowserApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let previous = self.ivars().sending.replace(Bool::YES);
            let _: () = msg_send![super(self), sendEvent: event];
            self.ivars().sending.set(previous);
        }
    }
    unsafe impl CrAppControlProtocol for BrowserApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling(&self, value: Bool) { self.ivars().sending.set(value); }
    }
    unsafe impl CrAppProtocol for BrowserApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn handling(&self) -> Bool { self.ivars().sending.get() }
    }
    unsafe impl CefAppProtocol for BrowserApplication {}
);
pub fn initialize() {
    use objc2::ClassType;
    let _: Retained<BrowserApplication> =
        unsafe { msg_send![BrowserApplication::class(), sharedApplication] };
    NSApplication::sharedApplication(MainThreadMarker::new().unwrap())
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}
