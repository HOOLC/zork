//! The model editor end to end through the real ProfilesView: add dialog,
//! inline edit, suggestions, presets, provenance, thinking editors, token
//! fields, fill sources, protocol and fetch, driven by pointer and keyboard.
use anyhow::{ensure, Context as _};
use gpui::{
    div, prelude::*, px, rgb, AppContext, Context, Entity, HeadlessAppContext, Render, Window,
    WindowHandle,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use zork_gui::{
    assets::EmbeddedAssets,
    automation::{protocol::UserAction, AutomationRoot, HeadlessAutomation},
    desktop::HeadlessProfilesView,
};

struct Frame {
    inner: Entity<HeadlessProfilesView>,
}
impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = zork_gui::design::ZORK_UI.palette;
        div()
            .size_full()
            .font_family("Inter Variable")
            .text_size(px(13.))
            .text_color(rgb(p.text))
            .bg(rgb(p.canvas))
            .child(
                div()
                    .id("settings-scroll")
                    .h_full()
                    .overflow_y_scroll()
                    .child(zork_gui::desktop::headless_settings_content(
                        self.inner.clone(),
                    )),
            )
    }
}

struct Fixture {
    view: Entity<HeadlessProfilesView>,
    cx: HeadlessAppContext,
    window: WindowHandle<AutomationRoot<Frame>>,
    driver: HeadlessAutomation,
    name: &'static str,
}

