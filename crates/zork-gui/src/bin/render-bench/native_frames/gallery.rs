//! Shared native dialog and scroll workloads, driven through real UI input.
use super::*;
use zork_ui::component_story::Gallery;

fn action(value: Value) -> FrameInput {
    FrameInput::Action(serde_json::from_value(value).expect("fixture input"))
}
fn bounds(driver: &HeadlessAutomation, id: &str) -> Option<zork_ui::automation::protocol::Rect> {
    driver
        .snapshot(true)
        .elements
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.bounds)
}
async fn settle(
    window: AnyWindowHandle,
    driver: &HeadlessAutomation,
    config: &Config,
    origin: Instant,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    let started = Instant::now();
    loop {
        frame(
            window,
            cx,
            driver,
            FrameInput::Idle,
            origin,
            config.offscreen,
        )
        .await?;
        if !window.update(cx, |_, window, _| window.has_animation_frames())? {
            break;
        }
        anyhow::ensure!(
            started.elapsed() < Duration::from_secs(7),
            "component gallery did not settle"
        );
        cx.background_executor()
            .timer(Duration::from_millis(2))
            .await;
    }
    Ok(())
}
async fn scroll(
    window: AnyWindowHandle,
    driver: &HeadlessAutomation,
    config: &Config,
    name: &str,
    marker: &str,
    position: [f32; 2],
    origin: Instant,
    cx: &mut AsyncApp,
) -> anyhow::Result<Value> {
    let settings = config
        .gallery_scroll
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing approved playground scroll fixture"))?;
    anyhow::ensure!(
        settings.legs > 0
            && settings.legs % 2 == 1
            && settings.px_per_second.is_finite()
            && settings.px_per_second > 0.
            && settings.min_displacement_px.is_finite()
            && settings.min_displacement_px > 0.,
        "invalid playground scroll fixture"
    );
    let leg_ms = config.phase_ms / u64::from(settings.legs);
    anyhow::ensure!(
        leg_ms > 0,
        "scroll phase is shorter than its direction legs"
    );
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"move","target":{"x":position[0],"y":position[1]}})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    let before = bounds(driver, marker);
    anyhow::ensure!(before.is_some(), "missing scroll marker {marker}");
    let begin = Instant::now();
    let mut previous = begin;
    let mut frames = Vec::new();
    let mut previous_leg = 0;
    let mut direction_changes = 0;
    // Alternate before either viewport reaches its end. An odd number of
    // legs leaves a measurable offset; every event follows the real wheel path.
    while begin.elapsed() < Duration::from_millis(config.phase_ms) {
        let now = Instant::now();
        let dt = now
            .duration_since(previous)
            .as_secs_f32()
            .clamp(0.0001, 0.05);
        previous = now;
        let leg = (begin.elapsed().as_millis() as u64 / leg_ms).min(u64::from(settings.legs - 1));
        if leg != previous_leg {
            direction_changes += 1;
            previous_leg = leg;
        }
        let direction = if leg % 2 == 0 { -1. } else { 1. };
        if let Some(row) = frame(
            window,
            cx,
            driver,
            FrameInput::ScrollAt(position, direction * settings.px_per_second * dt),
            origin,
            config.offscreen,
        )
        .await?
        {
            frames.push(row);
        }
    }
    let after = bounds(driver, marker);
    anyhow::ensure!(
        direction_changes == settings.legs - 1,
        "{name} skipped scroll direction legs"
    );
    anyhow::ensure!(
        after.is_some_and(|a| (a.y - before.unwrap().y).abs() > settings.min_displacement_px),
        "{name} did not move visible content: {before:?} -> {after:?}"
    );
    Ok(
        json!({"name":name,"frames":frames,"marker":marker,"before":before,"after":after,"scrollSpeed":settings.px_per_second,"directionChanges":direction_changes}),
    )
}

pub(super) async fn measure(
    window: AnyWindowHandle,
    _: Entity<Gallery>,
    driver: &HeadlessAutomation,
    config: &Config,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<Value> {
    let origin = Instant::now();
    settle(window, driver, config, origin, cx).await?;
    let page = [
        config.component_viewport[0] - 50.,
        config.component_viewport[1] * 0.55,
    ];
    let page_case = scroll(
        window,
        driver,
        config,
        "page-scroll",
        "component-gallery-row-10",
        page,
        origin,
        cx,
    )
    .await?;
    window
        .update(cx, |_, window, _| window.render_to_image())??
        .save(output.join("gallery-page-scroll.png"))?;
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"click","target":{"element_id":"component-gallery-toggle"}})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    let panel = bounds(driver, "component-gallery-dialog")
        .ok_or_else(|| anyhow::anyhow!("component directory dialog was not mounted"))?;
    let directory = [panel.x + panel.width - 36., panel.y + panel.height / 2.];
    let directory_case = scroll(
        window,
        driver,
        config,
        "directory-scroll",
        "component-gallery-directory-row-10",
        directory,
        origin,
        cx,
    )
    .await?;
    window
        .update(cx, |_, window, _| window.render_to_image())??
        .save(output.join("gallery-directory-scroll.png"))?;
    let viewport = window.update(cx, |_, window, _| {
        [
            window.viewport_size().width.as_f32(),
            window.viewport_size().height.as_f32(),
        ]
    })?;
    Ok(json!({
        "status":"measured", "viewport":viewport,
        "component":"zork-ui::component_story::Gallery",
        "cases":[page_case, directory_case],
    }))
}
