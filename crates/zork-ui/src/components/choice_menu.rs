//! Controlled selection on the library's virtual list and select focus contract.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::widgets::controls::{action_focus, adaptive_action, ActionStyle},
    controls::{self as ui, Selection},
    design::{BORDER_WIDTH, FORM, ZORK_UI},
};
use gpui::{prelude::*, *};
use gpui_component::{
    list::{List, ListDelegate, ListItem, ListState},
    IndexPath, Sizable,
};
use std::{cell::Cell, rc::Rc};

type Choose = Rc<dyn Fn(usize, &mut Window, &mut App)>;
type Close = Rc<dyn Fn(&mut Window, &mut App)>;
#[derive(Clone, PartialEq)]
struct OptionRow {
    id: String,
    label: String,
    checked: bool,
    icon: Option<&'static str>,
}
struct Options {
    rows: Vec<OptionRow>,
    highlighted: Option<IndexPath>,
    selection: Selection,
    enabled: bool,
    choose: Choose,
    close: Option<Close>,
}
#[derive(IntoElement)]
struct ChoiceRow {
    id: String,
    item: ListItem,
    label: String,
    enabled: bool,
    highlighted: bool,
}
impl gpui_component::Selectable for ChoiceRow {
    fn selected(mut self, selected: bool) -> Self {
        self.highlighted = selected;
        self.item = self.item.selected(selected);
        self
    }
    fn is_selected(&self) -> bool {
        self.highlighted
    }
}
impl RenderOnce for ChoiceRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(self.id)
            .w_full()
            .child(self.item)
            .automation_enabled(self.enabled, AutomationRole::Option, self.label)
    }
}
impl ListDelegate for Options {
    type Item = ChoiceRow;
    fn items_count(&self, _: usize, _: &App) -> usize {
        self.rows.len()
    }
    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let row = self.rows.get(ix.row)?;
        Some(ChoiceRow {
            id: row.id.clone(),
            label: row.label.clone(),
            enabled: self.enabled,
            highlighted: false,
            item: ListItem::new(row.id.clone())
                .disabled(!self.enabled)
                .h(px(32.))
                .px(px(14.))
                .py_0()
                .rounded(px(crate::design::RADIUS.control))
                .text_size(px(12.))
                .role(match self.selection {
                    Selection::Single => Role::RadioButton,
                    Selection::Multiple => Role::CheckBox,
                    Selection::Actions => Role::Button,
                })
                .aria_label(row.label.clone())
                .when(self.selection != Selection::Actions, |v| {
                    v.aria_toggled(if row.checked {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .min_w_0()
                        .when_some(row.icon, |v, icon| v.child(ui::icon(icon, 14.)))
                        .child(div().flex_1().min_w_0().truncate().child(row.label.clone()))
                        .child(
                            div()
                                .w(px(14.))
                                .flex_shrink_0()
                                .when(row.checked, |v| v.child(ui::icon("icons/check.svg", 12.))),
                        ),
                ),
        })
    }
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.highlighted = ix;
    }
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        cx.stop_propagation();
        if self.enabled {
            if let Some(ix) = self.highlighted.filter(|ix| ix.row < self.rows.len()) {
                (self.choose)(ix.row, window, cx);
            }
        }
    }
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        if let Some(close) = &self.close {
            cx.stop_propagation();
            close(window, cx);
        } else {
            cx.propagate();
        }
    }
}
struct MenuState {
    list: Entity<ListState<Options>>,
    open: bool,
    anchor: Rc<Cell<Bounds<Pixels>>>,
}

