mod agent_port;
mod codex_responses;
mod fake;
mod responses;

pub use agent_port::AgentModelPort;
pub use fake::FakeProvider;

use aimux_core::{
    content::ContentPart,
    error::AiMuxError,
    language_model::LanguageModel,
    language_model_message::{LanguageModelPrompt, LanguageModelPromptMessage},
    message::Role,
    options::CallOptions,
    result::{GenerateContent, GenerateResult, StreamResult},
    stream_part::StreamPart,
    tool::{FunctionTool, Tool},
    types::{FinishReason, FinishReasonUnified, ReasoningEffort, Usage},
};
use aimux_providers::{
    anthropic::{AnthropicConfig, AnthropicProvider},
    openai::{OpenAIConfig, OpenAIProvider},
};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use url::Url;

use zork_agent::session::{
    model::{ModelError, ModelOutcome, ModelRequest, ProviderFailure},
    ports::{ModelExecutor, ProfileExecution},
    wire::{
        ProviderContext, ProviderInputDiagnostics, ProviderInputMode, ProviderMessage,
        ProviderToolCall, TranscriptRole,
    },
};

pub struct ProviderRouter {
    codex_responses: codex_responses::CodexResponsesProvider,
}

impl Default for ProviderRouter {
    fn default() -> Self {
        Self::new()
    }
}

fn aimux_failure(stage: &'static str, error: AiMuxError) -> ModelError {
    let status_code = error.status_code();
    let (provider_code, request_id) = match &error {
        AiMuxError::ApiCall(detail) => (detail.provider_code.clone(), detail.request_id.clone()),
        _ => (None, None),
    };
    let message = error.to_string();
    let retryable =
        !provider_failure_is_certainly_permanent(status_code, provider_code.as_deref());
    ModelError::ProviderFailed(ProviderFailure {
        stage,
        retryable,
        status_code,
        provider_code,
        request_id,
        message,
        provider_input: None,
        usage: None,
    })
}

fn protocol_failure(stage: &'static str, message: impl Into<String>) -> ModelError {
    ModelError::ProviderFailed(ProviderFailure::new(stage, true, message))
}

pub(super) fn provider_failure_is_certainly_permanent(
    status_code: Option<u16>,
    provider_code: Option<&str>,
) -> bool {
    if matches!(status_code, Some(400 | 401 | 403)) {
        return true;
    }
    let Some(code) = provider_code else {
        return false;
    };
    matches!(
        code.to_ascii_lowercase().as_str(),
        "invalid_api_key"
            | "authentication_error"
            | "unauthorized"
            | "permission_denied"
            | "account_deactivated"
    )
}

impl ProviderRouter {
    pub fn new() -> Self {
        Self {
            codex_responses: codex_responses::CodexResponsesProvider::new(),
        }
    }

    async fn complete_request(
        &self,
        request: &ModelRequest,
        execution: ProfileExecution,
    ) -> Result<ModelOutcome, ModelError> {
        let selection = &request.selection;
        if execution.profile_id() != selection.profile_id
            || execution.model() != selection.model
            || execution.thinking() != selection.thinking
        {
            return Err(ModelError::InvalidSelection);
        }
        let base_url =
            Url::parse(execution.base_url()).map_err(|_| ModelError::InvalidSelection)?;
        if !matches!(base_url.scheme(), "http" | "https") {
            return Err(ModelError::InvalidSelection);
        }
        if execution.api() == "openai-codex-responses" {
            return self.codex_responses.complete(request, execution).await;
        }

        let responses_api = is_responses_api(execution.api());
        let prompt = if responses_api {
            Vec::new()
        } else {
            prompt_from_transcript(request.transcript.as_slice(), &execution)?
        };
        let tools = request
            .tools
            .iter()
            .map(|tool| {
                Tool::Function(
                    FunctionTool::new(tool.name.clone(), tool.input_schema.clone())
                        .with_description(tool.description.clone()),
                )
            })
            .collect::<Vec<_>>();
        let headers = request_headers(
            execution.provider(),
            if request.independent { &request.step_id } else { &request.session_id },
            execution.headers(),
        );
        let options = CallOptions {
            tools: (!tools.is_empty()).then_some(tools),
            max_output_tokens: request.max_output_tokens,
            headers: (!headers.is_empty()).then_some(headers),
            reasoning: Some(reasoning_effort(execution.thinking())),
            provider_options: responses_api
                .then(|| responses_provider_options(execution.thinking())),
            ..CallOptions::new(prompt)
        };
        if responses_api {
            let input = responses_input_from_transcript(request.transcript.as_slice(), &execution)?;
            let provider_input = full_input_diagnostics(request.transcript.len(), input.len());
            if execution.streaming() {
                let result = responses::do_stream(&options, &execution, input)
                    .await
                    .map_err(|error| {
                        with_provider_diagnostics(
                            aimux_failure("openai.responses.stream_start", error),
                            Some(Box::new(provider_input.clone())),
                            None,
                        )
                    })?;
                return model_outcome_from_stream_result(
                    result.result,
                    request,
                    &execution,
                    Some(result.output_items),
                    provider_input,
                )
                .await;
            }
            let result = responses::do_generate(&options, &execution, input)
                .await
                .map_err(|error| {
                    with_provider_diagnostics(
                        aimux_failure("openai.responses.generate", error),
                        Some(Box::new(provider_input.clone())),
                        None,
                    )
                })?;
            return model_outcome_from_generate_result(
                result.result,
                &execution,
                Some(result.output_items),
                provider_input,
            );
        }
        let provider_input = full_input_diagnostics(request.transcript.len(), options.prompt.len());
        if execution.streaming() {
            let result = stream_request(&options, &execution)
                .await
                .map_err(|error| {
                    with_provider_diagnostics(error, Some(Box::new(provider_input.clone())), None)
                })?;
            model_outcome_from_stream_result(result, request, &execution, None, provider_input)
                .await
        } else {
            let result = generate_request(&options, &execution)
                .await
                .map_err(|error| {
                    with_provider_diagnostics(error, Some(Box::new(provider_input.clone())), None)
                })?;
            model_outcome_from_generate_result(result, &execution, None, provider_input)
        }
    }
}

// Ordinary turns share Session routing; isolated context steps use their own key.
fn request_headers(
    provider: &str,
    routing_id: &str,
    configured: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut headers = configured.clone();
    if provider == "opencode-go" {
        // A profile-wide value would mix unrelated conversations. Generate this
        // per request from its durable Session/step identity, without changing auth.
        headers.retain(|key, _| !key.eq_ignore_ascii_case("x-opencode-session"));
        headers.insert("x-opencode-session".into(), routing_id.into());
        if !headers.keys().any(|key| key.eq_ignore_ascii_case("user-agent")) {
            headers.insert("User-Agent".into(), concat!("zork-agent/", env!("CARGO_PKG_VERSION")).into());
        }
    }
    if provider == "xai"
        && !headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("x-grok-conv-id"))
    {
        headers.insert("x-grok-conv-id".into(), routing_id.into());
    }
    headers
}

