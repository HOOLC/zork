//! Mesh display names in the real desktop: every surface shows the short
//! display name from core with the machine name as its hint, and the device
//! page renames it in place (Enter saves, Esc cancels) with validation.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use zork_config::membership::{MeshDevice, MeshGroup, MeshNames};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
};

const STUDIO: &str = "zuozijians-Mac-Studio";
const AIR: &str = "zuozijiandeMacBook-Air";

fn key(c: char) -> String {
    format!("key:{}", c.to_string().repeat(52))
}
fn device(c: char, name: &str) -> MeshDevice {
    MeshDevice {
        origin: key(c),
        name: name.into(),
        addr: None,
        routes: None,
    }
}

fn main() -> anyhow::Result<()> {
    let output =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/device-names");
    std::fs::create_dir_all(&output)?;
    if std::env::args().any(|arg| arg == "--desktop") {
        return desktop(&output);
    }
    let directory = tempfile::tempdir()?;
    let services = directory.path().join("services.json");
    std::fs::write(
        &services,
        serde_json::to_vec(
            &json!({"relay_urls":["http://127.0.0.1:9"],"discovery_url":"http://127.0.0.1:9/pkarr"}),
        )?,
    )?;
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("--desktop")
        .env("ZORK_CLIENT_DATA", directory.path())
        .env(
            "ZORK_GUI_PREFERENCES_PATH",
            directory.path().join("preferences.json"),
        )
        .env("ZORK_SERVICES_CONFIG", services)
        .env("ZORK_GUI_LOCALE", "zh-CN")
        .status()?;
    anyhow::ensure!(status.success(), "desktop device name regression failed");
    Ok(())
}

/// The Mesh authority's state as the fake Stations report it.
struct Mesh {
    group: MeshGroup,
    names: MeshNames,
    renames: Vec<String>,
}

/// Two Stations of one Mesh behind one listener: `127.0.0.1` is the Studio,
/// `localhost` the MacBook Air (the authority). Renames run through the real
/// `MeshNames` rules, as the authority applies them.
fn stations(mesh: Arc<Mutex<Mesh>>) -> anyhow::Result<u16> {
    use axum::{
        http::{header::HOST, HeaderMap, StatusCode},
        response::{sse, IntoResponse},
        routing::{get, post, put},
        Json, Router,
    };
    fn origin(headers: &HeaderMap) -> String {
        let air = headers
            .get(HOST)
            .and_then(|host| host.to_str().ok())
            .is_some_and(|host| host.starts_with("localhost"));
        key(if air { 'j' } else { 'b' })
    }
    let (port, ready) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let status = mesh.clone();
            let config = mesh.clone();
            let rename = mesh.clone();
            let router = Router::new()
                .route(
                    "/v1/im/events",
                    get(|| async {
                        sse::Sse::new(futures_util::stream::pending::<
                            Result<sse::Event, std::convert::Infallible>,
                        >())
                    }),
                )
                .route(
                    "/v1/mesh",
                    get(move |headers: HeaderMap| {
                        let mesh = status.clone();
                        async move {
                            let mesh = mesh.lock().unwrap();
                            Json(json!({"enabled":true,"origin":origin(&headers),
                                "group":mesh.group,"names":mesh.names,"peers":[]}))
                        }
                    }),
                )
                .route(
                    "/v1/node/mesh",
                    get(move |headers: HeaderMap| {
                        let mesh = config.clone();
                        async move {
                            let mesh = mesh.lock().unwrap();
                            Json(
                                json!({"config":{"enabled":true,"group":mesh.group,"peers":[]},
                                "names":mesh.names,"origin":origin(&headers)}),
                            )
                        }
                    }),
                )
                .route(
                    "/v1/node/info",
                    get(|headers: HeaderMap| async move {
                        let name = if origin(&headers) == key('j') {
                            AIR
                        } else {
                            STUDIO
                        };
                        Json(json!({"name":name,"station":{"version":"0.1.30"}}))
                    }),
                )
                .route(
                    "/v1/node/mesh/clients",
                    post(|| async { Json(json!({"registered":true})) }),
                )
                .route(
                    "/v1/node/mesh/names",
                    put(
                        move |headers: HeaderMap, Json(body): Json<serde_json::Value>| {
                            let mesh = rename.clone();
                            async move {
                                let name = body["name"].as_str().unwrap_or_default().to_owned();
                                let mut mesh = mesh.lock().unwrap();
                                mesh.renames.push(name.clone());
                                // Another device took this name a moment ago.
                                if name == "占用" {
                                    return (
                                        StatusCode::CONFLICT,
                                        Json(json!({"error":
                                    "名称“占用”已被 Mesh 中的另一台设备使用，请换一个名称"})),
                                    )
                                        .into_response();
                                }
                                let Mesh { group, names, .. } = &mut *mesh;
                                match names.rename(group, &origin(&headers), &name) {
                                    Ok(_) => {
                                        Json(json!({"name":name,"names":names})).into_response()
                                    }
                                    Err(error) => (
                                        StatusCode::CONFLICT,
                                        Json(json!({"error":error.to_string()})),
                                    )
                                        .into_response(),
                                }
                            }
                        },
                    ),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            port.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, router).await.unwrap();
        });
    });
    Ok(ready.recv_timeout(Duration::from_secs(10))?)
}

