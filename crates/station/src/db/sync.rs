//! Durable public projections. Trigger writes share the business transaction;
//! transport notifications are only hints. Export rows freeze a committed cut.
use super::*;
use anyhow::ensure;
use zork_client_types::sync::{
    Cursor, Kind, Page, Pull, Record, Reply, Scope, MAX_PAGE_BYTES, MAX_PAGE_RECORDS, PROTOCOL,
};

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    conn.execute_batch("SAVEPOINT sync_schema;
        CREATE TABLE IF NOT EXISTS sync_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),epoch TEXT NOT NULL,sequence INTEGER NOT NULL DEFAULT 0,floor INTEGER NOT NULL DEFAULT 0);
        INSERT OR IGNORE INTO sync_meta(singleton,epoch) VALUES(1,lower(hex(randomblob(16))));
        CREATE TABLE IF NOT EXISTS sync_change_signal(singleton INTEGER PRIMARY KEY CHECK(singleton=1),generation INTEGER NOT NULL DEFAULT 0);
        INSERT OR IGNORE INTO sync_change_signal(singleton) VALUES(1);
        CREATE TRIGGER IF NOT EXISTS sync_version_signal AFTER UPDATE OF sequence,epoch ON sync_meta
        WHEN OLD.sequence IS NOT NEW.sequence OR OLD.epoch IS NOT NEW.epoch BEGIN
            UPDATE sync_change_signal SET generation=generation+1 WHERE singleton=1;
        END;
        CREATE TABLE IF NOT EXISTS sync_identity(singleton INTEGER PRIMARY KEY CHECK(singleton=1),owner TEXT NOT NULL);
        INSERT OR IGNORE INTO sync_identity VALUES(1,'local:'||lower(hex(randomblob(16))));
        CREATE TABLE IF NOT EXISTS sync_entities(scope TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,value TEXT,revision INTEGER NOT NULL DEFAULT 0,PRIMARY KEY(scope,kind,id));
        CREATE INDEX IF NOT EXISTS sync_entities_revision ON sync_entities(scope,revision);
        CREATE INDEX IF NOT EXISTS sync_tombstone_revision ON sync_entities(revision) WHERE value IS NULL;
        CREATE TRIGGER IF NOT EXISTS sync_entity_insert AFTER INSERT ON sync_entities BEGIN
            UPDATE sync_meta SET sequence=sequence+1 WHERE singleton=1;
            UPDATE sync_entities SET revision=(SELECT sequence FROM sync_meta) WHERE scope=NEW.scope AND kind=NEW.kind AND id=NEW.id;
        END;
        CREATE TRIGGER IF NOT EXISTS sync_entity_update AFTER UPDATE OF value ON sync_entities WHEN OLD.value IS NOT NEW.value BEGIN
            UPDATE sync_meta SET sequence=sequence+1 WHERE singleton=1;
            UPDATE sync_entities SET revision=(SELECT sequence FROM sync_meta) WHERE scope=NEW.scope AND kind=NEW.kind AND id=NEW.id;
        END;
        CREATE TABLE IF NOT EXISTS sync_exports(id TEXT PRIMARY KEY,owner TEXT NOT NULL,scope TEXT NOT NULL,header TEXT NOT NULL,created INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS sync_export_rows(batch TEXT NOT NULL REFERENCES sync_exports(id) ON DELETE CASCADE,position INTEGER NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT,PRIMARY KEY(batch,position));
        CREATE TABLE IF NOT EXISTS sync_export_pages(batch TEXT NOT NULL REFERENCES sync_exports(id) ON DELETE CASCADE,page_index INTEGER NOT NULL,start_position INTEGER NOT NULL,PRIMARY KEY(batch,page_index));
    ")?;
    // Projection migrations rebuild derived rows, never business data, local
    // identities or receipts. A new epoch invalidates old frozen exports/cursors.
    const PROJECTION_SCHEMA: i64 = 2;
    conn.execute_batch("CREATE TABLE IF NOT EXISTS sync_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL);")?;
    let version: Option<i64> = conn
        .query_row(
            "SELECT version FROM sync_schema WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if version != Some(PROJECTION_SCHEMA) {
        for table in [
            "node_agents",
            "sessions",
            "visible_messages",
            "product_tasks",
            "task_file_snapshots",
            "conversation_file_snapshots",
        ] {
            for event in ["insert", "update", "delete"] {
                conn.execute_batch(&format!("DROP TRIGGER IF EXISTS sync_{table}_{event}"))?;
            }
        }
        for table in ["task_runs", "mesh_links", "worker_tasks", "sessions"] {
            for event in ["INSERT", "UPDATE", "DELETE"] {
                conn.execute_batch(&format!("DROP TRIGGER IF EXISTS sync_task_{table}_{event}"))?;
            }
        }
        for name in [
            "sync_marker_INSERT",
            "sync_marker_UPDATE",
            "sync_marker_DELETE",
            "sync_marker_session_delete",
            "sync_marker_worker_insert",
            "sync_marker_worker_delete",
        ] {
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS {name}"))?;
        }
        conn.execute_batch("DROP VIEW IF EXISTS sync_marker_view; DELETE FROM sync_entities WHERE kind IN ('agent','session','task','artifact','message','read_marker'); DELETE FROM sync_exports; UPDATE sync_meta SET epoch=lower(hex(randomblob(16)));")?;
        conn.execute("INSERT INTO sync_schema VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET version=excluded.version",[PROJECTION_SCHEMA])?;
    }
    // All exported fields are explicit public DTO fields. Never copy arbitrary
    // configuration JSON: it can grow credential fields in later versions.
    let catalog = "'{\"type\":\"catalog\"}'";
    for (table, marker) in [
        ("node_agents", "chat_agent_home"),
        ("sessions", "agent_control"),
        ("visible_messages", "chat_message_facts"),
    ] {
        let sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [format!("sync_{table}_insert")],
                |r| r.get(0),
            )
            .optional()?;
        if sql.is_some_and(|sql| !sql.contains(marker)) {
            conn.execute_batch(&format!("DROP TRIGGER IF EXISTS sync_{table}_insert; DROP TRIGGER IF EXISTS sync_{table}_update; DROP TRIGGER IF EXISTS sync_{table}_delete;"))?;
        }
    }
    conn.execute("UPDATE sync_entities SET value=NULL WHERE kind='session' AND value IS NOT NULL AND id IN(SELECT id FROM sessions WHERE channel_type='agent_control')", [])?;

    install_projection(conn, "node_agents", "id", "agent", catalog, "1", "json_object('id',NEW.id,'name',json_extract(NEW.value,'$.name'),'avatar',json_extract(NEW.value,'$.avatar'),'role',json_extract(NEW.value,'$.role'),'profile_id',json_extract(NEW.value,'$.profile_id'),'model',json_extract(NEW.value,'$.model'),'thinking',json_extract(NEW.value,'$.thinking'),'instructions',json_extract(NEW.value,'$.instructions'),'allowed_leaders',json(COALESCE(json_extract(NEW.value,'$.allowed_leaders'),'[]')),'session_id',COALESCE((SELECT chat_id FROM chat_agent_home WHERE agent_id=NEW.id),(SELECT chat_id FROM chat_channels WHERE session_key=NEW.session_key)))")?;
    install_projection(conn, "sessions", "id", "session", catalog, "NEW.platform='local_gui' AND NEW.id IS NOT NULL AND COALESCE(NEW.channel_type,'')!='agent_control'", "json_object('session_id',NEW.id,'kind',COALESCE(NEW.channel_type,'desktop'),'title',NEW.channel_name,'profile_id',COALESCE(NEW.profile_id,''),'model',COALESCE(NEW.model,''),'thinking',COALESCE(NEW.thinking,''),'workspace',NEW.workspace_path,'created_at',NEW.created_at,'updated_at',NEW.updated_at)")?;
    // The conversation scope is keyed by the public session id, not session_key.
    install_projection(conn, "visible_messages", "message_id", "message", "json_object('type','conversation','id',(SELECT id FROM sessions WHERE key=NEW.session_key))", "EXISTS(SELECT 1 FROM sessions WHERE key=NEW.session_key AND platform='local_gui' AND id IS NOT NULL)", "json_object('message_id',NEW.message_id,'sequence',NEW.sequence,'role',CASE WHEN NEW.role='user' AND (NEW.message_id GLOB 'assignment-*' OR NEW.message_id GLOB 'rework-*') AND EXISTS(SELECT 1 FROM worker_tasks WHERE session_key=NEW.session_key) THEN 'assistant' ELSE NEW.role END,'text','','kind',NEW.kind,'created_at',NEW.created_at,'author',json((SELECT author FROM chat_message_facts WHERE message_id=NEW.message_id)),'author_agent_id',(SELECT CASE WHEN json_extract(author,'$.kind')='agent' THEN author_id END FROM chat_message_facts WHERE message_id=NEW.message_id),'author_name',(SELECT json_extract(author,'$.name') FROM chat_message_facts WHERE message_id=NEW.message_id),'mentions',json((SELECT mentions FROM chat_message_facts WHERE message_id=NEW.message_id)),'reply_to',(SELECT reply_to FROM chat_message_facts WHERE message_id=NEW.message_id))")?;
    // Update only the task projection formula; its public shape and cursor
    // domain are unchanged. Do not rebuild message projections for this change.
    let task_trigger: Option<String> = conn.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='sync_product_tasks_insert'", [], |row| row.get(0)).optional()?;
    if task_trigger.is_some_and(|sql| !sql.contains("c.run_count")) {
        conn.execute_batch("DROP TRIGGER IF EXISTS sync_product_tasks_insert; DROP TRIGGER IF EXISTS sync_product_tasks_update; DROP TRIGGER IF EXISTS sync_product_tasks_delete;")?;
    }
    install_projection(conn, "product_tasks", "task_id", "task", catalog, "EXISTS(SELECT 1 FROM sessions WHERE key=NEW.session_key AND platform='local_gui')", "json_object('task_id',NEW.task_id,'session_id',(SELECT id FROM sessions WHERE key=NEW.session_key),'conversation_id',(SELECT channel_id FROM sessions WHERE key=NEW.session_key),'workspace',(SELECT workspace_path FROM sessions WHERE key=NEW.session_key),'leader_id',(SELECT leader_id FROM worker_tasks WHERE session_key=NEW.session_key),'title',NEW.title,'goal','','state',NEW.state,'revision',NEW.revision,'result_message_id',(SELECT message_id FROM visible_messages WHERE sequence=NEW.result_sequence),'result_text',NULL,'last_run_status',COALESCE((SELECT remote_status FROM mesh_links WHERE local_task_id=NEW.task_id AND role='owner'),(SELECT status FROM task_runs WHERE task_id=NEW.task_id AND agent_session_id=(SELECT id FROM sessions WHERE key=NEW.session_key) ORDER BY rowid DESC LIMIT 1)),'run_count',(SELECT COUNT(*) FROM task_runs WHERE task_id=NEW.task_id)+COALESCE((SELECT SUM(MAX(0,c.run_count-(SELECT COUNT(*) FROM task_runs r WHERE r.task_id=c.task_id AND r.agent_session_id=c.agent_session_id))) FROM task_event_cursors c WHERE c.task_id=NEW.task_id AND c.run_count IS NOT NULL),0),'created_at',NEW.created_at,'updated_at',NEW.updated_at,'mesh',json((SELECT json_object('role',role,'assignment_id',assignment_id,'state',state,'error',error,'owner_origin',json_extract(assignment_json,'$.owner_origin'),'executor_origin',json_extract(assignment_json,'$.executor_origin')) FROM mesh_links WHERE local_task_id=NEW.task_id)))")?;
    for (table, key, target) in [
        ("task_runs", "task_id", "task_id"),
        ("mesh_links", "local_task_id", "task_id"),
        ("worker_tasks", "session_key", "session_key"),
        ("sessions", "key", "session_key"),
    ] {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            let source = if event == "DELETE" { "OLD" } else { "NEW" };
            conn.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS sync_task_{table}_{event} AFTER {event} ON {table} BEGIN UPDATE product_tasks SET title=title WHERE {target}={source}.{key}; END;"))?;
        }
    }
    conn.execute_batch("CREATE TRIGGER IF NOT EXISTS sync_task_run_count_insert AFTER INSERT ON task_event_cursors WHEN NEW.run_count IS NOT NULL BEGIN UPDATE product_tasks SET title=title WHERE task_id=NEW.task_id; END;
        CREATE TRIGGER IF NOT EXISTS sync_task_run_count_update AFTER UPDATE OF run_count ON task_event_cursors WHEN OLD.run_count IS NOT NEW.run_count BEGIN UPDATE product_tasks SET title=title WHERE task_id=NEW.task_id; END;
        CREATE TRIGGER IF NOT EXISTS sync_task_run_count_delete AFTER DELETE ON task_event_cursors WHEN OLD.run_count IS NOT NULL BEGIN UPDATE product_tasks SET title=title WHERE task_id=OLD.task_id; END;")?;
    for table in ["task_file_snapshots", "conversation_file_snapshots"] {
        let task = if table == "task_file_snapshots" {
            "NEW.task_id"
        } else {
            "NULL"
        };
        let session = if table == "task_file_snapshots" {
            "(SELECT session_key FROM product_tasks WHERE task_id=NEW.task_id)"
        } else {
            "NEW.session_key"
        };
        install_projection(conn,table,"artifact_id","artifact",catalog,"1",&format!("json_object('artifact_id',NEW.artifact_id,'task_id',{task},'session_id',(SELECT id FROM sessions WHERE key={session}),'task_title','Conversation','name',NEW.name,'source_path',NEW.source_path,'workspace',NEW.workspace,'media_type',NEW.media_type,'caption',NEW.caption,'byte_len',json_extract(NEW.snapshot,'$.byte_len'),'version',NEW.version,'created_at',NEW.created_at)"))?;
    }
    install_markers(conn)?;
    install_projection(conn, "chat_channels", "chat_id", "resource", catalog, "1",
        "json_object('resource_type','chat_summary','schema_version',1,'chat_id',NEW.chat_id,'title',NEW.title,'creator',json(NEW.creator),'created_at',NEW.created_at,'last_message_at',NEW.last_message_at,'message_count',NEW.message_count)")?;
    // A complete empty directory is distinct from an older node without this
    // projection. Unknown Resource variants are ignored by existing clients.
    conn.execute_batch(
        "INSERT INTO sync_entities(scope,kind,id,value)
        VALUES('{\"type\":\"catalog\"}','resource','zork:chat-catalog',
          '{\"resource_type\":\"chat_catalog\",\"schema_version\":1}')
        ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value
        WHERE sync_entities.value IS NOT excluded.value;",
    )?;
    conn.execute_batch("CREATE TRIGGER IF NOT EXISTS chat_sync_author AFTER INSERT ON chat_message_facts BEGIN UPDATE visible_messages SET role=role WHERE message_id=NEW.message_id; END;
        CREATE TRIGGER IF NOT EXISTS chat_sync_home AFTER INSERT ON chat_agent_home BEGIN UPDATE node_agents SET value=value WHERE id=NEW.agent_id; END;")?;

    install_projection(conn, "conversation_pages", "id", "resource", catalog, "(SELECT id FROM sessions WHERE key=NEW.session_key) IS NOT NULL", "json_object('resource_type','conversation_page','id',NEW.id,'session_id',(SELECT id FROM sessions WHERE key=NEW.session_key),'message_id',NEW.message_id,'page',json(NEW.page),'source_session_id',NEW.source_session_id,'created_at',NEW.created_at)")?;
    install_projection(conn, "published_pages", "id", "resource", catalog, "(SELECT id FROM sessions WHERE key=NEW.session_key) IS NOT NULL", "json_object('resource_type','application','page',json(NEW.page),'owner_session_id',(SELECT id FROM sessions WHERE key=NEW.session_key),'created_at',NEW.created_at)")?;
    install_projection(conn, "conversation_file_refs", "id", "resource", catalog, "(SELECT id FROM sessions WHERE key=NEW.session_key) IS NOT NULL", "json_object('resource_type','conversation_file','id',NEW.id,'session_id',(SELECT id FROM sessions WHERE key=NEW.session_key),'artifact_id',NEW.artifact_id,'source_session_id',NEW.source_session_id)")?;
    conn.execute_batch("CREATE TRIGGER IF NOT EXISTS sync_page_session_before BEFORE UPDATE OF id ON sessions WHEN OLD.id IS NOT NEW.id BEGIN
        UPDATE sync_entities SET value=NULL WHERE kind='resource' AND id IN (SELECT id FROM conversation_pages WHERE session_key=OLD.key UNION ALL SELECT id FROM published_pages WHERE session_key=OLD.key UNION ALL SELECT id FROM conversation_file_refs WHERE session_key=OLD.key);
        END;
        CREATE TRIGGER IF NOT EXISTS sync_page_session_after AFTER UPDATE OF id ON sessions WHEN OLD.id IS NOT NEW.id BEGIN
        UPDATE conversation_pages SET message_id=message_id WHERE session_key=NEW.key;
        UPDATE published_pages SET page=page WHERE session_key=NEW.key;
        UPDATE conversation_file_refs SET artifact_id=artifact_id WHERE session_key=NEW.key;
        END;")?;
    conn.execute_batch("RELEASE sync_schema;")?;
    Ok(())
}

fn install_projection(
    conn: &Connection,
    table: &str,
    id: &str,
    kind: &str,
    scope: &str,
    visible: &str,
    value: &str,
) -> Result<()> {
    let installed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name=?1)",
        [format!("sync_{table}_insert")],
        |r| r.get(0),
    )?;
    let insert = format!("INSERT INTO sync_entities(scope,kind,id,value) SELECT {scope},'{kind}',NEW.{id},{value} WHERE {visible} ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value;");
    let old_scope = scope.replace("NEW.", "OLD.");
    let old_visible = visible.replace("NEW.", "OLD.");
    let remove = format!("UPDATE sync_entities SET value=NULL WHERE scope={old_scope} AND kind='{kind}' AND id=OLD.{id} AND ({old_visible});");
    // Updating unrelated fields must not manufacture revisions or tombstones.
    let moved = format!("(({old_visible}) AND (NOT ({visible}) OR OLD.{id} IS NOT NEW.{id} OR {old_scope} IS NOT {scope}))");
    conn.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS sync_{table}_insert AFTER INSERT ON {table} BEGIN {insert} END;
        CREATE TRIGGER IF NOT EXISTS sync_{table}_update AFTER UPDATE ON {table} BEGIN
            UPDATE sync_entities SET value=NULL WHERE scope={old_scope} AND kind='{kind}' AND id=OLD.{id} AND {moved};
            {insert} END;
        CREATE TRIGGER IF NOT EXISTS sync_{table}_delete BEFORE DELETE ON {table} BEGIN {remove} END;"))?;
    // A fresh projection imports once; subsequent starts use its durable
    // triggers and avoid scanning all message bodies on every Station boot.
    if !installed {
        let seed=format!("INSERT INTO sync_entities(scope,kind,id,value) SELECT {scope},'{kind}',NEW.{id},{value} FROM {table} NEW WHERE {visible} ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value;");
        conn.execute_batch(&seed)?;
    }
    Ok(())
}

