//! First-use login uses the production account controller and shared native controls.
use anyhow::ensure;
use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::DesktopRoot,
};

fn main() -> anyhow::Result<()> {
    let output =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/onboarding/native");
    std::fs::create_dir_all(&output)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let reject = Arc::new(AtomicBool::new(false));
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let origin = format!("http://{}", listener.local_addr()?);
    let server = runtime.spawn({
        let origin = origin.clone();
        let reject = reject.clone();
        async move {
            let app = Router::new()
                .route("/v1/auth/device", post(move |Json(value): Json<Value>| {
                    let origin = origin.clone();
                    let reject = reject.clone();
                    async move {
                        if reject.load(Ordering::Acquire) {
                            return (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"Isolated sign-in failure"}))).into_response();
                        }
                        Json(json!({
                            "verification_uri": format!("{origin}/v1/auth/device/{}", value["id"].as_str().unwrap()),
                            "expires_at": SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()+120,
                            "interval":3
                        })).into_response()
                    }
                }))
                .route("/v1/auth/device/token", post(|| async { Json(json!({"pending":true})) }))
                .route("/v1/auth/device/cancel", post(|| async { Json(json!({"ok":true})) }));
            axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), app).await.unwrap();
        }
    });
    for (width, locale) in [(960., "zh-CN"), (360., "en")] {
        reject.store(false, Ordering::Release);
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("client");
        std::fs::create_dir_all(&root)?;
        std::fs::write(
            root.join("services.json"),
            serde_json::to_vec(&json!({"relay_urls":[origin]}))?,
        )?;
        std::env::set_var("ZORK_CLIENT_DATA", &root);
        std::env::set_var(
            "ZORK_GUI_PREFERENCES_PATH",
            temp.path().join("preferences.json"),
        );
        std::env::set_var("ZORK_GUI_LOCALE", locale);
        let startup = zork_client_core::desktop::startup::Startup::open()?;
        let source = startup.directory.clone();
        let mut cx = HeadlessAppContext::with_platform(
            gpui_platform::current_platform(true).text_system(),
            Arc::new(EmbeddedAssets),
            gpui_platform::current_headless_renderer,
        );
        let driver = cx.update(|cx| {
            zork_gui::assets::init_fonts(cx);
            zork_gui::components::init(cx);
            cx.set_reduce_motion(true);
            DesktopRoot::install_startup(startup, cx);
            HeadlessAutomation::install(cx)
        });
        let window = cx.open_window(gpui::size(px(width), px(680.)), |_, cx| {
            let root = cx.new(DesktopRoot::new);
            cx.new(|_| AutomationRoot::new(root))
        })?;
        let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
            std::thread::sleep(Duration::from_millis(10));
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
            Ok(())
        };
        let visible = |id: &str| {
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == id && e.visible && e.enabled)
        };
        let wait =
            |cx: &mut HeadlessAppContext, condition: &dyn Fn() -> bool| -> anyhow::Result<()> {
                let deadline = Instant::now() + Duration::from_secs(15);
                loop {
                    pump(cx)?;
                    if condition() {
                        return Ok(());
                    }
                    ensure!(
                        Instant::now() < deadline,
                        "onboarding UI condition timed out"
                    );
                }
            };
        let click = |cx: &mut HeadlessAppContext, id: &str| -> anyhow::Result<()> {
            cx.update_window(window.into(), |_, w, cx| {
                driver.dispatch(
                    serde_json::from_value(json!({"type":"click","target":{"element_id":id}}))
                        .unwrap(),
                    w,
                    cx,
                )
            })??;
            pump(cx)
        };
        wait(&mut cx, &|| visible("desktop-welcome-login"))?;
        for _ in 0..8 {
            pump(&mut cx)?;
        }
        for id in [
            "desktop-start-local",
            "desktop-startup-settings",
            "desktop-manage",
        ] {
            ensure!(
                !visible(id),
                "first-use exposes workspace/device action {id}"
            );
        }
        let snapshot = driver.snapshot(false);
        let login = snapshot
            .elements
            .iter()
            .find(|e| e.id == "desktop-welcome-login")
            .unwrap();
        ensure!(
            login.bounds == login.visible_bounds,
            "login clipped at {width}"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("login-{locale}-{width}.png")))?;
        click(&mut cx, "desktop-welcome-login")?;
        wait(&mut cx, &|| {
            visible("onboarding-reopen-login") && visible("onboarding-cancel-login")
        })?;
        ensure!(
            !visible("desktop-welcome-login"),
            "duplicate login remains enabled"
        );
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("waiting-{locale}-{width}.png")))?;
        click(&mut cx, "onboarding-cancel-login")?;
        wait(&mut cx, &|| {
            visible("desktop-welcome-login") && !source.account.snapshot().busy()
        })?;
        ensure!(
            !source.account.snapshot().authenticated,
            "cancel signed the user in"
        );
        ensure!(
            source.snapshot().nodes.is_empty(),
            "cancel created a workspace"
        );
        reject.store(true, Ordering::Release);
        click(&mut cx, "desktop-welcome-login")?;
        wait(&mut cx, &|| {
            source.account.snapshot().error.is_some() && visible("desktop-welcome-login")
        })?;
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("failure-{locale}-{width}.png")))?;
        reject.store(false, Ordering::Release);
        click(&mut cx, "desktop-welcome-login")?;
        wait(&mut cx, &|| {
            visible("onboarding-cancel-login") && source.account.snapshot().login_url.is_some()
        })?;
        click(&mut cx, "onboarding-cancel-login")?;
        wait(&mut cx, &|| !source.account.snapshot().busy())?;
        ensure!(
            source.store.get::<bool>("client", "onboarding-complete")? == Some(false),
            "incomplete login marked complete"
        );
        drop(cx);
        drop(source);
    }
    server.abort();
    runtime.shutdown_background();
    println!("PASS onboarding: native login, browser wait, cancellation, failure/retry, no device entry; Chinese 960px and English 360px");
    Ok(())
}
