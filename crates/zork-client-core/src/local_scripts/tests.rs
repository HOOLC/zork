use super::*;
use crate::{
    api::{MessagePage, TranscriptMessage},
    store::{ClientStore, SavedNode},
};

fn fixture() -> (tempfile::TempDir, Arc<Controller>) {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(ClientStore::open(root.path()).unwrap());
    (root, Controller::new(store))
}
fn card(source: &str) -> Card {
    serde_json::from_value(json!({"title":"Local action","source":source})).unwrap()
}
async fn wait(controller: &Controller, predicate: impl Fn(&Value) -> bool) -> Arc<Value> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut updates = controller.source.subscribe();
        loop {
            if let Some(batch) = updates.prepare() {
                let value = batch.snapshot.value.clone();
                updates.acknowledge(batch.id);
                if predicate(&value) {
                    return value;
                }
            }
            updates.ready().await.unwrap();
        }
    })
    .await
    .expect("script state did not converge")
}
async fn pending(controller: &Controller) -> Value {
    wait(controller, |v| !v["request"].is_null()).await["request"].clone()
}
fn claim(controller: &Controller, request: &Value) -> Value {
    controller
        .apply(Action::Claim {
            run_id: request["run_id"].as_str().unwrap().into(),
            request_id: request["id"].as_str().unwrap().into(),
        })
        .unwrap()
}
fn complete(controller: &Controller, request: &Value, value: Value) -> Value {
    controller
        .apply(Action::Complete {
            run_id: request["run_id"].as_str().unwrap().into(),
            request_id: request["id"].as_str().unwrap().into(),
            value: Some(value),
            error: None,
        })
        .unwrap()
}

#[tokio::test]
async fn javascript_await_and_control_flow_use_claimed_local_capabilities_once() {
    let (_root, controller) = fixture();
    let script = card("const words = ['a', 'b']; for (const word of words) { await android.clipboard.write(word.toUpperCase()); } const volume = await android.volume.get(); console.log(volume.index + 1);");
    // Observing/rendering code never runs it, on Android or on a read-only PC.
    assert!(
        crate::interactions::local_script_card("message", &script, false)
            .actions
            .is_empty()
    );
    assert_eq!(
        crate::interactions::local_script_card("message", &script, true).actions[0].id,
        "run_local_script"
    );
    assert!(controller.snapshot()["run"].is_null());
    controller.start(script, None).unwrap();
    for text in ["A", "B"] {
        let request = pending(&controller).await;
        assert_eq!(request["operation"]["text"], text);
        assert_eq!(claim(&controller, &request)["accepted"], true);
        assert_eq!(claim(&controller, &request)["accepted"], false);
        assert_eq!(
            complete(&controller, &request, Value::Null)["accepted"],
            true
        );
        assert_eq!(
            complete(&controller, &request, Value::Null)["accepted"],
            false
        );
    }
    let request = pending(&controller).await;
    assert_eq!(
        claim(&controller, &request)["operation"]["method"],
        "volume_get"
    );
    complete(&controller, &request, json!({"index":4,"min":0,"max":15}));
    let done = wait(&controller, |v| v["run"]["phase"] == "succeeded").await;
    assert_eq!(done["run"]["logs"], json!(["5"]));
    assert!(done["request"].is_null());
}

#[tokio::test]
async fn cancellation_rejects_late_callbacks_and_prevents_later_effects() {
    let (_root, controller) = fixture();
    let run = controller.start(card("await android.requestPermissions(['android.permission.CAMERA']); await android.clipboard.write('must not run');"), None).unwrap();
    let request = pending(&controller).await;
    claim(&controller, &request);
    controller
        .apply(Action::Cancel {
            run_id: run["run_id"].as_str().unwrap().into(),
        })
        .unwrap();
    assert_eq!(
        complete(
            &controller,
            &request,
            json!({"android.permission.CAMERA":true})
        )["accepted"],
        false
    );
    wait(&controller, |_| {
        !controller
            .run
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .worker_active
    })
    .await;
    assert_eq!(controller.snapshot()["run"]["phase"], "cancelled");
    assert!(controller.snapshot()["request"].is_null());
    controller
        .start(card("console.log('next run');"), None)
        .unwrap();
    assert_eq!(
        complete(&controller, &request, json!(true))["accepted"],
        false
    );
    assert_eq!(
        wait(&controller, |v| v["run"]["phase"] == "succeeded").await["run"]["logs"],
        json!(["next run"])
    );
}

#[tokio::test]
async fn document_authority_comes_from_this_runs_picker_and_utf8_errors_return_to_js() {
    let (_root, controller) = fixture();
    controller
        .start(
            card("await android.content.readText('content://private/secrets');"),
            None,
        )
        .unwrap();
    let failed = wait(&controller, |v| v["run"]["phase"] == "failed").await;
    assert!(failed["run"]["error"]
        .as_str()
        .unwrap()
        .contains("Select this document"));
    assert!(failed["request"].is_null());
    controller.start(card("const picked = await android.startActivityForResult({action:'android.intent.action.OPEN_DOCUMENT',mimeType:'text/plain'}); try { await android.content.readText(picked.data); } catch (e) { console.log(e.message); }"), None).unwrap();
    let picker = pending(&controller).await;
    claim(&controller, &picker);
    complete(
        &controller,
        &picker,
        json!({"resultCode":-1,"data":"content://documents/one"}),
    );
    let read = pending(&controller).await;
    assert_eq!(
        claim(&controller, &read)["operation"]["method"],
        "content_read_text"
    );
    complete(&controller, &read, json!({"hex":"ff"}));
    let done = wait(&controller, |v| v["run"]["phase"] == "succeeded").await;
    assert!(done["run"]["logs"][0].as_str().unwrap().contains("UTF-8"));
}

