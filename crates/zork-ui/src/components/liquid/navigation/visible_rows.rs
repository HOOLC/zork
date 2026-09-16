//! Fixed row slots retain the complete focus order. Only visible/focused slots
//! construct text and material; offscreen slots keep lightweight input targets.
use super::*;

struct Row {
    id: String,
    label: SharedString,
    detail: Option<SharedString>,
    heading: bool,
    pose: Pose,
}
pub(super) struct Rows<V: 'static> {
    id: SharedString,
    items: Vec<Row>,
    width: f32,
    height: f32,
    group: Group,
    focus: Vec<FocusHandle>,
    style: Style,
    enabled: bool,
    selected: usize,
    hot: Option<usize>,
    keyboard_row: Option<usize>,
    choose: Rc<dyn Fn(&mut V, usize, &mut Context<V>)>,
    parent: WeakEntity<V>,
}
impl<V: 'static> Rows<V> {
    pub(super) fn new(
        id: SharedString,
        items: Vec<Item>,
        poses: Vec<Pose>,
        width: f32,
        height: f32,
        group: Group,
        focus: Vec<FocusHandle>,
        style: Style,
        enabled: bool,
        selected: usize,
        hot: Option<usize>,
        keyboard_row: Option<usize>,
        choose: Rc<dyn Fn(&mut V, usize, &mut Context<V>)>,
        parent: WeakEntity<V>,
    ) -> Self {
        let items = items
            .into_iter()
            .zip(poses)
            .map(|(item, pose)| Row {
                id: item.id,
                label: item.label,
                detail: item.detail,
                heading: item.heading,
                pose,
            })
            .collect();
        Self {
            id,
            items,
            width,
            height,
            group,
            focus,
            style,
            enabled,
            selected,
            hot,
            keyboard_row,
            choose,
            parent,
        }
    }
}
impl<V: 'static> IntoElement for Rows<V> {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl<V: 'static> Element for Rows<V> {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = gpui::Style::default();
        style.size = size(px(self.width).into(), px(self.height).into());
        (window.request_layout(style, [], cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let clip = window
            .content_mask()
            .bounds
            .intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
        let full: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let b = Bounds::new(
                    bounds.origin + point(px(row.pose.left() as f32), px(row.pose.top() as f32)),
                    size(px(row.pose.w as f32), px(row.pose.h as f32)),
                );
                let visible = b.intersect(&clip);
                (
                    b,
                    visible.size.width > px(0.) && visible.size.height > px(0.)
                        || self.focus[i].is_focused(window),
                )
            })
            .collect();
        let ids = full
            .iter()
            .enumerate()
            .filter(|(_, (_, visible))| *visible)
            .map(|(i, _)| self.items[i].id.clone().into())
            .collect();
        self.group.retain_visible_anchors(&ids);
        let mut children = div().relative().w(px(self.width)).h(px(self.height));
        for (i, (row, (row_bounds, full))) in self.items.iter().zip(full).enumerate() {
            let element = self
                .parent
                .update(cx, |_, cx| {
                    if full {
                        let item = Item {
                            id: row.id.clone(),
                            label: row.label.clone(),
                            detail: row.detail.clone(),
                            heading: row.heading,
                            gap_before: 0.,
                            trailing: None,
                        };
                        let pose = Pose::rect(0., 0., row.pose.w, row.pose.h, row.pose.r);
                        navigation_row(
                            self.id.clone(),
                            item,
                            pose,
                            &self.group,
                            &self.focus[i],
                            self.style,
                            self.enabled,
                            self.selected == i,
                            self.hot == Some(i) || self.keyboard_row == Some(i),
                            i,
                            self.choose.clone(),
                            cx,
                        )
                    } else {
                        let focus = self.focus[i].clone();
                        let choose = self.choose.clone();
                        let enabled = self.enabled;
                        div()
                            .id(row.id.clone())
                            .w(px(row.pose.w as f32))
                            .h(px(row.pose.h as f32))
                            .track_focus(&focus)
                            .on_click(cx.listener(move |v, _, window, cx| {
                                if enabled {
                                    window.focus(&focus, cx);
                                    choose(v, i, cx);
                                }
                            }))
                            .automation_enabled(
                                enabled,
                                AutomationRole::Button,
                                row.label.to_string(),
                            )
                            .into_any_element()
                    }
                })
                .unwrap_or_else(|_| gpui::Empty.into_any_element());
            children = children.child(
                div()
                    .absolute()
                    .left(row_bounds.origin.x - bounds.origin.x)
                    .top(row_bounds.origin.y - bounds.origin.y)
                    .w(row_bounds.size.width)
                    .h(row_bounds.size.height)
                    .child(element),
            );
        }
        // All slots share one layout root, including the lightweight offscreen
        // focus targets. Their fixed measured positions keep the same order
        // and hit geometry without restarting the layout engine for every row.
        let mut element = children.into_any_element();
        element.layout_as_root(bounds.size.map(AvailableSpace::Definite), window, cx);
        element.prepaint_at(bounds.origin, window, cx);
        element
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}
