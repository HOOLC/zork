//! Shared dialog options and text measurement for plain GPUI panels.
use gpui::*;

pub struct DialogOptions {
    pub title_editor: Option<AnyElement>,
    pub title_action: Option<AnyElement>,
    pub notice: Option<String>,
    pub dismissible: bool,
}
impl Default for DialogOptions {
    fn default() -> Self {
        Self {
            title_editor: None,
            title_action: None,
            notice: None,
            dismissible: true,
        }
    }
}

pub(crate) fn measure_label(label: &str, size: f32, window: &mut Window) -> f32 {
    measure_label_with_weight(label, size, window.text_style().font().weight, window)
}

pub(crate) fn measure_label_with_weight(
    label: &str,
    size: f32,
    weight: FontWeight,
    window: &mut Window,
) -> f32 {
    let style = window.text_style();
    let mut font = style.font();
    font.weight = weight;
    window
        .text_system()
        .shape_line(
            label.to_owned().into(),
            px(size),
            &[TextRun {
                len: label.len(),
                font,
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        )
        .width
        .as_f32()
}
