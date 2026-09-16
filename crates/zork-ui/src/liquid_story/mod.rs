//! Interactive fixtures built from the shared Rust liquid surfaces and editors.
//! These examples emit presentation events; they do not save settings or send messages.
mod catalog;
pub mod business;
mod composer;
pub use composer::{install as install_composer_fixture, Example as ComposerExample};
mod playground;
mod library;
mod primitives;
mod render;
use crate::components::{
    liquid::{self, Material, Options, Pose, Simulation, Spring, Surface, FIXED_DT},
    message::MessageDocument,
    selection::TranscriptSelection,
    text_input::{ComposerInput, ComposerLayoutChanged, ComposerSubmit},
};
pub use catalog::{Kind, Section};
use gpui::{prelude::*, *};
pub use primitives::Example as Primitive;
use serde_json::{json, Value};
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
use std::{cell::RefCell, rc::Rc};
#[cfg(target_family = "wasm")]
use web_time::Instant;

#[derive(Clone)]
struct Config {
    material: Material,
    slow: bool,
    cycle: bool,
    seed: u32,
    epoch: u64,
    reset: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            material: Material::default(),
            slow: false,
            cycle: false,
            seed: 7321,
            epoch: 0,
            reset: 0,
        }
    }
}

pub struct Gallery {
    regions: crate::components::region::Regions<Self>,
    background: Option<Entity<playground::BackgroundView>>,
    config: Rc<RefCell<Config>>,
    cards: Vec<Entity<Card>>,
    section: Section,
    group: usize,
    count: usize,
    focused: Option<Kind>,
    selected_kind: Option<Kind>,
    business_family: Option<String>,
    business_examples: std::collections::HashMap<String, Entity<business::Example>>,
    panel: Option<playground::Panel>,
    modal: liquid::overlay::Dialog,
    library: Option<Entity<library::Library>>,
    presented_panel: Option<playground::Panel>,
    recording: Option<Instant>,
    last_record: Option<Value>,
    record_frame_pending: bool,
}
impl Gallery {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_kind(None, cx)
    }
    pub fn with_kind(kind: Option<Kind>, cx: &mut Context<Self>) -> Self {
        let config = Rc::new(RefCell::new(Config::default()));
        let cards = Kind::ALL
            .into_iter()
            .filter(|k| kind.is_none_or(|v| v == *k))
            .map(|kind| {
                cx.new(|cx| Card::new(kind, format!("liquid-{}", kind.key()), config.clone(), cx))
            })
            .collect();
        Self {
            regions: Default::default(),
            background: None,
            config,
            cards,
            section: kind.map_or(Section::Components, Kind::section),
            group: kind.map_or(0, Kind::group),
            count: 4,
            focused: kind,
            selected_kind: None,
            business_family: None,
            business_examples: Default::default(),
            panel: None,
            modal: liquid::overlay::Dialog::new(cx),
            library: None,
            presented_panel: None,
            recording: None,
            last_record: None,
            record_frame_pending: false,
        }
    }
    fn region_stats(&self, cx: &App) -> Value {
        #[cfg(feature = "headless-bench")]
        { json!(self.regions.counters(cx)) }
        #[cfg(not(feature = "headless-bench"))]
        { let _ = cx; Value::Null }
    }
    pub fn inspect(&self, cx: &App) -> Value {
        json!({"renderRegions":self.region_stats(cx),"businessExample":self.business_family.as_ref().and_then(|family|self.business_examples.get(family)).map(|view|view.read(cx).inspect(cx)),"businessCatalog":business::families(cx),"businessFamily":self.business_family,"engine":"rust-ddr","canonicalKinds":Kind::ALL.iter().map(|k|k.key()).collect::<Vec<_>>(),"catalog":Kind::ALL.iter().map(|kind|json!({"kind":kind.key(),"section":kind.section(),"group":kind.group()})).collect::<Vec<_>>(),"section":self.section,"group":self.group,"selectedKind":self.selected_kind.map(Kind::key),"panel":self.panel.map(playground::Panel::key),"dialog":self.modal.inspect(),"navigation":self.library.as_ref().map_or_else(Vec::new, |v| v.read(cx).navigation.surfaces()),"parameters":self.config.borrow().material,"slow":self.config.borrow().slow,"benchmarkCount":self.count,"recording":self.recording.is_some(),"lastRecord":self.last_record,"cards":self.cards.iter().map(|c|c.read(cx).inspect(cx)).collect::<Vec<_>>()})
    }
    fn active_cards(&self, cx: &App) -> Vec<Entity<Card>> {
        if self.section == Section::Scenarios && self.focused.is_none() && self.group != 4 {
            return vec![];
        }
        if self.group == 4 {
            Kind::BENCH
                .into_iter()
                .take(self.count)
                .filter_map(|kind| self.cards.iter().find(|c| c.read(cx).kind == kind).cloned())
                .collect()
        } else {
            self.cards
                .iter()
                .filter(|c| {
                    self.focused.is_some()
                        || self.selected_kind.map_or_else(
                            || c.read(cx).kind.group() == self.group,
                            |kind| c.read(cx).kind == kind,
                        )
                })
                .cloned()
                .collect()
        }
    }
    fn refresh_cards(&self, cx: &mut Context<Self>) {
        crate::components::region::invalidate(cx, &["playground-body"]);
        for card in self.active_cards(cx) {
            card.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }
    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        self.recording = Some(Instant::now());
        self.last_record = None;
        let mut cfg = self.config.borrow_mut();
        cfg.cycle = true;
        cfg.reset += 1;
        drop(cfg);
        self.refresh_cards(cx);
    }
    fn record_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .recording
            .is_some_and(|start| start.elapsed().as_secs_f64() >= 5.)
        {
            let cfg = self.config.borrow().clone();
            let cards = self.active_cards(cx);
            let rows=cards.iter().map(|card|{let c=card.read(cx);json!({"kind":c.kind.key(),"width":c.width,"frames":c.samples(cx),"visible":c.visible(cx)})}).collect::<Vec<_>>();
            self.last_record = Some(
                json!({"count":cards.len(),"scope":"Rust physics and contour extraction only. GPUI layout, tessellation, GPU and presentation excluded.","parameters":cfg.material,"seed":cfg.seed,"viewport":{"width":window.viewport_size().width.as_f32(),"height":window.viewport_size().height.as_f32(),"scale":window.scale_factor()},"controls":rows}),
            );
            self.recording = None;
            self.config.borrow_mut().cycle = false;
            self.refresh_cards(cx);
        }
        if self.recording.is_some() && !self.record_frame_pending {
            self.record_frame_pending = true;
            let weak = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.record_frame_pending = false;
                    cx.notify();
                });
            });
        }
    }
    fn set_section(&mut self, section: Section, cx: &mut Context<Self>) {
        if self.recording.is_some() || (self.section == section && self.group != 4) {
            return;
        }
        self.section = section;
        self.group = section.first_group();
        self.selected_kind = None;
        self.config.borrow_mut().cycle = false;
        self.refresh_cards(cx);
        cx.notify();
    }
    fn select_business(&mut self, family: String, cx: &mut Context<Self>) {
        self.section = Section::Scenarios;
        self.group = 2;
        self.selected_kind = None;
        self.business_family = Some(family);
        self.panel = None;
        self.config.borrow_mut().cycle = false;
        crate::components::region::invalidate(cx, &["playground-body"]);
    }
    fn set_group(&mut self, group: usize, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        self.group = group;
        if let Some(kind) = Kind::ALL.into_iter().find(|kind| kind.group() == group) {
            self.section = kind.section();
        }
        self.selected_kind = None;
        self.panel = None;
        self.config.borrow_mut().cycle = false;
        self.refresh_cards(cx);
    }
    fn select_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        self.group = kind.group();
        self.section = kind.section();
        self.selected_kind = Some(kind);
        self.panel = None;
        self.config.borrow_mut().cycle = false;
        self.refresh_cards(cx);
    }
    fn set_parameter(&mut self, key: &str, direction: i32, cx: &mut Context<Self>) {
        if self.recording.is_some() {
            return;
        }
        let mut cfg = self.config.borrow_mut();
        match key {
            "budget" => {
                let presets = [6, 12, 24, 64];
                let i = presets
                    .iter()
                    .position(|n| *n == cfg.material.budget)
                    .unwrap_or(1);
                cfg.material.budget = presets[(i as i32 + direction).clamp(0, 3) as usize];
            }
            "flow" => {
                cfg.material.flow = (cfg.material.flow + direction as f64 * 0.04).clamp(0., 0.4)
            }
            "adhesion" => {
                cfg.material.adhesion =
                    (cfg.material.adhesion + direction as f64 * 0.12).clamp(0., 1.2)
            }
            "smoothing" => {
                cfg.material.smoothing =
                    (cfg.material.smoothing + direction as f64 * 0.1).clamp(0., 1.)
            }
            "damping" => {
                cfg.material.damping =
                    (cfg.material.damping + direction as f64 * 0.1).clamp(0.4, 2.)
            }
            _ => {}
        };
        cfg.epoch += 1;
        drop(cfg);
        self.refresh_cards(cx);
    }
}

