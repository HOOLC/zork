//! Content membership comes from delivered messages and registered services.
use crate::api::Artifact;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
pub use zork_client_types::pages::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContentIndex {
    File(usize),
    Page(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContentKind {
    Page,
    File,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversationContents {
    pub pages: Arc<Vec<ContentIndex>>,
    pub files: Arc<Vec<ContentIndex>>,
}

pub type ContentCatalog = HashMap<Option<String>, Arc<ConversationContents>>;

impl ConversationContents {
    pub fn entries(&self, kind: ContentKind) -> &Arc<Vec<ContentIndex>> {
        match kind {
            ContentKind::Page => &self.pages,
            ContentKind::File => &self.files,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pages.is_empty() && self.files.is_empty()
    }
}

/// Search within an already scoped conversation group. Empty queries share its index.
pub fn filter_contents(
    entries: &Arc<Vec<ContentIndex>>,
    files: &[Artifact],
    pages: &PageCatalog,
    query: &str,
) -> Arc<Vec<ContentIndex>> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries.clone();
    }
    let matches = |text: &str| text.to_lowercase().contains(&query);
    Arc::new(
        entries
            .iter()
            .copied()
            .filter(|entry| match *entry {
                ContentIndex::File(i) => files.get(i).is_some_and(|f| matches(&f.name)),
                ContentIndex::Page(i) => pages.references.get(i).is_some_and(|p| {
                    matches(&p.page.title) || matches(&p.page.description) || matches(&p.page.url)
                }),
            })
            .collect(),
    )
}

/// Parse actual Markdown links once when delivered content enters the cache.
/// Code, image sources and executable/credential-bearing URLs are excluded.
pub(crate) fn message_links(
    session: &str,
    message: &crate::api::TranscriptMessage,
) -> Vec<ConversationPage> {
    use markdown::mdast::Node;
    let crate::api::TranscriptMessage::Message {
        content, metadata, ..
    } = message;
    let Some(message_id) = metadata.id.as_deref() else {
        return vec![];
    };
    if !content.contains("://") {
        return vec![];
    }
    let source = zork_client_types::files::decode(content)
        .map(|(text, _)| text)
        .unwrap_or_else(|| content.clone());
    let Ok(root) = markdown::to_mdast(&source, &markdown::ParseOptions::gfm()) else {
        return vec![];
    };
    let mut stack = vec![&root];
    let mut definitions = HashMap::new();
    let mut links = Vec::new();
    while let Some(node) = stack.pop() {
        if let Node::Definition(definition) = node {
            definitions.insert(
                definition.identifier.clone(),
                (&definition.url, definition.title.as_deref()),
            );
        }
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
    }
    let mut stack = vec![&root];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        let link = match node {
            Node::Link(link) => Some((&link.url, link.title.as_deref())),
            Node::LinkReference(link) => definitions.get(&link.identifier).copied(),
            _ => None,
        };
        if let Some((url, description)) = link {
            if let Some(url) = page_url(url) {
                if seen.insert(url.clone()) {
                    let title = node
                        .to_string()
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let id = format!("page-{}", zork_mesh::content_root(url.as_bytes()));
                    links.push(ConversationPage {
                        id: format!(
                            "markdown-{}",
                            zork_mesh::content_root(format!("{session}\0{url}").as_bytes())
                        ),
                        session_id: session.into(),
                        message_id: message_id.into(),
                        source_session_id: None,
                        created_at: metadata.created_at.clone().unwrap_or_default(),
                        page: PageLink {
                            id,
                            title: if title.is_empty() {
                                url.clone()
                            } else {
                                title.chars().take(160).collect()
                            },
                            url,
                            description: description
                                .unwrap_or_default()
                                .chars()
                                .take(2048)
                                .collect(),
                        },
                    });
                }
            }
        }
        if let Some(children) = node.children() {
            stack.extend(children.iter().rev());
        }
    }
    links
}
fn page_url(input: &str) -> Option<String> {
    let url = reqwest::Url::parse(input).ok()?;
    if input.len() > 8192 || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    match url.scheme() {
        "http" | "https" if url.host_str().is_some() => Some(url.to_string()),
        "zork" if zork_mesh::services::ServiceLink::parse(input).is_ok() => Some(url.to_string()),
        _ => None,
    }
}
pub(crate) fn merge_message_links(catalog: &mut PageCatalog, links: Vec<ConversationPage>) {
    catalog
        .references
        .retain(|page| !page.id.starts_with("markdown-"));
    let mut seen: HashSet<_> = catalog
        .references
        .iter()
        .map(|reference| (reference.session_id.clone(), reference.page.url.clone()))
        .collect();
    catalog.references.extend(
        links
            .into_iter()
            .filter(|page| seen.insert((page.session_id.clone(), page.page.url.clone()))),
    );
}

pub enum LinkAction {
    Embedded(String),
    External(String),
}
impl crate::state::Device {
    pub fn link_action(&self, session: Option<&str>, url: &str) -> LinkAction {
        let _ = session;
        if page_url(url).is_some() {
            LinkAction::Embedded(url.into())
        } else {
            LinkAction::External(url.into())
        }
    }
}

pub fn content_indices(files: &[Artifact], pages: &PageCatalog) -> ContentCatalog {
    let mut groups: HashMap<Option<String>, Vec<ContentIndex>> = HashMap::new();
    let ids: HashMap<_, _> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.artifact_id.as_str(), i))
        .collect();
    for (i, file) in files.iter().enumerate() {
        groups
            .entry(file.session_id.clone())
            .or_default()
            .push(ContentIndex::File(i));
    }
    for reference in &pages.files {
        if let Some(&i) = ids.get(reference.artifact_id.as_str()) {
            groups
                .entry(Some(reference.session_id.clone()))
                .or_default()
                .push(ContentIndex::File(i));
        }
    }
    for (i, reference) in pages.references.iter().enumerate() {
        groups
            .entry(Some(reference.session_id.clone()))
            .or_default()
            .push(ContentIndex::Page(i));
    }
    groups
        .into_iter()
        .map(|(session, mut entries)| {
            let mut seen = HashSet::new();
            entries.retain(|entry| {
                seen.insert(match entry {
                    ContentIndex::File(i) => format!("file:{}", files[*i].artifact_id),
                    ContentIndex::Page(i) => pages.references[*i].page.id.clone(),
                })
            });
            let key = |entry: &ContentIndex| match entry {
                ContentIndex::File(i) => (
                    files[*i].created_at.as_str(),
                    1,
                    files[*i].artifact_id.as_str(),
                ),
                ContentIndex::Page(i) => (
                    pages.references[*i].created_at.as_str(),
                    0,
                    pages.references[*i].page.id.as_str(),
                ),
            };
            entries.sort_by(|a, b| {
                let a = key(a);
                let b = key(b);
                b.0.cmp(a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2))
            });
            let (files, pages): (Vec<_>, Vec<_>) = entries
                .into_iter()
                .partition(|entry| matches!(entry, ContentIndex::File(_)));
            (
                session,
                Arc::new(ConversationContents {
                    files: Arc::new(files),
                    pages: Arc::new(pages),
                }),
            )
        })
        .collect()
}