fn full_input_diagnostics(logical_items: usize, sent_items: usize) -> ProviderInputDiagnostics {
    ProviderInputDiagnostics {
        mode: ProviderInputMode::Full,
        logical_input_items: u64::try_from(logical_items).unwrap_or(u64::MAX),
        sent_input_items: u64::try_from(sent_items).unwrap_or(u64::MAX),
        previous_response_id: None,
        response_id: None,
    }
}

fn with_provider_diagnostics(
    mut error: ModelError,
    provider_input: Option<Box<ProviderInputDiagnostics>>,
    usage: Option<zork_agent::session::model::ModelTokenUsage>,
) -> ModelError {
    if let ModelError::ProviderFailed(failure) = &mut error {
        failure.provider_input = provider_input;
        failure.usage = usage.map(Box::new);
    }
    error
}

async fn stream_request(
    options: &CallOptions,
    execution: &ProfileExecution,
) -> Result<StreamResult, ModelError> {
    match execution.api() {
        "openai-completions" => {
            let mut config = OpenAIConfig::new(execution.secret().to_owned())
                .with_base_url(execution.base_url().to_owned())
                .with_provider(execution.provider().to_owned())
                .with_headers(options.headers.as_ref().unwrap_or(execution.headers()).clone());
            config.retry_config.max_retries = 0;
            OpenAIProvider::new(config)
                .model(execution.model())
                .do_stream(options)
                .await
                .map_err(|error| aimux_failure("openai.completions.stream_start", error))
        }
        "anthropic-messages" => {
            let mut config = AnthropicConfig::new(execution.secret().to_owned())
                .with_base_url(execution.base_url().to_owned())
                .with_headers(options.headers.as_ref().unwrap_or(execution.headers()).clone());
            config.retry_config.max_retries = 0;
            AnthropicProvider::new(config)
                .model(execution.model())
                .do_stream(options)
                .await
                .map_err(|error| aimux_failure("anthropic.messages.stream_start", error))
        }
        _ => Err(ModelError::InvalidSelection),
    }
}

async fn generate_request(
    options: &CallOptions,
    execution: &ProfileExecution,
) -> Result<GenerateResult, ModelError> {
    match execution.api() {
        "openai-completions" => {
            let mut config = OpenAIConfig::new(execution.secret().to_owned())
                .with_base_url(execution.base_url().to_owned())
                .with_provider(execution.provider().to_owned())
                .with_headers(options.headers.as_ref().unwrap_or(execution.headers()).clone());
            config.retry_config.max_retries = 0;
            OpenAIProvider::new(config)
                .model(execution.model())
                .do_generate(options)
                .await
                .map_err(|error| aimux_failure("openai.completions.generate", error))
        }
        "anthropic-messages" => {
            let mut config = AnthropicConfig::new(execution.secret().to_owned())
                .with_base_url(execution.base_url().to_owned())
                .with_headers(options.headers.as_ref().unwrap_or(execution.headers()).clone());
            config.retry_config.max_retries = 0;
            AnthropicProvider::new(config)
                .model(execution.model())
                .do_generate(options)
                .await
                .map_err(|error| aimux_failure("anthropic.messages.generate", error))
        }
        _ => Err(ModelError::InvalidSelection),
    }
}

