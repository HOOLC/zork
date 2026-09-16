use super::*;
fn config() -> ServerInput {
    serde_json::from_value(json!({"name":"fixture","transport":{"kind":"stdio","command":"/usr/bin/python3","args":[],"cwd":"/tmp"}})).unwrap()
}
fn who() -> Subject {
    Subject {
        origin: "key:caller".into(),
        agent: "agent".into(),
        session: "session".into(),
    }
}
#[test]
fn legacy_grants_do_not_restrict_members_but_disabled_and_tool_policy_still_apply() {
    let mut server = Server {
        id: "server".into(),
        revision: "revision".into(),
        config: config(),
    };
    server.config._grant = json!({"scope":"selected","subjects":[]});
    let mut other = who();
    other.agent = "another-agent".into();
    for local in [true, false] {
        assert!(server.access(&other, local, Some("echo")).is_ok());
    }
    server.config.enabled = false;
    for local in [true, false] {
        assert_eq!(
            server
                .access(&other, local, Some("echo"))
                .unwrap_err()
                .to_string(),
            "mcp_disabled"
        );
    }
    server.config.enabled = true;
    server.config.tool_allowlist = Some(vec!["echo".into()]);
    assert!(server.access(&other, false, Some("echo")).is_ok());
    assert_eq!(
        server
            .access(&other, false, Some("delete"))
            .unwrap_err()
            .to_string(),
        "mcp_tool_not_allowed"
    );
}
#[test]
fn persistent_ids_configuration_and_compare_and_swap() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(dir.path()).unwrap();
    let server = store.save(config(), None, None).unwrap();
    valid_id(&server.id).unwrap();
    assert!(server.allows(&who(), true, Some("echo")));
    assert!(server.allows(&who(), false, None));
    assert!(store
        .save(config(), Some(&server.id), Some("stale"))
        .is_err());
    let mut input = config();
    input._grant = json!({"scope":"local"});
    input.tool_allowlist = Some(vec!["echo".into()]);
    let server = store
        .save(input, Some(&server.id), Some(&server.revision))
        .unwrap();
    assert!(server.allows(&who(), false, Some("echo")));
    assert!(!server.allows(&who(), false, Some("delete")));
    drop(store);
    let store = store::Store::open(dir.path()).unwrap();
    assert_eq!(store.server(&server.id).unwrap().revision, server.revision);
    store.remove(&server.id, &server.revision).unwrap();
    assert!(store.server(&server.id).is_err());
    assert_ne!(store.save(config(), None, None).unwrap().id, server.id);
}
#[test]
fn duplicate_conflict_crash_recovery_and_private_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(dir.path()).unwrap();
    let (id, fresh) = store
        .submit(&who(), "request", "server", "echo", "digest")
        .unwrap();
    assert!(fresh);
    valid_id(&id).unwrap();
    assert_eq!(
        store
            .submit(&who(), "request", "server", "echo", "digest")
            .unwrap(),
        (id.clone(), false)
    );
    assert!(store
        .submit(&who(), "request", "server", "echo", "changed")
        .is_err());
    let mut stranger = who();
    stranger.session = "stranger".into();
    assert!(store.call(&id, &stranger).is_err());
    store.transition(&id, "dispatching", None).unwrap();
    drop(store);
    let store = store::Store::open(dir.path()).unwrap();
    assert_eq!(store.call(&id, &who()).unwrap().1, "outcome_unknown");
    assert_eq!(
        store
            .submit(&who(), "request", "server", "echo", "digest")
            .unwrap(),
        (id, false)
    );
}
#[test]
fn pagination_rejects_changed_snapshots_and_forged_offsets() {
    let items = (0..30).map(|i| json!({"name":i})).collect::<Vec<_>>();
    let first = page(items.clone(), &None).unwrap();
    assert_eq!(first["items"].as_array().unwrap().len(), 20);
    let cursor = Some(first["next_cursor"].as_str().unwrap().to_owned());
    assert_eq!(
        page(items, &cursor).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        10
    );
    assert!(page(vec![], &cursor).is_err());
    assert!(page(vec![], &Some("wrong".into())).is_err());
}
#[cfg(unix)]
#[tokio::test]
async fn actual_stdio_protocol_is_bounded_and_session_owned() {
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("server.py");
    std::fs::write(
        &fixture,
        r#"import sys,json
counter=0
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 method=r['method']
 if method=='initialize':
  print(json.dumps({'jsonrpc':'2.0','id':'probe','method':'ping'}),flush=True)
  assert json.loads(sys.stdin.readline())['result']=={}
  result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}}}
 elif method=='tools/list': result={'tools':[{'name':'count','inputSchema':{'type':'object'}}]}
 elif method=='tools/call':
  counter+=1
  result={'content':[{'type':'text','text':str(counter)}]}
 else: result={}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
