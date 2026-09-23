//! Cached, pure-Rust syntax highlighting for messages.
use gpui::{rgb, HighlightStyle};
use std::{cell::RefCell, collections::VecDeque, ops::Range};
use syntect::{
    easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet, util::LinesWithEndings,
};

pub(super) type Runs = Vec<(Range<usize>, HighlightStyle)>;
#[derive(Clone, Debug)]
pub(super) struct Presentation {
    pub text: gpui::SharedString,
    pub highlights: Runs,
    pub layout_cache: super::text_cache::TextCache,
}
pub(super) fn prepare(language: Option<&str>, code: &str) -> Presentation {
    Presentation {
        text: code.to_owned().into(),
        highlights: highlights(language, code),
        layout_cache: Default::default(),
    }
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
    cache: VecDeque<(String, String, Runs)>,
}
thread_local! {
    static HIGHLIGHTER: RefCell<Highlighter> = RefCell::new(Highlighter {
        syntaxes: SyntaxSet::load_defaults_newlines(),
        themes: ThemeSet::load_defaults(),
        cache: VecDeque::new(),
    });
}

pub(super) fn highlights(language: Option<&str>, code: &str) -> Runs {
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
    HIGHLIGHTER.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(index) = state
            .cache
            .iter()
            .position(|(lang, source, _)| lang == &token && source == code)
        {
            let entry = state.cache.remove(index).unwrap();
            let runs = entry.2.clone();
            state.cache.push_back(entry);
            return runs;
        }
        let Some(syntax) = state.syntaxes.find_syntax_by_token(&token) else {
            return Vec::new();
        };
        #[cfg(test)]
        PARSES.with(|count| count.set(count.get() + 1));
        let mut highlighter = HighlightLines::new(syntax, &state.themes.themes["InspiredGitHub"]);
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
            .push_back((token, code.to_owned(), runs.clone()));
        runs
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
