//! Active members have one activity band above the composer, never inside it.
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
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let mut view = None;
    let window = cx.open_window(gpui::size(px(900.), px(700.)), |_, cx| {
        let root = cx.new(|cx| RootView::render_benchmark_fixture(false, store, cx));
        view = Some(root.clone());
        cx.new(|_| AutomationRoot::new(root))
    })?;
    let view = view.unwrap();
    let conversation = view.update(&mut cx, |v, cx| v.benchmark_bind_core(cx));
    let mut state = conversation.snapshot().as_ref().clone();
    state.participants = Arc::new(vec![ParticipantStatus {
        subscribed: false,
        assigned: false,
        id: "atlas".into(),
        name: "Atlas".into(),
        avatar: Some("cat".into()),
        session_id: "fixture".into(),
        activity: Some(AgentStatus::Thinking),
    }]);
    conversation.seed(state);
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..8 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| w.draw(cx).clear(cx))?;
        }
        Ok(())
    };
    pump(&mut cx)?;
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    pump(&mut cx)?;
    let elements = driver.snapshot(false).elements;
    anyhow::ensure!(
        elements.iter().any(|e| e.id == "participant-activity"),
        "live status did not appear while history is unavailable: {:?}",
        elements
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| id.contains("activity") || id.contains("member"))
            .collect::<Vec<_>>()
    );
    anyhow::ensure!(
        elements.iter().any(|e| e.id == "header-member-atlas"),
        "member history entry disappeared with the composer avatar"
    );
    anyhow::ensure!(
        elements
            .iter()
            .all(|e| !e.id.starts_with("composer-member-")
                && !e.id.starts_with("composer-activity-")),
        "composer still rendered member activity"
    );
    let history = view
        .update(&mut cx, |v, _| v.benchmark_core_device())
        .conversation("fixture")
        .history();
    let records: Vec<zork_gui::session_history::Record> = serde_json::from_value(json!([
        {"event_id":"current-read","event":{"kind":"tool_result","result":{
            "invocation_id":"read-1","tool":"file.read","outcome":"succeeded",
            "finished_at_ms":1789025400000_i64,"data":{"stdout":"file contents"}}}}
    ]))?;
    history.seed(zork_client_core::state::HistoryData {
        records: records.into(),
        loaded: true,
        ..Default::default()
    });
    pump(&mut cx)?;
    let elements = driver.snapshot(false).elements;
    anyhow::ensure!(
        elements
            .iter()
            .any(|e| e.id == "session-activity-tool:read-1"),
        "Session preview did not replace the fallback status: {:?}",
        elements
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| id.contains("activity") || id.contains("member"))
            .collect::<Vec<_>>()
    );
    anyhow::ensure!(
        elements.iter().all(|e| e.id != "participant-activity"
            && !e.id.starts_with("composer-member-")
            && !e.id.starts_with("composer-activity-")),
        "member activity was rendered twice"
    );
    let open = serde_json::from_value(json!({
        "type": "click", "target": {"element_id": "header-member-atlas"}
    }))?;
    cx.update_window(window.into(), |_, window, cx| {
        driver.dispatch(open, window, cx)
    })??;
    pump(&mut cx)?;
    anyhow::ensure!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|element| element.id == "history-page"),
        "toolbar avatar did not open the member's Session History"
    );
    println!("PASS transcript activity replaces fallback without composer duplicates");
    Ok(())
}