pub(crate) fn render<V: 'static>(
    id: impl Into<SharedString>,
    label: String,
    options: Vec<(String, String, bool)>,
    open: bool,
    enabled: bool,
    selection: Selection,
    quiet: bool,
    max_width: Option<f32>,
    leading: Option<&'static str>,
    option_icons: Vec<Option<&'static str>>,
    window: &mut Window,
    cx: &mut Context<V>,
    set_open: impl Fn(&mut V, bool, &mut Context<V>) + 'static,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let id = id.into();
    let enabled = enabled && !options.is_empty();
    let requested_open = open;
    let open = open && enabled;
    let rows: Vec<_> = options
        .into_iter()
        .enumerate()
        .map(|(i, (id, label, checked))| OptionRow {
            id,
            label,
            checked,
            icon: option_icons.get(i).copied().flatten(),
        })
        .collect();
    // The open menu fits its longest option (row padding, icon, check mark,
    // panel inset and border) so option names are not cut short.
    let menu_width = if open {
        rows.iter()
            .map(|row| {
                crate::components::widgets::overlay::measure_label_with_weight(
                    &row.label,
                    12.,
                    FontWeight::NORMAL,
                    window,
                )
                .ceil()
                    + if row.icon.is_some() { 22. } else { 0. }
            })
            .fold(0., f32::max)
            + 28.
            + 22.
            + 12.
            + 2.
    } else {
        0.
    };
    let entry = rows.iter().position(|v| v.checked).unwrap_or(0);
    let trigger_focus = action_focus(format!("{id}-trigger"), window, cx);
    let owner = cx.entity().downgrade();
    let set_open = Rc::new(set_open);
    if requested_open && !enabled {
        let owner = owner.clone();
        let set_open = set_open.clone();
        cx.defer(move |app| {
            let _ = owner.update(app, |view, cx| set_open(view, false, cx));
        });
    }
    let close: Close = Rc::new({
        let owner = owner.clone();
        let set_open = set_open.clone();
        let focus = trigger_focus.clone();
        move |w, app| {
            let _ = owner.update(app, |v, cx| set_open(v, false, cx));
            w.focus(&focus, app);
        }
    });
    let activate: Choose = Rc::new({
        let owner = owner.clone();
        let close = close.clone();
        move |index, w, app| {
            let _ = owner.update(app, |v, cx| choose(v, index, cx));
            if selection != Selection::Multiple {
                close(w, app);
            }
        }
    });
    let state = window.use_keyed_state(format!("{id}-list-state"), cx, |window, cx| MenuState {
        list: cx.new(|cx| {
            ListState::new(
                Options {
                    rows: vec![],
                    highlighted: None,
                    selection,
                    enabled,
                    choose: activate.clone(),
                    close: Some(close.clone()),
                },
                window,
                cx,
            )
        }),
        open: false,
        anchor: Rc::new(Cell::new(Bounds::default())),
    });
    let (list, anchor, opening) = state.update(cx, |s, _| {
        let opening = open && !s.open;
        s.open = open;
        (s.list.clone(), s.anchor.clone(), opening)
    });
    list.update(cx, |s, window_cx| {
        let changed = s.delegate().rows != rows
            || s.delegate().enabled != enabled
            || s.delegate().selection != selection;
        let d = s.delegate_mut();
        d.rows = rows;
        d.enabled = enabled;
        d.selection = selection;
        d.choose = activate;
        d.close = Some(close.clone());
        if opening {
            s.set_selected_index(Some(IndexPath::default().row(entry)), window, window_cx);
            s.scroll_to_selected_item(window, window_cx);
        }
        if changed {
            window_cx.notify();
        }
    });
    let focus = list.focus_handle(cx);
    if opening {
        window.focus(&focus, cx);
    }
    if open && !focus.contains_focused(window, cx) && !trigger_focus.is_focused(window) {
        let owner = owner.clone();
        let set_open = set_open.clone();
        cx.defer(move |app| {
            let _ = owner.update(app, |v, cx| set_open(v, false, cx));
        });
    }
    let control_height = if quiet { 24. } else { ui::DROPDOWN_HEIGHT };
    let control_width = (crate::components::widgets::overlay::measure_label_with_weight(
        &label,
        if quiet { 12. } else { 13. },
        FontWeight::MEDIUM,
        window,
    )
    .ceil()
        + 24.
        + 7.
        + 12.
        + if leading.is_some() { 22. } else { 0. }
        + 2.)
        .min(max_width.unwrap_or(f32::MAX).max(48.));
    let pointer_owner = owner.clone();
    let pointer_open = set_open.clone();
    let pointer_focus = focus.clone();
    let source_focus = trigger_focus.clone();
    let trigger = adaptive_action(
        id.clone(),
        label.clone(),
        ActionStyle {
            select_trigger: !quiet,
            quiet,
            expanded: open,
            disabled: !enabled,
            icon: leading,
            trailing: Some("icons/chevron-down.svg"),
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
    .px_0()
    .w(px(control_width))
    .h(px(control_height))
    .min_h_0()
    .py_0()
    .track_focus(&trigger_focus)
    .on_click(move |event, w, app| {
        if enabled && !matches!(event, ClickEvent::Keyboard(_)) {
            let _ = pointer_owner.update(app, |v, cx| pointer_open(v, !open, cx));
            w.focus(if open { &source_focus } else { &pointer_focus }, app);
        }
    })
    .automation_enabled(enabled, AutomationRole::Button, label.clone());
    let mut select = gpui_base::Select::new(format!("{id}-select"))
        .open(open)
        .disabled(!enabled)
        .focus_handle(&trigger_focus)
        .content_focus_handle(&focus)
        .accessibility_label(label.clone())
        .on_open_change(move |next, _, app| {
            let _ = owner.update(app, |v, cx| set_open(v, next, cx));
        })
        .w_full()
        .h_full()
        .child(trigger);
    if open {
        let height = (window.viewport_size().height.as_f32() - 24.).clamp(32., 320.);
        let edge_list = list.clone();
        let panel = div()
            .id(format!("{id}-menu"))
            .occlude()
            .w(px(control_width.max(160.).max(menu_width.min(360.))))
            .max_h(px(height))
            .p(px(6.))
            .rounded(px(ui::PLAIN_POPOVER_RADIUS))
            .bg(rgb(ZORK_UI.palette.canvas))
            .border(px(BORDER_WIDTH))
            .border_color(rgb(FORM.outline))
            .shadow_sm()
            .child(crate::components::smooth::rounded_viewport(
                format!("{id}-viewport"),
                ui::PLAIN_POPOVER_RADIUS - 6.,
                List::new(&list).small().max_h(px(height - 13.)),
            ))
            .on_key_down(move |e: &KeyDownEvent, w, app| {
                if !matches!(e.keystroke.key.as_str(), "home" | "end") {
                    return;
                }
                edge_list.update(app, |s, cx| {
                    let n = s.delegate().rows.len();
                    if n > 0 {
                        let ix = if e.keystroke.key == "home" { 0 } else { n - 1 };
                        s.set_selected_index(Some(IndexPath::default().row(ix)), w, cx);
                        s.scroll_to_selected_item(w, cx);
                    }
                });
                w.prevent_default();
                app.stop_propagation();
            })
            .on_mouse_down_out(move |_, w, cx| close(w, cx))
            // Opens below its trigger: fade in while moving away from it.
            .with_animation(
                SharedString::from(format!("{id}-menu-enter")),
                crate::motion::enter(crate::motion::POPOVER),
                |panel, t| {
                    panel
                        .opacity(t)
                        .relative()
                        .top(px(-crate::motion::POPOVER_OFFSET * (1. - t)))
                },
            )
            .automation(AutomationRole::ScrollArea, label);
        select = select.child(
            deferred(
                gpui_base::Positioner::side(anchor.get())
                    .placement(gpui_base::Placement::Bottom)
                    .align(gpui_base::Align::Start)
                    .offset(px(ui::MENU_GAP))
                    .margin(px(8.))
                    .child(panel),
            )
            .with_priority(350),
        );
    }
    div()
        .relative()
        .w(px(control_width))
        .h(px(control_height))
        .flex_shrink_0()
        .child(
            canvas(move |bounds, _, _| anchor.set(bounds), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(select)
        .into_any_element()
}

/// Model/Profile panels share the same library list without closing their host on selection.
pub(crate) fn inline_list<V: 'static>(
    id: &'static str,
    options: Vec<(String, String, bool)>,
    enabled: bool,
    window: &mut Window,
    cx: &mut Context<V>,
    choose: impl Fn(&mut V, usize, &mut Context<V>) + 'static,
) -> AnyElement {
    let rows: Vec<_> = options
        .into_iter()
        .map(|(id, label, checked)| OptionRow {
            id,
            label,
            checked,
            icon: None,
        })
        .collect();
    let height = (rows.len() as f32 * 32.).min(280.);
    let entry = rows.iter().position(|r| r.checked).unwrap_or(0);
    let owner = cx.entity().downgrade();
    let choose: Choose = Rc::new(move |i, _, app| {
        let _ = owner.update(app, |v, cx| choose(v, i, cx));
    });
    let state = window.use_keyed_state(format!("{id}-state"), cx, |w, cx| {
        cx.new(|cx| {
            let mut list = ListState::new(
                Options {
                    rows: rows.clone(),
                    highlighted: None,
                    selection: Selection::Single,
                    enabled,
                    choose: choose.clone(),
                    close: None,
                },
                w,
                cx,
            );
            list.set_selected_index((!rows.is_empty()).then(|| IndexPath::new(entry)), w, cx);
            list.scroll_to_selected_item(w, cx);
            list
        })
    });
    let list = state.read(cx).clone();
    list.update(cx, |s, cx| {
        let rows_changed = s.delegate().rows != rows;
        let changed = rows_changed || s.delegate().enabled != enabled;
        let selected = (!rows.is_empty()).then(|| IndexPath::new(entry));
        let d = s.delegate_mut();
        d.rows = rows;
        d.enabled = enabled;
        d.choose = choose;
        // A new projection can change the current choice or remove/reorder rows.
        // Reconcile the library cursor as well as the visible check marks.
        if rows_changed {
            s.set_selected_index(selected, window, cx);
            s.scroll_to_selected_item(window, cx);
        }
        if changed {
            cx.notify();
        }
    });
    let edges = list.clone();
    div()
        .id(id)
        .w_full()
        .min_h_0()
        .h(px(height))
        .on_key_down(move |e: &KeyDownEvent, w, app| {
            if !matches!(e.keystroke.key.as_str(), "home" | "end") {
                return;
            }
            edges.update(app, |s, cx| {
                let n = s.delegate().rows.len();
                if n > 0 {
                    let ix = if e.keystroke.key == "home" { 0 } else { n - 1 };
                    s.set_selected_index(Some(IndexPath::new(ix)), w, cx);
                    s.scroll_to_selected_item(w, cx);
                }
            });
            w.prevent_default();
            app.stop_propagation();
        })
        .child(List::new(&list).small().max_h(px(height)))
        .automation(AutomationRole::ScrollArea, id)
        .into_any_element()
}
