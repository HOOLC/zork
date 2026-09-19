use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

pub struct Login {
    account: Account,
    listener: TcpListener,
    state: String,
    verifier: String,
    redirect: String,
    attempt: String,
    url: String,
}
pub(super) fn secret() -> Result<String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
impl Account {
    pub async fn begin_login(&self, name: &str) -> Result<Login> {
        ensure!(
            !name.is_empty() && name.len() <= 80,
            "login device name must be 1–80 bytes"
        );
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let redirect = format!(
            "http://127.0.0.1:{}/oauth/callback",
            listener.local_addr()?.port()
        );
        let verifier = secret()?;
        let state = secret()?;
        let mut url = reqwest::Url::parse(&format!("{}/v1/auth/google/start", self.origin))?;
        url.query_pairs_mut()
            .append_pair("redirect_uri", &redirect)
            .append_pair("state", &state)
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256")
            .append_pair("name", name);
        let attempt = ulid::Ulid::new().to_string();
        {
            let _lock = self.lock().await?;
            let mut account = storage::read(&self.root)?;
            account.login_attempt = Some(attempt.clone());
            storage::write(&self.root, &account)?;
        }
        Ok(Login {
            account: self.clone(),
            listener,
            state,
            verifier,
            redirect,
            attempt,
            url: url.to_string(),
        })
    }
}
impl Login {
    pub fn url(&self) -> &str {
        &self.url
    }
    pub async fn cancel(&self) -> Result<()> {
        self.account.cancel_login(&self.attempt).await
    }
    pub async fn finish(&self) -> Result<Status> {
        let result = tokio::time::timeout(Duration::from_secs(300), self.receive()).await;
        let _ = self.cancel().await;
        result.context("Google login timed out")?
    }
    async fn receive(&self) -> Result<Status> {
        loop {
            let (mut socket, _) = self.listener.accept().await?;
            let mut bytes = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(2), async {
                while bytes.len() < 8192 && !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut chunk = [0; 1024];
                    let length = socket.read(&mut chunk).await?;
                    if length == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..length]);
                }
                Ok::<(), std::io::Error>(())
            })
            .await;
            if !matches!(read, Ok(Ok(()))) {
                continue;
            }
            let callback = parse_callback(&bytes, &self.redirect, &self.state);
            let Some(code) = callback else {
                let _ = socket.write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 22\r\n\r\nInvalid login callback").await;
                continue;
            };
            let result = self.exchange(&code).await;
            let (status, body) = if result.is_ok() {
                ("200 OK", "Zork login complete. Return to the client.")
            } else {
                (
                    "400 Bad Request",
                    "Zork login failed. Return to the client and retry.",
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            result?;
            return self.account.status(false).await;
        }
    }
    async fn exchange(&self, code: &str) -> Result<()> {
        ensure!(!code.is_empty(), "Google login was cancelled");
        let tokens: Tokens = self.account.request(Method::POST, "/v1/auth/token", None,
            Some(serde_json::json!({ "code": code, "code_verifier": self.verifier, "redirect_uri": self.redirect }))).await?;
        self.account.complete_login(&self.attempt, tokens).await
    }
}
impl Account {
    pub(super) async fn reserve_login(&self, attempt: &str) -> Result<()> {
        let _lock = self.lock().await?;
        let mut file = storage::read(&self.root)?;
        file.login_attempt = Some(attempt.to_owned());
        storage::write(&self.root, &file)
    }
    pub(super) async fn cancel_login(&self, attempt: &str) -> Result<()> {
        let _lock = self.lock().await?;
        let mut file = storage::read(&self.root)?;
        if file.login_attempt.as_deref() == Some(attempt) {
            file.login_attempt = None;
            storage::write(&self.root, &file)?;
        }
        Ok(())
    }
    pub(super) async fn complete_login(&self, attempt: &str, tokens: Tokens) -> Result<()> {
        let session = self.session_from(tokens)?;
        {
            let _lock = self.lock().await?;
            let mut account = storage::read(&self.root)?;
            if account.login_attempt.as_deref() != Some(attempt) {
                account.pending_revocations.push(Revocation {
                    session,
                    all: false,
                });
                storage::write(&self.root, &account)?;
                anyhow::bail!("Google login was superseded or cancelled");
            }
            if let Some(previous) = account.current.replace(session) {
                account.pending_revocations.push(Revocation {
                    session: previous,
                    all: false,
                });
            }
            account.login_attempt = None;
            storage::write(&self.root, &account)?;
        }
        let _ = self.flush_revocations().await;
        Ok(())
    }
}

fn parse_callback(bytes: &[u8], redirect: &str, expected_state: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if !text.contains("\r\n\r\n") {
        return None;
    }
    let mut lines = text.split("\r\n");
    let mut request = lines.next()?.split_whitespace();
    if request.next()? != "GET" {
        return None;
    }
    let path = request.next()?;
    if !path.starts_with("/oauth/callback?")
        || request.next()? != "HTTP/1.1"
        || request.next().is_some()
    {
        return None;
    }
    let base = reqwest::Url::parse(redirect).ok()?;
    let hosts = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(key, _)| key.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .collect::<Vec<_>>();
    if hosts.len() != 1 || hosts[0] != format!("127.0.0.1:{}", base.port()?) {
        return None;
    }
    let url = base.join(path).ok()?;
    let pairs = url.query_pairs().collect::<Vec<_>>();
    let states = pairs
        .iter()
        .filter(|(key, _)| key == "state")
        .collect::<Vec<_>>();
    if states.len() != 1 || states[0].1 != expected_state {
        return None;
    }
    let codes = pairs
        .iter()
        .filter(|(key, _)| key == "code")
        .collect::<Vec<_>>();
    if pairs.iter().any(|(key, _)| key == "error") {
        return Some(String::new());
    }
    if codes.len() != 1 || codes[0].1.len() != 87 {
        return None;
    }
    Some(codes[0].1.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_requires_own_state_host_and_single_code() {
        let code = format!("{}.{}", "a".repeat(43), "b".repeat(43));
        let request = |query: &str, host: &str| {
            format!("GET /oauth/callback?{query} HTTP/1.1\r\nHost: {host}\r\n\r\n")
        };
        let good = format!("state=ours&code={code}");
        let parse = |text: String| {
            parse_callback(
                text.as_bytes(),
                "http://127.0.0.1:3456/oauth/callback",
                "ours",
            )
        };
        assert_eq!(parse(request(&good, "127.0.0.1:3456")), Some(code.clone()));
        assert!(parse(request(&good, "attacker.test")).is_none());
        assert!(parse(request(&good.replace("ours", "theirs"), "127.0.0.1:3456")).is_none());
        assert!(parse(request(&format!("{good}&state=ours"), "127.0.0.1:3456")).is_none());
        assert!(parse(request(&format!("{good}&code={code}"), "127.0.0.1:3456")).is_none());
    }
}
