//! Shared liquid palette. Packed colors are RGB; hosts add alpha.
/// Brand emphasis from apps/zork-design/tokens/design-tokens.json (brand.accent).
/// Keep identity emphasis separate from status colors and neutral surfaces.
pub const BRAND_ACCENT: u32 = 0xE9643B;

/// The outline of a liquid control or content surface. Keep it legible on both
/// the warm canvas and a modal scrim; interior feedback uses solid fills.
pub const LIQUID_OUTLINE: u32 = 0xB6BABD;

/// Shared hairline in logical pixels (one physical pixel at 2× scale).
pub const BORDER_WIDTH: f32 = 0.5;
pub const SMOOTHING: f64 = 0.6;
pub const BUTTON_RADIUS: f32 = 999.;
pub const FIELD_RADIUS: f32 = 10.;
pub const CARD_RADIUS: f32 = 12.;
pub const COMPACT_CARD_RADIUS: f32 = 12.;
pub const ICON_BUTTON_RADIUS: f32 = 10.;

/// Shared action feedback, independent of the page containing the control.
pub struct InteractionPalette {
    pub neutral_hover: u32,
    pub neutral_pressed: u32,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub accent_hover: u32,
    pub accent_pressed: u32,
    pub focus_border: u32,
}
pub const INTERACTION: InteractionPalette = InteractionPalette {
    neutral_hover: 0xEFEEEA,
    neutral_pressed: 0xEAE7E1,
    primary_hover: 0x41464C,
    primary_pressed: 0x1B1E21,
    accent_hover: 0xDB572F,
    accent_pressed: 0xC84A27,
    focus_border: 0x646970,
};
