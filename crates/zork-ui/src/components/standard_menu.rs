//! Product menus use GPUI Component's keyboard and accessibility implementation.
use crate::automation::{AutomationElementExt, AutomationRole};
use crate::components::widgets::controls::ControlElement;
use gpui::{prelude::*, *};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[derive(Clone, Copy, PartialEq)]
pub enum ItemKind {
    Action,
    Check(bool),
    Radio(bool),
    Heading,
    Separator,
}

#[derive(Clone)]
pub struct Item {
    pub key: String,
    pub label: SharedString,
    pub kind: ItemKind,
    pub disabled: bool,
    pub children: Vec<Item>,
    pub icon: Option<&'static str>,
    pub detail: Option<SharedString>,
}
impl Item {
    pub fn new(key: impl Into<String>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: ItemKind::Action,
            disabled: false,
            children: Vec::new(),
            icon: None,
            detail: None,
        }
    }
    pub fn disabled(mut self) -> Self {
        self.disabled = true;
        self
    }
    pub fn icon(mut self, path: &'static str) -> Self {
        self.icon = Some(path);
        self
    }
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }
    pub fn check(mut self, checked: bool) -> Self {
        self.kind = ItemKind::Check(checked);
        self
    }
    pub fn radio(mut self, checked: bool) -> Self {
        self.kind = ItemKind::Radio(checked);
        self
    }
    pub fn submenu(mut self, items: Vec<Self>) -> Self {
        self.children = items;
        self
    }
    pub fn separator(key: impl Into<String>) -> Self {
        Self {
            kind: ItemKind::Separator,
            ..Self::new(key, "")
        }
    }
}

#[derive(Default)]
struct State {
    open: bool,
    position: Point<Pixels>,
    focus: Option<FocusHandle>,
    popup: Option<Entity<PopupMenu>>,
    subscription: Option<Subscription>,
    /// The last popup and its position, kept for the exit fade after closing.
    leaving: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    presence: Option<crate::motion::Presence>,
}
#[derive(Clone, Default)]
pub struct Menu {
    state: Rc<RefCell<State>>,
}

impl Menu {
    pub fn is_open(&self) -> bool {
        self.state.borrow().open
    }
    pub fn dismiss(&self) {
        let mut state = self.state.borrow_mut();
        state.open = false;
        if let Some(popup) = state.popup.take() {
            state.leaving = Some((popup, state.position));
        }
        state.subscription = None;
    }
    pub fn close(&self, window: &mut Window, cx: &mut App) {
        let focus = self.state.borrow().focus.clone();
        self.dismiss();
        if let Some(focus) = focus {
            window.focus(&focus, cx);
        }
    }
    pub fn open_at(&self, position: Point<Pixels>, focus: FocusHandle) {
        let mut state = self.state.borrow_mut();
        state.open = true;
        state.position = position;
        state.focus = Some(focus);
        state.popup = None;
    }

