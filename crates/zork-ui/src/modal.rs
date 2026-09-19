//! Window-level modal surface shared by node settings editors.
use crate::controls as ui;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, App, Context, FocusHandle, FontWeight, MouseButton, Window};
use std::{
    cell::Cell,
    rc::{Rc, Weak},
};

mod state;
pub use state::ModalState;

pub(crate) struct FocusScope {
    pub focus: FocusHandle,
    active: Option<&'static str>,
    previous: Option<FocusHandle>,
    interactive: Rc<Cell<bool>>,
}
impl FocusScope {
    pub fn new(cx: &mut App) -> Self {
        let focus = cx.focus_handle();
        let interactive = Rc::new(Cell::new(false));
        let scopes = cx.default_global::<FocusScopes>();
        scopes.0.retain(|(_, active)| active.strong_count() > 0);
        scopes.0.push((focus.clone(), Rc::downgrade(&interactive)));
        Self {
            focus,
            active: None,
            previous: None,
            interactive,
        }
    }
    /// An explicit activation replaces the return target even when a close and
    /// another open arrive before the next render observes the closed state.
    pub fn activate(
        &mut self,
        active: &'static str,
        source: &FocusHandle,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.previous = Some(source.clone());
        self.active = Some(active);
        self.interactive.set(true);
        window.focus(&self.focus, cx);
    }
    pub fn sync(&mut self, active: Option<&'static str>, window: &mut Window, cx: &mut App) {
        self.interactive.set(active.is_some());
        if active == self.active {
            return;
        }
        if active.is_some() {
            if self.active.is_none() {
                self.previous = window.focused(cx);
            }
            window.focus(&self.focus, cx);
        } else {
            let previous = self.previous.take();
            // A closing sibling must not take focus back from a newly opened
            // dialog or from an explicit source selected by the host.
            if self.focus.contains_focused(window, cx) {
                if let Some(previous) = previous {
                    window.focus(&previous, cx);
                } else {
                    window.blur();
                }
            }
        }
        self.active = active;
    }
}

#[derive(Default)]
struct FocusScopes(Vec<(FocusHandle, Weak<Cell<bool>>)>);
impl gpui::Global for FocusScopes {}

/// Retired dialog contents still paint, but window traversal skips their focus
/// groups until they are opened again. Weak registrations retire with the view.
pub(crate) fn advance_focus(backwards: bool, window: &mut Window, cx: &mut App) {
    let scopes = cx
        .try_global::<FocusScopes>()
        .map(|scopes| scopes.0.clone())
        .unwrap_or_default();
    let first = window.focused(cx);
    for _ in 0..256 {
        if backwards {
            window.focus_prev(cx);
        } else {
            window.focus_next(cx);
        }
        if !scopes.iter().any(|(focus, active)| {
            active.upgrade().is_some_and(|active| !active.get())
                && focus.contains_focused(window, cx)
        }) {
            return;
        }
        if window.focused(cx) == first {
            break;
        }
    }
    window.blur();
}

/// Keep forward and reverse keyboard navigation within an active modal group.
pub fn cycle_focus(focus: &FocusHandle, backwards: bool, window: &mut Window, cx: &mut App) {
    advance_focus(backwards, window, cx);
    if focus.contains_focused(window, cx) {
        return;
    }
    window.focus(focus, cx);
    advance_focus(false, window, cx);
    if backwards {
        let first = window.focused(cx);
        let mut last = first.clone();
        for _ in 0..256 {
            advance_focus(false, window, cx);
            if !focus.contains_focused(window, cx)
                || first.as_ref().is_some_and(|f| f.is_focused(window))
            {
                break;
            }
            last = window.focused(cx);
        }
        if let Some(last) = last {
            window.focus(&last, cx);
        }
    }
}

