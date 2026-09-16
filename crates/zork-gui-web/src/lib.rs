#![cfg(target_family = "wasm")]
mod api;
#[path = "../../zork-gui/src/i18n.rs"]
mod i18n;
pub use zork_ui::assets;
mod automation;
pub use zork_ui::comments;
pub use zork_ui::components;
pub use zork_ui::design;
mod desktop;
use desktop::stories::{self, Story, StoryHost};
use gpui::{div, prelude::*, AppContext, Context, Entity, Window};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use wasm_bindgen::prelude::*;
struct Host {
    inner: gpui::AnyView,
    story: Story,
    pending: std::collections::VecDeque<serde_json::Value>,
    action_error: Option<String>,
    in_flight: bool,
    generation: u64,
    driver: automation::Automation,
}
impl Host {
    fn choose_family(&mut self, family: String, cx: &mut Context<Self>) {
        self.inner = cx
            .new(|cx| zork_ui::stories::FamilyStories::new(family.clone(), cx))
            .into();
        self.story.id = format!("family-{family}");
        self.pending.clear();
        self.action_error = None;
        self.in_flight = false;
        self.generation += 1;
        cx.notify();
    }
    fn choose(&mut self, story: Story, cx: &mut Context<Self>) {
        self.inner = cx.new(|cx| StoryHost::new(story.clone(), cx)).into();
        self.story = story;
        self.pending = self.story.actions.clone().into();
        self.action_error = None;
        self.in_flight = false;
        self.generation += 1;
        cx.notify();
    }
}
impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(action) = if self.in_flight {
            None
        } else {
            self.pending.pop_front()
        } {
            self.in_flight = true;
            let generation = self.generation;
            let driver = self.driver.clone();
            let weak = cx.entity().downgrade();
            window.on_next_frame(move |w, cx| {
                if weak
                    .update(cx, |host, _| host.generation == generation)
                    .ok()
                    != Some(true)
                {
                    return;
                }
                let result = serde_json::from_value(action)
                    .map_err(|error| error.to_string())
                    .and_then(|action| {
                        driver
                            .dispatch(action, w, cx)
                            .map_err(|error| error.to_string())
                    });
                let _ = weak.update(cx, |host, cx| {
                    host.in_flight = false;
                    if let Err(error) = result {
                        host.action_error = Some(error);
                        host.pending.clear();
                    }
                    cx.notify();
                });
            });
        }
        div().size_full().child(self.inner.clone())
    }
}
struct Runtime {
    host: Entity<Host>,
    cx: gpui::AsyncApp,
    window: gpui::AnyWindowHandle,
    driver: automation::Automation,
}
thread_local! {static APPLICATION:RefCell<Option<gpui::ApplicationHandle>>=const{RefCell::new(None)};}
thread_local! {static RUNTIME:RefCell<Option<Runtime>>=const{RefCell::new(None)};}
thread_local! {static REDUCE_MOTION:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
#[wasm_bindgen]
pub fn set_reduce_motion(value: bool) {
    REDUCE_MOTION.with(|v| v.set(value));
    RUNTIME.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.host
                .update(&mut r.cx, |_, cx| cx.set_reduce_motion(value));
        }
    });
}
#[wasm_bindgen]
pub fn catalog() -> String {
    serde_json::to_string(&stories::catalog()).unwrap()
}
#[wasm_bindgen]
pub fn snapshot() -> String {
    RUNTIME.with(|r| {
        r.borrow()
            .as_ref()
            .map(|r| serde_json::to_string(&r.driver.snapshot()).unwrap())
            .unwrap_or("null".into())
    })
}
#[wasm_bindgen]
pub fn story_state() -> String {
    RUNTIME.with(|r| {
        let r = r.borrow();
        let Some(r) = r.as_ref() else {
            return "null".into();
        };
        r.host.read_with(&r.cx, |h, cx| {
            let mut value = if let Ok(view) = h.inner.clone().downcast::<StoryHost>() {
                view.read(cx).inspect(cx)
            } else if let Ok(view) = h
                .inner
                .clone()
                .downcast::<zork_ui::stories::FamilyStories>()
            {
                view.read(cx).inspect(cx)
            } else {
                serde_json::json!({})
            };
            value["id"] = h.story.id.clone().into();
            value["pending_actions"] = (h.pending.len() + usize::from(h.in_flight)).into();
            value["action_error"] = h.action_error.clone().into();
            serde_json::to_string(&value).unwrap()
        })
    })
}
#[wasm_bindgen]
pub fn select_story(id: String) -> Result<(), JsValue> {
    let story = stories::catalog()
        .into_iter()
        .find(|s| s.id == id)
        .ok_or_else(|| JsValue::from_str("unknown story"))?;
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        let r = runtime
            .as_mut()
            .ok_or_else(|| JsValue::from_str("not ready"))?;
        r.host.update(&mut r.cx, |h, cx| h.choose(story, cx));
        Ok(())
    })
}
#[wasm_bindgen]
pub fn select_family(family: String) -> Result<(), JsValue> {
    if !zork_ui::stories::catalog()
        .iter()
        .any(|s| s.family == family)
    {
        return Err(JsValue::from_str("unknown component family"));
    }
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        let r = runtime
            .as_mut()
            .ok_or_else(|| JsValue::from_str("not ready"))?;
        r.host
            .update(&mut r.cx, |h, cx| h.choose_family(family, cx));
        Ok(())
    })
}
#[wasm_bindgen]
pub fn action(action: String) -> Result<(), JsValue> {
    let action = serde_json::from_str(&action).map_err(|e| JsValue::from_str(&e.to_string()))?;
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.borrow_mut();
        let r = runtime
            .as_mut()
            .ok_or_else(|| JsValue::from_str("not ready"))?;
        let driver = r.driver.clone();
        r.cx.update_window(r.window, |_, w, cx| driver.dispatch(action, w, cx))
            .map_err(|e| JsValue::from_str(&e.to_string()))?
            .map_err(|e| JsValue::from_str(&e.to_string()))
    })
}
#[wasm_bindgen]
pub fn start(id: String) {
    console_error_panic_hook::set_once();
    gpui_web::init_logging();
    let query = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    // A development build of the platform defaults to Debug. Font fallback
    // emits per-word diagnostics there, which stalls normal CJK interactions.
    // Keep useful diagnostics and make the verbose mode an explicit choice.
    log::set_max_level(
        if query
            .trim_start_matches('?')
            .split('&')
            .any(|p| p == "log=debug")
        {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        },
    );
    let story = stories::catalog()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| stories::catalog()[0].clone());
    let force_webgl = query.contains("backend=webgl");
    let platform = Rc::new(gpui_web::WebPlatform::new_with_backend(
        false,
        if force_webgl {
            gpui_web::WebBackendPreference::WebGl
        } else {
            gpui_web::WebBackendPreference::Auto
        },
    ));
    let http = Arc::new(platform.fetch_http_client());
    let application = gpui::Application::with_platform(platform)
        .with_http_client(http)
        .with_assets(assets::EmbeddedAssets)
        .run_embedded(move |cx| {
            api::initialize(cx);
            cx.set_reduce_motion(REDUCE_MOTION.with(|v| v.get()));
            // fontdb indexes the default instance of a variable font. Explicit
            // weights avoid accidentally rendering all CJK text at Thin (100).
            zork_ui::assets::init_fonts(cx);
            cx.text_system()
                .add_fonts(vec![
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../assets/static/NotoSansSC-400.ttf"
                    )),
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../assets/static/NotoSansSC-500.ttf"
                    )),
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../assets/static/NotoSansSC-600.ttf"
                    )),
                    std::borrow::Cow::Borrowed(include_bytes!(
                        "../assets/static/NotoSansSC-700.ttf"
                    )),
                ])
                .expect("Web story fonts");
            components::init(cx);
            let driver = automation::Automation::install(cx);
            let mut host = None;
            let window = cx
                .open_window(gpui::WindowOptions::default(), |window, cx| {
                    let inner = cx.new(|cx| StoryHost::new(story.clone(), cx));
                    let view = cx.new(|_| Host {
                        inner: inner.into(),
                        pending: story.actions.clone().into(),
                        story,
                        action_error: None,
                        in_flight: false,
                        generation: 0,
                        driver: driver.clone(),
                    });
                    host = Some(view.clone());
                    cx.new(|_| automation::AutomationRoot::new(view))
                })
                .expect("Web GPUI window");
            RUNTIME.with(|r| {
                *r.borrow_mut() = Some(Runtime {
                    host: host.unwrap(),
                    cx: cx.to_async(),
                    window: window.into(),
                    driver,
                })
            });
        });
    APPLICATION.with(|a| *a.borrow_mut() = Some(application));
}
