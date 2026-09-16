use super::super::controls::{self, ActionStyle};
use super::{disabled_node, selection, surface};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::collapse::Collapse,
    design::CUE_UI,
};
use gpui::{prelude::*, *};
use std::rc::Rc;

/// Navigation links keep individual Tab stops. The host resolves the selected
/// destination; this presentation component does not own routing or requests.
pub fn tab_nav<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<selection::Item>,
    selected: usize,
    width: f32,
    window: &mut Window,
    cx: &mut Context<V>,
    navigate: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let navigate = Rc::new(navigate);
    let mut row = div()
        .id(id.into())
        .w(px(width))
        .overflow_x_scroll()
        .role(Role::Navigation)
        .aria_label("标签导航")
        .flex()
        .gap(px(12.));
    for (index, item) in items.into_iter().enumerate() {
        let focus = controls::action_focus(item.id.clone(), window, cx).tab_stop(!item.disabled);
        let callback = navigate.clone();
        let disabled = item.disabled;
        row = row.child(
            div()
                .id(item.id)
                .role(Role::Link)
                .aria_label(item.label.clone())
                .track_focus(&focus)
                .tab_stop(!disabled)
                .flex_shrink_0()
                .py(px(8.))
                .text_size(px(13.))
                .line_height(px(20.))
                .text_color(rgb(if disabled {
                    CUE_UI.palette.muted
                } else {
                    CUE_UI.palette.text
                }))
                .border_b(px(crate::design::BORDER_WIDTH))
                .border_color(if index == selected {
                    rgb(crate::design::BRAND_ACCENT)
                } else {
                    rgba(0)
                })
                .a11y_synthetic_children(move |builder| {
                    if index == selected {
                        builder
                            .parent_node()
                            .set_aria_current(accesskit::AriaCurrent::Page);
                    }
                    if disabled {
                        builder.parent_node().set_disabled();
                    }
                })
                .when(!disabled, |v| {
                    v.cursor_pointer()
                        .hover(|v| v.underline())
                        .focus_visible(|v| v.underline())
                })
                .child(item.label.clone())
                .on_click(cx.listener(move |v, _, _, cx| {
                    if !disabled {
                        callback(v, index, cx);
                    }
                }))
                .automation_enabled(!disabled, AutomationRole::Button, item.label),
        );
    }
    row.into_any_element()
}

pub struct Item<V: 'static> {
    pub id: String,
    pub title: SharedString,
    pub disabled: bool,
    pub content: Box<dyn FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement>,
}
impl<V: 'static> Item<V> {
    /// The body is built only while mounted. `interactive` is false while
    /// clipped by the aperture; editors and controls must use it for tab stops.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<SharedString>,
        content: impl FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            disabled: false,
            content: Box::new(content),
        }
    }
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
}

#[derive(Clone, Copy)]
pub struct AccordionMode {
    pub multiple: bool,
    pub collapsible: bool,
}
impl Default for AccordionMode {
    fn default() -> Self {
        Self {
            multiple: false,
            collapsible: true,
        }
    }
}
impl AccordionMode {
    pub fn change(self, values: &[usize], index: usize) -> Vec<usize> {
        if values.contains(&index) && !self.multiple && !self.collapsible {
            return values.to_vec();
        }
        (if self.multiple {
            selection::Mode::MultipleToggle
        } else {
            selection::Mode::SingleToggle
        })
        .change(values, index)
    }
}