fn desktop(output: &std::path::Path) -> anyhow::Result<()> {
    // mini1 created the Mesh, the Air and the Studio joined, the Air's desktop
    // client and the phone connect as clients. Names are backfilled.
    let group = MeshGroup {
        authority: key('y'),
        revision: 4,
        members: vec![device('y', "mini1"), device('j', AIR), device('b', STUDIO)],
        clients: vec![device('n', &format!("{AIR} 客户端")), device('d', "PLP110")],
    };
    let mut names = MeshNames::new(&group.authority);
    names.reconcile(&group);
    let mesh = Arc::new(Mutex::new(Mesh {
        group,
        names,
        renames: vec![],
    }));
    let port = stations(mesh.clone())?;
    let store =
        zork_gui::desktop::store::ClientStore::open(&zork_client_core::desktop::client_root())?;
    store.set_local_node_enabled(false)?;
    for (id, name, url, origin) in [
        (
            "studio",
            STUDIO,
            format!("http://127.0.0.1:{port}"),
            key('b'),
        ),
        ("air", AIR, format!("http://localhost:{port}"), key('j')),
    ] {
        store.save_node(&serde_json::from_value(
            json!({"id":id,"name":name,"url":url,"local":false}),
        )?)?;
        store.put(id, "mesh-origin", &origin)?;
    }
    // A Mesh-only member this client cannot reach; it also makes startup
    // prepare the client Mesh that device connections depend on.
    store.save_node(&serde_json::from_value(json!({
        "id":"mini1","name":"mini1","url":"","local":false,
        "mesh":{"origin":key('y'),"addr":null},"group":key('y')
    }))?)?;
    // The Mesh membership and display names as the Stations reported them
    // to core (Station propagation has its own tests).
    {
        let mesh = mesh.lock().unwrap();
        store.apply_mesh_names(&mesh.group, Some(&mesh.names))?;
    }
    store.put("device", "last-node", &"studio")?;
    drop(store);

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
    let window = cx.open_window(gpui::size(px(1100.), px(760.)), |_, cx| {
        let desktop = cx.new(zork_gui::desktop::DesktopRoot::new);
        cx.new(|_| AutomationRoot::new(desktop))
    })?;
    let pump = |cx: &mut HeadlessAppContext, frames: usize| -> anyhow::Result<()> {
        for _ in 0..frames {
            std::thread::sleep(Duration::from_millis(16));
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
            })?;
        }
        Ok(())
    };
    let label = |id: &str| {
        driver
            .snapshot(false)
            .elements
            .into_iter()
            .find(|e| e.id == id && e.visible)
            .map(|e| e.label)
    };
    let act = |cx: &mut HeadlessAppContext, action: serde_json::Value| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(serde_json::from_value(action)?, w, cx)
        })??;
        Ok(())
    };
    let wait = |cx: &mut HeadlessAppContext, what: &str, done: &dyn Fn() -> bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !done() {
            if std::time::Instant::now() >= deadline {
                let seen: Vec<_> = driver
                    .snapshot(false)
                    .elements
                    .into_iter()
                    .filter(|e| {
                        e.visible && (e.id.contains("device-") || e.id.contains("settings-name"))
                    })
                    .map(|e| format!("{}={}", e.id, e.label))
                    .collect();
                anyhow::bail!("timed out: {what}; visible: {seen:?}");
            }
            pump(cx, 4)?;
        }
        pump(cx, 4)
    };
    let studio = format!("C（{STUDIO}） · 直连");
    let air = format!("B（{AIR}） · 直连");

    // Sidebar dock: short display names, machine names in the accessible label.
    wait(&mut cx, "dock shows display names", &|| {
        label("device-dock-name-studio").as_deref() == Some(studio.as_str())
            && label("device-dock-name-air").as_deref() == Some(air.as_str())
    })?;
    // New Chat's device switcher.
    let switcher: Vec<_> = driver
        .snapshot(false)
        .elements
        .into_iter()
        .filter(|e| e.id.starts_with("new-chat-device-name-") && e.visible)
        .map(|e| e.label)
        .collect();
    anyhow::ensure!(
        switcher.contains(&studio) && switcher.contains(&air),
        "switcher: {switcher:?}"
    );
    // Hover: the machine name the device registered with.
    act(
        &mut cx,
        json!({"type":"move","target":{"element_id":"device-name-device-dock-name-studio"}}),
    )?;
    wait(&mut cx, "dock hint", &|| {
        label("control-hint-device-name-device-dock-name-studio").as_deref()
            == Some(format!("机器名称：{STUDIO}").as_str())
    })?;
    cx.capture_screenshot(window.into())?
        .save(output.join("dock-hint.png"))?;

    // Settings list and device page.
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"desktop-manage"}}),
    )?;
    wait(&mut cx, "settings list", &|| {
        label("settings-name-studio").as_deref() == Some(studio.as_str())
            && label("settings-name-air").as_deref() == Some(air.as_str())
    })?;
    act(
        &mut cx,
        json!({"type":"click","target":{"element_id":"settings-device-studio"}}),
    )?;
    wait(&mut cx, "device page", &|| {
        label("settings-device-name").as_deref() == Some(studio.as_str())
            && label("device-machine-name").as_deref()
                == Some(format!("机器名称 {STUDIO}").as_str())
    })?;
    cx.capture_screenshot(window.into())?
        .save(output.join("device-page.png"))?;

    // Rename in place. Validation errors never reach the Station.
    let open = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        act(
            cx,
            json!({"type":"click","target":{"element_id":"device-rename-button"}}),
        )?;
        pump(cx, 6)?;
        anyhow::ensure!(
            label("device-name-input").is_some(),
            "the inline field did not open"
        );
        Ok(())
    };
    let submit = |cx: &mut HeadlessAppContext, text: &str| -> anyhow::Result<()> {
        act(cx, json!({"type":"key","keystroke":"cmd-a"}))?;
        act(cx, json!({"type":"key","keystroke":"backspace"}))?;
        if !text.is_empty() {
            act(cx, json!({"type":"type_text","text":text}))?;
        }
        act(cx, json!({"type":"key","keystroke":"enter"}))?;
        pump(cx, 8)
    };
    let error = || label("device-name-error").unwrap_or_default();
    open(&mut cx)?;
    submit(&mut cx, "")?;
    anyhow::ensure!(error() == "请输入显示名称", "empty: {:?}", error());
    submit(&mut cx, "a")?;
    anyhow::ensure!(
        error().contains("已被 Mesh 中的另一台设备使用"),
        "duplicate: {:?}",
        error()
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("rename-duplicate.png"))?;
    submit(&mut cx, &"长".repeat(25))?;
    anyhow::ensure!(
        error().contains("不能超过 24 个字符"),
        "long: {:?}",
        error()
    );
    anyhow::ensure!(
        mesh.lock().unwrap().renames.is_empty(),
        "invalid names were sent"
    );
    // Esc cancels and keeps the name.
    act(&mut cx, json!({"type":"key","keystroke":"escape"}))?;
    pump(&mut cx, 6)?;
    anyhow::ensure!(label("device-name-input").is_none(), "Esc kept the field");
    anyhow::ensure!(label("settings-device-name").as_deref() == Some(studio.as_str()));
    // A name taken on another device meanwhile: the authority's answer shows.
    open(&mut cx)?;
    submit(&mut cx, "占用")?;
    wait(&mut cx, "authority rejection", &|| {
        error().contains("“占用”已被 Mesh 中的另一台设备使用")
    })?;
    // Enter saves; every surface follows.
    submit(&mut cx, "工作室")?;
    let renamed = format!("工作室（{STUDIO}） · 直连");
    wait(&mut cx, "renamed everywhere", &|| {
        label("settings-device-name").as_deref() == Some(renamed.as_str())
            && label("settings-name-studio").as_deref() == Some(renamed.as_str())
            && label("device-name-input").is_none()
    })?;
    anyhow::ensure!(
        mesh.lock().unwrap().renames == ["占用", "工作室"],
        "renames sent: {:?}",
        mesh.lock().unwrap().renames
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("renamed.png"))?;
    // The other device keeps its letter and the rename survives a refresh
    // from the Station.
    pump(&mut cx, 60)?;
    anyhow::ensure!(label("settings-name-air").as_deref() == Some(air.as_str()));
    anyhow::ensure!(label("settings-name-studio").as_deref() == Some(renamed.as_str()));
    println!("PASS device names: display names with machine-name hints on dock, switcher, settings list and device page; inline rename validates, cancels and saves");
    Ok(())
}
