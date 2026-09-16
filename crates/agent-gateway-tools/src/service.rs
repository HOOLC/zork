//! Service tool contracts. Operational guidance lives in the bundled skill.
use super::Kind;
use serde_json::{json, Value};
type Definition = (Kind, &'static str, &'static str, Value, Vec<&'static str>);
pub(super) fn definitions() -> Vec<Definition> {
    let id = || json!({"type":"string","minLength":1,"maxLength":128});
    let name = || json!({"type":"string","minLength":1,"maxLength":80});
    let port = || json!({"type":"integer","minimum":1,"maximum":65535});
    vec![
        (Kind::ServiceOp("attach"), "service.attach", "Register an externally managed HTTP service on this node's loopback port. Returns a stable Mesh URL and lists the service in the client. The Agent owns the process through shell.run or job.register. Identical name/port reuses the record; conflicting configuration is rejected.", json!({"name":name(),"port":port()}), vec!["name","port"]),
        (Kind::ServiceOp("list"), "service.list", "List this Session's registered services, including previously registered records. Returns saved configuration, process state, Mesh URL and node identity; does not probe HTTP readiness or read log files.", json!({}), vec![]),
        (Kind::ServiceOp("inspect"), "service.inspect", "Inspect a Session-owned service by id. Returns process state, TCP readiness, last exit code/error, revision, sharing URL and node-qualified stdout/stderr file paths for managed services. External services have no captured logs. Does not read log contents or change desired state.", json!({"id":id()}), vec!["id"]),
    ]
}
