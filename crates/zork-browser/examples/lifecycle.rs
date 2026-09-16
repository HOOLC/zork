//! Real CEF lifecycle regression with an isolated profile and local HTTP fixture.
//! ZORK_BROWSER_RUNTIME selects the packaged engine. Run in a macOS GUI session.
use anyhow::{ensure, Result};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};
use zork_browser::{Action, Browser};

#[cfg(unix)]
fn assert_runtime_exited(profile: &Path) -> Result<()> {
    let profile = profile.canonicalize()?;
    let profile = profile.to_string_lossy();
    let patterns = [
        format!("/ZorkBrowser {profile}"),
        format!("/zork-browser-runtime {profile}"),
        format!("--user-data-dir={profile}"),
    ];
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = std::process::Command::new("ps")
            .args(["-axo", "args="])
            .output()?;
        ensure!(output.status.success(), "cannot inspect browser processes");
        let remaining: Vec<_> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| patterns.iter().any(|p| line.contains(p)))
            .map(str::to_owned)
            .collect();
        if remaining.is_empty() {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "browser processes survived last tab: {remaining:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn open(browser: &Browser, host: &str, url: &str) -> Result<String> {
    Ok(
        browser.execute(host, Action::Open { url: url.into() })?["tab"]["id"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}
fn close(browser: &Browser, host: &str, id: &str) -> Result<()> {
    browser.execute(host, Action::Close { tab_id: id.into() })?;
    ensure!(
        browser.frame(host, id).is_none(),
        "closed page pixels retained"
    );
    Ok(())
}
fn main() -> Result<()> {
    let root = tempfile::tempdir()?;
    let profile = std::env::var_os("ZORK_BROWSER_PROBE_PROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.path().join("cef"));
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let origin = format!("http://{}", listener.local_addr()?);
    let (loading, load_started) = mpsc::channel();
    let (release, load_release) = mpsc::channel();
    let load_release = Arc::new(Mutex::new(load_release));
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let loading = loading.clone();
            let load_release = load_release.clone();
            // A speculative preconnect must not block the actual page request.
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut request = [0; 4096];
                if !matches!(stream.read(&mut request), Ok(n) if n > 0) {
                    return;
                }
                if request.starts_with(b"GET /slow ") {
                    let _ = loading.send(());
                    let _ = load_release
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(15));
                }
                let body = "<!doctype html><title>Lifecycle fixture</title><body><script>document.body.textContent=document.cookie||'fresh';document.cookie='lifecycle=kept;max-age=3600;path=/';</script>";
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            });
        }
    });
    let browser = Browser::new(profile.clone());
    let first = open(&browser, "first", &origin)?;
    let other = open(&browser, "other", &origin)?;
    close(&browser, "first", &first)?;
    ensure!(browser.tabs("first").is_empty(), "closed tab retained");
    ensure!(
        browser.execute(
            "other",
            Action::Read {
                tab_id: other.clone()
            }
        )?["page"]["title"]
            == "Lifecycle fixture",
        "another conversation was closed"
    );
    close(&browser, "other", &other)?;
    #[cfg(unix)]
    assert_runtime_exited(&profile)?;
    println!("PASS last tab releases engine/helpers; another conversation survives");

    let reopened = open(&browser, "first", &origin)?;
    let read = browser.execute(
        "first",
        Action::Read {
            tab_id: reopened.clone(),
        },
    )?;
    ensure!(
        read["page"]["text"]
            .as_str()
            .unwrap()
            .contains("lifecycle=kept"),
        "cookie lost across idle shutdown"
    );
    close(&browser, "first", &reopened)?;
    #[cfg(unix)]
    assert_runtime_exited(&profile)?;
    println!("PASS engine reopens with persisted fixture cookie");

    std::thread::scope(|scope| -> Result<()> {
        let pending = scope.spawn(|| open(&browser, "loading", &format!("{origin}/slow")));
        load_started.recv_timeout(Duration::from_secs(10))?;
        let id = browser.tabs("loading")[0].id.clone();
        let began = Instant::now();
        let closed = close(&browser, "loading", &id);
        let _ = release.send(());
        closed?;
        ensure!(
            began.elapsed() < Duration::from_secs(5),
            "close blocked behind navigation"
        );
        ensure!(
            pending.join().unwrap().is_err(),
            "closed navigation completed"
        );
        Ok(())
    })?;
    #[cfg(unix)]
    assert_runtime_exited(&profile)?;
    println!("PASS closing a loading page cancels navigation and releases engine");
    Ok(())
}
