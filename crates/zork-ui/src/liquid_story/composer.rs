//! Offline adapter connecting the shared composer to a core-owned fixture.
use super::*;
use crate::components::{liquid::composer as view, text_input::ComposerFilesPasted};
use zork_client_types::composer::{Controller, Intent, Snapshot};
struct Factory(fn() -> Box<dyn Controller>);
impl Global for Factory {}
pub fn install(cx: &mut App, factory: fn() -> Box<dyn Controller>) {
    cx.set_global(Factory(factory));
}

pub(super) struct Host {
    controller: Option<Box<dyn Controller>>,
    pub snapshot: Snapshot,
    pub widths: Vec<f32>,
    pub fan: Spring,
    pub pinned: bool,
    pub member: Option<String>,
    pub hovered_member: Option<String>,
    pub member_leave: Option<Instant>,
    pub file: Option<u64>,
    pub choosing: bool,
    pub departures: liquid::departure::Departures,
    pub departure_origin: liquid::departure::Origin,
}
impl Host {
    pub fn new(kind: Kind, cx: &App) -> Self {
        let mut controller = if kind == Kind::Composer {
            cx.try_global::<Factory>().map(|f| (f.0)())
        } else {
            None
        };
        let snapshot = controller
            .as_mut()
            .map(|c| c.apply(Intent::Inspect))
            .unwrap_or_default();
        Self {
            controller,
            snapshot,
            widths: Vec::new(),
            fan: Spring::new(0.),
            pinned: false,
            member: None,
            hovered_member: None,
            member_leave: None,
            file: None,
            choosing: false,
            departures: Default::default(),
            departure_origin: Default::default(),
        }
    }
    pub fn apply(&mut self, intent: Intent) {
        if let Some(controller) = &mut self.controller {
            self.snapshot = controller.apply(intent);
        }
    }
}
impl Card {
    pub(super) fn composer_subscriptions(&self, cx: &mut Context<Self>) {
        cx.subscribe(&self.input, |v, _, event: &ComposerFilesPasted, cx| {
            if v.kind != Kind::Composer {
                return;
            }
            let names = event
                .0
                .entries()
                .iter()
                .flat_map(|entry| match entry {
                    ClipboardEntry::ExternalPaths(paths) => paths
                        .paths()
                        .iter()
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .collect(),
                    ClipboardEntry::Image(_) => vec!["粘贴的图片.png".into()],
                    _ => Vec::new(),
                })
                .collect();
            v.composer_intent(Intent::Files(names), cx);
        })
        .detach();
    }
    pub(super) fn composer_intent(&mut self, intent: Intent, cx: &mut Context<Self>) {
        let departing = matches!(intent, Intent::Primary | Intent::Submit)
            .then(|| self.composer.snapshot.text.clone());
        let before = self.composer.snapshot.events.len();
        self.composer.apply(intent);
        if let Some(text) = departing {
            if !cx.reduce_motion()
                && !self.surfaces.is_empty()
                && self.composer.snapshot.events.len() > before
                && self
                    .composer
                    .snapshot
                    .events
                    .last()
                    .is_some_and(|event| event.starts_with("send:"))
            {
                let sim = &mut self.surfaces[0].simulation;
                self.composer.departures.emit(
                    self.composer.departure_origin,
                    &text,
                    sim.pose(),
                    sim,
                );
            }
        }
        self.composer.file = None;
        self.composer.member = None;
        self.composer.hovered_member = None;
        let state = &self.composer.snapshot;
        if self.input.read(cx).value() != state.text {
            self.input
                .update(cx, |input, cx| input.set_value(state.text.clone(), cx));
        }
        self.actions = state.events.len();
        self.status = if state.events.is_empty() {
            String::new()
        } else {
            format!("core 样例已接受 {} 次操作 · 未连接服务", state.events.len())
        };
        self.disabled = !state.capabilities.editable;
        self.busy = false;
        self.measure_key = None;
        self.retarget();
        cx.notify();
    }
    pub(super) fn composer_action(
        &mut self,
        action: view::Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            view::Action::FocusEditor => window.focus(&self.input.read(cx).focus_handle(), cx),
            view::Action::MemberAnchor(_, _) => {}
            view::Action::Primary => self.composer_intent(Intent::Primary, cx),
            view::Action::ChooseFiles => self.choose_composer_files(cx),
            view::Action::FanHover(hover) => {
                self.composer.fan.target = if hover || self.composer.pinned {
                    1.
                } else {
                    0.
                };
                cx.notify();
            }
            view::Action::ToggleFan => {
                window.focus(&self.focus, cx);
                self.composer.pinned = !self.composer.pinned;
                self.composer.fan.target = if self.composer.pinned { 1. } else { 0. };
                cx.notify();
            }
            view::Action::RemoveFile(id) => self.composer_intent(Intent::RemoveFile(id), cx),
            view::Action::OpenFile(id) => {
                window.focus(&self.focus, cx);
                self.composer.file = Some(id);
                cx.notify();
            }
            view::Action::MemberHover(id) => {
                if let Some(id) = id {
                    self.composer.hovered_member = Some(id);
                    self.composer.member_leave = None;
                } else {
                    self.composer.member_leave = Some(Instant::now());
                }
                cx.notify();
            }
            view::Action::Member(id) => {
                window.focus(&self.focus, cx);
                self.composer.member = if self.composer.member.as_ref() == Some(&id) {
                    None
                } else {
                    Some(id)
                };
                cx.notify();
            }
        }
        let _ = window;
    }
    fn choose_composer_files(&mut self, cx: &mut Context<Self>) {
        if !self.composer.snapshot.capabilities.editable {
            return;
        }
        #[cfg(not(target_family = "wasm"))]
        {
            let paths = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: true,
                prompt: None,
            });
            cx.spawn(async move |this, cx| {
                if let Ok(Ok(Some(paths))) = paths.await {
                    let names = paths
                        .into_iter()
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .collect();
                    let _ = this.update(cx, |v, cx| v.composer_intent(Intent::Files(names), cx));
                }
            })
            .detach();
        }
        #[cfg(target_family = "wasm")]
        {
            // Browser capability adapter only: selected names enter the core
            // fixture. No file bytes leave the machine and no upload is modeled.
            let script=js_sys::Function::new_no_args("globalThis.__zorkLiquidPicked=null; const input=document.createElement('input'); input.type='file'; input.multiple=true; input.hidden=true; input.setAttribute('data-liquid-picker',''); const done=()=>{globalThis.__zorkLiquidPicked=JSON.stringify(Array.from(input.files||[],f=>f.name)); input.remove();}; input.addEventListener('change',done,{once:true}); input.addEventListener('cancel',done,{once:true}); document.body.append(input); input.click();");
            if script.call0(&js_sys::global()).is_ok() {
                self.composer.choosing = true;
                cx.notify();
            }
        }
    }
    pub(super) fn poll_composer_files(&mut self, cx: &mut Context<Self>) {
        if self
            .composer
            .member_leave
            .is_some_and(|t| t.elapsed().as_millis() >= 80)
        {
            self.composer.member_leave = None;
            self.composer.hovered_member = None;
        }
        #[cfg(target_family = "wasm")]
        if self.composer.choosing {
            if let Ok(result) =
                js_sys::Reflect::get(&js_sys::global(), &"__zorkLiquidPicked".into())
            {
                if let Some(result) = result.as_string() {
                    self.composer.choosing = false;
                    if let Ok(names) = serde_json::from_str(&result) {
                        self.composer_intent(Intent::Files(names), cx);
                    }
                }
            }
        }
        let _ = cx;
    }
}

mod example;
pub use example::Example;
