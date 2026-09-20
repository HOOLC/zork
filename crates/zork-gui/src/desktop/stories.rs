//! Development-only component stories. Every sample calls production renderers.
use super::{profiles::ProfilesView, ui};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, AnyView, Context, Window};
use serde_json::{json, Value};

pub use zork_ui::stories::{PrimitiveStory, Story};

fn new_chat_story(
    state: &str,
    width: f32,
    cx: &mut gpui::App,
) -> gpui::Entity<zork_ui::new_chat::Page> {
    let fixture = std::rc::Rc::new(std::cell::RefCell::new(
        zork_client_core::new_chat::Fixture::new(state),
    ));
    let text = zork_ui::resources::Text(std::rc::Rc::new(|key| {
        crate::i18n::Locale::ZhCn.text(key).into()
    }));
    let form_width = (width - 48.).clamp(220., 680.);
    let view = cx.new(|cx| {
        let mut view = zork_ui::new_chat::Page::new(text.clone(), cx);
        view.configure(fixture.borrow().snapshot(), form_width, text.clone(), cx);
        view
    });
    cx.subscribe(&view, move |view, event: &zork_ui::new_chat::Event, cx| {
        if let zork_ui::new_chat::Event::Intent(action) = event {
            fixture.borrow_mut().apply(action.clone());
            view.update(cx, |v, cx| {
                v.configure(fixture.borrow().snapshot(), form_width, text.clone(), cx)
            });
        }
    })
    .detach();
    view
}
fn click(id: &str) -> Value {
    json!({"type":"click","target":{"element_id":id}})
}
pub fn install(cx: &mut gpui::App) {
    cx.set_global(zork_ui::history_page::stories::StoryText(
        zork_ui::resources::Text(std::rc::Rc::new(|key| {
            crate::i18n::Locale::ZhCn.text(key).into()
        })),
    ));
    if cx
        .try_global::<zork_ui::liquid_story::business::Catalog>()
        .is_none()
    {
        zork_ui::liquid_story::business::install(
            catalog(),
            |story, cx| cx.new(|cx| StoryHost::new(story, cx)).into(),
            |view, cx| {
                view.clone()
                    .downcast::<StoryHost>()
                    .map(|view| view.read(cx).inspect(cx))
                    .unwrap_or_default()
            },
            cx,
        );
    }
    zork_ui::liquid_story::install_composer_fixture(cx, || {
        Box::new(zork_client_core::composer::fixture::Fixture::default())
    });
}

