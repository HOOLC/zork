//! The unified design at a glance, built only from production components so it
//! can be compared with the approved HTML specification item by item.
use crate::{
    components::{text_input::ComposerInput, widgets::controls as widgets},
    controls as ui,
    design::{TextRole, INTERACTION, RADIUS, ZORK_UI},
    device_name::{self, DeviceStatus},
};
use gpui::{div, prelude::*, px, rgb, AnyElement, App, Div, Entity};

fn section(title: &str, description: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(ui::text_role(title.to_owned(), TextRole::SectionTitle))
                .child(ui::text_role(description.to_owned(), TextRole::Description)),
        )
}

fn caption(text: impl Into<gpui::SharedString>) -> Div {
    ui::text_role(text, TextRole::Metadata)
}

fn swatch(name: &str, color: u32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .w(px(76.))
                .h(px(40.))
                .rounded(px(RADIUS.block))
                .border(px(crate::design::BORDER_WIDTH))
                .border_color(rgb(ZORK_UI.palette.border_strong))
                .bg(rgb(color)),
        )
        .child(caption(name.to_owned()))
}

fn corner(label: String, radius: f32, width: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(4.))
        .child(
            div()
                .w(px(width))
                .h(px(44.))
                .rounded(px(radius))
                .border(px(1.5))
                .border_color(rgb(ZORK_UI.palette.muted)),
        )
        .child(caption(label))
}

fn danger(id: &str, text: &str) -> widgets::Action {
    widgets::adaptive_action(
        id.to_owned(),
        text.to_owned(),
        widgets::ActionStyle {
            variant: Some(widgets::ButtonVariant::Danger),
            icon_only: Some(false),
            ..Default::default()
        },
        ZORK_UI.palette.canvas,
    )
}

