//! Development preview: reach one component state in one step, then compare
//! themes and widths. Export fixtures keep their catalog ids and sizes.
use super::*;
use gpui::{div, rgb, AnyElement, ClipboardItem, ScrollHandle, SharedString};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use zork_gui::design::ZORK_UI;
use zork_ui::{
    automation::{AutomationElementExt, AutomationRole},
    components::text_input::{ComposerEdited, ComposerInput, ComposerSubmit},
    components::widgets::controls::segmented,
    controls as ui,
    design::{Theme, INTERACTION, RADIUS},
    navigation::TabGroup,
};

pub(super) const SIDEBAR_WIDTH: f32 = 272.;
const RAIL_WIDTH: f32 = 64.;
const INSPECTOR_WIDTH: f32 = 300.;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Overview,
    Single,
}

/// Which themes the canvas shows. The palette is a process-wide global, so
/// only the current app theme can be live; "both" pairs the live specimen
/// with an offscreen render of the other theme.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Light,
    Dark,
    Both,
}
impl View {
    fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Both => "both",
        }
    }
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "light" => Self::Light,
            "dark" => Self::Dark,
            "both" => Self::Both,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Width {
    Compact,
    Medium,
    Wide,
    Window,
}
impl Width {
    const ALL: [Self; 4] = [Self::Compact, Self::Medium, Self::Wide, Self::Window];
    fn pixels(self) -> Option<f32> {
        match self {
            Self::Compact => Some(360.),
            Self::Medium => Some(600.),
            Self::Wide => Some(900.),
            Self::Window => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Compact => "360",
            Self::Medium => "600",
            Self::Wide => "900",
            Self::Window => "窗口",
        }
    }
    fn key(self) -> String {
        self.pixels()
            .map_or_else(|| "window".into(), |w| format!("{w}"))
    }
    pub(super) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.key() == value)
    }
}

/// Simulated input on the specimen's primary control.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interaction {
    Rest,
    Hover,
    Focus,
}

struct Session {
    selected: usize,
    host: Entity<StoryHost>,
    scroll: ScrollHandle,
    pending: Option<Vec<Value>>,
}

#[derive(Clone)]
struct Thumb {
    path: PathBuf,
    width: f32,
    height: f32,
    issues: Vec<String>,
}

#[derive(Default)]
struct Thumbs {
    /// (story id, theme, width key) → rendered still.
    ready: HashMap<(String, &'static str, String), Thumb>,
    /// (entry, theme, width key) jobs that finished, successfully or not.
    done: std::collections::HashSet<(String, &'static str, String)>,
    running: bool,
}

pub(super) struct Gallery {
    catalog: Vec<Story>,
    selected: usize,
    mode: Mode,
    view: View,
    width: Width,
    interaction: Interaction,
    sessions: HashMap<String, Session>,
    driver: HeadlessAutomation,
    navigation: TabGroup,
    directory_scroll: ScrollHandle,
    directory_anchor: gpui::ScrollAnchor,
    reveal_initial: bool,
    sidebar_open: bool,
    inspector_open: bool,
    search: Entity<ComposerInput>,
    query: String,
    focus: gpui::FocusHandle,
    thumbs: Rc<RefCell<Thumbs>>,
    highlight: Option<zork_ui::automation::protocol::Rect>,
    revision: String,
    /// Offscreen stills need the built binary; tests render without them.
    stills: bool,
    notice: Option<String>,
}

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cache_root() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Library/Caches/zork-design-pc")
}

/// Rebuilding the binary invalidates every cached still.
fn build_stamp() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::metadata(exe).ok())
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or_else(|| "dev".into(), |d| d.as_secs().to_string())
}

fn theme_key(theme: Theme) -> &'static str {
    match theme {
        Theme::Light => "light",
        Theme::Dark => "dark",
    }
}

/// Issues that can be read from an automation snapshot. Controls must offer a
/// 24 px target and stay fully visible across their width.
pub(super) fn checks(elements: &[Value]) -> Vec<String> {
    let mut issues = vec![];
    for element in elements {
        let role = element["role"].as_str().unwrap_or_default();
        if !matches!(role, "button" | "link" | "option" | "text_input")
            || element["visible"] != true
        {
            continue;
        }
        let id = element["id"].as_str().unwrap_or_default();
        let bounds = &element["bounds"];
        let visible = &element["visible_bounds"];
        let (w, h) = (
            bounds["width"].as_f64().unwrap_or(0.),
            bounds["height"].as_f64().unwrap_or(0.),
        );
        if w < 23.5 || h < 23.5 {
            issues.push(format!("点击区域小于 24 px：{id}（{w:.0} × {h:.0}）"));
        }
        let shown = visible["width"].as_f64().unwrap_or(0.);
        if shown > 0. && shown + 1. < w {
            issues.push(format!("横向被裁切：{id}"));
        }
    }
    issues
}

