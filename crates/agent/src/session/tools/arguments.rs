//! Builtin-owned argument parsing. Custom tools receive their original values.
use super::BuiltinKind;
use serde_json::{Map, Value};

fn fields<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "Tool arguments must be an object.".to_owned())?;
    if let Some(name) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!(
            "Unknown argument {name:?}; use tool.help for this tool's current usage."
        ));
    }
    Ok(object)
}

fn text(object: &Map<String, Value>, name: &str, nonempty: bool) -> Result<(), String> {
    if object
        .get(name)
        .and_then(Value::as_str)
        .is_some_and(|s| !nonempty || !s.is_empty())
    {
        Ok(())
    } else {
        Err(format!(
            "{name} must be {}string.",
            if nonempty { "a non-empty " } else { "a " }
        ))
    }
}

fn unsigned(object: &Map<String, Value>, name: &str, min: u64, max: u64) -> Result<(), String> {
    if let Some(value) = object.get(name) {
        if !value.as_u64().is_some_and(|v| (min..=max).contains(&v)) {
            return Err(format!(
                "{name} must be an integer between {min} and {max}."
            ));
        }
    }
    Ok(())
}

fn duration(object: &Map<String, Value>, name: &str) -> Result<(), String> {
    let seconds = object
        .get(name)
        .and_then(Value::as_f64)
        .filter(|v| *v > 0.0 && v.is_finite())
        .ok_or_else(|| format!("{name} must be a positive number."))?;
    std::time::Duration::try_from_secs_f64(seconds)
        .map(|_| ())
        .map_err(|_| format!("{name} is too large."))
}

pub(super) fn help(value: &Value) -> Result<(), String> {
    text(fields(value, &["tool"])?, "tool", true)
}

pub(super) fn builtin(kind: BuiltinKind, value: &Value) -> Result<(), String> {
    match kind {
        BuiltinKind::End => {
            // end owns this policy: only acknowledge_outstanding has end
            // semantics; descriptive extras do not alter the decision.
            let o = value
                .as_object()
                .ok_or_else(|| "Tool arguments must be an object.".to_owned())?;
            if o.get("acknowledge_outstanding")
                .is_some_and(|v| !v.is_boolean())
            {
                return Err("acknowledge_outstanding must be a boolean.".into());
            }
        }
        BuiltinKind::Wait => {
            let o = fields(value, &["seconds", "reason"])?;
            duration(o, "seconds")?;
            if o.contains_key("reason") {
                text(o, "reason", false)?;
            }
        }
        BuiltinKind::ToolCancel => text(fields(value, &["invocation_id"])?, "invocation_id", true)?,
        BuiltinKind::HistoryList => {
            let o = fields(value, &["before_event_id", "limit"])?;
            if o.contains_key("before_event_id") {
                text(o, "before_event_id", true)?;
            }
            unsigned(o, "limit", 1, 200)?;
        }
        BuiltinKind::FileRead => {
            let o = fields(value, &["path", "offset", "limit"])?;
            text(o, "path", true)?;
            unsigned(o, "offset", 0, u64::MAX)?;
            unsigned(o, "limit", 1, 1024 * 1024)?;
        }
        BuiltinKind::FileList => {
            let o = fields(value, &["path", "cursor", "limit"])?;
            text(o, "path", true)?;
            if o.contains_key("cursor") {
                text(o, "cursor", true)?;
            }
            unsigned(o, "limit", 1, 128)?;
        }
        BuiltinKind::FileMaterialize => text(fields(value, &["path"])?, "path", true)?,
        BuiltinKind::FileWrite => {
            let o = fields(value, &["path", "content"])?;
            text(o, "path", true)?;
            text(o, "content", false)?;
        }
        BuiltinKind::FileEdit => {
            let o = fields(value, &["path", "edits"])?;
            text(o, "path", true)?;
            let edits = o
                .get("edits")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
                .ok_or_else(|| "edits must be a non-empty array.".to_owned())?;
            for edit in edits {
                let e = fields(edit, &["old_text", "new_text"])?;
                text(e, "old_text", true)?;
                text(e, "new_text", false)?;
            }
        }
        BuiltinKind::ShellRun => {
            let o = fields(value, &["command", "cwd", "env"])?;
            text(o, "command", true)?;
            if o.contains_key("cwd") {
                text(o, "cwd", true)?;
            }
            if let Some(value) = o.get("env") {
                let env = value
                    .as_object()
                    .ok_or("env must be an object of strings.")?;
                if env.iter().any(|(key, value)| {
                    key.is_empty()
                        || key.contains(['=', '\0'])
                        || !value.as_str().is_some_and(|s| !s.contains('\0'))
                }) {
                    return Err(
                        "env must contain valid names and string values without NUL bytes.".into(),
                    );
                }
            }
        }
    }
    Ok(())
}
