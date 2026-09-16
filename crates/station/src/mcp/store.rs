use super::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

pub struct Store(
    Mutex<Connection>,
    zork_notify::Notifier,
    zork_notify::Notifier,
);
impl Store {
    pub fn cancel(&self, who: &Subject, invocation: &str) -> Result<()> {
        self.0.lock().expect("tool db").execute(
            "INSERT OR IGNORE INTO cancellations(origin,invocation) VALUES(?1,?2)",
            params![who.origin, invocation],
        )?;
        self.notify();
        Ok(())
    }
    pub async fn cancellation(&self, who: &Subject, invocation: &str) -> Result<()> {
        let mut changes = self.subscribe();
        loop {
            changes.checkpoint();
            if self.cancelled(who, invocation)? {
                return Ok(());
            }
            changes.changed().await?;
        }
    }
    pub fn cancelled(&self, who: &Subject, invocation: &str) -> Result<bool> {
        Ok(self.0.lock().expect("tool db").query_row(
            "SELECT EXISTS(SELECT 1 FROM cancellations WHERE origin=?1 AND invocation=?2)",
            params![who.origin, invocation],
            |row| row.get(0),
        )?)
    }

    pub fn subscribe(&self) -> zork_notify::Changes {
        self.1.subscribe()
    }
    pub fn definitions(&self) -> zork_notify::Changes {
        self.2.subscribe()
    }
    pub fn notify(&self) {
        self.1.notify();
    }
    pub fn open(root: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(root.join("mcp.sqlite"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS cancellations(origin TEXT NOT NULL,invocation TEXT NOT NULL,PRIMARY KEY(origin,invocation));
            CREATE TABLE IF NOT EXISTS servers(id TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS management(origin TEXT NOT NULL,invocation TEXT NOT NULL,digest TEXT NOT NULL,result TEXT NOT NULL,PRIMARY KEY(origin,invocation));
            CREATE TABLE IF NOT EXISTS calls(id TEXT PRIMARY KEY, origin TEXT NOT NULL, invocation TEXT NOT NULL, subject TEXT NOT NULL, server TEXT NOT NULL, tool TEXT NOT NULL, digest TEXT NOT NULL, state TEXT NOT NULL, result TEXT, created INTEGER NOT NULL, UNIQUE(origin,invocation));
            CREATE TABLE IF NOT EXISTS routes(origin TEXT NOT NULL, invocation TEXT NOT NULL, subject TEXT NOT NULL, digest TEXT NOT NULL, owner TEXT NOT NULL, request TEXT NOT NULL, call TEXT, PRIMARY KEY(origin,invocation));
            CREATE UNIQUE INDEX IF NOT EXISTS route_call_owner ON routes(subject,call) WHERE call IS NOT NULL;
            UPDATE calls SET state='outcome_unknown' WHERE state IN ('dispatching','running','cancel_requested');
            UPDATE calls SET state='not_dispatched' WHERE state='accepted';")?;
        Ok(Self(
            Mutex::new(conn),
            zork_notify::Notifier::default(),
            zork_notify::Notifier::default(),
        ))
    }
    pub fn servers(&self) -> Result<Vec<Server>> {
        let conn = self.0.lock().expect("mcp db");
        let mut stmt = conn.prepare("SELECT value FROM servers ORDER BY id")?;
        let values = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        values
            .into_iter()
            .map(|v| Ok(serde_json::from_str(&v)?))
            .collect()
    }
    pub fn server(&self, id: &str) -> Result<Server> {
        self.servers()?
            .into_iter()
            .find(|s| s.id == id)
            .context("mcp_not_found")
    }
    pub fn save(
        &self,
        config: ServerInput,
        id: Option<&str>,
        expected: Option<&str>,
    ) -> Result<Server> {
        config.validate()?;
        let mut conn = self.0.lock().expect("mcp db");
        let tx = conn.transaction()?;
        if let Some(id) = id {
            valid_id(id)?;
            let old: String = tx
                .query_row("SELECT value FROM servers WHERE id=?1", [id], |r| r.get(0))
                .optional()?
                .context("mcp_not_found")?;
            ensure!(
                Some(serde_json::from_str::<Server>(&old)?.revision.as_str()) == expected,
                "mcp_revision_conflict"
            );
        } else {
            let count: usize = tx.query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))?;
            ensure!(count < 64, "mcp_server_limit");
        }
        let server = Server {
            id: id.map(str::to_owned).unwrap_or_else(new_id),
            revision: new_id(),
            config,
        };
        tx.execute("INSERT INTO servers(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value", params![server.id, serde_json::to_string(&server)?])?;
        tx.commit()?;
        self.2.notify();
        self.notify();
        Ok(server)
    }
    pub fn remove(&self, id: &str, revision: &str) -> Result<()> {
        let mut conn = self.0.lock().expect("mcp db");
        let tx = conn.transaction()?;
        let old: String = tx
            .query_row("SELECT value FROM servers WHERE id=?1", [id], |r| r.get(0))
            .optional()?
            .context("mcp_not_found")?;
        ensure!(
            serde_json::from_str::<Server>(&old)?.revision == revision,
            "mcp_revision_conflict"
        );
        tx.execute("DELETE FROM servers WHERE id=?1", [id])?;
        tx.commit()?;
        self.2.notify();
        self.notify();
        Ok(())
    }
    pub fn existing(
        &self,
        who: &Subject,
        invocation: &str,
        digest: &str,
    ) -> Result<Option<String>> {
        let found = self
            .0
            .lock()
            .expect("mcp db")
            .query_row(
                "SELECT id,digest FROM calls WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        found
            .map(|(id, old)| {
                ensure!(old == digest, "mcp_invocation_conflict");
                Ok(id)
            })
            .transpose()
    }
    pub fn submit(
        &self,
        who: &Subject,
        invocation: &str,
        server: &str,
        tool: &str,
        digest: &str,
    ) -> Result<(String, bool)> {
        let mut conn = self.0.lock().expect("mcp db");
        let tx = conn.transaction()?;
        if let Some((id, previous)) = tx
            .query_row(
                "SELECT id,digest FROM calls WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(previous == digest, "mcp_invocation_conflict");
            return Ok((id, false));
        }
        let count: usize = tx.query_row("SELECT COUNT(*) FROM calls", [], |r| r.get(0))?;
        ensure!(count < 100_000, "mcp_call_history_limit");
        let id = new_id();
        tx.execute("INSERT INTO calls(id,origin,invocation,subject,server,tool,digest,state,created) VALUES(?1,?2,?3,?4,?5,?6,?7,'accepted',?8)", params![id,who.origin,invocation,serde_json::to_string(who)?,server,tool,digest,now()])?;
        tx.commit()?;
        Ok((id, true))
    }
    pub fn transition(&self, id: &str, state: &str, result: Option<Value>) -> Result<()> {
        let conn = self.0.lock().expect("mcp db");
        // Keep tombstones: expiration never permits an old request to execute again.
        conn.execute(
            "UPDATE calls SET result=NULL WHERE state NOT IN ('accepted','dispatching','running') AND created < ?1",
            [now() - 7 * 86400],
        )?;
        let result = result.map(|v| serde_json::to_string(&v)).transpose()?;
        let size: i64 = conn.query_row(
            "SELECT COALESCE(SUM(length(CAST(result AS BLOB))),0) FROM calls",
            [],
            |r| r.get(0),
        )?;
        if result
            .as_ref()
            .is_some_and(|r| size + r.len() as i64 > 64 * 1024 * 1024)
        {
            conn.execute(
                "UPDATE calls SET state='result_unavailable', result=NULL WHERE id=?1",
                [id],
            )?;
        } else {
            conn.execute(
                "UPDATE calls SET state=?2,result=?3 WHERE id=?1",
                params![id, state, result],
            )?;
        }
        self.notify();
        Ok(())
    }
    /// Ownership and policy metadata without loading a potentially large result.
    pub fn call_access(&self, id: &str, who: &Subject) -> Result<(String, String)> {
        valid_id(id)?;
        let (subject, server, tool): (String, String, String) = self
            .0
            .lock()
            .expect("mcp db")
            .query_row(
                "SELECT subject,server,tool FROM calls WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?
            .context("mcp_not_found")?;
        ensure!(
            serde_json::from_str::<Subject>(&subject)? == *who,
            "mcp_access_denied"
        );
        Ok((server, tool))
    }
    pub fn call_tool(&self, id: &str) -> Result<String> {
        Ok(self.0.lock().expect("mcp db").query_row(
            "SELECT tool FROM calls WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn call(&self, id: &str, who: &Subject) -> Result<(String, String, Option<String>)> {
        valid_id(id)?;
        let conn = self.0.lock().expect("mcp db");
        let (subject, server, state, result, created): (
            String,
            String,
            String,
            Option<String>,
            i64,
        ) = conn
            .query_row(
                "SELECT subject,server,state,result,created FROM calls WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?
            .context("mcp_not_found")?;
        ensure!(
            serde_json::from_str::<Subject>(&subject)? == *who,
            "mcp_access_denied"
        );
        if created < now() - 7 * 86400
            && !matches!(state.as_str(), "accepted" | "dispatching" | "running")
        {
            return Ok((server, "expired".into(), None));
        }
        Ok((server, state, result))
    }
    pub fn route(
        &self,
        who: &Subject,
        invocation: &str,
        digest: &str,
        owner: &str,
        request: &Operation,
    ) -> Result<bool> {
        let conn = self.0.lock().expect("mcp db");
        ensure!(
            !conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM cancellations WHERE origin=?1 AND invocation=?2)",
                params![who.origin, invocation],
                |row| row.get::<_, bool>(0)
            )?,
            "tool_cancelled"
        );
        let mut revive = false;
        if let Some((old, rejected)) = conn
            .query_row(
                "SELECT digest,(call IS NULL AND request='') FROM routes WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?,r.get::<_,bool>(1)?)),
            )
            .optional()?
        {
            ensure!(old == digest, "mcp_invocation_conflict");
            if !rejected {return Ok(false);}
            revive=true;
        }
        let body = serde_json::to_string(request)?;
        ensure!(body.len() <= 128 * 1024, "mcp_request_limit");
        let (count, size): (usize, usize) = conn.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(request AS BLOB))),0) FROM routes",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            (revive || count < 100_000) && size + body.len() <= 32 * 1024 * 1024,
            "mcp_outbox_limit"
        );
        if revive {
            conn.execute(
                "UPDATE routes SET request=?3 WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation, body],
            )?;
            return Ok(true);
        }
        conn.execute("INSERT INTO routes(origin,invocation,subject,digest,owner,request) VALUES(?1,?2,?3,?4,?5,?6)", params![who.origin,invocation,serde_json::to_string(who)?,digest,owner,body])?;
        Ok(true)
    }
    pub fn reject_route(&self, who: &Subject, invocation: &str) -> Result<()> {
        self.0.lock().expect("mcp db").execute(
            "UPDATE routes SET request='' WHERE origin=?1 AND invocation=?2 AND call IS NULL",
            params![who.origin, invocation],
        )?;
        Ok(())
    }
    pub fn pending(&self, who: &Subject) -> Result<Vec<(String, String, Operation)>> {
        let conn = self.0.lock().expect("mcp db");
        let mut stmt = conn.prepare("SELECT invocation,owner,request FROM routes WHERE subject=?1 AND call IS NULL AND request!='' LIMIT 16")?;
        let rows = stmt
            .query_map([serde_json::to_string(who)?], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, owner, body)| Ok((id, owner, serde_json::from_str(&body)?)))
            .collect()
    }
    pub fn bind_route(&self, who: &Subject, invocation: &str, id: &str) -> Result<()> {
        valid_id(id)?;
        self.0.lock().expect("mcp db").execute(
            "UPDATE routes SET call=?3 WHERE origin=?1 AND invocation=?2",
            params![who.origin, invocation, id],
        )?;
        Ok(())
    }
    pub fn owner(&self, who: &Subject, id: &str) -> Result<String> {
        self.0
            .lock()
            .expect("mcp db")
            .query_row(
                "SELECT owner FROM routes WHERE subject=?1 AND call=?2",
                params![serde_json::to_string(who)?, id],
                |r| r.get(0),
            )
            .optional()?
            .context("mcp_not_found")
    }
}

