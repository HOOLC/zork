//! A host retains only the presentation payload needed for a dialog's exit.
//! Business completion/cancellation still happens immediately in the host.
use super::*;
use crate::components::liquid::{overlay::{Dialog, DialogOptions, Placement, SourceBinding}, Material};
use std::{any::Any, cell::RefCell, collections::HashMap};

struct Slot {
    dialog: Dialog,
    open: bool,
    controlled: bool,
    payload: Option<Box<dyn Any>>,
}

pub struct ModalState {
    pub focus: FocusHandle,
    active: Option<&'static str>,
    slots: RefCell<HashMap<gpui::SharedString, Slot>>,
}
impl ModalState {
    pub fn new(cx: &mut App) -> Self {
        Self { focus: cx.focus_handle(), active: None, slots: Default::default() }
    }
    /// Synchronization does not steal focus on parent rerenders. The complete
    /// dialog owns entry, Tab containment and return to its actual source.
    pub fn sync(&mut self, active: Option<&'static str>, _: &mut Window, _: &mut App) {
        self.active = active;
        if let Some(slot) = active.and_then(|id| self.slots.get_mut().get(id)) {
            self.focus = slot.dialog.focus_handle();
        }
    }
    /// Call on every owner render, including while closed. The returned clone is
    /// read-only presentation data; it does not re-enter the core's business list.
    pub fn retain<T: Clone + 'static>(
        &mut self,
        id: impl Into<gpui::SharedString>,
        value: Option<T>,
        cx: &mut App,
    ) -> Option<T> {
        let id = id.into();
        let slot = self.slots.get_mut().entry(id.clone()).or_insert_with(|| Slot {
            dialog: Dialog::new(cx), open: false, controlled: true, payload: None,
        });
        slot.controlled = true;
        slot.open = value.is_some();
        if let Some(value) = value { slot.payload = Some(Box::new(value)); }
        if !slot.open && !slot.dialog.alive() { slot.payload = None; }
        if self.active.is_some_and(|active| active == id.as_ref()) {
            self.focus = slot.dialog.focus_handle();
        }
        slot.payload.as_ref().map(|payload| {
            payload.downcast_ref::<T>().expect("one presentation type per dialog ID").clone()
        })
    }
    pub fn source(&self, id: &str) -> SourceBinding {
        self.slots.borrow().get(id).expect("retain the dialog before building its sources").dialog.source_binding()
    }
    pub fn bind_source(&mut self, id: impl Into<gpui::SharedString>, source: SourceBinding, cx: &mut App) {
        self.slots.get_mut().entry(id.into()).or_insert_with(|| Slot {
            dialog: Dialog::new(cx), open: false, controlled: true, payload: None,
        }).dialog.bind_source(source);
    }
    pub fn presentation<T: Clone + 'static>(&self, id: &str) -> Option<T> {
        self.slots.borrow().get(id)?.payload.as_ref()?.downcast_ref::<T>().cloned()
    }
    pub fn open(&self, id: &str) -> bool {
        self.slots.borrow().get(id).is_some_and(|slot| slot.open)
    }
    pub fn alive(&self, id: &str) -> bool {
        self.slots.borrow().get(id).is_some_and(|slot| slot.dialog.alive())
    }
    pub fn inspect(&self, id: &str) -> serde_json::Value {
        self.slots.borrow().get(id).map_or(serde_json::Value::Null, |slot| slot.dialog.inspect())
    }
    pub(super) fn render<V: 'static>(
        &self,
        id: gpui::SharedString,
        title: gpui::SharedString,
        title_action: Option<TitleAction>,
        body: impl IntoElement,
        footer: Option<gpui::AnyElement>,
        notice: Option<String>,
        dismissible: bool,
        window: &mut Window,
        cx: &mut Context<V>,
        close: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
    ) -> gpui::AnyElement {
        let mut slots = self.slots.borrow_mut();
        let slot = slots.entry(id.clone()).or_insert_with(|| Slot {
            dialog: Dialog::new(cx), open: true, controlled: false, payload: None,
        });
        // Direct compositions are supported, while retained hosts explicitly
        // provide open=false and keep constructing their last payload on exit.
        let open = !slot.controlled || slot.open;
        let (title_editor, title_action) = title_action.map(|action| (action.editor, Some(action.action))).unwrap_or_default();
        slot.dialog.render_with_options(
            id, title, body, footer, open,
            Placement::Window { width: ui::DIALOG_WIDTH }, Material::default(),
            DialogOptions { title_editor, title_action, notice, dismissible },
            window, cx, close,
        ).unwrap_or_else(|| gpui::Empty.into_any_element())
    }
}
