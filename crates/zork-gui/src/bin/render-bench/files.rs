//! The delivered-file list is opt-in and independently virtualized.
use gpui::{AnyWindowHandle, Entity, HeadlessAppContext};
use std::{path::Path, time::Duration};
use zork_gui::{
    automation::{protocol::UserAction, HeadlessAutomation},
    views::RootView,
};

fn action(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    driver: &HeadlessAutomation,
    value: serde_json::Value,
) -> anyhow::Result<()> {
    let settle_overlay = value["type"] != "scroll";
    let action: UserAction = serde_json::from_value(value)?;
    cx.update_window(window, |_, w, cx| driver.dispatch(action, w, cx))??;
    cx.run_until_parked();
    if settle_overlay {
        // Shared overlays measure content and publish placement on later frames.
        // Keep these setup frames outside the continuous-scroll measurement.
        for _ in 0..4 {
            cx.advance_clock(Duration::from_millis(16));
            cx.update_window(window, |_, w, cx| w.simulate_next_frame(cx))?;
            cx.run_until_parked();
        }
    }
    Ok(())
}
fn rows(driver: &HeadlessAutomation) -> Vec<String> {
    driver
        .snapshot(false)
        .elements
        .into_iter()
        .filter(|e| {
            e.visible
                && (e.id.starts_with("conversation-artifact-") || e.id.starts_with("content-file-"))
        })
        .map(|e| e.id)
        .collect()
}
fn panel_visible(driver: &HeadlessAutomation) -> bool {
    driver
        .snapshot(false)
        .elements
        .iter()
        .any(|e| e.id == "conversation-files-panel" && e.visible)
}

pub fn verify(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    root: &Entity<RootView>,
    driver: &HeadlessAutomation,
    output: &Path,
    capture: bool,
) -> anyhow::Result<serde_json::Value> {
    anyhow::ensure!(
        rows(driver).is_empty() && !panel_visible(driver),
        "history files must be collapsed by default"
    );
    let page_toggle =
        serde_json::json!({"type":"click","target":{"element_id":"conversation-browser"}});
    let open_pages = !driver
        .snapshot(false)
        .elements
        .iter()
        .any(|e| e.id == "browser-tabs");
    if open_pages {
        action(cx, window, driver, page_toggle.clone())?;
        cx.advance_clock(Duration::from_millis(300));
        cx.update_window(window, |_, w, cx| {
            w.simulate_next_frame(cx);
        })?;
        cx.run_until_parked();
    }
    let open =
        serde_json::json!({"type":"click","target":{"element_id":"conversation-files-button"}});
    action(cx, window, driver, open.clone())?;
    if !panel_visible(driver) {
        std::fs::write(
            output.join("files-open-failure.json"),
            serde_json::to_vec_pretty(&driver.snapshot(false))?,
        )?;
        cx.capture_screenshot(window)?
            .save(output.join("files-open-failure.png"))?;
        anyhow::bail!("file entry did not open the file list");
    }
    let preview = rows(driver);
    anyhow::ensure!(
        !preview.is_empty() && preview.len() <= 3,
        "file preview is not bounded"
    );
    if capture {
        cx.capture_screenshot(window)?
            .save(output.join("files-open.png"))?;
    }
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"click","target":{"element_id":"conversation-files-all"}}),
    )?;
    anyhow::ensure!(!panel_visible(driver), "View all did not close the preview");
    let first = rows(driver);
    anyhow::ensure!(!first.is_empty(), "full file tab is empty");
    if capture {
        cx.capture_screenshot(window)?
            .save(output.join("files-tab.png"))?;
    }
    let before = cx.update_window(window, |_, w, _| w.frame_duration_snapshot())?;
    let mut maximum = 0;
    for _ in 0..60 {
        root.update(cx, |view, _| view.benchmark_begin_frame());
        cx.advance_clock(Duration::from_nanos(1_000_000_000 / super::FPS as u64));
        action(
            cx,
            window,
            driver,
            serde_json::json!({"type":"scroll","target":{"element_id":"conversation-artifacts"},"delta_y":-48}),
        )?;
        maximum = maximum.max(root.read_with(cx, |view, _| view.benchmark_frame_state(false).3));
    }
    let mut after = cx.update_window(window, |_, w, _| w.frame_duration_snapshot())?;
    after
        .draw_duration_histogram
        .subtract(&before.draw_duration_histogram)?;
    let p95 = after.draw_duration_histogram.value_at_quantile(0.95) as f64 / 1e6;
    let last = rows(driver);
    let count = root.read_with(cx, |view, _| {
        view.benchmark_message_coverage()["file_and_image_artifacts"]
            .as_u64()
            .unwrap()
    });
    anyhow::ensure!(count <= 8 || first != last, "file list did not scroll");
    anyhow::ensure!(
        maximum > 0 && maximum <= 18,
        "file list rendered {maximum} rows"
    );
    anyhow::ensure!(
        p95 <= 1000. / super::FPS as f64,
        "file list exceeded CPU frame budget: {p95:.2} ms"
    );
    // A real row click reaches the existing preview, and dismissals restore the conversation.
    let target = last
        .first()
        .ok_or_else(|| anyhow::anyhow!("file rows disappeared"))?;
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"click","target":{"element_id":target}}),
    )?;
    anyhow::ensure!(
        !panel_visible(driver)
            && driver
                .snapshot(false)
                .elements
                .iter()
                .any(|e| e.id == "drive-close-preview"),
        "file click did not open preview"
    );
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"click","target":{"element_id":"drive-close-preview"}}),
    )?;
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"click","target":{"element_id":"page-close-conversation-content-files"}}),
    )?;
    action(cx, window, driver, open.clone())?;
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"key","keystroke":"escape"}),
    )?;
    anyhow::ensure!(!panel_visible(driver), "Escape did not close files");
    action(cx, window, driver, open.clone())?;
    action(cx, window, driver, open.clone())?;
    anyhow::ensure!(!panel_visible(driver), "file entry did not toggle closed");
    action(cx, window, driver, open)?;
    action(
        cx,
        window,
        driver,
        serde_json::json!({"type":"click","target":{"element_id":"browser-address"}}),
    )?;
    anyhow::ensure!(!panel_visible(driver), "outside click did not close files");
    if open_pages {
        action(cx, window, driver, page_toggle)?;
    }
    Ok(
        serde_json::json!({"count":count,"max_rendered_rows":maximum,"p95_draw_ms":p95,"closed_by_default":true,"scroll_verified":count>8,"surface":"native content tab","preview_rows":preview.len(),"checks":["grouped preview","View all","scroll","preview","close tab","Escape","toggle","outside click"]}),
    )
}
