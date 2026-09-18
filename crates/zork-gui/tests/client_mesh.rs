use futures_util::StreamExt;
use serde_json::json;
use zork_gui::api::{StationClient, Role, TranscriptMessage};
#[test]
#[ignore = "run scripts/test-client-mesh.py with isolated real node and Synch"]
fn client_mesh() {
    let root = std::path::PathBuf::from(std::env::var("ZORK_TEST_CLIENT_ROOT").unwrap());
    let origin = std::env::var("ZORK_TEST_REMOTE_ORIGIN").unwrap();
    let config = zork_config::load_config(&root).unwrap().mesh;
    let transport = zork_gui::desktop::transport::ClientMesh::new(root.clone());
    let nodes = config
        .peers
        .iter()
        .map(|peer| zork_gui::desktop::store::SavedNode {
            id: peer.origin.clone(),
            name: peer.name.clone(),
            url: String::new(),
            token: None,
            local: false,
            group: None,
            mesh: Some(zork_gui::desktop::store::RemoteNode {
                origin: peer.origin.clone(),
                addr: peer.addr.clone(),
            }),
        })
        .collect::<Vec<_>>();
    transport.start_on(&nodes, Some(&config)).unwrap();
    let ready: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("client-mesh-ready.json")).unwrap())
            .unwrap();
    assert_eq!(ready["pid"], std::process::id());
    assert_eq!(ready["embedded"], true);
    let probe = std::process::Command::new("pgrep")
        .args(["-P", &std::process::id().to_string()])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let probe_pid = probe.id().to_string();
    let children = probe.wait_with_output().unwrap();
    assert!(
        String::from_utf8(children.stdout)
            .unwrap()
            .split_whitespace()
            .all(|pid| pid == probe_pid),
        "desktop Mesh started a child process"
    );
    let control = transport.control();
    let client = std::sync::Arc::new(StationClient::new_mesh(control.clone(), origin.clone()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // Runtime readiness precedes asynchronous pkarr publication. Wait for a
        // read-only request to become reachable; never replay a mutation here.
        let provider = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                match client.list_profiles().await {
                    Ok(profiles) => break profiles,
                    Err(error) if error.to_string().contains("No addressing information available") => {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    Err(error) => panic!("remote profiles failed: {error}"),
                }
            }
        }).await.expect("remote address was not published within 20 seconds");
        assert_eq!(provider.len(),1);
        let mut updates=client.stream_updates().await.unwrap();
        let initial=tokio::time::timeout(std::time::Duration::from_secs(3),updates.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(initial.name,"changed");
        assert!(tokio::time::timeout(std::time::Duration::from_secs(35),updates.next()).await.is_err(),"idle subscription must stay open past the old bridge/request deadlines without updates");
        let agent=client.node_request(reqwest::Method::POST,"/v1/node/agents".into(),Some(json!({"id":"mesh-leader","name":"Remote Leader","avatar":"panda","role":"leader","profile_id":"fixture","model":"fixture-model","thinking":"off"}))).await.unwrap();
        assert_eq!(agent["avatar"], "panda");
        let change=tokio::time::timeout(std::time::Duration::from_secs(3),updates.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(change.name,"changed");
        assert_eq!(client.node_request(reqwest::Method::POST,"/v1/node/agents".into(),Some(json!({"id":"bad-avatar","name":"Bad","avatar":"unknown","role":"leader","profile_id":"fixture","model":"fixture-model","thinking":"off"}))).await.unwrap_err().status(),Some(400));
        let changed=client.node_request(reqwest::Method::PUT,"/v1/node/agents/mesh-leader/avatar".into(),Some(json!({"avatar":"fox"}))).await.unwrap();
        assert_eq!(changed["avatar"],"fox");assert_eq!(changed["session_id"],agent["session_id"]);
        assert_eq!(client.node_request(reqwest::Method::PUT,"/v1/node/agents/mesh-leader/avatar".into(),Some(json!({"avatar":"../bad"}))).await.unwrap_err().status(),Some(400));
        assert_eq!(client.node_request(reqwest::Method::PUT,"/v1/node/agents/missing/avatar".into(),Some(json!({"avatar":"cat"}))).await.unwrap_err().status(),Some(404));
        let saved=client.node_request(reqwest::Method::GET,"/v1/node/agents".into(),None).await.unwrap();assert_eq!(saved["items"][0]["avatar"],"fox");
        let opened=client.node_request(reqwest::Method::POST,"/v1/node/agents/mesh-leader/open".into(),Some(json!({}))).await.unwrap();let id=opened["session_id"].as_str().unwrap();assert_eq!(agent["session_id"],id);
        let participants=client.node_request(reqwest::Method::GET,format!("/v1/im/sessions/{id}/status"),None).await.unwrap();
        // The browser lives on an access-only client. Exercise reverse delivery
        // over real Synch with a deterministic browser responder; native Chrome
        // and GUI interaction have separate real-browser coverage.
        let browser_path=format!("/v1/im/sessions/{id}/browser/poll");
        let browser_poll=json!({"client_id":"mesh-browser-fixture","secret":"a".repeat(64),"name":"Client browser","replies":[]});
        client.node_request(reqwest::Method::POST,browser_path.clone(),Some(browser_poll.clone())).await.unwrap();
        client.post_message_id(id,&json!({"fake_tools":[{"name":"browser","input":{"request_id":"mesh-browser-list","action":{"op":"list"}}}]}).to_string(),"mesh-browser-message").await.unwrap();
        let queued=client.node_request(reqwest::Method::POST,browser_path.clone(),Some(browser_poll.clone())).await.unwrap();
        assert_eq!(queued["commands"][0]["request_id"],"mesh-browser-list");
        let mut reply=browser_poll.clone();
        reply["replies"]=json!([{"request_id":"mesh-browser-list","result":{"tabs":[{"id":"client-owned-tab","title":"Mesh browser fixture"}]}}]);
        let received=client.node_request(reqwest::Method::POST,browser_path.clone(),Some(reply.clone())).await.unwrap();
        assert_eq!(received["accepted"],json!(["mesh-browser-list"]));
        assert_eq!(client.node_request(reqwest::Method::POST,browser_path.clone(),Some(reply)).await.unwrap()["commands"],json!([]));
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {
                let history=client.node_request(reqwest::Method::GET,format!("/v1/im/sessions/{id}/history?limit=100"),None).await.unwrap();
                if history["items"].as_array().unwrap().iter().any(|r|r["event"]["kind"]=="tool_result" && r.to_string().contains("client-owned-tab")){break;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.expect("browser result did not reach the remote Agent");
        let mut disconnect=browser_poll;disconnect["disconnect"]=json!(true);
        client.node_request(reqwest::Method::POST,browser_path,Some(disconnect)).await.unwrap();
        let denied_browser=control.exchange(&origin,&json!({"v":1,"request":{"kind":"client","method":"POST","path":"/v1/browser/command","body":null}})).await.unwrap();
        assert_eq!(denied_browser["ok"],false);
        assert_eq!(participants["items"][0]["id"],"mesh-leader");
        assert_eq!(participants["items"][0]["avatar"],"fox");
        assert_eq!(participants["items"][0]["session_id"],id);
        let session=client.list_sessions().await.unwrap().into_iter().find(|s|s.session_id==id).unwrap();
        let report=std::path::Path::new(&session.workspace).join("large-report.txt");let content="local Mesh artifact\n".repeat(20_000);std::fs::write(&report,&content).unwrap();
        let input=json!({"fake_tools":[{"name":"chat.post_file","input":{"file_path":report,"initial_comment":"Mesh client transfer"}},{"name":"chat.post_message","input":{"kind":"final","text":"Delivered to remote client"}}]}).to_string();
        let quota = client.node_request(reqwest::Method::POST,"/v1/node/profiles/fixture/refresh".into(),None).await.unwrap();
        assert_eq!(quota["profile_id"],"fixture");
        assert!(quota.get("rateLimits").is_some() && quota.get("auth").is_none());
        assert!(!quota.to_string().contains("sk-test"));
        let renamed = client.node_request(reqwest::Method::PUT,"/v1/node/profiles/fixture/name".into(),Some(json!({"name":"Remote connection"}))).await.unwrap();
        assert_eq!(renamed["profile_id"],"fixture");
        assert_eq!(renamed["name"],"Remote connection");
        assert!(renamed.get("auth").is_none());
        let disabled=client.node_request(reqwest::Method::PUT,"/v1/node/profiles/fixture/models/enabled".into(),Some(json!({"model_id":"fixture-model","enabled":false}))).await.unwrap();
        assert_eq!(disabled["models"][0]["enabled"],false);
        assert!(client.node_request(reqwest::Method::POST,"/v1/node/agents".into(),Some(json!({"id":"disabled-mesh-agent","name":"Disabled model","role":"worker","profile_id":"fixture","model":"fixture-model","thinking":"off"}))).await.is_err());
        client.node_request(reqwest::Method::PUT,"/v1/node/profiles/fixture/models/enabled".into(),Some(json!({"model_id":"fixture-model","enabled":true}))).await.unwrap();
        let mut activity=client.stream_events(id).await.unwrap();
        client.post_message_id(id,&input,"remote-client-send").await.unwrap();client.post_message_id(id,&input,"remote-client-send").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(20),async {
            while let Some(frame)=activity.next().await {let frame=frame.unwrap();if frame.name=="message" {
                let message:TranscriptMessage=serde_json::from_str(&frame.data).unwrap();
                if matches!(message,TranscriptMessage::Message{role:Role::Assistant,content,..} if content=="Delivered to remote client"){return}
            }}panic!("remote message stream ended before delivery");
        }).await.unwrap();
        let artifacts=client.artifacts().await.unwrap();assert_eq!(artifacts.len(),1);assert!(artifacts[0].task_id.is_none());
        assert_eq!(client.artifact_content(&artifacts[0].artifact_id).await.unwrap(),content.as_bytes());
        let history=client.node_request(reqwest::Method::GET,format!("/v1/im/sessions/{id}/history?limit=100"),None).await.unwrap();
        assert!(history["items"].as_array().unwrap().iter().any(|r|r["event"]["kind"]=="step_completed"));
        assert!(history["items"].as_array().unwrap().iter().any(|r|r["event"]["kind"]=="tool_result"));
        let denied_history=control.exchange(&origin,&json!({"v":1,"request":{"kind":"client","method":"POST","path":format!("/v1/im/sessions/{id}/history"),"body":null}})).await.unwrap();assert_eq!(denied_history["ok"],false);
        let denied=control.exchange(&origin,&json!({"v":1,"request":{"kind":"client","method":"GET","path":"/v1/tools/context?threadId=pretend","body":null}})).await.unwrap();assert_eq!(denied["ok"],false);
        // Client-side file bytes must cross the real bounded Mesh RPC transport.
        // The conversation owner cannot read this source path after it is deleted.
        let store=std::sync::Arc::new(zork_client_core::store::ClientStore::open(&root.join("files-client")).unwrap());
        let device=zork_client_core::state::Device::open(client.clone(),Some((store.clone(),origin.clone())),true);
        let source=root.join("client-only.dat");
        let uploaded=(0..410_000).map(|i|(i%251) as u8).collect::<Vec<_>>();
        std::fs::write(&source,&uploaded).unwrap();
        let file=device.attach_path(id,&source).unwrap();
        std::fs::remove_file(&source).unwrap();
        let queued=device.submit_draft(id,"Read the attached binary snapshot").unwrap().unwrap();
        // No message can be accepted before the complete referenced snapshot exists.
        assert_eq!(client.post_message_id(id,&queued.content,&queued.request_id).await.unwrap_err().status(),Some(400));
        let report=zork_client_core::delivery::flush(&client,&store,&origin).await;
        assert!(report.error.is_none(),"{:?}",report.error);
        assert_eq!(report.delivered,vec![queued.request_id.clone()]);
        assert_eq!(client.artifact_content(&file.id).await.unwrap(),uploaded);
        client.post_message_id(id,&queued.content,&queued.request_id).await.unwrap();
        let messages=client.list_messages(id, None, 100).await.unwrap().items;
        assert_eq!(messages.iter().filter(|m| matches!(m,TranscriptMessage::Message{metadata,..} if metadata.id.as_deref()==Some(&format!("client-{id}-{}",queued.request_id)))).count(),1);
        let other=client.node_request(reqwest::Method::POST,"/v1/node/agents".into(),Some(json!({"id":"other-files","name":"Other","role":"leader","profile_id":"fixture","model":"fixture-model","thinking":"off"}))).await.unwrap();
        let other_id=other["session_id"].as_str().unwrap();
        client.node_request(reqwest::Method::POST,"/v1/node/agents/other-files/open".into(),Some(json!({}))).await.unwrap();
        assert_eq!(client.post_message_id(other_id,&queued.content,"cross-conversation").await.unwrap_err().status(),Some(400));
        let uploaded_artifact=client.artifacts().await.unwrap().into_iter().find(|a|a.artifact_id==file.id).unwrap();
        assert_eq!(uploaded_artifact.session_id.as_deref(),Some(id));
        assert!(uploaded_artifact.task_id.is_none());
        let history=client.node_request(reqwest::Method::GET,format!("/v1/im/sessions/{id}/history?limit=100"),None).await.unwrap();
        assert!(history.to_string().contains("conversation-files"));
        let path=std::env::var("ZORK_TEST_REMOTE_CONFIG").unwrap();let mut config:serde_json::Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();config["mesh"]["peers"][0]["client"]=json!(false);std::fs::write(&path,serde_json::to_vec(&config).unwrap()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5),async {while updates.next().await.is_some(){}}).await.expect("revocation must close the existing subscription via the config watcher");
        drop(activity);
        assert_eq!(client.list_sessions().await.unwrap_err().status(),Some(403));
        assert_eq!(client.node_request(reqwest::Method::PUT,"/v1/node/agents/mesh-leader/avatar".into(),Some(json!({"avatar":"cat"}))).await.unwrap_err().status(),Some(403));
        assert_eq!(client.node_request(reqwest::Method::GET,format!("/v1/im/sessions/{id}/history"),None).await.unwrap_err().status(),Some(403));
    });
    let stopped = transport.shutdown();
    rt.block_on(async {
        stopped.await.unwrap();
    });
    assert!(!root.join("client-mesh-ready.json").exists());
}