impl Fixture {
    fn new(name: &'static str, custom: bool) -> anyhow::Result<Self> {
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
        let mut view = None;
        let window = cx.open_window(gpui::size(px(1100.), px(820.)), |_, cx| {
            let inner = cx.new(|cx| HeadlessProfilesView::headless_model_fixture(custom, cx));
            view = Some(inner.clone());
            let frame = cx.new(|_| Frame { inner });
            cx.new(|_| AutomationRoot::new(frame))
        })?;
        let mut f = Self {
            cx,
            window,
            view: view.unwrap(),
            driver,
            name,
        };
        f.idle(50)?;
        Ok(f)
    }
    fn frame(&mut self) -> anyhow::Result<()> {
        self.cx.run_until_parked();
        self.cx.update_window(self.window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })?;
        Ok(())
    }
    /// Lets `ms` of virtual time pass (the recognition debounce is 250 ms).
    fn idle(&mut self, ms: u64) -> anyhow::Result<()> {
        let mut left = ms;
        while left > 0 {
            let step = left.min(16);
            self.cx.advance_clock(Duration::from_millis(step));
            self.frame()?;
            left -= step;
        }
        for _ in 0..3 {
            self.frame()?;
        }
        Ok(())
    }
    fn act(&mut self, value: Value) -> anyhow::Result<()> {
        let action: UserAction = serde_json::from_value(value.clone())?;
        self.cx
            .update_window(self.window.into(), |_, w, cx| {
                self.driver.dispatch(action, w, cx)
            })?
            .with_context(|| format!("{}: {value}", self.name))?;
        self.idle(64)
    }
    fn click(&mut self, id: &str) -> anyhow::Result<()> {
        if self.exists(id) {
            self.reveal(id)?;
        }
        ensure!(self.has(id), "{}: nothing to click at {id}", self.name);
        self.act(json!({"type":"click","target":{"element_id":id}}))
    }
    /// Clicks inside a popover (an overlay the automation tree does not
    /// capture) at `dy` px below the bottom of `anchor`, `dx` px from its left.
    fn click_below(&mut self, anchor: &str, dx: f32, dy: f32) -> anyhow::Result<()> {
        let element = self
            .driver
            .snapshot(true)
            .elements
            .into_iter()
            .find(|e| e.id == anchor)
            .with_context(|| format!("{}: {anchor} not rendered", self.name))?;
        let x = element.bounds.x + dx;
        let y = element.bounds.y + element.bounds.height + dy;
        self.act(json!({"type":"click","target":{"x":x,"y":y}}))
    }
    fn key(&mut self, key: &str) -> anyhow::Result<()> {
        self.act(json!({"type":"key","keystroke":key}))
    }
    fn type_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.act(json!({"type":"type_text","text":text}))
    }
    fn replace_text(&mut self, text: &str) -> anyhow::Result<()> {
        self.key("cmd-a")?;
        self.type_text(text)
    }
    fn drag(&mut self, from: &str, to: &str) -> anyhow::Result<()> {
        self.act(json!({"type":"drag","from":{"element_id":from},"to":{"element_id":to},"steps":8}))
    }
    /// Present in the tree (it may be scrolled out of view).
    fn exists(&self, id: &str) -> bool {
        self.driver
            .snapshot(true)
            .elements
            .iter()
            .any(|e| e.id == id)
    }
    /// Scrolls the dialog until `id` is on screen, as a user would.
    fn reveal(&mut self, id: &str) -> anyhow::Result<()> {
        for _ in 0..12 {
            let snapshot = self.driver.snapshot(true);
            let Some(element) = snapshot.elements.iter().find(|e| e.id == id) else {
                anyhow::bail!("{}: {id} is not rendered", self.name);
            };
            if element.visible && element.visible_bounds.height >= element.bounds.height - 1. {
                return Ok(());
            }
            let Some(area) = snapshot
                .elements
                .iter()
                .find(|e| e.id == "model-editor-scroll")
            else {
                return Ok(());
            };
            let delta = area.center.y - element.center.y;
            self.act(json!({"type":"scroll","target":{"element_id":"model-editor-scroll"},"delta_y":delta}))?;
        }
        anyhow::bail!("{}: could not scroll {id} into view", self.name)
    }
    fn has(&self, id: &str) -> bool {
        self.driver
            .snapshot(false)
            .elements
            .iter()
            .any(|e| e.id == id && e.visible)
    }
    fn label(&self, id: &str) -> Option<String> {
        self.driver
            .snapshot(true)
            .elements
            .into_iter()
            .find(|e| e.id == id)
            .map(|e| e.label)
    }
    fn state(&self) -> Value {
        self.view.read_with(&self.cx, |v, cx| v.headless_state(cx))
    }
    fn editor(&self) -> Value {
        self.state()["editor"].clone()
    }
    fn view(&self) -> Value {
        self.editor()["view"].clone()
    }
    fn focus(&mut self) -> Option<&'static str> {
        let view = self.view.clone();
        self.cx
            .update_window(self.window.into(), |_, w, cx| {
                view.update(cx, |v, cx| v.headless_editor_focus(w, cx))
            })
            .ok()
            .flatten()
    }
    fn text(&self, field: &str) -> String {
        self.view
            .read_with(&self.cx, |v, cx| v.headless_editor_text(field, cx))
    }
    fn section(&self, key: &str) -> Value {
        self.view()["sections"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| s["section"] == key)
            .cloned()
            .unwrap_or(Value::Null)
    }
    fn shot(&mut self, name: &str) -> anyhow::Result<()> {
        let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/headless-interactions/model-editor");
        std::fs::create_dir_all(&out)?;
        self.frame()?;
        let pixels = self.cx.capture_screenshot(self.window.into())?;
        pixels.save(out.join(format!("{name}.png")))?;
        // Capturing draws its own frame; lay out a normal one before input.
        self.idle(32)
    }
    fn open_add(&mut self) -> anyhow::Result<()> {
        self.click("profile-model-add")?;
        let add = self
            .driver
            .snapshot(true)
            .elements
            .into_iter()
            .find(|e| e.id == "profile-model-add")
            .map(|e| (e.enabled, e.visible, e.bounds));
        ensure!(
            self.has("model-editor-dialog"),
            "add dialog did not open: editor={} busy={} message={} add={add:?}",
            self.editor(),
            self.state()["busy"],
            self.state()["message"]
        );
        ensure!(
            self.focus() == Some("id"),
            "the id field is not focused on open"
        );
        Ok(())
    }
    /// Adds with a typed id and settles it with Enter.
    fn add_with(&mut self, id: &str) -> anyhow::Result<()> {
        self.open_add()?;
        self.type_text(id)?;
        self.key("enter")?;
        Ok(())
    }
}

