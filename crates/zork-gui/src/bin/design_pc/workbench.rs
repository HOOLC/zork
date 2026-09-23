//! Interactive native host. Export fixtures retain their original catalog and dimensions.
use super::*;
use gpui::{div, rgb, AnyElement, ScrollAnchor, ScrollHandle};
use std::collections::HashMap;
use zork_gui::design::ZORK_UI;
use zork_ui::{
    automation::{AutomationElementExt, AutomationRole},
    components::message::render_document,
    components::workbench as wb,
    controls as ui,
    liquid_story::business,
    navigation::TabGroup,
};

pub(super) type DirectoryGroup = (&'static str, &'static [&'static str]);
pub(super) type DirectorySection = (&'static str, &'static [DirectoryGroup]);

// Directory placement describes what a specimen is for. Business fixture
// routing is deliberately separate: a full page must not become a primitive
// just because its family is absent from the fixture registry.
pub(super) const DIRECTORY: &[DirectorySection] = &[
    (
        "基础组件",
        &[
            (
                "操作与输入",
                &[
                    "button",
                    "field",
                    "choice",
                    "switch",
                    "dropdown",
                    "modal",
                    "avatar-picker",
                ],
            ),
            (
                "状态与导航",
                &["interaction", "loading", "feedback", "navigation"],
            ),
            ("身份与图形", &["avatar", "providers", "icons", "brand"]),
        ],
    ),
    (
        "业务组件",
        &[
            (
                "消息与输入",
                &[
                    "composer",
                    "markdown",
                    "comments",
                    "message-interaction",
                    "message-reader",
                ],
            ),
            (
                "活动与记录",
                &[
                    "activity",
                    "history",
                    "history-details",
                    "member-activity",
                    "tooltip",
                ],
            ),
            (
                "文件与附件",
                &["attachment", "attachment-viewer", "conversation-files"],
            ),
            ("设备呈现", &["device-name", "chat-navigation"]),
        ],
    ),
    (
        "页面与流程",
        &[
            (
                "工作区",
                &[
                    "new-chat",
                    "conversation",
                    "browser",
                    "shared-files",
                    "resources",
                ],
            ),
            (
                "设备与模型",
                &[
                    "node-directory",
                    "connection",
                    "model",
                    "device",
                    "mesh",
                    "enrollment",
                ],
            ),
            (
                "客户端",
                &[
                    "onboarding",
                    "client",
                    "appearance",
                    "data-settings",
                    "notifications",
                ],
            ),
        ],
    ),
    ("交互实验", &[("", &["liquid"])]),
];

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
    guides: Vec<super::guide::Guide>,
    images: Vec<super::guide::DesignImage>,
    image_categories: Vec<String>,
    selected_guide: Option<usize>,
    selected_image_category: Option<String>,
    sessions: HashMap<String, Session>,
    driver: HeadlessAutomation,
    navigation: TabGroup,
    directory_scroll: ScrollHandle,
    directory_anchor: ScrollAnchor,
    reveal_initial: bool,
    scenario_open: bool,
    size_open: bool,
    show_source: bool,
    fixed_size: bool,
    #[cfg(feature = "native-blur-bench")]
    benchmark_started: bool,
}
impl Gallery {
    pub(super) fn show_overview(&mut self) {
        self.selected_guide = Some(0);
    }

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
        let directory_scroll = ScrollHandle::new();
        let images = super::guide::images();
        let mut image_categories = Vec::new();
        for image in &images {
            if !image_categories.contains(&image.category) {
                image_categories.push(image.category.clone());
            }
        }
        let mut result = Self {
            catalog,
            families,
            selected,
            guides: super::guide::catalog(),
            images,
            image_categories,
            selected_guide: None,
            selected_image_category: None,
            sessions: HashMap::new(),
            driver,
            navigation: TabGroup::new(cx),
            directory_anchor: ScrollAnchor::for_handle(directory_scroll.clone()),
            directory_scroll,
            reveal_initial: true,
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
        if *family == self.catalog[self.selected].family
            && self.selected_guide.is_none()
            && self.selected_image_category.is_none()
        {
            return;
        }
        self.selected_guide = None;
        self.selected_image_category = None;
        let selected = self.sessions.get(family).map_or(index, |s| s.selected);
        if !self.sessions.contains_key(family) {
            self.create_session(selected, cx);
        }
        self.selected = selected;
        self.scenario_open = false;
        self.size_open = false;
        cx.notify();
    }

    fn select_guide(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_guide = Some(index);
        self.selected_image_category = None;
        cx.notify();
    }

    fn select_images(&mut self, category: String, cx: &mut Context<Self>) {
        self.selected_guide = None;
        self.selected_image_category = Some(category);
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
        if story.family == "onboarding" {
            return match story
                .state
                .trim_end_matches("-compact")
                .trim_end_matches("-wide")
            {
                "login" => "登录",
                "waiting" => "等待浏览器",
                "preparing" => "准备本机",
                "failure" => "准备失败",
                "models" => "选择模型",
                "model-form" => "添加模型连接",
                "ready" => "开始对话",
                _ => "首次使用",
            }
            .into();
        }
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
        DIRECTORY
            .iter()
            .find(|(_, groups)| {
                groups
                    .iter()
                    .any(|(_, families)| families.contains(&story.family.as_str()))
            })
            .map(|(category, _)| *category)
            .unwrap_or("未分类")
    }

    fn directory(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut content = self.navigation.column();
        let mut placed = std::collections::HashSet::new();
        for (section_index, (category, groups)) in DIRECTORY.iter().enumerate() {
            let mut section = self.navigation.section(*category, *category).mt_4();
            let mut present = false;
            for (group_index, (group, families)) in groups.iter().enumerate() {
                let indices: Vec<_> = families
                    .iter()
                    .filter_map(|family| {
                        self.families
                            .iter()
                            .copied()
                            .find(|&i| self.catalog[i].family == *family)
                    })
                    .collect();
                if indices.is_empty() {
                    continue;
                }
                present = true;
                if !group.is_empty() {
                    section = section.child(
                        div()
                            .id(format!("story-subgroup-{section_index}-{group_index}"))
                            .h(px(20.))
                            .mt_2()
                            .pl(px(15.))
                            .flex()
                            .items_center()
                            .text_size(px(10.))
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child(*group)
                            .automation(AutomationRole::Status, *group),
                    );
                }
                for i in indices {
                    placed.insert(self.catalog[i].family.as_str());
                    section = section.child(self.story_tab(i, cx));
                }
            }
            if present {
                content = content.child(section);
            }
        }
        let uncategorized: Vec<_> = self
            .families
            .iter()
            .copied()
            .filter(|&i| !placed.contains(self.catalog[i].family.as_str()))
            .collect();
        if !uncategorized.is_empty() {
            let mut section = self.navigation.section("未分类", "未分类").mt_4();
            for i in uncategorized {
                section = section.child(self.story_tab(i, cx));
            }
            content = content.child(section);
        }
        let mut documents = self.navigation.section("设计规范", "设计规范").mt_4();
        for (index, guide) in self.guides.iter().enumerate() {
            documents = documents.child(
                self.navigation
                    .tab(
                        format!("design-guide-{}", guide.id),
                        self.selected_guide == Some(index),
                    )
                    .aria_label(guide.title.clone())
                    .when(self.selected_guide == Some(index), |v| {
                        v.anchor_scroll(Some(self.directory_anchor.clone()))
                    })
                    .child(guide.title.clone())
                    .on_click(cx.listener(move |v, _, _, cx| v.select_guide(index, cx)))
                    .automation(AutomationRole::Button, guide.title.clone()),
            );
        }
        content = content.child(documents);
        let mut assets = self.navigation.section("设计素材", "设计素材").mt_4();
        for category in &self.image_categories {
            let target = category.clone();
            let label = super::guide::category_label(category).to_owned();
            assets = assets.child(
                self.navigation
                    .tab(
                        format!("design-assets-{category}"),
                        self.selected_image_category.as_ref() == Some(category),
                    )
                    .aria_label(label.clone())
                    .when(
                        self.selected_image_category.as_ref() == Some(category),
                        |v| v.anchor_scroll(Some(self.directory_anchor.clone())),
                    )
                    .child(label.clone())
                    .on_click(cx.listener(move |v, _, _, cx| v.select_images(target.clone(), cx)))
                    .automation(AutomationRole::Button, label),
            );
        }
        content = content.child(assets);
        self.navigation.surface(content).into_any_element()
    }

    fn story_tab(&self, i: usize, cx: &mut Context<Self>) -> AnyElement {
        let story = &self.catalog[i];
        self.navigation
            .tab(
                format!("story-family-{}", story.family),
                self.selected_guide.is_none()
                    && self.selected_image_category.is_none()
                    && story.family == self.catalog[self.selected].family,
            )
            .pl(px(15.))
            .aria_label(story.title.clone())
            .when(
                self.selected_guide.is_none()
                    && self.selected_image_category.is_none()
                    && story.family == self.catalog[self.selected].family,
                |v| v.anchor_scroll(Some(self.directory_anchor.clone())),
            )
            .child(story.title.clone())
            .on_click(cx.listener(move |v, _, _, cx| v.select_family(i, cx)))
            .automation(AutomationRole::Button, story.title.clone())
            .into_any_element()
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
            let mut expansion = wb::row(8.);
            for (id, label, expanded) in [
                ("story-expand", "全部展开", true),
                ("story-collapse", "全部收起", false),
            ] {
                expansion = expansion.child(
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
            toolbar = toolbar.child(expansion);
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
        if std::mem::take(&mut self.reveal_initial) {
            let owner = cx.entity().downgrade();
            let anchor = self.directory_anchor.clone();
            let id = if let Some(index) = self.selected_guide {
                format!("design-guide-{}", self.guides[index].id)
            } else if let Some(category) = &self.selected_image_category {
                format!("design-assets-{category}")
            } else {
                format!("story-family-{}", self.catalog[self.selected].family)
            };
            window.on_next_frame(move |window, cx| {
                let visible = owner
                    .read_with(cx, |v, _| {
                        v.driver.snapshot(false).elements.iter().any(|e| {
                            e.id == id && e.visible && e.visible_bounds.height >= e.bounds.height
                        })
                    })
                    .unwrap_or(true);
                if !visible {
                    anchor.scroll_to(window, cx);
                    window.on_next_frame(move |_, cx| {
                        let _ = owner.update(cx, |_, cx| cx.notify());
                    });
                }
            });
        }
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
        if self.selected_guide.is_none() && self.selected_image_category.is_none() {
            self.initial_actions(window, cx);
        }
        let directory = self.directory(cx);
        let content = if let Some(index) = self.selected_guide {
            let guide = &self.guides[index];
            let id = format!("design-document-{}", guide.id);
            wb::column(0.)
                .flex_1()
                .min_w_0()
                .h_full()
                .child(wb::header(
                    guide.title.clone(),
                    Some("设计规范"),
                    20.,
                    gpui::Empty,
                ))
                .child(
                    wb::viewport(
                        "design-guide-scroll",
                        28.,
                        div()
                            .w_full()
                            .max_w(px(880.))
                            .text_size(px(14.))
                            .line_height(px(23.))
                            .child(render_document(&id, &guide.document)),
                    )
                    .overflow_scroll()
                    .automation(AutomationRole::ScrollArea, "设计规范"),
                )
                .child(ui::label(guide.source).px_5().py_2())
                .into_any_element()
        } else if let Some(category) = &self.selected_image_category {
            let cards = self
                .images
                .iter()
                .filter(|image| &image.category == category)
                .map(|image| {
                    div()
                        .id(format!(
                            "design-image-{}",
                            image.path.replace('/', "-").replace('.', "-")
                        ))
                        .w(px(154.))
                        .min_h(px(140.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .w_full()
                                .h(px(88.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(rgb(ZORK_UI.palette.canvas))
                                .child(
                                    gpui::img(image.path.clone())
                                        .size(px(72.))
                                        .object_fit(gpui::ObjectFit::Contain),
                                ),
                        )
                        .child(ui::label(image.title.clone()))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(ZORK_UI.palette.muted))
                                .child(image.status.clone()),
                        )
                        .automation(AutomationRole::Status, image.title.clone())
                })
                .collect::<Vec<_>>();
            wb::column(0.)
                .flex_1()
                .min_w_0()
                .h_full()
                .child(wb::header(
                    super::guide::category_label(category).to_owned(),
                    Some("设计素材"),
                    20.,
                    gpui::Empty,
                ))
                .child(
                    wb::viewport("design-assets-scroll", 20., wb::wrap(18.).children(cards))
                        .overflow_scroll()
                        .automation(AutomationRole::ScrollArea, "设计素材"),
                )
                .into_any_element()
        } else {
            let story = self.catalog[self.selected].clone();
            let toolbar = self.toolbar(window, cx);
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
            wb::column(0.)
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
                })
                .into_any_element()
        };
        wb::shell(
            "zork-design-pc",
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
                            .child(ui::page_title("Zork Design / PC"))
                            .child(directory),
                    )
                    .track_scroll(&self.directory_scroll)
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
