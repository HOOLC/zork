use std::{fs, sync::Arc};
use zork_agent::session::{service::ServiceOptions, wire::SessionSelection};
use zork_agent::skills::{catalog_source, provision_bundled, CATALOG_NOTICE};
use zork_agent_testkit::TestWorld;

fn selection() -> SessionSelection {
    SessionSelection {
        profile_id: "test-profile".into(),
        model: "test-model".into(),
        thinking: "medium".into(),
    }
}

fn write_skill(root: &std::path::Path, name: &str, description: &str) {
    let directory = root.join("skills").join(name);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\nSecret full instructions"),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn model_requests_carry_current_skill_catalog_without_rewriting_prefix() {
    let root = tempfile::tempdir().unwrap();
    provision_bundled(root.path()).unwrap();
    write_skill(root.path(), "build", "Build on this device");
    let mut options = ServiceOptions::default();
    options.runner.skill_catalog = Some(catalog_source(root.path().to_owned()));
    let mut world = TestWorld::with_options(options);
    let id = world
        .create_session(selection(), None, root.path().to_string_lossy())
        .await
        .unwrap();
    world.send_mail(&id, "start").await.unwrap();
    let first = world.request().await;
    let original = first.transcript.clone();
    let catalogs = |transcript: &[zork_agent::session::wire::ProviderMessage]| {
        transcript
            .iter()
            .filter(|m| m.content.contains(CATALOG_NOTICE))
            .map(|m| m.content.to_string())
            .collect::<Vec<_>>()
    };
    let current = catalogs(&original);
    assert_eq!(current.len(), 1);
    assert!(current[0].contains("Build on this device"));
    assert!(current[0].contains("device-onboarding (bundled)"));
    assert!(current[0].contains("/skills/bundled/device-onboarding/SKILL.md"));
    assert!(!current[0].contains("Secret full instructions"));

    // Unchanged files are not repeated.
    world.send_mail(&id, "continue").await.unwrap();
    first.respond_text("working").unwrap();
    let second = world.request().await;
    assert_eq!(catalogs(&second.transcript).len(), 1);
    assert_eq!(&second.transcript[..original.len()], original.as_slice());

    // A user override and a removal appear as a new catalog after the prefix.
    fs::remove_dir_all(root.path().join("skills/build")).unwrap();
    write_skill(root.path(), "my-slack", "slack");
    fs::write(
        root.path().join("skills/my-slack/SKILL.md"),
        "---\nname: slack\ndescription: My own Slack rules\n---\nbody",
    )
    .unwrap();
    world.send_mail(&id, "files changed").await.unwrap();
    second.respond_text("working").unwrap();
    let third = world.request().await;
    let all = catalogs(&third.transcript);
    assert_eq!(all.len(), 2);
    let latest = all.last().unwrap();
    assert!(!latest.contains("Build on this device"));
    assert!(latest.contains("My own Slack rules"));
    assert!(!latest.contains("slack (bundled)"));
    assert_eq!(&third.transcript[..original.len()], original.as_slice());
    third.respond_text("done").unwrap();
    world
        .wait_for_state(&id, |state| state.active_turn.is_none())
        .await;
    world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn handoff_successor_receives_current_catalog() {
    use zork_agent::session::model::{ModelOutcome, ModelTokenUsage};
    let root = tempfile::tempdir().unwrap();
    write_skill(root.path(), "build", "Original description");
    let mut options = ServiceOptions::default();
    options.runner.context.strategy = zork_agent_api::ContextStrategy::Handoff;
    options.runner.input_budget = Arc::new(|_| Some(100));
    options.runner.skill_catalog = Some(catalog_source(root.path().to_owned()));
    let mut world = TestWorld::with_options(options);
    let id = world
        .create_session(selection(), None, root.path().to_string_lossy())
        .await
        .unwrap();
    world.send_mail(&id, "start").await.unwrap();
    let first = world.request().await;
    world
        .send_mail(&id, "continue the same task")
        .await
        .unwrap();
    first
        .respond(Ok(ModelOutcome {
            text: "progress".into(),
            tool_calls: vec![],
            provider_context: None,
            provider_input: None,
            usage: Some(ModelTokenUsage {
                input_tokens: 100,
                cached_input_tokens: None,
                output_tokens: 1,
                output_reasoning_tokens: None,
                output_text_tokens: None,
            }),
        }))
        .unwrap();
    let handoff = world.request().await;
    assert!(handoff
        .transcript
        .iter()
        .any(|m| m.content.contains("A context handoff is required")));
    write_skill(root.path(), "build", "Updated description");
    handoff
        .respond_call(
            "handoff",
            "handoff",
            serde_json::json!({"document":"Continue the task; work remains."}),
        )
        .unwrap();
    let successor = world.request().await;
    assert_eq!(successor.generation, 2);
    assert!(successor
        .transcript
        .iter()
        .any(|m| m.content.contains(CATALOG_NOTICE) && m.content.contains("Updated description")));
    successor.respond_text("done").unwrap();
    world.shutdown().await;
}
