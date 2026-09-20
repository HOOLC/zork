//! Interactive native host. Export fixtures retain their original catalog and dimensions.
use super::*;
use gpui::{AnyElement, ScrollHandle};
use std::collections::HashMap;
use zork_ui::{
    automation::{AutomationElementExt, AutomationRole},
    components::workbench as wb,
    controls as ui,
    liquid_story::business,
    navigation::TabGroup,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CanvasSize {
    Fit,
    Fixture,
    Narrow,
    Wide,
}
impl CanvasSize {
    const ALL: [Self; 4] = [Self::Fit, Self::Fixture, Self::Narrow, Self::Wide];
    fn label(self, story: &Story) -> String {
        match self {
            Self::Fit => "适应工作区".into(),
            Self::Fixture => format!("固定 {} × {}", story.width, story.height),
            Self::Narrow => "窄屏 320".into(),
            Self::Wide => "宽屏 900".into(),
        }
    }
}

struct Session {
    selected: usize,
    host: Entity<StoryHost>,
    size: CanvasSize,
    scroll: ScrollHandle,
    pending: bool,
}

pub(super) struct Gallery {
    catalog: Vec<Story>,
    families: Vec<usize>,
    selected: usize,
    sessions: HashMap<String, Session>,
    driver: HeadlessAutomation,
    navigation: TabGroup,
    scenario_open: bool,
    size_open: bool,
    show_source: bool,
    fixed_size: bool,
    #[cfg(feature = "native-blur-bench")]
    benchmark_started: bool,
}
impl Gallery {
    pub(super) fn new(
        catalog: Vec<Story>,
        selected: usize,
        driver: HeadlessAutomation,
        fixed_size: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut families: Vec<usize> = Vec::new();
        for (i, story) in catalog.iter().enumerate() {
            if !families.iter().any(|&j| catalog[j].family == story.family) {
                families.push(i);
            }
        }
        let mut result = Self {
            catalog,
            families,
            selected,
            sessions: HashMap::new(),
            driver,
            navigation: TabGroup::new(cx),
            scenario_open: false,
            size_open: false,
            show_source: false,
            fixed_size,
            #[cfg(feature = "native-blur-bench")]
            benchmark_started: false,
        };
        result.create_session(selected, cx);
        result
    }

    fn session(&self) -> &Session {
        &self.sessions[&self.catalog[self.selected].family]
    }

    fn create_session(&mut self, selected: usize, cx: &mut Context<Self>) {
        let story = self.catalog[selected].clone();
        let size = self.sessions.get(&story.family).map_or_else(
            || {
                if story.state == "narrow" {
                    CanvasSize::Narrow
                } else if self.fixed_size {
                    CanvasSize::Fixture
                } else {
                    CanvasSize::Fit
                }
            },
            |session| session.size,
        );
        let host = cx.new(|cx| StoryHost::new(story.clone(), cx));
        self.sessions.insert(
            story.family,
            Session {
                selected,
                host,
                size,
                scroll: ScrollHandle::new(),
                pending: true,
            },
        );
    }

    fn select_family(&mut self, index: usize, cx: &mut Context<Self>) {
        let family = &self.catalog[index].family;
        if *family == self.catalog[self.selected].family {
            return;
        }
        let selected = self.sessions.get(family).map_or(index, |s| s.selected);
        if !self.sessions.contains_key(family) {
            self.create_session(selected, cx);
        }
        self.selected = selected;
        self.scenario_open = false;
        self.size_open = false;
        cx.notify();
    }

    fn select_scenario(&mut self, index: usize, cx: &mut Context<Self>) {
        self.scenario_open = false;
        if index != self.selected {
            self.selected = index;
            self.create_session(index, cx);
        }
        cx.notify();
    }

    fn scenarios(&self) -> Vec<usize> {
        let selected = &self.catalog[self.selected];
        let has_normal_history = self.catalog.iter().any(|s| {
            s.family == selected.family && s.family == "history" && s.state == "collapsed"
        });
        let mut seen = std::collections::HashSet::new();
        self.catalog
            .iter()
            .enumerate()
            .filter(|(_, s)| s.family == selected.family)
            .filter(|(_, s)| {
                !has_normal_history || !matches!(s.state.as_str(), "expanded" | "narrow")
            })
            .filter(|(_, s)| {
                seen.insert(
                    s.state
                        .trim_end_matches("-compact")
                        .trim_end_matches("-wide"),
                )
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn scenario_label(story: &Story) -> String {
        if story.family == "history" {
            match story.state.as_str() {
                "collapsed" | "expanded" | "narrow" => return "正常记录".into(),
                "empty" => return "空记录".into(),
                "error" => return "加载失败".into(),
                _ => {}
            }
        }
        business::state_label(
            story
                .state
                .trim_end_matches("-compact")
                .trim_end_matches("-wide"),
        )
    }

    fn category(story: &Story) -> &'static str {
        if story.family == "liquid" {
            "交互实验"
        } else if business::is_business(&story.family) {
            "业务组件"
        } else {
            "基础组件"
        }
    }

    fn directory(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = self.navigation.column();
        for category in ["基础组件", "业务组件", "交互实验"] {
            let indices: Vec<_> = self
                .families
                .iter()
                .copied()
                .filter(|&i| Self::category(&self.catalog[i]) == category)
                .collect();
            if indices.is_empty() {
                continue;
            }
            let mut section = self.navigation.section(category, category).mt_4();
            for i in indices {
                let story = &self.catalog[i];
                section = section.child(
                    self.navigation
                        .tab(
                            format!("story-family-{}", story.family),
                            story.family == self.catalog[self.selected].family,
                        )
                        .aria_label(story.title.clone())
                        .child(story.title.clone())
                        .on_click(cx.listener(move |v, _, _, cx| v.select_family(i, cx)))
                        .automation(AutomationRole::Button, story.title.clone()),
                );
            }
            content = content.child(section);
        }
        self.navigation.surface(content).into_any_element()
    }

    fn toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let story = &self.catalog[self.selected];
        let scenarios = self.scenarios();
        let mut toolbar = wb::wrap(16.);
        if scenarios.len() > 1 {
            let options = scenarios
                .iter()
                .map(|&i| {
                    let candidate = &self.catalog[i];
                    (
                        format!("story-scenario-{}", candidate.id),
                        Self::scenario_label(candidate),
                        Self::scenario_label(candidate) == Self::scenario_label(story),
                    )
                })
                .collect();
            toolbar = toolbar.child(wb::row(8.).child(ui::label("场景")).child(
                wb::slot(172.).child(ui::dropdown(
                    "story-scenario",
                    Self::scenario_label(story),
                    options,
                    self.scenario_open,
                    true,
                    window,
                    cx,
                    |v, open, cx| {
                        v.scenario_open = open;
                        v.size_open = false;
                        cx.notify();
                    },
                    move |v, index, cx| v.select_scenario(scenarios[index], cx),
                )),
            ));
        }
        toolbar = toolbar.child(
            wb::row(8.).child(ui::label("画布")).child(
                wb::slot(172.).child(ui::dropdown(
                    "story-size",
                    self.session().size.label(story),
                    CanvasSize::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(i, size)| {
                            (
                                format!("story-size-{i}"),
                                size.label(story),
                                size == self.session().size,
                            )
                        })
                        .collect(),
                    self.size_open,
                    true,
                    window,
                    cx,
                    |v, open, cx| {
                        v.size_open = open;
                        v.scenario_open = false;
                        cx.notify();
                    },
                    |v, i, cx| {
                        v.size_open = false;
                        let family = &v.catalog[v.selected].family;
                        v.sessions.get_mut(family).unwrap().size = CanvasSize::ALL[i];
                        cx.notify();
                    },
                )),
            ),
        );
        if story.family == "history" && !matches!(story.state.as_str(), "empty" | "error") {
            for (id, label, expanded) in [
                ("story-expand", "全部展开", true),
                ("story-collapse", "全部收起", false),
            ] {
                toolbar = toolbar.child(
                    ui::button(id, label, false, true)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.session().host.clone().update(cx, |host, cx| {
                                host.set_history_expanded(expanded, cx);
                            });
                            cx.notify();
                        }))
                        .automation(AutomationRole::Button, label),
                );
            }
        }
        toolbar.px_5().py_3().into_any_element()
    }

    fn initial_actions(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session().pending || self.scenario_open || self.size_open {
            return;
        }
        let selected = self.selected;
        let host = self.session().host.clone();
        let owner = cx.entity().downgrade();
        window.on_next_frame(move |window, cx| {
            let replay = owner
                .update(cx, |v, cx| {
                    // A click can switch/reset the specimen before this frame arrives.
                    if v.selected != selected || v.session().host != host || !v.session().pending {
                        return None;
                    }
                    // The selector owns focus through its exit. Replay only after
                    // that transient input surface has relinquished input ownership.
                    if v.driver.snapshot(false).elements.iter().any(|e| {
                        e.visible
                            && matches!(e.id.as_str(), "story-scenario-menu" | "story-size-menu")
                    }) {
                        cx.notify();
                        return None;
                    }
                    let story = &v.catalog[selected];
                    v.sessions.get_mut(&story.family).unwrap().pending = false;
                    Some((story.actions.clone(), v.driver.clone()))
                })
                .ok()
                .flatten();
            // Input can bubble through Gallery's keyboard handlers. Never dispatch
            // while holding its mutable entity borrow.
            if let Some((actions, driver)) = replay {
                for action in actions {
                    if let Ok(input) = serde_json::from_value(action.clone()) {
                        let _ = driver.dispatch(input, window, cx);
                    }
                    if action["type"] == "key" && action["keystroke"] == "tab" {
                        if let Some(focus) = host.read(cx).specimen_focus(cx) {
                            window.focus(&focus, cx);
                        }
                    }
                }
            }
        });
    }
}

