//! One station, one status: the real desktop shows the same core status for a
//! station in the device dock, the New Chat switcher, the settings device list,
//! the device page and the Mesh member list. Mesh membership links are worded
//! separately and never reuse the status dot.
use gpui::{px, AppContext, HeadlessAppContext};
use serde_json::json;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationRoot, HeadlessAutomation},
};

const STATIONS: [&str; 4] = ["reachable", "second", "offline", "remote"];

fn main() -> anyhow::Result<()> {
    let output =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../artifacts/device-status");
    std::fs::create_dir_all(&output)?;
    if std::env::args().any(|arg| arg == "--desktop") {
        return desktop(&output);
    }
    local_device_row(&output)?;
    // A separate process supplies an isolated desktop store before GPUI starts;
    // no live device is restored and no local node is launched.
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
    anyhow::ensure!(status.success(), "desktop device status regression failed");
    Ok(())
}

/// The Mesh device list of this machine's own station leads with its own row,
/// tagged "本机" in words (read out with name and status), with the same
/// status as elsewhere and no removal action; other rows are untagged.
fn local_device_row(output: &std::path::Path) -> anyhow::Result<()> {
    use zork_gui::desktop::stories::{self, StoryHost};
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
    let story = stories::fixture("mesh-connected-wide")
        .ok_or_else(|| anyhow::anyhow!("no Mesh device list story"))?;
    let window = cx.open_window(gpui::size(px(1280.), px(800.)), |_, cx| {
        let host = cx.new(|cx| StoryHost::new(story, cx));
        cx.new(|_| AutomationRoot::new(host))
    })?;
    for _ in 0..4 {
        cx.advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
        })?;
    }
    let snapshot = driver.snapshot(false);
    let element = |id: &str| snapshot.elements.iter().find(|e| e.id == id);
    let rows: Vec<_> = snapshot
        .elements
        .iter()
        .filter(|e| {
            e.id.starts_with("mesh-peer-")
                && ![
                    "mesh-peer-link-",
                    "mesh-peer-permission-",
                    "mesh-peer-more-",
                ]
                .iter()
                .any(|prefix| e.id.starts_with(prefix))
        })
        .collect();
    anyhow::ensure!(
        rows.first().map(|e| (e.id.as_str(), e.label.as_str()))
            == Some(("mesh-peer-mini1", "A（mini1） · 本机 · 直连")),
        "this machine must lead, tagged in words: {:?}",
        rows.iter().map(|e| (&e.id, &e.label)).collect::<Vec<_>>()
    );
    anyhow::ensure!(
        rows.iter().skip(1).all(|e| !e.label.contains("本机")),
        "only this machine is tagged: {:?}",
        rows.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    anyhow::ensure!(
        element("mesh-peer-more-mini1").is_none() && element("mesh-peer-more-mini2").is_some(),
        "this machine cannot be removed from its own list"
    );
    anyhow::ensure!(
        element("mesh-peer-link-mini1").is_none(),
        "this machine has no Mesh link to itself"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("mesh-local-device.png"))?;
    println!("PASS Mesh local device: this machine leads its own list, tagged 本机 in words, without removal");
    Ok(())
}

/// A station whose event stream opens and stays open, with a Mesh membership
/// in which `offline` is linked to it although this client cannot reach it.
/// `second` (reached as `localhost`) goes away when the returned sender is set
/// to false: its stream ends and reconnects are refused.
fn station() -> anyhow::Result<(u16, tokio::sync::watch::Sender<bool>)> {
    use axum::{
        http::{header::HOST, HeaderMap, StatusCode},
        response::{sse, IntoResponse},
        routing::get,
        Json, Router,
    };
    use futures_util::StreamExt;
    let (port, ready) = std::sync::mpsc::channel();
    let (second_up, second) = tokio::sync::watch::channel(true);
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async move {
            let peer = |id: &str, name: &str, client: bool| {
                json!({"origin":format!("key:{id}"),"name":name,"addr":null,"client":client,"collaborate":!client})
            };
            let status = |id: &str, name: &str, online: bool| {
                json!({"origin":format!("key:{id}"),"name":name,"online":online,"execute_workspaces":[]})
            };
            let config = json!({"config":{"enabled":true,"peers":[
                peer("second", "second", false), peer("offline", "offline", false), peer("phone", "手机", true)
            ]},"origin":"key:reachable"});
            let mesh = json!({"enabled":true,"origin":"key:reachable","peers":[
                status("second", "second", true), status("offline", "offline", true), status("phone", "手机", false)
            ]});
            let router = Router::new()
                .route(
                    "/v1/im/events",
                    get(move |headers: HeaderMap| {
                        let mut up = second.clone();
                        async move {
                            let is_second = headers
                                .get(HOST)
                                .and_then(|host| host.to_str().ok())
                                .is_some_and(|host| host.starts_with("localhost"));
                            if !is_second {
                                return sse::Sse::new(futures_util::stream::pending::<
                                    Result<sse::Event, std::convert::Infallible>,
                                >())
                                .into_response();
                            }
                            if !*up.borrow() {
                                return StatusCode::SERVICE_UNAVAILABLE.into_response();
                            }
                            let end = futures_util::stream::once(async move {
                                let _ = up.wait_for(|up| !*up).await;
                            })
                            .filter_map(|_| async {
                                None::<Result<sse::Event, std::convert::Infallible>>
                            });
                            sse::Sse::new(end).into_response()
                        }
                    }),
                )
                .route("/v1/node/mesh", get(move || async move { Json(config) }))
                .route("/v1/mesh", get(move || async move { Json(mesh) }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            port.send(listener.local_addr().unwrap().port()).unwrap();
            axum::serve(listener, router).await.unwrap();
        });
    });
    Ok((ready.recv_timeout(Duration::from_secs(10))?, second_up))
}

