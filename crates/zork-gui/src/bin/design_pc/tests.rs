use super::*;

static PLATFORM: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn interactive_selectors_keep_the_catalog_and_batch_exports_filter_it() {
    let all = stories::catalog();
    for (family, story, expected) in [
        (Some("history"), None, "history-collapsed"),
        (None, Some("history-expanded"), "history-expanded"),
        (Some("history"), Some("history-narrow"), "history-collapsed"),
        (None, Some("conversation-composer-compact"), "conversation-composer"),
        (Some("icons-brand"), None, "providers-all"),
    ] {
        let (catalog, selected) = launch_catalog(all.clone(), family, story, false).unwrap();
        assert_eq!(catalog.len(), all.len());
        assert_eq!(catalog[selected].id, expected);
        assert!(catalog.iter().any(|s| s.family == "button"));
    }
    let (export, _) = launch_catalog(all.clone(), Some("history"), None, true).unwrap();
    assert!(export.len() > 1 && export.iter().all(|s| s.family == "history"));
    let (export, _) = launch_catalog(all.clone(), None, Some("history-narrow"), true).unwrap();
    assert_eq!(export.len(), 1);
    assert_eq!(export[0].width, 320.);
    let (export, _) =
        launch_catalog(all.clone(), None, Some("conversation-composer-wide"), true).unwrap();
    assert_eq!(export[0].width, 1280.);
    assert!(launch_catalog(all.clone(), Some("missing"), None, false).is_err());
    assert!(launch_catalog(all, Some("button"), Some("history-narrow"), false).is_err());
}

#[test]
fn catalog_is_grouped_and_free_of_retired_stories() {
    let all = stories::catalog();
    assert!(all.iter().all(|s| stories::GROUPS.contains(&s.group.as_str())));
    for retired in [
        "components",
        "unified-design",
        "interaction",
        "member-status",
        "tooltip",
    ] {
        assert!(all.iter().all(|s| s.family != retired), "{retired}");
    }
    assert!(all.iter().all(|s| !s.state.ends_with("-compact") && !s.state.ends_with("-wide")));
    assert!(all.iter().all(|s| s.id != "history-details-agent"));
    // Stories of one sidebar entry stay together.
    let mut seen = vec![];
    for story in &all {
        if seen.last() != Some(&story.entry) {
            assert!(!seen.contains(&story.entry), "{} is split", story.entry);
            seen.push(story.entry.clone());
        }
    }
}

#[test]
fn checks_flag_small_and_clipped_controls() {
    let element = |id: &str, w: f64, visible: f64| {
        json!({"id":id,"role":"button","visible":true,
            "bounds":{"x":0,"y":0,"width":w,"height":28},
            "visible_bounds":{"x":0,"y":0,"width":visible,"height":28}})
    };
    assert!(checks(&[element("ok", 32., 32.)]).is_empty());
    assert_eq!(checks(&[element("tiny", 16., 16.)]).len(), 1);
    assert_eq!(checks(&[element("cut", 80., 40.)]).len(), 1);
}

fn settle(app: &mut HeadlessAppContext, window: gpui::AnyWindowHandle) -> anyhow::Result<()> {
    // Initial fixture gestures and menu focus handoff run on actual frame callbacks.
    for _ in 0..4 {
        app.update_window(window, |_, w, cx| w.simulate_next_frame(cx))?;
        app.run_until_parked();
    }
    Ok(())
}