    pub fn trigger_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let measured = bounds.clone();
        let state = self.clone();
        let focus = focus.clone();
        element
            .control_focus(&focus)
            .aria_expanded(self.is_open())
            .on_click(cx.listener(move |_, _, window, cx| {
                if !enabled {
                    return;
                }
                if state.is_open() {
                    state.close(window, cx);
                } else {
                    state.open_at(
                        bounds.get().bottom_left() + point(px(0.), px(6.)),
                        focus.clone(),
                    );
                }
                cx.notify();
            }))
            .control_overlay(
                canvas(move |bounds, _, _| measured.set(bounds), |_, _, _, _| {})
                    .absolute()
                    .inset_0()
                    .into_any_element(),
            )
    }

    pub fn context_element<V: 'static, E: ControlElement>(
        &self,
        element: E,
        focus: &FocusHandle,
        enabled: bool,
        cx: &mut Context<V>,
    ) -> E {
        let state = self.clone();
        let focus = focus.clone();
        element.control_focus(&focus).on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                if enabled {
                    state.open_at(event.position, focus.clone());
                    window.focus(&focus, cx);
                    cx.notify();
                    cx.stop_propagation();
                }
            }),
        )
    }

    pub fn render<V: 'static>(
        &self,
        id: impl Into<SharedString>,
        items: Vec<Item>,
        window: &mut Window,
        cx: &mut Context<V>,
        choose: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static,
    ) -> Option<AnyElement> {
        let id: SharedString = id.into();
        let now = cx.background_executor().now();
        let mode = crate::motion::mode(cx);
        let frame = {
            let mut state = self.state.borrow_mut();
            let open = state.open;
            let presence = state
                .presence
                .get_or_insert_with(|| crate::motion::Presence::new(crate::motion::POPOVER));
            let (frame, moving) = presence.step(open, now, mode);
            if moving {
                window.request_animation_frame();
            }
            if frame.is_none() {
                state.leaving = None;
            }
            frame?
        };
        if frame.closing {
            // The dismissed popup fades where it was; a capture layer keeps
            // clicks from reaching its items while it leaves.
            let (popup, position) = self.state.borrow().leaving.clone()?;
            return Some(
                deferred(
                    gpui_base::Positioner::corner(Anchor::TopLeft, position).child(
                        div()
                            .opacity(frame.opacity)
                            .capture_any_mouse_down(|_, _, cx| cx.stop_propagation())
                            .capture_any_mouse_up(|_, _, cx| cx.stop_propagation())
                            .child(popup),
                    ),
                )
                .with_priority(350)
                .into_any_element(),
            );
        }
        let (position, focus, existing) = {
            let mut state = self.state.borrow_mut();
            state.leaving = None;
            (state.position, state.focus.clone(), state.popup.clone())
        };
        let popup = if let Some(popup) = existing {
            popup
        } else {
            let owner = cx.entity().downgrade();
            let choose = Rc::new(choose);
            let activate: Rc<dyn Fn(String, &mut Window, &mut App)> =
                Rc::new(move |key, window, app| {
                    let choose = choose.clone();
                    let _ = owner.update(app, |view, cx| choose(view, key, window, cx));
                });
            let popup = build_menu(
                items,
                id.to_string(),
                0,
                focus.clone(),
                activate,
                window,
                cx,
            );
            let state = Rc::downgrade(&self.state);
            let subscription =
                window.subscribe(&popup, cx, move |_, _: &DismissEvent, window, _| {
                    let Some(state) = state.upgrade() else {
                        return;
                    };
                    let mut state = state.borrow_mut();
                    state.open = false;
                    if let Some(popup) = state.popup.take() {
                        state.leaving = Some((popup, state.position));
                    }
                    window.refresh();
                });
            {
                let mut state = self.state.borrow_mut();
                state.popup = Some(popup.clone());
                state.subscription = Some(subscription);
            }
            window.focus(&popup.focus_handle(cx), cx);
            popup
        };
        Some(
            deferred(
                gpui_base::Positioner::corner(Anchor::TopLeft, position).child(
                    // Context menus open at the pointer and drop away from it.
                    div()
                        .relative()
                        .top(px(-crate::motion::POPOVER_OFFSET * frame.travel))
                        .opacity(frame.opacity)
                        .child(popup),
                ),
            )
                .with_priority(350)
                .into_any_element(),
        )
    }
}

fn build_menu(
    items: Vec<Item>,
    id: String,
    depth: usize,
    focus: Option<FocusHandle>,
    activate: Rc<dyn Fn(String, &mut Window, &mut App)>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PopupMenu> {
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let entry = match item.kind {
            ItemKind::Separator => PopupMenuItem::separator(),
            ItemKind::Heading => PopupMenuItem::label(item.label),
            _ if !item.children.is_empty() => {
                let submenu = build_menu(
                    item.children,
                    id.clone(),
                    depth + 1,
                    focus.clone(),
                    activate.clone(),
                    window,
                    cx,
                );
                PopupMenuItem::submenu(item.label, submenu).disabled(item.disabled)
            }
            ItemKind::Action | ItemKind::Check(_) | ItemKind::Radio(_) => {
                let key = item.key;
                let label = item.label;
                let content_key = format!("{id}-{depth}-{key}");
                let content_label = label.clone();
                let icon = item.icon;
                let detail = item.detail;
                let disabled = item.disabled;
                let mut entry = PopupMenuItem::element(move |_, _| {
                    div()
                        .id(content_key.clone())
                        .w_full()
                        .flex()
                        .items_center()
                        .gap_2()
                        .aria_label(content_label.clone())
                        .when_some(icon, |v, path| v.child(crate::controls::icon(path, 16.)))
                        .child(content_label.clone())
                        .when_some(detail.clone(), |v, detail| {
                            v.child(
                                div()
                                    .text_color(rgb(crate::design::ZORK_UI.palette.muted))
                                    .child(detail),
                            )
                        })
                        .automation_enabled(
                            !disabled,
                            AutomationRole::Option,
                            content_label.to_string(),
                        )
                });
                if let ItemKind::Check(checked) | ItemKind::Radio(checked) = item.kind {
                    entry = entry.checked(checked);
                }
                let activate = activate.clone();
                entry
                    .disabled(item.disabled)
                    .on_click(move |_, window, cx| activate(key.clone(), window, cx))
            }
        };
        entries.push(entry);
    }
    PopupMenu::build(window, cx, move |mut menu, _, _| {
        if let Some(focus) = focus {
            menu = menu.action_context(focus);
        }
        entries.into_iter().fold(menu, |menu, item| menu.item(item))
    })
}
