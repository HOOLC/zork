use super::*;
use crate::components::workbench as wb;
pub struct Story {
    viewer: Entity<Viewer>,
    data: Data,
    locale: Text,
    thumbnail: Option<String>,
}
impl Story {
    pub fn new(state: &str, locale: Text, cx: &mut Context<Self>) -> Self {
        let viewer = cx.new(|cx| Viewer::new(locale.clone(), cx));
        let image_view = state == "image";
        let document_text = if state == "long" {
            "# 设计说明\n\n共享组件使用相同的参数与交互合同。\n\n".repeat(50)
        } else {
            "# 设计说明\n\n支持 **格式化阅读**、原文切换和复制。\n\n```rust\nlet shared = true;\n```".into()
        };
        let image = image_view.then(|| {
            let renderer = cx.svg_renderer();
            let svg = Arc::new(
                renderer
                    .parse_svg(include_bytes!("../../assets/avatars/portraits/fox.svg"))
                    .expect("fixture SVG"),
            );
            let rendered = renderer.render_parsed(&svg, 1.).expect("fixture raster");
            let dimensions = rendered.size(0);
            DecodedImage {
                rendered,
                size: size(
                    i32::from(dimensions.width) as f32 / 2.,
                    i32::from(dimensions.height) as f32 / 2.,
                ),
                svg: Some(svg),
            }
        });
        let info = Info {
            id: "demo-file".into(),
            name: if image_view {
                "角色插图.svg"
            } else {
                "设计说明.md"
            }
            .into(),
            subtitle: if image_view {
                "SVG · 512 × 512"
            } else {
                "Markdown · 12 KB"
            }
            .into(),
            image_view,
            source_path: "项目资料".into(),
            version: 2,
            created_at: "2026-09-15".into(),
        };
        let data = Data {
            info: Some(info.clone()),
            group: Arc::new(vec![info]),
            loaded: !matches!(state, "loading" | "error"),
            failed: state == "error",
            image,
            text: (!image_view).then(|| Arc::from(document_text.clone())),
            document: (!image_view).then(|| MessageDocument::parse(&document_text)),
            source_document: (!image_view).then(|| MessageDocument::plain(&document_text)),
            can_reuse: true,
            ..Default::default()
        };
        cx.subscribe(&viewer, |v, _, action: &Action, cx| {
            match action {
                Action::Close => return,
                Action::Save => v.data.notice = Some("drive_saved"),
                Action::Retry => {
                    v.data.failed = false;
                    v.data.loaded = true;
                }
                Action::Reuse => v.data.notice = Some("preview_added_to_draft"),
                Action::Move(_) => {}
            }
            v.viewer.update(cx, |viewer, cx| {
                viewer.set_data(v.data.clone(), v.locale.clone(), cx)
            });
        })
        .detach();
        Self {
            viewer,
            data,
            locale,
            thumbnail: None,
        }
    }
    pub fn thumbnail(state: &str, locale: Text, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(
            if matches!(state, "image" | "message-image") {
                "image"
            } else if matches!(state, "loading" | "error") {
                state
            } else {
                "document"
            },
            locale,
            cx,
        );
        view.thumbnail = Some(state.into());
        view
    }
    fn open(&self, cx: &mut Context<Self>) {
        self.viewer.update(cx, |viewer, cx| {
            viewer.set_data(self.data.clone(), self.locale.clone(), cx)
        });
    }
}
impl Render for Story {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source = self.viewer.read(cx).source();
        let trigger = if let Some(state) = self.thumbnail.as_deref() {
            let name = self.data.info.as_ref().unwrap().name.clone();
            match state {
                "message-image" => crate::components::attachments::message_image(
                    "story-attachment",
                    self.data.image.as_ref().map(|image| image.rendered.clone()),
                    144.,
                )
                .on_click(cx.listener(|v, _, _, cx| v.open(cx)))
                .map(|trigger| source.bind(trigger, name.clone(), ui::ActionStyle::default()))
                .automation(AutomationRole::Button, name.clone())
                .into_any_element(),
                "message-document" => crate::components::attachments::message_document(
                    "story-attachment",
                    name.clone(),
                    "Markdown".into(),
                    300.,
                )
                .on_click(cx.listener(|v, _, _, cx| v.open(cx)))
                .map(|trigger| source.bind(trigger, name.clone(), ui::ActionStyle::default()))
                .automation(AutomationRole::Button, name.clone())
                .into_any_element(),
                "row" => crate::components::attachments::row_source(
                    "story-attachment",
                    name.clone(),
                    "12 KB · v2".into(),
                    Some(source.clone()),
                    cx,
                    |v, cx| v.open(cx),
                )
                .into_any_element(),
                _ => crate::components::attachments::card_source(
                    "story-attachment",
                    name.clone(),
                    match state {
                        "loading" => "正在加载…",
                        "error" => "加载失败，点击重试",
                        "unavailable" => "此设备暂时离线",
                        _ => "12 KB · v2",
                    }
                    .into(),
                    !matches!(state, "loading" | "unavailable"),
                    Some(source.clone()),
                    cx,
                    |v, cx| v.open(cx),
                )
                .into_any_element(),
            }
        } else {
            ui::button("attachment-viewer-open", "打开附件预览", false, true)
                .on_click(cx.listener(|v, _, _, cx| v.open(cx)))
                .map(|trigger| source.bind(trigger, "打开附件预览", ui::ActionStyle::default()))
                .automation(AutomationRole::Button, "打开附件预览")
                .into_any_element()
        };
        wb::column(12.).child(trigger).child(self.viewer.clone())
    }
}
