use serde_json::json;
use zork_client_core::{Client, Command};

#[test]
fn platform_preferences_are_local_and_persist_without_a_connected_peer() {
    let root = tempfile::tempdir().unwrap();
    let execute = |request| {
        let command: Command = serde_json::from_value(request).unwrap();
        assert!(command.is_local());
        Client::open(root.path()).unwrap().local().execute(command)
    };
    assert_eq!(
        execute(json!({"op":"preferences"})).unwrap()["message_preview_height"],
        0
    );
    assert_eq!(
        execute(json!({"op":"preferences","message_preview_height":480})).unwrap()
            ["message_preview_height"],
        480
    );
    assert!(execute(json!({"op":"preferences","message_preview_height":999})).is_err());
    assert_eq!(
        execute(json!({"op":"preferences"})).unwrap()["message_preview_height"],
        480
    );
}

#[test]
fn an_empty_client_can_reopen_after_persisting_no_selected_device() {
    let root = tempfile::tempdir().unwrap();
    for command in [
        Command::RestoreNavigation { peer: None },
        Command::SelectPeer { peer: None },
    ] {
        Client::open(root.path())
            .unwrap()
            .local()
            .execute(command)
            .unwrap();
        let client = Client::open(root.path()).unwrap();
        let mut invitation = client
            .local()
            .observe(zork_client_core::subscriptions::Key::Invitation)
            .unwrap();
        let opening = invitation.prepare().unwrap().unwrap();
        assert!(opening["snapshot"]["selected_peer"].is_null());
        assert!(opening["snapshot"]["nodes"].as_array().unwrap().is_empty());
    }
}