#[derive(Clone, Default)]
pub(crate) struct ApplicationSource {
    pub applications: Arc<Vec<Application>>,
    pub online: Option<bool>,
    pub origin: Option<String>,
}

pub(crate) fn applications(
    nodes: &[(String, String)],
    sources: &HashMap<String, ApplicationSource>,
) -> Vec<ApplicationEntry> {
    let mut by_page = HashMap::<String, ApplicationEntry>::new();
    for (device, name) in nodes {
        let Some(source) = sources.get(device) else {
            continue;
        };
        for app in source.applications.iter() {
            let runtime = zork_mesh::services::ServiceLink::parse(&app.page.url)
                .ok()
                .and_then(|link| {
                    nodes.iter().find(|(id, _)| {
                        sources
                            .get(id)
                            .is_some_and(|s| s.origin.as_deref() == Some(link.origin.as_str()))
                    })
                });
            let offline = runtime
                .is_some_and(|(id, _)| sources.get(id).is_some_and(|s| s.online == Some(false)));
            let (runtime_id, runtime_name) = runtime
                .cloned()
                .unwrap_or_else(|| (device.clone(), name.clone()));
            let entry = ApplicationEntry {
                page: app.page.clone(),
                device_id: runtime_id,
                device_name: runtime_name,
                offline,
            };
            by_page
                .entry(app.page.id.clone())
                .and_modify(|old| {
                    if old.offline && !entry.offline {
                        *old = entry.clone();
                    }
                })
                .or_insert(entry);
        }
    }
    let mut entries = by_page.into_values().collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        a.page
            .title
            .cmp(&b.page.title)
            .then(a.page.id.cmp(&b.page.id))
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    fn page(id: &str) -> PageLink {
        PageLink {
            id: id.into(),
            title: "Report".into(),
            url: format!("https://example.test/{id}"),
            description: String::new(),
        }
    }
    #[test]
    fn only_explicit_references_enter_a_conversation() {
        let catalog = PageCatalog {
            references: vec![ConversationPage {
                id: "r".into(),
                session_id: "leader".into(),
                message_id: "m".into(),
                page: page("report"),
                source_session_id: Some("worker".into()),
                created_at: "2".into(),
            }],
            applications: vec![Application {
                page: page("other"),
                owner_session_id: "elsewhere".into(),
                created_at: "1".into(),
            }],
            ..Default::default()
        };
        let index = content_indices(&[], &catalog);
        let contents = &index[&Some("leader".into())];
        assert_eq!(*contents.pages, vec![ContentIndex::Page(0)]);
        assert!(contents.files.is_empty());
        assert_eq!(
            *filter_contents(&contents.pages, &[], &catalog, "REPORT"),
            vec![ContentIndex::Page(0)]
        );
        assert!(filter_contents(&contents.pages, &[], &catalog, "other").is_empty());
        assert!(Arc::ptr_eq(
            &contents.pages,
            &filter_contents(&contents.pages, &[], &catalog, "  ")
        ));
        assert!(!index.contains_key(&Some("worker".into())));
    }

    #[test]
    fn grouped_files_keep_versions_order_and_search_scope() {
        let file = |id: &str, session: &str, created: &str, version: i64| Artifact {
            artifact_id: id.into(),
            task_id: None,
            session_id: Some(session.into()),
            task_title: String::new(),
            workspace: String::new(),
            name: "Report.HTML".into(),
            source_path: String::new(),
            media_type: "text/html".into(),
            caption: None,
            byte_len: 10,
            version,
            created_at: created.into(),
        };
        let files = vec![
            file("v1", "chat", "1", 1),
            file("v2", "chat", "2", 2),
            file("other", "other", "3", 1),
        ];
        let catalog = PageCatalog {
            files: vec![ConversationFile {
                id: "duplicate-reference".into(),
                session_id: "chat".into(),
                artifact_id: "v1".into(),
                source_session_id: "worker".into(),
            }],
            ..Default::default()
        };
        let groups = content_indices(&files, &catalog);
        let contents = &groups[&Some("chat".into())];
        assert_eq!(
            *contents.files,
            vec![ContentIndex::File(1), ContentIndex::File(0)]
        );
        assert!(contents.pages.is_empty());
        assert_eq!(
            *filter_contents(
                contents.entries(ContentKind::File),
                &files,
                &catalog,
                "report.html"
            ),
            *contents.files
        );
        assert!(filter_contents(
            contents.entries(ContentKind::Page),
            &files,
            &catalog,
            "report"
        )
        .is_empty());
        assert!(filter_contents(&contents.files, &files, &catalog, "missing").is_empty());
    }
    #[test]
    fn application_publication_is_global_but_removed_devices_are_excluded() {
        let source = ApplicationSource {
            applications: Arc::new(vec![Application {
                page: page("report"),
                owner_session_id: "task".into(),
                created_at: "1".into(),
            }]),
            online: Some(false),
            origin: None,
        };
        let sources = HashMap::from([("a".into(), source)]);
        let entries = applications(&[("a".into(), "Device".into())], &sources);
        assert_eq!(entries.len(), 1);
        assert!(
            !entries[0].offline,
            "public HTTP page does not depend on its publisher being online"
        );
        assert!(applications(&[], &sources).is_empty());
    }
}

