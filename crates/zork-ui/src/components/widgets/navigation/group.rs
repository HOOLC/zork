//! Navigation rows use GPUI's ordinary hover and selected states.
use super::*;

pub type GroupSurface = AnyElement;

#[derive(Clone)]
pub struct Group {
    style: Style,
    hot: Rc<RefCell<Option<ElementId>>>,
}

impl Group {
    pub fn new(_: &mut App) -> Self {
        Self {
            style: Style {
                kind: Kind::Sidebar,
                parent: ZORK_UI.palette.sidebar,
                row_radius: crate::controls::FIELD_RADIUS,
            },
            hot: Default::default(),
        }
    }

    pub fn keyed(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> Self {
        window
            .use_keyed_state(id, cx, |_, cx| Self::new(cx))
            .read(cx)
            .clone()
    }

    pub fn kind(mut self, kind: Kind) -> Self {
        self.style.kind = kind;
        self
    }

    pub fn column(&self) -> Div {
        div().flex().flex_col().gap(px(2.))
    }

    pub fn section(&self, id: impl Into<ElementId>, title: impl Into<SharedString>) -> Div {
        let title = title.into();
        self.column().flex_shrink_0().child(
            div()
                .id(id)
                .h(px(20.))
                .flex_shrink_0()
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(px(11.))
                .line_height(px(16.))
                .text_color(rgb(ZORK_UI.palette.muted))
                .child(title.clone())
                .automation(AutomationRole::Status, title),
        )
    }

    pub fn tab(&self, id: String, selected: bool) -> controls::Action {
        self.row(id, selected, true)
    }

    pub fn plate(&self, id: impl Into<ElementId>, selected: bool) -> controls::Action {
        self.row(id, selected, true)
    }

    pub fn row(&self, id: impl Into<ElementId>, selected: bool, enabled: bool) -> controls::Action {
        let id = id.into();
        let style = self.style;
        controls::adaptive_action(
            id.clone(),
            "",
            controls::ActionStyle {
                quiet: true,
                icon_only: Some(false),
                disabled: !enabled,
                radius: Some(style.row_radius),
                ..Default::default()
            },
            style.parent,
        )
        .group(format!("navigation-{id}"))
        .w_full()
        .h(px(crate::controls::CONTROL_HEIGHT))
        .px(px(7.))
        .gap_2()
        .justify_start()
        .font_weight(FontWeight::NORMAL)
        .when(style.kind == Kind::Tabs, |row| {
            row.role(Role::Tab).justify_center()
        })
        .when(selected, |row| {
            row.bg(rgb(ZORK_UI.palette.selected))
                .text_color(rgb(ZORK_UI.palette.text))
        })
    }

    pub fn surface(&self, content: impl IntoElement) -> GroupSurface {
        content.into_any_element()
    }

    pub fn hovered(&self) -> Option<ElementId> {
        self.hot.borrow().clone()
    }

    pub fn set_hover(&self, id: Option<ElementId>, _: &mut App) {
        *self.hot.borrow_mut() = id;
    }
}
