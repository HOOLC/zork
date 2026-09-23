use super::*;
use crate::components::workbench as wb;
pub struct Story {
    menu: Entity<Menu>,
    toolbar: bool,
    list: Option<Entity<List>>,
    text: Text,
    kind: Kind,
    query: String,
    empty: bool,
    opened: String,
}
impl Story {
    pub fn new(state: &str, text: Text, cx: &mut Context<Self>) -> Self {
        let menu = cx.new(|cx| Menu::new(text.clone(), cx));
        cx.subscribe(&menu, |v, _, event: &All, cx| v.open_list(event.0, cx))
            .detach();
        let mut view = Self {
            menu,
            toolbar: state == "toolbar",
            list: None,
            text,
            kind: if state == "pages" {
                Kind::Page
            } else {
                Kind::File
            },
            query: String::new(),
            empty: state == "empty",
            opened: String::new(),
        };
        if !matches!(state, "menu" | "toolbar") {
            view.open_list(view.kind, cx);
        }
        view
    }
    fn rows(&self, kind: Kind, cx: &Context<Self>) -> Rows {
        let owner = cx.entity().downgrade();
        let names = if self.empty {
            vec![]
        } else if kind == Kind::File {
            vec!["设计说明.md", "接口文档.pdf", "界面预览.png"]
        } else {
            vec!["产品原型", "接口参考", "研究资料"]
        };
        let names: Vec<_> = names
            .into_iter()
            .filter(|name| self.query.is_empty() || name.contains(&self.query))
            .collect();
        Rows {
            count: names.len(),
            row: Rc::new(move |index, full| {
                let name = names[index].to_owned();
                let owner = owner.clone();
                let opened = name.clone();
                let prefix = match (kind, full) {
                    (Kind::File, true) => "content-file",
                    (Kind::File, false) => "conversation-artifact",
                    (Kind::Page, true) => "content-page",
                    (Kind::Page, false) => "conversation-page",
                };
                Row {
                    id: format!("{prefix}-demo-{index}"),
                    icon: if kind == Kind::File {
                        "icons/file.svg"
                    } else {
                        "browser/globe.svg"
                    },
                    title: name,
                    detail: if kind == Kind::File {
                        "12 KB · v2".into()
                    } else {
                        "来自当前会话".into()
                    },
                    open: Rc::new(move |cx| {
                        let _ = owner.update(cx, |v, cx| {
                            v.opened = opened.clone();
                            cx.notify();
                        });
                    }),
                }
            }),
        }
    }
    fn open_list(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.kind = kind;
        self.query.clear();
        let list = cx.new(|cx| List::new(kind, self.text.clone(), cx));
        cx.subscribe(&list, |v, _, query: &Query, cx| {
            v.query = query.0.clone();
            v.update_list(cx);
            cx.notify();
        })
        .detach();
        self.list = Some(list);
        self.update_list(cx);
        cx.notify();
    }
    fn update_list(&self, cx: &mut Context<Self>) {
        if let Some(list) = &self.list {
            let rows = self.rows(self.kind, cx);
            list.update(cx, |list, cx| list.set_rows(rows, self.text.clone(), cx));
        }
    }
}
impl Render for Story {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = self.rows(Kind::Page, cx);
        let files = self.rows(Kind::File, cx);
        self.menu.update(cx, |menu, cx| {
            menu.configure(pages, files, self.text.clone(), 384., cx)
        });
        wb::column(12.)
            .relative()
            .size_full()
            .child(if self.toolbar {
                crate::conversation_toolbar::render(
                    vec![crate::conversation_toolbar::Member {
                        id: "leader".into(),
                        name: "产品领队".into(),
                        avatar: Some("fox".into()),
                    }],
                    self.menu.clone(),
                    12.,
                    self.text.text("history_title"),
                    cx,
                    |v, id, cx| {
                        v.opened = format!("成员历史：{id}");
                        cx.notify();
                    },
                )
                .into_any_element()
            } else {
                self.menu.clone().into_any_element()
            })
            .when(!self.opened.is_empty(), |v| {
                v.child(wb::description(format!("打开：{}", self.opened)))
            })
            .when_some(self.list.clone(), |v, list| v.child(list))
    }
}
