use super::surface;
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};

pub fn data_list(
    id: impl Into<SharedString>,
    rows: Vec<(SharedString, SharedString)>,
    width: f32,
) -> AnyElement {
    let id = id.into();
    let key_width = (width * 0.3).clamp(60., 120.);
    let mut list = div()
        .id(id.clone())
        .role(Role::DescriptionList)
        .w(px(width))
        .flex()
        .flex_col()
        .gap(px(10.))
        .text_size(px(13.))
        .line_height(px(21.));
    for (index, (label, value)) in rows.into_iter().enumerate() {
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(12.))
                .child(
                    div()
                        .id(format!("{id}-term-{index}"))
                        .role(Role::Term)
                        .aria_label(label.clone())
                        .w(px(key_width))
                        .flex_shrink_0()
                        .text_color(rgb(ZORK_UI.palette.muted))
                        .child(label),
                )
                .child(
                    div()
                        .id(format!("{id}-definition-{index}"))
                        .role(Role::Definition)
                        .aria_label(value.clone())
                        .min_w_0()
                        .flex_1()
                        .child(value),
                ),
        );
    }
    list.automation(AutomationRole::Status, "数据列表")
        .into_any_element()
}

pub struct Column {
    pub label: SharedString,
    pub width: f32,
    pub numeric: bool,
}
pub struct Row {
    pub key: String,
    pub cells: Vec<SharedString>,
}
pub fn table(
    id: impl Into<SharedString>,
    columns: Vec<Column>,
    rows: Vec<Row>,
    width: f32,
) -> AnyElement {
    let id = id.into();
    let count = columns.len();
    let total = columns.iter().map(|c| c.width).sum::<f32>().max(width);
    let mut table = div()
        .id(id.clone())
        .role(Role::Table)
        .aria_row_count(rows.len() + 1)
        .aria_column_count(count)
        .w(px(total))
        .flex()
        .flex_col()
        .text_size(px(12.))
        .line_height(px(20.));
    let mut header = div()
        .id(format!("{id}-head"))
        .role(Role::Row)
        .aria_row_index(0)
        .flex()
        .bg(rgb(ZORK_UI.palette.prompt));
    for (i, column) in columns.iter().enumerate() {
        header = header.child(
            div()
                .id(format!("{id}-column-{i}"))
                .role(Role::ColumnHeader)
                .aria_column_index(i)
                .aria_label(column.label.clone())
                .w(px(column.width))
                .flex_shrink_0()
                .px(px(12.))
                .py(px(9.))
                .font_weight(FontWeight::SEMIBOLD)
                .when(column.numeric, |v| v.text_right())
                .child(column.label.clone()),
        );
    }
    table = table.child(header);
    for (index, row) in rows.into_iter().enumerate() {
        let mut cells = div()
            .id(format!("{id}-row-{}", row.key))
            .role(Role::Row)
            .aria_row_index(index + 1)
            .flex()
            .border_b(px(crate::design::BORDER_WIDTH))
            .border_color(rgb(ZORK_UI.palette.border));
        for (column, cell) in columns.iter().zip(row.cells).enumerate() {
            let (spec, text) = cell;
            cells = cells.child(
                div()
                    .id(format!("{id}-cell-{}-{column}", row.key))
                    .role(if column == 0 {
                        Role::RowHeader
                    } else {
                        Role::Cell
                    })
                    .aria_column_index(column)
                    .aria_label(text.clone())
                    .w(px(spec.width))
                    .flex_shrink_0()
                    .px(px(12.))
                    .py(px(9.))
                    .when(spec.numeric, |v| v.text_right())
                    .child(text),
            );
        }
        table = table.child(cells);
    }
    div()
        .id(format!("{id}-viewport"))
        .w(px(width))
        .overflow_x_scroll()
        .child(table)
        .automation(AutomationRole::Status, "表格")
        .into_any_element()
}