pub fn catalog() -> Vec<Story> {
    let mut items = zork_ui::stories::catalog();
    for (family, title, states, source) in [
        (
            "new-chat",
            "新建 Chat",
            &["draft", "no-models", "creating", "retry", "error"][..],
            "crates/zork-ui/src/new_chat.rs",
        ),
        (
            "node-directory",
            "本机与已存设备",
            &[
                "empty",
                "running",
                "stopped",
                "background",
                "pairing",
                "loading",
            ][..],
            "crates/zork-ui/src/node_directory.rs",
        ),
    ] {
        for state in states {
            let mut story = Story::new(family, title, state, source, family);
            story.width = 900.;
            story.height = 700.;
            items.push(story);
        }
    }
    for state in ["entry", "agent", "long"] {
        let mut story = Story::new(
            "history-details",
            "执行记录与成员详情",
            state,
            "crates/zork-ui/src/history_details.rs",
            "history-details",
        );
        story.width = 900.;
        story.height = 700.;
        story.actions = vec![click("history-details-open")];
        items.push(story);
    }
    for state in ["idle", "active", "error"] {
        let mut story = Story::new(
            "member-activity",
            "成员活动预览",
            state,
            "crates/zork-ui/src/member_activity.rs",
            "member-activity",
        );
        story.width = 640.;
        story.height = 600.;
        story.actions =
            vec![json!({"type":"move","target":{"element_id":"member-activity-source"}})];
        items.push(story);
    }
    for state in ["automatic", "minimum", "custom", "maximum"] {
        let mut story = Story::new(
            "appearance",
            "客户端外观",
            state,
            "crates/zork-ui/src/settings/appearance.rs",
            "appearance",
        );
        story.width = 640.;
        story.height = 880.;
        items.push(story);
    }
    for state in ["idle", "error"] {
        let mut story = Story::new(
            "data-settings",
            "清空本机数据",
            state,
            "crates/zork-ui/src/settings/data.rs",
            "data-settings",
        );
        story.width = 640.;
        story.height = 640.;
        story.actions = vec![click("clear-client-data")];
        items.push(story);
    }
    for state in ["empty", "tabs", "applications", "loading", "error"] {
        let mut story = Story::new(
            "browser",
            "页面浏览器",
            state,
            "crates/zork-ui/src/browser_chrome.rs",
            "browser",
        );
        story.width = 640.;
        story.height = 600.;
        items.push(story);
    }
    let mut composer = Story::new(
        "composer",
        "消息输入",
        "interactive",
        "crates/zork-ui/src/components/liquid/composer.rs",
        "composer",
    );
    composer.width = 900.;
    composer.height = 700.;
    items.push(composer);
    for state in ["image", "document", "long", "loading", "error"] {
        let mut story = Story::new(
            "attachment-viewer",
            "附件预览",
            state,
            "crates/zork-ui/src/attachment_viewer.rs",
            "attachment-viewer",
        );
        story.width = 900.;
        story.height = 760.;
        story.actions = vec![click("attachment-viewer-open")];
        items.push(story);
    }
    for state in ["menu", "toolbar", "files", "pages", "empty"] {
        let mut story = Story::new(
            "conversation-files",
            "会话文件与页面",
            state,
            "crates/zork-ui/src/conversation_contents.rs",
            "conversation-files",
        );
        story.width = 640.;
        story.height = 600.;
        items.push(story);
    }
    for state in [
        "connected",
        "creator",
        "unread",
        "collapsed",
        "offline",
        "empty",
    ] {
        let mut story = Story::new(
            "chat-navigation",
            "设备与 Chat 导航",
            state,
            "crates/zork-ui/src/chat_navigation.rs",
            "chat-navigation",
        );
        story.width = 320.;
        story.height = 760.;
        items.push(story);
    }
    for state in ["markdown", "literal", "long"] {
        let mut story = Story::new(
            "message-reader",
            "全文阅读",
            state,
            "crates/zork-ui/src/components/message_reader.rs",
            "message-reader",
        );
        story.width = 900.;
        story.height = 760.;
        story.actions = vec![click("message-reader-open")];
        items.push(story);
    }
    for state in ["list", "grid", "preview", "empty", "loading", "error"] {
        let mut story = Story::new(
            "shared-files",
            "共享文件",
            state,
            "crates/zork-ui/src/shared_files.rs",
            "shared-files",
        );
        story.width = 900.;
        story.height = 600.;
        items.push(story);
    }
    for state in [
        "list", "detail", "services", "skills", "empty", "loading", "error",
    ] {
        let mut story = Story::new(
            "resources",
            "资源目录与详情",
            state,
            "crates/zork-ui/src/resources.rs",
            "resources",
        );
        story.width = 900.;
        story.height = 600.;
        items.push(story);
    }
    for (family, title, state, target, actions, reference) in [
        (
            "connection",
            "大模型",
            "list",
            "desktop-settings-column",
            vec![],
            "connection-list",
        ),
        (
            "connection",
            "大模型",
            "create",
            "profile-create-dialog",
            vec![click("profile-add")],
            "connection-create",
        ),
        (
            "connection",
            "大模型",
            "provider",
            "profile-create-dialog",
            vec![click("profile-add"), click("profile-provider-select")],
            "connection-provider",
        ),
        (
            "model",
            "模型配置",
            "detail",
            "profile-detail-dialog",
            vec![],
            "model-detail",
        ),
        (
            "model",
            "模型配置",
            "create",
            "model-editor-dialog",
            vec![
                click("profile-model-add"),
                click("profile-context-limit"),
                json!({"type":"type_text","text":"32000"}),
                click("profile-output-limit"),
                json!({"type":"type_text","text":"4096"}),
                click("profile-model"),
            ],
            "model-create",
        ),
        (
            "model",
            "模型配置",
            "protocol",
            "model-editor-dialog",
            vec![click("profile-model-add"), click("model-api-select")],
            "model-protocol",
        ),
        (
            "conversation",
            "会话",
            "messages",
            "story-component",
            vec![],
            "conversation",
        ),
        (
            "conversation",
            "会话",
            "composer",
            "liquid-composer-surface",
            vec![],
            "composer",
        ),
        (
            "conversation",
            "会话",
            "history",
            "story-component",
            vec![],
            "conversation-history",
        ),
    ] {
        for (width, height, suffix) in [(900., 600., "compact"), (1280., 800., "wide")] {
            let mut story = Story::new(
                family,
                title,
                &format!("{state}-{suffix}"),
                if family == "conversation" {
                    "crates/zork-ui/src/components/message_row.rs / history_page/mod.rs"
                } else if family == "agent" {
                    "desktop/agents.rs"
                } else {
                    "desktop/profiles.rs"
                },
                reference,
            );
            story.width = width;
            story.height = height;
            story.target = target.into();
            story.actions = actions.clone();
            items.push(story);
        }
    }
    for (family, title, states) in [
        (
            "client",
            "客户端设置",
            &["signed-out", "signed-in", "loading", "error"][..],
        ),
        (
            "device",
            "设备设置",
            &["running", "stopped", "loading", "error"][..],
        ),
        ("mesh", "设备连接", &["connected", "empty", "manual"][..]),
        (
            "enrollment",
            "连接设备",
            &["start", "command", "loading", "error", "expired"][..],
        ),
    ] {
        for state in states {
            for (width, height, suffix) in [(900., 600., "compact"), (1280., 800., "wide")] {
                let mut story = Story::new(
                    family,
                    title,
                    &format!("{state}-{suffix}"),
                    if matches!(family, "mesh" | "enrollment") {
                        "zork-ui/src/network.rs"
                    } else {
                        "zork-ui/src/settings.rs"
                    },
                    family,
                );
                story.width = width;
                story.height = height;
                story.target = if family == "enrollment" {
                    "add-device-dialog"
                } else if family == "mesh" && *state == "manual" {
                    "mesh-peer-dialog"
                } else {
                    "desktop-settings-column"
                }
                .into();
                items.push(story);
            }
        }
    }
    for state in [
        "approval",
        "approved",
        "declined",
        "cancelled",
        "input",
        "prefilled",
        "create",
        "update",
        "login",
        "login-device",
        "login-callback",
        "login-completed",
        "completed",
        "long",
    ] {
        for (width, height, suffix) in [(420., 760., "compact"), (900., 700., "wide")] {
            let mut story = Story::new(
                "message-interaction",
                "交互消息",
                &format!("{state}-{suffix}"),
                "zork-ui/src/components/interaction.rs",
                "interaction",
            );
            story.width = width;
            story.height = height;
            story.target = "interaction-card-preview".into();
            items.push(story);
        }
    }
    for state in ["enabled", "disabled", "muted", "denied", "busy", "error"] {
        let mut story = Story::new(
            "notifications",
            "通知设置",
            state,
            "zork-ui/src/settings/notifications.rs",
            "notifications",
        );
        story.width = 900.;
        story.height = 680.;
        items.push(story);
    }
    for state in ["long", "loading", "empty", "offline", "error"] {
        let mut story = Story::new(
            "conversation",
            "会话",
            state,
            "crates/zork-ui/src/components/message_row.rs",
            "conversation",
        );
        story.width = 900.;
        story.height = 700.;
        items.push(story);
    }
    let form = zork_ui::stories::page_fixture()["model_form"].clone();
    for story in &mut items {
        if story.family == "model"
            && (story.state.starts_with("create") || story.state.starts_with("protocol"))
        {
            story.actions = vec![
                click("profile-model-add"),
                click("profile-context-limit"),
                json!({"type":"type_text","text":form["context_window"].as_u64().unwrap().to_string()}),
                click("profile-output-limit"),
                json!({"type":"type_text","text":form["max_output_tokens"].as_u64().unwrap().to_string()}),
                click(if story.state.starts_with("protocol") {
                    "model-api-select"
                } else {
                    "profile-model"
                }),
            ];
        }
    }
    items
}

