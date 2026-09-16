//! Opt-in, bounded live-provider regression. Never reads a developer's default
//! profile implicitly. ZORK_LIVE_PROFILE points to an explicitly supplied private
//! copy; ZORK_LIVE_REPORT optionally receives secret-free aggregate evidence.
//! ZORK_LIVE_MODEL optionally selects one of the model cases below.

use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use serde_json::{json, Value};
use zork_agent::session::events::{SessionEvent, ToolOutcome, TurnOutcome};
use zork_agent::session::store::EventEnvelope;
use zork_agent::session::wire::SessionSelection;
use zork_agent_testkit::RealAgent;

fn history(agent: &RealAgent, session: &str) -> Vec<EventEnvelope> {
    agent
        .history(session, None, 2000)
        .expect("live test history")
}

fn summary(events: &[EventEnvelope]) -> Value {
    let mut started = 0;
    let mut completed = 0;
    let mut input_tokens = 0;
    let mut output_tokens = 0;
    let mut reasoning_items = 0;
    let mut tool_calls = BTreeMap::<String, usize>::new();
    let mut failed_tools = Vec::new();
    let mut provider_errors = Vec::new();
    for envelope in events {
        match &envelope.event {
            SessionEvent::StepStarted { .. } => started += 1,
            SessionEvent::StepCompleted {
                invocations,
                usage,
                provider_context,
                ..
            } => {
                completed += 1;
                for call in invocations {
                    *tool_calls.entry(call.tool.clone()).or_default() += 1;
                }
                if let Some(usage) = usage {
                    input_tokens += usage.input_tokens;
                    output_tokens += usage.output_tokens;
                }
                if let Some(context) = provider_context {
                    reasoning_items += context
                        .output_items
                        .iter()
                        .filter(|item| item["type"] == "reasoning")
                        .count();
                }
            }
            SessionEvent::StepFailed { error, .. } => provider_errors.push(json!({
                "stage": error.stage,
                "status": error.status_code,
                "retryable": error.retryable,
                "message": error.message,
            })),
            SessionEvent::ToolResult { result } if result.outcome != ToolOutcome::Succeeded => {
                failed_tools.push(json!({"tool": result.tool, "outcome": result.outcome}));
            }
            _ => {}
        }
    }
    json!({
        "requests": started, "completed_requests": completed,
        "input_tokens": input_tokens, "output_tokens": output_tokens,
        "reasoning_items": reasoning_items, "tool_calls": tool_calls,
        "provider_errors": provider_errors, "failed_tools": failed_tools,
    })
}

