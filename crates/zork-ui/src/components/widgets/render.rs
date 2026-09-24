//! Native GPUI rounded surfaces.
use gpui::{div, prelude::*, px, rgb, App, Div, ElementId, IntoElement, Stateful, Window};

#[derive(Clone, Copy)]
pub struct SurfaceColors {
    pub fill: u32,
    pub border: Option<u32>,
    pub parent: u32,
    pub focused: bool,
}
impl SurfaceColors {
    pub fn plain(fill: u32, border: u32, parent: u32) -> Self {
        Self {
            fill,
            border: Some(border),
            parent,
            focused: false,
        }
    }
    pub fn filled(fill: u32, parent: u32) -> Self {
        Self {
            fill,
            border: None,
            parent,
            focused: false,
        }
    }
    pub fn outlined(border: u32, parent: u32) -> Self {
        Self::plain(parent, border, parent)
    }
}

pub fn skin(
    id: impl Into<ElementId>,
    width: f32,
    height: f32,
    radius: f32,
    colors: SurfaceColors,
    content: impl IntoElement,
    _window: &mut Window,
    _cx: &mut App,
) -> Stateful<Div> {
    let border = if colors.focused {
        Some(crate::controls::FIELD_FOCUS_BORDER())
    } else {
        colors.border
    };
    div()
        .id(id.into())
        .relative()
        .w(px(width))
        .h(px(height))
        .rounded(px(radius))
        .overflow_hidden()
        .bg(rgb(colors.fill))
        .when_some(border, |v, border| {
            v.border(px(crate::design::BORDER_WIDTH))
                .border_color(rgb(border))
        })
        .child(content)
}
