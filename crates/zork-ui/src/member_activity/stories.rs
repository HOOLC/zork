use super::*;
use crate::{components::workbench as wb, controls as ui, resources::Text};
use std::cell::Cell;
pub struct Story {
    overlay: Entity<Overlay>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    details: DetailsTooltip,
    footer: String,
}
impl Story {
    pub fn new(state: &str, text: Text, cx: &mut Context<Self>) -> Self {
        Self {
            overlay: cx.new(|_| Default::default()),
            bounds: Default::default(),
            details: DetailsTooltip {
                key: "presence-demo-member".into(),
                title: "产品领队".into(),
                kind: text.text("presence_member"),
                avatar: Some("fox".into()),
                description: match state {
                    "active" => "正在整理产品资料",
                    "error" => "执行遇到问题",
                    _ => "等待新的消息",
                }
                .into(),
                rows: vec![
                    (
                        text.text("presence_recent"),
                        if state == "error" {
                            "读取文件失败：文件不存在"
                        } else {
                            "已整理项目资料"
                        }
                        .into(),
                    ),
                    (text.text("model"), "演示模型".into()),
                ],
            },
            footer: text.text("presence_history_hint"),
        }
    }
}
impl Render for Story {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.bounds.clone();
        let button = ui::button("member-activity-source", "产品领队", false, true)
            .on_hover(cx.listener(|v, hovered: &bool, _, cx| {
                if *hovered {
                    v.overlay.update(cx, |overlay, cx| {
                        overlay.show(
                            "demo-member".into(),
                            v.details.clone(),
                            v.footer.clone(),
                            v.bounds.get(),
                            cx,
                        )
                    });
                } else {
                    v.overlay
                        .update(cx, |overlay, cx| overlay.leave("demo-member", cx));
                }
            }))
            .child(
                canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .automation(AutomationRole::Button, "产品领队");
        wb::column(12.).child(button).child(self.overlay.clone())
    }
}