fn suggestions_and_keyboard() -> anyhow::Result<()> {
    let mut f = Fixture::new("suggestions", false)?;
    f.open_add()?;
    f.type_text("gpt-5-n")?;
    let editor = f.editor();
    ensure!(
        editor["suggestions_open"] == true,
        "suggestions did not open"
    );
    let ids: Vec<String> = editor["view"]["id"]["suggestions"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap().iter())
        .map(|i| i["id"].as_str().unwrap().to_owned())
        .collect();
    ensure!(
        ids.iter().any(|i| i == "gpt-5-nano"),
        "gpt-5-nano not suggested: {ids:?}"
    );
    ensure!(
        !ids.iter().any(|i| i == "gpt-5" || i == "gpt-5-mini"),
        "added ids suggested"
    );
    f.shot("suggestions-open")?;
    ensure!(
        !editor["view"]["id"]["suggestions"]["custom"].is_null(),
        "custom row missing"
    );
    // While typing an unknown prefix, nothing is concluded below the field.
    f.idle(300)?;
    ensure!(
        f.view()["sections"].as_array().unwrap().is_empty(),
        "form shown while the id was still being typed"
    );
    ensure!(
        f.view()["status"].is_null(),
        "status concluded while typing"
    );
    f.key("down")?;
    ensure!(
        f.editor()["suggestion"] == 0,
        "↓ did not highlight the first row"
    );
    ensure!(f.focus() == Some("id"), "↓ moved focus out of the field");
    f.key("up")?;
    f.key("down")?;
    let first = ids[0].clone();
    f.key("enter")?;
    let editor = f.editor();
    ensure!(
        editor["state"]["id"] == first.as_str(),
        "Enter did not pick {first}"
    );
    ensure!(
        editor["state"]["settled"] == true && editor["suggestions_open"] == false,
        "pick did not settle"
    );
    ensure!(f.text("id") == first, "field text not updated to the pick");
    ensure!(
        f.editor()["suggestions_open"] == false,
        "popover stayed open after picking"
    );
    ensure!(
        f.view()["status"]["kind"] == "recognized",
        "picked preset not recognized"
    );
    // Escape closes the popover first, then the dialog.
    f.type_text("x")?;
    ensure!(
        f.editor()["suggestions_open"] == true,
        "typing again did not reopen suggestions"
    );
    f.key("escape")?;
    ensure!(
        f.editor()["suggestions_open"] == false && f.has("model-editor-dialog"),
        "Escape closed the dialog before the popover"
    );
    f.key("escape")?;
    ensure!(
        !f.has("model-editor-dialog"),
        "second Escape did not close the dialog"
    );
    // Pointer pick.
    f.open_add()?;
    f.type_text("o3")?;
    // popover: 6 gap + 6 padding + 28 group title + half a 38 px row
    f.click_below("profile-model", 120., 6. + 6. + 28. + 19.)?;
    ensure!(
        f.editor()["state"]["settled"] == true,
        "click did not settle"
    );
    ensure!(
        f.focus() == Some("id"),
        "clicking a suggestion took focus from the field"
    );
    // Enter with nothing highlighted keeps the typed id.
    f.key("escape")?;
    f.open_add()?;
    f.type_text("my-model")?;
    f.key("enter")?;
    ensure!(
        f.editor()["state"]["id"] == "my-model",
        "Enter did not settle the typed id"
    );
    ensure!(
        f.view()["full"] == true,
        "unknown id did not open the numbered form"
    );
    f.shot("suggestions-settled")?;
    Ok(())
}