pub fn accordion<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<Item<V>>,
    expanded: Vec<usize>,
    mode: AccordionMode,
    width: f32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, Vec<usize>, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let callback = Rc::new(change);
    let count = items.len();
    let mut root = surface(
        id,
        crate::controls::CARD_RADIUS,
        CUE_UI.palette.prompt,
        false,
    )
    .w(px(width))
    .p(px(8.))
    .flex()
    .flex_col()
    .gap(px(2.))
    .role(Role::Group);
    let mut handles = Vec::with_capacity(count);
    let mut active = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        let open = expanded.contains(&i);
        let collapse = Collapse::new(
            format!("{}-collapse", item.id),
            open,
            width - 16.,
            window,
            cx,
        );
        let focus = collapse.header_focus(cx).tab_stop(!item.disabled);
        handles.push(focus.clone());
        if !item.disabled {
            active.push(i);
        }
        let callback = callback.clone();
        let selected = expanded.clone();
        let interactive = collapse.interactive(cx);
        let header = controls::action(
            format!("{}-trigger", item.id),
            item.title.clone(),
            width - 16.,
            40.,
            ActionStyle {
                quiet: true,
                leading: true,
                disabled: item.disabled,
                trailing: Some("icons/chevron-down.svg"),
                expanded: open,
                ..Default::default()
            },
            CUE_UI.palette.prompt,
            window,
            cx,
        )
        .track_focus(&focus)
        .tab_stop(!item.disabled)
        .aria_expanded(open)
        .a11y_synthetic_children(move |builder| disabled_node(builder.parent_node(), item.disabled))
        .on_click(cx.listener(move |v, _, w, cx| {
            if !item.disabled {
                w.focus(&focus, cx);
                callback(v, mode.change(&selected, i), cx);
            }
        }))
        .automation_enabled(!item.disabled, AutomationRole::Button, item.title.clone());
        let content = collapse.mounted(cx).then(|| {
            div()
                .id(format!("{}-content", item.id))
                .role(Role::Group)
                .aria_label(item.title)
                .px(px(12.))
                .pt(px(6.))
                .pb(px(14.))
                .text_size(px(13.))
                .line_height(px(21.))
                .capture_any_mouse_down(move |_, _, cx| {
                    if !interactive {
                        cx.stop_propagation()
                    }
                })
                .capture_key_down(move |_, _, cx| {
                    if !interactive {
                        cx.stop_propagation()
                    }
                })
                .child((item.content)(interactive, width - 40., window, cx))
                .into_any_element()
        });
        let weak = cx.entity().downgrade();
        root = root.child(
            div()
                .id(item.id)
                .w_full()
                .flex()
                .flex_col()
                .child(header)
                .child(collapse.element(
                    content,
                    move |_, cx| {
                        let _ = weak.update(cx, |_, cx| cx.notify());
                    },
                    cx,
                )),
        );
    }
    root.on_key_down(move |e: &KeyDownEvent, w, cx| {
        let Some(position) = active.iter().position(|i| handles[*i].is_focused(w)) else {
            return;
        };
        let next = match e.keystroke.key.as_str() {
            "up" => (position + active.len() - 1) % active.len(),
            "down" => (position + 1) % active.len(),
            "home" => 0,
            "end" => active.len() - 1,
            _ => return,
        };
        w.focus(&handles[active[next]], cx);
        w.prevent_default();
        cx.stop_propagation();
    })
    .into_any_element()
}

/// A single disclosure uses the same aperture and focus-return contract.
pub fn collapsible<V: 'static>(
    id: impl Into<String>,
    title: impl Into<SharedString>,
    body: impl FnOnce(bool, f32, &mut Window, &mut Context<V>) -> AnyElement + 'static,
    open: bool,
    disabled: bool,
    width: f32,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let mut item = Item::new(format!("{id}-item"), title, body);
    item.disabled = disabled;
    accordion(
        id,
        vec![item],
        if open { vec![0] } else { vec![] },
        AccordionMode::default(),
        width,
        window,
        cx,
        move |v, indices, cx| change(v, !indices.is_empty(), cx),
    )
}

/// A tab list and its associated panel are one component. Manual activation
/// changes focus on arrows and activates on Enter/Space; automatic activates on arrows.
pub fn tabs<V: 'static>(
    id: impl Into<SharedString>,
    items: Vec<selection::Item>,
    selected: usize,
    manual: bool,
    width: f32,
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut Context<V>,
    change: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let callback = Rc::new(change);
    let active: Vec<_> = items
        .iter()
        .enumerate()
        .filter(|(_, i)| !i.disabled)
        .map(|(i, _)| i)
        .collect();
    let selected = if active.contains(&selected) {
        selected
    } else {
        active.first().copied().unwrap_or(0)
    };
    let label = items
        .get(selected)
        .map_or_else(SharedString::default, |i| i.label.clone());
    let list_width = (items
        .iter()
        .map(|item| super::super::overlay::measure_label(&item.label, 12., window) + 24.)
        .fold(0f32, f32::max)
        * items.len() as f32
        + 8.)
        .min(width);
    let list = controls::segments(
        format!("{id}-list"),
        list_width,
        items
            .into_iter()
            .map(|item| controls::Segment {
                id: item.id,
                label: item.label,
                disabled: item.disabled,
            })
            .collect(),
        Some(selected),
        controls::SegmentKind::Tabs { manual },
        true,
        CUE_UI.palette.canvas,
        window,
        cx,
        move |v, i, cx| callback(v, i, cx),
    );
    div()
        .id(id.clone())
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(list)
        .child(
            div()
                .id(format!("{id}-panel-{selected}"))
                .role(Role::TabPanel)
                .aria_label(label.clone())
                .focusable()
                .tab_stop(true)
                .text_size(px(13.))
                .line_height(px(21.))
                .child(content)
                .automation(AutomationRole::Status, format!("{label}面板")),
        )
        .into_any_element()
}
