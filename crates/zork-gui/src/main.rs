use gpui::{prelude::*, px, size, AnyWindowHandle, App, Bounds, WindowAppearance};
use gpui_platform::application;
use zork_gui::assets::EmbeddedAssets;
use zork_gui::automation::{AutomationRoot, DevAutomation, DEFAULT_DEV_PORT};
use zork_gui::components;
use zork_gui::window_chrome::native_titlebar_options;

#[cfg(target_os = "macos")]
use anyhow::Context as _;
use zork_gui::desktop::DesktopRoot;

#[cfg(target_os = "macos")]
gpui::actions!(zork_gui, [Quit]);

#[cfg(target_os = "macos")]
fn install_app_menu(cx: &mut App) {
    use gpui::{KeyBinding, Menu, MenuItem};
    use objc2_foundation::{NSBundle, NSString};

    let app_name = NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleDisplayName"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Zork".to_owned());

    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.set_menus([
        Menu::new(app_name.clone()).items([MenuItem::action(format!("退出 {app_name}"), Quit)])
    ]);
}

#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    dev: bool,
    dev_port: u16,
    dev_token: Option<String>,
    help: bool,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            dev: false,
            dev_port: DEFAULT_DEV_PORT,
            dev_token: None,
            help: false,
        }
    }
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<CliOptions, String> {
    let mut options = CliOptions::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--dev" => options.dev = true,
            "--dev-port" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--dev-port requires a value".to_owned())?;
                options.dev_port = value
                    .parse::<u16>()
                    .map_err(|_| format!("invalid --dev-port value: {value}"))?;
            }
            "--dev-token" => {
                options.dev_token = Some(
                    args.next()
                        .ok_or_else(|| "--dev-token requires a value".to_owned())?,
                );
            }
            "--help" | "-h" => options.help = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(options)
}

fn print_help() {
    println!(
        "zork-gui [options]\n\n\
         Options:\n\
         --dev                enable loopback-only UI automation API\n\
         --dev-port <port>    automation port; 0 chooses a free port (default {DEFAULT_DEV_PORT})\n\
         --dev-token <token>  automation bearer token (default: generated)\n\
         Environment:\n\
         ZORK_GUI_DEV_TOKEN provides the automation token.\n"
    );
}

