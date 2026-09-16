use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};
use zork_agent::session::{events::ToolOutcome, tools::*};

struct Echo;
impl ToolImplementation for Echo {
    fn execute<'a>(
        &'a self,
        _: &'a ToolContext,
        args: &'a Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move { ToolExecution::success(args.clone()) })
    }
}
fn instance(name: &str, version: &str) -> Arc<ToolInstance> {
    Arc::new(
        ToolInstance::new(
            ToolContract {
                name: name.into(),
                version: ToolVersion::new(version).unwrap(),
                initial_description: "echo".into(),
                detailed_description: "exact name help".into(),
                input_schema: json!({"type":"object"}),
            },
            Arc::new(Echo),
            Arc::new(NoToolState),
        )
        .unwrap(),
    )
}
struct Namespace {
    prefix: &'static str,
    version: &'static str,
}
impl ToolNamespace for Namespace {
    fn introduction(&self) -> ToolIntroduction {
        ToolIntroduction {
            name: format!("{}*", self.prefix),
            version: ToolVersion::new(self.version).unwrap(),
            description: "namespace intro".into(),
        }
    }
    fn resolve(&self, name: &str) -> Option<Arc<ToolInstance>> {
        let suffix = name.strip_prefix(self.prefix)?;
        if suffix.is_empty() || suffix.contains('*') {
            None
        } else {
            Some(instance(name, self.version))
        }
    }
}
fn known(registry: &ToolRegistry) -> BTreeMap<String, ToolVersion> {
    registry
        .initial_catalog()
        .into_iter()
        .map(|t| (t.name, t.version))
        .collect()
}

#[test]
fn names_are_lazy_and_exact_removal_does_not_fall_back() {
    let registry = ToolRegistry::default();
    registry
        .register_namespace(
            "rpc.",
            Arc::new(Namespace {
                prefix: "rpc.",
                version: "v1",
            }),
        )
        .unwrap();
    let mut state = known(&registry);
    for i in 0..1000 {
        assert!(registry
            .current_contract(&format!("rpc.method{i}"))
            .is_some());
    }
    assert_eq!(registry.initial_catalog().len(), 1);
    let name = "rpc.example";
    let v = ToolVersion::new("v1").unwrap();
    assert!(matches!(
        registry.resolve(name, None),
        ToolResolution::VersionChanged { .. }
    ));
    assert!(matches!(
        registry.resolve(name, Some(&v)),
        ToolResolution::Ready(_)
    ));
    state.insert(name.into(), v);
    assert!(registry.changes(&state).is_empty());
    registry.register(instance(name, "exact"));
    assert_eq!(
        registry.current_contract(name).unwrap().version,
        ToolVersion::new("exact").unwrap()
    );
    assert!(registry.remove(name));
    assert!(registry.current_contract(name).is_none());
    assert!(matches!(
        registry.resolve(name, None),
        ToolResolution::Unavailable
    ));
    assert!(registry
        .changes(&state)
        .iter()
        .any(|c| matches!(c,ToolChange::Removed{name:n} if n==name)));
}

#[test]
fn longest_namespace_updates_known_names_and_survives_registry_recreation() {
    let registry = ToolRegistry::default();
    registry
        .register_namespace(
            "rpc.",
            Arc::new(Namespace {
                prefix: "rpc.",
                version: "v1",
            }),
        )
        .unwrap();
    registry
        .register_namespace(
            "rpc.nested.",
            Arc::new(Namespace {
                prefix: "rpc.nested.",
                version: "nested",
            }),
        )
        .unwrap();
    assert_eq!(
        registry
            .current_contract("rpc.nested.example")
            .unwrap()
            .version,
        ToolVersion::new("nested").unwrap()
    );
    let mut state = known(&registry);
    state.insert("rpc.example".into(), ToolVersion::new("v1").unwrap());
    registry
        .register_namespace(
            "rpc.",
            Arc::new(Namespace {
                prefix: "rpc.",
                version: "v2",
            }),
        )
        .unwrap();
    assert!(registry
        .changes(&state)
        .iter()
        .any(|c| matches!(c,ToolChange::Updated{name,..} if name=="rpc.example")));
    let restarted = ToolRegistry::default();
    restarted
        .register_namespace(
            "rpc.",
            Arc::new(Namespace {
                prefix: "rpc.",
                version: "v1",
            }),
        )
        .unwrap();
    restarted
        .register_namespace(
            "rpc.nested.",
            Arc::new(Namespace {
                prefix: "rpc.nested.",
                version: "nested",
            }),
        )
        .unwrap();
    assert!(restarted.changes(&state).is_empty());
    assert!(restarted.compatibility("rpc.example").is_some());
    assert!(restarted.remove_namespace("rpc."));
    assert!(restarted.current_contract("rpc.example").is_none());
    assert!(restarted.compatibility("rpc.example").is_some());
    assert!(matches!(
        restarted.resolve("rpc.example", None),
        ToolResolution::Unavailable
    ));
}

#[tokio::test]
async fn help_teaches_the_exact_version_and_execution_uses_original_arguments() {
    let registry = Arc::new(ToolRegistry::default());
    registry
        .register_namespace(
            "rpc.",
            Arc::new(Namespace {
                prefix: "rpc.",
                version: "v1",
            }),
        )
        .unwrap();
    let help = tool_help_instance(&registry, ToolVersion::new("help").unwrap()).unwrap();
    let context = ToolContext {
        session_id: "s".into(),
        invocation_id: "i".into(),
        workspace: "/unused".into(),
        control: None,
    };
    let result = help.execute(&context, &json!({"tool":"rpc.example"})).await;
    assert_eq!(result.outcome, ToolOutcome::Succeeded);
    let Some(ToolKnowledge::Current { name, version }) = result.knowledge else {
        panic!("exact tool knowledge missing")
    };
    assert_eq!(name, "rpc.example");
    let ToolResolution::Ready(tool) = registry.resolve(&name, Some(&version)) else {
        panic!("help version not executable")
    };
    let args = json!({"connect_id":"b","nested":{"unknown":[1,true,null,"中文"]}});
    assert_eq!(tool.execute(&context, &args).await.data, args);
}
