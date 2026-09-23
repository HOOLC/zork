//! Static panels positioned by GPUI and gpui-base.
use gpui::{prelude::*, *};
use std::rc::Rc;

mod floating;
pub use floating::{FloatingPanel, FloatingStyle, Hover, Side};
mod inline;
pub use inline::{inline, InlinePanel};

pub struct Content {
    pub sections: Vec<AnyElement>,
    pub padding: f32,
    pub gap: f32,
}
impl Content {
    pub fn new(sections: Vec<AnyElement>) -> Self {
        Self {
            sections,
            padding: 16.,
            gap: 12.,
        }
    }
}
