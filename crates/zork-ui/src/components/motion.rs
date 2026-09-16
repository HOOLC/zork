//! Short, interruptible motion shared by native and Web controls.
use gpui::{
    prelude::*, px, rgb, Animation, AnimationExt, ElementId, SpringAnimation, SpringConfig,
};
use std::time::Duration;

fn hover_spring_config() -> SpringConfig {
    SpringConfig::new(2500., 100., 1.)
}

pub fn spring(target: f32) -> SpringAnimation<f32> {
    // Critically damped: fast response without overshoot or a rubbery finish.
    SpringAnimation::new(hover_spring_config())
        .to(target)
        .with_epsilon(0.001)
}
pub fn enter<E: IntoElement + gpui::Styled + 'static>(
    element: E,
    id: impl Into<ElementId>,
    distance: f32,
) -> impl IntoElement {
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(180))
            .with_easing(|t| 1. - (1. - t).powi(3))
            .with_max_fps(60.),
        move |v, t| v.relative().top(px(distance * (1. - t))).opacity(t),
    )
}
#[derive(gpui::IntoElement)]
pub struct HoverFill {
    pub id: ElementId,
    pub color: u32,
    pub radius: f32,
    pub pressed: Option<(gpui::SharedString, u32)>,
}
impl gpui::RenderOnce for HoverFill {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let fill = super::smooth::fill("hover-contour", self.radius).current_color();
        self.render_with(fill, window, cx)
    }
}
impl HoverFill {
    pub(crate) fn render_with(
        self,
        fill: impl IntoElement,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let hovered = *state.read(cx);
        gpui::div()
            .id(self.id)
            .absolute()
            .inset_0()
            .text_color(rgb(self.color))
            .child(fill)
            .when_some(self.pressed, |v, (group, color)| {
                v.group_active(group, move |v| v.text_color(rgb(color)).opacity(1.))
            })
            .on_hover(move |hovered, _, cx| {
                state.update(cx, |v, cx| {
                    if *v != *hovered {
                        *v = *hovered;
                        cx.notify();
                    }
                })
            })
            .with_spring(
                "hover-fade",
                spring(if hovered { 1. } else { 0. }),
                |v, t| v.opacity(t),
            )
    }
}

pub fn enter_instrumented<E: gpui::Element + gpui::Styled + 'static>(
    element: crate::automation::element::AutomationElement<E>,
    id: impl Into<ElementId>,
    distance: f32,
) -> impl IntoElement {
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(180))
            .with_easing(|t| 1. - (1. - t).powi(3))
            .with_max_fps(60.),
        move |v, t| v.map_inner(|v| v.relative().top(px(distance * (1. - t))).opacity(t)),
    )
}

pub fn mix_rgb(from: u32, to: u32, progress: f32) -> gpui::Rgba {
    let t = progress.clamp(0., 1.);
    let value = [16, 8, 0].into_iter().fold(0, |value, shift| {
        let a = ((from >> shift) & 255) as f32;
        let b = ((to >> shift) & 255) as f32;
        value | (((a + (b - a) * t).round() as u32) << shift)
    });
    rgb(value)
}
