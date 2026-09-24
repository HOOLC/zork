//! Desktop conversation navigation and shared icon buttons.
use crate::design::ZORK_UI;
use gpui::{prelude::*, px, rgb, svg};

#[allow(non_snake_case)]
pub fn PANEL_BACKGROUND() -> u32 {
    ZORK_UI.palette.canvas
}
#[allow(non_snake_case)]
pub fn ICON_COLOR() -> u32 {
    ZORK_UI.palette.muted
}
#[allow(non_snake_case)]
pub fn DISABLED_COLOR() -> u32 {
    ZORK_UI.palette.subtle
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellRoute {
    Home,
    Task(String),
}

pub struct ShellState {
    route: ShellRoute,
    pub rail_open: bool,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            route: ShellRoute::Home,
            rail_open: true,
        }
    }
}
impl ShellState {
    pub fn route(&self) -> &ShellRoute {
        &self.route
    }
    pub fn navigate(&mut self, route: ShellRoute) {
        self.route = route;
    }
}

pub fn icon_button(
    id: impl Into<gpui::ElementId>,
    icon: &'static str,
    enabled: bool,
) -> zork_ui::controls::Action {
    zork_ui::controls::icon_button_sized(id, enabled, zork_ui::controls::IconButtonSize::Compact)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(svg().path(icon).size(px(16.)).text_color(rgb(if enabled {
            ICON_COLOR()
        } else {
            DISABLED_COLOR()
        })))
}
