//! Shared production component with a core-owned, offline business adapter.
use super::interaction_view;
use crate::{design::CUE_UI, i18n::Locale};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Window};
use zork_client_core::interactions::preview::Preview;
use zork_ui::components::interaction::InteractionCard;

pub struct InteractionStory {
    source: Preview,
    card: Entity<InteractionCard>,
}

impl InteractionStory {
    pub fn new(state: &str, cx: &mut Context<Self>) -> Self {
        let source = Preview::new(state);
        let card = cx.new(|cx| {
            InteractionCard::new(interaction_view::view(&source.card(), Locale::ZhCn), cx)
        });
        cx.subscribe(&card, |story, _, event, cx| {
            story.source.activate(&event.action, event.values.clone());
            let view = interaction_view::view(&story.source.card(), Locale::ZhCn);
            story.card.update(cx, |card, cx| card.set_view(view, cx));
            cx.notify();
        })
        .detach();
        Self { source, card }
    }

    pub fn inspect(&self) -> serde_json::Value {
        serde_json::to_value(self.source.card()).unwrap()
    }
}

impl Render for InteractionStory {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("interaction-story")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(CUE_UI.palette.canvas))
            .p_6()
            .child(div().max_w(px(620.)).mx_auto().child(self.card.clone()))
    }
}