fn desktop(output: &std::path::Path) -> anyhow::Result<()> {
    let (port, second_up) = station()?;
    let store =
        zork_gui::desktop::store::ClientStore::open(&zork_client_core::desktop::client_root())?;
    store.set_local_node_enabled(false)?;
    for (id, url) in [
        ("reachable", format!("http://127.0.0.1:{port}")),
        ("second", format!("http://localhost:{port}")),
        ("offline", "http://127.0.0.1:9".into()),
    ] {
        store.save_node(&serde_json::from_value(
            json!({"id":id,"name":id,"url":url,"local":false}),
        )?)?;
        store.put(id, "mesh-origin", &format!("key:{id}"))?;
    }
    // A Mesh-only member this client cannot reach; it also makes startup
    // prepare the client Mesh that device statuses depend on.
    store.save_node(&serde_json::from_value(json!({
        "id":"remote","name":"remote","url":"","local":false,
        "mesh":{"origin":format!("key:{}", "y".repeat(52)),"addr":null}
    }))?)?;
    store.put("device", "last-node", &"reachable")?;
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
    let mut root = None;
    let window = cx.open_window(gpui::size(px(1100.), px(760.)), |_, cx| {
        let desktop = cx.new(zork_gui::desktop::DesktopRoot::new);
        root = Some(desktop.clone());
        cx.new(|_| AutomationRoot::new(desktop))
    })?;
    let root = root.unwrap();
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
    // "name · status" → status, for one station.
    let status_of = |label: &str, station: &str| {
        label
            .strip_prefix(&format!("{station} · "))
            .map(str::to_owned)
    };
    let expected = BTreeMap::from([
        ("reachable", "直连"),
        ("second", "直连"),
        ("offline", "离线"),
        ("remote", "离线"),
    ]);
    // Connections settle through the real client: loopback streams open (直连)
    // and unreachable stations fail (离线). Nothing stays 连接中.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        pump(&mut cx, 4)?;
        let dock: BTreeMap<_, _> = STATIONS
            .iter()
            .map(|id| {
                (
                    *id,
                    label(&format!("device-dock-name-{id}"))
                        .and_then(|l| status_of(&l, id))
                        .unwrap_or_default(),
                )
            })
            .collect();
        if STATIONS.iter().all(|id| dock[id] == expected[id]) {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "device dock did not settle: {dock:?}"
        );
    }
    let mut seen: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    let mut record = |surface: &str, id: &str, status: String| {
        seen.entry(STATIONS.iter().find(|s| **s == id).copied().unwrap())
            .or_default()
            .push((surface.to_owned(), status));
    };
    for id in STATIONS {
        let dock = label(&format!("device-dock-name-{id}")).unwrap();
        record("device dock", id, status_of(&dock, id).unwrap());
    }
    // New Chat's device switcher, on the active station's home page.
    let snapshot = driver.snapshot(false);
    let switcher: Vec<_> = snapshot
        .elements
        .iter()
        .filter(|e| e.id.starts_with("new-chat-device-name-") && e.visible)
        .collect();
    anyhow::ensure!(
        switcher.len() == STATIONS.len(),
        "New Chat switcher is missing stations: {:?}",
        switcher.iter().map(|e| &e.label).collect::<Vec<_>>()
    );
    for element in switcher {
        let id = STATIONS
            .iter()
            .find(|id| element.label.starts_with(&format!("{id} · ")))
            .ok_or_else(|| anyhow::anyhow!("unknown switcher entry {}", element.label))?;
        record(
            "new chat switcher",
            id,
            status_of(&element.label, id).unwrap(),
        );
    }
    cx.capture_screenshot(window.into())?
        .save(output.join("dock-and-switcher.png"))?;

    // Settings: the device list and the active station's page.
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(
                json!({"type":"click","target":{"element_id":"desktop-manage"}}),
            )?,
            w,
            cx,
        )
    })??;
    pump(&mut cx, 20)?;
    for id in STATIONS {
        let row = label(&format!("settings-name-{id}"))
            .ok_or_else(|| anyhow::anyhow!("settings list lacks {id}"))?;
        record("settings device list", id, status_of(&row, id).unwrap());
    }
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(
                json!({"type":"click","target":{"element_id":"settings-device-offline"}}),
            )?,
            w,
            cx,
        )
    })??;
    pump(&mut cx, 20)?;
    let page = label("settings-device-name").ok_or_else(|| anyhow::anyhow!("no device page"))?;
    record(
        "device page",
        "offline",
        status_of(&page, "offline").unwrap(),
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("settings-device-offline.png"))?;
    cx.update_window(window.into(), |_, w, cx| {
        driver.dispatch(
            serde_json::from_value(
                json!({"type":"click","target":{"element_id":"settings-device-reachable"}}),
            )?,
            w,
            cx,
        )
    })??;
    pump(&mut cx, 20)?;
    let page = label("settings-device-name").ok_or_else(|| anyhow::anyhow!("no device page"))?;
    record(
        "device page",
        "reachable",
        status_of(&page, "reachable").unwrap(),
    );

    // The reachable station's Mesh members: this client's status per saved
    // device, and the station's own links in words.
    root.update(&mut cx, |root, cx| root.headless_manage(2, cx));
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while label("mesh-peer-key:offline").is_none() {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "Mesh member list did not load"
        );
        pump(&mut cx, 4)?;
    }
    pump(&mut cx, 8)?;
    for id in ["second", "offline"] {
        let row = label(&format!("mesh-peer-key:{id}")).unwrap();
        record("mesh member list", id, status_of(&row, id).unwrap());
    }
    anyhow::ensure!(
        label("mesh-peer-link-key:offline").as_deref() == Some("与 reachable 已互通"),
        "the member link must be worded separately: {:?}",
        label("mesh-peer-link-key:offline")
    );
    anyhow::ensure!(
        label("mesh-peer-link-key:phone").as_deref() == Some("与 reachable 未互通"),
        "client member link: {:?}",
        label("mesh-peer-link-key:phone")
    );
    // A member that is not one of this client's devices gets no status dot.
    anyhow::ensure!(
        label("mesh-peer-key:phone").as_deref() == Some("手机"),
        "non-device member shows a device status: {:?}",
        label("mesh-peer-key:phone")
    );
    anyhow::ensure!(
        !driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id.starts_with("device-status-") && e.id.contains("phone")),
        "non-device member shows a status badge"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("mesh-members.png"))?;

    // One isolated change must reach every surface without another event:
    // `second` drops while the Mesh page and the settings list are shown.
    second_up.send_replace(false);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        pump(&mut cx, 4)?;
        let list = label("settings-name-second").and_then(|l| status_of(&l, "second"));
        let member = label("mesh-peer-key:second").and_then(|l| status_of(&l, "second"));
        if list.as_deref() == Some("离线") && member.as_deref() == Some("离线") {
            break;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "a dropped station did not update every surface: list {list:?}, Mesh member {member:?}"
        );
    }
    // The link of the listed station is a different fact and stays as reported.
    anyhow::ensure!(
        label("mesh-peer-link-key:second").as_deref() == Some("与 reachable 已互通"),
        "member link changed with this client's status"
    );
    cx.capture_screenshot(window.into())?
        .save(output.join("mesh-members-second-dropped.png"))?;

    for (station, surfaces) in &seen {
        for (surface, status) in surfaces {
            anyhow::ensure!(
                status == expected[station],
                "{station} shows {status} in the {surface}, expected {}: {seen:?}",
                expected[station]
            );
        }
    }
    let surfaces = seen.values().map(Vec::len).sum::<usize>();
    anyhow::ensure!(surfaces >= 16, "not every surface was checked: {seen:?}");
    std::fs::write(
        output.join("statuses.json"),
        serde_json::to_vec_pretty(&seen)?,
    )?;
    println!("PASS device status: one core status per station across dock, switcher, settings list, device page and Mesh members; Mesh links worded separately");
    Ok(())
}
