use gpui::{point, px, AssetSource};
use zork_gui::assets::EmbeddedAssets;
use zork_gui::window_chrome::native_titlebar_options;

#[test]
fn native_window_uses_full_size_transparent_chrome() {
    let titlebar = native_titlebar_options();

    assert!(titlebar.title.is_none());
    assert!(titlebar.appears_transparent);
    assert_eq!(
        titlebar.traffic_light_position,
        Some(point(px(12.0), px(17.0)))
    );
}

#[test]
fn conversation_icons_are_embedded_assets() {
    let assets = EmbeddedAssets;

    for path in [
        "icons/paperclip.svg",
        "icons/arrow-up.svg",
        "icons/columns.svg",
        "icons/node.svg",
        "icons/x.svg",
    ] {
        assert!(assets.load(path).expect("asset load succeeds").is_some());
    }
}
