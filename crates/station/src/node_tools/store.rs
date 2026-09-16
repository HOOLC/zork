use super::*;
use rusqlite::{params, Connection, OptionalExtension};
pub struct Store(Mutex<Connection>, zork_notify::Notifier);
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
    pub fn notify(&self) {
        self.1.notify();
    }
    pub fn open(root: &std::path::Path) -> Result<Self> {
        let c = Connection::open(root.join("node-tools.sqlite"))?;
        c.busy_timeout(Duration::from_secs(5))?;
        c.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS cancellations(origin TEXT NOT NULL,invocation TEXT NOT NULL,PRIMARY KEY(origin,invocation));
        CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY,origin TEXT NOT NULL,invocation TEXT NOT NULL,subject TEXT NOT NULL,fingerprint TEXT NOT NULL,tool TEXT NOT NULL,state TEXT NOT NULL,result TEXT,UNIQUE(origin,invocation));
        CREATE TABLE IF NOT EXISTS outbox(origin TEXT NOT NULL,invocation TEXT NOT NULL,subject TEXT NOT NULL,fingerprint TEXT NOT NULL,target TEXT NOT NULL,rpc TEXT NOT NULL,id TEXT,status TEXT NOT NULL DEFAULT 'pending',PRIMARY KEY(origin,invocation));
        CREATE UNIQUE INDEX IF NOT EXISTS outbox_receipt ON outbox(subject,id) WHERE id IS NOT NULL;
        UPDATE operations SET state='outcome_unknown',result=CASE WHEN tool='device.exec' THEN json_set(COALESCE(result,'{}'),'$.process_state','unknown','$.effects_may_have_occurred',json('true')) ELSE result END WHERE state IN ('dispatching','running');
        UPDATE operations SET state='not_dispatched' WHERE state='accepted';")?;
        let columns = c
            .prepare("PRAGMA table_info(outbox)")?
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|c| c == "status") {
            c.execute(
                "ALTER TABLE outbox ADD COLUMN status TEXT NOT NULL DEFAULT 'pending'",
                [],
            )?;
        }
        Ok(Self(Mutex::new(c), zork_notify::Notifier::default()))
    }
    pub fn existing(&self, who: &Subject, invocation: &str, hash: &str) -> Result<Option<String>> {
        self.0
            .lock()
            .expect("node db")
            .query_row(
                "SELECT id,fingerprint FROM operations WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(id, old)| {
                ensure!(old == hash, "device_invocation_conflict");
                Ok(id)
            })
            .transpose()
    }
    pub fn accept(&self, rpc: &Rpc, hash: &str) -> Result<(String, bool)> {
        let mut c = self.0.lock().expect("node db");
        let tx = c.transaction()?;
        if let Some((id, old)) = tx
            .query_row(
                "SELECT id,fingerprint FROM operations WHERE origin=?1 AND invocation=?2",
                params![rpc.subject.origin, rpc.invocation_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(old == hash, "device_invocation_conflict");
            return Ok((id, false));
        }
        let count: usize = tx.query_row("SELECT COUNT(*) FROM operations", [], |r| r.get(0))?;
        ensure!(count < 10000, "device_history_limit");
        let id = id();
        tx.execute("INSERT INTO operations(id,origin,invocation,subject,fingerprint,tool,state) VALUES(?1,?2,?3,?4,?5,?6,'accepted')",params![id,rpc.subject.origin,rpc.invocation_id,serde_json::to_string(&rpc.subject)?,hash,rpc.tool])?;
        tx.commit()?;
        Ok((id, true))
    }
    pub fn finish(&self, id: &str, state: &str, result: Option<Value>) -> Result<()> {
        self.0.lock().expect("node db").execute(
            "UPDATE operations SET state=?2,result=?3 WHERE id=?1",
            params![
                id,
                state,
                result.map(|v| serde_json::to_string(&v)).transpose()?
            ],
        )?;
        self.notify();
        Ok(())
    }
    pub fn view(&self, id: &str, who: &Subject) -> Result<Value> {
        valid_id(id)?;
        let (subject, tool, state, result): (String, String, String, Option<String>) = self
            .0
            .lock()
            .expect("node db")
            .query_row(
                "SELECT subject,tool,state,result FROM operations WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .context("device_operation_not_found")?;
        ensure!(
            serde_json::from_str::<Subject>(&subject)? == *who,
            "device_operation_not_owned"
        );
        Ok(
            json!({"operation_id":id,"tool":tool,"state":state,"result":result.map(|v|serde_json::from_str::<Value>(&v)).transpose()?}),
        )
    }
    pub fn enqueue(
        &self,
        who: &Subject,
        invocation: &str,
        hash: &str,
        target: &str,
        rpc: &Rpc,
    ) -> Result<bool> {
        let mut c = self.0.lock().expect("node db");
        let tx = c.transaction()?;
        ensure!(
            !tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM cancellations WHERE origin=?1 AND invocation=?2)",
                params![who.origin, invocation],
                |row| row.get::<_, bool>(0)
            )?,
            "tool_cancelled"
        );
        if let Some((old, status)) = tx
            .query_row(
                "SELECT fingerprint,status FROM outbox WHERE origin=?1 AND invocation=?2",
                params![who.origin, invocation],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            ensure!(old == hash, "device_invocation_conflict");
            if status == "rejected" {
                tx.execute(
                    "UPDATE outbox SET status='pending' WHERE origin=?1 AND invocation=?2",
                    params![who.origin, invocation],
                )?;
                tx.commit()?;
                return Ok(true);
            }
            return Ok(false);
        }
        let (count, size): (usize, usize) = tx.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(rpc AS BLOB))),0) FROM outbox",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let body = serde_json::to_string(rpc)?;
        ensure!(
            body.len() <= 192 * 1024 && count < 10000 && size + body.len() < 32 * 1024 * 1024,
            "device_outbox_limit"
        );
        tx.execute("INSERT OR IGNORE INTO outbox(origin,invocation,subject,fingerprint,target,rpc) VALUES(?1,?2,?3,?4,?5,?6)",params![who.origin,invocation,serde_json::to_string(who)?,hash,target,body])?;
        let old: String = tx.query_row(
            "SELECT fingerprint FROM outbox WHERE origin=?1 AND invocation=?2",
            params![who.origin, invocation],
            |r| r.get(0),
        )?;
        ensure!(old == hash, "device_invocation_conflict");
        tx.commit()?;
        Ok(true)
    }
    pub fn reject(&self, who: &Subject, invocation: &str) -> Result<()> {
        self.0.lock().expect("node db").execute(
            "UPDATE outbox SET status='rejected' WHERE origin=?1 AND invocation=?2 AND id IS NULL",
            params![who.origin, invocation],
        )?;
        Ok(())
    }
    pub fn ack(&self, who: &Subject, invocation: &str, id: &str) -> Result<()> {
        valid_id(id)?;
        self.0.lock().expect("node db").execute(
            "UPDATE outbox SET id=?3,status='acknowledged' WHERE origin=?1 AND invocation=?2",
            params![who.origin, invocation, id],
        )?;
        Ok(())
    }
    pub fn target(&self, who: &Subject, id: &str) -> Result<String> {
        self.0
            .lock()
            .expect("node db")
            .query_row(
                "SELECT target FROM outbox WHERE subject=?1 AND id=?2",
                params![serde_json::to_string(who)?, id],
                |r| r.get(0),
            )
            .optional()?
            .context("device_operation_not_found")
    }
}

impl Store {
    pub fn invocation_route(
        &self,
        who: &Subject,
        invocation: &str,
    ) -> Result<Option<(String, Rpc)>> {
        self.0
            .lock()
            .expect("node db")
            .query_row(
                "SELECT target,rpc FROM outbox WHERE subject=?1 AND invocation=?2",
                params![serde_json::to_string(who)?, invocation],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .map(|(target, body)| Ok((target, serde_json::from_str(&body)?)))
            .transpose()
    }
}
