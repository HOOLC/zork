//! macOS application commands shared by the client and the native design app.

use gpui::{actions, App, KeyBinding, Menu, MenuItem, SystemMenuType};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSBundle, NSString};
use zork_ui::components::text_input::{
    CopyText, CutText, PasteText, RedoText, SelectAllText, UndoText,
};

actions!(
    zork_gui,
    [About, Hide, HideOthers, ShowAll, Quit, OpenSettings]
);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Client,
    Design,
}

impl AppKind {
    fn fallback_name(self) -> &'static str {
        match self {
            Self::Client => "Zork",
            Self::Design => "Zork Design PC",
        }
    }
}

fn display_name(kind: AppKind) -> String {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleDisplayName"))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| kind.fallback_name().to_owned())
}

pub fn install(cx: &mut App, kind: AppKind) {
    let name = display_name(kind);
    cx.on_action(|_: &About, _| {
        if let Some(main_thread) = MainThreadMarker::new() {
            NSApplication::sharedApplication(main_thread).orderFrontStandardAboutPanel(None);
        }
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());

    let mut bindings = vec![
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-z", UndoText, None),
        KeyBinding::new("cmd-shift-z", RedoText, None),
        KeyBinding::new("cmd-x", CutText, None),
        KeyBinding::new("cmd-c", CopyText, None),
        KeyBinding::new("cmd-v", PasteText, None),
        KeyBinding::new("cmd-a", SelectAllText, None),
    ];
    if kind == AppKind::Client {
        bindings.push(KeyBinding::new("cmd-,", OpenSettings, None));
    }
    cx.bind_keys(bindings);

    let mut app_items = vec![
        MenuItem::action(format!("关于 {name}"), About),
        MenuItem::separator(),
    ];
    if kind == AppKind::Client {
        app_items.push(MenuItem::action("设置…", OpenSettings));
        app_items.push(MenuItem::separator());
    }
    app_items.extend([
        MenuItem::os_submenu("服务", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action(format!("隐藏 {name}"), Hide),
        MenuItem::action("隐藏其他应用", HideOthers),
        MenuItem::action("显示全部", ShowAll),
        MenuItem::separator(),
        MenuItem::action(format!("退出 {name}"), Quit),
    ]);

    cx.set_menus([
        Menu::new(name).items(app_items),
        Menu::new("编辑").items([
            MenuItem::action("撤销", UndoText),
            MenuItem::action("重做", RedoText),
            MenuItem::separator(),
            MenuItem::action("剪切", CutText),
            MenuItem::action("复制", CopyText),
            MenuItem::action("粘贴", PasteText),
            MenuItem::action("全选", SelectAllText),
        ]),
    ]);
}
