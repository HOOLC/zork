//! Expandable readonly JSON with shared state and host invalidation notification.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::CUE_UI,
};
use gpui::{prelude::*, *};
pub type State = std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>;
pub fn render<V: 'static>(
    state: State,
    changed: std::rc::Rc<dyn Fn(&mut gpui::App)>,
    value: &serde_json::Value,
    path: String,
    depth: usize,
    label: Option<String>,
    cx: &mut Context<V>,
) -> Div {
    let collection = match value {
        serde_json::Value::Object(v) => Some(("{", "}", v.len())),
        serde_json::Value::Array(v) => Some(("[", "]", v.len())),
        _ => None,
    };
    let open = if depth == 0 {
        !state.borrow().contains(&path)
    } else {
        state.borrow().contains(&path)
    };
    let mut row = div()
        .flex()
        .items_center()
        .min_h(px(18.))
        .line_height(px(18.))
        .gap(px(4.));
    if collection.is_some() {
        row = row.child(ui::icon("icons/chevron-down.svg", 10.).when(!open, |s| {
            s.with_transformation(gpui::Transformation::rotate(gpui::radians(
                -std::f32::consts::FRAC_PI_2,
            )))
        }));
    }
    if let Some(label) = label {
        row = row.child(div().text_color(rgb(CUE_UI.palette.muted)).child(format!(
            "{}:",
            serde_json::to_string(&label).unwrap_or_default()
        )));
    }
    if let Some((a, b, n)) = collection {
        let key = path.clone();
        let toggled = state.clone();
        let notify = changed.clone();
        row = row.child(if open {
            a.to_owned()
        } else {
            format!("{a}…{b} ({n})")
        });
        let mut tree = div().child(
            div()
                .id(gpui::SharedString::from(path.clone()))
                .cursor_pointer()
                .on_click(cx.listener(move |_, _, _, cx| {
                    if !toggled.borrow_mut().remove(&key) {
                        toggled.borrow_mut().insert(key.clone());
                    }
                    notify(cx);
                    cx.notify();
                }))
                .child(row)
                .automation(AutomationRole::Button, format!("JSON {path}")),
        );
        if open {
            let children: Vec<(String, &serde_json::Value)> = match value {
                serde_json::Value::Object(v) => v.iter().map(|(k, v)| (k.clone(), v)).collect(),
                serde_json::Value::Array(v) => v
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v))
                    .collect(),
                _ => vec![],
            };
            tree = tree
                .child(
                    div()
                        .pl(px(16.))
                        .children(children.into_iter().map(|(key, val)| {
                            render(
                                state.clone(),
                                changed.clone(),
                                val,
                                format!("{path}/{key}"),
                                depth + 1,
                                if value.is_array() { None } else { Some(key) },
                                cx,
                            )
                        })),
                )
                .child(b);
        }
        tree
    } else {
        let color = match value {
            serde_json::Value::String(_) => CUE_UI.palette.text,
            serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {
                crate::components::history::SEND_COLOR
            }
            _ => CUE_UI.palette.muted,
        };
        div().child(row.child(div().text_color(rgb(color)).child(value.to_string())))
    }
}
