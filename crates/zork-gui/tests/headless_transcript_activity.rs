//! Live Session activity is the transcript's last item: it follows the latest
//! message inside the scrolling list, stays clear of the composer overlay,
//! follows the tail while it grows, never moves a reader who scrolled up, and
//! leaves once the round ends with its final message.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::automation::protocol::ElementInfo;
use zork_gui::{
    api::{AgentStatus, ParticipantStatus, Role},
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
    views::{RootView, TranscriptLine},
};

fn record(id: &str, tool: &str, at: i64, finished: bool) -> serde_json::Value {
    if finished {
        json!({"event_id":format!("{id}-done"),"event":{"kind":"tool_result","result":{
            "invocation_id":id,"tool":tool,"outcome":"succeeded",
            "finished_at_ms":at,"data":{}}}})
    } else {
        json!({"event_id":format!("{id}-start"),"event":{"kind":"step_completed",
            "step_id":format!("step-{id}"),"completed_at_ms":at,"invocations":[{
            "invocation_id":id,"tool":tool,"started_at_ms":at,
            "arguments":{"path":format!("src/{id}.rs")}}]}})
    }
}

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
    // A short Chat whose rows are all measured, so scrolling to the end and
    // tail following use real heights, as in an ordinary conversation.
    view.update(&mut cx, |v, cx| {
        v.benchmark_replace_messages(
            (0..8)
                .map(|i| TranscriptLine::Message {
                    role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                    content: format!(
                        "消息 {i}\n\n会话动态跟在最新消息后面，随消息一起滚动。\n\n第二段正文，让列表可以滚动。"
                    ),
                    metadata: serde_json::from_value(json!({"id":format!("row-{i}")})).unwrap(),
                })
                .collect(),
            cx,
        )
    });
    let conversation = view.update(&mut cx, |v, cx| v.benchmark_bind_core(cx));
    let mut state = conversation.snapshot().as_ref().clone();
    state.connected = true;
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
    let pump = |cx: &mut HeadlessAppContext, frames: usize| -> anyhow::Result<()> {
        for _ in 0..frames {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        Ok(())
    };
    pump(&mut cx, 8)?;
    view.update(&mut cx, |v, cx| v.benchmark_follow_messages(cx));
    pump(&mut cx, 8)?;
    let history = view
        .update(&mut cx, |v, _| v.benchmark_core_device())
        .conversation("fixture")
        .history();
    let t = 1_789_025_400_000_i64;
    let mut records = vec![
        record("read-1", "file.read", t, false),
        record("read-1", "file.read", t + 400, true),
        record("write-1", "file.write", t + 800, false),
    ];
    let seed = |records: &[serde_json::Value]| -> anyhow::Result<()> {
        history.seed(zork_client_core::state::HistoryData {
            records: serde_json::from_value::<Vec<zork_gui::session_history::Record>>(json!(
                records
            ))?
            .into(),
            loaded: true,
            ..Default::default()
        });
        Ok(())
    };
    seed(&records)?;
    pump(&mut cx, 12)?;

    let elements = || driver.snapshot(false).elements;
    let find = |elements: &[ElementInfo], id: &str| {
        elements.iter().find(|e| e.id == id && e.visible).cloned()
    };
    let following =
        |cx: &mut HeadlessAppContext| view.update(cx, |v, _| v.benchmark_message_motion().2);
    let len = view.update(&mut cx, |v, _| v.benchmark_transcript_len());

    // 1. One activity, the trailing item of the transcript, after the last
    //    message and above the composer overlay.
    let snapshot = elements();
    let members = snapshot
        .iter()
        .filter(|e| e.id == "session-activity-member")
        .count();
    anyhow::ensure!(members == 1, "expected one activity, found {members}");
    anyhow::ensure!(
        snapshot
            .iter()
            .all(|e| !e.id.starts_with("composer-member-")
                && !e.id.starts_with("composer-activity-")
                && e.id != "participant-activity"),
        "activity was duplicated outside the transcript"
    );
    let member = find(&snapshot, "session-activity-member")
        .ok_or_else(|| anyhow::anyhow!("activity is not visible at the tail"))?;
    let composer = find(&snapshot, "composer-surface")
        .ok_or_else(|| anyhow::anyhow!("composer surface missing"))?;
    let transcript = find(&snapshot, "conversation-transcript")
        .ok_or_else(|| anyhow::anyhow!("transcript missing"))?;
    let (item_top, item_bottom) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len))
        .ok_or_else(|| anyhow::anyhow!("trailing transcript item was not laid out"))?;
    let (_, last_bottom) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len - 1))
        .ok_or_else(|| anyhow::anyhow!("last message was not laid out"))?;
    anyhow::ensure!(
        member.bounds.y >= item_top - 0.5
            && member.bounds.y + member.bounds.height <= item_bottom + 0.5,
        "activity is not inside the transcript's trailing item: member={:?}, item={item_top}..{item_bottom}",
        member.bounds
    );
    anyhow::ensure!(
        (item_top - last_bottom).abs() < 0.5,
        "activity does not directly follow the last message: last={last_bottom}, activity={item_top}"
    );
    anyhow::ensure!(
        member.bounds.y >= transcript.bounds.y && item_bottom <= composer.bounds.y + 0.5,
        "activity hides behind the composer: item bottom {item_bottom}, composer top {}",
        composer.bounds.y
    );
    anyhow::ensure!(following(&mut cx), "fixture is not following the tail");
    println!("PASS activity is the last transcript item, once, clear of the composer");

    // 2. Growing while following keeps the grown row in view.
    let click = |cx: &mut HeadlessAppContext, id: &str| -> anyhow::Result<()> {
        let action = serde_json::from_value(json!({"type":"click","target":{"element_id":id}}))?;
        cx.update_window(window.into(), |_, w, cx| driver.dispatch(action, w, cx))??;
        Ok(())
    };
    click(&mut cx, "session-activity-expand")?;
    pump(&mut cx, 20)?;
    records.push(record("write-1", "file.write", t + 1200, true));
    records.push(record("shell-1", "shell.run", t + 1500, false));
    seed(&records)?;
    pump(&mut cx, 20)?;
    let snapshot = elements();
    let link = find(&snapshot, "session-activity-history")
        .ok_or_else(|| anyhow::anyhow!("expanded activity is not in view"))?;
    let composer = find(&snapshot, "composer-surface").unwrap();
    let (grown_top, grown_bottom) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len))
        .unwrap();
    anyhow::ensure!(
        grown_bottom - grown_top > item_bottom - item_top + 40.,
        "activity did not grow: {}..{} vs {item_top}..{item_bottom}",
        grown_top,
        grown_bottom
    );
    anyhow::ensure!(
        link.bounds.y + link.bounds.height <= composer.bounds.y + 0.5
            && grown_bottom <= composer.bounds.y + 0.5,
        "grown activity slid under the composer: row bottom {grown_bottom}, composer {}",
        composer.bounds.y
    );
    anyhow::ensure!(following(&mut cx), "growth stopped tail following");
    println!("PASS following keeps the growing activity in view");

    // 3. The activity scrolls with the messages.
    let before = find(&elements(), "session-activity-member").unwrap();
    let wheel = serde_json::from_value(
        json!({"type":"scroll","target":{"element_id":"conversation-transcript"},"delta_y":300}),
    )?;
    cx.update_window(window.into(), |_, w, cx| driver.dispatch(wheel, w, cx))??;
    pump(&mut cx, 12)?;
    anyhow::ensure!(!following(&mut cx), "scrolling up kept tail following");
    let moved = find(&elements(), "session-activity-member");
    anyhow::ensure!(
        moved
            .as_ref()
            .is_none_or(|m| m.bounds.y > before.bounds.y + 100.),
        "activity stayed pinned while the list scrolled: before {:?}, after {:?}",
        before.bounds,
        moved.map(|m| m.bounds)
    );
    println!("PASS activity scrolls with the list");

    // 4. A reader who scrolled up is not moved by activity changes or by
    //    a new message; the new-message affordance appears as before.
    let reading = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    records.push(record("shell-1", "shell.run", t + 2500, true));
    records.push(record("edit-1", "file.edit", t + 2600, false));
    seed(&records)?;
    pump(&mut cx, 20)?;
    let held = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    anyhow::ensure!(
        reading.1 == held.1 && (reading.2 - held.2).abs() < 0.5 && !following(&mut cx),
        "activity growth moved the reader: {reading:?} -> {held:?}"
    );
    conversation.seed_event(&zork_client_core::api::SseEvent {
        name: "message".into(),
        data: json!({"type":"message","id":"while-reading","role":"assistant",
            "content":"读历史时到达的新消息。"})
        .to_string(),
    });
    pump(&mut cx, 20)?;
    let held = view.update(&mut cx, |v, _| v.benchmark_frame_state(false));
    anyhow::ensure!(
        reading.1 == held.1 && (reading.2 - held.2).abs() < 0.5,
        "a new message moved the reader: {reading:?} -> {held:?}"
    );
    anyhow::ensure!(
        find(&elements(), "messages-new").is_some(),
        "new-message affordance missing while reading"
    );
    println!("PASS scrolled-up reading position is kept");

    // Back to the tail: the activity sits after the newest message.
    click(&mut cx, "messages-new")?;
    pump(&mut cx, 30)?;
    let len = view.update(&mut cx, |v, _| v.benchmark_transcript_len());
    let (_, last_bottom) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len - 1))
        .unwrap();
    let (tail_top, _) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len))
        .ok_or_else(|| anyhow::anyhow!("activity not laid out after returning to the tail"))?;
    anyhow::ensure!(
        (tail_top - last_bottom).abs() < 0.5
            && find(&elements(), "session-activity-member").is_some(),
        "activity is not after the newest message: last {last_bottom}, activity {tail_top}"
    );
    anyhow::ensure!(
        following(&mut cx),
        "returning to the tail did not resume following"
    );

    // 5. The final message ends the round: the activity leaves the list.
    let mut state = conversation.snapshot().as_ref().clone();
    let mut participants = state.participants.as_ref().clone();
    participants[0].activity = Some(AgentStatus::Finished);
    state.participants = Arc::new(participants);
    conversation.seed(state);
    conversation.seed_event(&zork_client_core::api::SseEvent {
        name: "message".into(),
        data: json!({"type":"message","id":"final-reply","role":"assistant",
            "content":"改好了，测试都通过。"})
        .to_string(),
    });
    pump(&mut cx, 90)?;
    let snapshot = elements();
    anyhow::ensure!(
        snapshot
            .iter()
            .all(|e| !e.id.starts_with("session-activity") && e.id != "participant-activity"),
        "activity stayed after the final message: {:?}",
        snapshot
            .iter()
            .map(|e| e.id.as_str())
            .filter(|id| id.contains("activity"))
            .collect::<Vec<_>>()
    );
    let len = view.update(&mut cx, |v, _| v.benchmark_transcript_len());
    let trailing = view.update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len));
    anyhow::ensure!(
        trailing.is_none_or(|(top, bottom)| bottom - top < 0.5),
        "empty activity item still takes space: {trailing:?}"
    );
    let composer = find(&snapshot, "composer-surface").unwrap();
    let (_, last_bottom) = view
        .update(&mut cx, |v, _| v.benchmark_transcript_item_bounds(len - 1))
        .ok_or_else(|| anyhow::anyhow!("final message not in view"))?;
    anyhow::ensure!(
        last_bottom <= composer.bounds.y + 0.5,
        "final message is hidden behind the composer"
    );
    println!("PASS activity leaves once the final message arrives");
    Ok(())
}
