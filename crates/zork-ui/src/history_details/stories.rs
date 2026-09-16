use super::*;
use crate::components::workbench as wb;
pub struct Story {
    view: Entity<Details>,
    presentation: Presentation,
    tree: json_tree::State,
    text: Text,
}
impl Story {
    pub fn new(state: &str, text: Text, cx: &mut Context<Self>) -> Self {
        let view = cx.new(|cx| Details::new(text.clone(), cx));
        let presentation = if state == "agent" {
            Presentation::Agent {
                id: "leader".into(),
                name: "产品领队".into(),
                avatar: Some("fox".into()),
                role: Some("整理产品需求与研究资料".into()),
            }
        } else {
            let fixture = crate::stories::page_fixture();
            let records: Vec<crate::history::Record> =
                serde_json::from_value(fixture["history"]["records"].clone()).unwrap();
            let mut entry = crate::history::entries(&records)
                .into_iter()
                .next()
                .unwrap();
            if state == "long" {
                entry.raw = (0..20).map(|i| serde_json::json!({"step":i,"details":{"inputs":["需求","资料"],"status":"completed"}})).collect();
            }
            Presentation::Entry(entry)
        };
        Self {
            view,
            presentation,
            tree: Default::default(),
            text,
        }
    }
}
impl Render for Story {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = ui::button("history-details-open", "查看详情", false, true).on_click(
            cx.listener(|v, _, _, cx| {
                v.view.update(cx, |view, cx| {
                    view.configure(
                        "demo".into(),
                        Some(v.presentation.clone()),
                        None,
                        v.tree.clone(),
                        v.text.clone(),
                        Rc::new(|_| {}),
                        cx,
                    )
                });
            }),
        );
        let source = self.view.read(cx).source();
        wb::column(12.)
            .child(
                source
                    .bind(button, "查看详情", ui::ActionStyle::default())
                    .automation(AutomationRole::Button, "查看详情"),
            )
            .child(self.view.clone())
    }
}
