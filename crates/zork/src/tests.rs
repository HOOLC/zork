use super::*;

#[tokio::test]
async fn legacy_supervisor_is_rejected_before_any_mutating_command() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("run")).unwrap();
    let listener = UnixListener::bind(zork_config::zork_sock_path(root.path())).unwrap();
    let supervisor = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        let mut command = String::new();
        stream.read_line(&mut command).await.unwrap();
        assert_eq!(command, "status\n");
        stream
            .get_mut()
            .write_all(b"{\"protocol\":1,\"pid\":123}\n")
            .await
            .unwrap();
        listener
    });

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        send_reload(vec![
            "--data".into(),
            root.path().to_string_lossy().into_owned(),
        ]),
    )
    .await
    .expect("legacy supervisor rejection must complete")
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("restart the zork supervisor once"));

    let listener = supervisor.await.unwrap();
    // The command has completed. Drain a pending connection before accepting
    // completion, so an attempted reload cannot hide behind task scheduling.
    tokio::select! {
        biased;
        connection = listener.accept() => panic!("unexpected mutating connection: {connection:?}"),
        _ = std::future::ready(()) => {}
    }
}
