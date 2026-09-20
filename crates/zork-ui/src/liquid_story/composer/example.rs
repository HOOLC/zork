//! Mock input/event adapter for the production composer scene and renderers.
use super::*;
use crate::{
    attachment_viewer as preview, components::tooltip::DetailsTooltip, design::ZORK_UI,
    member_activity, resources::Text,
};
use std::{collections::HashMap, sync::Arc};
const VARIANTS: &[&str] = &[
    "空白",
    "可发送",
    "运行中",
    "停止中",
    "只读",
    "附件准备中",
    "多人活动",
    "任务评论",
];
pub struct Example {
    controller: Box<dyn Controller>,
    snapshot: Snapshot,
    input: Entity<ComposerInput>,
    scene: view::Scene,
    frame: Option<Instant>,
    scheduled: bool,
    fan: Spring,
    pinned: bool,
    variant: usize,
    variants_open: bool,
    selector: liquid::overlay::Popover,
    details: Entity<member_activity::Overlay>,
    anchors: HashMap<String, Bounds<Pixels>>,
    hovered: Option<String>,
    viewer: Entity<preview::Viewer>,
    preview_id: Option<u64>,
    text: Text,
    labels: Vec<(String, f32)>,
    last_action: String,
    available_width: f32,
}
impl Example {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let text = cx
            .try_global::<crate::history_page::stories::StoryText>()
            .map(|v| v.0.clone())
            .unwrap_or_else(|| Text(Rc::new(str::to_owned)));
        let mut controller = (cx.global::<Factory>().0)();
        let snapshot = controller.apply(Intent::Inspect);
        let input = cx.new(|cx| ComposerInput::new(text.text("composer_placeholder"), cx));
        cx.subscribe(
            &input,
            |v, input, _: &crate::components::text_input::ComposerEdited, cx| {
                let value = input.read(cx).value().to_owned();
                v.apply(Intent::Edit(value), liquid::departure::Origin::Composer, cx);
            },
        )
        .detach();
        cx.subscribe(&input, |v, _, _: &ComposerSubmit, cx| {
            v.apply(Intent::Submit, liquid::departure::Origin::Composer, cx)
        })
        .detach();
        cx.subscribe(&input, |_, _, _: &ComposerLayoutChanged, cx| cx.notify())
            .detach();
        let details = cx.new(|_| member_activity::Overlay::default());
        let viewer = cx.new(|cx| preview::Viewer::new(text.clone(), cx));
        cx.subscribe(&viewer, |v, _, action: &preview::Action, cx| {
            match action {
                preview::Action::Close => v.preview_id = None,
                preview::Action::Reuse => {
                    if let Some(file) = v.snapshot.files.iter().find(|f| Some(f.id) == v.preview_id)
                    {
                        v.apply(
                            Intent::Files(vec![file.name.clone()]),
                            liquid::departure::Origin::Button,
                            cx,
                        );
                    }
                }
                _ => {}
            }
            v.configure_preview(cx);
            cx.notify();
        })
        .detach();
        Self {
            controller,
            snapshot,
            input,
            scene: Default::default(),
            frame: None,
            scheduled: false,
            fan: Spring::new(0.),
            pinned: false,
            variant: 0,
            variants_open: false,
            selector: liquid::overlay::Popover::new(cx),
            details,
            anchors: Default::default(),
            hovered: None,
            viewer,
            preview_id: None,
            text,
            labels: vec![],
            last_action: String::new(),
            available_width: 240.,
        }
    }
    fn apply(&mut self, intent: Intent, origin: liquid::departure::Origin, cx: &mut Context<Self>) {
        let before = self.snapshot.events.len();
        let text = self.snapshot.text.clone();
        self.snapshot = self.controller.apply(intent);
        if self.snapshot.events.len() > before
            && self
                .snapshot
                .events
                .last()
                .is_some_and(|event| event.starts_with("send:"))
        {
            self.scene.accepted(origin, &text);
        }
        self.input.update(cx, |input, cx| {
            input.set_value(self.snapshot.text.clone(), cx);
            input.set_editable(!self.snapshot.capabilities.editable, false, cx);
        });
        let ids = self
            .snapshot
            .members
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        self.anchors.retain(|id, _| ids.contains(id));
        self.details.update(cx, |v, cx| v.retain(&ids, cx));
        if self.snapshot.files.is_empty() {
            self.pinned = false;
            self.fan.target = 0.;
        }
        cx.notify();
    }
    fn configure_preview(&mut self, cx: &mut Context<Self>) {
        let file = self
            .snapshot
            .files
            .iter()
            .find(|file| Some(file.id) == self.preview_id);
        let data = file
            .map(|file| {
                let text: Arc<str> =
                    format!("# {}\n\nPlayground 附件的 mock 内容。", file.name).into();
                let info = preview::Info {
                    id: file.id.to_string(),
                    name: file.name.clone(),
                    subtitle: "演示附件".into(),
                    image_view: false,
                    source_path: file.name.clone(),
                    version: 1,
                    created_at: "2026-09-15".into(),
                };
                preview::Data {
                    info: Some(info.clone()),
                    group: Arc::new(vec![info]),
                    loaded: true,
                    can_reuse: true,
                    document: Some(MessageDocument::parse(&text)),
                    text: Some(text),
                    ..Default::default()
                }
            })
            .unwrap_or_default();
        self.viewer
            .update(cx, |view, cx| view.set_data(data, self.text.clone(), cx));
    }
    fn action(&mut self, action: view::Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            view::Action::FocusEditor => window.focus(&self.input.read(cx).focus_handle(), cx),
            view::Action::Primary => {
                self.apply(Intent::Primary, liquid::departure::Origin::Button, cx)
            }
            // Mock platform capability returns fixed file metadata. Production
            // supplies the same intent from its real file chooser.
            view::Action::ChooseFiles => self.apply(
                Intent::Files(vec!["组件说明.md".into()]),
                liquid::departure::Origin::Button,
                cx,
            ),
            view::Action::RemoveFile(id) => self.apply(
                Intent::RemoveFile(id),
                liquid::departure::Origin::Button,
                cx,
            ),
            view::Action::OpenFile(id) => {
                self.preview_id = Some(id);
                self.configure_preview(cx);
            }
            view::Action::FanHover(hover) => {
                self.fan.target = if hover || self.pinned { 1. } else { 0. };
            }
            view::Action::ToggleFan => {
                self.pinned = !self.pinned;
                self.fan.target = if self.pinned { 1. } else { 0. };
            }
            view::Action::MemberAnchor(id, anchor) => {
                if self.anchors.insert(id.clone(), anchor) == Some(anchor) {
                    return;
                }
                self.details.update(cx, |v, cx| v.anchor(&id, anchor, cx));
            }
            view::Action::MemberHover(Some(id)) => {
                self.hovered = Some(id.clone());
                if let (Some(member), Some(anchor)) = (
                    self.snapshot.members.iter().find(|m| m.id == id),
                    self.anchors.get(&id).copied(),
                ) {
                    let details = DetailsTooltip {
                        key: format!("presence-{id}"),
                        title: member.label.clone(),
                        kind: self.text.text("presence_member"),
                        avatar: Some(member.avatar.clone()),
                        description: member.label.clone(),
                        rows: vec![(self.text.text("model"), "演示模型".into())],
                    };
                    self.details.update(cx, |v, cx| {
                        v.show(
                            id,
                            details,
                            self.text.text("presence_history_hint"),
                            anchor,
                            cx,
                        )
                    });
                }
            }
            view::Action::MemberHover(None) => {
                if let Some(id) = self.hovered.take() {
                    self.details.update(cx, |v, cx| v.leave(&id, cx));
                }
            }
            view::Action::Member(id) => self.last_action = format!("查看成员历史：{id}"),
        }
        cx.notify();
    }
    pub fn inspect(&self) -> Value {
        json!({"composer": self.snapshot, "material": self.scene.inspect(), "variant": self.variant, "fan": self.fan.position, "selector": self.selector.inspect()})
    }
}
impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let elapsed = self
            .frame
            .replace(now)
            .map_or(0., |prior| {
                now.saturating_duration_since(prior).as_secs_f64()
            })
            .min(0.05);
        if cx.reduce_motion() {
            self.fan.snap();
        } else {
            self.fan.step(elapsed, 18., 0.82);
            if self.fan.near(0.001, 0.01) {
                self.fan.snap();
            }
        }
        let width = self.available_width.clamp(1., 900.);
        let height = self
            .input
            .read(cx)
            .content_height()
            .unwrap_or(view::EDITOR_MIN)
            .clamp(view::EDITOR_MIN, view::EDITOR_MAX);
        let body = view::body(width, height);
        if self.labels.iter().map(|(label, _)| label).ne(self
            .snapshot
            .members
            .iter()
            .map(|member| &member.label))
        {
            self.labels = self
                .snapshot
                .members
                .iter()
                .map(|member| {
                    let run = TextRun {
                        len: member.label.len(),
                        font: font("Inter Variable"),
                        color: rgb(ZORK_UI.palette.text).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let size = window
                        .text_system()
                        .shape_line(member.label.clone().into(), px(12.), &[run], None)
                        .width
                        .as_f32();
                    (member.label.clone(), size)
                })
                .collect();
        }
        let widths: Vec<_> = self.labels.iter().map(|(_, width)| *width).collect();
        let targets = view::poses(body, &self.snapshot, &widths)
            .into_iter()
            .skip(1)
            .zip(&self.snapshot.members)
            .map(|(pose, member)| (member.id.clone(), pose))
            .collect::<Vec<_>>();
        let moving = self.scene.frame(
            body,
            &targets,
            view::opening(body, self.snapshot.files.len(), self.fan.position as f32),
            elapsed,
            cx.reduce_motion(),
        );
        if (moving || !self.fan.near(0.001, 0.01)) && !self.scheduled {
            self.scheduled = true;
            let owner = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = owner.update(cx, |v, cx| {
                    v.scheduled = false;
                    cx.notify();
                });
            });
        }
        let owner = cx.entity().downgrade();
        let handler: view::Handler = Rc::new(move |action, window, cx| {
            let _ = owner.update(cx, |v, cx| v.action(action, window, cx));
        });
        let bubbles = self.scene.bubbles();
        let composer = view::render(
            view::Props {
                id: "liquid-composer",
                surface: self.scene.surface.as_ref().unwrap(),
                width,
                height: 280.,
                editor: &self.input,
                snapshot: &self.snapshot,
                fan_progress: self.fan.position as f32,
                fan_pinned: self.pinned,
                bubbles: &bubbles,
                handler,
                presentation: Some(view::Presentation {
                    editor_id: "liquid-composer-editor".into(),
                    attach_id: "liquid-composer-attach".into(),
                    show_attach: true,
                    primary_id: "liquid-composer-send".into(),
                    member_groups: self
                        .scene
                        .indices(self.snapshot.members.iter().map(|m| &m.id)),
                    member_colors: vec![ZORK_UI.palette.text; self.snapshot.members.len()],
                    member_names: self.snapshot.members.iter().map(|m| m.id.clone()).collect(),
                    fan: None,
                    busy: self.variant == 3,
                    editor_label: self.text.text("composer_placeholder").into(),
                    attach_label: self.text.text("add_files").into(),
                    primary_label: self
                        .text
                        .text(if self.snapshot.capabilities.stop {
                            "stop_task"
                        } else {
                            "send_message"
                        })
                        .into(),
                }),
            },
            window,
            cx,
        );
        use liquid::overlay::{Choice, Placement, Selection, Trigger};
        let width = self
            .selector
            .trigger_width(VARIANTS[self.variant], Trigger::Field, window);
        let variants = crate::components::workbench::slot(width).child(
            self.selector.render(
                "liquid-composer-variant-select",
                VARIANTS[self.variant].to_owned(),
                VARIANTS
                    .iter()
                    .enumerate()
                    .map(|(index, label)| Choice {
                        id: format!("liquid-composer-variant-{index}"),
                        label: (*label).into(),
                        checked: Some(index == self.variant),
                        disabled: false,
                    })
                    .collect(),
                Selection::Single,
                Trigger::Field,
                self.variants_open,
                true,
                Placement::Window { width },
                Material::default(),
                window,
                cx,
                |v, open, _, cx| {
                    v.variants_open = open;
                    cx.notify();
                },
                |v, index, _, cx| {
                    v.variant = index;
                    v.variants_open = false;
                    v.apply(
                        Intent::Scenario(index),
                        liquid::departure::Origin::Button,
                        cx,
                    );
                },
            ),
        );
        let owner = cx.entity().downgrade();
        crate::components::workbench::column(12.)
            .relative()
            .min_w_0()
            .child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = owner.update(cx, |v, cx| {
                            let next = bounds.size.width.as_f32();
                            if (v.available_width - next).abs() > 0.1 {
                                v.available_width = next;
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(variants)
            .child(composer)
            .when(!self.last_action.is_empty(), |v| {
                v.child(crate::components::workbench::description(
                    self.last_action.clone(),
                ))
            })
            .child(self.details.clone())
            .child(self.viewer.clone())
    }
}