pub(crate) fn overview(input: &Entity<ComposerInput>, cx: &mut App) -> AnyElement {
    let p = ZORK_UI.palette;
    let colors = section(
        "颜色",
        "暖纸打底、炭墨写字；柿橙只表示正在工作和发送；设备色只标身份，状态色只表状态。",
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .gap(px(12.))
            .child(swatch("window", p.window))
            .child(swatch("canvas", p.canvas))
            .child(swatch("prompt", p.prompt))
            .child(swatch("selected", p.selected))
            .child(swatch("border", p.border_strong))
            .child(swatch("text", p.text))
            .child(swatch("muted", p.muted))
            .child(swatch("subtle", p.subtle))
            .child(swatch("accent", INTERACTION.accent))
            .child(swatch("success", p.success))
            .child(swatch("warning", p.warning))
            .child(swatch("danger", p.danger)),
    )
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(caption("设备色"))
            .children(
                ["Studio", "MBA", "mini1", "Pixel 9", "Linux"]
                    .into_iter()
                    .map(|name| device_name::mark(name, 32.)),
            ),
    );

    let type_roles = section("文字角色", "层级靠字号和字重，不靠更浅的灰；最小 12 px。").children(
        [
            ("PageTitle", "模型连接", TextRole::PageTitle),
            ("SectionTitle", "Anthropic", TextRole::SectionTitle),
            ("Body", "控件、列表与表单", TextRole::Body),
            ("Label", "字段标题 · 分组", TextRole::Label),
            ("Description", "说明文字保持可读对比", TextRole::Description),
            ("Metadata", "14:33 · opus-5.5", TextRole::Metadata),
        ]
        .into_iter()
        .map(|(name, sample, role)| {
            div()
                .flex()
                .items_baseline()
                .gap(px(16.))
                .child(div().w(px(110.)).child(caption(name)))
                .child(ui::text_role(sample, role))
        }),
    );

    let corners = section(
        "圆角",
        "控件是胶囊；容器随之放大，内圆角 = 外圆角 − 内边距；全部为平滑圆角。",
    )
    .child(
        div()
            .flex()
            .items_end()
            .gap(px(18.))
            .child(corner(format!("{} 行内", RADIUS.inline), RADIUS.inline, 44.))
            .child(corner(format!("{} 控件", RADIUS.control), RADIUS.control, 88.))
            .child(corner(format!("{} 内嵌块", RADIUS.block), RADIUS.block, 44.))
            .child(corner(format!("{} 容器", RADIUS.container), RADIUS.container, 64.))
            .child(corner(format!("{} 表面", RADIUS.surface), RADIUS.surface, 88.)),
    );

    let buttons = section(
        "按钮",
        "主操作炭墨，危险操作红色，图标一侧少 4 px；焦点是轮廓外的柿橙环。",
    )
    .child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(ui::button("unified-primary", "保存", true, true))
            .child(ui::button("unified-secondary", "取消", false, true))
            .child(ui::quiet_button(
                "unified-quiet",
                "删除",
                true,
                ui::IconButtonSize::Standard,
            ))
            .child(danger("unified-danger", "清空数据"))
            .child(ui::button("unified-disabled", "不可用", true, false))
            .child(ui::busy_button("unified-busy", "保存中", true, false, true))
            .child(ui::page_action("unified-icon", "添加连接"))
            .child(ui::button("unified-focus", "键盘焦点", false, true).shadow(ui::focus_ring())),
    );

    let tabs = crate::navigation::TabGroup::new(cx);
    let rows = section("导航行", "32 px 胶囊；选中行悬停时加深而不被覆盖。").child(
        div()
            .w(px(300.))
            .p(px(8.))
            .rounded(px(RADIUS.container))
            .bg(rgb(p.sidebar))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                tabs.tab("unified-row-new".into(), false)
                    .child(ui::icon("icons/plus.svg", 16.))
                    .child("新建 Chat"),
            )
            .child(
                tabs.tab("unified-row-selected".into(), true)
                    .child(device_name::mark("Studio", 18.))
                    .child("统一 Zork 设计系统"),
            )
            .child(
                tabs.tab("unified-row-plain".into(), false)
                    .child(device_name::mark("MBA", 18.))
                    .child("修复 Android 底部表单"),
            ),
    );

    let fields = section("字段", "悬停与焦点加深边框；焦点同样使用柿橙环。").child(
        div()
            .w(px(300.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(ui::form_field(
                "设备名称",
                ui::input_control("unified-field", input, false, cx),
            )),
    );

    let devices = section("设备身份与状态", "身份靠折角色块；状态靠形状加文字，不只靠颜色。").child(
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(
                [
                    ("Studio", DeviceStatus::Direct),
                    ("mini1", DeviceStatus::Relay),
                    ("Linux Box", DeviceStatus::Connecting),
                    ("Pixel 9", DeviceStatus::Offline),
                    ("旧 iMac", DeviceStatus::Revoked),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (name, status))| {
                    div().w(px(300.)).child(device_name::label(
                        format!("unified-device-{index}"),
                        name,
                        &status,
                        None,
                    ))
                }),
            ),
    );

    let feedback = section("反馈", "失败写明原因并给出操作；离线与等待分别表达。").child(
        div()
            .w(px(420.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(ui::status_notice(
                "无法读取 mini1 上的连接 · 中继请求超时".into(),
                ui::NoticeKind::Warning,
            ))
            .child(ui::status_notice(
                "消息加载失败 · Studio 返回 503".into(),
                ui::NoticeKind::Error,
            ))
            .child(ui::status_notice(
                "离线中 · 3 条消息会在 Studio 恢复后发送".into(),
                ui::NoticeKind::Info,
            )),
    );

    div()
        .id("unified-design-overview")
        .w_full()
        .p(px(32.))
        .flex()
        .flex_col()
        .gap(px(32.))
        .bg(rgb(p.canvas))
        .child(colors)
        .child(
            div()
                .flex()
                .gap(px(48.))
                .child(div().flex_1().child(type_roles))
                .child(div().flex_1().child(corners)),
        )
        .child(buttons)
        .child(
            div()
                .flex()
                .gap(px(48.))
                .child(rows)
                .child(fields)
                .child(devices),
        )
        .child(feedback)
        .into_any_element()
}