#[tokio::test]
async fn revoked_source_cannot_claim_a_prepared_effect() {
    let (_root, controller) = fixture();
    controller
        .start(
            card("await android.clipboard.write('private');"),
            Some(Origin {
                peer: "node".into(),
                generation: 0,
            }),
        )
        .unwrap();
    let request = pending(&controller).await;
    controller.store.revoke_replica("node").unwrap();
    assert_eq!(claim(&controller, &request)["accepted"], false);
    let failed = wait(&controller, |v| v["run"]["phase"] == "failed").await;
    assert!(failed["run"]["error"]
        .as_str()
        .unwrap()
        .contains("access changed"));
}

#[tokio::test]
async fn source_cache_replay_and_restart_preserve_code_without_executing_or_creating_results() {
    let (root, controller) = fixture();
    controller
        .store
        .save_node(&SavedNode {
            machine_name: None,
            color_key: None,
            id: "node".into(),
            name: "Node".into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        })
        .unwrap();
    let script = card("console.log('explicit execution');");
    let message: TranscriptMessage = serde_json::from_value(
        json!({"type":"message","role":"assistant","id":"card","content":"", "interaction":script}),
    )
    .unwrap();
    let page = MessagePage {
        source_epoch: None,
        items: vec![message.clone()],
        older_cursor: None,
    };
    controller
        .store
        .cache_message_page("node", "chat", &page, None)
        .unwrap();
    controller
        .store
        .cache_message_page("node", "chat", &page, None)
        .unwrap();
    assert!(controller.snapshot()["run"].is_null());
    #[cfg(not(target_os = "android"))]
    assert!(controller.start_message("node", "chat", "card").is_err());
    controller.start(script, None).unwrap();
    wait(&controller, |v| v["run"]["phase"] == "succeeded").await;
    let source = controller
        .store
        .cached_messages("node", "chat", None, 100)
        .unwrap()
        .unwrap();
    assert_eq!(source.items.len(), 1);
    assert_eq!(source.items[0], message);
    assert!(controller.store.outbox("node").unwrap().is_empty());
    drop(controller);
    let reopened = Controller::new(Arc::new(ClientStore::open(root.path()).unwrap()));
    assert!(reopened.snapshot()["run"].is_null());
}

#[tokio::test]
async fn unsupported_calls_and_engine_limits_fail_locally() {
    let (_root, controller) = fixture();
    for source in [
        "await android.startActivity({action:'x', flags:268435456});",
        "import 'std';",
        "new ArrayBuffer(64 * 1024 * 1024);",
        "await new Promise(() => {});",
    ] {
        controller.start(card(source), None).unwrap();
        let failed = wait(&controller, |v| v["run"]["phase"] == "failed").await;
        assert!(failed["request"].is_null(), "{source}");
    }
    let started = Instant::now();
    let outcome = engine::execute(
        controller.clone(),
        "limit",
        "while (true) {}",
        Arc::new(AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
        Instant::now() + Duration::from_millis(100),
        &tokio::runtime::Handle::current(),
    );
    assert!(outcome.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn cancellation_wakes_a_sleeping_script_and_large_callbacks_fail_without_hanging() {
    let (_root, controller) = fixture();
    let run = controller
        .start(
            card("await android.sleep(30000); await android.clipboard.write('unexpected');"),
            None,
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    controller
        .apply(Action::Cancel {
            run_id: run["run_id"].as_str().unwrap().into(),
        })
        .unwrap();
    wait(&controller, |_| {
        !controller
            .run
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .worker_active
    })
    .await;
    assert!(controller.snapshot()["request"].is_null());
    controller
        .start(card("await android.clipboard.read();"), None)
        .unwrap();
    let request = pending(&controller).await;
    claim(&controller, &request);
    complete(
        &controller,
        &request,
        json!("x".repeat(MAX_REPLY_BYTES + 1)),
    );
    let failed = wait(&controller, |v| v["run"]["phase"] == "failed").await;
    assert!(failed["run"]["error"]
        .as_str()
        .unwrap()
        .contains("too large"));
}

#[tokio::test]
async fn script_observer_discard_and_ack_do_not_consume_platform_work() {
    let (_root, controller) = fixture();
    let mut observer =
        crate::subscriptions::WireSubscription::from_local_scripts(&controller.source);
    let initial = observer.prepare().unwrap().unwrap();
    assert_eq!(initial["reset"], true);
    assert!(observer.finish(initial["batch"].as_u64().unwrap(), true));
    controller
        .start(card("await android.clipboard.write('once');"), None)
        .unwrap();
    let request = pending(&controller).await;
    let frame = observer.prepare().unwrap().unwrap();
    observer.finish(frame["batch"].as_u64().unwrap(), false);
    assert_eq!(controller.snapshot()["request"], request);
    let frame = observer.prepare().unwrap().unwrap();
    assert_eq!(claim(&controller, &request)["accepted"], true);
    // A previously prepared presentation can still be applied. Its old request
    // cannot claim the effect again, and the next batch catches up from that ack.
    assert_eq!(claim(&controller, &request)["accepted"], false);
    assert!(observer.finish(frame["batch"].as_u64().unwrap(), true));
    let next = observer.prepare().unwrap().unwrap();
    assert!(next["snapshot"]["request"].is_null());
    assert!(observer.finish(next["batch"].as_u64().unwrap(), true));
    complete(&controller, &request, Value::Null);
    wait(&controller, |v| v["run"]["phase"] == "succeeded").await;
}
