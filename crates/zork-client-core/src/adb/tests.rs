use super::*;
use crate::store::{RemoteNode, SavedNode};

fn facts() -> Facts {
    Facts {
        supported: true,
        application_id: "fixture.phone".into(),
        name: "Phone".into(),
        developer_enabled: Some(true),
        usb_enabled: Some(true),
        wireless_enabled: Some(false),
    }
}
fn save_station(store: &ClientStore, id: &str, name: &str) -> Result<()> {
    store.save_node(&SavedNode {
        id: id.into(),
        name: name.into(),
        url: String::new(),
        token: None,
        local: false,
        mesh: Some(RemoteNode {
            origin: id.into(),
            addr: None,
        }),
        group: None,
    })
}
fn controller(root: &std::path::Path) -> Result<Arc<Controller>> {
    let store = Arc::new(ClientStore::open(root)?);
    save_station(&store, "station-a", "Mac mini")?;
    save_station(&store, "station-b", "MacBook Air")?;
    let controller = Controller::new(store)?;
    controller.execute(Action::Facts { facts: facts() })?;
    controller.execute(Action::SetEnabled { enabled: true })?;
    controller.state.lock().unwrap().active = true;
    reconcile(&controller, LocalState::Ready)?;
    Ok(controller)
}
fn reconcile(controller: &Controller, local: LocalState) -> Result<Vec<Lease>> {
    let state = controller.state.lock().unwrap().clone();
    Ok(controller
        .reconcile(
            state.configuration,
            &state.facts,
            local,
            controller.stations()?,
        )
        .unwrap())
}
fn update(controller: &Controller, peer: &str, state: HostState, serial: &str) {
    let generation = controller.state.lock().unwrap().stations[peer]
        .generation
        .clone();
    controller.update_station(peer, &generation, state, Some(serial.into()));
}
fn station<'a>(snapshot: &'a Value, id: &str) -> &'a Value {
    snapshot["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["peer"] == id)
        .unwrap()
}

#[test]
fn legacy_settings_do_not_restrict_stations_and_port_validation_stays_in_core() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = Arc::new(ClientStore::open(root.path())?);
    store.put(
        "",
        "adb-settings",
        &json!({"enabled":true,"port":5556,"peer":"removed"}),
    )?;
    let controller = Controller::new(store)?;
    controller.execute(Action::Facts { facts: facts() })?;
    assert!(controller.requested(), "no Station selection is required");
    assert_eq!(
        controller.snapshot()["settings"],
        json!({"enabled":true,"port":5556})
    );
    for port in ["", "abc", "-1", "0", "1023", "65536"] {
        let before = controller.snapshot();
        assert!(controller
            .execute(Action::SetPort { port: port.into() })
            .is_err());
        assert_eq!(controller.snapshot(), before);
    }
    controller.execute(Action::SetPort {
        port: " 65535 ".into(),
    })?;
    controller.execute(Action::SetEnabled { enabled: false })?;
    assert_eq!(controller.snapshot()["settings"]["port"], 65535);
    assert!(!controller.requested());
    Ok(())
}

#[test]
fn independent_stations_keep_phone_ready_and_revoke_only_their_own_lease() -> Result<()> {
    let root = tempfile::tempdir()?;
    let controller = controller(root.path())?;
    update(
        &controller,
        "station-a",
        HostState::Ready,
        "127.0.0.1:41001",
    );
    update(
        &controller,
        "station-b",
        HostState::Unauthorized,
        "127.0.0.1:41002",
    );
    let before = controller.snapshot();
    assert_eq!(before["page"]["step"], "ready");
    assert!(before["page"]["primary_action"].is_null());
    assert!(station(&before, "station-a")["help"].is_null());
    assert!(!station(&before, "station-b")["help"].is_null());
    let leases = controller.state.lock().unwrap().stations.clone();
    controller.store.revoke_replica("station-b")?;
    assert_eq!(
        controller.snapshot()["stations"].as_array().unwrap().len(),
        1,
        "revoked content is redacted before the wake"
    );
    controller.peer_revoked("station-b");
    assert!(*leases["station-b"].cancel.borrow());
    assert!(!*leases["station-a"].cancel.borrow());
    controller.update_station(
        "station-b",
        &leases["station-b"].generation,
        HostState::Ready,
        Some("stale".into()),
    );
    let after = controller.snapshot();
    assert_eq!(station(&after, "station-a"), station(&before, "station-a"));
    assert_eq!(after["stations"].as_array().unwrap().len(), 1);
    assert_eq!(after["service_requested"], true);
    Ok(())
}

#[test]
fn membership_refresh_and_replacement_fence_only_affected_stations() -> Result<()> {
    let root = tempfile::tempdir()?;
    let controller = controller(root.path())?;
    update(
        &controller,
        "station-a",
        HostState::Ready,
        "127.0.0.1:41001",
    );
    let before = controller.state.lock().unwrap().stations.clone();
    controller.execute(Action::Refresh)?;
    reconcile(&controller, LocalState::Ready)?;
    assert_eq!(
        controller.state.lock().unwrap().stations["station-a"].generation,
        before["station-a"].generation
    );
    save_station(&controller.store, "station-c", "Third Station")?;
    reconcile(&controller, LocalState::Ready)?;
    assert_eq!(
        station(&controller.snapshot(), "station-a")["serial"],
        "127.0.0.1:41001"
    );
    controller.store.remove_node("station-b")?;
    reconcile(&controller, LocalState::Ready)?;
    assert!(*before["station-b"].cancel.borrow());
    save_station(&controller.store, "station-b", "MacBook Air")?;
    reconcile(&controller, LocalState::Ready)?;
    controller.update_station(
        "station-b",
        &before["station-b"].generation,
        HostState::Ready,
        Some("stale".into()),
    );
    assert!(station(&controller.snapshot(), "station-b")["serial"].is_null());
    assert_eq!(
        controller.state.lock().unwrap().stations["station-a"].generation,
        before["station-a"].generation
    );
    Ok(())
}

