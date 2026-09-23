//! Complete shared-file browser. Hosts supply core snapshots, images and intents.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{
        message::{render_document, MessageDocument},
        text_input::{ComposerInput, ComposerSubmit},
    },
    controls as ui,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb, Context, Div, Entity, Render, Window};
use std::sync::Arc;
use zork_client_types::shared_files::{
    Action, EmptyState, EntryKind, Layout, SharedFilesData, Sort,
};
impl gpui::EventEmitter<Action> for SharedFilesView {}
pub struct SharedFilesView {
    data: Arc<SharedFilesData>,
    locale: Text,
    date: std::rc::Rc<dyn Fn(i64) -> String>,
    search: Entity<ComposerInput>,
    search_open: bool,
    details_open: bool,
    more_menu: crate::components::standard_menu::Menu,
    version_menu: crate::components::standard_menu::Menu,
    scroll: gpui::UniformListScrollHandle,
    document: Option<(String, MessageDocument)>,
    image: Option<(String, Arc<gpui::RenderImage>)>,
    available_width: Option<f32>,
    _input: gpui::Subscription,
}
impl SharedFilesView {
    pub fn new(
        data: Arc<SharedFilesData>,
        locale: Text,
        date: std::rc::Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search =
            cx.new(|cx| ComposerInput::new(locale.text("shared_search"), cx).single_line());
        let input = cx.subscribe(&search, |v, input, _: &ComposerSubmit, cx| {
            v.run(
                Action::Search {
                    query: input.read(cx).value().into(),
                },
                cx,
            );
        });
        let mut view = Self {
            data,
            locale,
            date,
            search,
            search_open: false,
            details_open: false,
            more_menu: Default::default(),
            version_menu: Default::default(),
            scroll: Default::default(),
            document: None,
            image: None,
            available_width: None,
            _input: input,
        };
        view.project_preview();
        view
    }
    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self
            .available_width
            .is_none_or(|old| (old - width).abs() > 0.5)
        {
            self.available_width = Some(width);
            cx.notify();
        }
    }
    pub fn set_text(&mut self, text: Text, cx: &mut Context<Self>) {
        self.locale = text;
        cx.notify();
    }
    fn run(&self, action: Action, cx: &mut Context<Self>) {
        cx.emit(action);
    }
    pub fn set_data(&mut self, data: Arc<SharedFilesData>, cx: &mut Context<Self>) {
        let changed_location = self.data.location != data.location;
        self.data = data;
        if changed_location {
            self.scroll = Default::default();
            self.search.update(cx, |input, cx| {
                input.set_value(self.data.search.clone(), cx)
            });
        }
        self.project_preview();
        cx.notify();
    }
    pub fn set_image(
        &mut self,
        image: Option<(String, Arc<gpui::RenderImage>)>,
        cx: &mut Context<Self>,
    ) {
        self.image = image;
        cx.notify();
    }
    fn project_preview(&mut self) {
        let source = self
            .data
            .preview
            .as_ref()
            .and_then(|preview| preview.text.as_ref().map(|text| (preview, text)));
        if let Some((preview, text)) = source {
            if self
                .document
                .as_ref()
                .is_none_or(|(root, _)| root != &preview.selected)
            {
                let document = if preview.name.ends_with(".md") {
                    MessageDocument::parse(text)
                } else {
                    MessageDocument::plain(text)
                };
                self.document = Some((preview.selected.clone(), document));
            }
        } else {
            self.document = None;
        }
    }
    fn icon(
        &self,
        id: &'static str,
        path: &'static str,
        label: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> gpui::AnyElement {
        ui::icon_button(id, true)
            .child(ui::icon(path, 18.))
            .on_click(cx.listener(move |v, _, _, cx| v.run(action.clone(), cx)))
            .automation(AutomationRole::Button, self.locale.text(label))
            .into_any_element()
    }
    fn row(&self, index: usize, grid: bool, cx: &Context<Self>) -> Div {
        let p = crate::design::ZORK_UI.palette;
        let (id, name, folder, versions, offline, action) = if self.data.location.is_none() {
            let space = &self.data.spaces[index];
            (
                space.id.clone(),
                space.name.clone(),
                true,
                0,
                space.sources.iter().all(|s| s.online == Some(false)),
                Action::OpenSpace {
                    space: space.id.clone(),
                },
            )
        } else {
            let entry = &self.data.entries[index];
            (
                entry.id.clone(),
                entry.name.clone(),
                entry.kind == EntryKind::Directory,
                entry.versions.len(),
                entry.sources.iter().all(|s| s.online == Some(false)),
                Action::OpenEntry {
                    id: entry.id.clone(),
                },
            )
        };
        let icon = if folder {
            "icons/phosphor-folder-simple.svg"
        } else {
            "icons/file.svg"
        };
        let selected = self
            .data
            .preview
            .as_ref()
            .is_some_and(|preview| format!("file:{}", preview.path) == id);
        let line = ui::quiet_button(
            format!("shared-row-{id}"),
            "",
            true,
            ui::IconButtonSize::Standard,
        )
        .w_full()
        .font_weight(gpui::FontWeight::NORMAL)
        .h(px(if grid { 136. } else { 64. }))
        .radius(12.)
        .px(px(12.))
        .justify_start()
        .gap(px(14.))
        .selected(selected)
        .when(grid, |v| {
            v.flex_col().items_start().justify_center().gap(px(10.))
        })
        .child(ui::icon(icon, if grid { 32. } else { 24. }))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .when(grid, |v| v.flex_none().w_full())
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .child(name.clone()),
                ),
        )
        .when(versions > 1 || offline, |v| {
            v.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(rgb(p.muted))
                    .child(if versions > 1 {
                        format!("{versions} {}", self.locale.text("shared_versions"))
                    } else {
                        self.locale.text("shared_offline").into()
                    }),
            )
        })
        .on_click(cx.listener(move |v, _, _, cx| {
            v.more_menu.dismiss();
            v.version_menu.dismiss();
            v.run(action.clone(), cx);
        }))
        .automation(AutomationRole::Button, name);
        div().w_full().px(px(16.)).py(px(2.)).child(line)
    }
    fn preview(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let p = crate::design::ZORK_UI.palette;
        let Some(preview) = &self.data.preview else {
            return div();
        };
        let version = preview.versions.iter().find(|v| v.root == preview.selected);
        let version_focus =
            crate::components::widgets::controls::action_focus("shared-version", window, cx);
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.canvas))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().truncate().child(preview.name.clone()))
                    .child(self.icon(
                        "shared-close-preview",
                        "icons/x.svg",
                        "close",
                        Action::ClosePreview,
                        cx,
                    )),
            )
            .child(
                div()
                    .px(px(24.))
                    .py(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        self.version_menu
                            .trigger_element(
                                ui::quiet_button(
                                    "shared-version",
                                    version.map(|v| size(v.size)).unwrap_or_default(),
                                    true,
                                    ui::IconButtonSize::Standard,
                                )
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(rgb(p.muted))
                                .self_start()
                                .justify_start(),
                                &version_focus,
                                true,
                                cx,
                            )
                            .automation(
                                AutomationRole::Button,
                                self.locale.text("shared_versions"),
                            ),
                    )
                    .child(
                        ui::quiet_button(
                            "shared-details",
                            self.locale.text("shared_details"),
                            true,
                            ui::IconButtonSize::Standard,
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.details_open = !v.details_open;
                            cx.notify();
                        })),
                    )
                    .when(self.details_open, |v| {
                        v.child(description(
                            version
                                .map(|version| {
                                    version
                                        .sources
                                        .iter()
                                        .map(|source| {
                                            crate::device_name::summary(
                                                &source.name,
                                                &source.status,
                                                Some(&self.locale),
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(" · ")
                                })
                                .unwrap_or_default(),
                        ))
                    })
                    .when(preview.cached, |v| {
                        v.child(description(self.locale.text("shared_cached")))
                    }),
            )
            .child(
                div()
                    .id("shared-preview-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(24.))
                    .pb(px(24.))
                    .when(preview.loading, |v| {
                        v.child(description(self.locale.text("loading")))
                    })
                    .when_some(preview.error.clone(), |v, e| v.child(ui::feedback(e)))
                    .when_some(self.document.as_ref(), |v, (_, document)| {
                        v.child(render_document("shared-file-document", document))
                    })
                    .when_some(self.image.as_ref(), |v, (_, image)| {
                        v.child(
                            gpui::img(image.clone())
                                .w_full()
                                .object_fit(gpui::ObjectFit::Contain),
                        )
                    })
                    .when(
                        !preview.loading
                            && preview.error.is_none()
                            && self.document.is_none()
                            && self.image.is_none(),
                        |v| v.child(description(self.locale.text("shared_no_preview"))),
                    )
                    .when(preview.truncated, |v| {
                        v.child(description(self.locale.text("shared_truncated")))
                    }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(
                        ui::busy_button(
                            "shared-save",
                            self.locale.text("shared_save"),
                            false,
                            preview.can_save && !self.data.save.busy,
                            self.data.save.busy,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.run(Action::PrepareSave, cx)))
                        .automation(AutomationRole::Button, self.locale.text("shared_save")),
                    )
                    .when_some(self.data.save.error.clone(), |v, e| {
                        v.child(ui::feedback(e))
                    })
                    .when(self.data.save.completed, |v| {
                        v.child(description(self.locale.text("shared_saved")))
                    }),
            )
    }
    fn menus(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        use crate::components::standard_menu::Item;
        let actions = std::collections::HashMap::from([
            ("shared-refresh".to_owned(), Action::Refresh),
            (
                "shared-layout".to_owned(),
                Action::Layout {
                    layout: if self.data.layout == Layout::List {
                        Layout::Grid
                    } else {
                        Layout::List
                    },
                },
            ),
            (
                "shared-sort".to_owned(),
                Action::Sort {
                    sort: if self.data.sort == Sort::Name {
                        Sort::NameDescending
                    } else {
                        Sort::Name
                    },
                },
            ),
            (
                "shared-source-all".to_owned(),
                Action::Source { peer: None },
            ),
        ])
        .into_iter()
        .chain(self.data.devices.iter().map(|device| {
            (
                format!("shared-source-{}", device.id),
                Action::Source {
                    peer: Some(device.id.clone()),
                },
            )
        }))
        .collect::<std::collections::HashMap<_, _>>();
        let mut sources = vec![Item::new(
            "shared-source-all",
            self.locale.text("shared_all_sources"),
        )];
        sources.extend(self.data.devices.iter().map(|device| {
            Item::new(
                format!("shared-source-{}", device.id),
                crate::device_name::summary(&device.name, &device.status, Some(&self.locale)),
            )
        }));
        let items = vec![
            Item::new("shared-refresh", self.locale.text("refresh")).icon("interface/reload.svg"),
            Item::new(
                "shared-layout",
                self.locale.text(if self.data.layout == Layout::List {
                    "shared_grid"
                } else {
                    "shared_list"
                }),
            ),
            Item::new(
                "shared-sort",
                self.locale.text(if self.data.sort == Sort::Name {
                    "shared_descending"
                } else {
                    "shared_ascending"
                }),
            ),
            Item::new("shared-sources", self.locale.text("shared_sources")).submenu(sources),
        ];
        let more =
            self.more_menu
                .render("shared-menu", items, window, cx, move |view, key, _, cx| {
                    if let Some(action) = actions.get(&key) {
                        view.run(action.clone(), cx);
                    }
                });
        let versions = self.data.preview.as_ref().map_or_else(Vec::new, |preview| {
            preview
                .versions
                .iter()
                .map(|version| {
                    let date = (self.date)(version.modified_ns);
                    Item::new(
                        version.root.clone(),
                        format!(
                            "{} · {}",
                            version
                                .sources
                                .iter()
                                .map(|s| s.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" · "),
                            size(version.size)
                        ),
                    )
                    .detail(date)
                    .radio(version.root == preview.selected)
                })
                .collect()
        });
        let versions = self.version_menu.render(
            "shared-versions",
            versions,
            window,
            cx,
            |view, root, _, cx| {
                view.run(Action::SelectVersion { root }, cx);
            },
        );
        div().children(more).children(versions)
    }
}

impl Render for SharedFilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let more_focus =
            crate::components::widgets::controls::action_focus("shared-more", window, cx);
        let p = crate::design::ZORK_UI.palette;
        let available = self
            .available_width
            .unwrap_or_else(|| window.viewport_size().width.as_f32());
        let compact = available < 760.;
        let showing_preview = self.data.preview.is_some();
        let count = if self.data.location.is_some() {
            self.data.entries.len()
        } else {
            self.data.spaces.len()
        };
        let columns = if self.data.layout == Layout::Grid {
            ((available
                - if showing_preview && !compact {
                    360.
                } else {
                    0.
                }
                - 32.)
                / 160.)
                .floor()
                .clamp(1., 5.) as usize
        } else {
            1
        };
        let title = self
            .data
            .location
            .as_ref()
            .map(|l| {
                if l.path.is_empty() {
                    self.locale.text("shared_files").into()
                } else {
                    l.path.clone()
                }
            })
            .unwrap_or_else(|| self.locale.text("shared_files").into());
        let body = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgb(p.canvas))
            .on_key_down(cx.listener(|v, e: &gpui::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    if v.more_menu.is_open() || v.version_menu.is_open() {
                        v.more_menu.dismiss();
                        v.version_menu.dismiss();
                    } else {
                        v.run(Action::ClosePreview, cx);
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .h(px(44.))
                    .flex_shrink_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when(self.data.location.is_some(), |v| {
                        v.child(self.icon(
                            "shared-back",
                            "icons/arrow-left.svg",
                            "back",
                            Action::Back,
                            cx,
                        ))
                    })
                    .when(!self.search_open, |v| {
                        v.child(div().flex_1().min_w_0().truncate().child(title))
                    })
                    .when(self.search_open, |v| {
                        v.child(
                            ui::input_control("shared-search-field", &self.search, false, cx)
                                .flex_1()
                                .min_w_0(),
                        )
                    })
                    .child(
                        ui::icon_button("shared-search", true)
                            .child(ui::icon("icons/search.svg", 18.))
                            .on_click(cx.listener(|v, _, window, cx| {
                                v.search_open = !v.search_open;
                                if v.search_open {
                                    window.focus(&v.search.read(cx).focus_handle(), cx);
                                } else {
                                    v.search.update(cx, |i, cx| i.set_value("", cx));
                                    v.run(
                                        Action::Search {
                                            query: String::new(),
                                        },
                                        cx,
                                    );
                                }
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, self.locale.text("shared_search")),
                    )
                    .child(
                        self.more_menu.trigger_element(ui::icon_button("shared-more", true)
                            .child(ui::icon("browser/more.svg", 18.)), &more_focus, true, cx)
                            .automation(AutomationRole::Button, self.locale.text("more")),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .when(!compact || !showing_preview, |v| {
                        v.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .flex()
                                .flex_col()
                                .when(self.data.offline, |v| {
                                    v.child(div().px(px(28.)).py(px(8.)).child(description(
                                        self.locale.text("shared_offline_hint"),
                                    )))
                                })
                                .when_some(self.data.error.clone(), |v, e| {
                                    v.child(div().px(px(28.)).child(ui::feedback(e)))
                                })
                                .when(count == 0, |v| {
                                    v.child(
                                        div()
                                            .flex_1()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .p(px(32.))
                                            .text_color(rgb(p.muted))
                                            .child(self.locale.text(if self.data.loading {
                                                "loading"
                                            } else {
                                                match self.data.empty {
                                                    Some(EmptyState::NoSpaces) => "shared_no_spaces",
                                                    Some(EmptyState::NoResults) => "shared_no_results",
                                                    Some(EmptyState::Unavailable) => "shared_unavailable",
                                                    _ => "shared_empty",
                                                }
                                            })),
                                    )
                                })
                                .when(count > 0, |v| {
                                    v.child(
                                        gpui::uniform_list(
                                            "shared-file-list",
                                            count.div_ceil(columns),
                                            cx.processor(move |view: &mut Self, range: std::ops::Range<usize>, _, cx| {
                                                range
                                                    .map(|row| {
                                                        div().w_full().flex().children(
                                                            (row * columns
                                                                ..((row + 1) * columns).min(
                                                                    if view.data.location.is_some()
                                                                    {
                                                                        view.data.entries.len()
                                                                    } else {
                                                                        view.data.spaces.len()
                                                                    },
                                                                ))
                                                                .map(|index| {
                                                                    div().flex_1().min_w_0().child(
                                                                        view.row(
                                                                            index,
                                                                            columns > 1,
                                                                            cx,
                                                                        ),
                                                                    )
                                                                }),
                                                        )
                                                    })
                                                    .collect()
                                            }),
                                        )
                                        .track_scroll(&self.scroll)
                                        .flex_1()
                                        .min_h_0(),
                                    )
                                })
                                .when(self.data.more, |v| {
                                    v.child(
                                        div().p(px(12.)).flex().justify_center().child(
                                            ui::busy_button(
                                                "shared-load-more",
                                                self.locale.text("shared_load_more"),
                                                false,
                                                !self.data.loading,
                                                self.data.loading,
                                            )
                                            .on_click(
                                                cx.listener(|v, _, _, cx| v.run(Action::More, cx)),
                                            ),
                                        ),
                                    )
                                }),
                        )
                    })
                    .when(showing_preview, |v| {
                        v.child(
                            div()
                                .h_full()
                                .min_w_0()
                                .when(compact, |v| v.flex_1())
                                .when(!compact, |v| {
                                    v.w(px(360.))
                                        .flex_shrink_0()
                                        .border_l(gpui::px(crate::design::BORDER_WIDTH))
                                        .border_color(rgb(p.border))
                                })
                                .child(self.preview(window, cx)),
                        )
                    }),
            )
            .child(self.menus(window, cx));

        body
    }
}
fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024. * 1024.))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.)
    } else {
        format!("{bytes} B")
    }
}

fn description(text: impl Into<gpui::SharedString>) -> Div {
    ui::text_role(text, crate::design::TextRole::Description)
}

#[cfg(feature = "stories")]
pub mod stories;
