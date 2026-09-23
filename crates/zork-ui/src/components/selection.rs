//! Text selection over the Markdown renderer's actual shaped text layouts.
//! The renderer keeps links and typography; selection never substitutes an editor.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    comments::CommentSource,
};
use gpui::{
    div, prelude::*, rgba, App, FocusHandle, HighlightStyle, MouseButton, Pixels, Point,
    StyledText, TextLayout,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

#[derive(Clone)]
struct Region {
    key: String,
    offset: usize,
    len: usize,
    layout: TextLayout,
}
#[derive(Clone)]
struct Drag {
    key: String,
    source: CommentSource,
    text: gpui::SharedString,
    anchor: usize,
    end: usize,
}
#[derive(Default)]
pub struct TranscriptSelection {
    regions: std::collections::HashMap<String, Region>,
    drag: Option<Drag>,
    pub dragging: bool,
}
impl TranscriptSelection {
    pub fn begin_frame(&mut self) {
        self.regions.clear();
    }
    pub fn clear(&mut self) {
        self.drag = None;
        self.dragging = false;
    }
    pub fn update(&mut self, point: Point<Pixels>) -> bool {
        if !self.dragging {
            return false;
        }
        let Some(drag) = &mut self.drag else {
            return false;
        };
        let nearest = self
            .regions
            .values()
            .filter(|r| r.key == drag.key)
            .min_by(|a, b| {
                region_distance_squared(a.layout.bounds(), point)
                    .total_cmp(&region_distance_squared(b.layout.bounds(), point))
                    .then_with(|| a.offset.cmp(&b.offset))
            });
        let Some(region) = nearest else {
            return false;
        };
        let index = region
            .layout
            .index_for_position(point)
            .unwrap_or_else(|index| index)
            .min(region.len);
        let end = region.offset + index;
        let changed = end != drag.end;
        drag.end = end;
        changed
    }
    /// Visible selection geometry, computed only when the drag finishes.
    pub fn selected_bounds(&self) -> Option<gpui::Bounds<Pixels>> {
        let drag = self.drag.as_ref()?;
        let range = clamp_selection(&drag.text, drag.anchor..drag.end);
        let mut bounds: Option<gpui::Bounds<Pixels>> = None;
        for region in self.regions.values().filter(|r| r.key == drag.key) {
            let start = range.start.max(region.offset);
            let end = range.end.min(region.offset + region.len);
            if start >= end {
                continue;
            }
            let a = region.layout.position_for_index(start - region.offset)?;
            let b = region.layout.position_for_index(end - region.offset)?;
            let wrapped = a.y != b.y;
            let left = if wrapped {
                region.layout.bounds().left()
            } else {
                a.x.min(b.x)
            };
            let right = if wrapped {
                region.layout.bounds().right()
            } else {
                a.x.max(b.x)
            };
            let selected = gpui::Bounds::from_corners(
                gpui::point(left, a.y.min(b.y)),
                gpui::point(right, a.y.max(b.y) + region.layout.line_height()),
            );
            bounds = Some(match bounds {
                Some(bounds) => bounds.union(&selected),
                None => selected,
            });
        }
        bounds
    }
    pub fn finish(&mut self) -> Option<CommentSource> {
        self.dragging = false;
        let drag = self.drag.as_ref()?;
        let range = clamp_selection(&drag.text, drag.anchor..drag.end);
        if range.is_empty() {
            return None;
        }
        let quote = drag.text.get(range)?.to_owned();
        if quote.trim().is_empty() {
            return None;
        }
        let mut source = drag.source.clone();
        source.quote = quote;
        Some(source)
    }
}

fn region_distance_squared(bounds: gpui::Bounds<Pixels>, point: Point<Pixels>) -> f32 {
    let dx = (bounds.left() - point.x)
        .as_f32()
        .max(0.)
        .max((point.x - bounds.right()).as_f32());
    let dy = (bounds.top() - point.y)
        .as_f32()
        .max(0.)
        .max((point.y - bounds.bottom()).as_f32());
    dx * dx + dy * dy
}

pub fn clamp_selection(text: &str, range: Range<usize>) -> Range<usize> {
    let mut a = range.start.min(range.end).min(text.len());
    let mut b = range.start.max(range.end).min(text.len());
    while !text.is_char_boundary(a) {
        a -= 1;
    }
    while !text.is_char_boundary(b) {
        b -= 1;
    }
    a..b
}

fn selection_in_region(selection: Range<usize>, offset: usize, len: usize) -> Option<Range<usize>> {
    let start = selection.start.max(offset);
    let end = selection.end.min(offset.saturating_add(len));
    if start >= end {
        return None;
    }
    Some(start - offset..end - offset)
}

#[derive(Clone)]
pub struct SelectionContext {
    pub key: String,
    pub source: CommentSource,
    pub text: gpui::SharedString,
    pub state: Rc<RefCell<TranscriptSelection>>,
    pub focus: FocusHandle,
    pub notify: Rc<dyn Fn(&mut App)>,
    cursor: Rc<Cell<usize>>,
    link_handler: Option<Rc<dyn Fn(&str, &mut App)>>,
}
impl SelectionContext {
    pub fn new(
        key: String,
        source: CommentSource,
        text: impl Into<gpui::SharedString>,
        state: Rc<RefCell<TranscriptSelection>>,
        focus: FocusHandle,
        notify: Rc<dyn Fn(&mut App)>,
    ) -> Self {
        Self {
            key,
            source,
            text: text.into(),
            state,
            focus,
            notify,
            cursor: Rc::new(Cell::new(0)),
            link_handler: None,
        }
    }
    pub fn with_link_handler(mut self, handler: Rc<dyn Fn(&str, &mut App)>) -> Self {
        self.link_handler = Some(handler);
        self
    }
    pub fn with_offset(self, offset: usize) -> Self {
        self.cursor.set(offset.min(self.text.len()));
        self
    }
    pub fn offset(&self, text: &str) -> usize {
        let cursor = self.cursor.get();
        let offset = self
            .text
            .get(cursor..)
            .and_then(|tail| tail.find(text))
            .map(|i| cursor + i)
            .unwrap_or(cursor);
        self.cursor.set((offset + text.len()).min(self.text.len()));
        offset
    }
    pub fn highlight(&self, offset: usize, len: usize) -> Option<(Range<usize>, HighlightStyle)> {
        let state = self.state.borrow();
        let drag = state.drag.as_ref()?;
        if drag.key != self.key {
            return None;
        }
        let selection = clamp_selection(&drag.text, drag.anchor..drag.end);
        selection_in_region(selection, offset, len).map(|range| {
            (
                range,
                HighlightStyle {
                    background_color: Some(rgba(*crate::design::TEXT_SELECTION).into()),
                    ..Default::default()
                },
            )
        })
    }
    pub fn wrap(
        &self,
        id: String,
        offset: usize,
        text: &str,
        styled: StyledText,
    ) -> gpui::AnyElement {
        self.wrap_linked(id, offset, text, styled, Vec::new(), Vec::new())
    }
    pub fn wrap_linked(
        &self,
        id: String,
        offset: usize,
        text: &str,
        styled: StyledText,
        links: Vec<Range<usize>>,
        urls: Vec<String>,
    ) -> gpui::AnyElement {
        let layout = styled.layout().clone();
        let child = if links.is_empty() {
            styled.into_any_element()
        } else {
            super::message::linked_text(
                format!("{id}-links"),
                styled,
                links.clone(),
                urls.clone(),
                false,
            )
            .into_any_element()
        };
        self.wrap_region(
            id,
            offset,
            text,
            child,
            layout,
            links.into_iter().zip(urls).collect(),
        )
    }
    pub(super) fn wrap_region(
        &self,
        id: String,
        offset: usize,
        text: &str,
        child: gpui::AnyElement,
        layout: TextLayout,
        links: Vec<(Range<usize>, String)>,
    ) -> gpui::AnyElement {
        let len = text.len();
        let registration = Region {
            key: self.key.clone(),
            offset,
            len,
            layout: layout.clone(),
        };
        let registration_state = self.state.clone();
        let registration_id = id.clone();
        let context = self.clone();
        let click_layout = layout.clone();
        let click_state = self.state.clone();
        let link_handler = self.link_handler.clone();
        div()
            .id(id)
            .relative()
            .cursor_text()
            .child(child)
            // Register after the StyledText has been prepainted. Virtual-list
            // measurement can construct text elements without painting them.
            .child(gpui::canvas(move|_,_,_|{registration_state.borrow_mut().regions.insert(registration_id.clone(),registration.clone());},|_,_,_,_|{}).absolute().inset_0())
            // Selection owns plain text's pointer interaction. Suppress GPUI's
            // generic pressed state, which otherwise refreshes the whole window
            // even when the text has no active style or click listener.
            .when(links.is_empty(), |v| v.capture_any_mouse_down(|event, window, _| {
                if event.button == MouseButton::Left {
                    window.prevent_default();
                }
            }))
            .on_mouse_down(MouseButton::Left, move |event, w, cx| {
                let index = layout
                    .index_for_position(event.position)
                    .unwrap_or_else(|i| i)
                    .min(len);
                context.state.borrow_mut().drag = Some(Drag {
                    key: context.key.clone(),
                    source: context.source.clone(),
                    text: context.text.clone(),
                    anchor: offset + index,
                    end: offset + index,
                });
                context.state.borrow_mut().dragging = true;
                w.focus(&context.focus, cx);
                (context.notify)(cx);
                cx.stop_propagation();
            })
            // GPUI's click listener tracks pressed state with a window-wide
            // refresh. Plain selectable text needs only the selection handler.
            .when(!links.is_empty(), |v| v.on_click(move |event, _, cx| {
                if click_state
                    .borrow()
                    .drag
                    .as_ref()
                    .is_some_and(|d| d.anchor != d.end)
                {
                    return;
                }
                if let Ok(index) = click_layout.index_for_position(event.position()) {
                    if let Some((_, url)) = links
                        .iter()
                        .find(|(range, _)| range.contains(&index))
                    {
                        if let Some(handler) = &link_handler { handler(url, cx); } else { cx.open_url(url); }
                    }
                }
            }))
            .automation(AutomationRole::Status, text.to_owned())
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_utf8_safe_and_can_run_backwards() {
        assert_eq!(clamp_selection("a🐈中文", 8..1), 1..8);
        assert_eq!(&"a🐈中文"[clamp_selection("a🐈中文", 2..7)], "🐈");
    }
    #[test]
    fn table_cells_on_the_same_row_are_hit_tested_horizontally() {
        let left = gpui::Bounds::new(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(gpui::px(10.), gpui::px(10.)),
        );
        let right = gpui::Bounds::new(
            gpui::point(gpui::px(20.), gpui::px(0.)),
            gpui::size(gpui::px(10.), gpui::px(10.)),
        );
        let point = gpui::point(gpui::px(25.), gpui::px(5.));
        assert_eq!(region_distance_squared(right, point), 0.);
        assert_eq!(region_distance_squared(left, point), 225.);
    }
    #[test]
    fn click_before_later_markdown_blocks_has_no_highlight() {
        // A click leaves an empty selection; later table cells/paragraphs must
        // not subtract their offsets from an earlier selection end.
        assert_eq!(selection_in_region(2..2, 10, 8), None);
        assert_eq!(selection_in_region(2..5, 10, 8), None);
        assert_eq!(selection_in_region(20..24, 10, 8), None);
        assert_eq!(selection_in_region(10..10, 10, 8), None);
    }
    #[test]
    fn selection_crossing_markdown_regions_is_clipped_locally() {
        assert_eq!(selection_in_region(2..14, 10, 8), Some(0..4));
        assert_eq!(selection_in_region(12..24, 10, 8), Some(2..8));
        assert_eq!(selection_in_region(2..24, 10, 8), Some(0..8));
        assert_eq!(
            selection_in_region(0..usize::MAX, usize::MAX - 2, 8),
            Some(0..2)
        );
    }
    #[test]
    fn finishing_preserves_exact_quote_and_station_source() {
        let mut selection = TranscriptSelection::default();
        selection.dragging = true;
        selection.drag = Some(Drag {
            key: "m".into(),
            source: CommentSource {
                message_id: Some("station-id".into()),
                ..Default::default()
            },
            text: "前句\n选中 原文\n后句".into(),
            anchor: 7,
            end: 20,
        });
        let source = selection.finish().unwrap();
        assert_eq!(source.message_id.as_deref(), Some("station-id"));
        assert_eq!(source.quote, "选中 原文");
        assert!(!selection.dragging);
    }
}
