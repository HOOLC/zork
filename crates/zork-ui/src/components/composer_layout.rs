//! Shared layout and palette for the composer and presence indicators.
pub const RADIUS: f32 = 16.;
pub const SPACING: f32 = 22.;
pub const EDGE: f32 = 22.;
pub const ACTIVE_EDGE: f32 = 0.;
pub const IMMERSION: f32 = 8.;
pub const ROW_SPACING: f32 = 35.;
pub const DOCK_GAP: f32 = 3.;
pub const TOP_EXTENSION: f32 = 12.;
pub const EDITOR_TOP_INSET: f32 = 8.;
/// Equal side and bottom spacing keeps the action concentric with the outer corner.
pub const ACTION_INSET: f32 = 6.;
/// Outer corner follows the circular send button plus its bottom inset.
pub const SURFACE_RADIUS: f32 = crate::design::ZORK_UI.composer.surface_radius;
pub const EDITOR_ACTION_GAP: f32 = 2.;
pub const COMPOSER_CHROME: f32 = EDITOR_TOP_INSET
    + EDITOR_ACTION_GAP
    + crate::design::ZORK_UI.composer.action_size
    + ACTION_INSET;
pub const DEFAULT_HEIGHT: f32 = 24. + TOP_EXTENSION + COMPOSER_CHROME;
pub const SURFACE_COLOR: u32 = crate::design::ZORK_UI.palette.window;
pub const LOWER_COLOR: u32 = SURFACE_COLOR;
pub const BORDER_COLOR: u32 = crate::design::ZORK_UI.palette.border_strong;
pub const BORDER_WIDTH: f32 = crate::design::BORDER_WIDTH;
pub const SLOT_BORDER_WIDTH: f32 = crate::design::BORDER_WIDTH;
pub const TEXT_COLOR: u32 = crate::design::ZORK_UI.palette.text;
pub const BUTTON_COLOR: u32 = crate::design::ZORK_UI.palette.accent;
pub const GLYPH_COLOR: u32 = 0xFFFFFF;
