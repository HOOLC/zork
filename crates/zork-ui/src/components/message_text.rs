//! Bounded reuse of text layout across scroll frames and offscreen revisits.
use super::SelectionContext;
use gpui::{
    AnyElement, AnyWindowHandle, App, Bounds, Element, GlobalElementId, HighlightStyle,
    InspectorElementId, IntoElement, LayoutId, Pixels, RenderOnce, SharedString, StyledText,
    TextStyle, TextSystem, Window,
};
use std::{
    cell::RefCell,
    fmt,
    ops::Range,
    rc::{Rc, Weak},
    sync::{Arc, Weak as SyncWeak},
};

// Box the state so an evicted entry's weak handle retains only a small control block.
type Highlights = Vec<(Range<usize>, HighlightStyle)>;
type Fonts = Vec<(Range<usize>, SharedString)>;
const MAX_BYTES: usize = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 1024;

#[derive(Default)]
pub(super) struct TextCache(Rc<RefCell<Weak<RefCell<Box<State>>>>>);
impl Clone for TextCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}
impl fmt::Debug for TextCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TextCache")
    }
}
struct State {
    id: String,
    text: SharedString,
    highlights: Highlights,
    fonts: Fonts,
    style: TextStyle,
    runs: Vec<gpui::TextRun>,
    window: AnyWindowHandle,
    system: SyncWeak<TextSystem>,
    rem: Pixels,
    scale: f32,
    element: Option<StyledText>,
    used: u64,
    cost: usize,
}
#[derive(Default)]
struct Pool {
    entries: Vec<Rc<RefCell<Box<State>>>>,
    bytes: usize,
    clock: u64,
    misses: u64,
}
thread_local! { static POOL: RefCell<Pool> = RefCell::new(Pool::default()); }
impl Pool {
    fn touch(&mut self, state: &Rc<RefCell<Box<State>>>, fresh: bool) {
        self.clock = self.clock.wrapping_add(1);
        state.borrow_mut().used = self.clock;
        if !fresh {
            return;
        }
        self.misses += 1;
        self.entries.retain(|entry| {
            if Rc::weak_count(entry) == 0 && Rc::strong_count(entry) == 1 {
                self.bytes = self.bytes.saturating_sub(entry.borrow().cost);
                false
            } else {
                true
            }
        });
        let cost = state.borrow().cost;
        if cost > MAX_BYTES {
            return;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= MAX_ENTRIES || self.bytes + cost > MAX_BYTES)
        {
            let oldest = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.borrow().used)
                .unwrap()
                .0;
            let removed = self.entries.swap_remove(oldest);
            self.bytes = self.bytes.saturating_sub(removed.borrow().cost);
        }
        self.entries.push(state.clone());
        self.bytes += cost;
    }
}

impl TextCache {
    pub(super) fn render(
        &self,
        id: String,
        text: SharedString,
        highlights: Highlights,
        fonts: Fonts,
        selection: Option<(SelectionContext, usize)>,
        dots: Vec<Range<usize>>,
    ) -> AnyElement {
        CachedText {
            owner: self.0.clone(),
            id,
            text,
            highlights,
            fonts,
            selection,
            dots,
        }
        .into_any_element()
    }
}
#[derive(IntoElement)]
struct CachedText {
    owner: Rc<RefCell<Weak<RefCell<Box<State>>>>>,
    id: String,
    text: SharedString,
    highlights: Highlights,
    fonts: Fonts,
    selection: Option<(SelectionContext, usize)>,
    /// Draft passages underlined with dots (`message::dotted`).
    dots: Vec<Range<usize>>,
}
impl RenderOnce for CachedText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = window.text_style();
        let system = Arc::downgrade(cx.text_system());
        let old = self.owner.borrow().upgrade();
        let existing = old.filter(|state| {
            let state = state.borrow();
            state.id == self.id
                && state.text == self.text
                && state.highlights == self.highlights
                && state.fonts == self.fonts
                && state.style == style
                && state.window == window.window_handle()
                && state.system.ptr_eq(&system)
                && state.rem == window.rem_size()
                && state.scale == window.scale_factor()
        });
        let fresh = existing.is_none();
        let state = existing.unwrap_or_else(|| {
            let runs = resolve_runs(&self.text, &style, &self.highlights, &self.fonts);
            let cost = self.text.len().saturating_mul(96).saturating_add(8192)
                + runs.capacity() * std::mem::size_of::<gpui::TextRun>()
                + self.highlights.capacity()
                    * std::mem::size_of::<(Range<usize>, HighlightStyle)>();
            Rc::new(RefCell::new(Box::new(State {
                id: self.id.clone(),
                text: self.text.clone(),
                highlights: self.highlights,
                fonts: self.fonts,
                style,
                runs,
                window: window.window_handle(),
                system,
                rem: window.rem_size(),
                scale: window.scale_factor(),
                element: Some(StyledText::new(self.text.clone())),
                used: 0,
                // A conservative glyph/run allowance, not the source-string size alone.
                cost,
            })))
        });
        *self.owner.borrow_mut() = Rc::downgrade(&state);
        POOL.with(|pool| pool.borrow_mut().touch(&state, fresh));
        let layout = state.borrow().element.as_ref().unwrap().layout().clone();
        let child = RetainedText(state).into_any_element();
        let child = if self.dots.is_empty() {
            child
        } else {
            super::dotted(child, layout.clone(), self.dots)
        };
        if let Some((selection, offset)) = self.selection {
            selection.wrap_region(self.id, offset, &self.text, child, layout, Vec::new())
        } else {
            child
        }
    }
}

fn resolve_runs(
    text: &str,
    style: &TextStyle,
    highlights: &Highlights,
    fonts: &Fonts,
) -> Vec<gpui::TextRun> {
    let mut runs = Vec::new();
    let mut offset = 0;
    for (range, highlight) in highlights {
        if offset < range.start {
            runs.push(style.to_run(range.start - offset));
        }
        runs.push(style.clone().highlight(*highlight).to_run(range.len()));
        offset = range.end;
    }
    if offset < text.len() {
        runs.push(style.to_run(text.len() - offset));
    }
    let mut offset = 0;
    let mut font_index = 0;
    for run in &mut runs {
        while font_index < fonts.len() && fonts[font_index].0.end <= offset {
            font_index += 1;
        }
        if let Some((range, family)) = fonts.get(font_index) {
            if offset >= range.start && offset + run.len <= range.end {
                run.font.family = family.clone();
            }
        }
        offset += run.len;
    }
    runs
}

struct RetainedText(Rc<RefCell<Box<State>>>);
impl IntoElement for RetainedText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for RetainedText {
    type RequestLayoutState = <StyledText as Element>::RequestLayoutState;
    type PrepaintState = <StyledText as Element>::PrepaintState;
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut state = self.0.borrow_mut();
        let mut element = state.element.take().unwrap().with_runs(state.runs.clone());
        let result = element.request_layout(id, inspector, window, cx);
        state.element = Some(element);
        result
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let mut state = self.0.borrow_mut();
        let element = state.element.as_mut().unwrap();
        let result = element.prepaint(id, inspector, bounds, request, window, cx);
        super::super::message_preview::record_lines(element.layout());
        result
    }
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0
            .borrow_mut()
            .element
            .as_mut()
            .unwrap()
            .paint(id, inspector, bounds, request, prepaint, window, cx)
    }
}

#[cfg(feature = "headless-bench")]
pub(super) fn stats() -> (usize, usize, u64, u64) {
    POOL.with(|pool| {
        let pool = pool.borrow();
        (pool.entries.len(), pool.bytes, pool.clock, pool.misses)
    })
}
