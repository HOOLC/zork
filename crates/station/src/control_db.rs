use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::config::now_rfc3339;
use crate::db::GATEWAY_DB;

const BUSY_TIMEOUT_MS: u32 = 5_000;

pub struct ControlDb {
    conn: Mutex<Connection>,
}

impl ControlDb {
    pub fn attach_realtime(&self, events: &crate::realtime::Realtime) {
        events.install(&self.conn.lock().expect("db mutex"));
    }

    pub fn open(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir).context("create state dir")?;
        let path = state_dir.join(GATEWAY_DB);
        let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_millis(BUSY_TIMEOUT_MS as u64))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("db mutex");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS admin_operations (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              kind TEXT NOT NULL,
              status TEXT NOT NULL,
              request TEXT NOT NULL,
              result TEXT,
              error TEXT,
              actor TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              started_at TEXT,
              completed_at TEXT
            );
            CREATE TABLE IF NOT EXISTS admin_audit_events (
              sequence INTEGER PRIMARY KEY AUTOINCREMENT,
              operation_id INTEGER REFERENCES admin_operations(id) ON DELETE SET NULL,
              action TEXT NOT NULL,
              status TEXT NOT NULL,
              detail TEXT,
              actor TEXT,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_admin_operations_updated ON admin_operations(updated_at);
            CREATE INDEX IF NOT EXISTS idx_admin_audit_created ON admin_audit_events(sequence);
            CREATE INDEX IF NOT EXISTS idx_admin_audit_operation ON admin_audit_events(operation_id, sequence);
            "#,
        )?;
        Ok(())
    }

    pub fn list_operations(&self, limit: i64) -> Result<Vec<Value>> {
        let conn = self.conn.lock().expect("db mutex");
        let mut stmt = conn.prepare(
            "SELECT id, kind, status, request, result, error, actor, created_at, updated_at, started_at, completed_at
             FROM admin_operations ORDER BY updated_at DESC, created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_operation)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list operations")
    }

    pub fn list_audit(&self, operation_id: Option<i64>, limit: i64) -> Result<Vec<Value>> {
        let conn = self.conn.lock().expect("db mutex");
        if let Some(operation_id) = operation_id {
            let mut stmt = conn.prepare(
                "SELECT sequence, operation_id, action, status, detail, actor, created_at
                 FROM admin_audit_events WHERE operation_id = ?1 ORDER BY sequence DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![operation_id, limit], row_to_audit)?;
            return rows
                .collect::<rusqlite::Result<Vec<_>>>()
                .context("list audit");
        }
        let mut stmt = conn.prepare(
            "SELECT sequence, operation_id, action, status, detail, actor, created_at
             FROM admin_audit_events ORDER BY sequence DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_audit)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("list audit")
    }

    pub fn record_operation(
        &self,
        kind: &str,
        request: &Value,
        result: Result<Value, String>,
    ) -> Result<Value> {
        let now = now_rfc3339();
        let (status, result_json, error) = match result {
            Ok(value) => ("succeeded", Some(value), None),
            Err(error) => ("failed", None, Some(error)),
        };
        let conn = self.conn.lock().expect("db mutex");
        conn.execute(
            "INSERT INTO admin_operations (kind, status, request, result, error, created_at, updated_at, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6, ?6)",
            params![
                kind,
                status,
                request.to_string(),
                result_json.as_ref().map(Value::to_string),
                error,
                now
            ],
        )?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO admin_audit_events (operation_id, action, status, detail, created_at)
             VALUES (?1, ?2, 'started', NULL, ?3)",
            params![id, kind, now],
        )?;
        conn.execute(
            "INSERT INTO admin_audit_events (operation_id, action, status, detail, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, kind, status, error.as_deref(), now],
        )?;
        drop(conn);
        self.get_operation(id)?
            .context("operation missing after insert")
    }

    fn get_operation(&self, id: i64) -> Result<Option<Value>> {
        let conn = self.conn.lock().expect("db mutex");
        conn.query_row(
            "SELECT id, kind, status, request, result, error, actor, created_at, updated_at, started_at, completed_at
             FROM admin_operations WHERE id = ?1",
            [id],
            row_to_operation,
        )
        .optional()
        .context("get operation")
    }
}

fn row_to_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, i64>(0)?,
        "kind": row.get::<_, String>(1)?,
        "status": row.get::<_, String>(2)?,
        "request": parse_json_or_null(row.get::<_, String>(3)?),
        "result": row.get::<_, Option<String>>(4)?.map(parse_json_or_null),
        "error": row.get::<_, Option<String>>(5)?,
        "actor": row.get::<_, Option<String>>(6)?,
        "createdAt": row.get::<_, String>(7)?,
        "updatedAt": row.get::<_, String>(8)?,
        "startedAt": row.get::<_, Option<String>>(9)?,
        "completedAt": row.get::<_, Option<String>>(10)?,
    }))
}

fn row_to_audit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, i64>(0)?,
        "operationId": row.get::<_, Option<i64>>(1)?,
        "action": row.get::<_, String>(2)?,
        "status": row.get::<_, String>(3)?,
        "detail": row.get::<_, Option<String>>(4)?,
        "actor": row.get::<_, Option<String>>(5)?,
        "createdAt": row.get::<_, String>(6)?,
    }))
}

fn parse_json_or_null(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_records_use_station_sequence_identity_in_station_database() {
        let root = tempfile::tempdir().expect("temporary state directory");
        let db = ControlDb::open(root.path()).expect("control db");

        let operation = db
            .record_operation("test", &json!({}), Ok(json!({ "ok": true })))
            .expect("record operation");
        let audit = db.list_audit(None, 10).expect("list audit");

        assert_eq!(operation["id"], 1);
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0]["id"], 2);
        assert_eq!(audit[1]["id"], 1);
        assert_eq!(audit[0]["operationId"], 1);
        assert!(root.path().join("station.sqlite").exists());
        assert!(!root.path().join("control.sqlite").exists());
    }
}
