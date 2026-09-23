//! Input-driven pressure using the shared material solver, with stable layout.
use super::{Material, Options, Pose, Simulation, Surface, SurfaceColors};
use gpui::{prelude::*, *};
use std::time::Instant;

struct PlaybackRate(f64);
impl Global for PlaybackRate {}
pub(crate) fn playback_rate(cx: &App) -> f64 {
    cx.try_global::<PlaybackRate>().map_or(1., |rate| rate.0)
}
/// Presentation-only slow playback, shared with the gallery's main surfaces.
pub fn set_playback_rate(rate: f64, cx: &mut App) {
    assert!(rate.is_finite() && rate > 0.);
    if cx
        .try_global::<PlaybackRate>()
        .is_none_or(|old| old.0 != rate)
    {
        cx.set_global(PlaybackRate(rate));
    }
}

struct Pressure {
    surface: Option<Surface>,
    rest_surface: Option<super::render::StaticSurface>,
    rest: Pose,
    smoothing: f64,
    bounds: Bounds<Pixels>,
    mouse: bool,
    inside: bool,
    keyboard: bool,
    press: zork_liquid::motion::Press,
    last: Option<Instant>,
    scheduled: bool,
}
impl Pressure {
    fn new(rest: Pose, smoothing: f64) -> Self {
        Self {
            surface: None,
            rest_surface: None,
            rest,
            smoothing,
            bounds: Bounds::default(),
            mouse: false,
            inside: false,
            keyboard: false,
            press: Default::default(),
            last: None,
            scheduled: false,
        }
    }
    fn target(&mut self, down: bool, point: Option<[f64; 2]>, tap: bool) {
        if self.press.down == down {
            return;
        }
        let rest = self.rest;
        let material = Material {
            smoothing: self.smoothing,
            ..Material::ordinary()
        };
        let surface = self.surface.get_or_insert_with(|| {
            let mut simulation = Simulation::new(rest, material, Options::default());
            simulation.finish();
            Surface::new(simulation).expect("valid button material")
        });
        self.press
            .set(&mut surface.simulation, rest, down, point, tap);
        self.last.get_or_insert_with(Instant::now);
    }
    fn advance(
        &mut self,
        rest: Pose,
        enabled: bool,
        focused: bool,
        reduced: bool,
        visible: bool,
        inside: bool,
        rate: f64,
    ) -> bool {
        if !visible {
            self.mouse = false;
            self.keyboard = false;
            self.press.reset();
            self.last = None;
            if let Some(surface) = &mut self.surface {
                if surface.simulation.moving() {
                    surface.simulation.set_target(rest);
                    surface.simulation.finish();
                }
            }
            return false;
        }
        if self.mouse && self.inside != inside {
            self.inside = inside;
            self.target(inside || self.keyboard, None, false);
        }
        if self.rest != rest {
            self.rest = rest;
            self.mouse = false;
            self.keyboard = false;
            self.press.reset();
            if let Some(surface) = &mut self.surface {
                surface.simulation.set_target(rest);
                surface.simulation.finish();
                surface.prepare();
            }
            self.last = None;
        }
        if !enabled {
            self.mouse = false;
            self.keyboard = false;
            // Disabling prevents new presses; an accepted press still releases
            // continuously, including the send button after its draft clears.
            self.target(false, None, true);
        }
        if self.keyboard && !focused {
            self.keyboard = false;
            self.target(false, None, false);
        }
        let Some(surface) = &mut self.surface else {
            return false;
        };
        let now = Instant::now();
        let elapsed = self
            .last
            .replace(now)
            .map_or(0., |t| now.duration_since(t).as_secs_f64())
            * rate;
        let moving = self
            .press
            .advance(&mut surface.simulation, rest, elapsed, reduced);
        surface.prepare();
        if !moving {
            self.last = None;
            if !self.press.down && !self.mouse && !self.keyboard {
                self.surface = None;
            }
        }
        moving
    }
}