impl Gallery {
    pub(super) fn new(
        catalog: Vec<Story>,
        selected: usize,
        driver: HeadlessAutomation,
        _fixed_size: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| ComposerInput::new("搜索组件或状态", cx).single_line());
        cx.subscribe(&search, |v, input, _: &ComposerEdited, cx| {
            v.query = input.read(cx).value().trim().to_owned();
            cx.notify();
        })
        .detach();
        cx.subscribe(&search, |v, _, _: &ComposerSubmit, cx| {
            if let Some(&index) = v.matches().first() {
                v.clear_search(cx);
                v.select_story(index, Mode::Single, cx);
            }
        })
        .detach();
        let revision = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repository())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_else(|| "dev".into());
        let directory_scroll = ScrollHandle::new();
        let mut gallery = Self {
            catalog,
            selected,
            mode: Mode::Single,
            view: View::Both,
            width: Width::Window,
            interaction: Interaction::Rest,
            sessions: HashMap::new(),
            driver,
            navigation: TabGroup::new(cx),
            directory_anchor: gpui::ScrollAnchor::for_handle(directory_scroll.clone()),
            directory_scroll,
            reveal_initial: true,
            sidebar_open: true,
            inspector_open: true,
            search,
            query: String::new(),
            focus: cx.focus_handle(),
            thumbs: Default::default(),
            highlight: None,
            revision,
            stills: false,
            notice: None,
        };
        gallery.create_session(selected, None, cx);
        gallery
    }

    /// The interactive app renders stills with its own binary in the background.
    pub(super) fn enable_stills(&mut self) {
        self.stills = true;
    }

    pub(super) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub(super) fn set_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.view = view;
        match view {
            View::Light => zork_ui::design::prefer_theme(Some(Theme::Light), cx),
            View::Dark => zork_ui::design::prefer_theme(Some(Theme::Dark), cx),
            View::Both => {}
        }
        self.persist();
        cx.notify();
    }

    pub(super) fn set_width(&mut self, width: Width) {
        self.width = width;
    }

    fn story(&self) -> &Story {
        &self.catalog[self.selected]
    }

    fn session(&self) -> &Session {
        &self.sessions[&self.story().entry]
    }

    fn states(&self, entry: &str) -> Vec<usize> {
        (0..self.catalog.len())
            .filter(|&i| self.catalog[i].entry == entry)
            .collect()
    }

    fn entries(&self) -> Vec<usize> {
        let mut seen = std::collections::HashSet::new();
        (0..self.catalog.len())
            .filter(|&i| seen.insert(self.catalog[i].entry.clone()))
            .collect()
    }

    fn create_session(&mut self, selected: usize, extra: Option<Vec<Value>>, cx: &mut Context<Self>) {
        let story = self.catalog[selected].clone();
        let mut actions = story.actions.clone();
        actions.extend(extra.unwrap_or_default());
        let host = cx.new(|cx| StoryHost::new(story.clone(), cx));
        self.sessions.insert(
            story.entry,
            Session {
                selected,
                host,
                scroll: ScrollHandle::new(),
                pending: Some(actions),
            },
        );
    }

    /// Opens a story; an existing specimen of the same state keeps its edits.
    fn select_story(&mut self, index: usize, mode: Mode, cx: &mut Context<Self>) {
        let entry = self.catalog[index].entry.clone();
        let reuse = self.sessions.get(&entry).is_some_and(|s| s.selected == index);
        if !reuse {
            self.interaction = Interaction::Rest;
            self.create_session(index, None, cx);
        }
        self.selected = index;
        self.mode = mode;
        self.highlight = None;
        self.persist();
        cx.notify();
    }

    fn select_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let entry = self.catalog[index].entry.clone();
        let index = self.sessions.get(&entry).map_or(index, |s| s.selected);
        let reuse = self.sessions.contains_key(&entry);
        if !reuse {
            self.create_session(index, None, cx);
        }
        self.selected = index;
        self.highlight = None;
        self.persist();
        cx.notify();
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.interaction = Interaction::Rest;
        self.create_session(self.selected, None, cx);
        cx.notify();
    }

    fn step_state(&mut self, delta: isize, cx: &mut Context<Self>) {
        let states = self.states(&self.story().entry.clone());
        let at = states.iter().position(|&i| i == self.selected).unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(states.len() as isize) as usize;
        self.select_story(states[next], self.mode, cx);
    }

    fn step_entry(&mut self, delta: isize, cx: &mut Context<Self>) {
        let entries = self.entries();
        let current = &self.story().entry;
        let at = entries
            .iter()
            .position(|&i| &self.catalog[i].entry == current)
            .unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(entries.len() as isize) as usize;
        self.select_entry(entries[next], cx);
    }

    /// Fuzzy match: every query character appears in order in
    /// "component / state / id".
    fn matches(&self) -> Vec<usize> {
        let query: Vec<char> = self
            .query
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '/')
            .collect();
        if query.is_empty() {
            return vec![];
        }
        (0..self.catalog.len())
            .filter(|&i| {
                let story = &self.catalog[i];
                let haystack =
                    format!("{}{}{}", story.entry_title, story.label, story.id).to_lowercase();
                let mut chars = haystack.chars();
                query.iter().all(|q| chars.any(|c| c == *q))
            })
            .take(12)
            .collect()
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.search.update(cx, |input, cx| input.set_value("", cx));
    }

    fn interactive(&self) -> bool {
        matches!(
            self.story().family.as_str(),
            "button" | "field" | "switch" | "navigation" | "choice" | "dropdown"
        )
    }

    fn disabled_state(&self) -> Option<usize> {
        let story = self.story();
        self.states(&story.entry)
            .into_iter()
            .find(|&i| self.catalog[i].state.starts_with("disabled"))
    }

    fn apply_interaction(&mut self, interaction: Interaction, cx: &mut Context<Self>) {
        self.interaction = interaction;
        let target = self.story().target.clone();
        let extra = match interaction {
            Interaction::Rest => vec![],
            Interaction::Hover => {
                vec![json!({"type":"move","target":{"element_id":target}})]
            }
            Interaction::Focus if self.story().family == "field" => {
                vec![json!({"type":"click","target":{"element_id":"story-field"}})]
            }
            Interaction::Focus => vec![json!({"type":"key","keystroke":"tab"})],
        };
        self.create_session(self.selected, Some(extra), cx);
        cx.notify();
    }

    fn canvas_width(&self) -> Option<f32> {
        self.width.pixels()
    }

    fn current_theme() -> Theme {
        zork_ui::design::theme()
    }

    /// Themes the overview and side-by-side canvas need as stills.
    fn still_themes(&self) -> Vec<&'static str> {
        match self.view {
            View::Light => vec!["light"],
            View::Dark => vec!["dark"],
            View::Both => vec!["light", "dark"],
        }
    }

    fn queue_stills(&mut self, cx: &mut Context<Self>) {
        if !self.stills || self.thumbs.borrow().running {
            return;
        }
        let entry = self.story().entry.clone();
        let width = self.width.key();
        let themes: Vec<&'static str> = if self.mode == Mode::Overview {
            self.still_themes()
        } else if self.view == View::Both {
            let live = theme_key(Self::current_theme());
            vec![if live == "light" { "dark" } else { "light" }]
        } else {
            vec![]
        };
        let Some(theme) = themes.into_iter().find(|theme| {
            !self
                .thumbs
                .borrow()
                .done
                .contains(&(entry.clone(), *theme, width.clone()))
        }) else {
            return;
        };
        self.thumbs.borrow_mut().running = true;
        let output = cache_root()
            .join("stills")
            .join(build_stamp())
            .join(format!("{theme}-{width}-{entry}"));
        let mut command = std::process::Command::new(
            std::env::current_exe().unwrap_or_else(|_| "zork-design-pc".into()),
        );
        command
            .arg("--export")
            .arg(&output)
            .arg("--entry")
            .arg(&entry)
            .arg("--partial")
            .env("ZORK_THEME", theme);
        if let Some(px) = self.width.pixels() {
            command.arg("--width").arg(px.to_string());
        }
        let job = (entry, theme, width);
        let stories: Vec<String> = self
            .states(&job.0)
            .into_iter()
            .map(|i| self.catalog[i].id.clone())
            .collect();
        let task = cx
            .background_executor()
            .spawn(async move { command.output() });
        cx.spawn(async move |this, cx| {
            let _ = task.await;
            let _ = this.update(cx, |v, cx| {
                let mut thumbs = v.thumbs.borrow_mut();
                for id in stories {
                    let image = output.join("native").join(format!("{id}.png"));
                    let Ok((w, h)) = image::image_dimensions(&image) else {
                        continue;
                    };
                    let geometry: Value = std::fs::read(output.join("native").join(format!("{id}.json")))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                        .unwrap_or_default();
                    let scale = geometry["scale_factor"].as_f64().unwrap_or(2.).max(1.) as f32;
                    let issues = geometry["elements"]
                        .as_array()
                        .map(|elements| checks(elements))
                        .unwrap_or_default();
                    thumbs.ready.insert(
                        (id, job.1, job.2.clone()),
                        Thumb {
                            path: image,
                            width: w as f32 / scale,
                            height: h as f32 / scale,
                            issues,
                        },
                    );
                }
                thumbs.done.insert(job);
                thumbs.running = false;
                drop(thumbs);
                cx.notify();
            });
        })
        .detach();
    }

    fn still(&self, id: &str, theme: &'static str) -> Option<Thumb> {
        self.thumbs
            .borrow()
            .ready
            .get(&(id.to_owned(), theme, self.width.key()))
            .cloned()
    }

    fn command(&self) -> String {
        let mut command = format!("zork-design-pc --story {}", self.story().id);
        match self.view {
            View::Light => command.push_str(" --theme light"),
            View::Dark => command.push_str(" --theme dark"),
            View::Both => {}
        }
        if let Some(px) = self.width.pixels() {
            command.push_str(&format!(" --width {px}"));
        }
        command
    }

    fn open_source(&self) {
        let path = repository().join(&self.story().source);
        let path = if path.exists() {
            path
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(path)
        };
        let _ = std::process::Command::new("open").arg("-t").arg(path).spawn();
    }

    fn export_png(&mut self, cx: &mut Context<Self>) {
        let story = self.story().id.clone();
        let theme = theme_key(Self::current_theme());
        let output = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Downloads/zork-design")
            .join(format!("{story}-{theme}-{}", self.width.key()));
        let mut command = std::process::Command::new(
            std::env::current_exe().unwrap_or_else(|_| "zork-design-pc".into()),
        );
        command
            .arg("--export")
            .arg(&output)
            .arg("--story")
            .arg(&story)
            .env("ZORK_THEME", theme);
        if let Some(px) = self.width.pixels() {
            command.arg("--width").arg(px.to_string());
        }
        self.notice = Some("正在导出…".into());
        let task = cx.background_executor().spawn(async move {
            let ok = command.output().is_ok_and(|o| o.status.success());
            (ok, output.join("native").join(format!("{story}.png")))
        });
        cx.spawn(async move |this, cx| {
            let (ok, image) = task.await;
            if ok {
                let _ = std::process::Command::new("open").arg("-R").arg(&image).spawn();
            }
            let _ = this.update(cx, |v, cx| {
                v.notice = Some(if ok { "已导出到 下载/zork-design" } else { "导出失败" }.into());
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn persist(&self) {
        let state = json!({
            "story": self.story().id,
            "mode": if self.mode == Mode::Overview { "overview" } else { "single" },
            "view": self.view.key(),
            "width": self.width.key(),
            "sidebar": self.sidebar_open,
            "inspector": self.inspector_open,
        });
        let path = cache_root().join("state.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_vec_pretty(&state).unwrap_or_default());
    }

    /// Last story, mode, theme view and width; launch flags override it.
    pub(super) fn restore() -> Value {
        std::fs::read(cache_root().join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub(super) fn apply_restored(&mut self, state: &Value, cx: &mut Context<Self>) {
        if let Some(mode) = state["mode"].as_str() {
            self.mode = if mode == "overview" { Mode::Overview } else { Mode::Single };
        }
        if let Some(width) = state["width"].as_str().and_then(Width::parse) {
            self.width = width;
        }
        if let Some(open) = state["sidebar"].as_bool() {
            self.sidebar_open = open;
        }
        if let Some(open) = state["inspector"].as_bool() {
            self.inspector_open = open;
        }
        if let Some(view) = state["view"].as_str().and_then(View::parse) {
            self.set_view(view, cx);
        }
    }

    fn key_down(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if event.keystroke.modifiers.platform && key == "k" {
            self.sidebar_open = true;
            let handle = self.search.read(cx).focus_handle();
            window.focus(&handle, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" && !self.query.is_empty() {
            self.clear_search(cx);
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        // Arrow keys belong to the specimen unless the browser itself has focus.
        if !self.focus.is_focused(window) {
            return;
        }
        match key {
            "up" => self.step_entry(-1, cx),
            "down" => self.step_entry(1, cx),
            "left" => self.step_state(-1, cx),
            "right" => self.step_state(1, cx),
            "enter" if self.mode == Mode::Overview => {
                self.mode = Mode::Single;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = ZORK_UI.palette;
        if !self.sidebar_open {
            return div()
                .id("design-rail")
                .w(px(RAIL_WIDTH))
                .h_full()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .pt(px(44.))
                .bg(rgb(p.sidebar))
                .border_r(px(zork_ui::design::BORDER_WIDTH))
                .border_color(rgb(p.border))
                .child(
                    ui::icon_button("design-sidebar-open", true)
                        .child(ui::icon("icons/panel-left.svg", 16.))
                        .aria_label("展开侧栏")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.sidebar_open = true;
                            v.persist();
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, "展开侧栏"),
                )
                .child(
                    ui::icon_button("design-search-open", true)
                        .child(ui::icon("icons/search.svg", 16.))
                        .aria_label("搜索")
                        .on_click(cx.listener(|v, _, window, cx| {
                            v.sidebar_open = true;
                            let handle = v.search.read(cx).focus_handle();
                            window.focus(&handle, cx);
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, "搜索"),
                )
                .into_any_element();
        }
        let current = self.story().entry.clone();
        let mut list = self.navigation.column();
        let matches = self.matches();
        if !self.query.is_empty() {
            let mut section = self
                .navigation
                .section("design-search-results", format!("{} 个结果", matches.len()));
            for index in matches {
                let story = &self.catalog[index];
                section = section.child(
                    self.navigation
                        .tab(format!("design-result-{}", story.id), false)
                        .aria_label(format!("{} / {}", story.entry_title, story.label))
                        .child(
                            div()
                                .flex()
                                .gap_1()
                                .min_w_0()
                                .child(div().text_color(rgb(p.muted)).child(format!("{} /", story.entry_title)))
                                .child(story.label.clone()),
                        )
                        .on_click(cx.listener(move |v, _, window, cx| {
                            v.clear_search(cx);
                            window.focus(&v.focus, cx);
                            v.select_story(index, Mode::Single, cx);
                        }))
                        .automation(AutomationRole::Button, format!("{} / {}", story.entry_title, story.label)),
                );
            }
            list = list.child(section);
        } else {
            for group in stories::GROUPS {
                let entries: Vec<usize> = self
                    .entries()
                    .into_iter()
                    .filter(|&i| self.catalog[i].group == group)
                    .collect();
                if entries.is_empty() {
                    continue;
                }
                let mut section = self.navigation.section(format!("design-group-{group}"), group).mt_3();
                for index in entries {
                    let story = &self.catalog[index];
                    let open = story.entry == current;
                    let states = self.states(&story.entry);
                    section = section.child(
                        self.navigation
                            .tab(format!("design-entry-{}", story.entry), open && self.mode == Mode::Overview)
                            .aria_label(story.entry_title.clone())
                            .when(open, |v| v.anchor_scroll(Some(self.directory_anchor.clone())))
                            .child(div().flex_1().min_w_0().child(story.entry_title.clone()))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.subtle))
                                    .child(states.len().to_string()),
                            )
                            .on_click(cx.listener(move |v, _, window, cx| {
                                window.focus(&v.focus, cx);
                                v.select_entry(index, cx);
                                v.mode = Mode::Overview;
                                v.persist();
                            }))
                            .automation(AutomationRole::Button, story.entry_title.clone()),
                    );
                    if open {
                        for state in states {
                            let item = &self.catalog[state];
                            let selected = state == self.selected && self.mode == Mode::Single;
                            section = section.child(
                                self.navigation
                                    .tab(format!("design-state-{}", item.id), selected)
                                    .aria_label(item.label.clone())
                                    .pl(px(32.))
                                    .h(px(28.))
                                    .text_size(px(12.5))
                                    .child(
                                        div()
                                            .size(px(6.))
                                            .rounded_full()
                                            .flex_shrink_0()
                                            .bg(rgb(if selected { p.text } else { p.border_strong })),
                                    )
                                    .child(item.label.clone())
                                    .on_click(cx.listener(move |v, _, window, cx| {
                                        window.focus(&v.focus, cx);
                                        v.select_story(state, Mode::Single, cx);
                                    }))
                                    .automation(AutomationRole::Button, item.label.clone()),
                            );
                        }
                    }
                }
                list = list.child(section);
            }
        }
        let search = ui::input_control("design-search", &self.search, false, cx)
            .automation(AutomationRole::TextInput, "搜索组件或状态");
        div()
            .id("story-navigation")
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(p.sidebar))
            .border_r(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(p.border))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .justify_end()
                    .px_2()
                    .child(
                        ui::icon_button_sized("design-sidebar-close", true, ui::IconButtonSize::Compact)
                            .child(ui::icon("icons/panel-left.svg", 16.))
                            .aria_label("收起侧栏")
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.sidebar_open = false;
                                v.persist();
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "收起侧栏"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px(px(20.))
                    .pt_1()
                    .pb_3()
                    .child(gpui::img("brand/mark.svg").size(px(20.)).flex_shrink_0())
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Zork Design"),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(12.))
                            .text_color(rgb(p.subtle))
                            .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                            .child(self.revision.clone()),
                    ),
            )
            .child(div().px_3().pb_2().child(search))
            .child(
                div()
                    .id("design-directory")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.directory_scroll)
                    .px_3()
                    .pb_3()
                    .child(list)
                    .automation(AutomationRole::ScrollArea, "组件目录"),
            )
            .automation(AutomationRole::ScrollArea, "组件导航")
            .into_any_element()
    }

    fn header(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = ZORK_UI.palette;
        let story = self.story();
        let count = self.states(&story.entry).len();
        let crumb = div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .text_size(px(14.))
            .child(div().text_color(rgb(p.muted)).child(format!("{} /", story.group)))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(if self.mode == Mode::Single {
                        format!("{} / {}", story.entry_title, story.label)
                    } else {
                        story.entry_title.clone()
                    }),
            )
            .child(div().ml_2().text_size(px(12.)).text_color(rgb(p.subtle)).child(
                if self.mode == Mode::Single {
                    let states = self.states(&story.entry);
                    let at = states.iter().position(|&i| i == self.selected).unwrap_or(0);
                    format!("{} / {count}", at + 1)
                } else {
                    format!("{count} 个状态")
                },
            ));
        let mode = segmented(
            "design-mode",
            150.,
            vec![
                ("design-mode-overview".into(), "总览".into()),
                ("design-mode-single".into(), "单个状态".into()),
            ],
            usize::from(self.mode == Mode::Single),
            true,
            p.canvas,
            window,
            cx,
            |v, index, cx| {
                v.mode = if index == 0 { Mode::Overview } else { Mode::Single };
                v.persist();
                cx.notify();
            },
        );
        let view = segmented(
            "design-view",
            168.,
            vec![
                ("design-view-light".into(), "浅色".into()),
                ("design-view-dark".into(), "深色".into()),
                ("design-view-both".into(), "并排".into()),
            ],
            match self.view {
                View::Light => 0,
                View::Dark => 1,
                View::Both => 2,
            },
            true,
            p.canvas,
            window,
            cx,
            |v, index, cx| {
                v.set_view([View::Light, View::Dark, View::Both][index], cx);
            },
        );
        let width = segmented(
            "design-width",
            220.,
            Width::ALL
                .into_iter()
                .map(|w| (format!("design-width-{}", w.key()), w.label().into()))
                .collect(),
            Width::ALL.iter().position(|w| *w == self.width).unwrap_or(3),
            true,
            p.canvas,
            window,
            cx,
            |v, index, cx| {
                v.width = Width::ALL[index];
                v.persist();
                cx.notify();
            },
        );
        div()
            .h(px(52.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(20.))
            .pr_3()
            .border_b(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(p.border))
            .child(crumb.flex_1())
            .child(mode)
            .child(view)
            .child(width)
            .child(
                ui::icon_button("story-reset", true)
                    .child(ui::icon("icons/reload.svg", 16.))
                    .aria_label("重置当前状态")
                    .on_click(cx.listener(|v, _, _, cx| v.reset(cx)))
                    .automation(AutomationRole::Button, "重置当前状态"),
            )
            .child(
                ui::icon_button("design-inspector-toggle", true)
                    .child(ui::icon("icons/panel-right.svg", 16.))
                    .aria_label(if self.inspector_open { "收起检查面板" } else { "展开检查面板" })
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.inspector_open = !v.inspector_open;
                        v.persist();
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "检查面板"),
            )
            .into_any_element()
    }

    fn still_frame(&self, thumb: Option<Thumb>, width: f32, height: f32) -> AnyElement {
        let p = ZORK_UI.palette;
        match thumb {
            Some(thumb) => {
                let scale = (width / thumb.width).min(height / thumb.height).min(1.);
                gpui::img(thumb.path)
                    .w(px(thumb.width * scale))
                    .h(px(thumb.height * scale))
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element()
            }
            None => div()
                .text_size(px(12.))
                .text_color(rgb(p.subtle))
                .child(if self.stills { "正在渲染…" } else { "未生成" })
                .into_any_element(),
        }
    }

    fn overview(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = ZORK_UI.palette;
        let themes = self.still_themes();
        let tiles = self.states(&self.story().entry).into_iter().map(|index| {
            let story = &self.catalog[index];
            let halves = themes.iter().map(|theme| {
                div()
                    .flex_1()
                    .h_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .bg(rgb(p.window))
                    .child(self.still_frame(
                        self.still(&story.id, theme),
                        if themes.len() == 2 { 140. } else { 280. },
                        150.,
                    ))
            });
            let issues = self
                .still(&story.id, themes[0])
                .map(|t| t.issues.len())
                .unwrap_or(0);
            let selected = index == self.selected;
            div()
                .id(format!("design-tile-{}", story.id))
                .w(px(300.))
                .flex_shrink_0()
                .rounded(px(RADIUS.container))
                .overflow_hidden()
                .border(px(if selected { 2. } else { zork_ui::design::BORDER_WIDTH }))
                .border_color(rgb(if selected { p.text } else { p.border }))
                .bg(rgb(p.canvas))
                .cursor_pointer()
                .child(div().h(px(166.)).flex().children(halves))
                .child(
                    div()
                        .h(px(40.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px(px(14.))
                        .border_t(px(zork_ui::design::BORDER_WIDTH))
                        .border_color(rgb(p.border))
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(div().flex_1().min_w_0().child(story.label.clone()))
                        .when(issues > 0, |v| {
                            v.child(
                                div()
                                    .px_2()
                                    .rounded_full()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.warning))
                                    .bg(gpui::Hsla::from(rgb(p.warning)).opacity(0.12))
                                    .child(format!("{issues} 项检查")),
                            )
                        }),
                )
                .on_click(cx.listener(move |v, _, window, cx| {
                    window.focus(&v.focus, cx);
                    v.select_story(index, Mode::Single, cx);
                }))
                .automation(AutomationRole::Button, story.label.clone())
        });
        div()
            .id("design-overview")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(20.))
            .bg(rgb(p.window))
            .child(div().flex().flex_wrap().gap_4().items_start().children(tiles))
            .automation(AutomationRole::ScrollArea, "全部状态")
            .into_any_element()
    }

    fn stage(&self) -> AnyElement {
        let p = ZORK_UI.palette;
        let story = self.story();
        let session = self.session();
        let live_theme = theme_key(Self::current_theme());
        let caption = |text: String| {
            div()
                .text_size(px(12.))
                .text_color(rgb(p.subtle))
                .mb_2()
                .child(text)
        };
        let size_text = self
            .canvas_width()
            .map_or_else(|| "窗口".to_owned(), |w| format!("{w:.0} × {:.0}", story.height));
        let live = div()
            .id("story-canvas")
            .when_some(self.canvas_width(), |v, w| v.w(px(w)).h(px(story.height)))
            .when(self.canvas_width().is_none(), |v| v.w_full().h_full())
            .flex_shrink_0()
            .rounded(px(14.))
            .overflow_hidden()
            .border(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(p.border_strong))
            .child(session.host.clone())
            .automation(AutomationRole::Status, "当前组件画布");
        let live_label = if live_theme == "light" { "浅色" } else { "深色" };
        let mut frames = div()
            .flex()
            .gap(px(20.))
            .items_start()
            .when(self.canvas_width().is_none(), |v| v.size_full())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(self.canvas_width().is_none(), |v| v.flex_1().h_full())
                    .child(caption(format!("{live_label} · {size_text} · 实时")))
                    .child(live),
            );
        if self.view == View::Both {
            let other = if live_theme == "light" { "dark" } else { "light" };
            let label = if other == "light" { "浅色" } else { "深色" };
            frames = frames.child(
                div()
                    .flex()
                    .flex_col()
                    .child(caption(format!("{label} · 快照")))
                    .child(
                        div()
                            .min_w(px(self.canvas_width().unwrap_or(400.)))
                            .min_h(px(120.))
                            .flex()
                            .items_start()
                            .justify_center()
                            .child(self.still_frame(self.still(&story.id, other), 1600., 1600.)),
                    ),
            );
        }
        div()
            .id("story-preview-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_scroll()
            .track_scroll(&session.scroll)
            .p(px(24.))
            .bg(rgb(p.window))
            .child(frames)
            .into_any_element()
    }

    fn inspector(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = ZORK_UI.palette;
        let story = self.story().clone();
        let section = |title: &str| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px(px(18.))
                .py(px(16.))
                .border_b(px(zork_ui::design::BORDER_WIDTH))
                .border_color(rgb(p.border))
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(rgb(p.subtle))
                        .child(title.to_owned()),
                )
        };
        let meta = |key: &str, value: String| {
            div()
                .flex()
                .gap_2()
                .text_size(px(12.5))
                .child(div().w(px(56.)).flex_shrink_0().text_color(rgb(p.muted)).child(key.to_owned()))
                .child(div().min_w_0().child(value))
        };
        let snapshot = self.driver.snapshot(false);
        let canvas = snapshot
            .elements
            .iter()
            .find(|e| e.id == "story-component")
            .map(|e| e.visible_bounds);
        let inside = |b: &zork_ui::automation::protocol::Rect| {
            canvas.is_some_and(|c| {
                let (x, y) = (b.x + b.width / 2., b.y + b.height / 2.);
                x >= c.x && x <= c.x + c.width && y >= c.y && y <= c.y + c.height
            })
        };
        let elements: Vec<_> = snapshot
            .elements
            .iter()
            .filter(|e| e.visible && e.id != "story-component" && inside(&e.bounds))
            .filter(|e| {
                matches!(
                    e.role,
                    AutomationRole::Button | AutomationRole::Link | AutomationRole::Option | AutomationRole::TextInput
                )
            })
            .cloned()
            .collect();
        let values: Vec<Value> = elements
            .iter()
            .filter_map(|e| serde_json::to_value(e).ok())
            .collect();
        let issues = checks(&values);
        let mut body = div().flex().flex_col();
        body = body.child(
            section("状态")
                .child(meta("状态", story.label.clone()))
                .child(meta("id", story.id.clone()))
                .child(meta("默认尺寸", format!("{:.0} × {:.0}", story.width, story.height)))
                .when_some(story.wide, |v, [w, h]| v.child(meta("宽版", format!("{w:.0} × {h:.0}")))),
        );
        if self.interactive() {
            let mut options = vec![
                ("design-interaction-rest".to_owned(), SharedString::from("静止")),
                ("design-interaction-hover".into(), "悬停".into()),
                ("design-interaction-focus".into(), "焦点".into()),
            ];
            let disabled = self.disabled_state();
            if disabled.is_some() {
                options.push(("design-interaction-disabled".into(), "禁用".into()));
            }
            let selected = if self.story().state.starts_with("disabled") {
                3
            } else {
                match self.interaction {
                    Interaction::Rest => 0,
                    Interaction::Hover => 1,
                    Interaction::Focus => 2,
                }
            };
            body = body.child(section("交互状态").child(segmented(
                "design-interaction",
                INSPECTOR_WIDTH - 36.,
                options,
                selected,
                true,
                p.canvas,
                window,
                cx,
                move |v, index, cx| match index {
                    0 => v.apply_interaction(Interaction::Rest, cx),
                    1 => v.apply_interaction(Interaction::Hover, cx),
                    2 => v.apply_interaction(Interaction::Focus, cx),
                    _ => {
                        if let Some(state) = disabled {
                            v.select_story(state, Mode::Single, cx);
                        }
                    }
                },
            )));
        }
        let mut check = section("检查");
        if issues.is_empty() {
            check = check.child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(p.success))
                    .child("点击区域 ≥ 24 px，控件未被裁切"),
            );
        } else {
            for issue in issues {
                check = check.child(
                    div()
                        .text_size(px(12.5))
                        .text_color(rgb(p.warning))
                        .child(issue),
                );
            }
        }
        body = body.child(check);
        let mut list = section("元素");
        for element in elements.iter().take(40) {
            let bounds = element.bounds;
            let hovered = self.highlight == Some(bounds);
            list = list.child(
                div()
                    .id(format!("design-element-{}", element.id))
                    .flex()
                    .gap_2()
                    .px_2()
                    .mx(px(-8.))
                    .h(px(24.))
                    .items_center()
                    .rounded(px(RADIUS.inline))
                    .when(hovered, |v| v.bg(rgb(INTERACTION.neutral_hover)))
                    .text_size(px(12.))
                    .font_family(zork_ui::assets::CODE_FONT_FAMILY)
                    .child(div().flex_1().min_w_0().truncate().child(element.id.clone()))
                    .child(
                        div()
                            .text_color(rgb(p.subtle))
                            .child(format!("{:.0} × {:.0}", bounds.width, bounds.height)),
                    )
                    .on_hover(cx.listener(move |v, inside: &bool, _, cx| {
                        v.highlight = inside.then_some(bounds);
                        cx.notify();
                    })),
            );
        }
        if elements.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(p.subtle))
                    .child("没有可交互元素"),
            );
        }
        body = body.child(list);
        let source = story
            .source
            .rsplit('/')
            .next()
            .unwrap_or(&story.source)
            .to_owned();
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px(px(18.))
                .py(px(16.))
                .child(
                    ui::button("design-open-source", format!("在编辑器中打开 {source}"), false, true)
                        .on_click(cx.listener(|v, _, _, _| v.open_source()))
                        .automation(AutomationRole::Button, "在编辑器中打开"),
                )
                .child(
                    ui::button("design-copy-command", "复制命令", false, true)
                        .on_click(cx.listener(|v, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(v.command()));
                            v.notice = Some("已复制命令".into());
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, "复制命令"),
                )
                .child(
                    ui::button("design-export", "导出 PNG", false, self.stills)
                        .on_click(cx.listener(|v, _, _, cx| v.export_png(cx)))
                        .automation(AutomationRole::Button, "导出 PNG"),
                )
                .when_some(self.notice.clone(), |v, notice| {
                    v.child(div().text_size(px(12.)).text_color(rgb(p.muted)).child(notice))
                }),
        );
        div()
            .id("design-inspector")
            .w(px(INSPECTOR_WIDTH))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_l(px(zork_ui::design::BORDER_WIDTH))
            .border_color(rgb(p.border))
            .bg(rgb(p.canvas))
            .child(body)
            .automation(AutomationRole::ScrollArea, "检查面板")
            .into_any_element()
    }

    /// Replays the story's gestures once the specimen has painted.
    fn pending_actions(&mut self, window: &mut Window, _cx: &mut Context<Self>) {
        let entry = self.story().entry.clone();
        let Some(actions) = self.sessions.get_mut(&entry).and_then(|s| s.pending.take()) else {
            return;
        };
        if actions.is_empty() {
            return;
        }
        let host = self.session().host.clone();
        let driver = self.driver.clone();
        window.on_next_frame(move |window, cx| {
            for action in actions {
                if let Ok(input) = serde_json::from_value(action.clone()) {
                    let _ = driver.dispatch(input, window, cx);
                }
                if action["type"] == "key" && action["keystroke"] == "tab" {
                    if let Some(focus) = host.read(cx).specimen_focus(cx) {
                        window.focus(&focus, cx);
                    }
                }
            }
        });
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reveal_initial) {
            // A launch below the fold scrolls the sidebar to its component.
            let owner = cx.entity().downgrade();
            let anchor = self.directory_anchor.clone();
            let id = format!("design-entry-{}", self.story().entry);
            window.on_next_frame(move |window, cx| {
                let visible = owner
                    .read_with(cx, |v, _| {
                        v.driver.snapshot(false).elements.iter().any(|e| {
                            e.id == id && e.visible && e.visible_bounds.height >= e.bounds.height
                        })
                    })
                    .unwrap_or(true);
                if !visible {
                    anchor.scroll_to(window, cx);
                    window.on_next_frame(move |_, cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            });
        }
        if self.mode == Mode::Single {
            self.pending_actions(window, cx);
        }
        self.queue_stills(cx);
        let p = ZORK_UI.palette;
        let sidebar = self.sidebar(cx);
        let header = self.header(window, cx);
        let canvas = if self.mode == Mode::Overview {
            self.overview(cx)
        } else {
            self.stage()
        };
        let main = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(canvas)
                    .when(self.inspector_open && self.mode == Mode::Single, |v| {
                        v.child(self.inspector(window, cx))
                    }),
            );
        let highlight = self.highlight.map(|b| {
            div()
                .absolute()
                .left(px(b.x - 2.))
                .top(px(b.y - 2.))
                .w(px(b.width + 4.))
                .h(px(b.height + 4.))
                .rounded(px(6.))
                .border_2()
                .border_color(rgb(INTERACTION.focus_ring))
        });
        div()
            .id("zork-design-pc")
            .relative()
            .size_full()
            .flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .font(zork_ui::components::workbench::font())
            .text_size(px(13.))
            .text_color(rgb(p.text))
            .bg(rgb(p.canvas))
            .child(sidebar)
            .child(main)
            .children(highlight)
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
