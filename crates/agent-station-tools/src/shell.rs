//! The host adds target routing to the ordinary shell invocation.
use super::*;
use zork_agent::session::tools::ToolResolution;

struct Shell {
    local: Arc<ToolInstance>,
    remote: Arc<dyn ToolImplementation>,
}
impl Shell {
    fn remote(args: &Value) -> bool {
        args["target"]
            .as_str()
            .is_some_and(|target| target != "local")
    }
    fn local_args(args: &Value) -> Value {
        let mut args = args.clone();
        if let Some(fields) = args.as_object_mut() {
            fields.remove("target");
        }
        args
    }
}
impl ToolImplementation for Shell {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            if args
                .get("target")
                .is_some_and(|target| target.as_str().is_none_or(|value| value.trim().is_empty()))
            {
                let mut result = ToolExecution::success(
                    json!({"error":"target must be a nonempty node identity"}),
                );
                result.outcome = ToolOutcome::Failed;
                return result;
            }
            if Self::remote(args) {
                self.remote.execute(context, args).await
            } else {
                self.local.execute(context, &Self::local_args(args)).await
            }
        })
    }
    fn cancel<'a>(
        &'a self,
        context: &'a ToolContext,
        args: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<ToolExecution>> + Send + 'a>> {
        Box::pin(async move {
            if Self::remote(args) {
                self.remote.cancel(context, args).await
            } else {
                self.local.cancel(context, &Self::local_args(args)).await
            }
        })
    }
}
pub fn extend_shell(registry: &Arc<ToolRegistry>, base: &str) -> anyhow::Result<()> {
    let mut contract = registry
        .current_contract("shell.run")
        .ok_or_else(|| anyhow::anyhow!("shell builtin missing"))?;
    let ToolResolution::Ready(local) = registry.resolve("shell.run", Some(&contract.version))
    else {
        anyhow::bail!("shell builtin unavailable")
    };
    contract.version = ToolVersion::new("shell-target-1")?;
    contract.initial_description = "Run a command locally, or on a Mesh node selected by target. Completes through the ordinary tool lifecycle; full output is kept in the invocation live log.".into();
    contract.detailed_description.push_str(" Optional target is a node identity from device.list; omitted or local uses this Session workspace. Remote cwd must be absolute on the target; omitted remote cwd uses its workspace for this Session. env overrides the command environment. Use wait for completion, tool.cancel for cancellation, and history.list/file.read for results. Do not repeat commands whose effects remain uncertain.");
    contract.input_schema["properties"]["target"] = json!({"type":"string","minLength":1});
    registry.register(Arc::new(
        ToolInstance::new(
            contract,
            Arc::new(Shell {
                local,
                remote: namespaced::remote_shell(base)?,
            }),
            Arc::new(history::Results),
        )?
        .with_activity(|_| ToolActivity::new("执行命令", "Running command", "")),
    ));
    Ok(())
}