fn install_markers(conn: &Connection) -> Result<()> {
    let existed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='view' AND name='sync_marker_view')",
        [],
        |r| r.get(0),
    )?;
    conn.execute_batch("CREATE VIEW IF NOT EXISTS sync_marker_view AS
        SELECT s.key AS session_key,s.id AS id,
        json_object('session_id',s.id,'last_message_id',m.message_id,'created_at',m.created_at,'role',CASE WHEN m.role='user' AND (m.message_id GLOB 'assignment-*' OR m.message_id GLOB 'rework-*') AND EXISTS(SELECT 1 FROM worker_tasks WHERE session_key=s.key) THEN 'assistant' ELSE m.role END) AS value
        FROM sessions s JOIN visible_messages m ON m.sequence=(SELECT MAX(sequence) FROM visible_messages WHERE session_key=s.key)
        WHERE s.platform='local_gui' AND s.id IS NOT NULL;")?;
    let refresh = |key: &str| {
        format!("INSERT INTO sync_entities(scope,kind,id,value) SELECT '{{\"type\":\"catalog\"}}','read_marker',id,value FROM sync_marker_view WHERE session_key={key} ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value;
        UPDATE sync_entities SET value=NULL WHERE scope='{{\"type\":\"catalog\"}}' AND kind='read_marker' AND id=(SELECT id FROM sessions WHERE key={key}) AND NOT EXISTS(SELECT 1 FROM sync_marker_view WHERE session_key={key});")
    };
    for event in ["INSERT", "UPDATE", "DELETE"] {
        let body = match event {
            "INSERT" => refresh("NEW.session_key"),
            "DELETE" => refresh("OLD.session_key"),
            _ => format!(
                "{}{}",
                refresh("OLD.session_key"),
                refresh("NEW.session_key")
            ),
        };
        conn.execute_batch(&format!("CREATE TRIGGER IF NOT EXISTS sync_marker_{event} AFTER {event} ON visible_messages BEGIN {body} END;"))?;
    }
    conn.execute_batch("CREATE TRIGGER IF NOT EXISTS sync_marker_session_delete BEFORE DELETE ON sessions BEGIN UPDATE sync_entities SET value=NULL WHERE kind='read_marker' AND id=OLD.id; END;
        CREATE TRIGGER IF NOT EXISTS sync_marker_worker_insert AFTER INSERT ON worker_tasks BEGIN UPDATE visible_messages SET role=role WHERE session_key=NEW.session_key AND (message_id GLOB 'assignment-*' OR message_id GLOB 'rework-*'); END;
        CREATE TRIGGER IF NOT EXISTS sync_marker_worker_delete AFTER DELETE ON worker_tasks BEGIN UPDATE visible_messages SET role=role WHERE session_key=OLD.session_key AND (message_id GLOB 'assignment-*' OR message_id GLOB 'rework-*'); END;")?;
    if !existed {
        conn.execute_batch("INSERT INTO sync_entities(scope,kind,id,value) SELECT '{\"type\":\"catalog\"}','read_marker',id,value FROM sync_marker_view WHERE 1 ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value;")?;
    }
    Ok(())
}

fn reset(owner: &str, epoch: &str, reason: &str) -> Reply {
    Reply::ResetRequired {
        owner: owner.into(),
        epoch: epoch.into(),
        reason: reason.into(),
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Watermark {
    owner: String,
    epoch: String,
    sequence: u64,
}

impl StationDb {
    pub(super) fn sync_check_lineage(&self, conn: &Connection) -> Result<()> {
        let current = Self::sync_watermark_value(conn)?;
        match fs::read(&self.sync_watermark) {
            Ok(bytes) => {
                let served: Watermark =
                    serde_json::from_slice(&bytes).context("invalid sync lineage watermark")?;
                if served.owner == current.owner
                    && (served.epoch != current.epoch || served.sequence > current.sequence)
                {
                    // SQLite-only backup restoration must not reuse issued
                    // revisions. Normal restart keeps the same durable epoch.
                    conn.execute_batch("UPDATE sync_meta SET epoch=lower(hex(randomblob(16))); DELETE FROM sync_exports;")?;
                    return self.sync_record_watermark(conn);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        // Opening a database issues no cursor. Preserve the last actually
        // published watermark; page/receipt publication persists newer values
        // before returning them. This also avoids rewriting it on an idle boot.
        Ok(())
    }
    fn sync_watermark_value(conn: &Connection) -> Result<Watermark> {
        Ok(conn.query_row(
            "SELECT i.owner,m.epoch,m.sequence FROM sync_identity i,sync_meta m",
            [],
            |r| {
                Ok(Watermark {
                    owner: r.get(0)?,
                    epoch: r.get(1)?,
                    sequence: r.get(2)?,
                })
            },
        )?)
    }
    pub(super) fn sync_record_watermark(&self, conn: &Connection) -> Result<()> {
        use std::io::Write;
        let value = Self::sync_watermark_value(conn)?;
        let bytes = serde_json::to_vec(&value)?;
        if fs::read(&self.sync_watermark).ok().as_deref() == Some(bytes.as_slice()) {
            return Ok(());
        }
        let temp = self.sync_watermark.with_extension("pending");
        let mut file = fs::File::create(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &self.sync_watermark)?;
        if let Some(parent) = self.sync_watermark.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    pub fn sync_bind_mesh_owner(&self, origin: &str) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex");
        let changed = conn.execute(
            "UPDATE sync_identity SET owner=?1 WHERE singleton=1 AND owner<>?1",
            [origin],
        )?;
        if changed > 0 {
            self.sync_record_watermark(&conn)?;
        }
        Ok(())
    }
    pub fn sync_local_owner(&self) -> Result<String> {
        Ok(self.conn.lock().expect("db mutex").query_row(
            "SELECT owner FROM sync_identity",
            [],
            |r| r.get(0),
        )?)
    }

    /// File-backed profiles remain a reconciled public projection until their
    /// authority migrates to SQLite. A failed source read must never mean empty.
    pub fn sync_reconcile_catalog(
        &self,
        device: Option<Value>,
        profiles: Option<&[zork_profile::ProfileView]>,
    ) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let scope = Scope::Catalog {}.key();
        let providers = zork_profile::list_providers();
        let profile_rows = profiles
            .unwrap_or_default()
            .iter()
            .map(|p| Ok((p.profile_id.clone(), serde_json::to_value(p)?)))
            .collect::<Result<Vec<_>>>()?;
        let provider_rows = providers["providers"]
            .as_array()
            .context("provider catalog missing")?
            .iter()
            .map(|v| {
                Ok((
                    v["id"].as_str().context("provider id missing")?.to_owned(),
                    v.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        for (kind, rows) in [
            (
                Kind::Device,
                device
                    .clone()
                    .into_iter()
                    .map(|value| ("self".to_owned(), value))
                    .collect(),
            ),
            (Kind::Profile, profile_rows),
            (Kind::Provider, provider_rows),
        ] {
            if (kind == Kind::Profile && profiles.is_none())
                || (kind == Kind::Device && device.is_none())
            {
                continue;
            }
            let mut ids = std::collections::HashSet::new();
            for (id, value) in rows {
                ids.insert(id.clone());
                tx.execute("INSERT INTO sync_entities(scope,kind,id,value) VALUES(?1,?2,?3,?4) ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value",params![scope,kind.key(),id,serde_json::to_string(&value)?])?;
            }
            let old = tx
                .prepare(
                    "SELECT id FROM sync_entities WHERE scope=?1 AND kind=?2 AND value IS NOT NULL",
                )?
                .query_map(params![scope, kind.key()], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for id in old {
                if !ids.contains(&id) {
                    tx.execute(
                        "UPDATE sync_entities SET value=NULL WHERE scope=?1 AND kind=?2 AND id=?3",
                        params![scope, kind.key(), id],
                    )?;
                }
            }
        }
        for (id, available) in [
            ("profiles", profiles.is_some()),
            ("device", device.is_some()),
        ] {
            let ready:bool=tx.query_row("SELECT COALESCE(json_extract(value,'$.ready'),0) FROM sync_entities WHERE scope=?1 AND kind='resource' AND id=?2",params![scope,id],|r|r.get(0)).optional()?.unwrap_or(false);
            let value = json!({"ready":ready||available,"error":if available{None}else{Some(format!("{id}_projection_unavailable"))}});
            tx.execute("INSERT INTO sync_entities(scope,kind,id,value) VALUES(?1,'resource',?2,?3) ON CONFLICT(scope,kind,id) DO UPDATE SET value=excluded.value WHERE sync_entities.value IS NOT excluded.value",params![scope,id,value.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The durable high-water mark is authoritative; notifications are hints.
    pub fn sync_cursor(&self, owner: &str, scope: Scope) -> Result<Cursor> {
        let conn = self.published_messages()?;
        let (epoch, sequence) =
            conn.query_row("SELECT epoch,sequence FROM sync_meta", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        Ok(Cursor {
            owner: owner.into(),
            epoch,
            scope,
            sequence,
        })
    }

    pub fn sync_maintain(&self) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE sync_meta SET floor=sequence-100000 WHERE floor<sequence-100000",
            [],
        )?;
        tx.execute(
            "DELETE FROM sync_entities WHERE value IS NULL AND revision<=(SELECT floor FROM sync_meta)",
            [],
        )?;
        tx.execute(
            "DELETE FROM sync_exports WHERE created < unixepoch()-900",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Compaction only discards deletion payloads older than an explicit floor.
    /// Readers behind that floor must bootstrap; exported batches stay frozen.
    #[cfg(test)]
    pub fn sync_compact(&self, floor: u64) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let sequence: u64 = tx.query_row("SELECT sequence FROM sync_meta", [], |r| r.get(0))?;
        ensure!(floor <= sequence, "sync_invalid_retention_floor");
        tx.execute("UPDATE sync_meta SET floor=MAX(floor,?1)", [floor])?;
        tx.execute(
            "DELETE FROM sync_entities WHERE value IS NULL AND revision<=?1",
            [floor],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// `owner` is the authenticated local node origin, supplied by the server.
    pub fn sync_pull(&self, owner: &str, request: &Pull) -> Result<Reply> {
        request.scope.validate().map_err(anyhow::Error::msg)?;
        zork_client_types::sync::identifier(owner).map_err(anyhow::Error::msg)?;
        let mut conn = self.published_messages()?;
        let tx = conn.transaction()?;
        let (epoch, sequence, floor): (String, u64, u64) =
            tx.query_row("SELECT epoch,sequence,floor FROM sync_meta", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        let through = Cursor {
            owner: owner.into(),
            epoch: epoch.clone(),
            scope: request.scope.clone(),
            sequence,
        };
        if let Some(after) = &request.after {
            after.validate().map_err(anyhow::Error::msg)?;
            if !after.same_stream(&through) || after.sequence < floor || after.sequence > sequence {
                return Ok(reset(owner, &epoch, "cursor_unavailable"));
            }
        }
        tx.execute(
            "DELETE FROM sync_exports WHERE created < unixepoch()-900",
            [],
        )?;
        let (mut page, start) = if let Some(next) = &request.continuation {
            let stored: Option<(String,u64)> = tx.query_row("SELECT e.header,p.start_position FROM sync_exports e JOIN sync_export_pages p ON p.batch=e.id WHERE e.id=?1 AND e.owner=?2 AND e.scope=?3 AND p.page_index=?4", params![next.batch_id,owner,request.scope.key(),next.index], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((header, start)) = stored else {
                tx.commit()?;
                return Ok(reset(owner, &epoch, "batch_expired"));
            };
            let mut page: Page = serde_json::from_str(&header)?;
            ensure!(
                page.from == request.after,
                "sync_continuation_cursor_mismatch"
            );
            page.index = next.index;
            (page, start)
        } else {
            let batch = ulid::Ulid::new().to_string();
            let page = Page {
                protocol: PROTOCOL,
                batch_id: batch.clone(),
                from: request.after.clone(),
                through,
                index: 0,
                last: false,
                records: vec![],
            };
            // Bound abandoned exports; expired clients restart their scope without
            // touching their already committed replica or local drafts.
            tx.execute("DELETE FROM sync_exports WHERE id IN (SELECT id FROM sync_exports ORDER BY created DESC,id LIMIT -1 OFFSET 15)",[])?;
            tx.execute("INSERT INTO sync_exports(id,owner,scope,header,created) VALUES(?1,?2,?3,?4,unixepoch())",params![batch,owner,request.scope.key(),serde_json::to_string(&page)?])?;
            tx.execute("INSERT INTO sync_export_rows(batch,position,kind,id,revision,value) SELECT ?1,row_number() OVER(ORDER BY kind,id),kind,id,revision,value FROM sync_entities WHERE scope=?2 AND revision>?3 AND (?4 OR value IS NOT NULL)",params![batch,request.scope.key(),request.after.as_ref().map_or(0,|c|c.sequence),request.after.is_some()])?;
            tx.execute("INSERT INTO sync_export_pages VALUES(?1,0,0)", [&batch])?;
            (page, 0)
        };
        let mut statement = tx.prepare("SELECT position,kind,id,revision,value FROM sync_export_rows WHERE batch=?1 AND position>?2 ORDER BY position LIMIT ?3")?;
        let rows = statement
            .query_map(params![page.batch_id, start, MAX_PAGE_RECORDS + 1], |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, u64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let mut last_position = start;
        let mut bytes = serde_json::to_vec(&page)?.len();
        let total = rows.len();
        for (position, kind, id, revision, value) in rows {
            let mut record = Record {
                kind: serde_json::from_value(Value::String(kind))?,
                id,
                revision,
                value: value.map(|v| serde_json::from_str(&v)).transpose()?,
            };
            if record.kind == Kind::Message {
                if let Some(value) = record.value.as_mut() {
                    let sequence = value["sequence"]
                        .as_i64()
                        .context("message source position missing")?;
                    // The export freezes metadata; source bodies are immutable and
                    // remain readable even if the binding is later deleted.
                    let source = self.message_log.get(sequence)?;
                    value["text"] = Value::String(source.text());
                    value["source_sequence"] = json!(sequence);
                    value["source_epoch"] =
                        json!(self.message_log.epoch(&source.message.chat_id)?);
                }
            }
            let size = serde_json::to_vec(&record)?.len() + 1;
            ensure!(size + 4096 <= MAX_PAGE_BYTES, "sync_entity_too_large");
            if page.records.len() == MAX_PAGE_RECORDS || bytes + size + 4096 > MAX_PAGE_BYTES {
                break;
            }
            bytes += size;
            last_position = position;
            page.records.push(record);
        }
        page.last = page.records.len() == total;
        if !page.last {
            tx.execute(
                "INSERT OR IGNORE INTO sync_export_pages VALUES(?1,?2,?3)",
                params![page.batch_id, page.index + 1, last_position],
            )?;
        }
        page.validate().map_err(anyhow::Error::msg)?;
        self.sync_record_watermark(&tx)?;
        tx.commit()?;
        Ok(Reply::Page { page })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_keeps_last_issued_watermark_until_new_data_is_published() {
        let root = tempfile::tempdir().unwrap();
        let db = open(&root);
        let watermark = db.sync_watermark.clone();
        assert!(!watermark.exists());
        agent(&db.conn.lock().unwrap(), "a", "Published");
        let first = pull(&db, None, None).through;
        let issued = fs::read(&watermark).unwrap();
        agent(&db.conn.lock().unwrap(), "a", "Not yet published");
        drop(db);

        let db = open(&root);
        assert_eq!(fs::read(&watermark).unwrap(), issued);
        let next = pull(&db, Some(first.clone()), None);
        assert_eq!(next.through.epoch, first.epoch);
        assert!(next.through.sequence > first.sequence);
        assert_ne!(fs::read(&watermark).unwrap(), issued);
        assert!(next.records.iter().any(|record| record
            .value
            .as_ref()
            .is_some_and(|value| value["name"] == "Not yet published")));
    }

    #[test]
    fn reads_and_identical_reconciliation_are_quiet() {
        let root = tempfile::tempdir().unwrap();
        let db = open(&root);
        db.sync_reconcile_catalog(Some(json!({"name":"Device"})), Some(&[]))
            .unwrap();
        let mut changes = db.realtime.subscribe();
        let before = db.sync_cursor("owner", Scope::Catalog {}).unwrap();
        for _ in 0..20 {
            let page = pull(&db, Some(before.clone()), None);
            assert!(page.records.is_empty());
            assert_eq!(page.through, before);
            db.sync_reconcile_catalog(Some(json!({"name":"Device"})), Some(&[]))
                .unwrap();
            db.sync_maintain().unwrap();
            assert!(
                !changes.has_changed().unwrap(),
                "read/cache bookkeeping emitted a change"
            );
        }
        db.sync_reconcile_catalog(Some(json!({"name":"Renamed"})), Some(&[]))
            .unwrap();
        assert!(changes.has_changed().unwrap());
        assert!(changes.borrow_and_update().sync > 0);
        let after = db.sync_cursor("owner", Scope::Catalog {}).unwrap();
        assert!(after.sequence > before.sequence);
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "BEGIN; UPDATE sync_entities SET value='{}' WHERE kind='device'; ROLLBACK",
        )
        .unwrap();
        drop(conn);
        assert!(!changes.has_changed().unwrap());
        assert_eq!(db.sync_cursor("owner", Scope::Catalog {}).unwrap(), after);
        // Retention metadata is not a new business revision, even when it writes.
        db.conn
            .lock()
            .unwrap()
            .execute("UPDATE sync_meta SET sequence=sequence+100001", [])
            .unwrap();
        changes.borrow_and_update();
        let retained = db.sync_cursor("owner", Scope::Catalog {}).unwrap();
        db.sync_maintain().unwrap();
        assert!(
            !changes.has_changed().unwrap(),
            "retention emitted a business change"
        );
        assert_eq!(
            db.sync_cursor("owner", Scope::Catalog {}).unwrap(),
            retained
        );
    }

    fn open(dir: &tempfile::TempDir) -> StationDb {
        StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap()
    }
    fn pull(
        db: &StationDb,
        after: Option<Cursor>,
        continuation: Option<zork_client_types::sync::Continuation>,
    ) -> Page {
        match db
            .sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Catalog {},
                    after,
                    continuation,
                },
            )
            .unwrap()
        {
            Reply::Page { page } => page,
            _ => panic!("unexpected reset"),
        }
    }
    fn agent(conn: &Connection, id: &str, name: &str) {
        conn.execute("INSERT INTO node_agents(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value",params![id,json!({"id":id,"name":name,"role":"leader","profile_id":"p","model":"m","thinking":"off","instructions":"","allowed_leaders":[],"secret":"must never escape"}).to_string()]).unwrap();
    }
    #[test]
    fn projection_commits_with_business_data_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        let empty = pull(&db, None, None);
        assert!(empty.last);
        assert_eq!(empty.records.len(), 1);
        assert_eq!(empty.records[0].id, "zork:chat-catalog");
        assert_eq!(
            empty.records[0].value.as_ref().unwrap()["resource_type"],
            "chat_catalog"
        );
        {
            let mut conn = db.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            agent(&tx, "a", "Rolled back");
        }
        assert_eq!(
            pull(&db, Some(empty.through.clone()), None).through,
            empty.through
        );
        agent(&db.conn.lock().unwrap(), "a", "Alice");
        let first = pull(&db, Some(empty.through), None);
        assert_eq!(first.records.len(), 1);
        assert!(first.records[0]
            .value
            .as_ref()
            .unwrap()
            .get("secret")
            .is_none());
        drop(db);
        let db = open(&dir);
        let restarted = pull(&db, Some(first.through.clone()), None);
        assert_eq!(restarted.through, first.through);
        assert!(restarted.records.is_empty());
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM node_agents WHERE id='a'", [])
            .unwrap();
        let deletion = pull(&db, Some(first.through), None);
        assert!(deletion.records[0].value.is_none());
        let remaining = pull(&db, None, None);
        assert_eq!(remaining.records.len(), 1);
        assert_eq!(remaining.records[0].id, "zork:chat-catalog");
    }
    #[test]
    fn paginated_export_is_stable_during_writes_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        {
            let mut conn = db.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            for i in 0..600 {
                agent(&tx, &format!("a{i:04}"), "Before");
            }
            tx.commit().unwrap();
        }
        let first = pull(&db, None, None);
        assert_eq!(first.records.len(), 512);
        assert!(!first.last);
        agent(&db.conn.lock().unwrap(), "a0599", "After");
        let next = zork_client_types::sync::Continuation {
            batch_id: first.batch_id.clone(),
            index: 1,
        };
        let last = pull(&db, None, Some(next.clone()));
        assert!(last.last);
        assert_eq!(last.records.len(), 89);
        assert_eq!(
            last.records
                .iter()
                .find(|record| record.id == "a0599")
                .unwrap()
                .value
                .as_ref()
                .unwrap()["name"],
            "Before"
        );
        assert!(last
            .records
            .iter()
            .any(|record| record.id == "zork:chat-catalog"));
        assert_eq!(pull(&db, None, Some(next)), last);
        let delta = pull(&db, Some(last.through), None);
        assert_eq!(delta.records.len(), 1);
        assert_eq!(delta.records[0].value.as_ref().unwrap()["name"], "After");
    }
    #[test]
    fn projection_upgrade_rebuilds_derived_rows_and_preserves_other_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        agent(&db.conn.lock().unwrap(), "a", "Before");
        let before = pull(&db, None, None).through;
        db.conn.lock().unwrap().execute_batch("DROP TRIGGER sync_node_agents_update; CREATE TRIGGER sync_node_agents_update AFTER UPDATE ON node_agents BEGIN SELECT 1; END; CREATE TRIGGER sync_custom_audit AFTER UPDATE ON node_agents BEGIN SELECT 1; END; DELETE FROM sync_schema;").unwrap();
        agent(&db.conn.lock().unwrap(), "a", "New public projection");
        drop(db);
        let db = open(&dir);
        let after = pull(&db, None, None);
        assert_ne!(before.epoch, after.through.epoch);
        assert_eq!(
            after
                .records
                .iter()
                .find(|r| r.kind == Kind::Agent)
                .unwrap()
                .value
                .as_ref()
                .unwrap()["name"],
            "New public projection"
        );
        let retained:usize=db.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name IN ('sync_custom_audit','sync_version_signal')",[],|r|r.get(0)).unwrap();
        assert_eq!(retained, 2);
    }
    #[test]
    fn sqlite_restore_rotates_epoch_but_ordinary_restart_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        agent(&db.conn.lock().unwrap(), "a", "Before");
        let first = pull(&db, None, None).through;
        db.conn
            .lock()
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        let backup = dir.path().join("backup.sqlite");
        fs::copy(dir.path().join(GATEWAY_DB), &backup).unwrap();
        agent(&db.conn.lock().unwrap(), "a", "After");
        let issued = pull(&db, Some(first.clone()), None).through;
        drop(db);
        let db = open(&dir);
        assert_eq!(pull(&db, None, None).through.epoch, issued.epoch);
        drop(db);
        fs::copy(&backup, dir.path().join(GATEWAY_DB)).unwrap();
        let db = open(&dir);
        let restored = pull(&db, None, None);
        assert_ne!(restored.through.epoch, issued.epoch);
        assert_eq!(
            restored.records[0].value.as_ref().unwrap()["name"],
            "Before"
        );
        assert!(matches!(
            db.sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Catalog {},
                    after: Some(issued),
                    continuation: None
                }
            )
            .unwrap(),
            Reply::ResetRequired { .. }
        ));
    }
    #[test]
    fn failed_external_resource_does_not_block_sqlite_catalog_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        db.sync_reconcile_catalog(Some(json!({"name":"Device"})), Some(&[]))
            .unwrap();
        let before = pull(&db, None, None).through;
        agent(&db.conn.lock().unwrap(), "a", "New Agent");
        db.sync_reconcile_catalog(None, None).unwrap();
        let delta = pull(&db, Some(before), None);
        assert!(delta.records.iter().any(|r| r.kind == Kind::Agent));
        let profiles = delta
            .records
            .iter()
            .find(|r| r.kind == Kind::Resource && r.id == "profiles")
            .unwrap()
            .value
            .as_ref()
            .unwrap();
        assert_eq!(profiles["ready"], true);
        assert!(profiles["error"].is_string());
        assert!(!delta.records.iter().any(|r| r.kind == Kind::Device));
        db.sync_reconcile_catalog(Some(json!({"name":"Device"})), Some(&[]))
            .unwrap();
        let recovered = pull(&db, Some(delta.through), None);
        assert!(recovered
            .records
            .iter()
            .find(|r| r.kind == Kind::Resource && r.id == "profiles")
            .unwrap()
            .value
            .as_ref()
            .unwrap()["error"]
            .is_null());
    }
    #[test]
    fn retention_floor_prevents_missing_compacted_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        agent(&db.conn.lock().unwrap(), "a", "A");
        let old = pull(&db, None, None).through;
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM node_agents WHERE id='a'", [])
            .unwrap();
        let latest = pull(&db, Some(old.clone()), None).through;
        db.sync_compact(latest.sequence).unwrap();
        assert!(matches!(
            db.sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Catalog {},
                    after: Some(old),
                    continuation: None
                }
            )
            .unwrap(),
            Reply::ResetRequired { .. }
        ));
        let remaining = pull(&db, None, None);
        assert_eq!(remaining.records.len(), 1);
        assert_eq!(remaining.records[0].id, "zork:chat-catalog");
    }
    #[test]
    fn foreign_and_expired_cursors_require_explicit_reset() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir);
        let mut cursor = pull(&db, None, None).through;
        cursor.epoch = "restored".into();
        assert!(matches!(
            db.sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Catalog {},
                    after: Some(cursor),
                    continuation: None
                }
            )
            .unwrap(),
            Reply::ResetRequired { .. }
        ));
        let next = zork_client_types::sync::Continuation {
            batch_id: "expired".into(),
            index: 1,
        };
        assert!(matches!(
            db.sync_pull(
                "owner",
                &Pull {
                    scope: Scope::Catalog {},
                    after: None,
                    continuation: Some(next)
                }
            )
            .unwrap(),
            Reply::ResetRequired { .. }
        ));
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use axum::{
        extract::State,
        routing::{get, post},
        Json, Router,
    };
    use std::sync::Arc;
    use zork_client_core::{
        api::StationClient,
        state::{Device, Domains},
        store::ClientStore,
    };

    #[tokio::test]
    async fn live_sync_converges_then_stops_even_with_duplicate_hints() {
        use axum::response::sse::{Event, Sse};
        use std::{
            convert::Infallible,
            sync::atomic::{AtomicUsize, Ordering},
            time::Duration,
        };
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(StationDb::open(root.path(), &root.path().join("workspaces")).unwrap());
        db.sync_reconcile_catalog(Some(json!({"name":"Before"})), Some(&[]))
            .unwrap();
        let pulls = Arc::new(AtomicUsize::new(0));
        let count = pulls.clone();
        let fail_once = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let events_db = db.clone();
        let router = Router::new()
            .route(
                "/v1/im/sessions",
                get(|| async { Json(json!({"items":[]})) }),
            )
            .route("/v1/mesh", get(|| async { Json(json!({"enabled":false})) }))
            .route(
                "/v1/node/info",
                get(|| async { Json(json!({"sync":{"protocol":1,"owner":"owner"}})) }),
            )
            .route(
                "/v1/node/sync",
                post(
                    move |State(db): State<Arc<StationDb>>, Json(pull): Json<Pull>| {
                        let count = count.clone();
                        let fail_once = fail_once.clone();
                        async move {
                            use axum::response::IntoResponse;
                            count.fetch_add(1, Ordering::SeqCst);
                            if fail_once.swap(false, Ordering::SeqCst) {
                                return (
                                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                    Json(json!({"error":"temporary failure"})),
                                )
                                    .into_response();
                            }
                            Json(db.sync_pull("owner", &pull).unwrap()).into_response()
                        }
                    },
                ),
            )
            .route(
                "/v1/im/events",
                get(move || {
                    let db = events_db.clone();
                    async move {
                        let changes = db.realtime.subscribe();
                        Sse::new(futures_util::stream::unfold(
                            (db, changes, true),
                            |(db, mut changes, first)| async move {
                                if !first && changes.changed().await.is_err() {
                                    return None;
                                }
                                changes.borrow_and_update();
                                let cursor = db.sync_cursor("owner", Scope::Catalog {}).unwrap();
                                let event = Event::default()
                                    .event("changed")
                                    .data(json!({"catalog":cursor}).to_string());
                                Some((Ok::<_, Infallible>(event), (db, changes, false)))
                            },
                        ))
                    }
                }),
            )
            .with_state(db.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Arc::new(StationClient::new(
            format!("http://{}", listener.local_addr().unwrap()),
            None,
        ));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let cache = tempfile::tempdir().unwrap();
        let store = Arc::new(ClientStore::open(cache.path()).unwrap());
        let device = Device::open(client.clone(), Some((store.clone(), "peer".into())), true);
        device.start();
        for name in ["Before", "After"] {
            db.sync_reconcile_catalog(Some(json!({"name":name})), Some(&[]))
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let value = store
                        .replica_record("peer", &Scope::Catalog {}, Kind::Device, "self")
                        .unwrap();
                    if value
                        .and_then(|r| r.value)
                        .is_some_and(|v| v["name"] == name)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let settled = pulls.load(Ordering::SeqCst);
            for _ in 0..50 {
                db.realtime.notify(crate::realtime::SYNC);
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(
                pulls.load(Ordering::SeqCst),
                settled,
                "caught-up client kept pulling"
            );
        }
        assert!(
            pulls.load(Ordering::SeqCst) >= 3,
            "failed initial pull was not retried"
        );
        let quiet = pulls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(35)).await;
        assert_eq!(
            pulls.load(Ordering::SeqCst),
            quiet,
            "healthy client still polls after 30 seconds"
        );
        drop(device);
        tokio::task::yield_now().await;
        let settled = pulls.load(Ordering::SeqCst);
        let reopened = Device::open(client, Some((store, "peer".into())), true);
        reopened.start();
        tokio::time::timeout(Duration::from_secs(5), async {
            while reopened.snapshot().online != Some(true) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            pulls.load(Ordering::SeqCst),
            settled,
            "reconnect reread an unchanged durable cursor"
        );
        drop(reopened);
        server.abort();
    }

    #[tokio::test]
    async fn two_clients_converge_and_reopen_offline_from_the_same_replica() {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(StationDb::open(root.path(), &root.path().join("workspaces")).unwrap());
        db.sync_reconcile_catalog(Some(json!({"name":"Fixture"})), Some(&[]))
            .unwrap();
        let write = |name: &str| {
            db.conn.lock().unwrap().execute("INSERT INTO node_agents(id,value) VALUES('a',?1) ON CONFLICT(id) DO UPDATE SET value=excluded.value",[json!({"name":name,"role":"leader","profile_id":"p","model":"m","thinking":"off","instructions":"","allowed_leaders":[]}).to_string()]).unwrap();
        };
        write("Before");
        let router = Router::new()
            .route(
                "/v1/node/info",
                get(|| async { Json(json!({"sync":{"protocol":1,"owner":"owner"}})) }),
            )
            .route(
                "/v1/node/sync",
                post(
                    |State(db): State<Arc<StationDb>>, Json(pull): Json<Pull>| async move {
                        Json(db.sync_pull("owner", &pull).unwrap())
                    },
                ),
            )
            .with_state(db.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let store_a = Arc::new(ClientStore::open(a.path()).unwrap());
        let store_b = Arc::new(ClientStore::open(b.path()).unwrap());
        let client = Arc::new(StationClient::new(&url, None));
        let one = Device::open(client.clone(), Some((store_a.clone(), "peer".into())), true);
        let two = Device::open(client.clone(), Some((store_b.clone(), "peer".into())), true);
        tokio::join!(one.refresh(Domains::AGENTS), two.refresh(Domains::AGENTS));
        assert_eq!(one.snapshot().agents[0]["name"], "Before");
        assert!(!one.profiles().snapshot().providers.is_empty());
        write("After");
        tokio::join!(one.refresh(Domains::AGENTS), two.refresh(Domains::AGENTS));
        assert_eq!(one.snapshot().agents, two.snapshot().agents);
        assert_eq!(two.snapshot().agents[0]["name"], "After");
        let cursor = store_a
            .replica_state("peer", &Scope::Catalog {})
            .unwrap()
            .cursor
            .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM node_agents WHERE id='a'", [])
            .unwrap();
        one.refresh(Domains::AGENTS).await;
        assert!(one.snapshot().agents.is_empty());
        assert!(
            store_a
                .replica_state("peer", &Scope::Catalog {})
                .unwrap()
                .cursor
                .unwrap()
                .sequence
                > cursor.sequence
        );
        // The second client was disconnected before deletion; its last committed
        // state remains usable after a process-style reopen, with unknown presence.
        drop(two);
        drop(store_b);
        server.abort();
        let store_b = Arc::new(ClientStore::open(b.path()).unwrap());
        let reopened = Device::open(client, Some((store_b, "peer".into())), true);
        assert_eq!(reopened.snapshot().agents[0]["name"], "After");
        assert_eq!(reopened.snapshot().online, None);
        assert!(!reopened.profiles().snapshot().providers.is_empty());
    }
}