pub struct StoryHost {
    inner: AnyView,
    settings: bool,
    #[cfg(not(target_family = "wasm"))]
    _directory: tempfile::TempDir,
}
impl StoryHost {
    pub fn inspect(&self, cx: &gpui::App) -> Value {
        if let Ok(view) = self.inner.clone().downcast::<zork_ui::new_chat::Page>() {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<zork_ui::settings::data::DataSettings>()
        {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<zork_ui::liquid_story::ComposerExample>()
        {
            return view.read(cx).inspect();
        }
        if let Ok(view) = self.inner.clone().downcast::<PrimitiveStory>() {
            return view.read(cx).inspect(cx);
        }
        if let Ok(view) = self.inner.clone().downcast::<ProfilesView>() {
            return view.read(cx).headless_state(cx);
        }
        if let Ok(view) = self
            .inner
            .clone()
            .downcast::<super::interaction_story::InteractionStory>()
        {
            return view.read(cx).inspect();
        }
        json!({})
    }

    pub fn new(story: Story, cx: &mut Context<Self>) -> Self {
        install(cx);
        #[cfg(not(target_family = "wasm"))]
        let directory = tempfile::tempdir().expect("isolated story directory");
        let settings = matches!(
            story.family.as_str(),
            "connection" | "model" | "client" | "device" | "mesh" | "enrollment"
        );
        let inner = match story.family.as_str() {
            "new-chat" => new_chat_story(&story.state, story.width, cx).into(),
            "node-directory" => cx
                .new(|cx| zork_ui::node_directory::Story::new(&story.state, cx))
                .into(),
            "history" => cx
                .new(|cx| {
                    zork_ui::history_page::stories::Story::with_text(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "conversation" if story.state.starts_with("history") => cx
                .new(|cx| {
                    zork_ui::history_page::stories::Story::with_text(
                        "compact",
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "history-details" => cx
                .new(|cx| {
                    zork_ui::history_details::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "member-activity" => cx
                .new(|cx| {
                    zork_ui::member_activity::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "appearance" => cx
                .new(|cx| {
                    zork_ui::settings::appearance::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "data-settings" => cx
                .new(|cx| {
                    zork_ui::settings::data::DataSettings::new(
                        zork_ui::settings::data::Data {
                            busy: false,
                            error: (story.state == "error")
                                .then(|| "本机运行尚未结束，请稍后重试".into()),
                        },
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "browser" => cx
                .new(|cx| {
                    zork_ui::browser_chrome::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "composer" => cx.new(zork_ui::liquid_story::ComposerExample::new).into(),
            "attachment-viewer" => cx
                .new(|cx| {
                    zork_ui::attachment_viewer::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "conversation-files" => cx
                .new(|cx| {
                    zork_ui::conversation_contents::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "chat-navigation" => zork_ui::chat_navigation::stories::create(
                &story.state,
                zork_ui::resources::Text(std::rc::Rc::new(|key| {
                    crate::i18n::Locale::ZhCn.text(key).into()
                })),
                cx,
            )
            .into(),
            "message-reader" => cx
                .new(|cx| {
                    zork_ui::components::message_reader::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            "shared-files" => zork_ui::shared_files::stories::create(
                &story.state,
                zork_ui::resources::Text(std::rc::Rc::new(|key| {
                    crate::i18n::Locale::ZhCn.text(key).into()
                })),
                std::rc::Rc::new(|time| {
                    chrono::DateTime::from_timestamp_nanos(time)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                }),
                cx,
            )
            .into(),
            "resources" => zork_ui::resources::stories::create(
                &story.state,
                zork_ui::resources::Text(std::rc::Rc::new(|key| {
                    crate::i18n::Locale::ZhCn.text(key).into()
                })),
                cx,
            )
            .into(),
            "notifications" => cx
                .new(|cx| super::notification_story::NotificationStory::new(&story.state, cx))
                .into(),
            "message-interaction" => cx
                .new(|cx| super::interaction_story::InteractionStory::new(&story.state, cx))
                .into(),
            "mesh" | "enrollment" => cx
                .new(|cx| {
                    zork_ui::network::NetworkStory::new(
                        story.family.clone(),
                        story
                            .state
                            .trim_end_matches("-compact")
                            .trim_end_matches("-wide")
                            .into(),
                        zork_client_core::device_edit::validate_peer,
                        cx,
                    )
                })
                .into(),
            "client" | "device" => cx
                .new(|cx| {
                    zork_ui::settings::SettingsStory::new(
                        story.family.clone(),
                        story
                            .state
                            .trim_end_matches("-compact")
                            .trim_end_matches("-wide")
                            .into(),
                        zork_client_core::device_edit::validate_name,
                        cx,
                    )
                })
                .into(),
            "connection" => cx
                .new(|cx| ProfilesView::headless_fixture(false, cx))
                .into(),
            "model" => cx.new(|cx| ProfilesView::headless_fixture(true, cx)).into(),
            "conversation" => cx
                .new(|cx| {
                    zork_ui::components::message_row::stories::Story::new(
                        &story.state,
                        zork_ui::resources::Text(std::rc::Rc::new(|key| {
                            crate::i18n::Locale::ZhCn.text(key).into()
                        })),
                        cx,
                    )
                })
                .into(),
            _ => cx.new(|cx| PrimitiveStory::new(story, cx)).into(),
        };
        Self {
            inner,
            settings,
            #[cfg(not(target_family = "wasm"))]
            _directory: directory,
        }
    }
}
impl Render for StoryHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let inner = if self.settings {
            div()
                .size_full()
                .bg(rgb(ZORK_UI.palette.canvas))
                .child(ui::settings_content(self.inner.clone()))
        } else {
            div().size_full().child(self.inner.clone())
        };
        div()
            .id("story-component")
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .text_color(rgb(ZORK_UI.palette.text))
            .bg(rgb(ZORK_UI.palette.canvas))
            .child(inner)
            .automation(AutomationRole::Status, "组件画布")
    }
}
