use crate::output;
use cef::*;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc as LocalRc,
    sync::Arc,
};

#[derive(Clone)]
struct Surface {
    bounds: LocalRc<RefCell<Rect>>,
    visible: LocalRc<Cell<bool>>,
    scale: LocalRc<Cell<f32>>,
    base: LocalRc<RefCell<Option<(i32, i32, Arc<Vec<u8>>)>>>,
    popup: LocalRc<RefCell<Option<Rect>>>,
}
impl Default for Surface {
    fn default() -> Self {
        Self {
            bounds: LocalRc::new(RefCell::new(Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            })),
            visible: LocalRc::new(Cell::new(false)),
            scale: LocalRc::new(Cell::new(1.)),
            base: Default::default(),
            popup: Default::default(),
        }
    }
}
struct Tab {
    browser: Browser,
    surface: Surface,
    _observer: Option<Registration>,
}
#[derive(Default)]
struct State {
    tabs: HashMap<String, Tab>,
    closing: bool,
    downloads: std::path::PathBuf,
    reserved_downloads: std::collections::HashSet<std::path::PathBuf>,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
pub fn set_profile(profile: &std::path::Path) {
    STATE.with(|s| s.borrow_mut().downloads = profile.join("downloads"));
}
pub fn clear() {
    STATE.with(|s| s.borrow_mut().tabs.clear());
}
fn target_id(browser: &Browser) -> String {
    static GENERATION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let generation = GENERATION.get_or_init(|| {
        format!(
            "{:x}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    });
    format!("{generation}-{}", browser.identifier())
}
fn get(id: &str) -> anyhow::Result<(Browser, Surface)> {
    STATE
        .with(|s| {
            s.borrow()
                .tabs
                .get(id)
                .map(|t| (t.browser.clone(), t.surface.clone()))
        })
        .ok_or_else(|| anyhow::anyhow!("browser page is closed"))
}
fn reply(request: &Value, result: anyhow::Result<Value>) {
    if let Some(id) = request.get("id") {
        output::control(match result {
            Ok(value) => json!({"id":id,"result":value}),
            Err(error) => json!({"id":id,"error":{"message":error.to_string()}}),
        });
    }
}
fn notify(method: &str, browser: &Browser, params: Value) {
    output::control(json!({"method":method,"sessionId":target_id(&browser),"params":params}));
}
fn close_all() {
    let browsers = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.closing = true;
        s.tabs
            .values()
            .map(|t| t.browser.clone())
            .collect::<Vec<_>>()
    });
    if browsers.is_empty() {
        quit_message_loop();
    }
    for browser in browsers {
        if let Some(host) = browser.host() {
            host.close_browser(1);
        }
    }
}
fn flush_and_close() {
    if let Some(manager) = cookie_manager_get_global_manager(None) {
        let mut completion = CookiesFlushed::new();
        if manager.flush_store(Some(&mut completion)) == 1 {
            return;
        }
    }
    close_all();
}
wrap_completion_callback! {
    struct CookiesFlushed;
    impl CompletionCallback { fn on_complete(&self) { close_all(); } }
}
pub fn post(request: Value) {
    let mut task = CommandTask::new(request);
    post_task(ThreadId::UI, Some(&mut task));
}
wrap_task! {
    struct CommandTask { request: Value }
    impl Task { fn execute(&self) { command(&self.request); } }
}
fn command(request: &Value) {
    let method = request["method"].as_str().unwrap_or("");
    let params = &request["params"];
    let result = (|| -> anyhow::Result<Option<Value>> {
        match method {
            "Browser.close" => {
                reply(request, Ok(json!({})));
                flush_and_close();
                return Ok(None);
            }
            "Target.setDiscoverTargets" => return Ok(Some(json!({}))),
            "Target.createTarget" => {
                let window = WindowInfo {
                    windowless_rendering_enabled: 1,
                    ..Default::default()
                };
                let settings = BrowserSettings {
                    windowless_frame_rate: 60,
                    ..Default::default()
                };
                let mut client = RuntimeClient::new(Surface::default(), None);
                let browser = browser_host_create_browser_sync(
                    Some(&window),
                    Some(&mut client),
                    Some(&params["url"].as_str().unwrap_or("about:blank").into()),
                    Some(&settings),
                    None,
                    None,
                )
                .ok_or_else(|| anyhow::anyhow!("CEF could not create a page"))?;
                return Ok(Some(json!({"targetId":target_id(&browser)})));
            }
            "Target.attachToTarget" => {
                let id = params["targetId"].as_str().unwrap_or("");
                get(id)?;
                return Ok(Some(json!({"sessionId":id})));
            }
            "Target.closeTarget" => {
                let (browser, _) = get(params["targetId"].as_str().unwrap_or(""))?;
                browser.host().unwrap().close_browser(1);
                return Ok(Some(json!({"success":true})));
            }
            "Target.activateTarget" => {
                let (browser, _) = get(params["targetId"].as_str().unwrap_or(""))?;
                browser.host().unwrap().set_focus(1);
                return Ok(Some(json!({})));
            }
            "Zork.viewport" => {
                let (browser, surface) = get(request["sessionId"].as_str().unwrap_or(""))?;
                let visible = params["visible"].as_bool().unwrap_or(false);
                if visible {
                    let width = params["width"].as_i64().unwrap_or(0);
                    let height = params["height"].as_i64().unwrap_or(0);
                    anyhow::ensure!(
                        (200..=4096).contains(&width) && (100..=4096).contains(&height),
                        "invalid page dimensions"
                    );
                    surface.bounds.replace(Rect {
                        x: 0,
                        y: 0,
                        width: width as i32,
                        height: height as i32,
                    });
                }
                surface
                    .scale
                    .set(params["scale"].as_f64().unwrap_or(1.).clamp(1., 2.) as f32);
                surface.visible.set(visible);
                let host = browser.host().unwrap();
                host.was_hidden((!visible) as i32);
                if visible {
                    host.notify_screen_info_changed();
                    host.was_resized();
                    host.invalidate(PaintElementType::VIEW);
                }
                return Ok(Some(json!({})));
            }
            _ => {}
        }
        let (browser, _) = get(request["sessionId"].as_str().unwrap_or(""))?;
        let message = json!({"id":request["id"],"method":method,"params":params});
        let bytes = serde_json::to_vec(&message)?;
        anyhow::ensure!(
            browser.host().unwrap().send_dev_tools_message(Some(&bytes)) == 1,
            "CEF rejected browser command"
        );
        Ok(None)
    })();
    match result {
        Ok(Some(value)) => reply(request, Ok(value)),
        Err(error) => reply(request, Err(error)),
        _ => {}
    }
}
wrap_app! {
    pub struct RuntimeApp;
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(RuntimeProcess::new()) }
        fn on_before_command_line_processing(&self, _process_type: Option<&CefString>, command_line: Option<&mut CommandLine>) {
            if let Some(line)=command_line {
                line.append_switch(Some(&"no-startup-window".into()));
                #[cfg(target_os = "macos")]
                super::mac::configure_runtime(line);
            }
        }
    }
}
wrap_browser_process_handler! {
    struct RuntimeProcess;
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) { output::control(json!({"method":"Zork.ready"})); }
        fn on_before_child_process_launch(&self, command_line: Option<&mut CommandLine>) {
            #[cfg(target_os = "macos")]
            if let Some(command_line) = command_line { super::mac::select_helper(command_line); }
            #[cfg(not(target_os = "macos"))]
            let _ = command_line;
        }
    }
}
wrap_client! {
    struct RuntimeClient { surface: Surface, opener: Option<String> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Lifecycle::new(self.surface.clone(), self.opener.clone())) }
        fn render_handler(&self) -> Option<RenderHandler> { Some(Painter::new(self.surface.clone())) }
        fn display_handler(&self) -> Option<DisplayHandler> { Some(Display::new()) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(Loading::new()) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(Downloads::new()) }
    }
}
wrap_life_span_handler! {
    struct Lifecycle { surface: Surface, opener: Option<String> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser)=browser else {return};
            let id=target_id(&browser);
            let mut observer=DevTools::new();
            let registration=browser.host().unwrap().add_dev_tools_message_observer(Some(&mut observer));
            // New tabs stay hidden until the native panel supplies a visible
            // viewport; background pages must not render unconsumed animations.
            browser.host().unwrap().was_hidden(1);
            STATE.with(|s| s.borrow_mut().tabs.insert(id.clone(), Tab {browser:browser.clone(),surface:self.surface.clone(),_observer:registration}));
            if let Some(opener)=&self.opener {
                output::control(json!({"method":"Target.targetCreated","params":{"targetInfo":{"targetId":id,"openerId":opener,"type":"page","url":browser.main_frame().map(|f|CefString::from(&f.url()).to_string()).unwrap_or_default()}}}));
            }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let Some(browser)=browser else {return};
            let id=target_id(&browser);
            let (removed,finish)=STATE.with(|s| {let mut s=s.borrow_mut();let removed=s.tabs.remove(&id); (removed,s.closing&&s.tabs.is_empty())});
            drop(removed);
            output::control(json!({"method":"Target.targetDestroyed","params":{"targetId":id}}));
            if finish {quit_message_loop();}
        }
        fn on_before_popup(&self, browser: Option<&mut Browser>, _frame: Option<&mut Frame>, _popup_id:i32,
            target_url:Option<&CefString>, _target_frame_name:Option<&CefString>, _target_disposition:WindowOpenDisposition,
            _user_gesture:i32, _popup_features:Option<&PopupFeatures>, window_info:Option<&mut WindowInfo>,
            client:Option<&mut Option<Client>>, _settings:Option<&mut BrowserSettings>, _extra_info:Option<&mut Option<DictionaryValue>>,
            _no_javascript_access:Option<&mut i32>) -> i32 {
            let url=target_url.map(|u|u.to_string()).unwrap_or_default();
            if !(url.is_empty()||url=="about:blank"||url.starts_with("https://")||url.starts_with("http://")) {return 1;}
            if let Some(window)=window_info {window.windowless_rendering_enabled=1;}
            if let Some(client)=client {*client=Some(RuntimeClient::new(Surface::default(),browser.map(|b|target_id(&b))));}
            0
        }
    }
}
wrap_dev_tools_message_observer! {
    struct DevTools;
    impl DevToolsMessageObserver {
        fn on_dev_tools_message(&self, browser:Option<&mut Browser>, message:Option<&[u8]>) -> i32 {
            let (Some(browser),Some(message))=(browser,message) else{return 0};
            if let Ok(mut value)=serde_json::from_slice::<Value>(message) {
                value["sessionId"]=json!(target_id(&browser));
                output::control(value);
            }
            1
        }
    }
}
wrap_display_handler! {
    struct Display;
    impl DisplayHandler {
        fn on_address_change(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>,url:Option<&CefString>) {
            if let (Some(browser),Some(frame),Some(url))=(browser,frame,url) {if frame.is_main()==1 {
                notify("Target.targetInfoChanged",browser,json!({"targetInfo":{"targetId":target_id(&browser),"url":url.to_string()}}));
            }}
        }
        fn on_title_change(&self,browser:Option<&mut Browser>,title:Option<&CefString>) {
            if let(Some(browser),Some(title))=(browser,title) {notify("Target.targetInfoChanged",browser,json!({"targetInfo":{"targetId":target_id(&browser),"title":title.to_string()}}));}
        }
    }
}
wrap_load_handler! {
    struct Loading;
    impl LoadHandler {
        fn on_loading_state_change(&self,browser:Option<&mut Browser>,is_loading:i32,_can_go_back:i32,_can_go_forward:i32) {
            if let Some(browser)=browser {notify(if is_loading==1 {"Page.frameStartedLoading"}else{"Page.frameStoppedLoading"},browser,json!({}));}
        }
    }
}
wrap_render_handler! {
    struct Painter { surface: Surface }
    impl RenderHandler {
        fn view_rect(&self,_browser:Option<&mut Browser>,rect:Option<&mut Rect>) {if let Some(rect)=rect {*rect=self.surface.bounds.borrow().clone();}}
        fn screen_info(&self,_browser:Option<&mut Browser>,info:Option<&mut ScreenInfo>)->i32 {
            if let Some(info)=info {info.device_scale_factor=self.surface.scale.get();1}else{0}
        }
        fn on_popup_size(&self,_browser:Option<&mut Browser>,rect:Option<&Rect>) {*self.surface.popup.borrow_mut() = rect.cloned();}
        fn on_popup_show(&self,browser:Option<&mut Browser>,show:i32) {
            if show==0 {*self.surface.popup.borrow_mut() = None;if let (Some(browser),Some((w,h,pixels)))=(browser,self.surface.base.borrow().as_ref()) {output::paint(target_id(&browser),*w,*h,self.surface.scale.get(),pixels.clone());}}
        }
        fn on_paint(&self,browser:Option<&mut Browser>,type_:PaintElementType,_dirty_rects:Option<&[Rect]>,buffer:*const u8,width:i32,height:i32) {
            let Some(browser)=browser else{return};
            if !self.surface.visible.get()||buffer.is_null()||width<=0||height<=0||width>8192||height>8192 {return;}
            let pixels=unsafe {std::slice::from_raw_parts(buffer,width as usize*height as usize*4)};
            if type_==PaintElementType::VIEW {
                let rect=self.surface.bounds.borrow();let scale=self.surface.scale.get();
                if width!=(rect.width as f32*scale).round() as i32||height!=(rect.height as f32*scale).round() as i32{return;}
                let pixels=Arc::new(pixels.to_vec());
                *self.surface.base.borrow_mut()=Some((width,height,pixels.clone()));
                output::paint(target_id(&browser),width,height,self.surface.scale.get(),pixels);
            } else if let (Some(rect),Some((w,h,base)))=(self.surface.popup.borrow().clone(),self.surface.base.borrow().as_ref()) {
                let mut composed=base.as_ref().clone();
                for y in 0..height {for x in 0..width {
                    let scale=self.surface.scale.get();let (dx,dy)=((rect.x as f32*scale).round() as i32+x,(rect.y as f32*scale).round() as i32+y);
                    if dx>=0&&dy>=0&&dx<*w&&dy<*h {let to=((dy**w+dx)*4) as usize;let from=((y*width+x)*4) as usize;composed[to..to+4].copy_from_slice(&pixels[from..from+4]);}
                }}
                output::paint(target_id(&browser),*w,*h,self.surface.scale.get(),Arc::new(composed));
            }
        }
    }
}