#[test]
fn physical_navigation_preserves_edits_and_reset_is_local() {
    let _serial = PLATFORM.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = app.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let catalog: Vec<_> = stories::catalog()
        .into_iter()
        .filter(|s| {
            matches!(
                s.id.as_str(),
                "field-empty" | "history-collapsed" | "button-primary" | "button-focus"
            )
        })
        .collect();
    let initial = catalog.iter().position(|s| s.id == "field-empty").unwrap();
    let mut root = None;
    let window = app
        .open_window(size(px(1320.), px(860.)), |_, cx| {
            let gallery = cx.new(|cx| Gallery::new(catalog, initial, driver.clone(), false, cx));
            root = Some(gallery.clone());
            cx.new(|_| AutomationRoot::new(gallery))
        })
        .unwrap();
    let root = root.unwrap();
    let window = window.into();
    let act = |app: &mut HeadlessAppContext, action: Value| {
        app.update_window(window, |_, w, cx| {
            driver.dispatch(serde_json::from_value(action).unwrap(), w, cx)
        })
        .unwrap()
        .unwrap();
        settle(app, window).unwrap();
    };
    let click = |app: &mut HeadlessAppContext, id: &str| {
        act(app, json!({"type":"click","target":{"element_id":id}}));
    };
    let text = |app: &HeadlessAppContext| {
        root.read_with(app, |v, cx| {
            v.sessions["field"].host.read(cx).inspect(cx)["text"]
                .as_str()
                .unwrap()
                .to_owned()
        })
    };
    settle(&mut app, window).unwrap();
    click(&mut app, "story-field");
    act(
        &mut app,
        json!({"type":"type_text","text":"保留 Native 123"}),
    );
    assert_eq!(text(&app), "保留 Native 123");
    click(&mut app, "design-state-field-empty");
    assert_eq!(text(&app), "保留 Native 123", "reselecting must not reset");
    click(&mut app, "design-entry-history");
    click(&mut app, "design-state-history-collapsed");
    click(&mut app, "design-entry-field");
    click(&mut app, "design-state-field-empty");
    assert_eq!(text(&app), "保留 Native 123");
    click(&mut app, "design-width-360");
    assert_eq!(
        text(&app),
        "保留 Native 123",
        "resize must not recreate the editor"
    );
    let canvas = driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "story-canvas")
        .unwrap();
    assert_eq!(canvas.bounds.width, 360.);
    click(&mut app, "story-reset");
    assert_eq!(text(&app), "");
    click(&mut app, "design-entry-button");
    click(&mut app, "design-state-button-focus");
    let focus = root.read_with(&app, |v, cx| {
        v.session().host.read(cx).specimen_focus(cx).unwrap()
    });
    assert!(
        app.update_window(window, |_, w, _| focus.is_focused(w))
            .unwrap(),
        "specimen lost focus before Enter"
    );
    act(&mut app, json!({"type":"key", "keystroke":"enter"}));
    assert_eq!(
        root.read_with(&app, |v, cx| v.session().host.read(cx).inspect(cx)
            ["clicks"]
            .clone()),
        1,
        "focus fixtures must focus their own control, not the browser"
    );
}

#[test]
fn focused_component_is_revealed_and_other_components_remain_reachable() {
    let _serial = PLATFORM.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = app.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let (catalog, initial) =
        launch_catalog(stories::catalog(), Some("notifications"), None, false).unwrap();
    let window = app
        .open_window(size(px(900.), px(600.)), |_, cx| {
            let gallery = cx.new(|cx| Gallery::new(catalog, initial, driver.clone(), false, cx));
            cx.new(|_| AutomationRoot::new(gallery))
        })
        .unwrap()
        .into();
    settle(&mut app, window).unwrap();
    assert!(
        driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == "design-entry-notifications" && e.visible),
        "launching a component below the fold must reveal its directory row"
    );
    let act = |app: &mut HeadlessAppContext, action: Value| {
        app.update_window(window, |_, w, cx| {
            driver.dispatch(serde_json::from_value(action).unwrap(), w, cx)
        })
        .unwrap()
        .unwrap();
        settle(app, window).unwrap();
    };
    act(
        &mut app,
        json!({"type":"scroll", "target":{"element_id":"design-directory"}, "delta_y":10000}),
    );
    act(
        &mut app,
        json!({"type":"click", "target":{"element_id":"design-entry-button"}}),
    );
    act(
        &mut app,
        json!({"type":"click", "target":{"element_id":"design-state-button-primary"}}),
    );
    assert!(driver
        .snapshot(false)
        .elements
        .iter()
        .any(|e| e.id == "story-button" && e.visible));
}