impl Render for Gallery {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "native-blur-bench")]
        if !self.benchmark_started {
            if let Ok(path) = std::env::var("ZORK_MODAL_FRAME_REPORT") {
                self.benchmark_started = true;
                let family = self.catalog[self.selected].family.clone();
                self.sessions.get_mut(&family).unwrap().pending = false;
                let actions = self.catalog[self.selected].actions.clone();
                let driver = self.driver.clone();
                cx.spawn_in(window,async move |_,cx| {
                    cx.background_executor().timer(Duration::from_millis(200)).await;
                    cx.update(|window,cx|{for action in actions {let _=driver.dispatch(serde_json::from_value(action).unwrap(),window,cx);}}).ok();
                    let mut samples=Vec::new();
                    let mut measured=Instant::now();
                    for index in 0..210 {
                        cx.background_executor().timer(Duration::from_millis(12)).await;
                        if index==30 { measured=Instant::now(); }
                        let start=Instant::now();
                        cx.update(|window,cx|{window.simulate_next_frame(cx);window.refresh();window.draw(cx).clear(cx);window.present_if_needed();}).ok();
                        if index>=30 {samples.push(start.elapsed().as_secs_f64()*1000.);}
                    }
                    let fps=samples.len() as f64/measured.elapsed().as_secs_f64();
                    samples.sort_by(f64::total_cmp);
                    let report=json!({"frames":samples.len(),"fps":fps,"mean_draw_present_ms":samples.iter().sum::<f64>()/samples.len() as f64,"p95_draw_present_ms":samples[(samples.len() as f64*0.95) as usize],"blur_disabled":std::env::var_os("ZORK_DISABLE_NATIVE_BLUR").is_some()});
                    std::fs::write(path,serde_json::to_vec_pretty(&report).unwrap()).unwrap();
                    cx.update(|_,cx|cx.quit()).ok();
                }).detach();
            }
        }
        self.initial_actions(window, cx);
        let story = self.catalog[self.selected].clone();
        let toolbar = self.toolbar(window, cx);
        let directory = self.directory(cx);
        let session = self.session();
        let canvas = wb::slot(story.width)
            .h(px(story.height))
            .when(session.size == CanvasSize::Fit, |v| v.w_full().h_full())
            .when(session.size == CanvasSize::Narrow, |v| {
                v.w(px(320.)).h_full()
            })
            .when(session.size == CanvasSize::Wide, |v| v.w(px(900.)).h_full())
            .child(session.host.clone())
            .id("story-canvas")
            .automation(AutomationRole::Status, "当前组件画布");
        let content = wb::column(0.)
            .flex_1()
            .min_w_0()
            .h_full()
            .child(wb::header(
                story.title.clone(),
                Some(Self::category(&story)),
                20.,
                ui::button("story-reset", "重置当前示例", false, true)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.create_session(v.selected, cx);
                        cx.notify();
                    }))
                    .automation(AutomationRole::Button, "重置当前示例"),
            ))
            .child(toolbar)
            .child(
                wb::viewport("story-preview-scroll", 20., canvas)
                    .overflow_scroll()
                    .track_scroll(&session.scroll),
            )
            .child(
                wb::row(12.)
                    .px_5()
                    .py_2()
                    .justify_between()
                    .child(ui::label(format!(
                        "{} · {}",
                        Self::scenario_label(&story),
                        session.size.label(&story)
                    )))
                    .child(
                        ui::button("story-source", "组件来源", false, true)
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.show_source = !v.show_source;
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "组件来源"),
                    ),
            )
            .when(self.show_source, |v| {
                v.child(ui::label(story.source).px_5().pb_3())
            });
        wb::shell(
            "native-storybook",
            wb::font(),
            gpui::Empty,
            wb::body()
                .child(
                    wb::rail(
                        "story-navigation",
                        248.,
                        wb::Rail::Navigation,
                        wb::column(0.)
                            .pt_5()
                            .child(ui::page_title("Zork / Components"))
                            .child(directory),
                    )
                    .automation(AutomationRole::ScrollArea, "组件目录"),
                )
                .child(content),
            None,
        )
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
