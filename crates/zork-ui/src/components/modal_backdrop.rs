//! Dark compositor scrim with a foreground exclusion; no background filtering.
use gpui::{prelude::*, Bounds, Pixels};
use std::{cell::Cell, rc::Rc};

fn card_curves(bounds: Bounds<Pixels>) -> Vec<super::liquid::Cubic> {
    super::liquid::rounded_rectangle(
        super::liquid::Pose::rect(
            0.,
            0.,
            bounds.size.width.as_f32() as f64,
            bounds.size.height.as_f32() as f64,
            crate::controls::MODAL_RADIUS as f64,
        ),
        super::smooth::SMOOTHING,
    )
}

#[derive(gpui::IntoElement)]
pub struct ModalBackdrop {
    pub id: String,
    pub card: Rc<Cell<Bounds<Pixels>>>,
}
impl gpui::RenderOnce for ModalBackdrop {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        #[cfg(target_os = "macos")]
        {
            let layer = window.use_keyed_state(self.id, cx, |window, _| native::Layer::new(window));
            return gpui::canvas(
                move |_, _, cx| layer.update(cx, |layer, _| layer.update(self.card.get())),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
        }
        #[cfg(target_family = "wasm")]
        {
            let layer =
                window.use_keyed_state(self.id.clone(), cx, |_, _| web::Layer::new(self.id));
            return gpui::canvas(
                move |_, _, cx| layer.update(cx, |layer, _| layer.update(self.card.get())),
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
        }
        #[cfg(not(any(target_os = "macos", target_family = "wasm")))]
        {
            let _ = (self, window, cx);
            gpui::canvas(|_, _, _| {}, |_, _, _, _| {})
                .absolute()
                .inset_0()
        }
    }
}
#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ffi::c_void;
    unsafe extern "C" {
        fn zork_modal_blur_available(view: *mut c_void) -> i32;
        fn zork_modal_blur_create(view: *mut c_void) -> *mut c_void;
        fn zork_modal_blur_update(
            handle: *mut c_void,
            x: f64,
            y: f64,
            width: f64,
            height: f64,
            curves: *const f64,
            curve_count: usize,
        );
        fn zork_modal_blur_destroy(handle: *mut c_void);
    }
    pub fn available(window: &gpui::Window) -> bool {
        if std::env::var_os("ZORK_DISABLE_NATIVE_BLUR").is_some() {
            return false;
        }
        match HasWindowHandle::window_handle(window)
            .ok()
            .map(|h| h.as_raw())
        {
            Some(RawWindowHandle::AppKit(handle)) => unsafe {
                zork_modal_blur_available(handle.ns_view.as_ptr()) != 0
            },
            _ => false,
        }
    }
    pub struct Layer {
        handle: *mut c_void,
        bounds: Option<Bounds<Pixels>>,
    }
    impl Layer {
        pub fn new(window: &gpui::Window) -> Self {
            if std::env::var_os("ZORK_DISABLE_NATIVE_BLUR").is_some() {
                return Self {
                    handle: std::ptr::null_mut(),
                    bounds: None,
                };
            }
            let raw = HasWindowHandle::window_handle(window)
                .ok()
                .map(|h| h.as_raw());
            Self {
                handle: if let Some(RawWindowHandle::AppKit(handle)) = raw {
                    unsafe { zork_modal_blur_create(handle.ns_view.as_ptr()) }
                } else {
                    std::ptr::null_mut()
                },
                bounds: None,
            }
        }
        pub fn update(&mut self, b: Bounds<Pixels>) {
            if !self.handle.is_null() && b.size.width > gpui::px(0.) && self.bounds != Some(b) {
                self.bounds = Some(b);
                let curves: Vec<[f64; 8]> = card_curves(b)
                    .into_iter()
                    .map(|c| {
                        [
                            c.from[0], c.from[1], c.c1[0], c.c1[1], c.c2[0], c.c2[1], c.to[0],
                            c.to[1],
                        ]
                    })
                    .collect();
                unsafe {
                    zork_modal_blur_update(
                        self.handle,
                        b.origin.x.as_f32() as f64,
                        b.origin.y.as_f32() as f64,
                        b.size.width.as_f32() as f64,
                        b.size.height.as_f32() as f64,
                        curves.as_ptr().cast(),
                        curves.len(),
                    );
                }
            }
        }
    }
    impl Drop for Layer {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    zork_modal_blur_destroy(self.handle);
                }
            }
        }
    }
}

#[cfg(target_family = "wasm")]
mod web {
    use super::*;
    thread_local! { static NEXT: Cell<u64> = const { Cell::new(0) }; }
    pub struct Layer {
        id: String,
        bounds: Option<Bounds<Pixels>>,
    }
    fn send(value: serde_json::Value) {
        if let Ok(function) = js_sys::Reflect::get(&js_sys::global(), &"zorkModalBackdrop".into()) {
            if function.is_function() {
                let function: js_sys::Function = function.into();
                let _ = function.call1(&js_sys::global(), &value.to_string().into());
            }
        }
    }
    impl Layer {
        pub fn new(id: String) -> Self {
            let serial = NEXT.with(|n| {
                let next = n.get() + 1;
                n.set(next);
                next
            });
            Self {
                id: format!("{id}-{serial}"),
                bounds: None,
            }
        }
        pub fn update(&mut self, b: Bounds<Pixels>) {
            if self.bounds == Some(b) || b.size.width <= gpui::px(0.) {
                return;
            }
            self.bounds = Some(b);
            let curves = card_curves(b);
            let mut path = String::new();
            if let Some(first) = curves.first() {
                use std::fmt::Write;
                let _ = write!(path, "M{} {}", first.from[0], first.from[1]);
                for c in curves {
                    let _ = write!(
                        path,
                        "C{} {} {} {} {} {}",
                        c.c1[0], c.c1[1], c.c2[0], c.c2[1], c.to[0], c.to[1]
                    );
                }
                path.push('Z');
            }
            send(
                serde_json::json!({"id":self.id,"x":b.origin.x.as_f32(),"y":b.origin.y.as_f32(),"width":b.size.width.as_f32(),"height":b.size.height.as_f32(),"path":path}),
            );
        }
    }
    impl Drop for Layer {
        fn drop(&mut self) {
            send(serde_json::json!({"id":self.id,"remove":true}));
        }
    }
}

pub fn available(window: &gpui::Window) -> bool {
    #[cfg(target_os = "macos")]
    {
        return native::available(window);
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = window;
        return js_sys::Reflect::get(&js_sys::global(), &"zorkModalBackdrop".into())
            .is_ok_and(|f| f.is_function());
    }
    #[cfg(not(any(target_os = "macos", target_family = "wasm")))]
    {
        let _ = window;
        false
    }
}
