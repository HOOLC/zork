pub mod activity;
pub mod attachment_fan;
pub mod attachments;
pub mod brand;
pub(crate) mod choice_menu;
pub mod comments;
pub mod flyout;
pub mod frame_delivery;
pub mod geometry;
pub mod history;
pub mod interaction;
pub mod loading;
pub mod message;
pub mod selection;
pub mod text_input;
pub fn init(cx: &mut gpui::App) {
    text_input::init(cx);
}

pub mod tooltip;

pub mod region;

pub mod message_preview;

pub mod composer_layout;
pub mod standard_menu;
pub mod widgets;

pub mod smooth;
pub mod workbench;

pub mod json_tree;

pub mod message_row;

pub mod message_placeholder;
pub mod profile_card;