wrap_download_handler! {
    struct Downloads;
    impl DownloadHandler {
        fn can_download(&self,_browser:Option<&mut Browser>,_url:Option<&CefString>,_request_method:Option<&CefString>) -> i32 {1}
        fn on_before_download(&self,_browser:Option<&mut Browser>,_item:Option<&mut DownloadItem>,suggested_name:Option<&CefString>,callback:Option<&mut BeforeDownloadCallback>) -> i32 {
            let Some(callback)=callback else{return 0};
            let name=suggested_name.map(|s|s.to_string()).unwrap_or_else(||"download".into());
            let file=std::path::Path::new(&name).file_name().and_then(|n|n.to_str()).filter(|n|!n.is_empty()).unwrap_or("download");
            let path=STATE.with(|state| {
                let mut state=state.borrow_mut();
                let mut path=state.downloads.join(file);
                let mut suffix=1;
                while path.exists()||state.reserved_downloads.contains(&path) {path=state.downloads.join(format!("{suffix}-{file}"));suffix+=1;}
                state.reserved_downloads.insert(path.clone());path
            });
            if std::fs::create_dir_all(path.parent().unwrap()).is_err(){return 0;}
            callback.cont(Some(&path.to_string_lossy().as_ref().into()),0);
            1
        }
    }
}
