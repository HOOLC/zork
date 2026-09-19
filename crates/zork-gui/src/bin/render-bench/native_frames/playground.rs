//! Shared native dialog and scroll workloads, driven through real UI input.
use super::*;
use zork_ui::liquid_story::Gallery;

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
            "playground did not settle"
        );
        cx.background_executor()
            .timer(Duration::from_millis(2))
            .await;
    }
    Ok(())
}
async fn reveal(
    window: AnyWindowHandle,
    driver: &HeadlessAutomation,
    config: &Config,
    id: &str,
    position: [f32; 2],
    origin: Instant,
    cx: &mut AsyncApp,
) -> anyhow::Result<()> {
    for _ in 0..32 {
        if driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == id && e.visible)
        {
            return Ok(());
        }
        frame(
            window,
            cx,
            driver,
            action(
                json!({"type":"scroll","target":{"x":position[0],"y":position[1]},"delta_y":-240.}),
            ),
            origin,
            config.offscreen,
        )
        .await?;
        settle(window, driver, config, origin, cx).await?;
    }
    anyhow::bail!("physical scrolling did not reveal {id}")
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
        .playground_scroll
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
    gallery: Entity<Gallery>,
    driver: &HeadlessAutomation,
    config: &Config,
    output: &Path,
    cx: &mut AsyncApp,
) -> anyhow::Result<Value> {
    let origin = Instant::now();
    settle(window, driver, config, origin, cx).await?;
    let mut cases = Vec::new();
    let page = [
        config.liquid_viewport[0] - 50.,
        config.liquid_viewport[1] * 0.55,
    ];
    cases.push(
        scroll(
            window,
            driver,
            config,
            "page-scroll",
            "liquid-demo",
            page,
            origin,
            cx,
        )
        .await?,
    );
    window
        .update(cx, |_, window, _| window.render_to_image())??
        .save(output.join("playground-page-scroll.png"))?;
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"scroll","target":{"x":page[0],"y":page[1]},"delta_y":10000.})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"click","target":{"element_id":"liquid-library-toggle"}})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    let state = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
    let panel = &state["dialog"]["pose"];
    let directory = [
        (panel["cx"].as_f64().unwrap() + panel["w"].as_f64().unwrap() / 2. - 36.) as f32,
        panel["cy"].as_f64().unwrap() as f32,
    ];
    cases.push(
        scroll(
            window,
            driver,
            config,
            "directory-scroll",
            "liquid-section-components",
            directory,
            origin,
            cx,
        )
        .await?,
    );
    window
        .update(cx, |_, window, _| window.render_to_image())??
        .save(output.join("playground-directory-scroll.png"))?;
    reveal(
        window,
        driver,
        config,
        "liquid-tab-7",
        directory,
        origin,
        cx,
    )
    .await?;
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"click","target":{"element_id":"liquid-tab-7"}})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    frame(
        window,
        cx,
        driver,
        action(json!({"type":"scroll","target":{"x":page[0],"y":page[1]},"delta_y":10000.})),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    reveal(
        window,
        driver,
        config,
        "liquid-dialog-trigger",
        page,
        origin,
        cx,
    )
    .await?;
    let trigger = bounds(driver, "liquid-dialog-trigger").expect("revealed dialog");
    frame(
        window,
        cx,
        driver,
        action(
            json!({"type":"scroll","target":{"x":page[0],"y":page[1]},"delta_y":434.-trigger.y}),
        ),
        origin,
        config.offscreen,
    )
    .await?;
    settle(window, driver, config, origin, cx).await?;
    for index in 0..config.panel_pairs * 2 {
        let open = index % 2 == 0;
        let mut input = Some(if open {
            action(json!({"type":"click","target":{"element_id":"liquid-dialog-trigger"}}))
        } else {
            action(json!({"type":"key","keystroke":"escape"}))
        });
        let begin = Instant::now();
        let mut frames = Vec::new();
        loop {
            if let Some(row) = frame(
                window,
                cx,
                driver,
                input.take().unwrap_or(FrameInput::Idle),
                origin,
                config.offscreen,
            )
            .await?
            {
                frames.push(row);
            }
            let continuing = window.update(cx, |_, window, _| window.has_animation_frames())?;
            if begin.elapsed() >= Duration::from_millis(config.phase_ms) && !continuing {
                break;
            }
            anyhow::ensure!(
                begin.elapsed() < Duration::from_secs(7),
                "dialog specimen did not settle"
            );
            if !continuing {
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        }
        let state = window.update(cx, |_, _, cx| gallery.read(cx).inspect(cx))?;
        let dialog = state["cards"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["kind"] == "dialog")
            .expect("dialog specimen")["primitive"]["dialog"]
            .clone();
        anyhow::ensure!(
            dialog["moving"] == false && dialog["backdropAlpha"] == if open { 1. } else { 0. },
            "dialog motion incomplete: {dialog}"
        );
        let mounted = driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "liquid-dialog-panel");
        anyhow::ensure!(
            mounted == open,
            "dialog input tree does not match its state"
        );
        cases.push(json!({"name":if open {"dialog-open"} else {"dialog-close"},"index":index,"frames":frames,
            "paintOnlyFrames":dialog["paintOnlyFrames"],
            "after":{"open":dialog["open"],"moving":dialog["moving"],"backdropAlpha":dialog["backdropAlpha"],"mounted":mounted}}));
        if index < 2 {
            window
                .update(cx, |_, window, _| window.render_to_image())??
                .save(output.join(if open {
                    "playground-dialog-open.png"
                } else {
                    "playground-dialog-closed.png"
                }))?;
        }
    }
    let viewport = window.update(cx, |_, window, _| {
        [
            window.viewport_size().width.as_f32(),
            window.viewport_size().height.as_f32(),
        ]
    })?;
    Ok(
        json!({"status":"measured","viewport":viewport,"component":"zork-ui::liquid::Dialog","control":"liquid-dialog-panel","cases":cases}),
    )
}
