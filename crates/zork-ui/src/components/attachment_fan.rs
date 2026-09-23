//! Static sizing for the attachment preview ribbon.

pub const MAX_FILES: usize = 16;
pub const REMOVE_SIZE: f32 = 24.;
pub const REMOVE_GLYPH: f32 = 10.;
const CARD_WIDTH: f32 = 72.;
const CARD_HEIGHT: f32 = 76.;
const TOGGLE_WIDTH: f32 = 28.;

pub fn dimensions(count: usize, available: f32) -> (f32, f32) {
    if count == 0 {
        return (0., 0.);
    }
    (
        (TOGGLE_WIDTH + count.min(MAX_FILES) as f32 * CARD_WIDTH)
            .min(available.max(TOGGLE_WIDTH + CARD_WIDTH)),
        CARD_HEIGHT,
    )
}