impl Store {
    pub fn management_receipt(
        &self,
        who: &Subject,
        invocation: &str,
        digest: &str,
    ) -> Result<Option<Value>> {
        let row = self
            .0
            .lock()
            .expect("mcp db")
            .query_row(
                "SELECT digest,result FROM management WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        row.map(|(old, result)| {
            ensure!(old == digest, "mcp_invocation_conflict");
            Ok(serde_json::from_str(&result)?)
        })
        .transpose()
    }
    pub fn manage(
        &self,
        who: &Subject,
        invocation: &str,
        fingerprint: &str,
        owner: &str,
        request: &Operation,
        prepared: Option<ServerInput>,
    ) -> Result<Value> {
        let mut conn = self.0.lock().expect("mcp db");
        let tx = conn.transaction()?;
        ensure!(
            !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM cancellations WHERE origin=?1 AND invocation=?2)",
                params![who.origin, invocation],
                |row| row.get::<_, bool>(0)
            )?,
            "tool_cancelled"
        );
        if let Some((old, result)) = tx
            .query_row(
                "SELECT digest,result FROM management WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(old == fingerprint, "mcp_invocation_conflict");
            return Ok(serde_json::from_str(&result)?);
        }
        let count: usize = tx.query_row("SELECT COUNT(*) FROM management", [], |r| r.get(0))?;
        ensure!(count < 100_000, "mcp_management_history_limit");
        let mut server = if request.op == "install" {
            let count: usize = tx.query_row("SELECT COUNT(*) FROM servers", [], |r| r.get(0))?;
            ensure!(count < 64, "mcp_server_limit");
            Server {
                id: new_id(),
                revision: new_id(),
                config: prepared.clone().context("mcp_missing_config")?,
            }
        } else {
            let id = &request
                .server_ref
                .as_ref()
                .context("mcp_missing_server")?
                .server_id;
            valid_id(id)?;
            let value: String = tx
                .query_row("SELECT value FROM servers WHERE id=?1", [id], |r| r.get(0))
                .optional()?
                .context("mcp_not_found")?;
            let server: Server = serde_json::from_str(&value)?;
            ensure!(
                Some(server.revision.as_str()) == request.expected_revision.as_deref(),
                "mcp_revision_conflict"
            );
            server
        };
        match request.op.as_str() {
            "install" => {}
            "update" => server.config = prepared.context("mcp_missing_config")?,
            "uninstall" => {}
            _ => anyhow::bail!("mcp_invalid_operation"),
        }
        server.config.validate()?;
        if request.op == "uninstall" {
            tx.execute("DELETE FROM servers WHERE id=?1", [&server.id])?;
        } else {
            server.revision = new_id();
            tx.execute("INSERT INTO servers(id,value) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET value=excluded.value",params![server.id,serde_json::to_string(&server)?])?;
        }
        let mut result = server.descriptor(owner);
        result["operation_id"] = json!(new_id());
        result["state"] = json!("succeeded");
        result["op"] = json!(request.op);
        result["enabled"] = json!(server.config.enabled);
        if request.op == "uninstall" {
            result["availability"] = json!("removed");
        }
        result["next_step"] = json!(if matches!(request.op.as_str(), "install" | "update") {
            Some("inspect")
        } else {
            None
        });
        tx.execute(
            "INSERT INTO management(origin,invocation,digest,result) VALUES(?1,?2,?3,?4)",
            params![
                who.origin,
                invocation,
                fingerprint,
                serde_json::to_string(&result)?
            ],
        )?;
        tx.commit()?;
        Ok(result)
    }
}

impl Store {
    pub fn invocation_route(
        &self,
        who: &Subject,
        invocation: &str,
    ) -> Result<Option<(String, Operation)>> {
        self.0
            .lock()
            .expect("mcp db")
            .query_row(
                "SELECT owner,request FROM routes WHERE subject=?1 AND invocation=?2",
                params![serde_json::to_string(who)?, invocation],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(owner, body)| Ok((owner, serde_json::from_str(&body)?)))
            .transpose()
    }
}