fn recognized_sections() -> anyhow::Result<()> {
    let mut f = Fixture::new("recognized", false)?;
    f.add_with("gpt-5-nano")?;
    let view = f.view();
    ensure!(
        view["status"]["detail"] == "参数已按预设填好",
        "status: {}",
        view["status"]
    );
    ensure!(view["full"] == false);
    ensure!(
        view["sections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["open"] == false),
        "recognized sections should start collapsed"
    );
    f.click("model-section-length")?;
    ensure!(f.section("length")["open"] == true && f.section("thinking")["open"] == false);
    f.click("model-section-image")?;
    ensure!(f.section("image")["open"] == true && f.section("length")["open"] == true);
    f.click("model-section-length")?;
    ensure!(
        f.section("length")["open"] == false && f.section("image")["open"] == true,
        "rows are not individually collapsible"
    );
    // Recognition runs 250 ms after typing stops, keeping the old one meanwhile.
    f.click("profile-model")?;
    f.key("cmd-a")?;
    f.type_text("gpt-5-nano-2026-08-07")?;
    ensure!(
        f.view()["id"]["pending"] == true,
        "recognition should be pending"
    );
    ensure!(
        f.view()["status"]["title"] == "GPT-5 nano",
        "previous recognition vanished while pending"
    );
    f.idle(300)?;
    ensure!(
        f.view()["id"]["pending"] == false,
        "recognition did not run after the pause"
    );
    let detail = f.view()["status"]["detail"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    ensure!(
        detail.contains("是它的变体"),
        "variant not explained: {detail}"
    );
    f.shot("recognized")?;
    Ok(())
}

fn modify_restore_and_refill() -> anyhow::Result<()> {
    let mut f = Fixture::new("modify", false)?;
    f.add_with("gpt-5-nano")?;
    f.click("model-section-thinking")?;
    ensure!(f.has("model-level-0"), "levels editor missing");
    f.click("model-level-0")?;
    let thinking = f.section("thinking");
    ensure!(
        !thinking["provenance"].is_null(),
        "change not marked: {thinking}"
    );
    ensure!(
        thinking["provenance"]["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("预设：档位 · 默认 ")),
        "provenance text: {}",
        thinking["provenance"]
    );
    ensure!(f.has("model-restore-thinking"));
    f.shot("modified")?;
    f.click("model-restore-thinking")?;
    ensure!(
        f.section("thinking")["provenance"].is_null(),
        "restore did not clear the mark"
    );
    ensure!(
        f.section("thinking")["open"] == true,
        "restore toggled the row"
    );
    // A modified form is not overwritten by a new id…
    f.click("model-section-image")?;
    f.click("model-image")?;
    f.click("profile-model")?;
    f.replace_text("gpt-4o")?;
    f.key("enter")?;
    let status = f.view()["status"].clone();
    ensure!(!status["refill"].is_null(), "no refill offer: {status}");
    ensure!(
        status["refill"]["text"] == "ID 对应 GPT-4o，你改过参数所以没有覆盖",
        "refill text {}",
        status["refill"]["text"]
    );
    ensure!(
        f.editor()["state"]["image"] == false,
        "user value overwritten"
    );
    f.click("model-refill-action")?;
    ensure!(f.view()["status"]["refill"].is_null(), "refill stayed");
    ensure!(
        f.editor()["state"]["image"] == true,
        "refill did not apply GPT-4o"
    );
    // …while an untouched one follows the id.
    f.click("profile-model")?;
    f.replace_text("o3")?;
    f.key("enter")?;
    ensure!(
        f.view()["status"]["title"] == "o3",
        "status {}",
        f.view()["status"]
    );
    ensure!(
        f.view()["status"]["refill"].is_null(),
        "untouched preset should refill silently"
    );
    Ok(())
}

fn unknown_form_and_save_errors() -> anyhow::Result<()> {
    let mut f = Fixture::new("unknown", false)?;
    f.add_with("my-model")?;
    let view = f.view();
    ensure!(view["status"]["title"] == "没有这个模型的预设");
    ensure!(view["full"] == true);
    ensure!(
        view["thinking"]["selected"].is_null(),
        "a kind was pre-selected"
    );
    ensure!(
        f.exists("model-question-thinking")
            && f.exists("model-question-length")
            && f.exists("model-question-image")
    );
    // No errors before trying to save.
    ensure!(!f.exists("model-thinking-error") && !f.exists("profile-context-limit-error"));
    f.click("profile-model-save")?;
    ensure!(f.has("model-editor-dialog"), "invalid form closed");
    ensure!(f.exists("model-thinking-error"), "thinking error missing");
    ensure!(f.label("profile-context-limit-error").as_deref() == Some("填写上下文长度"));
    ensure!(f.label("profile-output-limit-error").as_deref() == Some("填写最长输出"));
    ensure!(
        f.focus() == Some("thinking"),
        "focus not on the first error: {:?}",
        f.focus()
    );
    ensure!(f.text("id") == "my-model", "typed id lost");
    f.shot("save-errors")?;
    // Fix it in order; each save moves to the next error.
    f.click("model-kind-0")?;
    f.click("profile-model-save")?;
    ensure!(f.focus() == Some("context"), "focus {:?}", f.focus());
    f.type_text("32K")?;
    f.click("profile-model-save")?;
    ensure!(f.focus() == Some("output"), "focus {:?}", f.focus());
    f.type_text("4K")?;
    f.click("profile-model-save")?;
    ensure!(!f.has("model-editor-dialog"), "valid form did not save");
    ensure!(
        f.exists("model-edit-my-model"),
        "saved model missing from the list"
    );
    let notice = f.state()["notice"].clone();
    ensure!(notice == "已添加 my-model", "toast {notice}");
    Ok(())
}

fn levels_editing() -> anyhow::Result<()> {
    let mut f = Fixture::new("levels", false)?;
    f.add_with("my-model")?;
    f.click("model-kind-3")?;
    let levels = |f: &Fixture| -> Vec<(String, bool)> {
        f.view()["thinking"]["levels"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| (v["name"].as_str().unwrap().to_owned(), v["default"] == true))
            .collect()
    };
    ensure!(
        levels(&f)
            == vec![
                ("low".into(), false),
                ("medium".into(), true),
                ("high".into(), false)
            ]
    );
    f.click("model-level-add")?;
    ensure!(f.editor()["level_open"] == true, "add popover did not open");
    ensure!(
        f.focus() == Some("level"),
        "name field not focused: {:?}",
        f.focus()
    );
    // ↓ then Enter picks the first common name.
    f.key("down")?;
    ensure!(
        f.editor()["level_highlight"] == 0,
        "↓ did not highlight a name"
    );
    f.key("enter")?;
    ensure!(
        levels(&f)[0].0 == "minimal",
        "common name not in canonical place: {:?}",
        levels(&f)
    );
    ensure!(
        f.editor()["level_open"] == false,
        "popover stayed after adding"
    );
    // Pointer pick of a name (first row under the chips).
    f.click("model-level-add")?;
    f.click_below("model-level-0", 20., 1. + 6. + 6. + 17.)?;
    ensure!(
        levels(&f).iter().any(|l| l.0 == "xhigh"),
        "pointer pick failed: {:?}",
        levels(&f)
    );
    f.click("model-level-add")?;
    f.type_text("a b")?;
    f.key("enter")?;
    ensure!(f.has("model-level-error"), "invalid name accepted");
    f.key("cmd-a")?;
    f.type_text("turbo")?;
    ensure!(!f.has("model-level-error"), "error stayed while typing");
    f.key("enter")?;
    ensure!(
        levels(&f).last().unwrap().0 == "turbo",
        "custom name not appended"
    );
    // Esc closes the popover, not the dialog.
    f.click("model-level-add")?;
    f.key("escape")?;
    ensure!(
        f.editor()["level_open"] == false && f.has("model-editor-dialog"),
        "Esc layering for + 档位"
    );
    // Default and delete.
    f.click("model-level-3")?;
    ensure!(levels(&f)[3] == ("high".into(), true));
    f.click("model-level-3-remove")?;
    let now = levels(&f);
    ensure!(!now.iter().any(|l| l.0 == "high"), "delete failed");
    ensure!(
        now[3] == ("xhigh".into(), true),
        "default did not move to the neighbour: {now:?}"
    );
    // Drag to reorder.
    f.drag("model-level-0-chip", "model-level-2-chip")?;
    let now = levels(&f);
    ensure!(now[2].0 == "minimal", "drag did not reorder: {now:?}");
    f.shot("levels")?;
    Ok(())
}

fn budget_editing() -> anyhow::Result<()> {
    let mut f = Fixture::new("budget", false)?;
    f.add_with("my-model")?;
    f.click("profile-context-limit")?;
    f.type_text("128K")?;
    f.click("profile-output-limit")?;
    f.type_text("16K")?;
    f.click("model-kind-4")?;
    let chips = f.view()["thinking"]["budget"]["presets"].clone();
    ensure!(
        chips[1]["bad"] == true && chips[0]["bad"] == false,
        "bad chips {chips}"
    );
    ensure!(
        f.exists("model-thinking-error"),
        "budget ≥ output not reported"
    );
    f.click("model-budget-add")?;
    ensure!(f.focus() == Some("budget"), "budget input not focused");
    f.type_text("20K")?;
    f.key("enter")?;
    ensure!(f.label("model-budget-error").as_deref() == Some("需小于最长输出 16K"));
    f.key("cmd-a")?;
    f.type_text("4000")?;
    f.key("enter")?;
    ensure!(f.label("model-budget-error").as_deref() == Some("已经有 4K"));
    f.key("cmd-a")?;
    f.type_text("8K")?;
    f.key("enter")?;
    ensure!(!f.has("model-budget-error"), "valid budget rejected");
    ensure!(
        f.focus() == Some("budget"),
        "the field should stay for the next value"
    );
    f.key("escape")?;
    ensure!(
        f.has("model-editor-dialog") && !f.has("model-budget-input"),
        "Esc layering"
    );
    // Remove the ones above the output limit.
    f.click("model-budget-3-remove")?;
    f.click("model-budget-2-remove")?;
    ensure!(
        !f.exists("model-thinking-error"),
        "error stayed after fixing"
    );
    // Switches: the default follows the remaining options.
    f.click("model-budget-dynamic")?;
    f.click("model-budget-default-1")?;
    ensure!(f.editor()["state"]["scheme"]["default"]["kind"] == "dynamic");
    f.click("model-budget-dynamic")?;
    ensure!(
        f.editor()["state"]["scheme"]["default"]["kind"] == "off",
        "default did not fall back"
    );
    f.click("model-budget-off")?;
    ensure!(
        f.editor()["state"]["scheme"]["default"]["kind"] == "tokens",
        "default did not move to a budget"
    );
    f.click("model-budget-0-remove")?;
    f.click("model-budget-0-remove")?;
    ensure!(
        f.label("model-thinking-error").as_deref() == Some("至少保留一个预算，或改成“不思考”"),
        "empty budget error: {:?}",
        f.label("model-thinking-error")
    );
    f.shot("budget")?;
    Ok(())
}

fn length_fields() -> anyhow::Result<()> {
    let mut f = Fixture::new("length", false)?;
    f.add_with("my-model")?;
    f.click("profile-context-limit")?;
    ensure!(
        f.exists("profile-context-limit-picks"),
        "quick picks hidden while focused: focus={:?}",
        f.focus()
    );
    ensure!(!f.exists("profile-output-limit-picks"));
    f.type_text("12x")?;
    ensure!(
        !f.has("profile-context-limit-error"),
        "error shown on a keystroke"
    );
    f.click("profile-output-limit")?;
    ensure!(
        f.label("profile-context-limit-error").as_deref() == Some("写成 128K 或 131072 这样的数字"),
        "blur did not report the bad value"
    );
    ensure!(
        !f.exists("profile-context-limit-picks"),
        "picks stayed after blur"
    );
    f.click("profile-context-limit")?;
    f.key("cmd-a")?;
    f.type_text("131072")?;
    ensure!(f.view()["length"]["context"]["exact"] == "131,072");
    f.key("enter")?;
    ensure!(
        f.text("context") == "131,072",
        "Enter did not normalize: {}",
        f.text("context")
    );
    ensure!(f.focus() == Some("context"));
    f.key("cmd-a")?;
    f.type_text("128000")?;
    f.click("profile-output-limit")?;
    ensure!(
        f.text("context") == "128K",
        "blur did not normalize: {}",
        f.text("context")
    );
    // ↑/↓ step through enabled picks; picks ≥ context are disabled.
    f.key("up")?;
    ensure!(
        f.text("output") == "8K",
        "↑ from empty: {}",
        f.text("output")
    );
    f.key("up")?;
    f.key("up")?;
    f.key("up")?;
    f.key("up")?;
    ensure!(
        f.text("output") == "64K",
        "128K is not below the context: {}",
        f.text("output")
    );
    ensure!(f.view()["length"]["output"]["picks"][4]["disabled"] == true);
    f.key("down")?;
    ensure!(f.text("output") == "32K");
    f.click("profile-output-limit-pick-1")?;
    ensure!(
        f.text("output") == "16K" && f.focus() == Some("output"),
        "pick lost focus"
    );
    // Output ≥ context shows at once.
    f.key("cmd-a")?;
    f.type_text("200K")?;
    ensure!(
        f.label("profile-output-limit-error").as_deref() == Some("需小于上下文 128K"),
        "conflict not shown: {:?}",
        f.label("profile-output-limit-error")
    );
    f.click("profile-context-limit")?;
    f.key("cmd-a")?;
    f.type_text("1M")?;
    ensure!(
        !f.has("profile-output-limit-error"),
        "conflict stayed after raising the context"
    );
    f.shot("length")?;
    Ok(())
}

fn duplicate_goes_to_edit() -> anyhow::Result<()> {
    let mut f = Fixture::new("duplicate", false)?;
    f.open_add()?;
    f.type_text("gpt-5")?;
    f.key("enter")?;
    ensure!(
        f.view()["id"]["error"] == "这个连接里已经有 gpt-5",
        "duplicate message {}",
        f.view()["id"]["error"]
    );
    ensure!(
        f.view()["sections"].as_array().unwrap().is_empty(),
        "form shown for a duplicate"
    );
    f.click("model-duplicate-edit")?;
    ensure!(!f.has("model-editor-dialog"), "dialog stayed open");
    ensure!(f.has("model-inline-editor"), "inline editor not opened");
    ensure!(f.editor()["view"]["title"] == "gpt-5");
    Ok(())
}

fn fill_from_similar() -> anyhow::Result<()> {
    let mut f = Fixture::new("sources", false)?;
    f.add_with("my-model")?;
    f.click("model-source-open")?;
    ensure!(
        f.editor()["sources_open"] == true,
        "source popover did not open"
    );
    ensure!(
        f.focus() == Some("query"),
        "query not focused: {:?}",
        f.focus()
    );
    ensure!(
        f.editor()["source_items"]
            .as_array()
            .is_some_and(|i| !i.is_empty()),
        "no sources listed"
    );
    f.type_text("sonnet")?;
    ensure!(f.editor()["sources_open"] == true);
    f.key("enter")?;
    ensure!(
        f.editor()["sources_open"] == false,
        "popover stayed after picking"
    );
    let view = f.view();
    ensure!(
        view["status"]["kind"] == "source",
        "status {}",
        view["status"]
    );
    ensure!(view["full"] == false);
    ensure!(
        ["thinking", "length", "image"]
            .iter()
            .all(|k| f.section(k)["open"] == true),
        "filled sections should be open"
    );
    ensure!(f.editor()["state"]["id"] == "my-model", "id replaced");
    // “你的模型” lists this connection's configured models, not the edited one.
    f.key("escape")?;
    f.key("escape")?;
    f.click("model-edit-gpt-5-mini")?;
    f.click("model-source-open")?;
    let offered: Vec<String> = f.editor()["source_items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l.as_str().map(str::to_owned))
        .collect();
    ensure!(!offered.is_empty(), "no sources for the inline editor");
    ensure!(
        !offered.iter().any(|l| l == "gpt-5-mini"),
        "the edited model offered as its own source: {offered:?}"
    );
    ensure!(
        offered.iter().any(|l| l == "gpt-5"),
        "other configured models missing: {offered:?}"
    );
    Ok(())
}

fn custom_protocol() -> anyhow::Result<()> {
    let mut f = Fixture::new("custom", true)?;
    f.add_with("qwen3-coder-plus")?;
    ensure!(
        !f.section("api").is_null(),
        "protocol row missing for a custom connection"
    );
    // The fixture's custom template defaults to the Responses protocol.
    ensure!(
        f.section("api")["summary"] == "OpenAI Responses · 跟随连接",
        "api summary {}",
        f.section("api")["summary"]
    );
    f.click("model-section-api")?;
    f.click("model-api-3")?;
    ensure!(
        f.has("model-api-warning"),
        "no warning when leaving the default"
    );
    ensure!(f.editor()["state"]["api"] == "anthropic-messages");
    f.shot("custom-protocol")?;
    // Other connections never show it.
    let mut g = Fixture::new("custom-other", false)?;
    g.add_with("gpt-5-nano")?;
    ensure!(
        g.section("api").is_null(),
        "protocol shown for a vendor connection"
    );
    Ok(())
}

fn inline_edit_save_and_remove() -> anyhow::Result<()> {
    let mut f = Fixture::new("inline", false)?;
    ensure!(
        f.label("model-edit-gpt-5")
            .is_some_and(|l| l.contains("400K · 档位")),
        "row meta {:?}",
        f.label("model-edit-gpt-5")
    );
    ensure!(f
        .label("model-edit-internal-preview")
        .is_some_and(|l| l.contains("待配置")));
    f.click("model-edit-gpt-5-mini")?;
    ensure!(f.has("model-inline-editor"), "inline editor did not open");
    ensure!(
        f.view()["status"]["detail"] == "1 项与预设不同",
        "{}",
        f.view()["status"]
    );
    ensure!(f.has("profile-model-remove"));
    ensure!(!f.section("thinking")["provenance"].is_null());
    f.shot("inline")?;
    // Clicking the same row again closes it; stale state never survives.
    f.click("model-edit-gpt-5-mini")?;
    ensure!(
        !f.has("model-inline-editor"),
        "row click did not close the editor"
    );
    f.click("model-edit-gpt-5-mini")?;
    f.click("model-restore-thinking")?;
    f.click("profile-model-save")?;
    ensure!(!f.has("model-inline-editor"), "save did not close");
    ensure!(f.state()["notice"] == "已保存 gpt-5-mini");
    ensure!(
        f.label("model-edit-gpt-5-mini")
            .is_some_and(|l| l.contains("400K")),
        "row not updated"
    );
    // An unconfigured model opens the numbered form.
    f.click("model-edit-internal-preview")?;
    ensure!(
        f.view()["full"] == true,
        "待配置 model did not open the numbered form"
    );
    f.key("escape")?;
    ensure!(
        !f.has("model-inline-editor"),
        "Escape did not close the inline editor"
    );
    // Remove.
    f.click("model-edit-fixture-model")?;
    f.click("profile-model-remove")?;
    ensure!(!f.exists("model-edit-fixture-model"), "model not removed");
    ensure!(f.state()["notice"] == "已移除 fixture-model");
    // Reopening add after cancel starts clean.
    f.shot("before-reopen")?;
    f.open_add()?;
    f.type_text("abc")?;
    // The suggestion list overlays the footer while typing; Esc closes it.
    f.key("escape")?;
    f.click("profile-model-cancel")?;
    ensure!(!f.has("model-editor-dialog"), "cancel did not close");
    f.open_add()?;
    ensure!(
        f.text("id").is_empty() && f.view()["sections"].as_array().unwrap().is_empty(),
        "stale add state"
    );
    Ok(())
}

fn fetch_counts() -> anyhow::Result<()> {
    let mut f = Fixture::new("fetch", false)?;
    f.click("profile-model-discover")?;
    f.idle(200)?;
    let notice = f.state()["notice"].clone();
    ensure!(
        notice == "获取到 3 个新模型：2 个已填好，1 个待配置",
        "fetch toast {notice}"
    );
    ensure!(
        f.label("model-edit-gpt-5-nano")
            .is_some_and(|l| l.contains("400K")),
        "recognized import not filled: {:?}",
        f.label("model-edit-gpt-5-nano")
    );
    ensure!(
        f.label("model-edit-vendor-preview-x")
            .is_some_and(|l| l.contains("待配置")),
        "unknown import not pending"
    );
    f.shot("fetched")?;
    Ok(())
}

fn rapid_typing_and_long_ids() -> anyhow::Result<()> {
    let mut f = Fixture::new("typing", false)?;
    f.open_add()?;
    for chunk in ["deep", "seek", "-rea", "soner"] {
        f.act(json!({"type":"type_text","text":chunk}))?;
        ensure!(f.focus() == Some("id"), "focus lost while typing");
    }
    ensure!(
        f.text("id") == "deepseek-reasoner",
        "text lost: {}",
        f.text("id")
    );
    f.idle(300)?;
    ensure!(f.view()["status"]["title"] == "DeepSeek Reasoner");
    f.key("cmd-a")?;
    f.type_text(&"x".repeat(240))?;
    f.key("enter")?;
    ensure!(
        f.view()["id"]["error"]
            .as_str()
            .is_some_and(|e| e.contains("200")),
        "long id accepted: {}",
        f.view()["id"]["error"]
    );
    let card = f
        .driver
        .snapshot(false)
        .elements
        .into_iter()
        .find(|e| e.id == "model-editor-dialog")
        .unwrap()
        .bounds;
    ensure!(card.width < 600., "long id stretched the dialog: {card:?}");
    f.shot("long-id")?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    std::env::set_var("SEED", "0");
    let state = tempfile::tempdir()?;
    std::env::set_var(
        "ZORK_GUI_PREFERENCES_PATH",
        state.path().join("preferences.json"),
    );
    let only = std::env::args().nth(1);
    let checks: [(&str, fn() -> anyhow::Result<()>); 13] = [
        ("suggestions", suggestions_and_keyboard),
        ("recognized", recognized_sections),
        ("modify", modify_restore_and_refill),
        ("unknown", unknown_form_and_save_errors),
        ("levels", levels_editing),
        ("budget", budget_editing),
        ("length", length_fields),
        ("duplicate", duplicate_goes_to_edit),
        ("sources", fill_from_similar),
        ("custom", custom_protocol),
        ("inline", inline_edit_save_and_remove),
        ("fetch", fetch_counts),
        ("typing", rapid_typing_and_long_ids),
    ];
    let mut failed = vec![];
    for (name, check) in checks {
        if only
            .as_deref()
            .is_some_and(|o| o != name && !o.starts_with("--"))
        {
            continue;
        }
        match check() {
            Ok(()) => println!("ok   {name}"),
            Err(error) => {
                println!("FAIL {name}: {error:#}");
                failed.push(name);
            }
        }
    }
    ensure!(failed.is_empty(), "model editor checks failed: {failed:?}");
    println!("Model editor headless checks passed.");
    Ok(())
}