/// Raw control keys and editor traversal actions share one modal boundary.
pub fn trap_focus<E: gpui::InteractiveElement>(element: E, focus: &FocusHandle) -> E {
    let raw = focus.clone();
    let next = focus.clone();
    let previous = focus.clone();
    element
        .track_focus(&focus.clone().tab_stop(false))
        .tab_group()
        .tab_index(0)
        .tab_stop(false)
        .capture_key_down(move |event, window, cx| {
            if event.keystroke.key == "tab" {
                cycle_focus(&raw, event.keystroke.modifiers.shift, window, cx);
                cx.stop_propagation();
            }
        })
        .on_action(move |_: &crate::navigation::Next, window, cx| {
            cycle_focus(&next, false, window, cx);
            cx.stop_propagation();
        })
        .on_action(move |_: &crate::navigation::Previous, window, cx| {
            cycle_focus(&previous, true, window, cx);
            cx.stop_propagation();
        })
}

pub fn modal<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    body: impl IntoElement,
    footer: impl IntoElement,
    notice: Option<String>,
    state: &ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    modal_surface(
        id,
        title,
        None,
        body,
        Some(footer.into_any_element()),
        notice,
        state,
        window,
        cx,
        dismissible,
        close,
    )
}
pub fn detail_modal<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    body: impl IntoElement,
    notice: Option<String>,
    state: &ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    modal_surface(
        id,
        title,
        None,
        body,
        None,
        notice,
        state,
        window,
        cx,
        dismissible,
        close,
    )
}

pub(crate) struct TitleAction {
    pub(crate) editor: Option<gpui::AnyElement>,
    pub(crate) action: gpui::AnyElement,
}

pub fn detail_modal_with_title_action<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    title_editor: Option<gpui::AnyElement>,
    title_action: impl IntoElement,
    body: impl IntoElement,
    notice: Option<String>,
    state: &ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    modal_surface(
        id,
        title,
        Some(TitleAction {
            editor: title_editor,
            action: title_action.into_any_element(),
        }),
        body,
        None,
        notice,
        state,
        window,
        cx,
        dismissible,
        close,
    )
}

pub fn modal_preview<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    body: impl IntoElement,
    footer: Option<gpui::AnyElement>,
    notice: Option<String>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    preview_height: Option<f32>,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    modal_preview_with_title_action(
        id,
        title,
        None,
        body,
        footer,
        notice,
        focus,
        window,
        cx,
        dismissible,
        preview_height,
        close,
    )
}

fn modal_preview_with_title_action<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    title_action: Option<TitleAction>,
    body: impl IntoElement,
    footer: Option<gpui::AnyElement>,
    notice: Option<String>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    preview_height: Option<f32>,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let id = id.into();
    let title = title.into();
    let viewport = window.viewport_size();
    let max_height = preview_height.map(px).unwrap_or(viewport.height) - px(40.);
    let close = bind_close(cx, close);
    let contents = panel_contents_with_title_action(
        id.clone(),
        title.clone(),
        title_action,
        body.into_any_element(),
        footer,
        notice,
        focus,
        max_height,
        None,
        window,
        cx,
        dismissible,
        close,
    );
    crate::components::smooth::surface(id, ui::MODAL_RADIUS)
        .occlude()
        .w(px(ui::DIALOG_WIDTH).min(viewport.width - px(40.)))
        .max_w_full()
        .max_h(max_height)
        .flex()
        .flex_col()
        .bg(rgb(ZORK_UI.palette.canvas))
        .border(gpui::px(crate::design::BORDER_WIDTH))
        .border_color(rgb(crate::design::LIQUID_OUTLINE))
        .child(contents)
        .automation(AutomationRole::Status, title.to_string())
        .into_any_element()
}

/// A modal embedded in an existing material surface uses the same header, body,
/// footer, close action and focus scope as a window-level dialog.
pub fn panel_contents<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    body: impl IntoElement,
    footer: Option<gpui::AnyElement>,
    notice: Option<String>,
    focus: &FocusHandle,
    max_height: gpui::Pixels,
    clip: Option<crate::components::liquid::ContentClipBinding>,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    let close = bind_close(cx, close);
    panel_contents_with_title_action(
        id.into(),
        title.into(),
        None,
        body.into_any_element(),
        footer,
        notice,
        focus,
        max_height,
        clip,
        window,
        cx,
        dismissible,
        close,
    )
}
pub(crate) type CloseAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) fn bind_close<V: 'static>(
    cx: &Context<V>,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> CloseAction {
    let owner = cx.entity().downgrade();
    Rc::new(move |window, cx| {
        let _ = owner.update(cx, |view, cx| close(view, window, cx));
    })
}