fn parse_tool_arguments(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

struct PendingToolCall {
    tool_call_id: String,
    tool_name: String,
    input: String,
    parsed: Option<Value>,
}

#[derive(Default)]
struct ModelOutcomeAccumulator {
    text: String,
    tool_calls: Vec<PendingToolCall>,
    provider_context: Option<ProviderContext>,
    usage: Option<zork_agent::session::model::ModelTokenUsage>,
    provider_input: Option<Box<ProviderInputDiagnostics>>,
}

impl ModelOutcomeAccumulator {
    fn tool_call_mut(&mut self, tool_call_id: &str) -> Option<&mut PendingToolCall> {
        self.tool_calls
            .iter_mut()
            .find(|call| call.tool_call_id == tool_call_id)
    }

    fn attach_failure(&self, error: ModelError) -> ModelError {
        with_provider_diagnostics(error, self.provider_input.clone(), self.usage.clone())
    }

    fn observe_response_id(&mut self, response_id: Option<String>) {
        if let (Some(input), Some(response_id)) = (&mut self.provider_input, response_id) {
            input.response_id = Some(response_id);
        }
    }

    fn complete(self, finish_reason: FinishReason) -> Result<ModelOutcome, ModelError> {
        let Self {
            text,
            tool_calls,
            provider_context,
            usage,
            provider_input,
        } = self;
        let failure = |stage, message| {
            with_provider_diagnostics(
                protocol_failure(stage, message),
                provider_input.clone(),
                usage.clone(),
            )
        };
        match finish_reason.unified {
            FinishReasonUnified::Stop if tool_calls.is_empty() => Ok(ModelOutcome {
                text,
                tool_calls: Vec::new(),
                provider_context,
                usage,
                provider_input,
            }),
            FinishReasonUnified::ToolCalls if !tool_calls.is_empty() => {
                let mut calls = Vec::with_capacity(tool_calls.len());
                for call in tool_calls {
                    // A completed provider response is a durable fact even when
                    // its call arguments are malformed. The logical executor
                    // rejects these values and returns a paired error result.
                    let input = match call.parsed {
                        Some(Value::String(raw)) => parse_tool_arguments(&raw),
                        Some(input) => input,
                        None => parse_tool_arguments(&call.input),
                    };
                    calls.push(ProviderToolCall {
                        tool_call_id: call.tool_call_id,
                        tool_name: call.tool_name,
                        arguments: input,
                    });
                }
                Ok(ModelOutcome {
                    text,
                    tool_calls: calls,
                    provider_context,
                    usage,
                    provider_input,
                })
            }
            FinishReasonUnified::Length | FinishReasonUnified::ContentFilter => {
                let mut error = failure(
                    "provider.outcome.finish_reason",
                    format!("unsupported finish reason: {finish_reason:?}"),
                );
                if let ModelError::ProviderFailed(detail) = &mut error {
                    detail.provider_code = Some(
                        if finish_reason.unified == FinishReasonUnified::Length {
                            "max_output_tokens"
                        } else {
                            "content_filter"
                        }
                        .into(),
                    );
                }
                Err(error)
            }
            FinishReasonUnified::Stop
            | FinishReasonUnified::ToolCalls
            | FinishReasonUnified::Error
            | FinishReasonUnified::Other => Err(failure(
                "provider.outcome.finish_reason",
                format!("unsupported finish reason: {:?}", finish_reason),
            )),
        }
    }
}

async fn model_outcome_from_stream_result(
    mut result: StreamResult,
    request: &ModelRequest,
    execution: &ProfileExecution,
    output_items: Option<responses::CapturedOutputItems>,
    provider_input: ProviderInputDiagnostics,
) -> Result<ModelOutcome, ModelError> {
    let mut outcome = ModelOutcomeAccumulator {
        provider_input: Some(Box::new(provider_input)),
        ..Default::default()
    };
    let mut finish_reason = None;
    let mut output_bytes = 0_u64;
    let mut output_published: Option<std::time::Instant> = None;
    while let Some(part) = result.stream.next().await {
        let part = match part {
            Ok(part) => part,
            Err(error) => {
                return Err(outcome.attach_failure(aimux_failure("provider.stream.read", error)))
            }
        };
        if let StreamPart::TextDelta { delta, .. }
        | StreamPart::ReasoningDelta { delta, .. }
        | StreamPart::ToolInputDelta { delta, .. } = &part
        {
            output_bytes = output_bytes.saturating_add(delta.len() as u64);
            if output_bytes > 0
                && output_published
                    .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(250))
            {
                request.stream_observer.output_delta(
                    &request.session_id,
                    request.generation,
                    &request.step_id,
                    output_bytes,
                );
                output_bytes = 0;
                output_published = Some(std::time::Instant::now());
            }
        }
        match part {
            StreamPart::TextDelta { delta, .. } => {
                request.stream_observer.text_delta(
                    &request.session_id,
                    request.generation,
                    &request.step_id,
                    &delta,
                );
                outcome.text.push_str(&delta);
            }
            StreamPart::ToolInputStart { id, tool_name, .. }
                if outcome.tool_call_mut(&id).is_none() =>
            {
                outcome.tool_calls.push(PendingToolCall {
                    tool_call_id: id,
                    tool_name,
                    input: String::new(),
                    parsed: None,
                });
            }
            StreamPart::ToolInputDelta { id, delta, .. } => {
                let Some(call) = outcome.tool_call_mut(&id) else {
                    return Err(outcome.attach_failure(protocol_failure(
                        "provider.stream.tool_input_delta",
                        "tool input delta has no matching call",
                    )));
                };
                call.input.push_str(&delta);
            }
            StreamPart::ToolInputEnd { id, .. } if outcome.tool_call_mut(&id).is_none() => {
                return Err(outcome.attach_failure(protocol_failure(
                    "provider.stream.tool_input_end",
                    "tool input end has no matching call",
                )));
            }
            StreamPart::ToolInputEnd { .. } => {}
            StreamPart::ToolCall {
                tool_call_id,
                tool_name,
                input,
                ..
            } => {
                if let Some(call) = outcome.tool_call_mut(&tool_call_id) {
                    call.tool_name = tool_name;
                    match input {
                        Value::String(raw) => {
                            call.input = raw;
                            call.parsed = None;
                        }
                        input => call.parsed = Some(input),
                    }
                } else {
                    let (input, parsed) = match input {
                        Value::String(raw) => (raw, None),
                        input => (String::new(), Some(input)),
                    };
                    outcome.tool_calls.push(PendingToolCall {
                        tool_call_id,
                        tool_name,
                        input,
                        parsed,
                    });
                }
            }
            StreamPart::ResponseMetadata { id, .. } => outcome.observe_response_id(id),
            StreamPart::ReasoningStart { .. } | StreamPart::ReasoningEnd { .. } => {}
            StreamPart::Finish {
                finish_reason: observed_finish,
                usage,
                ..
            } => {
                outcome.usage = model_token_usage(&usage);
                finish_reason = Some(observed_finish);
            }
            StreamPart::Error { error } => {
                return Err(
                    outcome.attach_failure(aimux_failure("provider.stream.error_event", error))
                )
            }
            _ => {}
        }
    }
    if output_bytes > 0 {
        request.stream_observer.output_delta(
            &request.session_id,
            request.generation,
            &request.step_id,
            output_bytes,
        );
    }
    if let Some(output_items) = output_items {
        if !output_items.terminal_received() {
            return Err(outcome.attach_failure(protocol_failure(
                "provider.stream.finish",
                "Responses stream ended without a terminal response event",
            )));
        }
        let ordered = output_items.ordered().map_err(|error| {
            outcome.attach_failure(protocol_failure("provider.output_items", error))
        })?;
        outcome.provider_context = Some(provider_context_from_output_items(ordered, execution));
    }
    let Some(finish_reason) = finish_reason else {
        return Err(outcome.attach_failure(protocol_failure(
            "provider.stream.finish",
            "provider stream ended without a finish event",
        )));
    };
    if execution.api() == "openai-completions" && finish_reason.raw.is_none() {
        return Err(outcome.attach_failure(protocol_failure(
            "provider.stream.finish",
            "provider stream ended without an explicit finish reason",
        )));
    }
    outcome.complete(finish_reason)
}

fn model_outcome_from_generate_result(
    result: GenerateResult,
    execution: &ProfileExecution,
    output_items: Option<std::sync::Arc<Vec<Value>>>,
    provider_input: ProviderInputDiagnostics,
) -> Result<ModelOutcome, ModelError> {
    let mut outcome = ModelOutcomeAccumulator {
        usage: model_token_usage(&result.usage),
        provider_input: Some(Box::new(provider_input)),
        ..Default::default()
    };
    outcome.observe_response_id(result.response.id.clone());
    for content in result.content {
        match content {
            GenerateContent::Text { text, .. } => outcome.text.push_str(&text),
            GenerateContent::ToolCall {
                tool_call_id,
                tool_name,
                input,
                ..
            } => outcome.tool_calls.push(PendingToolCall {
                tool_call_id,
                tool_name,
                input: String::new(),
                parsed: Some(input),
            }),
            GenerateContent::Reasoning { .. } => {}
            GenerateContent::Source { .. }
            | GenerateContent::File { .. }
            | GenerateContent::ToolResult { .. } => {}
        }
    }
    if let Some(output_items) = output_items {
        outcome.provider_context =
            Some(provider_context_from_output_items(output_items, execution));
    }
    outcome.complete(result.finish_reason)
}

fn provider_context_from_output_items(
    output_items: std::sync::Arc<Vec<Value>>,
    execution: &ProfileExecution,
) -> ProviderContext {
    ProviderContext {
        profile_id: execution.profile_id().to_owned(),
        provider: execution.provider().to_owned(),
        model: execution.model().to_owned(),
        api: execution.api().to_owned(),
        output_items,
    }
}

fn model_token_usage(usage: &Usage) -> Option<zork_agent::session::model::ModelTokenUsage> {
    usage
        .input_tokens
        .total
        .map(|input_tokens| zork_agent::session::model::ModelTokenUsage {
            input_tokens: u64::from(input_tokens),
            cached_input_tokens: usage.input_tokens.cache_read.map(u64::from),
            output_tokens: u64::from(usage.output_tokens.total.unwrap_or(0)),
            output_reasoning_tokens: usage.output_tokens.reasoning.map(u64::from),
            output_text_tokens: usage.output_tokens.text.map(u64::from),
        })
}

