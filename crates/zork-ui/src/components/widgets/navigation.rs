//! GPUI navigation rows and tabs.
use super::controls;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc};

mod group;
pub use group::{Group, GroupSurface};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sidebar,
    Tabs,
    Rows,
    Actions,
}

#[derive(Clone, Copy)]
struct Style {
    kind: Kind,
    parent: u32,
    row_radius: f32,
}