pub(crate) fn panel_contents_with_title_action(
    id: gpui::SharedString,
    title: gpui::SharedString,
    title_action: Option<TitleAction>,
    body: gpui::AnyElement,
    footer: Option<gpui::AnyElement>,
    notice: Option<String>,
    focus: &FocusHandle,
    max_height: gpui::Pixels,
    clip: Option<crate::components::liquid::ContentClipBinding>,
    window: &mut Window,
    cx: &mut App,
    dismissible: bool,
    close: CloseAction,
) -> gpui::AnyElement {
    let (title_editor, title_action) = title_action
        .map(|item| (item.editor, Some(item.action)))
        .unwrap_or((None, None));
    let has_footer = footer.is_some();
    let p = ZORK_UI.palette;
    let escape_close = close.clone();
    div()
        .id(format!("{id}-content"))
        .map(|panel| trap_focus(panel, focus))
        .w_full()
        .max_h(max_height)
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                if dismissible {
                    escape_close(window, cx);
                }
                cx.stop_propagation();
            }
        })
        .child(clip_section(
            &clip,
            div()
                .h(px(76.))
                .flex_shrink_0()
                .px(px(24.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .mr_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(title_editor.unwrap_or_else(|| {
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(20.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title.clone())
                                .into_any_element()
                        }))
                        .when_some(title_action, |v, action| v.child(action)),
                )
                .child(
                    crate::components::liquid::controls::action(
                        format!("{id}-close"),
                        "",
                        32.,
                        32.,
                        crate::components::liquid::controls::ActionStyle {
                            quiet: true,
                            icon: Some("icons/x.svg"),
                            disabled: !dismissible,
                            ..Default::default()
                        },
                        p.canvas,
                        window,
                        cx,
                    )
                    .w(px(ui::CONTROL_HEIGHT))
                    .h(px(ui::CONTROL_HEIGHT))
                    .px_0()
                    .border_0()
                    .on_click(move |_, window, cx| {
                        if dismissible {
                            close(window, cx);
                        }
                    })
                    .automation_enabled(
                        dismissible,
                        AutomationRole::Button,
                        format!("关闭{title}"),
                    ),
                ),
            22.,
            22.,
        ))
        .when_some(notice, |v, notice| {
            v.child(clip_section(
                &clip,
                div().px(px(24.)).pb_3().child(ui::feedback(notice)),
                0.,
                12.,
            ))
        })
        .child(clip_section(
            &clip,
            div()
                .id(format!("{id}-body"))
                .min_h_0()
                .max_h(max_height - px(if has_footer { 160. } else { 90. }))
                .overflow_y_scroll()
                .px(px(24.))
                .pt_1()
                .pb(px(if has_footer { 12. } else { 24. }))
                .child(body),
            4.,
            if has_footer { 12. } else { 24. },
        ))
        .when_some(footer, |v, footer| {
            v.child(clip_section(
                &clip,
                div().flex_shrink_0().px(px(24.)).pt_5().pb_6().child(
                    div()
                        .id(format!("{id}-footer"))
                        .w_full()
                        .child(footer)
                        .automation(AutomationRole::Status, "弹窗操作区"),
                ),
                20.,
                24.,
            ))
        })
        .into_any_element()
}

fn clip_section(
    clip: &Option<crate::components::liquid::ContentClipBinding>,
    content: impl IntoElement,
    top: f32,
    bottom: f32,
) -> gpui::AnyElement {
    match clip {
        Some(clip) => clip.region(content, top, bottom).into_any_element(),
        None => content.into_any_element(),
    }
}

fn modal_surface<V: 'static>(
    id: impl Into<gpui::SharedString>,
    title: impl Into<gpui::SharedString>,
    title_action: Option<TitleAction>,
    body: impl IntoElement,
    footer: Option<gpui::AnyElement>,
    notice: Option<String>,
    state: &ModalState,
    window: &mut Window,
    cx: &mut Context<V>,
    dismissible: bool,
    close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) -> gpui::AnyElement {
    state.render(
        id.into(),
        title.into(),
        title_action,
        body,
        footer,
        notice,
        dismissible,
        window,
        cx,
        close,
    )
}
