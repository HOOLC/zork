use zork_gui::components::selector_menu::{SelectorKind, SelectorMenuState};

#[test]
fn selector_menu_opens_one_kind_and_returns_the_exact_choice() {
    let mut menu = SelectorMenuState::default();
    assert_eq!(menu.open(), None);

    menu.toggle(SelectorKind::Profile);
    assert_eq!(menu.open(), Some(SelectorKind::Profile));

    menu.toggle(SelectorKind::Model);
    assert_eq!(menu.open(), Some(SelectorKind::Model));
    assert_eq!(menu.choose(SelectorKind::Model, 2, 3), Some(2));
    assert_eq!(menu.open(), None);

    menu.toggle(SelectorKind::Thinking);
    assert_eq!(menu.choose(SelectorKind::Model, 1, 3), None);
    assert_eq!(menu.open(), Some(SelectorKind::Thinking));
    menu.dismiss();
    assert_eq!(menu.open(), None);
}

#[test]
fn selector_menu_keyboard_highlight_wraps_and_is_the_enter_choice() {
    let mut menu = SelectorMenuState::default();
    menu.open_at(SelectorKind::Model, 1);
    assert_eq!(menu.highlighted(), Some(1));

    assert_eq!(menu.move_highlight(1, 3), Some(2));
    assert_eq!(menu.move_highlight(1, 3), Some(0));
    assert_eq!(menu.move_highlight(-1, 3), Some(2));
    assert_eq!(menu.choose_highlighted(SelectorKind::Model, 3), Some(2));
    assert_eq!(menu.open(), None);
}