fn reasoning_effort(value: &str) -> ReasoningEffort {
    match value {
        "none" | "off" => ReasoningEffort::None,
        "minimal" => ReasoningEffort::Minimal,
        "low" => ReasoningEffort::Low,
        "medium" => ReasoningEffort::Medium,
        "high" => ReasoningEffort::High,
        "xhigh" => ReasoningEffort::Xhigh,
        _ => ReasoningEffort::ProviderDefault,
    }
}

fn responses_provider_options(thinking: &str) -> HashMap<String, Value> {
    HashMap::from([(
        "openai".to_owned(),
        serde_json::json!({
            "forceReasoning": thinking != "off",
            "reasoningEffort": if thinking == "off" { "none" } else { thinking },
            "store": false,
        }),
    )])
}

fn is_responses_api(api: &str) -> bool {
    api == "openai-responses"
}

impl ModelExecutor for ProviderRouter {
    fn complete<'a>(
        &'a self,
        request: &'a ModelRequest,
        execution: ProfileExecution,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ModelOutcome, ModelError>> + Send + 'a>,
    > {
        Box::pin(self.complete_request(request, execution))
    }

    fn release_session(&self, session_id: &str) {
        self.codex_responses.release_session(session_id);
    }
}

fn tool_text(message: &ProviderMessage, image_input: bool) -> String {
    if !message.images.is_empty() && !image_input {
        format!("{}\n[Images were not sent: image input is disabled for the selected model. Tell the user to select an image-capable model and enable image input. Do not claim to have viewed the images.]", message.content)
    } else {
        message.content.to_string()
    }
}

fn responses_tool_output(message: &ProviderMessage, image_input: bool) -> Value {
    let text = tool_text(message, image_input);
    if message.images.is_empty() || !image_input {
        return Value::String(text);
    }
    let mut parts = vec![serde_json::json!({"type": "input_text", "text": text})];
    parts.extend(message.images.iter().map(|image| {
        serde_json::json!({
            "type": "input_image", "image_url": image.data_url(), "detail": "auto"
        })
    }));
    Value::Array(parts)
}

fn responses_input_from_transcript(
    transcript: &[ProviderMessage],
    execution: &ProfileExecution,
) -> Result<Vec<Value>, ModelError> {
    let mut input = Vec::new();
    for message in transcript {
        match message.role {
            TranscriptRole::System => input.push(serde_json::json!({
                "role": "developer",
                "content": message.content,
            })),
            TranscriptRole::User => input.push(serde_json::json!({
                "role": "user",
                "content": [{ "type": "input_text", "text": message.content }],
            })),
            TranscriptRole::Assistant => {
                let matching_context = message.provider_context.as_ref().filter(|context| {
                    context.profile_id == execution.profile_id()
                        && context.provider == execution.provider()
                        && context.model == execution.model()
                        && context.api == execution.api()
                });
                if let Some(context) = matching_context {
                    input.extend(context.output_items.iter().cloned());
                    continue;
                }
                // DeepSeek requires nonempty reasoning for each assistant tool
                // turn, including through OpenCode Go. Limit the gateway policy
                // to its DeepSeek model family, not every model on that provider.
                // Only synthetic calls get a marker; real output stays opaque.
                if message.runtime_generated
                    && !message.tool_calls.is_empty()
                    && (execution.provider() == "deepseek"
                        || (execution.provider() == "opencode-go"
                            && execution.model().starts_with("deepseek-")))
                    && !matches!(execution.thinking(), "off" | "none")
                {
                    input.push(serde_json::json!({
                        "type": "reasoning",
                        "content": [{
                            "type": "reasoning_text",
                            "text": "Runtime-generated notification; no model reasoning was produced for this synthetic call.",
                        }],
                    }));
                }
                if !message.content.is_empty() || message.tool_calls.is_empty() {
                    input.push(serde_json::json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": message.content }],
                    }));
                }
                for call in &message.tool_calls {
                    input.push(serde_json::json!({
                        "type": "function_call",
                        "call_id": call.tool_call_id,
                        "name": call.tool_name,
                        "arguments": serde_json::to_string(&call.arguments).map_err(|error| {
                            protocol_failure(
                                "provider.prompt.tool_arguments_json",
                                error.to_string(),
                            )
                        })?,
                    }));
                }
            }
            TranscriptRole::Tool => {
                let tool_call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                    protocol_failure(
                        "provider.prompt.tool_result_call_id",
                        "tool result has no tool call id",
                    )
                })?;
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": responses_tool_output(message, execution.image_input()),
                }));
            }
        }
    }
    Ok(input)
}

