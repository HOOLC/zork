use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock, Weak};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::events::{OutstandingItem, ToolOutcome};
use super::ports::{Clock, FilePage, FileSystem, ProcessRequest, ProcessSpawner, SpawnedProcess};

pub mod activity;
mod arguments;
mod namespace;
mod parameters;
pub use activity::{ActivityTarget, ToolActivity};
pub use namespace::ToolNamespace;

pub const PROVIDER_CALL_NAME: &str = "call";

/// Render a tool-owned schema as agent-facing TypeScript parameter documentation.
pub fn parameter_types(schema: &Value) -> String {
    parameters::describe(schema)
}

pub fn provider_call_definition() -> crate::session::model::ToolDefinition {
    crate::session::model::ToolDefinition {
        name: PROVIDER_CALL_NAME.into(),
        description: "Call one zork-agent logical tool. Fill action with what this particular invocation will do. It is a concise user-visible description in the user's language, not reasoning, shell commands, or claims of success. Name the concrete task and object; avoid generic labels like thinking, working, executing a command, or completing the task. Set optional wait to your estimate, in seconds, of when this tool progress or result is worth checking again. The batch resumes on completion or the shortest explicit wait; reaching wait does not stop the tool. Use 0 when you need to continue immediately. Put only tool-specific parameters in arguments. Use tool.help to read the TypeScript argument type and field comments.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "tool": {"type": "string", "minLength": 1},
                "action": {"type": "string", "minLength": 1, "maxLength": 80, "description": "Brief description of this specific tool action, e.g. 查看提交和未提交改动. Shown in the companion activity bubble. Do not paste commands or private input content."},
                "arguments": {"type": "object"},
                "wait": {"type":"number","minimum":0,"description":"Estimated seconds until you should check this tool again. The shortest explicit wait in the batch controls when the agent can continue; tools keep running. Omit to use the runtime default."}
            },
            "required": ["tool", "action", "arguments"],
            "additionalProperties": false
        }),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynamicCall {
    pub tool: String,
    pub arguments: Value,
    pub action: String,
    pub wait: Option<std::time::Duration>,
}

impl DynamicCall {
    /// Both call.wait and the wait control tool express a batch wake estimate.
    pub(crate) fn batch_wait(&self) -> Option<std::time::Duration> {
        let control_wait = (self.tool == super::events::WAIT_TOOL_NAME
            && arguments::builtin(BuiltinKind::Wait, &self.arguments).is_ok())
        .then(|| self.arguments["seconds"].as_f64())
        .flatten()
        .and_then(|seconds| std::time::Duration::try_from_secs_f64(seconds).ok());
        self.wait.into_iter().chain(control_wait).min()
    }