/// Geometry deforms inside a stable input/layout slot. Pointer capture begins
/// on the painted contour; an already captured press can finish after squeezing.
pub fn surface(
    id: impl Into<ElementId>,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f64,
    colors: SurfaceColors,
    content: impl IntoElement,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let content = content.into_any_element();
    content_surface(
        id.into(),
        width,
        height,
        radius,
        smoothing,
        colors,
        false,
        move |_, _, _, _| content,
        enabled,
        window,
        cx,
    )
}

pub(super) fn content_surface(
    id: ElementId,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f64,
    colors: SurfaceColors,
    clear: bool,
    content: impl FnOnce(Option<super::ContentClip>, bool, &mut Window, &mut App) -> AnyElement,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    content_surface_with_source(
        id, width, height, radius, smoothing, colors, clear, content, enabled, None, false, window,
        cx,
    )
}

pub(super) fn content_surface_with_source(
    id: ElementId,
    width: f32,
    height: f32,
    radius: f32,
    smoothing: f64,
    colors: SurfaceColors,
    clear: bool,
    content: impl FnOnce(Option<super::ContentClip>, bool, &mut Window, &mut App) -> AnyElement,
    enabled: bool,
    source: Option<super::render::SourceMaterial>,
    keeps_input_slot: bool,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let source_binding = source;
    let source = source_binding.as_ref().and_then(|source| source.resolve());
    let rest = Pose::rect(0., 0., width as f64, height as f64, radius as f64);
    let state = window.use_keyed_state(format!("liquid-pressure-{id:?}"), cx, |_, _| {
        Pressure::new(rest, smoothing)
    });
    let reduced = cx.reduce_motion();
    let rate = cx.try_global::<PlaybackRate>().map_or(1., |rate| rate.0);
    let bounds = state.read(cx).bounds;
    let clip = bounds.intersect(&Bounds::new(point(px(0.), px(0.)), window.viewport_size()));
    let visible = clip.size.width > px(0.) && clip.size.height > px(0.);
    let inside = bounds.contains(&window.mouse_position());
    let moving = state.update(cx, |v, _| {
        v.advance(
            rest,
            enabled,
            colors.focused,
            reduced,
            visible,
            inside,
            rate,
        )
    });
    if moving && !state.read(cx).scheduled {
        state.update(cx, |v, _| v.scheduled = true);
        let weak = state.downgrade();
        window.on_next_frame(move |_, cx| {
            let _ = weak.update(cx, |v, cx| {
                v.scheduled = false;
                cx.notify();
            });
        });
    }
    state.update(cx, |v, _| {
        if v.rest_surface
            .as_ref()
            .is_none_or(|s| !s.matches(rest, smoothing))
        {
            v.rest_surface = Some(super::render::StaticSurface::new(rest, smoothing));
        }
    });
    let clip = source.as_ref().map_or_else(
        || {
            let v = state.read(cx);
            v.surface.as_ref().map_or_else(
                || v.rest_surface.as_ref().unwrap().content_clip(),
                Surface::content_clip,
            )
        },
        |source| source.part.content_clip(),
    );
    let pressed = state.read(cx).press.down || state.read(cx).press.release_pending;
    let content = content(Some(clip), pressed, window, cx);
    let content = if let Some(binding) = &source_binding {
        binding.capture_content(content)
    } else {
        content
    };
    let motion = state.read(cx);
    let fill = (!clear).then_some(colors.fill);
    let pose = source.as_ref().map_or_else(
        || {
            motion
                .surface
                .as_ref()
                .map_or(rest, |s| s.simulation.pose())
        },
        |source| {
            Pose::rect(
                source.pose.left() - source.rest.left(),
                source.pose.top() - source.rest.top(),
                source.pose.w,
                source.pose.h,
                source.pose.r,
            )
        },
    );
    let (background, outline) = if let Some(source) = &source {
        let offset = point(
            px(-source.rest.left() as f32),
            px(-source.rest.top() as f32),
        );
        (
            source
                .part
                .background(Some(colors.fill), None, offset, false),
            source
                .part
                .background(None, colors.border, offset, colors.focused),
        )
    } else if let Some(surface) = &motion.surface {
        (
            surface
                .background_colors(fill, None, point(px(0.), px(0.)), false)
                .into_any_element(),
            surface
                .background_colors(None, colors.border, point(px(0.), px(0.)), colors.focused)
                .into_any_element(),
        )
    } else {
        let shape = motion.rest_surface.as_ref().unwrap();
        (
            shape.background(fill, None, false).into_any_element(),
            shape
                .background(None, colors.border, colors.focused)
                .into_any_element(),
        )
    };
    // The material may be supplied by a paired overlay, but the original
    // control still owns its input contour at its current page position.
    let input_placement = source.is_some().then(|| {
        if let Some(surface) = &motion.surface {
            surface
                .background_colors(None, None, point(px(0.), px(0.)), false)
                .into_any_element()
        } else {
            motion
                .rest_surface
                .as_ref()
                .unwrap()
                .background(None, None, false)
                .into_any_element()
        }
    });
    let control = div()
        .id(id)
        .relative()
        .w(px(width))
        .h(px(height))
        .when_some(input_placement, |control, placement| {
            control.child(placement)
        })
        .child(background)
        .child(
            div()
                .absolute()
                .left(px(if keeps_input_slot {
                    0.
                } else {
                    (pose.cx - rest.cx) as f32
                }))
                .top(px(if keeps_input_slot {
                    0.
                } else {
                    (pose.cy - rest.cy) as f32
                }))
                .w(px(width))
                .h(px(height))
                .child(content),
        );
    let control = control.child(outline);
    let control = if let Some(surface) = &motion.surface {
        surface.guard(control)
    } else {
        motion.rest_surface.as_ref().unwrap().guard(control)
    };
    let down = state.clone();
    let up = state.clone();
    let outside = state.clone();
    let key_down = state.clone();
    let key_up = state.clone();
    control
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    state.update(cx, |v, _| v.bounds = bounds);
                    // Observe a captured drag without consuming the caller's
                    // single on_hover hook (tooltips use that hook themselves).
                    if state.read(cx).mouse {
                        let weak = state.downgrade();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = weak.update(cx, |v, cx| {
                                    let inside = v.bounds.contains(&event.position);
                                    if v.mouse && event.pressed_button != Some(MouseButton::Left) {
                                        v.mouse = false;
                                        v.target(v.keyboard, None, false);
                                        cx.notify();
                                    } else if v.mouse && v.inside != inside {
                                        v.inside = inside;
                                        v.target(inside || v.keyboard, None, false);
                                        cx.notify();
                                    }
                                });
                            }
                        });
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
        .capture_any_mouse_down(move |event, _, cx| {
            if enabled && event.button == MouseButton::Left {
                down.update(cx, |v, cx| {
                    v.mouse = true;
                    v.inside = true;
                    let p = [
                        (event.position.x - v.bounds.origin.x).as_f32() as f64 / width as f64,
                        (event.position.y - v.bounds.origin.y).as_f32() as f64 / height as f64,
                    ];
                    v.target(true, Some(p.map(|x| x.clamp(0., 1.))), false);
                    cx.notify();
                });
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            up.update(cx, |v, cx| {
                if v.mouse {
                    v.mouse = false;
                    v.target(v.keyboard, None, true);
                    cx.notify();
                }
            });
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            outside.update(cx, |v, cx| {
                if v.mouse {
                    v.mouse = false;
                    v.target(v.keyboard, None, false);
                    cx.notify();
                }
            });
        })
        .on_key_down(move |event, _, cx| {
            if enabled
                && !event.keystroke.modifiers.modified()
                && matches!(event.keystroke.key.as_str(), "enter" | "space")
            {
                key_down.update(cx, |v, cx| {
                    v.keyboard = true;
                    v.target(true, None, false);
                    cx.notify();
                });
            } else if event.keystroke.key == "escape" {
                key_down.update(cx, |v, cx| {
                    v.keyboard = false;
                    v.mouse = false;
                    v.target(false, None, false);
                    cx.notify();
                });
            }
        })
        .on_key_up(move |event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                key_up.update(cx, |v, cx| {
                    v.keyboard = false;
                    v.target(v.mouse && v.inside, None, true);
                    cx.notify();
                });
            }
        })
}
