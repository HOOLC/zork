//! Exercise retained regions through the production conversation and real brand.
use futures_util::FutureExt;
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::RootView,
};

fn main() -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/ui-state-isolation/application");
    std::fs::create_dir_all(&output)?;
    let directory = tempfile::tempdir()?;
    let store = Arc::new(zork_gui::desktop::store::ClientStore::open(
        directory.path(),
    )?);
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(false);
        HeadlessAutomation::install(cx)
    });
    let mut view = None;
    let window = cx.open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
        let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
        view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let view = view.unwrap();
    for _ in 0..5 {
        cx.advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
    }
    let initial = driver.snapshot(false);
    let composer = initial
        .elements
        .iter()
        .find(|e| e.id == "composer-surface")
        .expect("composer surface is absent")
        .bounds;
    anyhow::ensure!(
        (composer.x + composer.width / 2. - 760.).abs() < 1.
            && (composer.y + composer.height - 784.).abs() < 1.,
        "cached composer must remain centered in the conversation and 16px above its bottom: {composer:?}"
    );
    let transcript = initial
        .elements
        .iter()
        .find(|e| e.id == "conversation-transcript")
        .expect("transcript is absent")
        .bounds;
    anyhow::ensure!(
        (transcript.y + transcript.height - 800.).abs() < 1.,
        "message viewport must extend behind the floating composer: {transcript:?}"
    );
    cx.update_window(window.into(), |_, w, cx| {
        w.refresh();
        w.draw(cx).clear(cx)
    })?;
    let refreshed = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "composer-surface")
        .unwrap()
        .bounds;
    anyhow::ensure!(
        composer == refreshed,
        "unrelated full refresh moved the composer: cached={composer:?}, refreshed={refreshed:?}"
    );
    let leader = initial
        .elements
        .iter()
        .find(|e| e.id.starts_with("leader-mini1-"))
        .expect("leader row is absent");
    anyhow::ensure!(
        (leader.bounds.width - 224.).abs() < 1.,
        "cached leader row must fill the 240px sidebar minus padding: {:?}",
        leader.bounds
    );
    anyhow::ensure!(
        initial.elements.iter().any(|e| e.id == "brand-header"),
        "real brand is absent"
    );
    let first = cx.capture_screenshot(window.into())?;
    let action =
        serde_json::from_value(json!({"type":"move","target":{"element_id":"brand-header"}}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let before = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let mut requested = 0;
    for _ in 0..120 {
        // GPUI Animation uses scheduler::Instant (wall time); advancing the
        // executor's test clock alone does not advance the animation phase.
        std::thread::sleep(Duration::from_millis(8));
        cx.advance_clock(Duration::from_millis(8));
        cx.run_until_parked();
        requested += cx.update_window(window.into(), |_, w, cx| {
            let requests = w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
            requests
        })?;
    }
    let after = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(requested > 0, "brand never requested animation frames");
    anyhow::ensure!(
        before.contains_key("transcript") && before.contains_key("composer"),
        "production regions missing: {before:?}"
    );
    anyhow::ensure!(
        before == after,
        "brand rebuilt unrelated regions: before={before:?}, after={after:?}"
    );
    let final_frame = cx.capture_screenshot(window.into())?;
    anyhow::ensure!(first != final_frame, "brand did not move");
    let retained = driver.snapshot(false);
    let expected: Vec<_> = initial
        .elements
        .iter()
        .filter(|e| e.id != "brand-header")
        .map(|e| &e.id)
        .collect();
    for id in expected {
        anyhow::ensure!(
            retained.elements.iter().any(|e| &e.id == id),
            "cached control disappeared: {id}"
        );
    }
    final_frame.save(output.join("brand-hover.png"))?;
    std::fs::write(
        output.join("isolation.json"),
        serde_json::to_vec_pretty(
            &json!({"animation_requests":requested,"before":before,"after":after,"controls":retained.elements.len()}),
        )?,
    )?;
    let conversation = view.update(&mut cx, |v, cx| v.benchmark_bind_core(cx));
    // Isolate a tail-following arrival. Reading older messages legitimately
    // updates the composer's unread indicator.
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    for _ in 0..5 {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
    }
    // Binding the core replaces the activity fixture and animates the member
    // parcels. Measure an unrelated message only after that prior change rests.
    for _ in 0..120 {
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
        if view.read_with(&cx, |v, _| v.benchmark_composer_material()["moving"] == false) { break; }
    }
    anyhow::ensure!(view.read_with(&cx, |v, _| v.benchmark_composer_material()["moving"] == false), "composer did not settle after core binding");
    cx.update_window(window.into(), |_, w, cx| w.simulate_next_frame(cx))?;
    let motion_before_message = view.read_with(&cx, |v, _| v.benchmark_message_motion());
    let before_message = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let original_count = view.update(&mut cx, |v, _| v.benchmark_record_count(false));
    let mut message_probe = conversation.subscribe();
    message_probe.snapshot();
    conversation.seed_event(&zork_client_core::api::SseEvent {
        name: "message".into(),
        data: json!({"type":"message","role":"assistant","id":"isolation-core-message","content":"来自核心订阅的新消息"}).to_string(),
    });
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_next_frame(cx);
        w.draw(cx).clear(cx)
    })?;
    let after_message = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let delta = message_probe.snapshot();
    std::fs::write(output.join("message-topics.json"), serde_json::to_vec_pretty(&json!({
        "reset":delta.reset,"activity":delta.activity_changed,"participants":delta.participants_changed,
        "loading":delta.loading_changed,"arrivals":delta.message_arrivals.count,"before":before_message,"after":after_message
    }))?)?;
    anyhow::ensure!(
        view.update(&mut cx, |v, _| v.benchmark_record_count(false)) == original_count + 1,
        "core message did not reach the presentation list"
    );
    anyhow::ensure!(
        after_message["transcript"][0] > before_message["transcript"][0],
        "message region stayed stale"
    );
    for (key, counts) in &before_message {
        if key != "transcript" {
            anyhow::ensure!(
                after_message.get(key) == Some(counts),
                "message refreshed unrelated {key}; before={counts:?}, after={:?}; tail state before event: {motion_before_message:?}", after_message.get(key)
            );
        }
    }
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.label.contains("来自核心订阅的新消息")),
        "new message was not rendered at the followed tail"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("core-message.png"))?;
    std::fs::write(
        output.join("core-message.json"),
        serde_json::to_vec_pretty(&json!({"before":before_message,"after":after_message}))?,
    )?;
    let snapshot = driver.snapshot(false);
    let message = snapshot
        .elements
        .iter()
        // The just-appended tail can be clipped at the viewport boundary.
        // Click a fully visible paragraph above it, outside the composer.
        .find(|e| e.label.starts_with("消息 0599") && e.id.ends_with("-selection"))
        .unwrap();
    let before_click = snapshot
        .elements
        .iter()
        .find(|e| e.id == "composer-surface")
        .unwrap()
        .bounds;
    let action = serde_json::from_value(json!({"type":"click","target":{
        "x":message.visible_bounds.x + 2., "y":message.center.y
    }}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let after_click = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "composer-surface")
        .unwrap()
        .bounds;
    anyhow::ensure!(
        before_click == after_click,
        "message click moved composer: {before_click:?} -> {after_click:?}"
    );
    // The first focus change requests a full GPUI refresh. Re-establish its
    // retained ranges before measuring an ordinary selection interaction.
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
    }
    let before_selection = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let action = serde_json::from_value(json!({"type":"click","target":{
        "x":message.visible_bounds.x + 2., "y":message.center.y
    }}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let after_selection = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(
        before_selection["composer"] == after_selection["composer"],
        "message selection invalidated composer: {before_selection:?} -> {after_selection:?}"
    );
    for (width, center) in [(1024., 632.), (1280., 760.)] {
        cx.update_window(window.into(), |_, w, cx| {
            w.resize(gpui::size(px(width), px(800.)));
            w.bounds_changed(cx);
        })?;
        for _ in 0..4 {
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
            let bounds = driver
                .snapshot(false)
                .elements
                .into_iter()
                .find(|e| e.id == "composer-surface")
                .unwrap()
                .bounds;
            anyhow::ensure!(
                (bounds.x + bounds.width / 2. - center).abs() < 1.
                    && (bounds.y + bounds.height - 784.).abs() < 1.,
                "composer shifted during resize/cache transition at {width}px: {bounds:?}"
            );
        }
    }
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_next_frame(cx);
        w.draw(cx).clear(cx)
    })?;
    let before_burst = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let burst_base = view.update(&mut cx, |v, _| v.benchmark_record_count(false));
    for i in 0..1_000 {
        conversation.seed_event(&zork_client_core::api::SseEvent { name: "message".into(),
            data: json!({"type":"message","role":"assistant","id":format!("frame-burst-{i}"),"content":format!("帧内突发 {i}")}).to_string() });
    }
    cx.run_until_parked();
    anyhow::ensure!(
        view.update(&mut cx, |v, _| v.benchmark_record_count(false)) == burst_base,
        "subscription converted the burst before its scheduled frame"
    );
    cx.update_window(window.into(), |_, w, cx| {
        w.simulate_next_frame(cx);
        w.draw(cx).clear(cx)
    })?;
    let after_burst = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(
        view.update(&mut cx, |v, _| v.benchmark_record_count(false)) == burst_base + 1_000,
        "frame coalescing lost messages across journal reset"
    );
    anyhow::ensure!(
        after_burst["transcript"][0] == before_burst["transcript"][0] + 1,
        "one burst caused more than one transcript render: {before_burst:?} -> {after_burst:?}"
    );
    std::fs::write(
        output.join("frame-burst.json"),
        serde_json::to_vec_pretty(&json!({
        "commits":1000,"base":burst_base,"before":before_burst,"after":after_burst}))?,
    )?;
    println!(
        "Composer position stable across cached/full frames, message clicks, and window resize"
    );
    // The paperclip invokes the platform picker directly. The headless
    // platform cancels it; no intermediate composer menu should be rendered.
    let action =
        serde_json::from_value(json!({"type":"click","target":{"element_id":"composer-options"}}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx)
        })?;
    }
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "composer-menu-close" || e.id == "composer-add-files"),
        "attachment picker still has an intermediate popup"
    );
    let core = view.update(&mut cx, |v, _| v.benchmark_core_device());
    let mut navigation = core.navigation();
    navigation.snapshot();
    // GPUI deliberately refreshes hover/focus styles once when switching from
    // mouse to keyboard. Measure subsequent edits within keyboard modality.
    let action = serde_json::from_value(json!({"type":"key","keystroke":"left"}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let before_typing = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    let action = serde_json::from_value(json!({"type":"type_text","text":"核心草稿"}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    anyhow::ensure!(
        core.draft("render-fixture").text == "核心草稿",
        "input edit did not commit the core draft"
    );
    let action = serde_json::from_value(json!({"type":"key","keystroke":"left"}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
    let after_typing = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    for (key, counts) in &before_typing {
        if key.starts_with("navigation/") || key == "header" {
            anyhow::ensure!(
                after_typing.get(key) == Some(counts),
                "typing/caret updated unrelated {key}"
            );
        }
    }
    anyhow::ensure!(
        navigation.changed().now_or_never().is_none(),
        "draft/caret changed core navigation"
    );
    for width in [900., 1280.] {
        cx.update_window(window.into(), |_, w, cx| {
            w.resize(gpui::size(px(width), px(800.)));
            w.bounds_changed(cx);
        })?;
        view.update(&mut cx, |v, cx| v.benchmark_jump_to(580, cx));
        for _ in 0..5 {
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        cx.capture_screenshot(window.into())?
            .save(output.join(format!("floating-composer-{width:.0}.png")))?;
    }
    println!(
        "120 frames: unrelated render/layout/prepaint/paint counters unchanged; cached controls retained"
    );
    println!("Core message publication updates only the transcript and becomes visible");
    println!("Direct file picker, draft commit and caret/navigation isolation passed");
    Ok(())
}
