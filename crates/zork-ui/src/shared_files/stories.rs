//! Mock snapshots and event responses for the complete shared-file component.
use super::*;
use zork_client_types::shared_files::*;

pub fn create(
    state: &str,
    text: Text,
    date: std::rc::Rc<dyn Fn(i64) -> String>,
    cx: &mut gpui::App,
) -> Entity<SharedFilesView> {
    let source = Source {
        id: "demo-device".into(),
        name: "工作设备".into(),
        online: Some(true),
        cached: true,
    };
    let version = Version {
        root: "demo-version".into(),
        size: 256,
        modified_ns: 0,
        sources: vec![source.clone()],
        can_read: true,
    };
    let entries = Arc::new(vec![
        Entry {
            target: None,
            id: "readme".into(),
            path: "README.md".into(),
            name: "README.md".into(),
            kind: EntryKind::File,
            sources: vec![source.clone()],
            versions: vec![version.clone()],
        },
        Entry {
            target: None,
            id: "folder".into(),
            path: "文档".into(),
            name: "文档".into(),
            kind: EntryKind::Directory,
            sources: vec![source.clone()],
            versions: vec![],
        },
    ]);
    let preview = Preview {
        path: "README.md".into(),
        name: "README.md".into(),
        versions: vec![version],
        selected: "demo-version".into(),
        loading: false,
        error: None,
        text: Some("# 项目资料\n\n这是共享文件的固定版本预览。".into()),
        truncated: false,
        mime: "text/markdown".into(),
        cached: true,
        can_save: true,
        bytes: None,
    };
    let data = SharedFilesData {
        active: true,
        spaces: vec![Space {
            id: "demo-space".into(),
            name: "团队共享".into(),
            sources: vec![source],
        }],
        location: Some(Location {
            space: "demo-space".into(),
            path: String::new(),
        }),
        location_name: "团队共享".into(),
        entries: if matches!(state, "empty" | "loading" | "error") {
            Arc::new(vec![])
        } else {
            entries.clone()
        },
        preview: (state == "preview").then(|| preview.clone()),
        layout: if state == "grid" {
            Layout::Grid
        } else {
            Layout::List
        },
        loading: state == "loading",
        error: (state == "error").then(|| "设备暂时不可用".into()),
        empty: (state == "empty").then_some(EmptyState::Directory),
        ..Default::default()
    };
    let view = cx.new(|cx| SharedFilesView::new(Arc::new(data), text, date, cx));
    cx.subscribe(&view, move |view, event: &Action, cx| {
        view.update(cx, |view, cx| {
            let mut data = (*view.data).clone();
            match event {
                Action::OpenEntry { id } if id == "readme" => data.preview = Some(preview.clone()),
                Action::OpenEntry { id } if id == "folder" => {
                    data.location = Some(Location {
                        space: "demo-space".into(),
                        path: "文档".into(),
                    });
                    data.entries = Arc::new(vec![entries[0].clone()]);
                }
                Action::Back | Action::OpenSpace { .. } => {
                    data.location = Some(Location {
                        space: "demo-space".into(),
                        path: String::new(),
                    });
                    data.entries = entries.clone();
                    data.preview = None;
                }
                Action::ClosePreview => data.preview = None,
                Action::Layout { layout } => data.layout = *layout,
                Action::Sort { sort } => data.sort = *sort,
                Action::Source { peer } => data.source = peer.clone(),
                Action::Search { query } => {
                    data.search = query.clone();
                    data.entries = if query.is_empty() {
                        entries.clone()
                    } else {
                        Arc::new(
                            entries
                                .iter()
                                .filter(|entry| entry.name.contains(query))
                                .cloned()
                                .collect(),
                        )
                    };
                }
                Action::PrepareSave => data.save.completed = true,
                Action::Refresh => {
                    data.error = None;
                    data.loading = false;
                    data.empty = None;
                    data.entries = entries.clone();
                }
                Action::SelectVersion { root } => {
                    if let Some(p) = &mut data.preview {
                        p.selected = root.clone();
                    }
                }
                _ => {}
            }
            view.set_data(Arc::new(data), cx);
        });
    })
    .detach();
    view
}