#[cfg(target_os = "macos")]
fn prepare_app_environment() -> anyhow::Result<()> {
    use std::os::fd::AsRawFd;

    if objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some()
    {
        let log = zork_client_core::desktop::prepare_app_environment()?;
        for target in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            // The open file remains owned here while dup2 installs independent
            // process-wide descriptors, before GPUI or background workers start.
            if unsafe { libc::dup2(log.as_raw_fd(), target) } == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn quit_on_sigterm() -> anyhow::Result<futures_channel::oneshot::Receiver<()>> {
    let (armed, ready) = std::sync::mpsc::sync_channel(1);
    let (quit, requested) = futures_channel::oneshot::channel();
    std::thread::Builder::new()
        .name("zork-gui-terminate".into())
        .spawn(move || {
            let failed = armed.clone();
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .and_then(|runtime| {
                    runtime.block_on(async {
                        let mut signal = tokio::signal::unix::signal(
                            tokio::signal::unix::SignalKind::terminate(),
                        )?;
                        let _ = armed.send(Ok(()));
                        signal.recv().await;
                        let _ = quit.send(());
                        Ok(())
                    })
                });
            if let Err(error) = result {
                let _ = failed.send(Err(error));
            }
        })?;
    ready.recv().context("SIGTERM handler did not start")??;
    Ok(requested)
}

#[cfg(target_os = "macos")]
fn mark_gui_main_for_smoke() {
    let Some(path) = std::env::var_os("ZORK_GUI_MAIN_NS_FILE") else {
        return;
    };
    let mut clock = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) } == 0 {
        let ns = clock.tv_sec as u64 * 1_000_000_000 + clock.tv_nsec as u64;
        let _ = std::fs::write(path, format!("{} {ns}\n", std::process::id()));
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    mark_gui_main_for_smoke();
    zork_client_core::desktop::trace_startup("gui.main");
    let options = parse_args_from(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\nTry zork-gui --help");
        std::process::exit(2);
    });
    if options.help {
        print_help();
        return;
    }
    gpui::set_startup_observer(zork_client_core::desktop::trace_startup);
    #[cfg(target_os = "macos")]
    gpui_apple::metal_renderer::prepare_renderer();
    zork_client_core::desktop::trace_startup("gui.renderer_preparation_dispatched");
    let _data_lease = zork_client_core::desktop::data_reset::initialize().unwrap_or_else(|error| {
        eprintln!("无法打开或清空客户端数据：{error:#}");
        std::process::exit(1);
    });
    zork_client_core::desktop::trace_startup("gui.data_lease_ready");
    #[cfg(target_os = "macos")]
    prepare_app_environment().unwrap_or_else(|error| {
        eprintln!("failed to prepare Zork app: {error}");
        std::process::exit(1);
    });
    // Packaged apps redirect stderr to their own data directory. Keep core
    // transport failures observable there; RUST_LOG enables scoped diagnostics.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| zork_client_core::desktop::DEFAULT_LOG_FILTER.into()),
        )
        .with_ansi(false)
        .try_init();
    zork_client_core::desktop::trace_startup("gui.environment_ready");
    let startup = zork_client_core::desktop::startup::Startup::prepare().unwrap_or_else(|error| {
        eprintln!("无法打开客户端运行时：{error:#}");
        std::process::exit(1);
    });
    #[cfg(target_os = "macos")]
    let terminate = quit_on_sigterm().unwrap_or_else(|error| {
        eprintln!("failed to install graceful termination: {error:#}");
        std::process::exit(1);
    });
    let initial_size = options
        .dev
        .then(|| std::env::var("ZORK_GUI_TEST_WINDOW_SIZE").ok())
        .flatten()
        .and_then(|s| {
            let (w, h) = s.split_once('x')?;
            let w = w.parse::<u32>().ok()?;
            let h = h.parse::<u32>().ok()?;
            (w >= 900 && h >= 600 && w <= 3840 && h <= 2160).then_some((w as f32, h as f32))
        })
        .unwrap_or((1280., 800.));
    let mut automation = options.dev.then(|| {
        let token = options
            .dev_token
            .or_else(|| std::env::var("ZORK_GUI_DEV_TOKEN").ok());
        DevAutomation::bind(options.dev_port, token).unwrap_or_else(|error| {
            eprintln!("failed to start zork-gui dev API: {error}");
            std::process::exit(2);
        })
    });
    if let Some(automation) = automation.as_ref() {
        eprintln!(
            "zork-gui dev API: http://{}/v1\nzork-gui dev token: {}",
            automation.address(),
            automation.token()
        );
    }

    zork_client_core::desktop::trace_startup("gui.before_application");
    let app = application();
    zork_client_core::desktop::trace_startup("gui.application_created");
    app.with_assets(EmbeddedAssets).run(move |cx: &mut App| {
        let startup = startup.finish().unwrap_or_else(|error| {
            eprintln!("无法打开客户端运行时：{error:#}");
            std::process::exit(1);
        });
        DesktopRoot::install_startup(startup, cx);
        #[cfg(target_os = "macos")]
        install_app_menu(cx);
        #[cfg(target_os = "macos")]
        cx.spawn(async move |cx| {
            if terminate.await.is_ok() {
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
        zork_client_core::desktop::trace_startup("gui.run_callback");
        zork_gui::assets::init_fonts(cx);
        zork_client_core::desktop::trace_startup("gui.fonts_ready");
        components::init(cx);
        zork_client_core::desktop::trace_startup("gui.components_ready");
        cx.set_window_appearance(Some(WindowAppearance::Light));
        let window_options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(initial_size.0), px(initial_size.1)),
                cx,
            ))),
            titlebar: Some(native_titlebar_options()),
            window_min_size: Some(size(px(900.0), px(600.0))),
            ..Default::default()
        };
        if let Some(dev) = automation.as_ref() {
            dev.install(cx);
        }
        zork_client_core::desktop::trace_startup("gui.before_window");
        let window: AnyWindowHandle = if automation.is_some() {
            cx.open_window(window_options, |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                let root = cx.new(DesktopRoot::new);
                cx.new(|_| AutomationRoot::new(root))
            })
            .expect("failed to open window")
            .into()
        } else {
            cx.open_window(window_options, |window, cx| {
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                cx.new(DesktopRoot::new)
            })
            .expect("failed to open window")
            .into()
        };
        zork_client_core::desktop::trace_startup("gui.window_opened");
        if let Some(dev) = automation.take() {
            dev.attach(window, cx);
        }
        cx.activate(true);
        zork_client_core::desktop::trace_startup("gui.activated");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dev_automation_options() {
        let options = parse_args_from(
            ["--dev", "--dev-port", "0", "--dev-token", "test-token"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("arguments should parse");

        assert!(options.dev);
        assert_eq!(options.dev_port, 0);
        assert_eq!(options.dev_token.as_deref(), Some("test-token"));
    }

    #[test]
    fn rejects_unknown_or_incomplete_options() {
        assert!(parse_args_from(["--unknown".to_owned()]).is_err());
        assert!(parse_args_from([
            "--station-url".to_owned(),
            "http://localhost:3000".to_owned()
        ])
        .is_err());
        assert!(parse_args_from(["--station-token".to_owned(), "token".to_owned()]).is_err());
        assert!(parse_args_from(["--dev-port".to_owned()]).is_err());
        assert!(parse_args_from(["--dev-port".to_owned(), "70000".to_owned()]).is_err());
    }
}
