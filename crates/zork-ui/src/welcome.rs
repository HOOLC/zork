//! Welcome panel with independently retained copy and the shared animated brand.
use crate::{
    components::{brand::Brand, region::Regions},
    design::ZORK_UI,
};
use gpui::{prelude::*, *};
#[derive(Clone, PartialEq)]
pub struct Data {
    pub title: String,
    pub description: String,
}
pub struct Welcome {
    data: Data,
    brand: Entity<Brand>,
    width: f32,
    regions: Regions<Self>,
}
impl Welcome {
    pub fn new(data: Data, brand: Entity<Brand>) -> Self {
        Self {
            data,
            brand,
            width: 440.,
            regions: Default::default(),
        }
    }
    pub fn configure(
        &mut self,
        data: Data,
        brand: Entity<Brand>,
        width: f32,
        cx: &mut Context<Self>,
    ) {
        if self.data != data || self.brand != brand || self.width != width {
            self.data = data;
            self.brand = brand;
            self.width = width;
            crate::components::region::invalidate_all(cx);
        }
    }
}
impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let copy = self
            .regions
            .auto_height("home-copy", self.width, cx, |view, _, _| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .text_size(px(24.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(view.data.title.clone()),
                    )
                    .child(
                        div()
                            .text_center()
                            .text_size(px(13.))
                            .line_height(px(22.))
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child(view.data.description.clone()),
                    )
                    .into_any_element()
            });
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .px_8()
            .child(
                div()
                    .w_full()
                    .max_w(px(440.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_4()
                    .child(self.brand.clone())
                    .child(copy),
            )
    }
}
#[cfg(feature = "stories")]
pub fn story(state: &str, text: crate::resources::Text, cx: &mut App) -> Entity<Welcome> {
    let empty = state == "first-agent";
    let brand = cx.new(|_| {
        Brand::new(
            if empty {
                crate::components::brand::BrandMotion::Morph
            } else {
                crate::components::brand::BrandMotion::Icon
            },
            ZORK_UI.palette.canvas,
        )
    });
    cx.new(|_| {
        Welcome::new(
            Data {
                title: if empty {
                    "创建你的第一位 领队"
                } else {
                    "从一段对话开始"
                }
                .into(),
                description: if empty {
                    "在设备设置中添加模型连接，再创建 领队。".into()
                } else {
                    text.text("device_choose_leader")
                },
            },
            brand,
        )
    })
}