async fn run_turn(
    agent: &RealAgent,
    session: &str,
    phase: &str,
    prompt: String,
    reports: &mut Vec<Value>,
) -> Vec<EventEnvelope> {
    let baseline = history(agent, session).len();
    let started = Instant::now();
    agent
        .send_mail(session, prompt)
        .await
        .expect("live test input");
    let mut last_requests = 0;
    loop {
        let all = history(agent, session);
        let events = &all[baseline..];
        let evidence = summary(events);
        let requests = evidence["requests"].as_u64().unwrap();
        if requests != last_requests {
            eprintln!("{}", json!({"phase": phase, "requests": requests}));
            last_requests = requests;
        }
        let finished = events.iter().find_map(|event| match event.event {
            SessionEvent::TurnFinished { outcome, .. } => Some(outcome),
            _ => None,
        });
        if let Some(outcome) = finished {
            let report = json!({
                "phase": phase, "outcome": outcome,
                "elapsed_ms": started.elapsed().as_millis(), "evidence": evidence,
            });
            eprintln!("{report}");
            reports.push(report);
            assert_eq!(outcome, TurnOutcome::Finished, "live phase {phase} failed");
            return events.to_vec();
        }
        if requests >= 16 || started.elapsed() >= Duration::from_secs(180) {
            agent
                .cancel(session)
                .await
                .expect("cancel bounded live test");
            reports
                .push(json!({"phase": phase, "outcome": "budget_exceeded", "evidence": evidence}));
            panic!("live phase {phase} exceeded its request/time bound");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn tool_count(events: &[EventEnvelope], tool: &str) -> usize {
    events
        .iter()
        .filter_map(|event| match &event.event {
            SessionEvent::StepCompleted { invocations, .. } => Some(invocations),
            _ => None,
        })
        .flatten()
        .filter(|invocation| invocation.tool == tool)
        .count()
}

fn assert_each_invocation_settled_once(events: &[EventEnvelope]) {
    let mut invocations = BTreeMap::new();
    let mut results = BTreeMap::<&str, usize>::new();
    for event in events {
        match &event.event {
            SessionEvent::StepCompleted {
                invocations: calls,
                provider_context,
                ..
            } => {
                for call in calls {
                    assert!(invocations
                        .insert(call.invocation_id.as_str(), call)
                        .is_none());
                    assert_eq!(
                        provider_context
                            .as_ref()
                            .unwrap()
                            .output_items
                            .iter()
                            .filter(|item| {
                                item["type"] == "function_call"
                                    && item["call_id"] == call.provider_call_id
                            })
                            .count(),
                        1,
                        "each invocation must originate from one real model call"
                    );
                }
            }
            SessionEvent::ToolResult { result } => {
                *results.entry(result.invocation_id.as_str()).or_default() += 1;
            }
            _ => {}
        }
    }
    for id in invocations.keys() {
        assert_eq!(
            results.get(id),
            Some(&1),
            "one settlement per invocation {id}"
        );
    }
}

async fn exercise(agent: &mut RealAgent, profile: Value, reports: &mut Vec<Value>) {
    let requested_model = std::env::var("ZORK_LIVE_MODEL").ok();
    let mut cases = 0;
    for (model, streaming) in [
        ("deepseek-flash", true),
        ("deepseek-flash", false),
        ("muse-spark-1.2-contributor", true),
    ] {
        if requested_model
            .as_deref()
            .is_some_and(|selected| selected != model)
        {
            continue;
        }
        cases += 1;
        let mut selected = profile["models"]
            .as_array()
            .expect("profile models")
            .iter()
            .find(|entry| entry["id"] == model)
            .unwrap_or_else(|| panic!("explicit live profile lacks {model}"))
            .clone();
        assert_eq!(selected["api"], "openai-responses");
        assert_eq!(
            selected["enabled"], true,
            "do not enable a disabled live model"
        );
        assert!(selected["thinking"]
            .as_array()
            .unwrap()
            .contains(&json!("high")));
        assert!(selected["limits"]["context_window_tokens"]
            .as_u64()
            .is_some());
        selected["streaming"] = json!(streaming);
        selected["default"] = json!(true);
        // Bound this opt-in test's spending without changing the source profile.
        selected["limits"]["max_output_tokens"] = json!(selected["limits"]["max_output_tokens"]
            .as_u64()
            .unwrap()
            .min(4096));
        let mut settings = profile.clone();
        settings["models"] = json!([selected]);
        let case = format!("{model}-{}", if streaming { "stream" } else { "complete" });
        agent
            .install_profile(&case, settings)
            .expect("private test profile");
        let session = agent
            .create_configured_session(
                SessionSelection {
                    profile_id: case.clone(), model: model.into(), thinking: "high".into(),
                },
                Some("You are validating a coding-agent runtime inside an isolated disposable workspace. Use only the explicitly requested relative fixture files and commands. Never inspect credentials, environment variables, parent directories, or external services. Keep replies short. Complete every requested verification before ending the turn.".into()),
            )
            .await
            .expect("isolated live session");
        let workspace = agent.workspace(&session).unwrap().to_owned();
        let proof = format!("PROOF-{}", ulid::Ulid::new());
        std::fs::write(workspace.join("seed.txt"), &proof).unwrap();

        let events = run_turn(
            agent, &session, &format!("{case}/read-write-read"),
            "Read seed.txt with file.read. Create result.txt with file.write containing the exact seed contents followed by one newline and VERIFIED. Then read result.txt with file.read to verify it. Do not use shell for this phase. Reply DONE after verification.".into(),
            reports,
        ).await;
        assert!(tool_count(&events, "file.read") >= 2);
        // A live model may verify and correct its first write. Validate real
        // effects and per-invocation settlement, not one idealized model trace.
        assert!(tool_count(&events, "file.write") >= 1);
        assert_each_invocation_settled_once(&events);
        assert_eq!(
            std::fs::read_to_string(workspace.join("result.txt"))
                .unwrap()
                .trim_end(),
            format!("{proof}\nVERIFIED")
        );

        let events = run_turn(
            agent, &session, &format!("{case}/late-result-and-wait"),
            "Call shell.run exactly once with command `sleep 1; printf LATE-PROOF > late.txt` and outer call.wait=0. Then call the wait control tool once with arguments {\"seconds\":2}. After the wait, read late.txt with file.read and report its contents. Never repeat the shell command; wait for the original tool if still running.".into(),
            reports,
        ).await;
        assert_eq!(tool_count(&events, "shell.run"), 1);
        assert!(tool_count(&events, "wait") >= 1);
        assert!(tool_count(&events, "file.read") >= 1);
        assert_each_invocation_settled_once(&events);
        assert_eq!(
            std::fs::read_to_string(workspace.join("late.txt")).unwrap(),
            "LATE-PROOF"
        );

        agent
            .restart()
            .await
            .expect("restart real runtime and durable storage");
        let events = run_turn(
            agent, &session, &format!("{case}/restart-and-next-turn"),
            "Read result.txt and late.txt with file.read. Reply with the complete PROOF value from result.txt and the complete value from late.txt. Do not rewrite either file.".into(),
            reports,
        ).await;
        assert!(tool_count(&events, "file.read") >= 2);
        assert_eq!(tool_count(&events, "file.write"), 0);
        assert_each_invocation_settled_once(&events);
        let text = events
            .iter()
            .filter_map(|event| match &event.event {
                SessionEvent::StepCompleted { assistant_text, .. } => Some(assistant_text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(text.contains(&proof));
        assert!(text.contains("LATE-PROOF"));
        assert!(history(agent, &session)
            .iter()
            .any(|event| matches!(&event.event,
            SessionEvent::StepCompleted { provider_context: Some(context), .. }
            if context.output_items.iter().any(|item| item["type"] == "reasoning"))));
    }
    assert!(
        cases > 0,
        "ZORK_LIVE_MODEL must select a declared test case"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires an explicit private ZORK_LIVE_PROFILE and makes bounded billable model calls"]
// Contract: docs/design/agent-runtime.md [PROVIDER-01, PROJECTION-02, WAIT-01, RECOVERY-01]
async fn opencode_go_real_models_complete_tools_waits_and_restart_replay() {
    let profile_path =
        std::env::var_os("ZORK_LIVE_PROFILE").expect("set ZORK_LIVE_PROFILE explicitly");
    let bytes = std::fs::read(profile_path).expect("read explicit private profile");
    let profile: Value = serde_json::from_slice(&bytes).expect("profile JSON");
    assert_eq!(profile["provider"], "opencode-go");
    assert_eq!(profile["base_url"], "https://opencode.ai/zen/go/v1");
    let mut agent = RealAgent::new().expect("isolated production composition");
    let mut reports = Vec::new();
    let result = AssertUnwindSafe(exercise(&mut agent, profile, &mut reports))
        .catch_unwind()
        .await;
    agent.shutdown().await;
    if let Some(path) = std::env::var_os("ZORK_LIVE_REPORT") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&json!({
                "passed": result.is_ok(), "production_runtime": true,
                "real_provider": true, "phases": reports,
            }))
            .unwrap(),
        )
        .expect("write secret-free live report");
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
