//! Review-only resource concept composed from the existing native settings controls.
use gpui::{
    div, prelude::*, px, rgb, AppContext, Context, Entity, HeadlessAppContext, Render, Window,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{AutomationElementExt, AutomationRole, AutomationRoot, HeadlessAutomation},
    components::brand::{Brand, BrandMotion},
};
use zork_ui::{
    controls as ui,
    design::{TextRole, CUE_UI},
    navigation::TabGroup,
};

struct Concept {
    nav: TabGroup,
    brand: Entity<Brand>,
    modal: ui::ModalState,
    selected: Option<usize>,
    technical: bool,
}
impl Concept {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            nav: TabGroup::new(cx),
            brand: cx.new(|_| Brand::new(BrandMotion::Header, CUE_UI.palette.sidebar)),
            modal: ui::ModalState::new(cx),
            selected: None,
            technical: false,
        }
    }
    fn fact(&self, label: &str, value: &str) -> impl IntoElement {
        div()
            .flex()
            .gap_5()
            .py_2()
            .child(
                div()
                    .w(px(90.))
                    .flex_shrink_0()
                    .child(ui::text_role(label.to_owned(), TextRole::Description)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(ui::text_role(value.to_owned(), TextRole::Body)),
            )
    }
    fn row(
        &self,
        id: usize,
        name: &str,
        description: &str,
        icon: &'static str,
        status: &str,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        ui::quiet_button(
            format!("concept-resource-{id}"),
            "",
            true,
            ui::IconButtonSize::Standard,
        )
        .w_full()
        .h(px(64.))
        .px_2()
        .gap_3()
        .justify_start()
        .child(
            div()
                .size(px(36.))
                .rounded(px(12.))
                .bg(rgb(CUE_UI.palette.sidebar))
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon(icon, 20.)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(ui::text_role(name.to_owned(), TextRole::Body))
                .child(ui::text_role(description.to_owned(), TextRole::Description)),
        )
        .child(ui::text_role(status.to_owned(), TextRole::Metadata))
        .on_click(cx.listener(move |v, _, _, cx| {
            v.selected = Some(id);
            v.technical = false;
            cx.notify();
        }))
        .automation(AutomationRole::Button, name.to_owned())
    }
}
impl Render for Concept {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = CUE_UI.palette;
        self.modal
            .sync(self.selected.map(|_| "concept-detail"), w, cx);
        let sidebar = self.nav.surface(
            self.nav
                .column()
                .w(px(240.))
                .h_full()
                .bg(rgb(p.sidebar))
                .px_2()
                .child(div().h(px(48.)).pl(px(64.)).child(self.brand.clone()))
                .child(
                    self.nav
                        .tab("concept-back".into(), false)
                        .child(ui::icon("icons/arrow-left.svg", 16.))
                        .child("返回对话"),
                )
                .child(
                    self.nav
                        .section("concept-client-heading", "客户端设置")
                        .child(
                            self.nav
                                .tab("concept-appearance".into(), false)
                                .child("外观"),
                        )
                        .child(
                            self.nav
                                .tab("concept-notifications".into(), false)
                                .child("通知"),
                        ),
                )
                .child(
                    self.nav
                        .section("concept-device-heading", "设备设置")
                        .child(
                            self.nav
                                .tab("concept-device".into(), false)
                                .child(ui::icon("icons/node.svg", 20.))
                                .child("mini1"),
                        )
                        .child(
                            self.nav
                                .tab("concept-agents".into(), false)
                                .pl(px(36.))
                                .child("队员"),
                        )
                        .child(
                            self.nav
                                .tab("concept-models".into(), false)
                                .pl(px(36.))
                                .child("大模型"),
                        )
                        .child(
                            self.nav
                                .tab("concept-resources".into(), true)
                                .pl(px(36.))
                                .child("资源"),
                        ),
                )
                .child(
                    self.nav
                        .tab("concept-add-device".into(), false)
                        .child(ui::icon("icons/plus.svg", 20.))
                        .child("连接设备"),
                ),
        );
        let section = |title: &'static str, count: &'static str| {
            div()
                .px_2()
                .pt_6()
                .pb_2()
                .flex()
                .gap_2()
                .items_center()
                .child(ui::text_role(title, TextRole::Label))
                .child(ui::text_role(count, TextRole::Metadata))
        };
        let content = div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_start()
                    .pb_2()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(ui::page_title("资源"))
                            .child(ui::text_role("mini1 · 4 项资源", TextRole::Description)),
                    )
                    .child(ui::button("concept-refresh", "刷新", false, true).text_size(px(11.))),
            )
            .child(section("Skill", "2"))
            .child(self.row(
                0,
                "live-echo-guide",
                "回声服务的使用说明与辅助脚本",
                "icons/workspace.svg",
                "研究领队",
                cx,
            ))
            .child(self.row(
                1,
                "zork-validation",
                "项目构建与回归验证流程",
                "icons/workspace.svg",
                "资料整理",
                cx,
            ))
            .child(section("MCP", "1"))
            .child(self.row(
                2,
                "live-echo",
                "回声工具 · 用于验证调用结果",
                "icons/mesh.svg",
                "本机可用",
                cx,
            ))
            .child(section("Service", "1"))
            .child(self.row(
                3,
                "报告预览",
                "工具验收 · 研究领队",
                "icons/panel-right.svg",
                "运行中",
                cx,
            ));
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .bg(rgb(p.canvas))
            .font_family("Inter Variable")
            .text_color(rgb(p.text))
            .text_size(px(13.))
            .child(sidebar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(ui::settings_content(content)),
            );
        if let Some(id) = self.selected {
            let (name, desc) = match id {
                0 => ("live-echo-guide", "回声服务的使用说明与辅助脚本"),
                1 => ("zork-validation", "项目构建与回归验证流程"),
                2 => ("live-echo", "回声工具 · 用于验证调用结果"),
                _ => ("报告预览", "工具验收生成的网页预览"),
            };
            let mut body = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(ui::text_role(desc, TextRole::Description));
            if id < 2 {
                body = body.child(ui::text_role("已绑定", TextRole::Label)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(ui::agent_avatar(
                            Some(if id == 0 { "fox" } else { "panda" }),
                            32.,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(ui::text_role(
                                    if id == 0 {
                                        "研究领队"
                                    } else {
                                        "资料整理"
                                    },
                                    TextRole::Body,
                                ))
                                .child(ui::text_role(
                                    if id == 0 { "领队" } else { "队员" },
                                    TextRole::Metadata,
                                )),
                        ),
                );
                body = body.child(self.fact(
                    "附带资源",
                    if id == 0 {
                        "1 个辅助脚本"
                    } else {
                        "3 份验证清单"
                    },
                ));
            } else if id == 2 {
                body = body
                    .child(self.fact("状态", "最近检查通过"))
                    .child(self.fact("使用范围", "本机 Agent"))
                    .child(self.fact("远端调用", "未共享"))
                    .child(self.fact("工具", "echo"));
            } else {
                body = body
                    .child(self.fact("状态", "运行中"))
                    .child(self.fact("所属任务", "工具验收"))
                    .child(self.fact("所属领队", "研究领队"))
                    .child(self.fact("共享范围", "已授权的 Mesh 客户端"));
            }
            body = body.child(
                ui::action_link(
                    "concept-technical",
                    if self.technical {
                        "收起技术信息"
                    } else {
                        "技术信息"
                    },
                    true,
                )
                .on_click(cx.listener(|v, _, _, cx| {
                    v.technical = !v.technical;
                    cx.notify();
                }))
                .automation(AutomationRole::Button, "技术信息"),
            );
            if self.technical {
                body = body.child(self.fact("设备", "mini1")).child(self.fact(
                    "标识",
                    match id {
                        0 => "01M258G5R2YREKRC1DS3GSHCA6",
                        1 => "01M258K0XZPQKP8B4YRXDYJQNA",
                        2 => "01M258FWGJRF2RWWE6M82H7E9P",
                        _ => "report-preview",
                    },
                ));
                if id < 2 {
                    body = body.child(self.fact(
                        "路径",
                        if id == 0 {
                            "skills/live-echo-guide"
                        } else {
                            "skills/zork-validation"
                        },
                    ));
                } else if id == 2 {
                    body = body.child(self.fact("连接方式", "stdio"));
                } else {
                    body = body.child(self.fact("端口", "3000"));
                }
            }
            root = root.child(ui::detail_modal(
                "concept-detail",
                name,
                body,
                None,
                &self.modal.focus,
                w,
                cx,
                true,
                |v, _, cx| {
                    v.selected = None;
                    v.technical = false;
                    cx.notify();
                },
            ));
        }
        root
    }
}
fn main() -> anyhow::Result<()> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../artifacts/resource-design-preview-v2");
    std::fs::create_dir_all(&dir)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        Arc::new(EmbeddedAssets),
        gpui_platform::current_headless_renderer,
    );
    let driver = cx.update(|cx| {
        zork_gui::assets::init_fonts(cx);
        zork_gui::components::init(cx);
        cx.set_reduce_motion(true);
        HeadlessAutomation::install(cx)
    });
    let window = cx.open_window(gpui::size(px(1024.), px(720.)), |_, cx| {
        let v = cx.new(Concept::new);
        cx.new(|_| AutomationRoot::new(v))
    })?;
    let pump = |cx: &mut HeadlessAppContext| -> anyhow::Result<()> {
        for _ in 0..5 {
            cx.advance_clock(Duration::from_millis(16));
            cx.run_until_parked();
            cx.update_window(window.into(), |_, w, cx| {
                w.simulate_next_frame(cx);
                w.draw(cx).clear(cx)
            })?;
        }
        Ok(())
    };
    let capture = |cx: &mut HeadlessAppContext, name: &str| -> anyhow::Result<()> {
        pump(cx)?;
        std::thread::sleep(Duration::from_millis(250));
        pump(cx)?;
        let image = cx.capture_screenshot(window.into())?;
        image::ImageEncoder::write_image(
            image::codecs::png::PngEncoder::new_with_quality(
                std::fs::File::create(dir.join(format!("{name}.png")))?,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            ),
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )?;
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_vec_pretty(&driver.snapshot(false))?,
        )?;
        Ok(())
    };
    let click = |cx: &mut HeadlessAppContext, id: &str| -> anyhow::Result<()> {
        cx.update_window(window.into(), |_, w, cx| {
            driver.dispatch(
                serde_json::from_value(json!({"type":"click","target":{"element_id":id}})).unwrap(),
                w,
                cx,
            )
        })??;
        pump(cx)
    };
    capture(&mut cx, "resources")?;
    for (id, name) in [(0, "skill"), (1, "validation"), (2, "mcp"), (3, "service")] {
        click(&mut cx, &format!("concept-resource-{id}"))?;
        capture(&mut cx, name)?;
        click(&mut cx, "concept-technical")?;
        capture(&mut cx, &format!("{name}-technical"))?;
        click(&mut cx, "concept-detail-close")?;
    }
    println!("PASS: native settings concept, resource dialogs and technical disclosure");
    Ok(())
}
