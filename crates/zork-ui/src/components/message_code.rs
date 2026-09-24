//! Cached, pure-Rust syntax highlighting for messages.
use gpui::{rgb, AnyWindowHandle, App, Global, HighlightStyle, Window};
use std::{
    cell::RefCell,
    collections::VecDeque,
    ops::Range,
    rc::{Rc, Weak},
    sync::{LazyLock, Mutex},
};
use syntect::{
    easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings,
};

pub(super) type Runs = Vec<(Range<usize>, HighlightStyle)>;
#[derive(Debug)]
pub(super) struct Presentation {
    pub text: gpui::SharedString,
    state: Rc<RefCell<State>>,
    pub layout_cache: super::text_cache::TextCache,
}
pub(super) fn prepare(language: Option<&str>, code: &str) -> Rc<Presentation> {
    Rc::new(Presentation {
        text: code.to_owned().into(),
        state: Rc::new(RefCell::new(State {
            language: language.unwrap_or_default().into(),
            ready: (!eligible(language, code)).then(Vec::new),
            eligible: eligible(language, code),
            theme: crate::design::theme(),
            queued: false,
            windows: Vec::new(),
        })),
        layout_cache: Default::default(),
    })
}

#[derive(Debug)]
struct State {
    language: String,
    ready: Option<Runs>,
    eligible: bool,
    /// The theme `ready` was highlighted for; a theme switch recomputes it.
    theme: crate::design::Theme,
    queued: bool,
    windows: Vec<AnyWindowHandle>,
}
struct Request {
    state: Weak<RefCell<State>>,
    text: gpui::SharedString,
}
#[derive(Default)]
struct Worker {
    running: bool,
    pending: VecDeque<Request>,
}
impl Global for Worker {}
const MAX_PENDING: usize = 64;

impl Presentation {
    pub(super) fn highlights(&self, window: &mut Window, cx: &mut App) -> Runs {
        let mut state = self.state.borrow_mut();
        let theme = crate::design::theme();
        if state.eligible && state.theme != theme {
            state.ready = None;
            state.queued = false;
            state.theme = theme;
        }
        if let Some(runs) = &state.ready {
            return runs.clone();
        }
        let handle = window.window_handle();
        if !state.windows.contains(&handle) {
            state.windows.push(handle);
        }
        if state.queued {
            return Vec::new();
        }
        state.queued = true;
        drop(state);
        let worker = cx.default_global::<Worker>();
        while worker.pending.len() >= MAX_PENDING {
            if let Some(old) = worker.pending.pop_front().and_then(|r| r.state.upgrade()) {
                old.borrow_mut().queued = false;
            }
        }
        worker.pending.push_back(Request {
            state: Rc::downgrade(&self.state),
            text: self.text.clone(),
        });
        pump(cx);
        Vec::new()
    }
}

fn pump(cx: &mut App) {
    let worker = cx.default_global::<Worker>();
    if worker.running {
        return;
    }
    let request = loop {
        let Some(request) = worker.pending.pop_front() else {
            return;
        };
        if request.state.strong_count() > 0 {
            break request;
        }
    };
    let language = request.state.upgrade().unwrap().borrow().language.clone();
    let theme = crate::design::theme();
    worker.running = true;
    let work = cx
        .background_executor()
        .spawn(async move { highlights_for(Some(&language), &request.text, theme) });
    cx.spawn(async move |cx| {
        let runs = work.await;
        let _ = cx.update(|cx| {
            if let Some(state) = request.state.upgrade() {
                let windows = {
                    let mut state = state.borrow_mut();
                    // A late result for an earlier theme is dropped; the next paint requeues.
                    if state.theme == theme {
                        state.ready = Some(runs);
                    }
                    state.queued = false;
                    std::mem::take(&mut state.windows)
                };
                for window in windows {
                    let _ = window.update(cx, |_, window, _| window.refresh());
                }
            }
            cx.default_global::<Worker>().running = false;
            pump(cx);
        });
    })
    .detach();
}

fn eligible(language: Option<&str>, code: &str) -> bool {
    language.is_some_and(|s| !s.is_empty())
        && code.len() <= 64 * 1024
        && !code.lines().any(|line| line.len() > 4096)
}
#[cfg(test)]
thread_local! { static PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn parse_count() -> usize {
    PARSES.with(|count| count.get())
}

struct Highlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
    cache: VecDeque<(crate::design::Theme, String, String, Runs)>,
}
static HIGHLIGHTER: LazyLock<Mutex<Highlighter>> = LazyLock::new(|| {
    Mutex::new(Highlighter {
        syntaxes: SyntaxSet::load_defaults_newlines(),
        themes: ThemeSet::load_defaults(),
        cache: VecDeque::new(),
    })
});

pub(super) fn highlights(language: Option<&str>, code: &str) -> Runs {
    highlights_for(language, code, crate::design::theme())
}

