//! Read-only core card -> shared visual component, retaining unsubmitted input
//! across metadata updates. No business state is reduced in this adapter.
use crate::{i18n::Locale, transcript::TranscriptLine, views::RootView};
use gpui::{App, AppContext, Entity, WeakEntity};
use std::sync::Arc;
use zork_ui::components::interaction as ui;

pub struct Rendered {
    source: Arc<TranscriptLine>,
    locale: Locale,
    entity: Entity<ui::InteractionCard>,
}

use crate::desktop::interaction_view::view;

pub fn render(
    source: Arc<TranscriptLine>,
    cache: &super::message::MessageRenderDocument,
    locale: Locale,
    session: &str,
    root: WeakEntity<RootView>,
    cx: &mut App,
) -> Option<Entity<ui::InteractionCard>> {
    let TranscriptLine::Message { metadata, .. } = source.as_ref();
    let card = metadata.interaction_view.as_ref()?;
    let mut cached = cache.interaction.borrow_mut();
    if let Some(rendered) = cached.as_mut() {
        if !Arc::ptr_eq(&rendered.source, &source) || rendered.locale != locale {
            let value = view(card, locale);
            rendered
                .entity
                .update(cx, |card, cx| card.set_view(value, cx));
            rendered.source = source;
            rendered.locale = locale;
        }
        return Some(rendered.entity.clone());
    }
    let value = view(card, locale);
    let session = session.to_owned();
    let id = card.message_id.clone();
    let entity = cx.new(|cx| {
        ui::InteractionCard::new(value, cx).with_action_handler(move |event, cx| {
            let _ = root.update(cx, |view, cx| {
                view.activate_interaction(&session, &id, &event.action, event.values.clone(), cx)
            });
        })
    });
    *cached = Some(Box::new(Rendered {
        source,
        locale,
        entity: entity.clone(),
    }));
    Some(entity)
}
