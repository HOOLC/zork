//! Production desktop account interactions; run with the local Worker harness.
use anyhow::ensure;
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    desktop::DesktopRoot,
};

fn main() -> anyhow::Result<()> {
    let Some(output) = std::env::var_os("ZORK_ACCOUNT_UI_OUTPUT").map(PathBuf::from) else {
        println!(
            "Use deploy/cloudflare/test/product-account.ts to supply the isolated Worker fixture."
        );
        return Ok(());
    };
    std::fs::create_dir_all(&output)?;
    // This harness covers account settings in an existing installation.
    // First-use login and cancellation are exercised by headless_onboarding.
    zork_client_core::store::ClientStore::open(&zork_client_core::desktop::client_root())?.put(
        "client",
        "onboarding-complete",
        &true,
    )?;
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
    let window = cx.open_window(gpui::size(px(960.), px(680.)), |_, cx| {
        let root = cx.new(DesktopRoot::new);
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        std::thread::sleep(Duration::from_millis(20));
        cx.advance_clock(Duration::from_millis(20));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
        })?;
        Ok(())
    };
    let wait = |cx: &mut HeadlessAppContext,
                condition: &dyn Fn() -> bool,
                label: &str|
     -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(90);
        while !condition() {
            ensure!(Instant::now() < deadline, "timeout: {label}");
            pump(cx)?;
        }
        for _ in 0..3 {
            pump(cx)?;
        }
        Ok(())
    };
    let click = |cx: &mut HeadlessAppContext, id: &str| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"click","target":{"element_id":id}})).unwrap(),
                w,
                cx,
            )
        })??;
        for _ in 0..4 {
            pump(cx)?;
        }
        Ok(())
    };
    for _ in 0..8 {
        pump(&mut cx)?;
    }
    ensure!(
        !source.account.snapshot().authenticated,
        "fresh profile must not be logged in"
    );
    click(&mut cx, "desktop-welcome-login")?;
    wait(
        &mut cx,
        &|| source.account.snapshot().login_url.is_some(),
        "welcome login URL",
    )?;
    click(&mut cx, "desktop-startup-settings")?;
    click(&mut cx, "client_account")?;
    click(&mut cx, "zork-account-cancel")?;
    wait(
        &mut cx,
        &|| !source.account.snapshot().busy(),
        "cancel login",
    )?;
    ensure!(
        !source.account.snapshot().authenticated,
        "cancel must leave profile signed out"
    );
    click(&mut cx, "zork-account-login")?;
    wait(
        &mut cx,
        &|| source.account.snapshot().login_url.is_some(),
        "settings login URL",
    )?;
    std::fs::write(
        output.join("authorization.json"),
        serde_json::to_vec(&json!({"url":source.account.snapshot().login_url}))?,
    )?;
    wait(
        &mut cx,
        &|| source.account.snapshot().authenticated && !source.account.snapshot().busy(),
        "browser approval",
    )?;
    ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "zork-account-more"),
        "signed-in page lacks the account menu"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("signed-in.png"))?;
    let account = source.account.snapshot();
    let root = zork_client_core::desktop::client_root();
    for child in ["node", "transport"] {
        let shared =
            zork_client_core::relay_account::Account::new(root.join(child), &account.origin)?;
        ensure!(
            shared.cached_access()?.is_some(),
            "owned {child} did not share the account"
        );
        ensure!(
            !zork_config::relay_account::path(&root.join(child)).exists(),
            "owned {child} copied credentials"
        );
    }
    std::fs::write(output.join("signed-in"), b"ready")?;
    wait(
        &mut cx,
        &|| output.join("relay-ready").exists(),
        "owned Station relay admission",
    )?;
    // Sign-out is a rare action in the account's 更多 menu.
    click(&mut cx, "zork-account-more")?;
    click(&mut cx, "zork-account-more-menu-0-zork-account-logout")?;
    wait(
        &mut cx,
        &|| {
            let state = source.account.snapshot();
            !state.busy() && !state.authenticated && state.pending_revocations == 0
        },
        "logout confirmation",
    )?;
    for child in ["node", "transport"] {
        ensure!(
            zork_client_core::relay_account::Account::new(root.join(child), &account.origin)?
                .cached_access()?
                .is_none(),
            "owned {child} retained access after logout"
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("signed-out.png"))?;
    std::fs::write(
        output.join("complete.json"),
        serde_json::to_vec_pretty(
            &json!({"passed":true,"checks":["welcome login available before Mesh", "account tab always available", "cancel login", "Google device approval completes product controller", "owned Station and client share one private session", "logout clears both owners"]}),
        )?,
    )?;
    println!("PASS: desktop account UI, shared profile and logout");
    let _ = source.transport.shutdown();
    Ok(())
}