#[cfg(test)]
mod markdown_tests {
    use super::*;
    use crate::api::{MessageMetadata, Role, TranscriptMessage};
    fn message(content: &str) -> TranscriptMessage {
        TranscriptMessage::Message {
            role: Role::User,
            content: content.into(),
            metadata: MessageMetadata {
                id: Some("message".into()),
                ..Default::default()
            },
        }
    }
    #[test]
    fn markdown_content_index_resolves_references_and_excludes_nonlinks() {
        let body = "[Report **one**](https://example.com/report)\n<https://example.com/report>\n[Reference][report]\n\n[report]: https://example.com/other \"Details\"\n\n`https://ignored.example/code`\n\n![Image](https://ignored.example/image.png)\n[Private](https://user:password@example.com/)\n[Executable](javascript:alert(1))";
        let links = message_links("chat-a", &message(body));
        assert_eq!(links.len(), 2, "{links:?}");
        assert_eq!(links[0].page.title, "Report one");
        assert_eq!(links[1].page.description, "Details");
        assert_eq!(links[0].message_id, "message");
        assert_ne!(message_links("chat-b", &message(body))[0].id, links[0].id);
    }
    #[test]
    fn cached_links_follow_replay_pagination_and_source_replacement() {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(crate::store::ClientStore::open(root.path()).unwrap());
        let device = crate::state::Device::open(
            Arc::new(crate::api::StationClient::new("http://127.0.0.1:9", None)),
            Some((store.clone(), "node".into())),
            false,
        );
        let page = crate::api::MessagePage {
            source_epoch: Some("epoch-a".into()),
            items: vec![message("[Report](https://example.com/report)")],
            older_cursor: Some("older".into()),
        };
        store
            .cache_message_page("node", "chat", &page, None)
            .unwrap();
        assert_eq!(device.snapshot().pages.references.len(), 1);
        let original = device.snapshot().pages.clone();
        store
            .cache_message_page("node", "chat", &page, None)
            .unwrap();
        assert!(Arc::ptr_eq(&original, &device.snapshot().pages));
        assert_eq!(store.message_links("node").unwrap().len(), 1);
        let mut old_message = message("[Earlier](https://example.com/older)");
        let TranscriptMessage::Message { metadata, .. } = &mut old_message;
        metadata.id = Some("old-message".into());
        let older = crate::api::MessagePage {
            source_epoch: Some("epoch-a".into()),
            items: vec![old_message],
            older_cursor: None,
        };
        store
            .cache_message_page("node", "chat", &older, Some("older"))
            .unwrap();
        assert_eq!(device.snapshot().pages.references.len(), 2);
        let cached = device.snapshot().pages.clone();
        store.cached_messages("node", "chat", None, 100).unwrap();
        assert!(Arc::ptr_eq(&cached, &device.snapshot().pages));
        let replacement = crate::api::MessagePage {
            source_epoch: Some("epoch-b".into()),
            items: page.items.clone(),
            older_cursor: None,
        };
        store
            .cache_message_page("node", "chat", &replacement, None)
            .unwrap();
        assert_eq!(device.snapshot().pages.references.len(), 1);
        assert_eq!(
            device.snapshot().pages.references[0].page.url,
            "https://example.com/report"
        );
        let empty = crate::api::MessagePage {
            source_epoch: Some("epoch-c".into()),
            items: vec![],
            older_cursor: None,
        };
        store
            .cache_message_page("node", "chat", &empty, None)
            .unwrap();
        assert!(device.snapshot().pages.references.is_empty());
        assert!(store.message_links("node").unwrap().is_empty());
    }
}