/// Syntax colors come from a light or dark syntect theme to match the surface.
fn highlights_for(language: Option<&str>, code: &str, theme: crate::design::Theme) -> Runs {
    let Some(language) = language.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    // Large outputs stay readable without blocking the UI on syntax parsing.
    if code.len() > 64 * 1024 || code.lines().any(|line| line.len() > 4096) {
        return Vec::new();
    }
    let token = match language.to_ascii_lowercase().as_str() {
        "shell" | "sh" | "zsh" => "bash".to_owned(),
        "javascript" | "jsx" => "js".to_owned(),
        "python" => "py".to_owned(),
        "rust" => "rs".to_owned(),
        "yml" => "yaml".to_owned(),
        other => other.to_owned(),
    };
    {
        let mut state = HIGHLIGHTER
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(index) = state
            .cache
            .iter()
            .position(|(cached, lang, source, _)| {
                *cached == theme && lang == &token && source == code
            })
        {
            let entry = state.cache.remove(index).unwrap();
            let runs = entry.3.clone();
            state.cache.push_back(entry);
            return runs;
        }
        let Some(syntax) = state.syntaxes.find_syntax_by_token(&token) else {
            return Vec::new();
        };
        #[cfg(test)]
        PARSES.with(|count| count.set(count.get() + 1));
        let theme_name = match theme {
            crate::design::Theme::Light => "InspiredGitHub",
            crate::design::Theme::Dark => "base16-ocean.dark",
        };
        let mut highlighter = HighlightLines::new(syntax, &state.themes.themes[theme_name]);
        let mut offset = 0;
        let mut runs: Runs = Vec::new();
        for line in LinesWithEndings::from(code) {
            let Ok(parts) = highlighter.highlight_line(line, &state.syntaxes) else {
                return Vec::new();
            };
            for (style, text) in parts {
                let end = offset + text.len();
                let color = style.foreground;
                let highlight = HighlightStyle {
                    color: Some(
                        rgb((u32::from(color.r) << 16)
                            | (u32::from(color.g) << 8)
                            | u32::from(color.b))
                        .into(),
                    ),
                    ..Default::default()
                };
                match runs.last_mut() {
                    Some((range, style)) if *style == highlight => range.end = end,
                    _ => runs.push((offset..end, highlight)),
                }
                offset = end;
            }
        }
        if state.cache.len() >= 32 {
            state.cache.pop_front();
        }
        state
            .cache
            .push_back((theme, token, code.to_owned(), runs.clone()));
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn first_render_does_not_parse_and_completion_belongs_to_the_document(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let old = prepare(Some("rust"), "let old = 1;\n");
        let retired = Rc::downgrade(&old.state);
        let current = prepare(Some("rust"), "let current = \"你好🐈\";\n");
        let before = parse_count();
        cx.update(|window, cx| {
            cx.default_global::<Worker>().running = true;
            assert!(old.highlights(window, cx).is_empty());
            assert!(current.highlights(window, cx).is_empty());
            assert_eq!(parse_count(), before, "render initialized the parser");
        });
        drop(old);
        assert!(retired.upgrade().is_none());
        cx.update(|_, cx| {
            cx.default_global::<Worker>().running = false;
            pump(cx);
        });
        cx.run_until_parked();
        let ready = current
            .state
            .borrow()
            .ready
            .clone()
            .expect("background highlight completed");
        assert_eq!(ready, highlights(Some("rust"), &current.text));
        assert!(!ready.is_empty());
        let parses = parse_count();
        cx.update(|window, cx| assert_eq!(current.highlights(window, cx), ready));
        cx.run_until_parked();
        assert_eq!(
            parse_count(),
            parses,
            "revisiting unchanged code parsed again"
        );
    }

    #[gpui::test]
    fn rapid_scrolling_keeps_pending_work_bounded_and_releases_retired_documents(
        cx: &mut gpui::TestAppContext,
    ) {
        let cx = cx.add_empty_window();
        let documents = (0..100)
            .map(|i| prepare(Some("rust"), &format!("let item_{i} = 1;")))
            .collect::<Vec<_>>();
        cx.update(|window, cx| {
            cx.default_global::<Worker>().running = true;
            for document in &documents {
                document.highlights(window, cx);
            }
            assert_eq!(cx.global::<Worker>().pending.len(), MAX_PENDING);
            assert!(
                !documents[0].state.borrow().queued,
                "evicted work must be retryable"
            );
        });
        drop(documents);
        cx.update(|_, cx| {
            cx.default_global::<Worker>().running = false;
            pump(cx);
            assert!(cx.global::<Worker>().pending.is_empty());
            assert!(!cx.global::<Worker>().running);
        });
    }

    #[test]
    fn syntax_colors_preserve_utf8_ranges_and_newlines() {
        for (language, code) in [
            ("rust", "let name = \"你好🐈\";\n// comment\n"),
            ("json", "{\"name\": \"你好\", \"count\": 42}"),
        ] {
            let runs = highlights(Some(language), code);
            assert!(!runs.is_empty());
            assert_eq!(
                runs.iter()
                    .map(|(range, _)| &code[range.clone()])
                    .collect::<String>(),
                code
            );
            assert!(runs.iter().any(|(_, style)| style.color != runs[0].1.color));
            assert_eq!(runs, highlights(Some(language), code));
        }
        assert!(highlights(Some("unknown-language"), "你好").is_empty());
        assert!(highlights(None, "plain text").is_empty());
    }
}