pub fn separator(
    id: impl Into<SharedString>,
    vertical: bool,
    decorative: bool,
    length: f32,
) -> AnyElement {
    div()
        .id(id.into())
        .when(!decorative, |v| {
            v.role(Role::Splitter).aria_orientation(if vertical {
                Orientation::Vertical
            } else {
                Orientation::Horizontal
            })
        })
        .when(vertical, |v| {
            v.w(px(crate::design::BORDER_WIDTH)).h(px(length))
        })
        .when(!vertical, |v| {
            v.h(px(crate::design::BORDER_WIDTH)).w(px(length))
        })
        .bg(rgb(ZORK_UI.palette.border))
        .into_any_element()
}
pub fn aspect_ratio(
    id: impl Into<SharedString>,
    ratio: f32,
    width: f32,
    content: impl IntoElement,
) -> AnyElement {
    let ratio = if ratio.is_finite() && ratio > 0. {
        ratio
    } else {
        1.
    };
    div()
        .id(id.into())
        .relative()
        .w(px(width))
        .aspect_ratio(ratio)
        .child(div().absolute().inset_0().child(content))
        .automation(AutomationRole::Status, format!("比例 {ratio:.3}"))
        .into_any_element()
}
#[derive(Clone, Copy)]
pub enum Inset {
    Top,
    Horizontal,
    All,
}
pub fn inset(content: impl IntoElement, padding: f32, edge: Inset) -> Div {
    let padding = padding.max(0.);
    div()
        .when(matches!(edge, Inset::Top | Inset::All), |v| {
            v.mt(-px(padding))
        })
        .when(
            matches!(edge, Inset::Top | Inset::Horizontal | Inset::All),
            |v| v.mx(-px(padding)),
        )
        .when(matches!(edge, Inset::All), |v| v.mb(-px(padding)))
        .child(content)
}

#[derive(Default)]
pub struct ScrollArea {
    handle: ScrollHandle,
}
impl ScrollArea {
    pub fn inspect(&self) -> serde_json::Value {
        let offset = self.handle.offset();
        serde_json::json!({"x":offset.x.as_f32(),"y":offset.y.as_f32()})
    }
    pub fn render(
        &self,
        id: impl Into<SharedString>,
        width: f32,
        height: f32,
        content: impl IntoElement,
    ) -> AnyElement {
        let id = id.into();
        let handle = self.handle.clone();
        let viewport = div()
            .id(format!("{id}-viewport"))
            .size_full()
            .overflow_scroll()
            .track_scroll(&handle)
            .focusable()
            .tab_stop(true)
            .role(Role::ScrollView)
            .aria_label("可滚动内容")
            .on_key_down(move |e: &KeyDownEvent, w, cx| {
                let mut offset = handle.offset();
                match e.keystroke.key.as_str() {
                    "down" => offset.y -= px(40.),
                    "up" => offset.y += px(40.),
                    "right" => offset.x -= px(40.),
                    "left" => offset.x += px(40.),
                    "pagedown" => offset.y -= px(height * 0.85),
                    "pageup" => offset.y += px(height * 0.85),
                    "home" => offset = point(px(0.), px(0.)),
                    "end" => offset.y = -px(1_000_000.),
                    _ => return,
                }
                handle.set_offset(offset);
                w.refresh();
                w.prevent_default();
                cx.stop_propagation();
            })
            .child(content)
            .automation(AutomationRole::Status, "滚动区域");
        surface(id.clone(), 0., ZORK_UI.palette.canvas, true)
            .w(px(width))
            .h(px(height))
            .child(viewport)
            .child(
                gpui_base::Scrollbar::new(&self.handle)
                    .id(format!("{id}-scrollbar"))
                    .mode(gpui_base::ScrollbarMode::Always)
                    .styles(|s| {
                        s.thumb(|t| {
                            t.bg(rgb(ZORK_UI.palette.muted))
                                .width(px(6.))
                                .radius(px(3.))
                        })
                    }),
            )
            .into_any_element()
    }
}
