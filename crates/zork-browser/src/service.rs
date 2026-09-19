use crate::{cdp::Cdp, normalize_url, Action, Inspection};
use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub const MAX_TABS: usize = 32;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tab {
    pub id: String,
    pub host: String,
    pub url: String,
    pub title: String,
    pub loading: bool,
}
#[derive(Clone)]
pub struct Frame {
    pub sequence: u64,
    pub bgra: Arc<Vec<u8>>,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub width: f64,
    pub height: f64,
}
struct LiveTab {
    info: Tab,
    session: String,
    frame: Option<Frame>,
}
#[derive(Default)]
struct State {
    generation: u64,
    tabs: HashMap<String, LiveTab>,
    sequence: u64,
    disconnected: bool,
}
struct Running {
    cdp: Cdp,
}

/// Serializes browser mutations off the UI thread. Host is supplied by the
/// authenticated conversation binding, never by page content or model arguments.
pub struct Browser {
    profile: PathBuf,
    lifecycle: Mutex<()>,
    running: Mutex<Option<Running>>,
    state: Arc<Mutex<State>>,
    changes: zork_notify::Hub<String>,
}
impl Browser {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            lifecycle: Mutex::new(()),
            running: Mutex::new(None),
            state: Arc::new(Mutex::new(State::default())),
            changes: Default::default(),
        }
    }
    pub fn subscribe(&self, host: &str) -> zork_notify::Changes {
        self.changes.subscribe([host.to_owned()])
    }
    pub fn shutdown(&self) {
        let _lifecycle = self.lifecycle.lock().unwrap();
        self.stop();
    }
    // The lifecycle lock prevents a new tab from entering this runtime while
    // its last tab is being closed. Never hold it while waiting on navigation.
    fn stop(&self) {
        let running = self.running.lock().unwrap().take();
        if let Some(running) = running {
            running.cdp.close();
        }
    }
    fn stop_if_idle(&self) {
        if self.state.lock().unwrap().tabs.is_empty() {
            self.stop();
        }
    }
    fn start(&self) -> Result<()> {
        let mut running = self.running.lock().unwrap();
        if running.is_some() && !self.state.lock().unwrap().disconnected {
            return Ok(());
        }
        *running = None;
        let generation = {
            let mut state = self.state.lock().unwrap();
            let generation = state.generation.wrapping_add(1);
            *state = State {
                generation,
                ..State::default()
            };
            generation
        };
        let events = self.state.clone();
        let changed = self.changes.clone();
        let cdp = Cdp::launch(&self.profile, move |event, pixels| {
            changed.publish(observe(&events, generation, event, pixels));
        })?;
        cdp.call("Target.setDiscoverTargets", json!({"discover":true}), None)?;
        *running = Some(Running { cdp });
        Ok(())
    }
    fn live(&self) -> Result<(Cdp, Arc<Mutex<State>>)> {
        let running = self.running.lock().unwrap();
        let r = running.as_ref().context("浏览器未启动")?;
        ensure!(
            !self.state.lock().unwrap().disconnected,
            "浏览器 已关闭，请重新打开网页"
        );
        Ok((r.cdp.clone(), self.state.clone()))
    }
    pub fn tabs(&self, host: &str) -> Vec<Tab> {
        self.all_tabs()
            .into_iter()
            .filter(|tab| tab.host == host)
            .collect()
    }
    pub fn all_tabs(&self) -> Vec<Tab> {
        let state = self.state.lock().unwrap();
        if state.disconnected {
            return vec![];
        }
        let mut tabs: Vec<_> = state.tabs.values().map(|tab| tab.info.clone()).collect();
        tabs.sort_by(|a, b| a.id.cmp(&b.id));
        tabs
    }
    pub fn frame(&self, host: &str, id: &str) -> Option<Frame> {
        let state = self.state.lock().unwrap();
        let t = state.tabs.get(id)?;
        (t.info.host == host).then(|| t.frame.clone()).flatten()
    }
    fn target(state: &Mutex<State>, host: &str, id: &str) -> Result<(Tab, String)> {
        let state = state.lock().unwrap();
        ensure!(!state.disconnected, "浏览器 已关闭，请重新打开浏览器");
        let tab = state
            .tabs
            .get(id)
            .filter(|t| t.info.host == host)
            .context("此对话没有该网页，或网页已关闭")?;
        Ok((tab.info.clone(), tab.session.clone()))
    }
    pub fn execute(&self, host: &str, action: Action) -> Result<Value> {
        ensure!(
            !host.is_empty() && host.len() <= 512,
            "invalid browser conversation"
        );
        if matches!(action, Action::List) {
            return Ok(json!({"tabs":self.tabs(host)}));
        }
        let mut lifecycle = matches!(action, Action::Open { .. } | Action::Close { .. })
            .then(|| self.lifecycle.lock().unwrap());
        if let Action::Open { url } = &action {
            // Rejected input must not start an otherwise unused engine.
            normalize_url(url)?;
        }
        if matches!(action, Action::Open { .. }) {
            self.start()?;
        }
        let (cdp, state) = self.live()?;
        if let Action::Open { url } = action {
            let url = normalize_url(&url)?;
            ensure!(
                state.lock().unwrap().tabs.len() < MAX_TABS,
                "最多保留 {MAX_TABS} 个网页，请先关闭标签页"
            );
            let target = cdp.call("Target.createTarget", json!({"url":"about:blank"}), None)?;
            let id = target["targetId"]
                .as_str()
                .context("浏览器 未返回标签页 ID")?
                .to_owned();
            let setup = (|| -> Result<()> {
                let attached = cdp.call(
                    "Target.attachToTarget",
                    json!({"targetId":id,"flatten":true}),
                    None,
                )?;
                let session = attached["sessionId"]
                    .as_str()
                    .context("浏览器 未返回会话 ID")?
                    .to_owned();
                state.lock().unwrap().tabs.insert(
                    id.clone(),
                    LiveTab {
                        info: Tab {
                            id: id.clone(),
                            host: host.into(),
                            url: url.clone(),
                            title: "新标签页".into(),
                            loading: true,
                        },
                        session: session.clone(),
                        frame: None,
                    },
                );
                self.changes.publish([host.to_owned()]);
                drop(lifecycle.take());
                cdp.call("Page.enable", json!({}), Some(&session))?;
                navigate(&cdp, &session, &url)?;
                Ok(())
            })();
            drop(lifecycle.take());
            if let Err(error) = setup {
                let _lifecycle = self.lifecycle.lock().unwrap();
                state.lock().unwrap().tabs.remove(&id);
                self.changes.publish([host.to_owned()]);
                let _ = cdp.call("Target.closeTarget", json!({"targetId":id}), None);
                self.stop_if_idle();
                return Err(error);
            }
            return Ok(json!({"tab":Self::target(&state, host, &id)?.0}));
        }
        let id = match &action {
            Action::Navigate { tab_id, .. }
            | Action::Back { tab_id }
            | Action::Forward { tab_id }
            | Action::Reload { tab_id }
            | Action::Stop { tab_id }
            | Action::Close { tab_id }
            | Action::Read { tab_id }
            | Action::Click { tab_id, .. }
            | Action::Type { tab_id, .. }
            | Action::Key { tab_id, .. }
            | Action::Scroll { tab_id, .. }
            | Action::Screenshot { tab_id } => tab_id.clone(),
            _ => unreachable!(),
        };
        let (tab, session) = Self::target(&state, host, &id)?;
        // 浏览器 internal/file pages may be reached manually; never expose them to Agents.
        if !matches!(action, Action::Close { .. }) {
            normalize_url(&tab.url)?;
        }
        match action {
            Action::Navigate { url, .. } => {
                navigate(&cdp, &session, &normalize_url(&url)?)?;
            }
            Action::Back { .. } | Action::Forward { .. } => {
                let direction = if matches!(action, Action::Back { .. }) {
                    -1
                } else {
                    1
                };
                let history = cdp.call("Page.getNavigationHistory", json!({}), Some(&session))?;
                let current = history["currentIndex"].as_i64().unwrap_or(0);
                let next = current + direction;
                let entries = history["entries"]
                    .as_array()
                    .context("invalid 浏览器 navigation history")?;
                ensure!(
                    next >= 0 && (next as usize) < entries.len(),
                    "没有可用的历史页面"
                );
                normalize_url(entries[next as usize]["url"].as_str().unwrap_or(""))?;
                cdp.call(
                    "Page.navigateToHistoryEntry",
                    json!({"entryId":entries[next as usize]["id"]}),
                    Some(&session),
                )?;
            }
            Action::Reload { .. } => {
                cdp.call("Page.reload", json!({}), Some(&session))?;
            }
            Action::Stop { .. } => {
                cdp.call("Page.stopLoading", json!({}), Some(&session))?;
            }
            Action::Close { .. } => {
                cdp.call("Target.closeTarget", json!({"targetId":id}), None)?;
                state.lock().unwrap().tabs.remove(&id);
                self.changes.publish([host.to_owned()]);
                // This is global across conversation hosts: closing one chat's
                // last page must not stop another chat's browser or Agent tab.
                self.stop_if_idle();
            }
            Action::Read { .. } => {
                let page = evaluate(&cdp, &session, "({url:location.href,title:document.title,viewport:{width:innerWidth,height:innerHeight},text:(document.body?.innerText||'').slice(0,32000),elements:Array.from(document.querySelectorAll('a,button,input,textarea,select,[role=button]')).slice(0,100).map(e=>({tag:e.tagName.toLowerCase(),id:e.id,role:e.getAttribute('role'),name:e.getAttribute('name'),label:e.getAttribute('aria-label'),text:(e.innerText||'').slice(0,200),type:e.getAttribute('type')}))})")?;
                return Ok(json!({"tab_id":id,"page":page,"content_trust":"untrusted_web_page"}));
            }
            Action::Click { selector, .. } => {
                let point = element_point(&cdp, &session, &selector)?;
                mouse(
                    &cdp,
                    &session,
                    point.0,
                    point.1,
                    "mousePressed",
                    "left",
                    1,
                    0,
                )?;
                mouse(
                    &cdp,
                    &session,
                    point.0,
                    point.1,
                    "mouseReleased",
                    "left",
                    1,
                    0,
                )?;
            }
            Action::Type { selector, text, .. } => {
                ensure!(text.len() <= 16000, "输入文字过长");
                let encoded = serde_json::to_string(&selector)?;
                let editable=evaluate(&cdp,&session,&format!("(()=>{{const e=document.querySelector({encoded});return !!e&&!e.disabled&&!e.readOnly&&(e.isContentEditable||e.tagName==='TEXTAREA'||(e.tagName==='INPUT'&&!['button','submit','reset','checkbox','radio','file','hidden','range','color','image'].includes(e.type)))}})()"))?;
                ensure!(editable.as_bool() == Some(true), "目标元素不可输入文字");
                let point = element_point(&cdp, &session, &selector)?;
                mouse(
                    &cdp,
                    &session,
                    point.0,
                    point.1,
                    "mousePressed",
                    "left",
                    1,
                    0,
                )?;
                mouse(
                    &cdp,
                    &session,
                    point.0,
                    point.1,
                    "mouseReleased",
                    "left",
                    1,
                    0,
                )?;
                cdp.call("Input.insertText", json!({"text":text}), Some(&session))?;
            }
            Action::Key { key, .. } => {
                key_event(&cdp, &session, &key, 0)?;
            }
            Action::Scroll {
                x,
                y,
                delta_x,
                delta_y,
                ..
            } => {
                ensure_coordinates(x, y)?;
                ensure!(
                    delta_x.is_finite()
                        && delta_y.is_finite()
                        && delta_x.abs() <= 10000.
                        && delta_y.abs() <= 10000.,
                    "invalid scroll delta"
                );
                cdp.call(
                    "Input.dispatchMouseEvent",
                    json!({"type":"mouseWheel","x":x,"y":y,"deltaX":delta_x,"deltaY":delta_y}),
                    Some(&session),
                )?;
            }
            Action::Screenshot { .. } => {
                let layout = cdp.call("Page.getLayoutMetrics", json!({}), Some(&session))?;
                let width = layout["cssLayoutViewport"]["clientWidth"]
                    .as_f64()
                    .unwrap_or(800.);
                let height = layout["cssLayoutViewport"]["clientHeight"]
                    .as_f64()
                    .unwrap_or(600.);
                for (quality, maximum) in [(65, 800.), (40, 640.), (25, 480.)] {
                    let scale = (maximum / width.max(height)).min(1.);
                    let result = cdp.call("Page.captureScreenshot", json!({"format":"jpeg","quality":quality,"captureBeyondViewport":false,"clip":{"x":layout["cssLayoutViewport"]["pageX"],"y":layout["cssLayoutViewport"]["pageY"],"width":width,"height":height,"scale":scale}}), Some(&session))?;
                    let data = result["data"].as_str().context("screenshot missing")?;
                    if data.len() <= 160 * 1024 {
                        return Ok(
                            json!({"tab_id":id,"url":tab.url,"mime":"image/jpeg","base64":data}),
                        );
                    }
                }
                bail!("网页截图超出传输上限");
            }
            _ => unreachable!(),
        }
        Ok(json!({"ok":true,"tab_id":id}))
    }
    pub fn viewport(
        &self,
        host: &str,
        id: &str,
        width: u32,
        height: u32,
        visible: bool,
    ) -> Result<()> {
        self.viewport_scaled(host, id, width, height, visible, 1.)
    }
    pub fn viewport_scaled(
        &self,
        host: &str,
        id: &str,
        width: u32,
        height: u32,
        visible: bool,
        scale: f32,
    ) -> Result<()> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        // A resize does not invalidate the last paint. Its dimensions travel
        // with the frame, and consumers can display it until the next paint.
        // Clearing on every request can erase a frame before the UI reads it.
        if !visible {
            if let Some(tab) = state.lock().unwrap().tabs.get_mut(id) {
                tab.frame = None;
            }
        }
        cdp.call(
            "Zork.viewport",
            json!({"width":width,"height":height,"visible":visible,"scale":scale}),
            Some(&session),
        )?;
        Ok(())
    }
    pub fn handoff(&self, host: &str, id: &str) -> Result<()> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        let _ = session;
        cdp.call("Target.activateTarget", json!({"targetId":id}), None)?;
        Ok(())
    }
    pub fn refresh_title(&self, host: &str, id: &str) -> Result<()> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        let page = evaluate(&cdp, &session, "({url:location.href,title:document.title})")?;
        if let Some(tab) = state.lock().unwrap().tabs.get_mut(id) {
            if page["url"].as_str() == Some(tab.info.url.as_str()) {
                if let Some(title) = page["title"].as_str().filter(|title| !title.is_empty()) {
                    tab.info.title = title.chars().take(512).collect();
                }
            }
        }
        Ok(())
    }
    /// Human-only action: reveal downloaded files in the system file manager.
    pub fn open_downloads(&self) -> Result<()> {
        let directory = self.profile.join("downloads");
        std::fs::create_dir_all(&directory)?;
        #[cfg(target_os = "macos")]
        let status = std::process::Command::new("open")
            .arg(&directory)
            .status()?;
        #[cfg(target_os = "windows")]
        let status = std::process::Command::new("explorer")
            .arg(&directory)
            .status()?;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let status = std::process::Command::new("xdg-open")
            .arg(&directory)
            .status()?;
        ensure!(status.success(), "无法打开下载文件夹");
        Ok(())
    }
    pub fn pointer(
        &self,
        host: &str,
        id: &str,
        x: f64,
        y: f64,
        kind: &str,
        button: &str,
        clicks: u32,
        modifiers: u8,
    ) -> Result<()> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        mouse(&cdp, &session, x, y, kind, button, clicks, modifiers)
    }
    pub fn input(&self, host: &str, id: &str, text: &str) -> Result<()> {
        ensure!(text.len() <= 16000, "输入文字过长");
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        cdp.call("Input.insertText", json!({"text":text}), Some(&session))?;
        Ok(())
    }
    pub fn composition(
        &self,
        host: &str,
        id: &str,
        text: &str,
        start: usize,
        end: usize,
    ) -> Result<()> {
        ensure!(
            text.len() <= 16000 && start <= end && end <= text.encode_utf16().count(),
            "invalid composition"
        );
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        cdp.call(
            "Input.imeSetComposition",
            json!({"text":text,"selectionStart":start,"selectionEnd":end}),
            Some(&session),
        )?;
        Ok(())
    }
    pub fn selected_text(&self, host: &str, id: &str) -> Result<String> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        let value=evaluate(&cdp,&session,"(()=>{const e=document.activeElement;if(e?.tagName==='INPUT'&&e.type==='password')return '';if(e&&typeof e.selectionStart==='number')return e.value.slice(e.selectionStart,e.selectionEnd).slice(0,64000);return String(window.getSelection()||'').slice(0,64000)})()")?;
        Ok(value.as_str().unwrap_or("").into())
    }
    pub fn key(&self, host: &str, id: &str, key: &str, modifiers: u8) -> Result<()> {
        let (cdp, state) = self.live()?;
        let (_, session) = Self::target(&state, host, id)?;
        key_event(&cdp, &session, key, modifiers)
    }
    pub fn inspect(&self, host: &str, id: &str, x: f64, y: f64) -> Result<Inspection> {
        ensure_coordinates(x, y)?;
        let (cdp, state) = self.live()?;
        let (tab, session) = Self::target(&state, host, id)?;
        normalize_url(&tab.url)?;
        let script = format!("(()=>{{const e=document.elementFromPoint({x},{y});if(!e)throw Error('此处没有元素');const parts=[];for(let n=e;n&&n.nodeType===1;n=n.parentElement){{if(n.id){{parts.unshift('#'+CSS.escape(n.id));break;}}const tag=n.tagName.toLowerCase();let i=1;for(let p=n.previousElementSibling;p;p=p.previousElementSibling)if(p.tagName===n.tagName)i++;parts.unshift(tag+':nth-of-type('+i+')');}}const copy=e.cloneNode(true);for(const n of [copy,...copy.querySelectorAll('*')]){{for(const a of [...n.attributes])if(a.name.startsWith('on')||['value','srcdoc','nonce'].includes(a.name))n.removeAttribute(a.name);if(['SCRIPT','STYLE','INPUT','TEXTAREA'].includes(n.tagName))n.textContent='';}}return {{selector:parts.join(' > '),tag:e.tagName.toLowerCase(),text:(e.innerText||'').slice(0,2000),html:copy.outerHTML.slice(0,4000)}}}})()");
        let result = evaluate(&cdp, &session, &script)?;
        ensure!(
            Self::target(&state, host, id)?.0.url == tab.url,
            "网页已跳转，请重新选择元素"
        );
        Ok(Inspection {
            tab_id: id.into(),
            url: tab.url,
            title: tab.title,
            selector: result["selector"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(4000)
                .collect(),
            tag: result["tag"].as_str().unwrap_or("").into(),
            text: result["text"].as_str().unwrap_or("").into(),
            html: result["html"].as_str().unwrap_or("").into(),
        })
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn observe(shared: &Mutex<State>, generation: u64, event: Value, pixels: Vec<u8>) -> Vec<String> {
    let mut state = shared.lock().unwrap();
    // The previous event reader can finish after a replacement process starts.
    // Its late disconnect/paint events must not mutate the new runtime's state.
    if state.generation != generation {
        return vec![];
    }
    let method = event["method"].as_str().unwrap_or("");
    let session = event["sessionId"].as_str().unwrap_or("");
    if method == "Zork.disconnected" {
        if state.disconnected {
            return vec![];
        }
        state.disconnected = true;
        return state
            .tabs
            .values()
            .map(|tab| tab.info.host.clone())
            .collect();
    }
    let info = &event["params"]["targetInfo"];
    let id = event["params"]["targetId"]
        .as_str()
        .or_else(|| info["targetId"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            state
                .tabs
                .iter()
                .find(|(_, tab)| tab.session == session)
                .map(|(id, _)| id.clone())
        });
    let Some(id) = id else {
        return vec![];
    };
    if method == "Target.targetCreated" {
        let owner = info["openerId"]
            .as_str()
            .and_then(|parent| state.tabs.get(parent))
            .map(|tab| tab.info.host.clone());
        if let Some(host) = owner.filter(|_| !state.tabs.contains_key(&id)) {
            state.tabs.insert(
                id.clone(),
                LiveTab {
                    info: Tab {
                        id: id.clone(),
                        host: host.clone(),
                        url: info["url"].as_str().unwrap_or("about:blank").into(),
                        title: String::new(),
                        loading: true,
                    },
                    session: id,
                    frame: None,
                },
            );
            return vec![host];
        }
        return vec![];
    }
    let Some(tab) = state.tabs.get(&id) else {
        return vec![];
    };
    let before = tab.info.clone();
    if method == "Target.targetDestroyed" {
        state.tabs.remove(&id);
        return vec![before.host];
    }
    if method == "Zork.paint" {
        let width = event["params"]["width"].as_u64().unwrap_or(0);
        let height = event["params"]["height"].as_u64().unwrap_or(0);
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || pixels.len() != (width * height * 4) as usize
        {
            return vec![];
        }
        state.sequence += 1;
        let sequence = state.sequence;
        let scale = event["params"]["scale"]
            .as_f64()
            .unwrap_or(1.)
            .clamp(1., 2.);
        state.tabs.get_mut(&id).unwrap().frame = Some(Frame {
            sequence,
            bgra: Arc::new(pixels),
            pixel_width: width as u32,
            pixel_height: height as u32,
            width: width as f64 / scale,
            height: height as f64 / scale,
        });
        return vec![before.host];
    }
    let tab = state.tabs.get_mut(&id).unwrap();
    match method {
        "Target.targetInfoChanged" => {
            if let Some(url) = info["url"].as_str() {
                tab.info.url = url.into();
            }
            if let Some(title) = info["title"].as_str() {
                tab.info.title = title.chars().take(512).collect();
            }
        }
        "Page.frameStartedLoading" | "Page.frameStoppedLoading" => {
            tab.info.loading = method == "Page.frameStartedLoading"
        }
        _ => return vec![],
    }
    if tab.info == before {
        vec![]
    } else {
        vec![before.host]
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn late_disconnect_from_previous_runtime_cannot_close_replacement() {
        let state = Mutex::new(State {
            generation: 2,
            tabs: HashMap::from([(
                "new-tab".into(),
                LiveTab {
                    info: Tab {
                        id: "new-tab".into(),
                        host: "conversation".into(),
                        url: "about:blank".into(),
                        title: String::new(),
                        loading: false,
                    },
                    session: "new-tab".into(),
                    frame: None,
                },
            )]),
            ..State::default()
        });
        let disconnected = json!({"method":"Zork.disconnected"});
        assert!(observe(&state, 1, disconnected.clone(), vec![]).is_empty());
        assert!(!state.lock().unwrap().disconnected);
        assert_eq!(
            observe(&state, 2, disconnected.clone(), vec![]),
            ["conversation"]
        );
        assert!(state.lock().unwrap().disconnected);
        assert!(observe(&state, 2, disconnected, vec![]).is_empty());
    }
}

fn navigate(cdp: &Cdp, session: &str, url: &str) -> Result<()> {
    let mut changes = cdp.navigation(session);
    let result = cdp.call("Page.navigate", json!({"url":url}), Some(session))?;
    if let Some(error) = result["errorText"].as_str() {
        bail!("网页打开失败：{error}");
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        changes.checkpoint();
        if let Ok(tree) = cdp.call("Page.getFrameTree", json!({}), Some(session)) {
            let frame = &tree["frameTree"]["frame"];
            let committed = result
                .get("loaderId")
                .is_none_or(|loader| frame["loaderId"] == *loader);
            if committed {
                if let Ok(value) = evaluate(
                    cdp,
                    session,
                    "({ready:document.readyState,url:location.href})",
                ) {
                    if value["url"] == frame["url"]
                        && matches!(value["ready"].as_str(), Some("interactive" | "complete"))
                    {
                        break;
                    }
                }
            }
        }
        ensure!(
            changes.blocking_changed(deadline)?,
            "网页仍在加载，请检查页面状态后继续"
        );
    }
    Ok(())
}
fn evaluate(cdp: &Cdp, session: &str, expression: &str) -> Result<Value> {
    let result = cdp.call(
        "Runtime.evaluate",
        json!({"expression":expression,"returnByValue":true,"awaitPromise":true,"timeout":8000}),
        Some(session),
    )?;
    if result.get("exceptionDetails").is_some() {
        bail!("网页操作失败：{}", result["exceptionDetails"]["text"]);
    }
    result["result"]
        .get("value")
        .cloned()
        .context("网页未返回结果")
}
fn element_point(cdp: &Cdp, session: &str, selector: &str) -> Result<(f64, f64)> {
    ensure!(
        !selector.is_empty() && selector.len() <= 4000,
        "invalid selector"
    );
    let selector = serde_json::to_string(selector)?;
    let value = evaluate(cdp, session, &format!("(()=>{{const matches=document.querySelectorAll({selector});if(matches.length!==1)throw Error('Selector must match exactly one element');const e=matches[0];e.scrollIntoView({{block:'center',inline:'center'}});const r=e.getBoundingClientRect();if(!r.width||!r.height||e.disabled)throw Error('Element is not actionable');return {{x:r.x+r.width/2,y:r.y+r.height/2}}}})()"))?;
    Ok((
        value["x"].as_f64().context("missing x")?,
        value["y"].as_f64().context("missing y")?,
    ))
}
fn ensure_coordinates(x: f64, y: f64) -> Result<()> {
    ensure!(
        x.is_finite()
            && y.is_finite()
            && (0. ..=16384.).contains(&x)
            && (0. ..=16384.).contains(&y),
        "invalid page coordinates"
    );
    Ok(())
}
fn mouse(
    cdp: &Cdp,
    session: &str,
    x: f64,
    y: f64,
    kind: &str,
    button: &str,
    clicks: u32,
    modifiers: u8,
) -> Result<()> {
    ensure_coordinates(x, y)?;
    ensure!(
        matches!(kind, "mousePressed" | "mouseReleased" | "mouseMoved")
            && matches!(button, "none" | "left" | "right" | "middle")
            && clicks <= 3,
        "invalid mouse input"
    );
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({"type":kind,"x":x,"y":y,"button":button,"clickCount":clicks,"modifiers":modifiers}),
        Some(session),
    )?;
    Ok(())
}
fn key_event(cdp: &Cdp, session: &str, key: &str, mut modifiers: u8) -> Result<()> {
    let mut parts = key.split('+').collect::<Vec<_>>();
    let key = parts.pop().unwrap_or(key);
    for modifier in parts {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => 2,
            "cmd" | "meta" => 4,
            "shift" => 8,
            "alt" => 1,
            _ => bail!("未知按键修饰符"),
        };
    }
    let (key, code) = match key {
        "enter" | "Enter" => ("Enter", 13),
        "tab" | "Tab" => ("Tab", 9),
        "backspace" | "Backspace" => ("Backspace", 8),
        "delete" | "Delete" => ("Delete", 46),
        "escape" | "Escape" => ("Escape", 27),
        "left" | "ArrowLeft" => ("ArrowLeft", 37),
        "right" | "ArrowRight" => ("ArrowRight", 39),
        "up" | "ArrowUp" => ("ArrowUp", 38),
        "down" | "ArrowDown" => ("ArrowDown", 40),
        "home" | "Home" => ("Home", 36),
        "end" | "End" => ("End", 35),
        "pageup" | "PageUp" => ("PageUp", 33),
        "pagedown" | "PageDown" => ("PageDown", 34),
        "a" | "A" if modifiers != 0 => ("a", 65),
        _ => return Err(anyhow!("不支持此按键：{key}")),
    };
    for kind in ["rawKeyDown", "keyUp"] {
        cdp.call(
            "Input.dispatchKeyEvent",
            json!({"type":kind,"key":key,"windowsVirtualKeyCode":code,"modifiers":modifiers}),
            Some(session),
        )?;
    }
    Ok(())
}