"#,
    )
    .unwrap();
    let transport = runtime::Transport::Stdio {
        command: "/usr/bin/python3".into(),
        args: vec![fixture.to_string_lossy().into_owned()],
        cwd: dir.path().to_string_lossy().into_owned(),
        env: Default::default(),
        secret_env: Default::default(),
    };
    let mut first = runtime::Client::connect(&transport).await.unwrap();
    assert_eq!(first.list_tools().await.unwrap().len(), 1);
    for n in 1..=2 {
        let result = first
            .request("tools/call", json!({"name":"count","arguments":{}}))
            .await
            .unwrap();
        assert_eq!(result["content"][0]["text"], n.to_string());
    }
    let mut second = runtime::Client::connect(&transport).await.unwrap();
    assert_eq!(
        second
            .request("tools/call", json!({"name":"count","arguments":{}}))
            .await
            .unwrap()["content"][0]["text"],
        "1"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn oversized_protocol_frames_are_rejected_before_json_parsing() {
    let transport = runtime::Transport::Stdio {
        command: "/usr/bin/python3".into(),
        args: vec![
            "-c".into(),
            format!(
                "import sys; sys.stdin.readline(); print('x'*{},flush=True)",
                runtime::MAX_MESSAGE + 1
            ),
        ],
        cwd: "/tmp".into(),
        env: Default::default(),
        secret_env: Default::default(),
    };
    let result = tokio::time::timeout(Duration::from_secs(5), runtime::Client::connect(&transport))
        .await
        .unwrap();
    assert_eq!(result.err().unwrap().to_string(), "mcp_response_limit");
}

#[test]
fn outbox_is_bound_to_the_session_and_retains_the_original_request() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(dir.path()).unwrap();
    let request: Operation =
        serde_json::from_value(json!({"op":"call","tool":"echo","arguments":{"text":"once"}}))
            .unwrap();
    store
        .route(&who(), "invoke", "digest", "key:owner", &request)
        .unwrap();
    assert!(store
        .route(&who(), "invoke", "different", "key:owner", &request)
        .is_err());
    let pending = store.pending(&who()).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].2.arguments, request.arguments);
    let mut stranger = who();
    stranger.session = "other".into();
    assert!(store.pending(&stranger).unwrap().is_empty());
    let id = new_id();
    store.bind_route(&who(), "invoke", &id).unwrap();
    assert!(store.pending(&who()).unwrap().is_empty());
    assert_eq!(store.owner(&who(), &id).unwrap(), "key:owner");
}

#[test]
fn object_key_order_does_not_change_a_binding_or_retry_digest() {
    let first: Value = serde_json::from_str(r#"{"b":{"y":2,"x":1},"a":0}"#).unwrap();
    let second: Value = serde_json::from_str(r#"{"a":0,"b":{"x":1,"y":2}}"#).unwrap();
    assert_eq!(digest(&first).unwrap(), digest(&second).unwrap());
}

#[test]
fn management_receipts_are_atomic_and_uninstall_retries_do_not_resurrect() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(dir.path()).unwrap();
    let request: Operation =
        serde_json::from_value(json!({"op":"install","config":config()})).unwrap();
    let installed = store
        .manage(
            &who(),
            "install",
            "fingerprint",
            "key:owner",
            &request,
            Some(config()),
        )
        .unwrap();
    valid_id(installed["operation_id"].as_str().unwrap()).unwrap();
    let id = installed["server_ref"]["server_id"].as_str().unwrap();
    let revision = installed["config_revision"].as_str().unwrap();
    assert_eq!(
        store
            .manage(
                &who(),
                "install",
                "fingerprint",
                "key:owner",
                &request,
                None
            )
            .unwrap(),
        installed
    );
    assert_eq!(store.servers().unwrap().len(), 1);
    assert!(store
        .manage(&who(), "install", "changed", "key:owner", &request, None)
        .is_err());
    let mut remove:Operation=serde_json::from_value(json!({"op":"uninstall","server_ref":{"owner_origin":"key:owner","server_id":id},"expected_revision":"stale"})).unwrap();
    assert!(store
        .manage(&who(), "remove", "remove", "key:owner", &remove, None)
        .is_err());
    assert!(store
        .management_receipt(&who(), "remove", "remove")
        .unwrap()
        .is_none());
    remove.expected_revision = Some(revision.into());
    let removed = store
        .manage(&who(), "remove", "remove", "key:owner", &remove, None)
        .unwrap();
    drop(store);
    let store = store::Store::open(dir.path()).unwrap();
    assert_eq!(
        store
            .manage(&who(), "remove", "remove", "key:owner", &remove, None)
            .unwrap(),
        removed
    );
    assert_eq!(
        store
            .manage(
                &who(),
                "install",
                "fingerprint",
                "key:owner",
                &request,
                None
            )
            .unwrap(),
        installed
    );
    assert!(store.servers().unwrap().is_empty());
}

#[test]
fn retry_after_a_definitive_rejection_restores_the_recoverable_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let store = store::Store::open(dir.path()).unwrap();
    let request: Operation =
        serde_json::from_value(json!({"op":"install","config":config()})).unwrap();
    assert!(store
        .route(&who(), "install", "fingerprint", "key:owner", &request)
        .unwrap());
    store.reject_route(&who(), "install").unwrap();
    assert!(store.pending(&who()).unwrap().is_empty());
    assert!(store
        .route(&who(), "install", "fingerprint", "key:owner", &request)
        .unwrap());
    assert_eq!(store.pending(&who()).unwrap().len(), 1);
    assert!(!store
        .route(&who(), "install", "fingerprint", "key:owner", &request)
        .unwrap());
}
