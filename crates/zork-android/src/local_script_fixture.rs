//! Debug-only ingress fixture. Execution still uses cached messages and the production command.
use super::*;
use zork_client_core::{
    api::MessagePage,
    store::{ClientStore, SavedNode},
};

pub(super) fn seed(root: &Path, input: &str) -> Result<Value> {
    ensure!(input.len() <= 96 * 1024, "fixture too large");
    let card: zork_client_core::api::TranscriptMessage = serde_json::from_value(json!({
        "type":"message", "role":"assistant", "id":"local-script-card", "content":"",
        "interaction":serde_json::from_str::<Value>(input)?,
    }))?;
    let _host = host(root)?;
    let store = ClientStore::open(root)?;
    store.save_node(&SavedNode {
        id: "local-script-node".into(),
        name: "Local script fixture".into(),
        url: String::new(),
        token: None,
        local: false,
        mesh: None,
        group: None,
    })?;
    // Each fixture gets a distinct chat so immutable source IDs are never edited.
    let chat = ulid::Ulid::new().to_string();
    store.cache_message_page(
        "local-script-node",
        &chat,
        &MessagePage {
            source_epoch: None,
            items: vec![card],
            older_cursor: None,
        },
        None,
    )?;
    let mut value = call(
        root,
        &json!({"op":"cached_messages","peer":"local-script-node","session":chat}).to_string(),
    )?;
    value["chat"] = json!(chat);
    Ok(value)
}