    pub fn from_value(value: Value) -> Result<Self, DynamicCallError> {
        #[derive(Deserialize)]
        struct WireCall {
            tool: String,
            arguments: Map<String, Value>,
            // Defaults allow one useful validation error for both missing fields.
            // Durable historical invocations are deserialized separately.
            #[serde(default)]
            action: String,
            #[serde(default)]
            wait: Option<f64>,
        }

        let call: WireCall = serde_json::from_value(value)
            .map_err(|error| DynamicCallError::Invalid(error.to_string()))?;
        if call.tool.trim().is_empty() {
            return Err(DynamicCallError::EmptyTool);
        }
        let action = activity::bounded(&call.action)
            .chars()
            .take(80)
            .collect::<String>();
        if action.is_empty() {
            return Err(DynamicCallError::DescriptionsRequired);
        }
        let wait = call
            .wait
            .map(|seconds| {
                std::time::Duration::try_from_secs_f64(seconds).map_err(|_| {
                    DynamicCallError::Invalid(
                        "wait must be a finite nonnegative number of seconds".into(),
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            wait,
            tool: call.tool,
            arguments: Value::Object(call.arguments),
            action,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DynamicCallError {
    #[error("invalid dynamic call: {0}")]
    Invalid(String),
    #[error("dynamic call tool must not be empty")]
    EmptyTool,
    #[error(
        "call requires non-empty top-level action (what this invocation does), alongside tool and arguments. No tool was executed. Retry with an action description in the user's language; keep the original tool arguments inside arguments."
    )]
    DescriptionsRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ToolVersion(String);

impl ToolVersion {
    pub fn new(value: impl Into<String>) -> Result<Self, ToolDefinitionError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ToolDefinitionError::EmptyVersion);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolContract {
    pub name: String,
    pub version: ToolVersion,
    pub initial_description: String,
    pub detailed_description: String,
    pub input_schema: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolContext {
    pub control: Option<super::ports::ToolControl>,
    pub session_id: String,
    pub invocation_id: String,
    pub workspace: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolExecution {
    pub images: Vec<super::wire::ToolImage>,
    pub outcome: ToolOutcome,
    pub data: Value,
    pub result_schema_version: u32,
    pub knowledge: Option<ToolKnowledge>,
}

impl ToolExecution {
    pub fn success(data: impl Into<Value>) -> Self {
        Self {
            outcome: ToolOutcome::Succeeded,
            data: data.into(),
            images: Vec::new(),
            result_schema_version: 1,
            knowledge: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolKnowledge {
    Current { name: String, version: ToolVersion },
    Removed { name: String },
}

pub trait ToolImplementation: Send + Sync {
    /// Complete external cancellation cleanup before the invocation is retired.
    /// Local tools generally release resources when their Future is dropped.
    fn cancel<'a>(
        &'a self,
        _context: &'a ToolContext,
        _arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<ToolExecution>> + Send + 'a>> {
        Box::pin(async { None })
    }

    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>>;
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolState {
    pub schema_version: u32,
    pub value: Value,
}

pub trait ToolCompatibility: Send + Sync {
    /// Related logical tools may fold into one registered tool's durable state.
    fn state_namespace(&self) -> Option<&'static str> {
        None
    }

    fn migrate_result(&self, schema_version: u32, value: Value) -> Result<Value, String>;

    fn migrate_state(&self, state: ToolState) -> Result<ToolState, String>;

    fn initial_state(&self) -> Option<ToolState>;

    fn fold(&self, state: Option<&ToolState>, result: &Value) -> Result<Option<ToolState>, String>;

    fn outstanding(&self, state: Option<&ToolState>) -> Vec<OutstandingItem>;
}

pub struct NoToolState;

impl ToolCompatibility for NoToolState {
    fn migrate_result(&self, schema_version: u32, value: Value) -> Result<Value, String> {
        if schema_version == 1 {
            Ok(value)
        } else {
            Err(format!(
                "unsupported result schema version {schema_version}"
            ))
        }
    }

    fn migrate_state(&self, state: ToolState) -> Result<ToolState, String> {
        Ok(state)
    }

    fn initial_state(&self) -> Option<ToolState> {
        None
    }

    fn fold(
        &self,
        _state: Option<&ToolState>,
        _result: &Value,
    ) -> Result<Option<ToolState>, String> {
        Ok(None)
    }

    fn outstanding(&self, _state: Option<&ToolState>) -> Vec<OutstandingItem> {
        Vec::new()
    }
}

pub struct ToolInstance {
    contract: ToolContract,
    implementation: Arc<dyn ToolImplementation>,
    compatibility: Arc<dyn ToolCompatibility>,
    activity: Arc<dyn Fn(&Value) -> ToolActivity + Send + Sync>,
    advertised: bool,
}

impl ToolInstance {
    pub fn new(
        contract: ToolContract,
        implementation: Arc<dyn ToolImplementation>,
        compatibility: Arc<dyn ToolCompatibility>,
    ) -> Result<Self, ToolDefinitionError> {
        validate_contract(&contract)?;
        Ok(Self {
            contract,
            implementation,
            compatibility,
            activity: Arc::new(|_| ToolActivity::default()),
            advertised: true,
        })
    }

    /// Register presentation alongside execution, without exposing it to the model.
    pub fn with_activity(
        mut self,
        activity: impl Fn(&Value) -> ToolActivity + Send + Sync + 'static,
    ) -> Self {
        self.activity = Arc::new(activity);
        self
    }

    /// Retain compatibility without teaching the legacy entry to new agents.
    pub fn hidden(mut self) -> Self {
        self.advertised = false;
        self
    }
    pub fn advertise(mut self, advertised: bool) -> Self {
        self.advertised = advertised;
        self
    }

    pub fn cancel<'a>(
        &'a self,
        context: &'a ToolContext,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = Option<ToolExecution>> + Send + 'a>> {
        self.implementation.cancel(context, arguments)
    }

    pub fn contract(&self) -> &ToolContract {
        &self.contract
    }

    pub fn compatibility(&self) -> Arc<dyn ToolCompatibility> {
        self.compatibility.clone()
    }

    pub fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        self.implementation.execute(context, arguments)
    }
}

impl fmt::Debug for ToolInstance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolInstance")
            .field("contract", &self.contract)
            .finish_non_exhaustive()
    }
}

struct RegistryEntry {
    current: Option<Arc<ToolInstance>>,
    compatibility: Arc<dyn ToolCompatibility>,
}

#[derive(Default)]
pub struct ToolRegistry {
    entries: RwLock<BTreeMap<String, RegistryEntry>>,
    namespaces: RwLock<BTreeMap<String, namespace::NamespaceEntry>>,
}

impl ToolRegistry {
    pub fn activity(&self, name: &str, arguments: &Value) -> ToolActivity {
        let instance = self.instance(name);
        instance.map_or_else(ToolActivity::default, |tool| (tool.activity)(arguments))
    }
    pub fn register(&self, instance: Arc<ToolInstance>) {
        let name = instance.contract().name.clone();
        let compatibility = instance.compatibility();
        self.entries
            .write()
            .expect("tool registry lock poisoned")
            .insert(
                name,
                RegistryEntry {
                    current: Some(instance),
                    compatibility,
                },
            );
    }

    pub fn remove(&self, name: &str) -> bool {
        let mut entries = self.entries.write().expect("tool registry lock poisoned");
        let Some(entry) = entries.get_mut(name) else {
            return false;
        };
        entry.current.take().is_some()
    }

    /// Preserve historical state/result decoding without registering a callable
    /// tool or adding anything to the model's catalog.
    pub fn register_retired(&self, name: &str, compatibility: Arc<dyn ToolCompatibility>) {
        self.entries
            .write()
            .expect("tool registry lock poisoned")
            .insert(
                name.into(),
                RegistryEntry {
                    current: None,
                    compatibility,
                },
            );
    }

    pub fn resolve(&self, name: &str, known: Option<&ToolVersion>) -> ToolResolution {
        let Some(instance) = self.instance(name) else {
            return ToolResolution::Unavailable;
        };
        if known == Some(&instance.contract().version) {
            ToolResolution::Ready(instance.clone())
        } else {
            ToolResolution::VersionChanged {
                current: instance.contract().version.clone(),
            }
        }
    }

    pub fn compatibility(&self, name: &str) -> Option<Arc<dyn ToolCompatibility>> {
        if let Some(entry) = self
            .entries
            .read()
            .expect("tool registry lock poisoned")
            .get(name)
        {
            return Some(entry.compatibility.clone());
        }
        self.namespace_instance(name, true)
            .map(|tool| tool.compatibility())
    }

    pub fn current_contract(&self, name: &str) -> Option<ToolContract> {
        self.instance(name).map(|tool| tool.contract().clone())
    }

    pub fn initial_catalog(&self) -> Vec<ToolIntroduction> {
        let mut catalog: Vec<_> = self
            .entries
            .read()
            .expect("tool registry lock poisoned")
            .values()
            .filter_map(|entry| entry.current.as_ref())
            .filter(|instance| instance.advertised)
            .map(|instance| ToolIntroduction {
                name: instance.contract().name.clone(),
                version: instance.contract().version.clone(),
                description: instance.contract().initial_description.clone(),
            })
            .collect();
        catalog.extend(self.namespace_introductions());
        catalog.sort_by(|a, b| a.name.cmp(&b.name));
        catalog
    }

    pub fn changes(&self, known: &BTreeMap<String, ToolVersion>) -> Vec<ToolChange> {
        let mut current: BTreeMap<_, _> = self
            .initial_catalog()
            .into_iter()
            .map(|i| (i.name, i.version))
            .collect();
        // Only resolve exact dynamic names the session already knows. Merely
        // describing a namespace does not enumerate or retain its method space.
        for name in known.keys() {
            if !current.contains_key(name) {
                if let Some(contract) = self.current_contract(name) {
                    current.insert(name.clone(), contract.version);
                }
            }
        }
        let mut changes = Vec::new();
        for (name, version) in &current {
            match known.get(name) {
                None => changes.push(ToolChange::Added {
                    name: name.clone(),
                    version: version.clone(),
                }),
                Some(old) if old != version => changes.push(ToolChange::Updated {
                    name: name.clone(),
                    version: version.clone(),
                }),
                Some(_) => {}
            }
        }
        for name in known.keys() {
            if !current.contains_key(name) {
                changes.push(ToolChange::Removed { name: name.clone() });
            }
        }
        changes.sort_by(|a, b| a.name().cmp(b.name()));
        changes
    }
}

pub fn tool_help_instance(
    registry: &Arc<ToolRegistry>,
    version: ToolVersion,
) -> Result<Arc<ToolInstance>, ToolDefinitionError> {
    Ok(Arc::new(ToolInstance::new(
        ToolContract {
            name: "tool.help".into(),
            version,
            initial_description: "Use tool.help to read current usage and TypeScript argument types for a specific logical tool.".into(),
            detailed_description: "Return the latest detailed usage, TypeScript parameter definition and current version for one exact logical tool name. This does not search or recommend tools.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"tool": {"type": "string", "minLength": 1}},
                "required": ["tool"],
                "additionalProperties": false
            }),
        },
        Arc::new(ToolHelp {
            registry: Arc::downgrade(registry),
        }),
        Arc::new(NoToolState),
    )?.with_activity(|args| ToolActivity::field("查看工具说明", "Reading tool help", args, "/tool"))))
}

struct ToolHelp {
    registry: Weak<ToolRegistry>,
}

impl ToolImplementation for ToolHelp {
    fn execute<'a>(
        &'a self,
        _context: &'a ToolContext,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            if let Err(error) = arguments::help(arguments) {
                return failed(format!("Invalid tool arguments: {error}"));
            }
            let Some(name) = arguments.get("tool").and_then(Value::as_str) else {
                return ToolExecution {
                    images: Vec::new(),
                    outcome: ToolOutcome::Failed,
                    data: error_data("tool.help requires an exact tool name."),
                    result_schema_version: 1,
                    knowledge: None,
                };
            };
            let Some(registry) = self.registry.upgrade() else {
                return ToolExecution {
                    images: Vec::new(),
                    outcome: ToolOutcome::Failed,
                    data: error_data("Tool registry is unavailable."),
                    result_schema_version: 1,
                    knowledge: None,
                };
            };
            let Some(contract) = registry.current_contract(name) else {
                return ToolExecution {
                    images: Vec::new(),
                    outcome: ToolOutcome::Failed,
                    data: serde_json::json!({
                        "error": format!("Tool {name} is unavailable."),
                        "tool": name,
                    }),
                    result_schema_version: 1,
                    knowledge: Some(ToolKnowledge::Removed {
                        name: name.to_owned(),
                    }),
                };
            };
            let ToolContract {
                name: tool_name,
                version,
                detailed_description,
                input_schema,
                ..
            } = contract;
            ToolExecution {
                images: Vec::new(),
                outcome: ToolOutcome::Succeeded,
                data: serde_json::json!({
                    "tool": tool_name,
                    "version": version.clone(),
                    "description": detailed_description,
                    "parameters": parameter_types(&input_schema),
                }),
                result_schema_version: 1,
                knowledge: Some(ToolKnowledge::Current {
                    name: name.to_owned(),
                    version,
                }),
            }
        })
    }
}

#[derive(Clone, Debug)]
pub enum ToolResolution {
    Ready(Arc<ToolInstance>),
    VersionChanged { current: ToolVersion },
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolIntroduction {
    pub name: String,
    pub version: ToolVersion,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolChange {
    Added { name: String, version: ToolVersion },
    Updated { name: String, version: ToolVersion },
    Removed { name: String },
}

impl ToolChange {
    pub fn name(&self) -> &str {
        match self {
            Self::Added { name, .. } | Self::Updated { name, .. } | Self::Removed { name } => name,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolDefinitionError {
    #[error("tool name must not be empty")]
    EmptyName,
    #[error("namespace prefix must end in a dot and advertise prefix.*")]
    InvalidNamespace,
    #[error("tool version must not be empty")]
    EmptyVersion,
    #[error("tool initial description must not be empty")]
    EmptyInitialDescription,
    #[error("tool detailed description must not be empty")]
    EmptyDetailedDescription,
    #[error("invalid tool input schema: {0}")]
    InvalidInputSchema(String),
}

fn validate_contract(contract: &ToolContract) -> Result<(), ToolDefinitionError> {
    if contract.name.trim().is_empty() {
        return Err(ToolDefinitionError::EmptyName);
    }
    if contract.initial_description.trim().is_empty() {
        return Err(ToolDefinitionError::EmptyInitialDescription);
    }
    if contract.detailed_description.trim().is_empty() {
        return Err(ToolDefinitionError::EmptyDetailedDescription);
    }
    if !contract.input_schema.is_object() {
        return Err(ToolDefinitionError::InvalidInputSchema(
            "schema must be a JSON object".into(),
        ));
    }
    jsonschema::validator_for(&contract.input_schema)
        .map_err(|error| ToolDefinitionError::InvalidInputSchema(error.to_string()))?;
    Ok(())
}

#[derive(Clone)]
pub struct BuiltinToolDependencies {
    pub environment: BTreeMap<String, String>,
    pub query: Arc<dyn super::query::SessionQuery>,
    pub clock: Arc<dyn Clock>,
    pub files: Arc<dyn FileSystem>,
    pub processes: Arc<dyn ProcessSpawner>,
}

pub fn register_builtin_tools(
    registry: &Arc<ToolRegistry>,
    dependencies: BuiltinToolDependencies,
) -> Result<(), ToolDefinitionError> {
    for (kind, contract) in builtin_contracts()? {
        registry.register(Arc::new(
            ToolInstance::new(
                contract,
                Arc::new(BuiltinTool {
                    kind,
                    dependencies: dependencies.clone(),
                }),
                Arc::new(NoToolState),
            )?
            .with_activity(move |args| kind.activity(args)),
        ));
    }
    registry.register(tool_help_instance(
        registry,
        ToolVersion::new("builtin-1")?,
    )?);
    Ok(())
}

#[derive(Clone, Copy)]
enum BuiltinKind {
    End,
    Wait,
    ToolCancel,
    HistoryList,
    FileRead,
    FileList,
    FileMaterialize,
    FileWrite,
    FileEdit,
    ShellRun,
}

impl BuiltinKind {
    fn activity(self, args: &Value) -> ToolActivity {
        match self {
            Self::End => ToolActivity::new("完成本轮工作", "Finishing", ""),
            Self::Wait => ToolActivity::field("等待", "Waiting", args, "/reason"),
            Self::ToolCancel => ToolActivity::new("取消操作", "Cancelling operation", ""),
            Self::HistoryList => ToolActivity::new("查看执行历史", "Reading activity history", ""),
            Self::FileRead => ToolActivity::field("读取", "Reading", args, "/path"),
            Self::FileList => ToolActivity::field("浏览目录", "Listing files", args, "/path"),
            Self::FileMaterialize => {
                ToolActivity::field("准备执行文件", "Preparing files", args, "/path")
            }
            Self::FileWrite => ToolActivity::field("写入", "Writing", args, "/path"),
            Self::FileEdit => ToolActivity::field("编辑", "Editing", args, "/path"),
            Self::ShellRun => ToolActivity::new("执行命令", "Running command", ""),
        }
    }
}

fn builtin_contracts() -> Result<Vec<(BuiltinKind, ToolContract)>, ToolDefinitionError> {
    let version = || ToolVersion::new("builtin-1");
    Ok(vec![
        (
            BuiltinKind::End,
            ToolContract {
                name: super::events::END_TOOL_NAME.into(),
                version: version()?,
                initial_description: "Finish the current turn while leaving unfinished work. Replies without tool calls finish automatically when nothing is outstanding.".into(),
                detailed_description: "If the runtime reports unfinished items, resolve them or call end with acknowledge_outstanding=true to explicitly finish while leaving the disclosed items outstanding. Without outstanding work, simply reply without tool calls to finish.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"acknowledge_outstanding": {"type": "boolean"}},
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::Wait,
            ToolContract {
                name: super::events::WAIT_TOOL_NAME.into(),
                version: version()?,
                initial_description: "Pause until a duration elapses or new mailbox input arrives.".into(),
                detailed_description: "Pause this session for seconds unless new mailbox input arrives first. Tool completions do not by themselves end this explicit wait.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "seconds": {"type": "number", "exclusiveMinimum": 0},
                        "reason": {"type": "string"}
                    },
                    "required": ["seconds"],
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::ToolCancel,
            ToolContract {
                name: super::events::TOOL_CANCEL_NAME.into(),
                version: ToolVersion::new("builtin-2")?,
                initial_description: "Cancel one tool invocation and await its terminal result.".into(),
                detailed_description: "Cancel invocation_id and wait until its terminal ToolResult is persisted. The result includes target_outcome and target_result; inspect them to distinguish cancellation, prior completion, failure, and uncertain external effects. Previous effects are not undone. A target that is no longer pending returns signalled=false without claiming it was cancelled.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"invocation_id": {"type": "string", "minLength": 1}},
                    "required": ["invocation_id"],
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::HistoryList,
            ToolContract {
                name: super::events::HISTORY_LIST_NAME.into(),
                version: version()?,
                initial_description: "List bounded durable session history pages. Use file.read on a returned zork://history URI for full event content.".into(),
                detailed_description: "List at most limit visible events strictly before before_event_id, newest page first in durable order. Omit before_event_id for the most recent page. Read a specific returned event with file.read(path=\"zork://history/<event-id>\").".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "before_event_id": {"type": "string", "minLength": 16, "maxLength": 16},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 200}
                    },
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::FileRead,
            ToolContract {
                name: super::events::FILE_READ_NAME.into(),
                version: ToolVersion::new("builtin-2")?,
                initial_description: "Read text or PNG/JPEG/GIF/WebP images, or zork://history/<event-id> with {path, offset?, limit?}. offset is a zero-based byte offset (default 0), limit is bytes (default 65536). Continue from the returned next_offset. start/end and line numbers are not read parameters.".into(),
                detailed_description: "Read at most limit bytes beginning at zero-based byte offset. Relative filesystem paths resolve from the session workspace. PNG/JPEG/GIF/WebP images are detected by their header and returned as complete image attachments (max 20 MiB); offset must be 0 and limit is ignored for images. The text offset/limit contract also applies to durable history events. Continue from next_offset until it is absent.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "minLength": 1},
                        "offset": {"type": "integer", "minimum": 0},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 1048576}
                    },
                    "required": ["path"],
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::FileMaterialize,
            ToolContract {
                name: "file.materialize".into(),
                version: version()?,
                initial_description: "Prepare a synch:// file or directory as an immutable local snapshot when a shell command needs local paths.".into(),
                detailed_description: "Fetch the selected shared file versions into a read-only cache outside the published tree. Returns a local path stable for this Station process lifetime. Prefer file.read for inspection. Does not install a skill or bind a source; copy into the workspace explicitly if editing is needed.".into(),
                input_schema: serde_json::json!({"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}),
            },
        ),
        (BuiltinKind::FileList, ToolContract {
            name:"file.list".into(),version:version()?,
            initial_description:"List one page of a local or synch:// directory. Use returned file paths with file.read and next_cursor for continuation.".into(),
            detailed_description:"Read direct children without fetching file bodies. Keep the returned directory path and next_cursor together when continuing; a synch:// path with origin captures one fixed tree snapshot. The cursor is scoped to that directory and snapshot.".into(),
            input_schema:serde_json::json!({"type":"object","properties":{"path":{"type":"string","minLength":1},"cursor":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":128}},"required":["path"],"additionalProperties":false}),
        }),
        (
            BuiltinKind::FileWrite,
            ToolContract {
                name: super::events::FILE_WRITE_NAME.into(),
                version: version()?,
                initial_description: "Write complete file content, creating parent directories.".into(),
                detailed_description: "Replace path with content, creating parent directories as needed. Relative paths resolve from the session workspace; absolute paths and parent traversal are allowed because the workspace is not a sandbox.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "minLength": 1},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"],
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::FileEdit,
            ToolContract {
                name: super::events::FILE_EDIT_NAME.into(),
                version: version()?,
                initial_description: "Apply exact text replacements with {path, edits:[{old_text, new_text}]}. Each old_text must occur exactly once; replacements cannot overlap.".into(),
                detailed_description: "Each edits[].old_text must occur exactly once in the original file. All matches are resolved against the original content, replacements must not overlap, and the file is written once after validation.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "minLength": 1},
                        "edits": {
                            "type": "array", "minItems": 1,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_text": {"type": "string"},
                                    "new_text": {"type": "string"}
                                },
                                "required": ["old_text", "new_text"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["path", "edits"],
                    "additionalProperties": false
                }),
            },
        ),
        (
            BuiltinKind::ShellRun,
            ToolContract {
                name: super::events::SHELL_RUN_NAME.into(),
                version: version()?,
                initial_description: "Run a shell command in the session workspace. Full output is retained in a live file.".into(),
                detailed_description: "Run command through /bin/sh in the session workspace with the agent process environment plus configured overrides. The workspace is not a sandbox. stdout and stderr are combined and streamed to .zork/live-<invocation-id>.log. The same file remains available after completion; the result contains its path and a bounded tail. Execution continues until completion or cancellation.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string", "minLength": 1},
                        "cwd": {"type": "string", "minLength": 1},
                        "env": {"type": "object", "additionalProperties": {"type": "string"}}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            },
        ),
    ])
}

struct BuiltinTool {
    kind: BuiltinKind,
    dependencies: BuiltinToolDependencies,
}

impl ToolImplementation for BuiltinTool {
    fn execute<'a>(
        &'a self,
        context: &'a ToolContext,
        arguments: &'a Value,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        let kind = self.kind;
        let dependencies = self.dependencies.clone();
        let context = context.clone();
        let arguments = arguments.clone();
        Box::pin(async move {
            if let Err(error) = arguments::builtin(kind, &arguments) {
                return failed(format!("Invalid tool arguments: {error}"));
            }
            match kind {
                BuiltinKind::End => success(serde_json::json!({
                    "acknowledge_outstanding": arguments
                        .get("acknowledge_outstanding")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })),
                BuiltinKind::Wait => wait_result(dependencies.clock.as_ref(), &arguments),
                BuiltinKind::ToolCancel => {
                    let target = arguments["invocation_id"].as_str().expect("parsed target");
                    if target == context.invocation_id {
                        return failed("tool.cancel cannot target itself.");
                    }
                    let Some(control) = &context.control else {
                        return failed("Session control is unavailable.");
                    };
                    match control.cancel(target.to_owned()).await {
                        Ok(completion) => success(serde_json::json!({
                            "target_invocation_id": target,
                            "signalled": completion.signalled,
                            "target_outcome": completion.result.as_ref().map(|result|result.outcome),
                            "target_result": completion.result.as_ref().map(|result|&result.data),
                            "note": if completion.result.is_some() { "The target's terminal ToolResult is persisted. Cancellation does not undo previous effects." } else { "The target is not pending in this session. No cancellation was requested or confirmed." }
                        })),
                        Err(error) => failed(error),
                    }
                }
                BuiltinKind::HistoryList => {
                    history_list(dependencies.query.as_ref(), &context.session_id, &arguments)
                }
                BuiltinKind::FileRead => file_read(&dependencies, &context, &arguments).await,
                BuiltinKind::FileList => {
                    let path = resolve_path(
                        &context.workspace,
                        arguments["path"].as_str().unwrap_or_default(),
                    );
                    let result = dependencies
                        .files
                        .clone()
                        .list_page_async(
                            path,
                            arguments["cursor"].as_str().map(str::to_owned),
                            arguments["limit"].as_u64().unwrap_or(128) as usize,
                        )
                        .await;
                    match result {
                        Ok(value) => success(value),
                        Err(error) => failed(error.to_string()),
                    }
                }
                BuiltinKind::FileMaterialize => {
                    let path =
                        std::path::PathBuf::from(arguments["path"].as_str().expect("parsed path"));
                    match dependencies.files.materialize(path).await {
                        Ok(path) => success(
                            serde_json::json!({"path":path,"lifetime":"station_process","read_only":true}),
                        ),
                        Err(error) => failed(error.to_string()),
                    }
                }
                BuiltinKind::FileWrite => {
                    blocking(move || file_write(&dependencies, &context, &arguments)).await
                }
                BuiltinKind::FileEdit => {
                    blocking(move || file_edit(&dependencies, &context, &arguments)).await
                }
                BuiltinKind::ShellRun => shell_run(&dependencies, &context, &arguments).await,
            }
        })
    }
}

fn success(data: Value) -> ToolExecution {
    ToolExecution {
        images: Vec::new(),
        outcome: super::events::ToolOutcome::Succeeded,
        data,
        result_schema_version: 1,
        knowledge: None,
    }
}

fn failed(message: impl Into<String>) -> ToolExecution {
    ToolExecution {
        images: Vec::new(),
        outcome: super::events::ToolOutcome::Failed,
        data: error_data(message),
        result_schema_version: 1,
        knowledge: None,
    }
}

fn error_data(message: impl Into<String>) -> Value {
    serde_json::json!({"error": message.into()})
}

async fn blocking(work: impl FnOnce() -> ToolExecution + Send + 'static) -> ToolExecution {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => failed(format!("Blocking tool task failed: {error}")),
    }
}

fn wait_result(clock: &dyn Clock, arguments: &Value) -> ToolExecution {
    let seconds = arguments
        .get("seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let now_ms = clock.now_ms();
    let duration_ms = (seconds * 1000.0).ceil().min(i64::MAX as f64) as i64;
    success(serde_json::json!({
        "until_ms": now_ms.saturating_add(duration_ms),
        "reason": arguments.get("reason").cloned().unwrap_or(Value::Null),
    }))
}

fn history_list(
    query: &dyn super::query::SessionQuery,
    session_id: &str,
    input: &Value,
) -> ToolExecution {
    let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let before = input.get("before_event_id").and_then(Value::as_str);
    match query.before(session_id, before, limit) {
        Ok(events) => {
            let items = events
                .iter()
                .map(|event| {
                    serde_json::json!({
                        "event_id": event.event_id,
                        "kind": event.event.kind(),
                        "uri": format!("zork://history/{}", event.event_id),
                    })
                })
                .collect::<Vec<_>>();
            let data = serde_json::json!({
                "events": items,
                "next_before_event_id": events.first().map(|event| event.event_id.clone()),
            });
            success(data)
        }
        Err(error) => failed(format!("History query failed: {error}")),
    }
}

pub(crate) const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

fn image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

async fn file_read(
    dependencies: &BuiltinToolDependencies,
    context: &ToolContext,
    input: &Value,
) -> ToolExecution {
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let offset = input.get("offset").and_then(Value::as_u64).unwrap_or(0);
    let limit = input
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(64 * 1024) as usize;

    let page = if let Some(event_id) = path.strip_prefix("zork://history/") {
        match dependencies.query.event(&context.session_id, event_id) {
            Ok(Some(event)) => match serde_json::to_vec_pretty(&event) {
                Ok(bytes) => page_bytes(bytes, offset, limit),
                Err(error) => {
                    return failed(format!("History event serialization failed: {error}"));
                }
            },
            Ok(None) => return failed(format!("History event {event_id} was not found.")),
            Err(error) => return failed(format!("History event read failed: {error}")),
        }
    } else {
        let path = resolve_path(&context.workspace, path);
        // Detect from the file header even when a caller requests a tiny page
        // or a nonzero offset. Binary images are indivisible tool content.
        let header = match read_file_page(dependencies.files.clone(), path.clone(), 0, 12).await {
            Ok(page) => page,
            Err(error) => return failed(format!("Failed to read {}: {error}", path.display())),
        };
        if let Some(media_type) = image_media_type(&header.bytes) {
            if offset != 0 {
                return failed("Images require offset=0; image reads return the complete file.");
            }
            if header.total_size > MAX_IMAGE_BYTES as u64 {
                return failed("Image exceeds the 20 MiB limit. Resize it before reading.");
            }
            let image =
                match read_file_page(dependencies.files.clone(), path.clone(), 0, MAX_IMAGE_BYTES)
                    .await
                {
                    Ok(page)
                        if page.next_offset.is_none()
                            && image_media_type(&page.bytes) == Some(media_type) =>
                    {
                        page
                    }
                    Ok(_) => {
                        return failed(
                            "Image changed or exceeds the 20 MiB limit; retry the read.",
                        );
                    }
                    Err(error) => return failed(format!("Failed to read image: {error}")),
                };
            use base64::Engine;
            let mut result = success(serde_json::json!({
                "path": path.to_string_lossy(), "media_type": media_type,
                "total_size": image.bytes.len(), "content": "Image attached to this tool result."
            }));
            result.images.push(super::wire::ToolImage {
                media_type: media_type.into(),
                base64: base64::engine::general_purpose::STANDARD
                    .encode(&image.bytes)
                    .into(),
            });
            return result;
        }
        match read_file_page(dependencies.files.clone(), path.clone(), offset, limit).await {
            Ok(page) => page,
            Err(error) => return failed(format!("Failed to read {}: {error}", path.display())),
        }
    };

    let content = String::from_utf8_lossy(&page.bytes).into_owned();
    ToolExecution {
        images: Vec::new(),
        outcome: super::events::ToolOutcome::Succeeded,
        data: serde_json::json!({
            "content": content,
            "offset": page.offset,
            "total_size": page.total_size,
            "next_offset": page.next_offset,
        }),
        result_schema_version: 1,
        knowledge: None,
    }
}

fn page_bytes(bytes: Vec<u8>, offset: u64, limit: usize) -> FilePage {
    let total_size = bytes.len() as u64;
    let start = offset.min(total_size) as usize;
    let end = start.saturating_add(limit).min(bytes.len());
    FilePage {
        bytes: bytes[start..end].to_vec(),
        offset,
        total_size,
        next_offset: (end < bytes.len()).then_some(end as u64),
    }
}

async fn read_file_page(
    files: Arc<dyn FileSystem>,
    path: std::path::PathBuf,
    offset: u64,
    limit: usize,
) -> std::io::Result<FilePage> {
    files.read_page_async(path, offset, limit).await
}

fn resolve_path(workspace: &str, path: &str) -> std::path::PathBuf {
    if path.starts_with("synch://") {
        return path.into();
    }
    let path = std::path::PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::path::Path::new(workspace).join(path)
    }
}

fn file_write(
    dependencies: &BuiltinToolDependencies,
    context: &ToolContext,
    input: &Value,
) -> ToolExecution {
    let path = resolve_path(
        &context.workspace,
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let content = input
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            dependencies.files.create_dir_all(parent)?;
        }
        dependencies.files.write(&path, content.as_bytes())
    })();
    match result {
        Ok(()) => success(serde_json::json!({"path": path, "bytes": content.len()})),
        Err(error) => failed(format!("Failed to write {}: {error}", path.display())),
    }
}

fn file_edit(
    dependencies: &BuiltinToolDependencies,
    context: &ToolContext,
    input: &Value,
) -> ToolExecution {
    let path = resolve_path(
        &context.workspace,
        input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let result = (|| -> Result<usize, String> {
        let original = dependencies
            .files
            .read_to_string(&path)
            .map_err(|error| error.to_string())?;
        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .ok_or_else(|| "edits must be an array".to_owned())?;
        let mut replacements = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            let old = edit
                .get("old_text")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("edit {} has no old_text", index + 1))?;
            let new = edit
                .get("new_text")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("edit {} has no new_text", index + 1))?;
            let matches = original.match_indices(old).collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err(format!(
                    "edit {} old_text matched {} locations",
                    index + 1,
                    matches.len()
                ));
            }
            let start = matches[0].0;
            replacements.push((start, start + old.len(), new.to_owned()));
        }
        replacements.sort_by_key(|replacement| replacement.0);
        if replacements.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err("edits overlap".into());
        }
        let mut updated = original;
        for (start, end, replacement) in replacements.iter().rev() {
            updated.replace_range(*start..*end, replacement);
        }
        dependencies
            .files
            .write(&path, updated.as_bytes())
            .map_err(|error| error.to_string())?;
        Ok(replacements.len())
    })();
    match result {
        Ok(count) => success(serde_json::json!({"path": path, "edits": count})),
        Err(error) => failed(format!("Failed to edit {}: {error}", path.display())),
    }
}

async fn shell_run(
    dependencies: &BuiltinToolDependencies,
    context: &ToolContext,
    input: &Value,
) -> ToolExecution {
    let command = input
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let workspace = std::path::Path::new(&context.workspace);
    let current_dir = input["cwd"]
        .as_str()
        .map(|cwd| workspace.join(cwd))
        .unwrap_or_else(|| workspace.to_owned());
    let mut environment = dependencies.environment.clone();
    if let Some(overrides) = input["env"].as_object() {
        for (key, value) in overrides {
            environment.insert(
                key.clone(),
                value.as_str().expect("validated environment").into(),
            );
        }
    }
    let relative = format!(".zork/live-{}.log", context.invocation_id);
    let live_path = workspace.join(&relative);
    if let Err(error) = dependencies.files.create_dir_all(&workspace.join(".zork")) {
        return failed(format!("Failed to create live output directory: {error}"));
    }
    let output = match dependencies.files.create(&live_path) {
        Ok(file) => file,
        Err(error) => return failed(format!("Failed to create live output file: {error}")),
    };
    let SpawnedProcess {
        output: mut process_output,
        handle,
    } = match dependencies.processes.spawn(ProcessRequest {
        command: command.to_owned(),
        current_dir,
        environment,
    }) {
        Ok(process) => process,
        Err(error) => return failed(format!("Failed to start command: {error}")),
    };
    use std::io::Write;
    let mut output = output;
    let mut outcome = super::events::ToolOutcome::Succeeded;
    // Drain descendants' output while retaining the process-group owner.
    let collected: std::io::Result<()> = async {
        while let Some(chunk) = process_output.recv().await {
            output.write_all(&chunk?)?;
        }
        output.flush()
    }
    .await;
    if let Err(error) = collected {
        return failed(format!("Failed to persist command output: {error}"));
    }
    let status = handle
        .wait()
        .await
        .map_err(|error| format!("Failed while waiting for command: {error}"));
    match status {
        Ok(status) if status.success => {}
        Ok(status) => {
            outcome = super::events::ToolOutcome::Failed;
            let _ = status;
        }
        Err(error) => return failed(error),
    }
    let tail = dependencies
        .files
        .tail(&live_path, 64 * 1024)
        .unwrap_or_default();
    let mut message = String::from_utf8_lossy(&tail).into_owned();
    if message.is_empty() {
        message.push_str("(no output)");
    }
    if outcome == super::events::ToolOutcome::Failed {
        message.push_str("\n\nCommand exited unsuccessfully.");
    }
    ToolExecution {
        images: Vec::new(),
        outcome,
        data: serde_json::json!({
            "output": message,
            "output_path": relative,
        }),
        result_schema_version: 1,
        knowledge: None,
    }
}
