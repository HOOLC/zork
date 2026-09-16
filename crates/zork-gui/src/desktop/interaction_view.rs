//! Map the read-only core contract into shared native/Web presentation.
use crate::i18n::Locale;
use zork_ui::components::interaction as ui;

pub fn view(card: &zork_client_core::interactions::Card, locale: Locale) -> ui::View {
    ui::View {
        id: card.message_id.clone(),
        title: if card.localized_title {
            locale.text(&card.title).to_owned()
        } else {
            card.title.clone()
        }
        .into(),
        status: locale.text(&card.status_key).to_owned().into(),
        description: card
            .description_key
            .as_ref()
            .map(|key| locale.text(key).to_owned().into()),
        fields: card
            .fields
            .iter()
            .map(|field| ui::Field {
                id: field.field.id.clone(),
                label: if field.localized_label {
                    locale.text(&field.field.label).to_owned()
                } else {
                    field.field.label.clone()
                }
                .into(),
                kind: match field.field.kind {
                    zork_client_core::interactions::FieldKind::Text => ui::FieldKind::Text,
                    zork_client_core::interactions::FieldKind::Multiline
                    | zork_client_core::interactions::FieldKind::Json => ui::FieldKind::Multiline,
                    zork_client_core::interactions::FieldKind::Choice => ui::FieldKind::Choice,
                    zork_client_core::interactions::FieldKind::MultiChoice => {
                        ui::FieldKind::MultiChoice
                    }
                },
                advanced: field.advanced,
                value: field.value.clone().into(),
                options: field
                    .field
                    .options
                    .iter()
                    .map(|option| {
                        (
                            option.value.clone(),
                            if field.localized_options {
                                locale.text(&option.label).to_owned().into()
                            } else {
                                option.label.clone().into()
                            },
                        )
                    })
                    .collect(),
                error: field
                    .error_key
                    .as_ref()
                    .map(|error| locale.text(error).to_owned().into()),
            })
            .collect(),
        details: card
            .details
            .iter()
            .map(|detail| {
                (
                    locale.text(&detail.label_key).to_owned().into(),
                    detail.value.clone().into(),
                )
            })
            .collect(),
        actions: card
            .actions
            .iter()
            .map(|action| ui::Action {
                id: action.id.clone(),
                label: locale.text(&action.label_key).to_owned().into(),
                primary: action.primary,
            })
            .collect(),
        editable: card.editable,
        placeholder: locale.text("interaction_choose").into(),
        expand_label: locale.text("message_expand").into(),
        collapse_label: locale.text("message_collapse").into(),
        more_label: locale.text("interaction_more_settings").into(),
        less_label: locale.text("interaction_less_settings").into(),
        empty_label: locale.text("interaction_none").into(),
        error: card
            .error
            .as_ref()
            .map(|text| locale.text(text).to_owned().into()),
    }
}