#[test]
fn disabling_and_changing_the_phone_port_cancel_every_old_stream_generation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let controller = controller(root.path())?;
    let old = controller.state.lock().unwrap().stations.clone();
    controller.execute(Action::SetPort {
        port: "5557".into(),
    })?;
    assert!(old.values().all(|lease| *lease.cancel.borrow()));
    assert!(controller.snapshot()["stations"]
        .as_array()
        .unwrap()
        .is_empty());
    reconcile(&controller, LocalState::Ready)?;
    controller.update_station(
        "station-a",
        &old["station-a"].generation,
        HostState::Ready,
        Some("old port".into()),
    );
    assert!(station(&controller.snapshot(), "station-a")["serial"].is_null());
    let current = controller.state.lock().unwrap().stations.clone();
    controller.execute(Action::SetEnabled { enabled: false })?;
    assert!(current.values().all(|lease| *lease.cancel.borrow()));
    assert_eq!(controller.snapshot()["page"]["step"], "disabled");
    assert_eq!(controller.snapshot()["service_requested"], false);
    assert!(controller.snapshot()["stations"]
        .as_array()
        .unwrap()
        .is_empty());
    Ok(())
}

#[test]
fn settings_guidance_exposes_only_the_current_phone_step() -> Result<()> {
    let root = tempfile::tempdir()?;
    let controller = controller(root.path())?;
    for (local, step, action) in [
        (LocalState::Checking, "checking", None),
        (
            LocalState::DeveloperDisabled,
            "developer",
            Some("open_device_info"),
        ),
        (
            LocalState::UsbDisabled,
            "usb",
            Some("open_developer_options"),
        ),
        (
            LocalState::ActivationRequired,
            "activation",
            Some("activation_help"),
        ),
        (
            LocalState::WirelessOnly,
            "activation",
            Some("activation_help"),
        ),
        (LocalState::Ready, "ready", None),
    ] {
        reconcile(&controller, local)?;
        let snapshot = controller.snapshot();
        let page = &snapshot["page"];
        assert_eq!(page["step"], step);
        assert_eq!(page["primary_action"]["kind"].as_str(), action);
        assert_eq!(page["show_connections"], local == LocalState::Ready);
        assert_eq!(!page["activation_help"].is_null(), step == "activation");
    }
    Ok(())
}

#[tokio::test]
async fn snapshots_discard_old_batches_and_service_cleanup_keeps_the_new_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut client = crate::Client::open(root.path())?;
    let local = client.local();
    let mut wire = local.observe(crate::subscriptions::Key::Adb)?;
    let first = wire.prepare()?.unwrap();
    local.execute(crate::Command::Adb {
        operation: Action::Facts { facts: facts() },
    })?;
    assert!(!wire.valid(first["batch"].as_u64().unwrap()));
    for (running, instance) in [(true, "old"), (true, "new"), (false, "old")] {
        client
            .execute(crate::Command::AdbBackgroundService {
                running,
                instance: instance.into(),
            })
            .await?;
    }
    assert_eq!(client.adb_background_service.as_deref(), Some("new"));
    client
        .execute(crate::Command::AdbBackgroundService {
            running: false,
            instance: "new".into(),
        })
        .await?;
    assert!(client.adb_background_service.is_none());
    Ok(())
}

#[test]
fn android_presentation_fixtures_use_core_snapshots() -> Result<()> {
    let mut examples = serde_json::Map::new();
    for (name, local) in [
        ("checking", LocalState::Checking),
        ("developer", LocalState::DeveloperDisabled),
        ("usb", LocalState::UsbDisabled),
        ("activation", LocalState::ActivationRequired),
        ("ready", LocalState::Ready),
        ("mixed", LocalState::Ready),
        ("disabled", LocalState::Ready),
        ("waiting", LocalState::Ready),
    ] {
        let root = tempfile::tempdir()?;
        let controller = controller(root.path())?;
        reconcile(&controller, local)?;
        if name == "ready" || name == "mixed" {
            update(
                &controller,
                "station-a",
                HostState::Ready,
                "127.0.0.1:41001",
            );
            update(
                &controller,
                "station-b",
                if name == "mixed" {
                    HostState::Unauthorized
                } else {
                    HostState::Ready
                },
                "127.0.0.1:41002",
            );
        }
        if name == "disabled" {
            controller.execute(Action::SetEnabled { enabled: false })?;
        }
        if name == "waiting" {
            controller.store.remove_node("station-a")?;
            controller.store.remove_node("station-b")?;
            reconcile(&controller, local)?;
        }
        examples.insert(name.into(), controller.snapshot());
    }
    let examples = Value::Object(examples);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/android/app/src/androidTest/assets/adb-states.json");
    if std::env::var_os("ZORK_UPDATE_ADB_FIXTURES").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&examples)? + "\n")?;
    }
    let fixture: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(
        fixture, examples,
        "regenerate Android fixtures with ZORK_UPDATE_ADB_FIXTURES=1"
    );
    Ok(())
}
