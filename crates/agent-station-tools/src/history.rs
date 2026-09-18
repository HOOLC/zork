//! Historical results remain readable after an interface is removed.
use super::*;
use zork_agent::session::{events::OutstandingItem, tools::ToolState};

pub(super) struct Results;
impl ToolCompatibility for Results {
    fn migrate_result(&self, version: u32, value: Value) -> Result<Value, String> {
        match version {
            1 | 2 => Ok(value),
            _ => Err(format!("unsupported station result version {version}")),
        }
    }
    fn migrate_state(&self, state: ToolState) -> Result<ToolState, String> {
        Ok(state)
    }
    fn initial_state(&self) -> Option<ToolState> {
        None
    }
    fn fold(&self, _: Option<&ToolState>, _: &Value) -> Result<Option<ToolState>, String> {
        Ok(None)
    }
    fn outstanding(&self, _: Option<&ToolState>) -> Vec<OutstandingItem> {
        Vec::new()
    }
}

pub(super) fn register(registry: &ToolRegistry) {
    for name in [
        "mcp",
        "mcp.setup",
        "mcp.installed",
        "mcp.configure",
        "mcp.probe",
        "mcp.enable",
        "mcp.disable",
        "mcp.share",
        "mcp.status",
        "mcp.read",
        "mcp.cancel",
        "mcp.recover",
        "device.exec",
        "device.agents",
        "device.status",
        "device.read",
        "device.cancel",
        "device.recover",
        "chat.post_page",
        "chat.search",
        "chat.recover",
        "agent.interrupt",
        "agent.message",
        "agent.recover",
        "channels.operations",
        "channel.operations",
        "provider.login",
        "android.devices",
        "browser",
        "service",
        "service.start",
        "service.stop",
        "service.restart",
        "service.share",
        "service.unshare",
        "page.publish",
        "page.unpublish",
        "page.deliver",
        "skill.installed",
        "skill.install",
        "skill.import",
        "skill.export",
        "skill.share",
        "skill.reference",
        "skill.bind",
        "skill.unbind",
        "skill.bindings",
        "skill.uninstall",
    ] {
        registry.register_retired(name, Arc::new(Results));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_tools_remain_readable_without_becoming_callable_or_blocking_completion() {
        let registry = Arc::new(ToolRegistry::default());
        crate::register(&registry, "http://127.0.0.1:9".into()).unwrap();
        for name in [
            "chat.search",
            "chat.recover",
            "agent.interrupt",
            "agent.message",
            "agent.recover",
            "device.agents",
            "device.exec",
            "mcp.share",
            "mcp.installed",
            "mcp.configure",
            "mcp.probe",
            "mcp.enable",
            "mcp.disable",
            "skill.import",
            "skill.bind",
            "service.start",
            "service.share",
            "page.publish",
            "provider.login",
            "android.devices",
            "browser",
        ] {
            assert!(registry.current_contract(name).is_none(), "{name}");
            let compatibility = registry.compatibility(name).expect(name);
            let old = json!({"state":"outcome_unknown","operation_id":"old"});
            assert_eq!(compatibility.migrate_result(1, old.clone()).unwrap(), old);
            assert!(compatibility.outstanding(None).is_empty());
            assert!(compatibility.fold(None, &old).unwrap().is_none());
        }
        assert!(registry.current_contract("client.browser").is_some());
        let mcp: Vec<_> = registry
            .initial_catalog()
            .into_iter()
            .filter(|tool| tool.name.starts_with("mcp."))
            .map(|tool| tool.name)
            .collect();
        assert_eq!(
            mcp,
            [
                "mcp.call",
                "mcp.inspect",
                "mcp.install",
                "mcp.list",
                "mcp.search",
                "mcp.uninstall",
                "mcp.update"
            ]
        );
    }
}
