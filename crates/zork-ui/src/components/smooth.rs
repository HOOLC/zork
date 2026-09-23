//! Static rounded surfaces rendered by GPUI.
use gpui::{div, prelude::*, px, App, Div, SharedString, Stateful, Window};

#[derive(gpui::IntoElement)]
pub struct Fill {
    inner: Stateful<Div>,
    current_color: bool,
}

pub fn fill(id: impl Into<SharedString>, radius: f32) -> Fill {
    Fill {
        inner: div().id(id.into()).absolute().inset_0().rounded(px(radius)),
        current_color: false,
    }
}

impl Fill {
    pub fn current_color(mut self) -> Self {
        self.current_color = true;
        self
    }
}

impl gpui::Styled for Fill {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.inner.style()
    }
}

impl gpui::RenderOnce for Fill {
    fn render(self, window: &mut Window, _: &mut App) -> impl gpui::IntoElement {
        if self.current_color {
            self.inner.bg(window.text_style().color)
        } else {
            self.inner
        }
    }
}

pub type Surface = Stateful<Div>;

pub fn surface(id: impl Into<SharedString>, radius: f32) -> Surface {
    div().id(id.into()).rounded(px(radius)).overflow_hidden()
}
