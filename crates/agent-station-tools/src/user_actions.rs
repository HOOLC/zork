//! Runtime lifecycle tracking for business tools that can wait for a user.
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zork_agent::session::tools::ToolContext;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Track {
        session_id: String,
        invocation_id: String,
        target: String,
    },
    Finish {
        session_id: String,
        invocation_id: String,
        target: String,
    },
    Cancel {
        session_id: String,
        invocation_id: String,
        target: String,
    },
}
impl Command {
    pub fn identity(&self) -> (&str, &str, &str) {
        match self {
            Self::Track {
                session_id,
                invocation_id,
                target,
                ..
            }
            | Self::Finish {
                session_id,
                invocation_id,
                target,
            }
            | Self::Cancel {
                session_id,
                invocation_id,
                target,
            } => (session_id, invocation_id, target),
        }
    }
}
pub(crate) async fn lifecycle(
    base: &str,
    http: &reqwest::Client,
    context: &ToolContext,
    target: Option<&str>,
    phase: &str,
) -> anyhow::Result<()> {
    let (session_id, invocation_id, target) = (
        context.session_id.clone(),
        context.invocation_id.clone(),
        target.unwrap_or("local").into(),
    );
    let command = match phase {
        "track" => Command::Track {
            session_id,
            invocation_id,
            target,
        },
        "finish" => Command::Finish {
            session_id,
            invocation_id,
            target,
        },
        "cancel" => Command::Cancel {
            session_id,
            invocation_id,
            target,
        },
        _ => unreachable!(),
    };
    let mut retry = Duration::from_millis(750);
    loop {
        let result = async {
            let response = http
                .post(format!("{base}/v1/internal/user-actions"))
                .json(&command)
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            if status.is_server_error() {
                return Ok(None);
            }
            super::namespaced::decode_response(status, &body).map(Some)
        }
        .await;
        match result {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) if e.is::<reqwest::Error>() => {}
            Err(e) => return Err(e),
        }
        tokio::time::sleep(retry).await;
        retry = retry.saturating_mul(2).min(Duration::from_secs(30));
    }
}
