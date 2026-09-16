//! Explicit delivery/publication relations, committed with their visible message.
use super::*;
use anyhow::ensure;
use zork_client_types::pages::{Application, DeliveredPage, PageCatalog, PageLink};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS conversation_pages(
        id TEXT PRIMARY KEY,session_key TEXT NOT NULL REFERENCES sessions(key) ON DELETE CASCADE,
        message_id TEXT NOT NULL,page TEXT NOT NULL,source_session_id TEXT,created_at TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS conversation_pages_message ON conversation_pages(message_id);
        CREATE TABLE IF NOT EXISTS published_pages(
        id TEXT PRIMARY KEY,session_key TEXT NOT NULL REFERENCES sessions(key) ON DELETE CASCADE,
        page TEXT NOT NULL,created_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS conversation_file_refs(
        id TEXT PRIMARY KEY,session_key TEXT NOT NULL REFERENCES sessions(key) ON DELETE CASCADE,
        artifact_id TEXT NOT NULL,source_session_id TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS page_requests(
        session_key TEXT NOT NULL,request_id TEXT NOT NULL,fingerprint TEXT NOT NULL,result TEXT NOT NULL,
        PRIMARY KEY(session_key,request_id));")?;
    Ok(())
}

pub(crate) fn page_link(title: &str, url: &str, description: &str) -> Result<PageLink> {
    ensure!(
        !title.trim().is_empty() && title.len() <= 160,
        "invalid_page_title"
    );
    ensure!(!title.chars().any(char::is_control), "invalid_page_title");
    ensure!(description.len() <= 2048, "page_description_too_long");
    ensure!(url.len() <= 8192, "page_url_too_long");
    let parsed = reqwest::Url::parse(url)?;
    ensure!(
        parsed.username().is_empty() && parsed.password().is_none(),
        "page_credentials_forbidden"
    );
    match parsed.scheme() {
        "zork" => {
            zork_mesh::services::ServiceLink::parse(url)?;
        }
        "https" | "http" => {
            ensure!(parsed.host_str().is_some(), "invalid_page_url");
        }
        _ => anyhow::bail!("unsupported_page_url"),
    }
    let url = parsed.to_string();
    Ok(PageLink {
        id: format!("page-{}", blake3::hash(url.as_bytes()).to_hex()),
        title: title.trim().into(),
        url,
        description: description.trim().into(),
    })
}

pub(super) fn validate_page(page: &PageLink) -> Result<()> {
    ensure!(
        page_link(&page.title, &page.url, &page.description)? == *page,
        "invalid_page_reference"
    );
    Ok(())
}

fn recipient(conn: &Connection, key: &str) -> Result<Option<(String, String)>> {
    // Only an explicit Worker delivery is handed to its owning Leader. There is
    // no traversal of other tasks, workspace files, ports or tool dependencies.
    Ok(conn.query_row("SELECT s.key,s.id FROM worker_tasks w JOIN node_agents a ON a.id=w.leader_id JOIN sessions s ON s.id=a.session_id WHERE w.session_key=?1 AND s.id IS NOT NULL",[key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?)
}

pub(super) fn record_pages(
    conn: &Connection,
    key: &str,
    message: &str,
    pages: &[DeliveredPage],
    now: &str,
) -> Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    ensure!(pages.len() <= 16, "too_many_pages");
    let session: Option<String> =
        conn.query_row("SELECT id FROM sessions WHERE key=?1", [key], |r| r.get(0))?;
    let mut recipients = vec![(key.to_owned(), None)];
    if let Some((parent, _)) = recipient(conn, key)? {
        recipients.push((parent, session.clone()));
    }
    for delivered in pages {
        validate_page(&delivered.page)?;
        let encoded = serde_json::to_string(&delivered.page)?;
        for (target, source) in &recipients {
            let id = format!(
                "page-ref-{}",
                blake3::hash(format!("{target}\0{}", delivered.page.id).as_bytes()).to_hex()
            );
            conn.execute("INSERT INTO conversation_pages(id,session_key,message_id,page,source_session_id,created_at) VALUES(?1,?2,?3,?4,?5,?6)
                ON CONFLICT(id) DO UPDATE SET page=excluded.page,message_id=excluded.message_id,source_session_id=excluded.source_session_id
                WHERE COALESCE((SELECT sequence FROM visible_messages WHERE message_id=excluded.message_id),0)
                    >= COALESCE((SELECT sequence FROM visible_messages WHERE message_id=conversation_pages.message_id),0)
                  AND (conversation_pages.page IS NOT excluded.page OR conversation_pages.message_id IS NOT excluded.message_id OR conversation_pages.source_session_id IS NOT excluded.source_session_id)",
                params![id,target,message,encoded,source,now])?;
        }
    }
    Ok(())
}

pub(super) fn message_pages(conn: &Connection, message: &str) -> Result<Vec<DeliveredPage>> {
    let mut query = conn
        .prepare("SELECT DISTINCT page FROM conversation_pages WHERE message_id=?1 ORDER BY id")?;
    let values = query
        .query_map([message], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    values
        .into_iter()
        .map(|v| {
            Ok(DeliveredPage {
                page: serde_json::from_str(&v)?,
            })
        })
        .collect()
}

pub(super) fn record_file_handoff(conn: &Connection, message: &VisibleMessageRow) -> Result<()> {
    if message.role != "assistant" || message.kind.as_deref() != Some("file") {
        return Ok(());
    }
    let Some((parent, _)) = recipient(conn, &message.session_key)? else {
        return Ok(());
    };
    let Some((_, files)) = zork_client_types::files::decode(&message.text) else {
        return Ok(());
    };
    ensure!(
        zork_client_types::files::valid(&files),
        "invalid_attachments"
    );
    let source: String = conn.query_row(
        "SELECT id FROM sessions WHERE key=?1",
        [&message.session_key],
        |r| r.get(0),
    )?;
    for file in files {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM conversation_file_snapshots WHERE artifact_id=?1 AND session_key=?2 UNION ALL SELECT 1 FROM task_file_snapshots a JOIN product_tasks t ON t.task_id=a.task_id WHERE a.artifact_id=?1 AND t.session_key=?2)",params![file.id,message.session_key],|r|r.get(0))?;
        ensure!(exists, "attachment_not_in_conversation");
        let id = format!(
            "file-ref-{}",
            blake3::hash(format!("{parent}\0{}", file.id).as_bytes()).to_hex()
        );
        conn.execute("INSERT OR IGNORE INTO conversation_file_refs(id,session_key,artifact_id,source_session_id) VALUES(?1,?2,?3,?4)",params![id,parent,file.id,source])?;
    }
    Ok(())
}

impl GatewayDb {
    pub fn page_catalog(&self) -> Result<PageCatalog> {
        let conn = self.published_messages()?;
        let mut catalog = PageCatalog::default();
        let mut query=conn.prepare("SELECT value FROM sync_entities WHERE kind='resource' AND value IS NOT NULL AND (id LIKE 'page-ref-%' OR id LIKE 'app-%' OR id LIKE 'file-ref-%') ORDER BY id")?;
        for value in query.query_map([], |r| r.get::<_, String>(0))? {
            let value: Value = serde_json::from_str(&value?)?;
            match value["resource_type"].as_str() {
                Some("conversation_page") => {
                    catalog.references.push(serde_json::from_value(value)?)
                }
                Some("application") => catalog.applications.push(serde_json::from_value(value)?),
                Some("conversation_file") => catalog.files.push(serde_json::from_value(value)?),
                _ => {}
            }
        }
        Ok(catalog)
    }

    pub fn publish_page(
        &self,
        session: &SessionRow,
        request: &str,
        page: &PageLink,
    ) -> Result<Application> {
        validate_page(page)?;
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let fingerprint = serde_json::to_string(&("publish", page))?;
        if let Some(result) = receipt(&tx, &session.key, request, &fingerprint)? {
            return Ok(serde_json::from_str(&result)?);
        }
        let id = format!("app-{}", page.id);
        let previous: Option<(String, String, String)> = tx
            .query_row(
                "SELECT session_key,page,created_at FROM published_pages WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let encoded = serde_json::to_string(page)?;
        let now = now_rfc3339();
        let created = if let Some((owner, value, created)) = previous {
            ensure!(
                owner == session.key || value == encoded,
                "page_owner_mismatch"
            );
            tx.execute(
                "UPDATE published_pages SET page=?1 WHERE id=?2 AND page IS NOT ?1",
                params![encoded, id],
            )?;
            created
        } else {
            let count: i64 =
                tx.query_row("SELECT count(*) FROM published_pages", [], |r| r.get(0))?;
            ensure!(count < 1024, "application_limit");
            tx.execute(
                "INSERT INTO published_pages VALUES(?1,?2,?3,?4)",
                params![id, session.key, encoded, now],
            )?;
            now
        };
        let owner:String=tx.query_row("SELECT s.id FROM published_pages p JOIN sessions s ON s.key=p.session_key WHERE p.id=?1",[id],|r|r.get(0))?;
        let result = Application {
            page: page.clone(),
            owner_session_id: owner,
            created_at: created,
        };
        tx.execute(
            "INSERT INTO page_requests VALUES(?1,?2,?3,?4)",
            params![
                session.key,
                request,
                fingerprint,
                serde_json::to_string(&result)?
            ],
        )?;
        tx.commit()?;
        self.flush_messages(&conn)?;
        Ok(result)
    }

    #[cfg(test)]
    pub fn unpublish_page(&self, session: &SessionRow, request: &str, page_id: &str) -> Result<()> {
        ensure!(page_id.len() <= 128, "invalid_page_id");
        let mut guard = self.conn.lock().expect("db mutex");
        let conn = guard.transaction()?;
        let fingerprint = serde_json::to_string(&("unpublish", page_id))?;
        if receipt(&conn, &session.key, request, &fingerprint)?.is_some() {
            return Ok(());
        }
        let id = format!("app-{page_id}");
        let owner: Option<String> = conn
            .query_row(
                "SELECT session_key FROM published_pages WHERE id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(owner) = owner {
            ensure!(owner == session.key, "page_owner_mismatch");
            conn.execute("DELETE FROM published_pages WHERE id=?1", [id])?;
        }
        conn.execute(
            "INSERT INTO page_requests VALUES(?1,?2,?3,'null')",
            params![session.key, request, fingerprint],
        )?;
        conn.commit()?;
        Ok(())
    }

    #[cfg(test)]
    pub fn deliver_page(
        &self,
        session: &SessionRow,
        request: &str,
        page: &PageLink,
    ) -> Result<VisibleMessageRow> {
        ensure!(
            !request.is_empty() && request.len() <= 160,
            "invalid_page_delivery_id"
        );
        validate_page(page)?;
        let id = format!(
            "page-message-{}",
            blake3::hash(format!("{}\0{request}", session.key).as_bytes()).to_hex()
        );
        let text = page_text(page);
        let now = now_rfc3339();
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let inserted=tx.execute("INSERT INTO visible_messages(message_id,session_key,connection_id,conversation_id,root_message_id,role,text,kind,created_at) VALUES(?1,?2,?3,?4,?5,'assistant',?6,'page',?7) ON CONFLICT(message_id) DO NOTHING",params![id,session.key,session.connection_id,session.channel_id,session.root_thread_ts,text,now])?;
        let message=tx.query_row("SELECT sequence,message_id,session_key,role,text,kind,created_at FROM visible_message_content WHERE message_id=?1",[&id],map_visible_message_row)?;
        ensure!(
            message.session_key == session.key
                && message.text == text
                && message.kind.as_deref() == Some("page"),
            "page_delivery_id_conflict"
        );
        let mut topics = vec![];
        if inserted > 0 {
            mesh::validate_export_message(&tx, &session.key, "assistant", &text, Some("page"))?;
            topics = tasks::record_message(&tx, &message)?;
            record_pages(
                &tx,
                &session.key,
                &id,
                &[DeliveredPage { page: page.clone() }],
                &now,
            )?;
            tx.execute(
                "UPDATE sessions SET updated_at=?1 WHERE key=?2",
                params![now, session.key],
            )?;
        }
        tx.commit()?;
        self.flush_messages(&conn)?;
        self.chat_topics.publish(topics);
        Ok(message)
    }
}

#[cfg(test)]
pub(crate) fn page_text(page: &PageLink) -> String {
    let title = page
        .title
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    format!(
        "[{title}](<{}>){}",
        page.url,
        if page.description.is_empty() {
            String::new()
        } else {
            format!("\n\n{}", page.description)
        }
    )
}

fn receipt(conn: &Connection, owner: &str, id: &str, fingerprint: &str) -> Result<Option<String>> {
    ensure!(!id.is_empty() && id.len() <= 160, "invalid_page_request_id");
    let saved: Option<(String, String)> = conn
        .query_row(
            "SELECT fingerprint,result FROM page_requests WHERE session_key=?1 AND request_id=?2",
            params![owner, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    saved
        .map(|(stored, result)| {
            ensure!(stored == fingerprint, "page_request_id_conflict");
            Ok(result)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(db: &GatewayDb, root: &Path, name: &str) -> SessionRow {
        let session = db
            .create_session_at_workspace(
                EnsureSession {
                    connection_id: "local_gui",
                    platform: "local_gui",
                    channel_id: name,
                    root_thread_ts: name,
                    channel_type: Some("desktop"),
                    initiator_user_id: None,
                    initiator_message_ts: None,
                },
                &root.join(name),
            )
            .unwrap();
        db.set_agent_session(
            &session.key,
            name,
            &session.workspace_path,
            "fixture",
            "model",
            "off",
        )
        .unwrap();
        db.get_session(&session.key).unwrap().unwrap()
    }
    #[test]
    fn delivery_publication_and_unpublication_have_independent_durable_membership() {
        let root = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(root.path(), &root.path().join("workspaces")).unwrap();
        let owner = session(&db, root.path(), "owner");
        let other = session(&db, root.path(), "other");
        let page = page_link(
            "Project [report]",
            "https://example.test/report",
            "Ready to read",
        )
        .unwrap();
        let message = db.deliver_page(&owner, "deliver", &page).unwrap();
        assert_eq!(message.kind.as_deref(), Some("page"));
        let mut changes = db.realtime.subscribe();
        assert_eq!(
            db.deliver_page(&owner, "deliver", &page)
                .unwrap()
                .message_id,
            message.message_id
        );
        assert!(
            !changes.has_changed().unwrap(),
            "retry must be a synchronization no-op"
        );
        let catalog = db.page_catalog().unwrap();
        assert_eq!(catalog.references.len(), 1);
        assert_eq!(
            catalog.references[0].session_id,
            owner.id.as_deref().unwrap()
        );
        assert!(catalog.applications.is_empty());
        let mut revised = page.clone();
        revised.title = "Changed".into();
        assert!(db.deliver_page(&owner, "deliver", &revised).is_err());
        let app = db.publish_page(&owner, "publish", &page).unwrap();
        changes.borrow_and_update();
        assert_eq!(db.publish_page(&owner, "publish", &page).unwrap(), app);
        assert!(!changes.has_changed().unwrap());
        assert_eq!(db.page_catalog().unwrap().applications.len(), 1);
        assert!(db.unpublish_page(&other, "remove", &page.id).is_err());
        db.unpublish_page(&owner, "remove", &page.id).unwrap();
        changes.borrow_and_update();
        // A delayed retry of a successful publish cannot undo a later removal.
        db.publish_page(&owner, "publish", &page).unwrap();
        db.unpublish_page(&owner, "remove", &page.id).unwrap();
        assert!(!changes.has_changed().unwrap());
        assert!(db.page_catalog().unwrap().applications.is_empty());
        assert_eq!(db.page_catalog().unwrap().references.len(), 1);
        drop(db);
        let reopened = GatewayDb::open(root.path(), &root.path().join("workspaces")).unwrap();
        assert_eq!(reopened.page_catalog().unwrap().references.len(), 1);
        assert!(reopened.page_catalog().unwrap().applications.is_empty());
    }
    #[test]
    fn worker_handoff_targets_only_its_own_leader_and_rejects_unowned_files() {
        let root = tempfile::tempdir().unwrap();
        let db = GatewayDb::open(root.path(), &root.path().join("workspaces")).unwrap();
        let leader = session(&db, root.path(), "leader");
        let worker = session(&db, root.path(), "worker");
        let unrelated = session(&db, root.path(), "unrelated");
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO node_agents VALUES('leader','{}',?1,?2)",
                params![leader.key, leader.id],
            )
            .unwrap();
            conn.execute("INSERT INTO worker_tasks(request_id,leader_id,worker_id,session_key,session_id,goal) VALUES('job','leader','worker',?1,?2,'work')", params![worker.key,worker.id]).unwrap();
        }
        let page = page_link("Report", "https://example.test/result", "").unwrap();
        db.deliver_page(&worker, "result", &page).unwrap();
        let catalog = db.page_catalog().unwrap();
        assert_eq!(catalog.references.len(), 2);
        assert!(
            catalog
                .references
                .iter()
                .any(|r| Some(&r.session_id) == leader.id.as_ref()
                    && r.source_session_id == worker.id)
        );
        assert!(!catalog
            .references
            .iter()
            .any(|r| Some(&r.session_id) == unrelated.id.as_ref()));
        assert!(catalog.applications.is_empty());
        let bytes = b"delivered report";
        let file = zork_client_types::files::FileRef {
            id: "file-report".into(),
            name: "report.txt".into(),
            byte_len: bytes.len(),
            content_root: zork_mesh::content_root(bytes),
        };
        let text = zork_client_types::files::compose("Ready", &[file.clone()]);
        let deliver = || {
            db.record_visible_message(
                "file-message",
                &worker.key,
                "local_gui",
                "worker",
                "worker",
                "assistant",
                &text,
                Some("file"),
            )
        };
        assert!(deliver().is_err());
        assert!(db.page_catalog().unwrap().files.is_empty());
        db.receive_file_chunk(&worker, &file, 0, bytes).unwrap();
        deliver().unwrap();
        deliver().unwrap();
        let files = db.page_catalog().unwrap().files;
        assert_eq!(files.len(), 1);
        assert_eq!(Some(&files[0].session_id), leader.id.as_ref());
        assert_eq!(Some(&files[0].source_session_id), worker.id.as_ref());
    }
    #[test]
    fn page_identity_and_url_validation_do_not_admit_commands_or_credentials() {
        for url in [
            "javascript:alert(1)",
            "file:///etc/hosts",
            "https://user:secret@example.test/",
            "zork://invalid/path",
        ] {
            assert!(page_link("Page", url, "").is_err());
        }
        let page = page_link(" Page ", "https://EXAMPLE.test", " description ").unwrap();
        assert_eq!(page.url, "https://example.test/");
        assert_eq!(page.title, "Page");
        assert!(validate_page(&page).is_ok());
        let mut forged = page;
        forged.id = "another".into();
        assert!(validate_page(&forged).is_err());
    }
}
