//! Static sizing for the draft attachment row inside the composer.

pub const MAX_FILES: usize = 16;
/// Capsule height; the thumbnail sits concentric inside it.
pub const CHIP_HEIGHT: f32 = 40.;
pub const CHIP_INSET: f32 = 4.;
pub const THUMB_SIZE: f32 = CHIP_HEIGHT - 2. * CHIP_INSET;
/// Concentric with the capsule: outer radius minus the inset.
pub const THUMB_RADIUS: f32 = CHIP_HEIGHT / 2. - CHIP_INSET;
pub const CHIP_MAX_WIDTH: f32 = 236.;
pub const REMOVE_SIZE: f32 = 28.;
pub const REMOVE_GLYPH: f32 = 14.;
/// Space the row adds at the top of the composer surface: the row starts at
/// the action inset and the editor keeps its usual gap below it.
pub const FILES_BAND: f32 = CHIP_HEIGHT + 4.;

/// Width a chip takes for `name`/`meta`, used to decide the overflow fade
/// before layout. Text width is estimated generously per character.
pub fn chip_width(name: &str, meta: &str) -> f32 {
    let text = |s: &str, size: f32| {
        s.chars()
            .map(|c| if c.is_ascii() { size * 0.6 } else { size })
            .sum::<f32>()
    };
    let label = text(name, 12.5).max(text(meta, 12.));
    (CHIP_INSET + THUMB_SIZE + 8. + label + 2. + REMOVE_SIZE + CHIP_INSET).min(CHIP_MAX_WIDTH)
}

/// Total row width for `chips` (name, meta) pairs with the 8 px gap.
pub fn row_width<'a>(chips: impl IntoIterator<Item = (&'a str, &'a str)>) -> f32 {
    let mut total = 0.;
    let mut count = 0;
    for (name, meta) in chips.into_iter().take(MAX_FILES) {
        total += chip_width(name, meta);
        count += 1;
    }
    total + 8. * (count.max(1) - 1) as f32
}
