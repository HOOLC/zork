//! Real shared actions plus fixed state swatches for visual comparison.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    controls as ui,
    design::{INTERACTION, ZORK_UI},
};
use gpui::{div, prelude::*, px, rgb, Context, Render, Window};

pub struct InteractionStory {
    id: String,
    clicks: usize,
}
impl InteractionStory {
    pub fn new(id: String) -> Self {
        Self { id, clicks: 0 }
    }
}
impl Render for InteractionStory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = ZORK_UI.palette;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(div().text_size(px(19.)).child("交互反馈 · 第一版"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child("试着移动鼠标、按住按钮，或按 Tab 定位。上排可操作，下排为状态对照。"),
            )
            .children(
                [
                    ("canvas", "白色内容区", p.canvas),
                    ("sidebar", "暖灰侧栏", p.sidebar),
                ]
                .into_iter()
                .map(|(surface, title, color)| {
                    div()
                        .w_full()
                        .p_4()
                        .rounded(px(crate::design::RADIUS.surface))
                        .border(gpui::px(crate::design::BORDER_WIDTH))
                        .border_color(rgb(p.border_strong))
                        .bg(rgb(color))
                        .flex()
                        .flex_col()
                        .gap_4()
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(p.muted))
                                .child(title),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_4()
                                .children(
                                    [
                                        ("chat", "聊天", "icons/x.svg"),
                                        ("browser", "浏览器", "icons/arrow-left.svg"),
                                        ("settings", "设置", "icons/settings.svg"),
                                    ]
                                    .into_iter()
                                    .map(
                                        |(key, label, icon)| {
                                            let id = format!("{}-{surface}-{key}", self.id);
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    ui::icon_button_sized(
                                                        id.clone(),
                                                        true,
                                                        ui::IconButtonSize::Compact,
                                                    )
                                                    .child(ui::icon(icon, 16.))
                                                    .on_click(cx.listener(|v, _, _, cx| {
                                                        v.clicks += 1;
                                                        cx.notify();
                                                    }))
                                                    .automation(AutomationRole::Button, label),
                                                )
                                                .child(div().text_size(px(12.)).child(label))
                                        },
                                    ),
                                )
                                .child(
                                    ui::quiet_button(
                                        format!("{}-{surface}-quiet", self.id),
                                        "附件",
                                        true,
                                        ui::IconButtonSize::Compact,
                                    )
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.clicks += 1;
                                        cx.notify();
                                    }))
                                    .automation(AutomationRole::Button, "附件"),
                                )
                                .child(
                                    ui::button(
                                        format!("{}-{surface}-primary", self.id),
                                        "主操作",
                                        true,
                                        true,
                                    )
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.clicks += 1;
                                        cx.notify();
                                    }))
                                    .automation(AutomationRole::Button, "主操作"),
                                ),
                        )
                        .child(
                            div().flex().gap_4().children(
                                ["默认", "悬停", "按下", "键盘焦点", "禁用"]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, label)| {
                                        div()
                                            .w(px(92.))
                                            .flex()
                                            .flex_col()
                                            .gap_2()
                                            .items_start()
                                            .child(
                                                ui::icon_button(
                                                    format!("{}-{surface}-state-{index}", self.id),
                                                    index != 4,
                                                )
                                                .child(ui::icon("icons/plus.svg", 16.))
                                                .when(index == 1, |v| {
                                                    v.bg(rgb(INTERACTION.neutral_hover))
                                                })
                                                .when(index == 2, |v| {
                                                    v.bg(rgb(INTERACTION.neutral_pressed))
                                                })
                                                .when(index == 3, |v| {
                                                    v.border_color(rgb(INTERACTION.focus_border))
                                                })
                                                .tab_stop(false),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(p.muted))
                                                    .child(label),
                                            )
                                    }),
                            ),
                        )
                }),
            )
            .child(
                div()
                    .id(format!("{}-count", self.id))
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .child(format!("已触发 {} 次操作", self.clicks))
                    .automation(
                        AutomationRole::Status,
                        format!("已触发 {} 次操作", self.clicks),
                    ),
            )
    }
}
