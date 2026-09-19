//! Exercise composer presence through the production conversation subscription.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    api::{AgentStatus, ParticipantStatus},
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::RootView,
};

fn main() -> anyhow::Result<()> {
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    let output = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        std::env::var("ZORK_LIQUID_OUTPUT")
            .unwrap_or_else(|_| "../../artifacts/liquid-composer/simulated".into()),
    );
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
    let conversation = view.update(&mut cx, |v, cx| v.benchmark_bind_core(cx));
    let mut state = conversation.snapshot().as_ref().clone();
    state.participants = Arc::new(
        ["Atlas", "Nova", "Echo"]
            .iter()
            .enumerate()
            .map(|(i, name)| ParticipantStatus {
                subscribed: false,
                assigned: false,
                id: format!("presence-{i}"),
                name: name.to_string(),
                avatar: Some(["cat", "bunny", "bear"][i].into()),
                session_id: "fixture".into(),
                activity: None,
            })
            .collect(),
    );
    conversation.seed(state.clone());
    let frame_positions = std::cell::RefCell::new(Vec::<f32>::new());
    let max_rows = std::cell::Cell::new(0_usize);
    let pump = |cx: &mut HeadlessAppContext, frames: usize| -> anyhow::Result<()> {
        // Two 120 Hz display ticks per legacy 16 ms fixture step.
        for _ in 0..frames * 2 {
            let tick = Duration::from_nanos(8_333_333);
            std::thread::sleep(tick);
            view.update(cx, |v, _| v.benchmark_begin_frame());
            cx.advance_clock(tick);
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
            })?;
            let rows = view.read_with(cx, |v, _| v.benchmark_frame_state(false).0);
            max_rows.set(max_rows.get().max(rows));
            if let Some(member) = driver
                .snapshot(false)
                .elements
                .iter()
                .find(|e| e.id == "composer-member-presence-0")
            {
                frame_positions.borrow_mut().push(member.bounds.y);
            }
        }
        Ok(())
    };
    pump(&mut cx, 5)?;
    // Binding the conversation and inserting its members starts real motion.
    // Capture the idle reference only after that initial transition settles.
    for _ in 0..120 {
        if !view.read_with(&cx, |v, _| v.benchmark_composer_material())["moving"]
            .as_bool()
            .unwrap_or(false)
        {
            break;
        }
        pump(&mut cx, 1)?;
    }
    anyhow::ensure!(
        !view.read_with(&cx, |v, _| v.benchmark_composer_material())["moving"]
            .as_bool()
            .unwrap_or(false),
        "initial composer motion did not settle"
    );
    let requested_anchor = std::env::var("ZORK_BENCH_ANCHOR")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    if let Some(anchor) = requested_anchor {
        view.update(&mut cx, |v, cx| v.benchmark_jump_to(anchor, cx));
        pump(&mut cx, 2)?;
    }
    let initial_anchor = view.read_with(&cx, |v, _| v.benchmark_frame_state(false).1);
    if let Some(anchor) = requested_anchor {
        anyhow::ensure!(
            initial_anchor.abs_diff(anchor) <= 10,
            "benchmark missed requested anchor: {initial_anchor} vs {anchor}"
        );
    }
    let message_count = view.read_with(&cx, |v, _| v.benchmark_record_count(false));
    if let Ok(expected) = std::env::var("ZORK_BENCH_MESSAGE_COUNT") {
        anyhow::ensure!(
            message_count == expected.parse::<usize>()?,
            "message fixture count mismatch"
        );
    }
    let bounds = |id: &str| {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .map(|e| e.bounds)
            .ok_or_else(|| anyhow::anyhow!("Missing {id}"))
    };
    let check_action_insets = || -> anyhow::Result<()> {
        let surface = bounds("composer-surface")?;
        let send = bounds("send-button")?;
        let options = bounds("composer-options")?;
        let right = surface.x + surface.width - send.x - send.width;
        let bottom = surface.y + surface.height - send.y - send.height;
        let left = options.x - surface.x;
        anyhow::ensure!(
            (right - bottom).abs() < 0.01 && (left - bottom).abs() < 0.01,
            "composer action insets differ: left={left}, right={right}, bottom={bottom}"
        );
        anyhow::ensure!(
            (zork_ui::design::CUE_UI.composer.surface_radius - send.width / 2. - right).abs()
                < 0.01,
            "send button is not concentric with the composer corner"
        );
        Ok(())
    };
    check_action_insets()?;
    let composer = bounds("composer-input")?;
    let idle = bounds("composer-member-presence-0")?;
    anyhow::ensure!(
        idle.y < composer.y && composer.y - idle.y < 90.,
        "member not near composer"
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("header-member-")),
        "old header avatars remain"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("idle.png"))?;
    let before_motion = cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    Arc::make_mut(&mut state.participants)[0].activity = Some(AgentStatus::Thinking);
    conversation.seed(state.clone());
    let motion_start = frame_positions.borrow().len();
    pump(&mut cx, 5)?;
    let distinct_frames = frame_positions.borrow()[motion_start..]
        .windows(2)
        .filter(|pair| (pair[1] - pair[0]).abs() > 0.001)
        .count();
    anyhow::ensure!(
        distinct_frames >= 8,
        "120 Hz motion repeated frames: {:?}",
        &frame_positions.borrow()[motion_start..]
    );
    let middle = bounds("composer-member-presence-0")?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "composer-activity-presence-0" && e.bounds.width > 1.),
        "member extended its label while still lifting"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("moving.png"))?;
    pump(&mut cx, 30)?;
    let active = bounds("composer-member-presence-0")?;
    let active_anchor = view.read_with(&cx, |v, _| v.benchmark_frame_state(false).1);
    if let Some(anchor) = requested_anchor {
        anyhow::ensure!(
            active_anchor.abs_diff(anchor) <= 10,
            "activity moved the historical reading anchor: {active_anchor} vs {anchor}"
        );
    }
    anyhow::ensure!(
        active.y < middle.y && middle.y < idle.y,
        "avatar did not travel continuously: {active:?}, {middle:?}, {idle:?}"
    );
    anyhow::ensure!(
        (bounds("composer-input")?.y - composer.y).abs() < 0.5,
        "composer moved during activity"
    );
    Arc::make_mut(&mut state.participants)[1].activity = Some(AgentStatus::Waiting {
        reason: "等待你的确认".into(),
        deadline_ms: 0,
    });
    Arc::make_mut(&mut state.participants)[2].activity = Some(AgentStatus::Failed {
        reason: "连接中断，请重试".into(),
    });
    conversation.seed(state.clone());
    pump(&mut cx, 60)?;
    cx.capture_screenshot(window.into())?
        .save(output.join("multiple.png"))?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .filter(|e| e.id.starts_with("composer-activity-"))
            .count()
            == 3,
        "parallel activity missing"
    );
    let mut right_padding = Vec::new();
    for (i, label) in [
        "Atlas · 思考中",
        "Nova · 等待中 · 等待你的确认",
        "Echo · 执行失败 · 连接中断，请重试",
    ]
    .into_iter()
    .enumerate()
    {
        let text_width = cx.update_window(window.into(), |_, w, _| {
            let run = gpui::TextRun {
                len: label.len(),
                font: gpui::font("Inter Variable"),
                color: gpui::rgb(0).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            w.text_system()
                .shape_line(label.into(), px(12.), &[run], None)
                .width
                .as_f32()
        })?;
        let padding = bounds(&format!("composer-activity-presence-{i}"))?.width - text_width + 4.8;
        anyhow::ensure!(
            (padding - 12.).abs() < 1.,
            "activity right padding varies with text: {label}: {padding}"
        );
        right_padding.push(padding);
    }
    let mut motion_draws =
        cx.update_window(window.into(), |_, w, _| w.frame_duration_snapshot())?;
    motion_draws
        .draw_duration_histogram
        .subtract(&before_motion.draw_duration_histogram)?;
    let p95 = motion_draws.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6;
    let p99 = motion_draws.draw_duration_histogram.value_at_quantile(0.99) as f64 / 1e6;
    // The shared material owns settling, including residual particle motion.
    // Wait for its explicit idle state, bounded to three additional seconds;
    // the subsequent no-work assertions still run for a full 24 display ticks.
    let mut settling_frames = 0;
    while view.read_with(&cx, |v, _| {
        v.benchmark_composer_material()["moving"] == true
    }) && settling_frames < 180
    {
        pump(&mut cx, 1)?;
        settling_frames += 1;
    }
    anyhow::ensure!(
        view.read_with(&cx, |v, _| v.benchmark_composer_material()["moving"]
            == false),
        "composer material did not settle within the bounded transition: {}",
        view.read_with(&cx, |v, _| v.benchmark_composer_material())
    );
    // Commit the retained region's final cached layout after its material
    // reaches rest; the following display ticks must perform no more work.
    pump(&mut cx, 1)?;
    let stable = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    #[cfg(target_os = "macos")]
    let stable_gpu = zork_ui::components::liquid_composer::gpu_stats().0;
    pump(&mut cx, 12)?;
    #[cfg(target_os = "macos")]
    anyhow::ensure!(
        stable_gpu == zork_ui::components::liquid_composer::gpu_stats().0,
        "stationary liquid field dispatches GPU work"
    );
    let after = view.update(&mut cx, |v, cx| v.benchmark_region_counts(cx));
    anyhow::ensure!(
        stable == after,
        "steady activity continuously redraws regions: before={stable:?}, after={after:?}, material={}",
        view.read_with(&cx, |v, _| v.benchmark_composer_material())
    );
    for (detail, expected) in [
        (
            zork_client_core::activity::Detail::Action {
                text: "读取图例".into(),
            },
            "Atlas · 读取图例",
        ),
        (
            zork_client_core::activity::Detail::Requesting,
            "Atlas · 请求中",
        ),
        (
            zork_client_core::activity::Detail::Streaming {
                tokens_per_second: 42,
                estimated: true,
            },
            "Atlas · ≈42 token/s",
        ),
    ] {
        Arc::make_mut(&mut state.participants)[0].activity = Some(AgentStatus::Live {
            presentation: zork_client_core::activity::Presentation { detail },
        });
        conversation.seed(state.clone());
        pump(&mut cx, 40)?;
        anyhow::ensure!(
            driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "composer-activity-presence-0" && e.label == expected),
            "core detail missing: {expected}"
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("core-streaming.png"))?;
    Arc::make_mut(&mut state.participants)[0].activity = Some(AgentStatus::Finished);
    conversation.seed(state.clone());
    pump(&mut cx, 8)?;
    anyhow::ensure!(
        (bounds("composer-member-presence-0")?.y - active.y).abs() < 0.5,
        "idle skipped grace period"
    );
    pump(&mut cx, 95)?;
    anyhow::ensure!(
        (bounds("composer-member-presence-0")?.y - idle.y).abs() < 0.5,
        "idle did not return: initial={idle:?}, current={:?}, material={}",
        bounds("composer-member-presence-0")?,
        view.read_with(&cx, |v, _| v.benchmark_composer_material())
    );
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .filter(|e| e.id.starts_with("composer-activity-"))
            .count()
            == 2,
        "waiting/error disappeared"
    );
    let action = serde_json::from_value(
        json!({"type":"move","target":{"element_id":"composer-member-presence-1"}}),
    )?;
    let records = serde_json::from_value::<Vec<zork_gui::session_history::Record>>(json!([
        {"event_id":"preview-1","event":{"kind":"tool_result","result":{
            "invocation_id":"a","tool":"file.read","outcome":"succeeded",
            "finished_at_ms":1789025400000_i64,"data":{"stdout":"file contents"}}}},
        {"event_id":"preview-2","event":{"kind":"tool_result","result":{
            "invocation_id":"b","tool":"shell.run","outcome":"failed",
            "finished_at_ms":1789025460000_i64,"data":{"error":"测试未通过：缺少配置文件"}}}}
    ]))?;
    view.update(&mut cx, |v, cx| {
        v.benchmark_presence_history(
            "presence-1",
            zork_client_core::state::HistoryData {
                entries: zork_gui::session_history::entries(&records).into(),
                loaded: true,
                runtime: Some(zork_client_core::state::HistoryRuntime {
                    model: Some("GPT-5.4".into()),
                    context_tokens: Some(28400),
                    context_limit: Some(128000),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
        )
    });
    pump(&mut cx, 2)?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 40)?;
    std::fs::write(
        output.join("member-hover-elements.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("member-hover.png"))?;
    let hover = bounds("detail-tooltip-presence-presence-1")?;
    let hovered_avatar = bounds("composer-member-presence-1")?;
    anyhow::ensure!(
        hover.y + hover.height <= hovered_avatar.y - 6.,
        "member popup is not above its avatar"
    );
    anyhow::ensure!(
        hover.x >= 0.
            && hover.y >= 0.
            && hover.x + hover.width <= 1280.
            && hover.y + hover.height <= 800.,
        "member hover escaped viewport"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("member-hover.png"))?;
    // Entering the card must keep it readable after the origin is left.
    let action = serde_json::from_value(
        json!({"type":"move","target":{"element_id":"detail-tooltip-presence-presence-1"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 40)?;
    bounds("detail-tooltip-presence-presence-1")?;
    let action = serde_json::from_value(json!({"type":"move","target":{"x":800,"y":30}}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 40)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("detail-tooltip-presence-")),
        "member hover remained after leaving"
    );
    let action = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"composer-member-presence-1"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 3)?;
    anyhow::ensure!(
        bounds("history-page")?.y < 800.,
        "avatar did not open history directly"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("member-history.png"))?;
    let action = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"conversation-browser"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(gpui::size(px(900.), px(600.)));
        w.bounds_changed(cx);
    })?;
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    cx.update(|cx| cx.set_reduce_motion(true));
    for member in Arc::make_mut(&mut state.participants) {
        member.activity = Some(AgentStatus::Thinking);
    }
    conversation.seed(state);
    pump(&mut cx, 3)?;
    check_action_insets()?;
    let compact = bounds("composer-input")?;
    let action = serde_json::from_value(
        json!({"type":"move","target":{"element_id":"composer-member-presence-2"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 40)?;
    std::fs::write(
        output.join("member-hover-compact-elements.json"),
        serde_json::to_vec_pretty(&driver.snapshot(true))?,
    )?;
    cx.capture_screenshot(window.into())?
        .save(output.join("member-hover-compact.png"))?;
    let hover = bounds("detail-tooltip-presence-presence-2")?;
    anyhow::ensure!(
        hover.x >= 0.
            && hover.y >= 0.
            && hover.x + hover.width <= 900.
            && hover.y + hover.height <= 600.,
        "compact hover escaped viewport"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("member-hover-compact.png"))?;
    let action = serde_json::from_value(json!({"type":"move","target":{"x":800,"y":30}}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 40)?;
    anyhow::ensure!(
        (bounds("composer-member-presence-0")?.y - (compact.y - (composer.y - active.y))).abs()
            < 0.5,
        "reduced-motion endpoint missing"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("compact-reduced.png"))?;
    let core = view.update(&mut cx, |v, _| v.benchmark_core_device());
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("detail-tooltip-presence-")),
        "compact hover remained after leaving"
    );
    let mut idle_state = conversation.snapshot().as_ref().clone();
    for member in Arc::make_mut(&mut idle_state.participants) {
        member.activity = None;
    }
    conversation.seed(idle_state);
    pump(&mut cx, 95)?;
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "0");
    cx.update(|cx| cx.set_reduce_motion(false));
    let move_member = |cx: &mut HeadlessAppContext, member: usize| -> anyhow::Result<()> {
        let action = serde_json::from_value(
            json!({"type":"move","target":{"element_id":format!("composer-member-presence-{member}")}}),
        )?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        Ok(())
    };
    let popup_pose = || -> anyhow::Result<zork_gui::automation::protocol::Rect> {
        let popups: Vec<_> = driver
            .snapshot(false)
            .elements
            .into_iter()
            .filter(|e| e.id.starts_with("detail-tooltip-presence-"))
            .collect();
        anyhow::ensure!(
            popups.len() == 1,
            "member targets did not share one floating surface: {}",
            popups.len()
        );
        Ok(popups[0].bounds.clone())
    };
    move_member(&mut cx, 0)?;
    pump(&mut cx, 30)?;
    let from = popup_pose()?;
    move_member(&mut cx, 2)?;
    pump(&mut cx, 2)?;
    let moving = popup_pose()?;
    anyhow::ensure!(
        moving.x > from.x && moving.x < from.x + 70.,
        "member popup jumped instead of sliding: {from:?} -> {moving:?}"
    );
    // Retarget consecutive display frames. Screenshot readback/PNG encoding
    // uses wall time and can exceed this short animation's entire duration.
    move_member(&mut cx, 0)?;
    pump(&mut cx, 1)?;
    let reversing = popup_pose()?;
    anyhow::ensure!(
        reversing.x <= moving.x + 1. && reversing.x > from.x,
        "member popup reset on reversal: {from:?} -> {moving:?} -> {reversing:?}"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("member-hover-sliding.png"))?;
    pump(&mut cx, 30)?;
    anyhow::ensure!(
        (popup_pose()?.x - from.x).abs() <= 1.,
        "member popup did not settle after reversal"
    );
    std::env::set_var("ZORK_GUI_TEST_REDUCE_MOTION", "1");
    cx.update(|cx| cx.set_reduce_motion(true));
    move_member(&mut cx, 2)?;
    pump(&mut cx, 1)?;
    let reduced_popup = popup_pose()?;
    let target = bounds("composer-member-presence-2")?;
    anyhow::ensure!(
        (reduced_popup.x + reduced_popup.width / 2. - target.x - target.width / 2.).abs() < 1.,
        "reduced-motion member popup did not snap to its avatar"
    );
    let action = serde_json::from_value(json!({"type":"move","target":{"x":800,"y":30}}))?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 30)?;
    std::fs::write(
        output.join("hover-motion.json"),
        serde_json::to_vec_pretty(
            &json!({"from":from,"moving":moving,"reversing":reversing,"reduced":reduced_popup}),
        )?,
    )?;
    let surface = bounds("composer-surface")?;
    // Blank action-row space must focus the editor after focus was elsewhere.
    cx.update_window(window.into(), |_, w, _| w.blur())?;
    for action in [
        json!({"type":"click","target":{"x":surface.x + surface.width / 2.,"y":surface.y + surface.height - 12.}}),
        json!({"type":"type_text","text":"中文输入与 selection"}),
    ] {
        let action = serde_json::from_value(action)?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(&mut cx, 2)?;
    }
    anyhow::ensure!(
        core.draft("render-fixture").text == "中文输入与 selection",
        "composer input lost text: {:?}",
        core.draft("render-fixture").text
    );
    let surface = bounds("composer-surface")?;
    let editor = bounds("composer-input")?;
    let padding_y = (surface.y + editor.y) / 2.;
    anyhow::ensure!(
        padding_y > surface.y && padding_y < editor.y,
        "composer has no top padding"
    );
    for action in [
        json!({"type":"key","keystroke":"cmd-a"}),
        // Clicking top padding preserves both focus and the editor selection.
        json!({"type":"click","target":{"x":surface.x + surface.width / 2.,"y":padding_y}}),
        json!({"type":"type_text","text":"替换"}),
        json!({"type":"key","keystroke":"shift-enter"}),
        json!({"type":"type_text","text":"第二行"}),
    ] {
        let action = serde_json::from_value(action)?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        pump(&mut cx, 2)?;
    }
    anyhow::ensure!(
        core.draft("render-fixture").text == "替换\n第二行",
        "selection or newline failed: {:?}",
        core.draft("render-fixture").text
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("input.png"))?;
    cx.update_window(window.into(), |_, w, cx| {
        w.resize(gpui::size(px(1280.), px(800.)));
        w.bounds_changed(cx);
    })?;
    pump(&mut cx, 3)?;
    anyhow::ensure!(
        bounds("conversation-browser")?.y < 28.,
        "page toggle is not in the top corner"
    );
    let action = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"conversation-browser"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 20)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "page-tab-history"),
        "right page panel did not open"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("right-panel.png"))?;
    let action = serde_json::from_value(
        json!({"type":"click","target":{"element_id":"conversation-browser"}}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
    pump(&mut cx, 20)?;
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "page-tab-history"),
        "right page panel did not close"
    );
    let original_members = conversation.snapshot().participants.as_ref().clone();
    let mut member_counts = Vec::new();
    for count in [4, 7, 2, 400, 3] {
        let mut next = conversation.snapshot().as_ref().clone();
        next.participants = Arc::new(
            (0..count)
                .map(|i| {
                    original_members
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| ParticipantStatus {
                            subscribed: false,
                            assigned: false,
                            id: format!("dynamic-{i}"),
                            name: format!("成员 {i}"),
                            avatar: Some("cat".into()),
                            session_id: "fixture".into(),
                            activity: None,
                        })
                })
                .collect(),
        );
        conversation.seed(next);
        pump(&mut cx, 3)?;
        let rendered = driver
            .snapshot(false)
            .elements
            .iter()
            .filter(|e| e.id.starts_with("composer-member-"))
            .count();
        anyhow::ensure!(
            conversation.snapshot().participants.len() == count,
            "presentation changed authoritative membership"
        );
        if count < 20 {
            anyhow::ensure!(
                rendered == count,
                "dynamic members missing: {count} -> {rendered}"
            );
        } else {
            anyhow::ensure!(rendered < 64, "offscreen members built {rendered} controls");
        }
        member_counts.push(json!({"core": count, "rendered": rendered}));
    }
    #[cfg(target_os = "macos")]
    let (gpu_dispatches, gpu_max_ms) = zork_ui::components::liquid_composer::gpu_stats();
    #[cfg(not(target_os = "macos"))]
    let (gpu_dispatches, gpu_max_ms) = (0_u64, 0_f64);
    #[cfg(target_os = "macos")]
    if std::env::var_os("ZORK_LIQUID_GPU").is_some()
        && std::env::var_os("ZORK_LIQUID_CPU").is_none()
    {
        anyhow::ensure!(gpu_dispatches > 10, "Metal field path was not exercised");
    }
    let coverage = view.read_with(&cx, |v, _| v.benchmark_message_coverage());
    anyhow::ensure!(
        max_rows.get() > 0 && max_rows.get() <= 50,
        "presence broke message virtualization: {} rows",
        max_rows.get()
    );
    std::fs::write(
        output.join("checks.json"),
        serde_json::to_vec_pretty(
            &json!({"message_count":message_count,"initial_anchor":initial_anchor,"active_anchor":active_anchor,"max_rendered_rows":max_rows.get(),"coverage":coverage,"right_padding_px":right_padding,"gpu_dispatches":gpu_dispatches,"gpu_max_ms":gpu_max_ms,"display_tick_hz":120,"moving_frame_changes":distinct_frames,"animation_p95_draw_ms":p95,"animation_p99_draw_ms":p99,"idle":idle,"intermediate":middle,"active":active,"composer":composer,"stable_regions":stable,"after_stable":after,"settling_frames":settling_frames,"dynamic_members":member_counts,"checks":["animated travel","fixed composer","idle grace","idle return","parallel agents","persistent waiting and failure","avatar opens history directly","member hover with history excerpt","hoverable card and dismissal","compact hover bounds","compact layout","reduced motion","no continuous redraw"]}),
        )?,
    )?;
    anyhow::ensure!(
        p95 <= 1000. / 120.,
        "presence animation exceeded CPU frame budget: {p95:.2} ms"
    );
    println!("PASS composer presence motion, lifecycle, geometry and idle redraw checks");
    Ok(())
}
