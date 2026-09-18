use zork_agent::skills;

#[test]
fn service_sharing_skill_is_shipped_and_discoverable() {
    let temp = tempfile::tempdir().unwrap();
    skills::management::provision_bundled(temp.path()).unwrap();
    let sources = zork_config::skill_bundles::sources(temp.path()).unwrap();
    let catalog = skills::discover(&sources);
    let service = catalog
        .skills
        .iter()
        .find(|skill| skill.name == "service-sharing")
        .expect("service-sharing must be available to Station Agents");
    assert!(service
        .path
        .starts_with(temp.path().canonicalize().unwrap()));
    assert!(!skills::read_document(&service.path).unwrap().is_empty());
}