struct Card {
    layout_cache: crate::components::region::IntrinsicCache,
    kind: Kind,
    specimen: Option<Entity<primitives::Specimen>>,
    composer: composer::Host,
    id: String,
    config: Rc<RefCell<Config>>,
    seen_epoch: u64,
    seen_reset: u64,
    surfaces: Vec<Surface>,
    progress: Spring,
    width: f32,
    open: bool,
    selected: usize,
    popover_measure_key: Option<(&'static str, Font)>,
    popover_label_width: f32,
    variant: usize,
    variant_menu: bool,
    selector: liquid::overlay::Popover,
    popover: liquid::overlay::Popover,
    dialog: liquid::overlay::Dialog,
    panel: liquid::panel::ContentPanel,
    navigation: liquid::navigation::Navigation,
    show_title: bool,
    input: Entity<ComposerInput>,
    second: Entity<ComposerInput>,
    endpoint: Entity<ComposerInput>,
    input_height: f32,
    measure_key: Option<(String, u32)>,
    disabled: bool,
    invalid: bool,
    busy: bool,
    status: String,
    actions: usize,
    files: Vec<&'static str>,
    members: [bool; 3],
    drafts: Vec<crate::comments::DraftComment>,
    comment_editor: Entity<crate::components::comments::Editor>,
    comment_anchor: Rc<std::cell::Cell<Bounds<Pixels>>>,
    quote: Option<String>,
    preview: bool,
    selection: Rc<RefCell<TranscriptSelection>>,
    document: MessageDocument,
    focus: FocusHandle,
    trigger_focus: FocusHandle,
    last: Option<Instant>,
    pending_frame: bool,
    cycle_ticks: usize,
    local_accumulator: f64,
    frames: Vec<FrameSample>,
    pending: Option<(Instant, String)>,
    hud: String,
    hud_at: Option<Instant>,
}
use liquid::overlay::FrameSample;

impl Card {
    fn new(kind: Kind, id: String, config: Rc<RefCell<Config>>, cx: &mut Context<Self>) -> Self {
        crate::components::region::forget_on_release(cx);
        let comment_editor = cx.new(|cx| crate::components::comments::Editor::new(id.clone(), cx));
        cx.subscribe(&comment_editor, |v, _, event: &crate::components::comments::Submit, cx| {
            v.actions += 1;
            let comment = crate::comments::DraftComment {
                id: event.editing.clone().unwrap_or_else(|| format!("mock-comment-{}", v.actions)),
                source: event.source.clone(), comment: event.text.clone(),
            };
            if let Some(old) = v.drafts.iter_mut().find(|old| old.id == comment.id) { *old = comment; }
            else { v.drafts.push(comment); }
            v.comment_editor.update(cx, |editor, cx| editor.dismiss(cx));
            v.open = false;
            v.status = format!("{} 条 mock 草稿", v.drafts.len());
            cx.notify();
        }).detach();
        cx.subscribe(&comment_editor, |v, _, _: &crate::components::comments::Closed, cx| {
            v.open = false;
            v.selection.borrow_mut().clear();
            cx.notify();
        }).detach();
        let input = if kind == Kind::Comments { comment_editor.read(cx).input() } else { cx.new(|cx| {
            let input = ComposerInput::new(
                if kind == Kind::Composer {
                    "输入内容…"
                } else if kind == Kind::Comments {
                    "补充你的看法…"
                } else {
                    "填写示例名称…"
                },
                cx,
            );
            if matches!(kind, Kind::Fields | Kind::Modal) {
                input.single_line()
            } else {
                input
            }
        }) };
        let second = cx.new(|cx| ComposerInput::new("访问凭据…", cx).single_line());
        let endpoint = cx.new(|cx| ComposerInput::new("服务地址…", cx).single_line());
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.observe(&second, |_, _, cx| cx.notify()).detach();
        cx.observe(&endpoint, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&input, |v, _, _: &ComposerLayoutChanged, cx| {
            v.measure_key = None;
            cx.notify();
        })
        .detach();
        cx.subscribe(&input, |v, _, _: &ComposerSubmit, cx| {
            if v.kind == Kind::Composer {
                v.composer_intent(zork_client_types::composer::Intent::Submit, cx);

            }
        })
        .detach();
        let specimen = if let Kind::Primitive(example) = kind {
            Some(cx.new(|cx| primitives::Specimen::new(example, id.clone(), cx)))
        } else {
            None
        };
        let value = Self {
            layout_cache: Default::default(),
            kind,
            specimen,
            composer: composer::Host::new(kind, cx),
            id,
            config,
            seen_epoch: 0,
            seen_reset: 0,
            surfaces: Vec::new(),
            progress: Spring::new(0.),
            width: 0.,
            open: false,
            selected: 0,
            popover_measure_key: None,
            popover_label_width: 0.,
            variant: 0,
            variant_menu: false,
            selector: liquid::overlay::Popover::new(cx),
            popover: liquid::overlay::Popover::new(cx),
            dialog: liquid::overlay::Dialog::new(cx),
            panel: liquid::panel::ContentPanel::default(),
            navigation: liquid::navigation::Navigation::new(),
            show_title: true,
            input,
            second,
            endpoint,
            input_height: 20.,
            measure_key: None,
            disabled: false,
            invalid: false,
            busy: false,
            status: String::new(),
            actions: 0,
            files: vec!["组件规范.md", "界面参考.png", "实现笔记.txt"],
            members: [true, false, false],
            drafts: Vec::new(),
            comment_editor,
            comment_anchor: Default::default(),
            quote: None,
            preview: false,
            selection: Default::default(),
            document: MessageDocument::parse(
                "小控件保持紧致，大表面可以呈现局部流动。选中这段文字，为它添加评论。",
            ),
            focus: cx.focus_handle(),
            trigger_focus: cx.focus_handle(),
            last: None,
            pending_frame: false,
            cycle_ticks: 0,
            local_accumulator: 0.,
            frames: Vec::new(),
            pending: None,
            hud: String::new(),
            hud_at: None,
        };
        value.composer_subscriptions(cx);
        value.second.update(cx, |input, cx| {
            input.set_value("fixture-secret", cx);
            input.set_secret(true, cx);
        });
        value.endpoint.update(cx, |input, cx| {
            input.set_value("https://api.example.com/v1", cx)
        });
        value
    }
    fn sid(&self, key: &str) -> String {
        format!("{}-{key}", self.id)
    }
    fn poses(&self) -> (Pose, Pose) {
        let w = (self.width - 36.).max(140.) as f64;
        let rect = |x, y, w, h, r| Pose::rect(x, y, w, h, r);
        match self.kind {
            Kind::Composer => (
                liquid::composer::body(self.width, 24.),
                liquid::composer::body(self.width, self.input_height),
            ),
            Kind::Popover => {
                let compact = self.variant == 2;
                let sw = if compact {
                    24.
                } else {
                    (self.popover_label_width as f64 + 24.).min(w)
                };
                let tw = if compact { 190. } else { w.min(320.) };
                let x = if compact {
                    self.width as f64 - 18. - sw
                } else {
                    18.
                };
                (
                    rect(
                        x,
                        18.,
                        sw,
                        if compact { 24. } else { 32. },
                        if compact { 8. } else { 12. },
                    ),
                    rect(
                        if compact {
                            self.width as f64 - 18. - tw
                        } else {
                            18.
                        },
                        64.,
                        tw,
                        if compact { 124. } else { 152. },
                        crate::controls::MENU_RADIUS as f64,
                    ),
                )
            }
            Kind::Choices => (
                liquid::controls::segment_pose(w.min(280.) as f32, 3, 0),
                liquid::controls::segment_pose(w.min(280.) as f32, 3, self.selected),
            ),
            Kind::Switch => (
                liquid::controls::toggle_pose(false),
                liquid::controls::toggle_pose(self.selected == 1),
            ),
            _ => (rect(18., 18., w, 32., 12.), rect(18., 18., w, 32., 12.)),
        }
    }
    fn layout(&mut self, width: f32) {
        self.layout_cache.invalidate();
        self.width = width;
        if matches!(self.kind, Kind::Primitive(_) | Kind::Comments) {
            return;
        }
        if matches!(
            self.kind,
            Kind::Popover
                | Kind::Modal
                | Kind::Navigation
                | Kind::Rows
                | Kind::Details
                | Kind::Disclosure
                | Kind::Attachments
                    | Kind::Notice
        ) {
            return;
        }
        let (from, to) = self.poses();
        let cfg = self.config.borrow().clone();
        if self.surfaces.is_empty() {
            let options = Options {
                anchor: [0., 0.],
                capacity: (from.w * from.h).max(to.w * to.h),
                seed: cfg.seed,
                ..Options::default()
            };
            let simulation = if self.kind == Kind::Composer {
                Simulation::compound(
                    &self.composer_targets(to),
                    4.,
                    cfg.material,
                    Options {
                        anchor: [0., 1.],
                        ..options
                    },
                )
            } else {
                Simulation::new(from, cfg.material, options)
            };
            self.surfaces
                .push(Surface::new(simulation).expect("valid liquid story surface"));
            self.retarget();
            for s in &mut self.surfaces {
                s.simulation.finish();
                s.prepare();
            }
            self.progress.snap();
        } else {
            self.retarget();
        }
    }
    fn retarget(&mut self) {
        if self.surfaces.is_empty() {
            return;
        }
        let (from, to) = self.poses();
        let composer_targets = (self.kind == Kind::Composer).then(|| self.composer_targets(to));
        let s = &mut self.surfaces[0].simulation;
        if self.kind == Kind::Composer {
            s.set_compound_targets(composer_targets.as_ref().unwrap());
        } else {
            s.set_target(
                if matches!(
                    self.kind,
                    Kind::Composer | Kind::Navigation | Kind::Rows | Kind::Choices | Kind::Switch
                ) || self.open
                {
                    to
                } else {
                    from
                },
            );
        }
        self.progress.target = if self.open { 1. } else { 0. };
        self.last.get_or_insert_with(Instant::now);
    }
    fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = open;
        if self.kind == Kind::Comments {
            if open {
                let source = crate::comments::CommentSource {
                    session_id: "mock-session".into(), message_id: Some("mock-message".into()),
                    quote: self.quote.clone().unwrap_or_else(|| self.document.plain_text().to_owned()),
                    ..Default::default()
                };
                let bounds = self.comment_anchor.get();
                let focus = Some(self.trigger_focus.clone());
                self.comment_editor.update(cx, |editor, cx| editor.open_at(
                    crate::components::comments::EditorRequest { source, editing: None, text: String::new(), toolbar: false },
                    bounds, focus, window, cx));
            } else { self.comment_editor.update(cx, |editor, cx| editor.dismiss(cx)); }
            cx.notify();
            return;
        }
        if !open {
            self.pending = None;
            self.busy = false;
            if !matches!(self.kind, Kind::Popover | Kind::Modal) {
                window.focus(&self.trigger_focus, cx);
            }

        }
        self.retarget();
        cx.notify();
    }
    fn composer_targets(&self, body: Pose) -> Vec<Pose> {
        let mut poses =
            liquid::composer::poses(body, &self.composer.snapshot, &self.composer.widths);
        poses.extend(self.composer.departures.targets(body));
        poses
    }
    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.disabled {
            return;
        }
        // The gallery records an emitted intent. It has no send/save policy and
        // does not pretend to create a model connection or delivered message.
        self.actions += 1;
        self.busy = true;
        self.status = "已触发操作 · 等待样例结果".into();
        let submitted = self.input.read(cx).value().to_owned();
        self.pending = Some((Instant::now(), submitted));
        self.last.get_or_insert_with(Instant::now);
        cx.notify();
    }
    fn demo(&mut self, cx: &mut Context<Self>) {
        match self.kind {
            Kind::Navigation | Kind::Rows | Kind::Choices | Kind::Details => {
                self.selected = (self.selected + 1) % 3;
            }
            Kind::Switch => self.selected = 1 - self.selected,
            Kind::Notice => self.variant = (self.variant + 1) % 5,
            Kind::Composer => {
                self.open = !self.open;
                self.composer_intent(zork_client_types::composer::Intent::Edit(if self.open {"请检查这个实现。\n输入按内容自然增高。\n最多显示三行。\n后续内容在编辑器内滚动。"}else{"请检查这个实现。"}.into()),cx);
            }
            Kind::Actions => {
                self.selected = (self.selected + 1) % 2;
            }
            Kind::Fields => {
                self.invalid = !self.invalid;
            }
            _ => self.open = !self.open,
        }
        self.retarget();
        cx.notify();
    }
    fn component_surfaces(&self, cx: &App) -> Vec<Value> {
        match self.kind {
            Kind::Comments => vec![self.comment_editor.read(cx).inspect()["material"].clone()],
            Kind::Navigation | Kind::Rows => self.navigation.surfaces(),
            Kind::Popover => vec![self.popover.inspect()]
                .into_iter()
                .filter(|v| !v.is_null())
                .collect(),
            Kind::Modal => vec![self.dialog.inspect()]
                .into_iter()
                .filter(|v| !v.is_null())
                .collect(),
            Kind::Details
            | Kind::Disclosure
            | Kind::Attachments
            | Kind::Notice => vec![self.panel.inspect()]
                .into_iter()
                .filter(|v| !v.is_null())
                .collect(),
            _ => vec![],
        }
    }
    fn samples(&self, cx: &App) -> Vec<FrameSample> {
        match self.kind {
            Kind::Comments => self.comment_editor.read(cx).samples(),
            Kind::Popover => self.popover.samples().to_vec(),
            Kind::Modal => self.dialog.samples().to_vec(),
            Kind::Navigation | Kind::Rows => self.navigation.samples(),
            Kind::Details
            | Kind::Disclosure
            | Kind::Attachments
            | Kind::Notice => self.panel.samples().to_vec(),
            _ => self.frames.clone(),
        }
    }
    fn visible(&self, cx: &App) -> bool {
        match self.kind {
            Kind::Comments => self.comment_editor.read(cx).visible(),
            Kind::Popover => self.popover.visible(),
            Kind::Modal => self.dialog.visible(),
            Kind::Navigation | Kind::Rows => self.navigation.visible(),
            Kind::Details
            | Kind::Disclosure
            | Kind::Attachments
            | Kind::Notice => self.panel.visible(),
            _ => self.surfaces.first().is_some_and(Surface::visible),
        }
    }
    fn inspect(&self, cx: &App) -> Value {
        let mut costs = self.samples(cx).iter().map(|f| f.work_ms).collect::<Vec<_>>();
        costs.sort_by(f64::total_cmp);
        let p95 = costs
            .get((costs.len() as f64 * 0.95).ceil().max(1.) as usize - 1)
            .copied()
            .unwrap_or(0.);
        json!({"primitive":self.specimen.as_ref().map(|entity|entity.read(cx).inspect(cx)),"menuHover":self.popover.hovered(),"menuHeld":self.popover.held(),"departures":if self.kind == Kind::Composer && !self.surfaces.is_empty() {Some(self.composer.departures.inspect(&self.surfaces[0].simulation))} else {None},"departureOrigin":self.composer.departure_origin,"kind":self.kind.key(),"composer":if self.kind==Kind::Composer {Some(&self.composer.snapshot)} else {None},"inputHeight":self.input_height,"fanProgress":self.composer.fan.position,"fanPinned":self.composer.pinned,"memberPreview":self.composer.member,"hoveredMember":self.composer.hovered_member,"filePreview":self.composer.file,"id":self.id,"open":self.open,"selected":self.selected,"variant":self.variant,"input":self.input.read(cx).value(),"actions":self.actions,"busy":self.busy,"disabled":self.disabled,"invalid":self.invalid,"status":self.status,"files":self.files,"members":self.members,"drafts":self.drafts,"quote":self.quote,"frames":self.samples(cx).len(),"workP95Ms":p95,"selector":self.selector.inspect(),"overlay":if self.kind == Kind::Popover {self.popover.inspect()} else {self.dialog.inspect()},"surfaces":self.surfaces.iter().map(|s|json!({"pose":s.simulation.pose(),"particles":s.simulation.particles().count(),"moving":s.simulation.moving(),"error":s.last_error.map(|e|format!("{e:?}")),"paintError":s.paint_failed(),"visible":s.visible(),"path":s.contour().svg_path()})).chain(self.component_surfaces(cx)).collect::<Vec<_>>()})
    }
    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.kind, Kind::Primitive(_) | Kind::Comments) {
            return;
        }
        if self.kind == Kind::Composer {
            self.poll_composer_files(cx);
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|(start, _)| start.elapsed().as_millis() >= 650)
        {
            let _ = self.pending.take().unwrap();
            self.busy = false;
            self.status = format!("样例操作完成 · {} 次", self.actions);
            if self.kind == Kind::Notice {
                self.variant = 2;
                self.retarget();
            }
        }
        let cfg = self.config.borrow().clone();
        if self.seen_epoch != cfg.epoch {
            self.seen_epoch = cfg.epoch;
            for s in &mut self.surfaces {
                s.simulation.configure(cfg.material);
                s.simulation.set_seed(cfg.seed);
            }
            self.retarget();
        }
        if self.seen_reset != cfg.reset {
            self.seen_reset = cfg.reset;
            self.frames.clear();
            self.popover.reset_samples();
            self.dialog.reset_samples();
            self.panel.reset_samples();
            self.comment_editor.read(cx).reset_samples();
            self.navigation.reset_samples();
            self.cycle_ticks = 0;
        }
        let now = Instant::now();
        let elapsed = self
            .last
            .map_or(0., |last| (now - last).as_secs_f64())
            .min(0.05)
            * if cfg.slow { 0.3 } else { 1. };
        self.last = Some(now);
        let started = Instant::now();
        // Hidden cards are retained but their owners do not request more frames.
        if !self.visible(cx) && !self.samples(cx).is_empty() {
            self.last = None;
            return;
        }
        if cfg.cycle && !cx.reduce_motion() {
            self.local_accumulator += elapsed;
            while self.local_accumulator >= FIXED_DT {
                self.local_accumulator -= FIXED_DT;
                if self.cycle_ticks % 168 == 0 {
                    self.demo(cx);
                }
                self.cycle_ticks += 1;
            }
        }
        if matches!(
            self.kind,
            Kind::Popover
                | Kind::Modal
                | Kind::Navigation
                | Kind::Rows
                | Kind::Details
                | Kind::Disclosure
                | Kind::Attachments
                    | Kind::Notice
        ) {
            if ((cfg.cycle && !cx.reduce_motion()) || self.pending.is_some()) && !self.pending_frame
            {
                self.pending_frame = true;
                let weak = cx.entity().downgrade();
                window.on_next_frame(move |_, cx| {
                    let _ = weak.update(cx, |v, cx| {
                        v.pending_frame = false;
                        cx.notify();
                    });
                });
            }
            return;
        }
        if self.kind == Kind::Composer
            && self.composer.departures.advance(
                elapsed,
                cx.reduce_motion(),
                &mut self.surfaces[0].simulation,
            )
        {
            self.retarget();
        }
        for s in &mut self.surfaces {
            s.simulation.advance(elapsed, cx.reduce_motion());
        }
        if cx.reduce_motion() {
            self.progress.snap();
        } else {
            self.progress
                .step(elapsed, self.surfaces[0].simulation.omega(), 0.84);
            if self.progress.near(0.001, 0.01) {
                self.progress.snap();
            }
        }
        let physics = started.elapsed().as_secs_f64() * 1000.;
        if self.kind == Kind::Composer {
            if cx.reduce_motion() {
                self.composer.fan.snap();
            } else {
                self.composer
                    .fan
                    .step(elapsed, self.surfaces[0].simulation.omega(), 0.84);
                if self.composer.fan.near(0.001, 0.01) {
                    self.composer.fan.snap();
                }
            }
            let p = self.surfaces[0].simulation.pose();
            let cutouts = liquid::composer::opening(
                p,
                self.composer.snapshot.files.len(),
                self.composer.fan.position.clamp(0., 1.) as f32,
            )
            .map(|opening| {
                opening
                    .hole
                    .into_iter()
                    .map(|c| liquid::Cubic {
                        from: [c[0].x as f64, c[0].y as f64],
                        c1: [c[1].x as f64, c[1].y as f64],
                        c2: [c[2].x as f64, c[2].y as f64],
                        to: [c[3].x as f64, c[3].y as f64],
                    })
                    .collect()
            })
            .into_iter()
            .collect();
            self.surfaces[0].set_cutouts(cutouts);
        }
        for s in &mut self.surfaces {
            s.prepare();
        }
        if self.kind == Kind::Composer {
            self.composer
                .departures
                .prepare(&self.surfaces[0].simulation);
        }
        let work = started.elapsed().as_secs_f64() * 1000.;
        let moving = self.surfaces.iter().any(|s| s.simulation.moving())
            || self.composer.departures.active()
            || !self.progress.near(0.001, 0.01)
            || !self.composer.fan.near(0.001, 0.01);
        self.frames.push(FrameSample {
            work_ms: work,
            physics_ms: physics,
            contour_ms: work - physics,
            controls: 1,
            particles: self
                .surfaces
                .iter()
                .map(|s| s.simulation.particles().count())
                .sum(),
            moving,
            sampled: self
                .surfaces
                .iter()
                .map(|s| s.contour().sampled_points)
                .sum(),
            full_grid: self
                .surfaces
                .iter()
                .map(|s| s.contour().full_grid_points)
                .sum(),
            fallbacks: self
                .surfaces
                .iter()
                .filter(|s| s.contour().used_fallback)
                .count(),
        });
        if self.frames.len() > 3000 {
            self.frames.drain(..1000);
        }
        if cfg.cycle
            && self
                .hud_at
                .is_none_or(|t| now.duration_since(t).as_millis() >= 300)
        {
            self.hud_at = Some(now);
            let mut values = self
                .frames
                .iter()
                .rev()
                .take(120)
                .map(|f| f.work_ms)
                .collect::<Vec<_>>();
            values.sort_by(f64::total_cmp);
            let p95 = values[(values.len() as f64 * 0.95).ceil().max(1.) as usize - 1];
            self.hud = format!(
                "{} 粒 · {} 个液面 · 液面计算 P95 {:.2} ms",
                self.surfaces
                    .iter()
                    .map(|s| s.simulation.particles().count())
                    .sum::<usize>(),
                self.surfaces.len(),
                p95
            );
        }

        if (((moving || cfg.cycle) && !cx.reduce_motion())
            || self.pending.is_some()
            || self.composer.choosing
            || self.composer.member_leave.is_some())
            && !self.pending_frame
        {
            self.pending_frame = true;
            let weak = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.pending_frame = false;
                    cx.notify();
                });
            });
        } else if !moving && !cfg.cycle && self.pending.is_none() {
            self.last = None;
        }
    }
}