fn prompt_from_transcript(
    transcript: &[ProviderMessage],
    execution: &ProfileExecution,
) -> Result<LanguageModelPrompt, ModelError> {
    let mut prompt = Vec::new();
    let mut pending_images = Vec::new();
    // Opt-in endpoint policy; never infer chat-template behavior from model names.
    // Merge only the immutable leading block, preserving subsequent wire prefixes.
    let leading_systems =
        if execution.api() == "openai-completions" && execution.single_system_message() {
            transcript
                .iter()
                .take_while(|message| message.role == TranscriptRole::System)
                .count()
        } else {
            0
        };
    for (index, message) in transcript.iter().enumerate() {
        if index < leading_systems {
            if index == 0 {
                prompt.push(LanguageModelPromptMessage {
                    role: Role::System,
                    content: vec![ContentPart::text(
                        transcript[..leading_systems]
                            .iter()
                            .map(|message| message.content.as_ref())
                            .collect::<Vec<_>>()
                            .join("\n\n"),
                    )],
                    provider_options: None,
                });
            }
            continue;
        }
        let role = match message.role {
            TranscriptRole::System => Role::System,
            TranscriptRole::User => Role::User,
            TranscriptRole::Assistant => Role::Assistant,
            TranscriptRole::Tool => Role::Tool,
        };
        let mut content = Vec::new();
        // The durable transcript keeps any assistant preamble alongside
        // its tool calls, but OpenAI-compatible tool-call turns use a
        // null/omitted content field on the wire. Projecting that text
        // back into the next request changes the provider conversation
        // (and breaks replay) even though the text remains observable in
        // the durable Agent transcript.
        let assistant_tool_turn = execution.api() == "openai-completions"
            && message.role == TranscriptRole::Assistant
            && !message.tool_calls.is_empty();
        if message.role != TranscriptRole::Tool
            && !message.content.is_empty()
            && !assistant_tool_turn
        {
            content.push(ContentPart::text(message.content.to_string()));
        }
        if message.role == TranscriptRole::Assistant {
            for call in &message.tool_calls {
                content.push(ContentPart::tool_call(
                    call.tool_call_id.clone(),
                    call.tool_name.clone(),
                    call.arguments.clone(),
                ));
            }
        }
        if message.role == TranscriptRole::Tool {
            let tool_call_id = message.tool_call_id.clone().ok_or_else(|| {
                protocol_failure(
                    "provider.prompt.tool_result_call_id",
                    "tool result has no tool call id",
                )
            })?;
            let text = tool_text(message, execution.image_input());
            let mut result =
                serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.clone()));
            if execution.image_input() && !message.images.is_empty() {
                if execution.api() == "anthropic-messages" {
                    let mut blocks = vec![serde_json::json!({"type": "text", "text": text})];
                    blocks.extend(message.images.iter().map(|image| {
                        serde_json::json!({
                            "type": "image", "source": {"type": "base64",
                                "media_type": image.media_type, "data": image.base64}
                        })
                    }));
                    result = serde_json::json!({"type": "content", "value": blocks});
                } else {
                    // Chat Completions only supports images in user content.
                    // Flush after the entire tool-result block, preserving
                    // adjacency of every assistant call and its results.
                    use base64::Engine;
                    pending_images.push(ContentPart::text(format!(
                        "Image attachment from tool result {tool_call_id}:"
                    )));
                    for image in &message.images {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(image.base64.as_ref())
                            .map_err(|_| {
                                protocol_failure("provider.prompt.image", "invalid image encoding")
                            })?;
                        pending_images.push(ContentPart::image(bytes, &image.media_type));
                    }
                }
            }
            content.push(ContentPart::ToolResult {
                tool_call_id,
                result,
                tool_name: None,
                is_error: Some(message.is_error),
                preliminary: None,
                dynamic: None,
                provider_options: None,
            });
        }
        if content.is_empty() {
            content.push(ContentPart::text(String::new()));
        }
        prompt.push(LanguageModelPromptMessage {
            role,
            content,
            provider_options: None,
        });
        if !pending_images.is_empty()
            && transcript
                .get(index + 1)
                .is_none_or(|next| next.role != TranscriptRole::Tool)
        {
            prompt.push(LanguageModelPromptMessage {
                role: Role::User,
                content: std::mem::take(&mut pending_images),
                provider_options: None,
            });
        }
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01, RETRY-02]
    fn aimux_failure_preserves_structured_diagnostics_without_raw_body() {
        let error = AiMuxError::ApiCall(aimux_core::ApiCallError {
            status_code: Some(429),
            provider_code: Some("rate_limit_exceeded".to_owned()),
            message: "slow down".to_owned(),
            response_body: Some("raw body must not be copied".to_owned()),
            request_id: Some("req_123".to_owned()),
            is_retryable: true,
            ..Default::default()
        });

        let ModelError::ProviderFailed(failure) =
            aimux_failure("openai.responses.stream_start", error)
        else {
            panic!("expected provider failure");
        };
        assert_eq!(failure.stage, "openai.responses.stream_start");
        assert!(failure.retryable);
        assert_eq!(failure.status_code, Some(429));
        assert_eq!(
            failure.provider_code.as_deref(),
            Some("rate_limit_exceeded")
        );
        assert_eq!(failure.request_id.as_deref(), Some("req_123"));
        assert!(failure.message.contains("slow down"));
        assert!(!failure.message.contains("raw body must not be copied"));
    }

    #[test]
    fn missing_tool_output_is_not_retried() {
        let error = AiMuxError::ApiCall(aimux_core::ApiCallError {
            status_code: Some(400),
            provider_code: Some("invalid_request_error".into()),
            message: "No tool output found for tool call call_end.".into(),
            is_retryable: true,
            ..Default::default()
        });
        let ModelError::ProviderFailed(failure) =
            aimux_failure("openai.responses.stream_start", error)
        else {
            panic!("expected provider failure");
        };
        assert!(!failure.retryable);
    }

    #[test]
    fn every_http_400_is_permanent_without_reclassifying_other_statuses() {
        let missing = "The `reasoning_text` in the thinking mode must be passed back to the API.";
        for (status, code, message, retryable, overflow) in [
            (400, "invalid_request_error", missing, false, false),
            (
                400,
                "invalid_request_error",
                "invalid request body",
                false,
                false,
            ),
            (400, "context_length_exceeded", "context limit", false, true),
            (400, "rate_limit_exceeded", "slow down", false, false),
            (400, "", "Request is missing x-opencode-session", false, false),
            (429, "rate_limit_exceeded", "slow down", true, false),
            (502, "upstream_error", missing, true, false),
        ] {
            let error = AiMuxError::ApiCall(aimux_core::ApiCallError {
                status_code: Some(status),
                provider_code: Some(code.into()),
                message: message.into(),
                is_retryable: true,
                ..Default::default()
            });
            let ModelError::ProviderFailed(failure) =
                aimux_failure("openai.responses.stream_start", error)
            else {
                panic!("expected provider failure");
            };
            assert_eq!(failure.retryable, retryable, "{status}: {message}");
            assert_eq!(
                failure.is_context_overflow(),
                overflow,
                "{status}: {message}"
            );
        }
    }

    #[test]
    fn runtime_reasoning_requires_provenance_and_a_deepseek_route() {
        let original = ProviderMessage {
            images: Vec::new(),
            role: TranscriptRole::Assistant,
            content: "".into(),
            is_error: false,
            runtime_generated: false,
            tool_call_id: None,
            tool_calls: vec![ProviderToolCall {
                tool_call_id: "call_notice_looks_synthetic".into(),
                tool_name: "call".into(),
                arguments: serde_json::json!({"tool": "runtime.notice", "arguments": {}}),
            }],
            provider_context: None,
        };
        for (provider, model, thinking, expected) in [
            ("deepseek", "model", "high", true),
            ("deepseek", "model", "off", false),
            ("deepseek", "model", "none", false),
            ("opencode-go", "deepseek-flash", "high", true),
            ("opencode-go", "deepseek-v4-pro", "xhigh", true),
            ("opencode-go", "deepseek-v4.1-flash", "high", true),
            ("opencode-go", "deepseek-flash", "off", false),
            ("opencode-go", "deepseek-flash", "none", false),
            ("opencode-go", "muse-spark-1.2-contributor", "high", false),
            ("opencode-go", "gpt-5.6-luna", "high", false),
            ("openai", "model", "high", false),
            ("openai-compatible", "deepseek-flash", "high", false),
        ] {
            let execution = ProfileExecution::new(
                "profile".into(),
                provider.into(),
                model.into(),
                "openai-responses".into(),
                true,
                false,
                None,
                "https://example.invalid".into(),
                HashMap::new(),
                thinking.into(),
                execution("openai-responses", true).limits().clone(),
                "secret".into(),
            );
            let genuine = responses_input_from_transcript(&[original.clone()], &execution).unwrap();
            assert_eq!(genuine.len(), 1, "unmarked calls must never get reasoning");
            assert_eq!(genuine[0]["type"], "function_call");
            let mut runtime = original.clone();
            runtime.runtime_generated = true;
            let generated =
                responses_input_from_transcript(&[runtime.clone()], &execution).unwrap();
            assert_eq!(
                generated.len(),
                1 + usize::from(expected),
                "{provider}/{model} thinking={thinking}"
            );
            assert_eq!(generated.last(), genuine.last());
            if expected {
                assert_eq!(generated[0]["type"], "reasoning");
                assert!(!generated[0]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .is_empty());
            }
            // Even inconsistent provenance must not replace opaque provider data.
            let raw = std::sync::Arc::new(vec![
                serde_json::json!({
                    "id": "rs_original", "type": "reasoning", "summary": [],
                    "content": [{"type": "reasoning_text", "text": "Original reasoning.\n"}],
                }),
                genuine[0].clone(),
            ]);
            runtime.provider_context = Some(ProviderContext {
                profile_id: "profile".into(),
                provider: provider.into(),
                model: model.into(),
                api: "openai-responses".into(),
                output_items: raw.clone(),
            });
            assert_eq!(
                responses_input_from_transcript(&[runtime], &execution).unwrap(),
                *raw
            );
        }
        let mut legacy = serde_json::to_value(original).unwrap();
        legacy.as_object_mut().unwrap().remove("runtime_generated");
        assert!(
            !serde_json::from_value::<ProviderMessage>(legacy)
                .unwrap()
                .runtime_generated
        );
    }

    #[test]
    fn xai_cache_routing_is_session_stable_and_respects_explicit_headers() {
        let empty = HashMap::new();
        assert_eq!(
            request_headers("xai", "session-a", &empty)["x-grok-conv-id"],
            "session-a"
        );
        assert_ne!(
            request_headers("xai", "session-a", &empty),
            request_headers("xai", "session-b", &empty)
        );
        assert!(request_headers("openai", "session-a", &empty).is_empty());
        let configured = HashMap::from([("X-Grok-Conv-Id".into(), "custom".into())]);
        assert_eq!(request_headers("xai", "session-a", &configured), configured);
    }

    #[test]
    fn opencode_routing_uses_the_session_identity_and_an_identifiable_user_agent() {
        let configured = HashMap::from([
            ("X-OpenCode-Session".into(), "profile-wide-session".into()),
            ("x-custom".into(), "keep".into()),
        ]);
        let first = request_headers("opencode-go", "session-a", &configured);
        assert_eq!(first["x-opencode-session"], "session-a");
        assert!(!first.contains_key("X-OpenCode-Session"));
        assert!(first["User-Agent"].starts_with("zork-agent/"));
        assert_eq!(first["x-custom"], "keep");
        assert_eq!(first, request_headers("opencode-go", "session-a", &configured));
        assert_ne!(first["x-opencode-session"], request_headers("opencode-go", "session-b", &configured)["x-opencode-session"]);
        let configured = HashMap::from([("user-agent".into(), "custom-zork/1".into())]);
        let headers = request_headers("opencode-go", "session-a", &configured);
        assert_eq!(headers["user-agent"], "custom-zork/1");
        assert!(!headers.contains_key("User-Agent"));
    }

    #[test]
    fn tool_images_follow_capabilities_and_each_transport_contract() {
        use crate::session::wire::ToolImage;
        let tool = |id: &str, images| ProviderMessage {
            role: TranscriptRole::Tool,
            content: "image metadata".into(),
            images,
            is_error: false,
            runtime_generated: false,
            tool_call_id: Some(id.into()),
            tool_calls: Vec::new(),
            provider_context: None,
        };
        let messages = vec![
            tool(
                "image-call",
                vec![ToolImage {
                    media_type: "image/png".into(),
                    base64: "aW1n".into(),
                }],
            ),
            tool("peer-call", Vec::new()),
        ];
        let enabled = execution("openai-responses", true).with_image_input(true);
        let wire = responses_input_from_transcript(&messages, &enabled).unwrap();
        assert_eq!(
            wire[0]["output"][1]["image_url"],
            "data:image/png;base64,aW1n"
        );
        assert_eq!(wire[1]["call_id"], "peer-call");
        let disabled = execution("openai-responses", true);
        let wire = responses_input_from_transcript(&messages, &disabled).unwrap();
        assert!(wire[0]["output"]
            .as_str()
            .unwrap()
            .contains("Images were not sent"));
        assert!(!serde_json::to_string(&wire).unwrap().contains("aW1n"));
        let chat = prompt_from_transcript(
            &messages,
            &execution("openai-completions", true).with_image_input(true),
        )
        .unwrap();
        assert_eq!(chat.len(), 3);
        assert_eq!(chat[0].role, Role::Tool);
        assert_eq!(chat[1].role, Role::Tool);
        assert_eq!(chat[2].role, Role::User);
        assert!(matches!(&chat[2].content[1], ContentPart::Image { image, .. } if image == b"img"));
        let anthropic = prompt_from_transcript(
            &messages,
            &execution("anthropic-messages", true).with_image_input(true),
        )
        .unwrap();
        assert_eq!(anthropic.len(), 2);
        let ContentPart::ToolResult { result, .. } = &anthropic[0].content[0] else {
            panic!("tool result")
        };
        assert_eq!(result["value"][1]["source"]["data"], "aW1n");
    }

    #[test]
    fn chat_completions_merges_only_leading_system_messages_and_keeps_prefix() {
        let message = |role, content: &str| ProviderMessage {
            role,
            content: content.into(),
            images: Vec::new(),
            is_error: false,
            runtime_generated: false,
            tool_call_id: None,
            tool_calls: Vec::new(),
            provider_context: None,
        };
        let mut transcript = vec![
            message(TranscriptRole::System, "Session instructions"),
            message(TranscriptRole::System, "Tool catalog"),
            message(TranscriptRole::User, "New task"),
        ];
        let original = execution("openai-completions", true);
        let unchanged = prompt_from_transcript(&transcript, &original).unwrap();
        assert_eq!(unchanged.len(), 3);
        assert_eq!(unchanged[0].role, Role::System);
        assert_eq!(unchanged[1].role, Role::System);
        let execution = original.with_single_system_message(true);
        let first = prompt_from_transcript(&transcript, &execution).unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].role, Role::System);
        assert!(
            matches!(&first[0].content[0], ContentPart::Text { text, .. } if text == "Session instructions\n\nTool catalog")
        );
        assert_eq!(first[1].role, Role::User);
        transcript.push(message(TranscriptRole::Assistant, "Done"));
        transcript.push(message(TranscriptRole::User, "Next task"));
        let second = prompt_from_transcript(&transcript, &execution).unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&second[..first.len()]).unwrap()
        );
        assert_eq!(transcript.len(), 5);
        // Do not hoist a later system message into the already-sent prefix.
        transcript.push(message(TranscriptRole::System, "Late system message"));
        let late = prompt_from_transcript(&transcript, &execution).unwrap();
        assert_eq!(late.last().unwrap().role, Role::System);
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&late[..first.len()]).unwrap()
        );
    }

    #[test]
    fn system_position_rejection_is_permanent_only_for_bad_request() {
        for (status, retryable) in [(400, false), (502, true)] {
            let error = AiMuxError::ApiCall(aimux_core::ApiCallError {
                status_code: Some(status),
                message: "System message must be at the beginning.".into(),
                is_retryable: true,
                ..Default::default()
            });
            let ModelError::ProviderFailed(failure) =
                aimux_failure("openai.completions.stream_start", error)
            else {
                panic!("expected provider failure");
            };
            assert_eq!(failure.retryable, retryable);
        }
    }

    fn execution(api: &str, streaming: bool) -> ProfileExecution {
        ProfileExecution::new(
            "profile".to_owned(),
            "openai".to_owned(),
            "gpt-5.6-luna".to_owned(),
            api.to_owned(),
            streaming,
            false,
            None,
            "https://example.invalid".to_owned(),
            HashMap::new(),
            "max".to_owned(),
            zork_agent::session::ports::ModelLimits {
                context_window_tokens: 1_000_000,
                max_output_tokens: 128_000,
                reserve_percent: 10,
            },
            "secret".to_owned(),
        )
    }

    fn stream_request_fixture() -> ModelRequest {
        ModelRequest {
            session_id: "session".to_owned(),
            generation: 0,
            step_id: "step".to_owned(),
            selection: zork_agent::session::wire::SessionSelection {
                profile_id: "profile".to_owned(),
                model: "gpt-5.6-luna".to_owned(),
                thinking: "max".to_owned(),
            },
            transcript: std::sync::Arc::new(Vec::new()),
            tools: std::sync::Arc::new(Vec::new()),
            max_output_tokens: Some(128_000),
            independent: false,
            stream_observer: std::sync::Arc::new(zork_agent::session::model::SilentStreamObserver),
        }
    }

    fn stream_result(parts: Vec<StreamPart>) -> StreamResult {
        StreamResult {
            stream: Box::pin(futures_util::stream::iter(
                parts.into_iter().map(Ok::<_, AiMuxError>),
            )),
            request_body: None,
            response_headers: None,
        }
    }

    fn usage(input: u32, output: u32) -> Usage {
        Usage {
            input_tokens: aimux_core::types::TokenUsage {
                total: Some(input),
                ..Default::default()
            },
            output_tokens: aimux_core::types::TokenUsage {
                total: Some(output),
                reasoning: Some(output),
                ..Default::default()
            },
            raw: None,
        }
    }

    #[tokio::test]
    async fn progress_counts_reasoning_text_and_tool_bytes_without_exposing_reasoning() {
        #[derive(Debug, Default)]
        struct Observer {
            bytes: std::sync::Mutex<Vec<u64>>,
            text: std::sync::Mutex<String>,
        }
        impl zork_agent::session::model::ModelStreamObserver for Observer {
            fn output_delta(&self, _: &str, _: u64, _: &str, bytes: u64) {
                self.bytes.lock().unwrap().push(bytes);
            }
            fn text_delta(&self, _: &str, _: u64, _: &str, text: &str) {
                self.text.lock().unwrap().push_str(text);
            }
        }
        let observer = std::sync::Arc::new(Observer::default());
        let mut request = stream_request_fixture();
        request.stream_observer = observer.clone();
        let parts = vec![
            StreamPart::ReasoningDelta {
                id: "r".into(),
                delta: "hidden".into(),
                provider_metadata: None,
            },
            StreamPart::TextDelta {
                id: "t".into(),
                delta: "visible".into(),
                provider_metadata: None,
            },
            StreamPart::ToolInputStart {
                id: "c".into(),
                tool_name: "call".into(),
                provider_executed: None,
                dynamic: None,
                title: None,
                provider_metadata: None,
            },
            StreamPart::ToolInputDelta {
                id: "c".into(),
                delta: "{}".into(),
                provider_metadata: None,
            },
        ];
        let _ = model_outcome_from_stream_result(
            stream_result(parts),
            &request,
            &execution("openai-completions", true),
            None,
            full_input_diagnostics(0, 0),
        )
        .await;
        assert_eq!(observer.bytes.lock().unwrap().iter().sum::<u64>(), 15);
        assert_eq!(
            observer.bytes.lock().unwrap().len(),
            2,
            "first output and one coalesced tail"
        );
        assert_eq!(&*observer.text.lock().unwrap(), "visible");
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01, RETRY-02]
    async fn incomplete_tool_stream_keeps_request_and_usage_without_a_false_json_error() {
        let partial = r#"{"tool":"file.read","arguments":{"path":"/tmp"#;
        let result = stream_result(vec![
            StreamPart::ResponseMetadata {
                id: Some("response-123".to_owned()),
                timestamp: None,
                model_id: None,
            },
            StreamPart::ToolInputStart {
                id: "call-1".to_owned(),
                tool_name: "call".to_owned(),
                provider_executed: None,
                dynamic: None,
                title: None,
                provider_metadata: None,
            },
            StreamPart::ToolInputDelta {
                id: "call-1".to_owned(),
                delta: partial.to_owned(),
                provider_metadata: None,
            },
            StreamPart::ToolInputEnd {
                id: "call-1".to_owned(),
                provider_metadata: None,
            },
            StreamPart::ToolCall {
                tool_call_id: "call-1".to_owned(),
                tool_name: "call".to_owned(),
                input: Value::String(partial.to_owned()),
                provider_executed: None,
                dynamic: None,
                thought_signature: None,
                provider_metadata: None,
            },
            StreamPart::Finish {
                finish_reason: FinishReason {
                    unified: FinishReasonUnified::Stop,
                    raw: None,
                },
                usage: usage(321, 45),
                provider_metadata: None,
            },
        ]);

        let error = model_outcome_from_stream_result(
            result,
            &stream_request_fixture(),
            &execution("openai-completions", true),
            None,
            full_input_diagnostics(4, 5),
        )
        .await
        .expect_err("missing finish reason must fail");
        let ModelError::ProviderFailed(failure) = error else {
            panic!("expected provider failure");
        };
        assert_eq!(failure.stage, "provider.stream.finish");
        assert!(!failure.message.contains("EOF while parsing"));
        let input = failure.provider_input.expect("provider input");
        assert_eq!(input.logical_input_items, 4);
        assert_eq!(input.sent_input_items, 5);
        assert_eq!(input.response_id.as_deref(), Some("response-123"));
        let usage = failure.usage.expect("observed usage");
        assert_eq!(usage.input_tokens, 321);
        assert_eq!(usage.output_tokens, 45);
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01, RETRY-02]
    async fn transport_error_after_tool_input_end_is_not_masked_by_partial_json() {
        let result = stream_result(vec![
            StreamPart::ToolInputStart {
                id: "call-1".to_owned(),
                tool_name: "call".to_owned(),
                provider_executed: None,
                dynamic: None,
                title: None,
                provider_metadata: None,
            },
            StreamPart::ToolInputDelta {
                id: "call-1".to_owned(),
                delta: "{".to_owned(),
                provider_metadata: None,
            },
            StreamPart::ToolInputEnd {
                id: "call-1".to_owned(),
                provider_metadata: None,
            },
            StreamPart::Error {
                error: AiMuxError::InvalidResponseData("transport reset".to_owned()),
            },
        ]);

        let error = model_outcome_from_stream_result(
            result,
            &stream_request_fixture(),
            &execution("openai-completions", true),
            None,
            full_input_diagnostics(2, 2),
        )
        .await
        .expect_err("transport error must fail");
        let ModelError::ProviderFailed(failure) = error else {
            panic!("expected provider failure");
        };
        assert_eq!(failure.stage, "provider.stream.error_event");
        assert!(failure.message.contains("transport reset"));
        assert_eq!(failure.provider_input.unwrap().sent_input_items, 2);
    }

    #[test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01]
    fn non_streaming_result_preserves_the_streaming_outcome_contract() {
        let result = aimux_core::result::GenerateResult {
            content: vec![
                aimux_core::result::GenerateContent::Reasoning {
                    text: "summary".to_owned(),
                    provider_metadata: Some(serde_json::json!({
                        "openai": {
                            "itemId": "rs_1",
                            "reasoningEncryptedContent": "ciphertext",
                        }
                    })),
                },
                aimux_core::result::GenerateContent::Text {
                    text: "working".to_owned(),
                    provider_metadata: None,
                },
                aimux_core::result::GenerateContent::ToolCall {
                    tool_call_id: "call_1".to_owned(),
                    tool_name: "bash".to_owned(),
                    input: serde_json::json!({"cmd": "pwd"}),
                    provider_executed: None,
                    dynamic: None,
                    thought_signature: None,
                    provider_metadata: None,
                },
            ],
            finish_reason: aimux_core::types::FinishReason {
                unified: FinishReasonUnified::ToolCalls,
                raw: Some("tool_calls".to_owned()),
            },
            usage: aimux_core::types::Usage {
                input_tokens: aimux_core::types::TokenUsage {
                    total: Some(41),
                    ..Default::default()
                },
                output_tokens: aimux_core::types::TokenUsage {
                    total: Some(12),
                    text: Some(3),
                    reasoning: Some(9),
                    ..Default::default()
                },
                raw: None,
            },
            warnings: Vec::new(),
            provider_metadata: None,
            response: Default::default(),
            request_body: None,
            response_headers: None,
        };

        let output_items = std::sync::Arc::new(vec![
            serde_json::json!({
                "id": "rs_1",
                "type": "reasoning",
                "status": "completed",
                "encrypted_content": "ciphertext",
                "summary": [],
            }),
            serde_json::json!({
                "id": "msg_1",
                "type": "message",
                "status": "completed",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": "working", "annotations": [] }],
            }),
            serde_json::json!({
                "id": "fc_1",
                "type": "function_call",
                "status": "completed",
                "call_id": "call_1",
                "name": "bash",
                "arguments": "{\"cmd\":\"pwd\"}",
            }),
        ]);
        let outcome = model_outcome_from_generate_result(
            result,
            &execution("openai-responses", false),
            Some(output_items.clone()),
            full_input_diagnostics(1, 1),
        )
        .expect("valid non-streaming result");

        assert_eq!(outcome.text, "working");
        assert_eq!(outcome.tool_calls.len(), 1);
        assert_eq!(outcome.tool_calls[0].tool_call_id, "call_1");
        assert_eq!(outcome.tool_calls[0].tool_name, "bash");
        assert_eq!(
            outcome.tool_calls[0].arguments,
            serde_json::json!({"cmd": "pwd"})
        );
        let context = outcome.provider_context.expect("provider context");
        assert_eq!(context.output_items.as_ref(), output_items.as_ref());
        assert_eq!(context.output_items.len(), 3);
        assert_eq!(context.output_items[0]["id"], "rs_1");
        assert_eq!(context.output_items[0]["encrypted_content"], "ciphertext");
        let usage = outcome.usage.expect("exact provider usage");
        assert_eq!(usage.input_tokens, 41);
        assert_eq!(usage.cached_input_tokens, None);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.output_reasoning_tokens, Some(9));
        assert_eq!(usage.output_text_tokens, Some(3));
    }

    #[test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01]
    fn non_streaming_responses_accepts_plain_raw_reasoning() {
        let result = aimux_core::result::GenerateResult {
            content: vec![aimux_core::result::GenerateContent::Reasoning {
                text: "summary".to_owned(),
                provider_metadata: Some(serde_json::json!({
                    "openai": { "itemId": "rs_1" }
                })),
            }],
            finish_reason: aimux_core::types::FinishReason {
                unified: FinishReasonUnified::Stop,
                raw: Some("stop".to_owned()),
            },
            usage: Default::default(),
            warnings: Vec::new(),
            provider_metadata: None,
            response: Default::default(),
            request_body: None,
            response_headers: None,
        };

        let plain = std::sync::Arc::new(vec![serde_json::json!({
            "id": "rs_1",
            "type": "reasoning",
            "status": null,
            "summary": [],
            "content": [{ "type": "reasoning_text", "text": "summary" }],
            "encrypted_content": null,
        })]);
        let outcome = model_outcome_from_generate_result(
            result,
            &execution("openai-responses", false),
            Some(plain.clone()),
            full_input_diagnostics(1, 1),
        )
        .expect("plain reasoning is replayable as a raw output item");

        assert_eq!(
            outcome.provider_context.unwrap().output_items.as_ref(),
            plain.as_ref()
        );
    }

    #[test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01, PROVIDER-03]
    fn responses_options_preserve_profile_defined_reasoning_exactly() {
        let options = responses_provider_options("future-depth");

        assert_eq!(options["openai"]["reasoningEffort"], "future-depth");
        assert_eq!(options["openai"]["forceReasoning"], true);
        assert_eq!(options["openai"]["store"], false);

        let call_options = CallOptions {
            provider_options: Some(options),
            ..CallOptions::new(vec![LanguageModelPromptMessage {
                role: Role::User,
                content: vec![ContentPart::text("test")],
                provider_options: None,
            }])
        };
        let request = aimux_providers::openai::responses::build_responses_request_body(
            "gpt-5.6-luna",
            &call_options,
            true,
        );
        assert_eq!(request.body["reasoning"]["effort"], "future-depth");
        assert_eq!(request.body["store"], false);
    }
}
