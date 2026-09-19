//! Embedded Zork design assets. Legacy Cue resources remain registered for historical compatibility, not current product defaults.
use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbeddedAssets;
impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: Option<&'static [u8]> = match path {
            "icons/more-horizontal.svg" => {
                Some(include_bytes!("../assets/icons/more-horizontal.svg"))
            }
            "icons/copy.svg" => Some(include_bytes!("../assets/icons/copy.svg")),
            "icons/message-square.svg" => {
                Some(include_bytes!("../assets/icons/message-square.svg"))
            }
            "history/agents.svg" => Some(include_bytes!("../assets/history/agents.svg")),
            "history/assign.svg" => Some(include_bytes!("../assets/history/assign.svg")),
            "history/attachment.svg" => Some(include_bytes!("../assets/history/attachment.svg")),
            "history/browser.svg" => Some(include_bytes!("../assets/history/browser.svg")),
            "history/clock.svg" => Some(include_bytes!("../assets/history/clock.svg")),
            "history/end.svg" => Some(include_bytes!("../assets/history/end.svg")),
            "history/file-edit.svg" => Some(include_bytes!("../assets/history/file-edit.svg")),
            "history/file-read.svg" => Some(include_bytes!("../assets/history/file-read.svg")),
            "history/file-write.svg" => Some(include_bytes!("../assets/history/file-write.svg")),
            "history/generic-tool.svg" => {
                Some(include_bytes!("../assets/history/generic-tool.svg"))
            }
            "history/help.svg" => Some(include_bytes!("../assets/history/help.svg")),
            "history/history.svg" => Some(include_bytes!("../assets/history/history.svg")),
            "history/job.svg" => Some(include_bytes!("../assets/history/job.svg")),
            "history/notify.svg" => Some(include_bytes!("../assets/history/notify.svg")),
            "history/operations.svg" => Some(include_bytes!("../assets/history/operations.svg")),
            "history/receive.svg" => Some(include_bytes!("../assets/history/receive.svg")),
            "history/send.svg" => Some(include_bytes!("../assets/history/send.svg")),
            "history/stop.svg" => Some(include_bytes!("../assets/history/stop.svg")),
            "history/terminal.svg" => Some(include_bytes!("../assets/history/terminal.svg")),

            "avatars/portraits/cat.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/cat.svg"))
            }
            "avatars/portraits/bunny.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/bunny.svg"))
            }
            "avatars/portraits/bear.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/bear.svg"))
            }
            "avatars/portraits/fox.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/fox.svg"))
            }
            "avatars/portraits/panda.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/panda.svg"))
            }
            "avatars/portraits/chick.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/chick.svg"))
            }
            "avatars/portraits/dog.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/dog.svg"))
            }
            "avatars/portraits/owl.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/owl.svg"))
            }
            "avatars/portraits/koala.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/koala.svg"))
            }
            "avatars/portraits/penguin.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/penguin.svg"))
            }
            "avatars/portraits/deer.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/deer.svg"))
            }
            "avatars/portraits/octopus.svg" => {
                Some(include_bytes!("../assets/avatars/portraits/octopus.svg"))
            }
            "browser/globe.svg" => Some(include_bytes!("../assets/browser/globe.svg")),
            "browser/expand.svg" => Some(include_bytes!("../assets/browser/expand.svg")),
            "browser/restore.svg" => Some(include_bytes!("../assets/browser/restore.svg")),
            "browser/minimize.svg" => Some(include_bytes!("../assets/browser/minimize.svg")),
            "browser/more.svg" => Some(include_bytes!("../assets/browser/more.svg")),
            "browser/go.svg" => Some(include_bytes!("../assets/browser/go.svg")),
            "browser/download.svg" => Some(include_bytes!("../assets/browser/download.svg")),
            "loading/native-ring.svg" => Some(include_bytes!("../assets/loading/native-ring.svg")),
            "avatars/cat.svg" => Some(include_bytes!("../assets/avatars/cat.svg")),
            "avatars/bunny.svg" => Some(include_bytes!("../assets/avatars/bunny.svg")),
            "avatars/bear.svg" => Some(include_bytes!("../assets/avatars/bear.svg")),
            "avatars/fox.svg" => Some(include_bytes!("../assets/avatars/fox.svg")),
            "avatars/panda.svg" => Some(include_bytes!("../assets/avatars/panda.svg")),
            "avatars/chick.svg" => Some(include_bytes!("../assets/avatars/chick.svg")),
            "cue/circle-x.svg" => Some(include_bytes!("../assets/cue/circle-x.svg")),
            "cue/mascot.svg" => Some(include_bytes!("../assets/cue/mascot.svg")),
            "cue/arrow-up.svg" => Some(include_bytes!("../assets/cue/arrow-up.svg")),
            "cue/chevron-down.svg" => Some(include_bytes!("../assets/cue/chevron-down.svg")),
            "cue/folder1.svg" => Some(include_bytes!("../assets/cue/folder1.svg")),
            "cue/home.svg" => Some(include_bytes!("../assets/cue/home.svg")),
            "cue/list-checks.svg" => Some(include_bytes!("../assets/cue/list-checks.svg")),
            "cue/panel-left.svg" => Some(include_bytes!("../assets/cue/panel-left.svg")),
            "cue/plus.svg" => Some(include_bytes!("../assets/cue/plus.svg")),
            "cue/search.svg" => Some(include_bytes!("../assets/cue/search.svg")),
            "cue/settings-slider-horizontal.svg" => Some(include_bytes!(
                "../assets/cue/settings-slider-horizontal.svg"
            )),
            "cue/shapes-plus-x-square-circle.svg" => Some(include_bytes!(
                "../assets/cue/shapes-plus-x-square-circle.svg"
            )),
            "cue/sparkles.svg" => Some(include_bytes!("../assets/cue/sparkles.svg")),
            "cue/inbox.svg" => Some(include_bytes!("../assets/cue/inbox.svg")),
            "cue/folder2.svg" => Some(include_bytes!("../assets/cue/folder2.svg")),
            "cue/puzzle.svg" => Some(include_bytes!("../assets/cue/puzzle.svg")),
            "cue/settings.svg" => Some(include_bytes!("../assets/cue/settings.svg")),
            "cue/arrow-left.svg" => Some(include_bytes!("../assets/cue/arrow-left.svg")),
            "cue/arrow-right.svg" => Some(include_bytes!("../assets/cue/arrow-right.svg")),
            "cue/clock.svg" => Some(include_bytes!("../assets/cue/clock.svg")),
            "cue/panel-right.svg" => Some(include_bytes!("../assets/cue/panel-right.svg")),
            "cue/x.svg" => Some(include_bytes!("../assets/cue/x.svg")),
            "cue/reload.svg" => Some(include_bytes!("../assets/cue/reload.svg")),
            "cue/settings-slider-three.svg" => {
                Some(include_bytes!("../assets/cue/settings-slider-three.svg"))
            }
            "cue/microphone-filled.svg" => {
                Some(include_bytes!("../assets/cue/microphone-filled.svg"))
            }
            "cue/provider-logos.png" => Some(include_bytes!("../assets/cue/provider-logos.png")),
            "cue/paperclip.svg" => Some(include_bytes!("../assets/cue/paperclip.svg")),
            "cue/check.svg" => Some(include_bytes!("../assets/cue/check.svg")),
            "cue/filter2.svg" => Some(include_bytes!("../assets/cue/filter2.svg")),
            "cue/layout-column.svg" => Some(include_bytes!("../assets/cue/layout-column.svg")),
            "cue/bars-three.svg" => Some(include_bytes!("../assets/cue/bars-three.svg")),
            "cue/circle-dashed.svg" => Some(include_bytes!("../assets/cue/circle-dashed.svg")),
            "cue/loader.svg" => Some(include_bytes!("../assets/cue/loader.svg")),
            "cue/file.svg" => Some(include_bytes!("../assets/cue/file.svg")),
            "cue/download.svg" => Some(include_bytes!("../assets/cue/download.svg")),
            "avatars/owl.svg" => Some(include_bytes!("../assets/avatars/owl.svg")),
            "avatars/octopus.svg" => Some(include_bytes!("../assets/avatars/octopus.svg")),
            "avatars/deer.svg" => Some(include_bytes!("../assets/avatars/deer.svg")),
            "avatars/dog.svg" => Some(include_bytes!("../assets/avatars/dog.svg")),
            "avatars/koala.svg" => Some(include_bytes!("../assets/avatars/koala.svg")),
            "avatars/penguin.svg" => Some(include_bytes!("../assets/avatars/penguin.svg")),
            "icons/task.svg" => Some(include_bytes!("../assets/icons/task.svg")),
            "icons/offline.svg" => Some(include_bytes!("../assets/icons/offline.svg")),
            "icons/result.svg" => Some(include_bytes!("../assets/icons/result.svg")),
            "icons/cancelled.svg" => Some(include_bytes!("../assets/icons/cancelled.svg")),
            "icons/review.svg" => Some(include_bytes!("../assets/icons/review.svg")),
            "icons/phosphor-brain.svg" => {
                Some(include_bytes!("../assets/icons/phosphor-brain.svg"))
            }
            "icons/leader.svg" => Some(include_bytes!("../assets/icons/leader.svg")),
            "icons/working.svg" => Some(include_bytes!("../assets/icons/working.svg")),
            "icons/attention.svg" => Some(include_bytes!("../assets/icons/attention.svg")),
            "icons/phosphor-arrow-up.svg" => {
                Some(include_bytes!("../assets/icons/phosphor-arrow-up.svg"))
            }
            "icons/phosphor-cube.svg" => Some(include_bytes!("../assets/icons/phosphor-cube.svg")),
            "icons/phosphor-caret-down.svg" => {
                Some(include_bytes!("../assets/icons/phosphor-caret-down.svg"))
            }
            "icons/permission.svg" => Some(include_bytes!("../assets/icons/permission.svg")),
            "icons/open.svg" => Some(include_bytes!("../assets/icons/open.svg")),
            "icons/mesh.svg" => Some(include_bytes!("../assets/icons/mesh.svg")),
            "icons/phosphor-terminal-window.svg" => Some(include_bytes!(
                "../assets/icons/phosphor-terminal-window.svg"
            )),
            "icons/workspace.svg" => Some(include_bytes!("../assets/icons/workspace.svg")),
            "icons/phosphor-stop-fill.svg" => {
                Some(include_bytes!("../assets/icons/phosphor-stop-fill.svg"))
            }
            "icons/worker.svg" => Some(include_bytes!("../assets/icons/worker.svg")),
            "icons/handoff.svg" => Some(include_bytes!("../assets/icons/handoff.svg")),
            "icons/phosphor-folder-simple.svg" => {
                Some(include_bytes!("../assets/icons/phosphor-folder-simple.svg"))
            }
            "icons/queued.svg" => Some(include_bytes!("../assets/icons/queued.svg")),
            "icons/completed.svg" => Some(include_bytes!("../assets/icons/completed.svg")),
            "icons/history.svg" => Some(include_bytes!("../assets/icons/history.svg")),
            "icons/node.svg" => Some(include_bytes!("../assets/icons/node.svg")),
            "brand/lockup.svg" => Some(include_bytes!("../assets/brand/lockup.svg")),
            "brand/mark.svg" => Some(include_bytes!("../assets/brand/mark.svg")),
            "providers/githubcopilot.svg" => {
                Some(include_bytes!("../assets/providers/githubcopilot.svg"))
            }
            "providers/xai.svg" => Some(include_bytes!("../assets/providers/xai.svg")),
            "providers/compatible.svg" => {
                Some(include_bytes!("../assets/providers/compatible.svg"))
            }
            "providers/openai.svg" => Some(include_bytes!("../assets/providers/openai.svg")),
            "providers/kimi.svg" => Some(include_bytes!("../assets/providers/kimi.svg")),
            "providers/opencode.svg" => Some(include_bytes!("../assets/providers/opencode.svg")),
            "providers/anthropic.svg" => Some(include_bytes!("../assets/providers/anthropic.svg")),
            "providers/openrouter.svg" => {
                Some(include_bytes!("../assets/providers/openrouter.svg"))
            }
            "brand/zork-wordmark.svg" => Some(include_bytes!("../assets/brand/zork-wordmark.svg")),
            "icons/columns.svg" => Some(include_bytes!("../assets/icons/columns.svg")),
            "icons/search.svg" => Some(include_bytes!("../assets/icons/search.svg")),
            "icons/bars-three.svg" => Some(include_bytes!("../assets/icons/bars-three.svg")),
            "icons/home.svg" => Some(include_bytes!("../assets/icons/home.svg")),
            "icons/chevron-down.svg" => Some(include_bytes!("../assets/icons/chevron-down.svg")),
            "icons/task-chat.svg" => Some(include_bytes!("../assets/icons/task-chat.svg")),
            "icons/inbox.svg" => Some(include_bytes!("../assets/icons/inbox.svg")),
            "icons/microphone.svg" => Some(include_bytes!("../assets/icons/microphone.svg")),
            "icons/file.svg" => Some(include_bytes!("../assets/icons/file.svg")),
            "icons/puzzle.svg" => Some(include_bytes!("../assets/icons/puzzle.svg")),
            "icons/brain.svg" => Some(include_bytes!("../assets/icons/brain.svg")),
            "icons/terminal.svg" => Some(include_bytes!("../assets/icons/terminal.svg")),
            "icons/sparkles.svg" => Some(include_bytes!("../assets/icons/sparkles.svg")),
            "icons/x.svg" => Some(include_bytes!("../assets/icons/x.svg")),
            "icons/arrow-right.svg" => Some(include_bytes!("../assets/icons/arrow-right.svg")),
            "icons/settings.svg" => Some(include_bytes!("../assets/icons/settings.svg")),
            "icons/edit.svg" => Some(include_bytes!("../assets/icons/edit.svg")),
            "icons/download.svg" => Some(include_bytes!("../assets/icons/download.svg")),
            "icons/shapes.svg" => Some(include_bytes!("../assets/icons/shapes.svg")),
            "icons/stop.svg" => Some(include_bytes!("../assets/icons/stop.svg")),
            "icons/cube.svg" => Some(include_bytes!("../assets/icons/cube.svg")),
            "icons/plus.svg" => Some(include_bytes!("../assets/icons/plus.svg")),
            "icons/minus.svg" => Some(include_bytes!("../assets/icons/minus.svg")),
            "icons/check.svg" => Some(include_bytes!("../assets/icons/check.svg")),
            "icons/loader.svg" => Some(include_bytes!("../assets/icons/loader.svg")),
            "icons/reload.svg" => Some(include_bytes!("../assets/icons/reload.svg")),
            "icons/panel-left.svg" => Some(include_bytes!("../assets/icons/panel-left.svg")),
            "icons/paperclip.svg" => Some(include_bytes!("../assets/icons/paperclip.svg")),
            "icons/settings-three.svg" => {
                Some(include_bytes!("../assets/icons/settings-three.svg"))
            }
            "icons/checklist.svg" => Some(include_bytes!("../assets/icons/checklist.svg")),
            "icons/clock.svg" => Some(include_bytes!("../assets/icons/clock.svg")),
            "icons/arrow-up.svg" => Some(include_bytes!("../assets/icons/arrow-up.svg")),
            "icons/panel-right.svg" => Some(include_bytes!("../assets/icons/panel-right.svg")),
            "icons/filter.svg" => Some(include_bytes!("../assets/icons/filter.svg")),
            "icons/arrow-left.svg" => Some(include_bytes!("../assets/icons/arrow-left.svg")),
            "icons/mention.svg" => Some(include_bytes!("../assets/icons/mention.svg")),
            "brand/mark-micro.svg" => Some(include_bytes!("../assets/brand/mark-micro.svg")),
            "brand/mark-orange.svg" => Some(include_bytes!("../assets/brand/mark-orange.svg")),
            "brand/mark-reverse.svg" => Some(include_bytes!("../assets/brand/mark-reverse.svg")),
            "brand/zork-wordmark-draft.svg" => {
                Some(include_bytes!("../assets/brand/zork-wordmark-draft.svg"))
            }
            "illustrations/review-ready.svg" => {
                Some(include_bytes!("../assets/illustrations/review-ready.svg"))
            }
            "illustrations/device-offline.svg" => {
                Some(include_bytes!("../assets/illustrations/device-offline.svg"))
            }
            "illustrations/inbox-clear.svg" => {
                Some(include_bytes!("../assets/illustrations/inbox-clear.svg"))
            }
            "illustrations/first-conversation.svg" => Some(include_bytes!(
                "../assets/illustrations/first-conversation.svg"
            )),
            "illustrations/hero.svg" => Some(include_bytes!("../assets/illustrations/hero.svg")),
            "illustrations/files-empty.svg" => {
                Some(include_bytes!("../assets/illustrations/files-empty.svg"))
            }
            "illustrations/task-accepted.svg" => {
                Some(include_bytes!("../assets/illustrations/task-accepted.svg"))
            }
            "illustrations/node-setup.svg" => {
                Some(include_bytes!("../assets/illustrations/node-setup.svg"))
            }
            "motion/wordmark.svg" => Some(include_bytes!("../assets/motion/wordmark.svg")),
            "motion/icon.svg" => Some(include_bytes!("../assets/motion/icon.svg")),
            "motion/linked.svg" => Some(include_bytes!("../assets/motion/linked.svg")),
            "motion/icon-to-wordmark.svg" => {
                Some(include_bytes!("../assets/motion/icon-to-wordmark.svg"))
            }
            "providers/provider-logos.png" => {
                Some(include_bytes!("../assets/providers/provider-logos.png"))
            }
            _ => None,
        };
        Ok(bytes.map(Cow::Borrowed))
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path == "history" {
            return Ok(vec![
                "agents.svg".into(),
                "assign.svg".into(),
                "attachment.svg".into(),
                "browser.svg".into(),
                "clock.svg".into(),
                "end.svg".into(),
                "file-edit.svg".into(),
                "file-read.svg".into(),
                "file-write.svg".into(),
                "generic-tool.svg".into(),
                "help.svg".into(),
                "history.svg".into(),
                "job.svg".into(),
                "notify.svg".into(),
                "operations.svg".into(),
                "receive.svg".into(),
                "send.svg".into(),
                "stop.svg".into(),
                "terminal.svg".into(),
            ]);
        }

        if path == "avatars" {
            return Ok(vec![
                "cat.svg".into(),
                "bunny.svg".into(),
                "bear.svg".into(),
                "fox.svg".into(),
                "panda.svg".into(),
                "chick.svg".into(),
                "dog.svg".into(),
                "owl.svg".into(),
                "koala.svg".into(),
                "penguin.svg".into(),
                "deer.svg".into(),
                "octopus.svg".into(),
            ]);
        }
        if path == "icons" {
            return Ok(vec![
                "more-horizontal.svg".into(),
                "arrow-left.svg".into(),
                "arrow-right.svg".into(),
                "arrow-up.svg".into(),
                "attention.svg".into(),
                "bars-three.svg".into(),
                "brain.svg".into(),
                "cancelled.svg".into(),
                "check.svg".into(),
                "checklist.svg".into(),
                "chevron-down.svg".into(),
                "clock.svg".into(),
                "columns.svg".into(),
                "completed.svg".into(),
                "cube.svg".into(),
                "download.svg".into(),
                "edit.svg".into(),
                "file.svg".into(),
                "filter.svg".into(),
                "handoff.svg".into(),
                "history.svg".into(),
                "home.svg".into(),
                "inbox.svg".into(),
                "leader.svg".into(),
                "loader.svg".into(),
                "mention.svg".into(),
                "mesh.svg".into(),
                "microphone.svg".into(),
                "minus.svg".into(),
                "node.svg".into(),
                "offline.svg".into(),
                "open.svg".into(),
                "panel-left.svg".into(),
                "panel-right.svg".into(),
                "paperclip.svg".into(),
                "permission.svg".into(),
                "plus.svg".into(),
                "puzzle.svg".into(),
                "queued.svg".into(),
                "reload.svg".into(),
                "result.svg".into(),
                "review.svg".into(),
                "search.svg".into(),
                "settings-three.svg".into(),
                "settings.svg".into(),
                "shapes.svg".into(),
                "sparkles.svg".into(),
                "stop.svg".into(),
                "task-chat.svg".into(),
                "task.svg".into(),
                "terminal.svg".into(),
                "worker.svg".into(),
                "working.svg".into(),
                "workspace.svg".into(),
                "x.svg".into(),
            ]);
        }
        if path == "brand" {
            return Ok(vec![
                "lockup.svg".into(),
                "mark-micro.svg".into(),
                "mark-orange.svg".into(),
                "mark-reverse.svg".into(),
                "mark.svg".into(),
                "zork-wordmark-draft.svg".into(),
                "zork-wordmark.svg".into(),
            ]);
        }
        if path == "providers" {
            return Ok(vec![
                "anthropic.svg".into(),
                "compatible.svg".into(),
                "githubcopilot.svg".into(),
                "kimi.svg".into(),
                "openai.svg".into(),
                "opencode.svg".into(),
                "openrouter.svg".into(),
                "xai.svg".into(),
            ]);
        }
        if path == "illustrations" {
            return Ok(vec![
                "device-offline.svg".into(),
                "files-empty.svg".into(),
                "first-conversation.svg".into(),
                "hero.svg".into(),
                "inbox-clear.svg".into(),
                "node-setup.svg".into(),
                "review-ready.svg".into(),
                "task-accepted.svg".into(),
            ]);
        }
        if path == "motion" {
            return Ok(vec![
                "icon-to-wordmark.svg".into(),
                "icon.svg".into(),
                "linked.svg".into(),
                "wordmark.svg".into(),
            ]);
        }
        if path != "cue" {
            return Ok(Vec::new());
        }
        Ok(vec![
            "home.svg".into(),
            "list-checks.svg".into(),
            "folder1.svg".into(),
            "arrow-up.svg".into(),
            "chevron-down.svg".into(),
            "sparkles.svg".into(),
            "shapes-plus-x-square-circle.svg".into(),
            "panel-left.svg".into(),
            "plus.svg".into(),
            "search.svg".into(),
            "settings-slider-horizontal.svg".into(),
            "inbox.svg".into(),
            "folder2.svg".into(),
            "puzzle.svg".into(),
            "settings.svg".into(),
            "arrow-left.svg".into(),
            "arrow-right.svg".into(),
            "clock.svg".into(),
            "panel-right.svg".into(),
            "x.svg".into(),
            "reload.svg".into(),
            "settings-slider-three.svg".into(),
            "microphone-filled.svg".into(),
            "paperclip.svg".into(),
            "check.svg".into(),
            "filter2.svg".into(),
            "layout-column.svg".into(),
            "bars-three.svg".into(),
            "circle-dashed.svg".into(),
            "loader.svg".into(),
            "file.svg".into(),
            "download.svg".into(),
        ])
    }
}
/// Embedded monospace face shared by native and Web message rendering.
pub const CODE_FONT_FAMILY: &str = "JetBrains Mono";
pub const CODE_FONT: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// Register static UI faces and the portable code font.
/// Static instances preserve the design while avoiding CoreText variable-font
/// fallback setup on every newly shaped line.
pub fn init_fonts(cx: &gpui::App) {
    cx.text_system()
        .add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/static/Inter-400.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/static/Inter-500.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/static/Inter-600.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/static/Inter-700.ttf")),
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/static/Inter-400-Italic.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/static/Inter-500-Italic.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/static/Inter-600-Italic.ttf"
            )),
            Cow::Borrowed(include_bytes!(
                "../assets/fonts/static/Inter-700-Italic.ttf"
            )),
            Cow::Borrowed(CODE_FONT),
        ])
        .expect("failed to load embedded UI fonts");
    #[cfg(target_os = "macos")]
    {
        let text_system = cx.text_system().clone();
        cx.background_executor()
            .spawn(async move {
                gpui::observe_startup("gpui.text_prepare_begin");
                // CoreText family selection and the Latin/CJK fallback shaper are
                // shared by all windows. Initialize them while AppKit builds the
                // native window; no frame or view is created by this worker.
                let text_system = gpui::WindowTextSystem::new(text_system);
                let sample: gpui::SharedString = "Aa中".into();
                for family in [".SystemUIFont", "Inter Variable"] {
                    for weight in [
                        gpui::FontWeight::NORMAL,
                        gpui::FontWeight::MEDIUM,
                        gpui::FontWeight::SEMIBOLD,
                    ] {
                        let mut font = gpui::font(family);
                        font.weight = weight;
                        let _ = text_system.shape_line(
                            sample.clone(),
                            gpui::px(13.),
                            &[gpui::TextRun {
                                len: sample.len(),
                                font,
                                color: gpui::Hsla::black(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }],
                            None,
                        );
                    }
                }
                gpui::observe_startup("gpui.text_prepare_ready");
            })
            .detach();
    }
}
