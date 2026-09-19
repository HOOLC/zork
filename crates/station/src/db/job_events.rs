//! A job result and its pending Session notification commit together. Receipts
//! retire only the outbox entry; the job record and Agent history remain intact.
use super::*;
use crate::jobs::JobEvent;

fn enqueue(conn: &Connection, event: &JobEvent) -> Result<()> {
    conn.execute(
        "INSERT INTO job_mailbox(session_key,event_json) VALUES(?1,?2)",
        params![event.session_key, serde_json::to_string(event)?],
    )?;
    conn.execute(
        "UPDATE background_jobs SET last_event_at=?1,last_event_kind=?2,last_event_summary=?3 WHERE id=?4",
        params![now_rfc3339(), event.event_kind, event.summary, event.job_id],
    )?;
    Ok(())
}

impl StationDb {
    pub fn job_event_source(&self) -> Result<String> {
        Ok(self.conn.lock().expect("db mutex").query_row(
            "SELECT 'background-jobs-' || value FROM chat_metadata WHERE key='epoch'",
            [],
            |row| row.get(0),
        )?)
    }

    pub fn queue_job_event(&self, event: &JobEvent) -> Result<()> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        enqueue(&tx, event)?;
        tx.commit()?;
        self.realtime.notify(crate::realtime::JOB_EVENTS);
        Ok(())
    }

    /// First terminal observation wins, including timeout/exit/cancel races.
    pub fn finish_job(&self, event: &JobEvent, succeeded: bool) -> Result<bool> {
        let mut conn = self.conn.lock().expect("db mutex");
        let tx = conn.transaction()?;
        let changed = tx.execute(
            "UPDATE background_jobs SET status=?1,error=?2,completed_at=?3,updated_at=?3
             WHERE id=?4 AND status IN ('registered','running')",
            params![
                if succeeded { "succeeded" } else { "failed" },
                (!succeeded).then_some(&event.summary),
                now_rfc3339(),
                event.job_id,
            ],
        )? > 0;
        if changed {
            enqueue(&tx, event)?;
        }
        tx.commit()?;
        if changed {
            self.realtime.notify(crate::realtime::JOB_EVENTS);
        }
        Ok(changed)
    }

    pub fn pending_job_events(&self) -> Result<Vec<(u64, JobEvent)>> {
        let conn = self.conn.lock().expect("db mutex");
        // Preserve order within a Session without one broken Session hiding
        // another Session's result behind its own backlog.
        let rows = conn
            .prepare(
                "SELECT sequence,event_json FROM job_mailbox WHERE sequence IN
             (SELECT MIN(sequence) FROM job_mailbox GROUP BY session_key)
             ORDER BY sequence LIMIT 64",
            )?
            .query_map([], |row| {
                Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(seq, json)| Ok((seq, serde_json::from_str(&json)?)))
            .collect()
    }

    pub fn acknowledge_job_event(&self, sequence: u64) -> Result<()> {
        self.conn
            .lock()
            .expect("db mutex")
            .execute("DELETE FROM job_mailbox WHERE sequence=?1", [sequence])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(job: &str, session: &str) -> JobEvent {
        JobEvent {
            session_key: session.into(),
            job_id: job.into(),
            kind: "test".into(),
            event_kind: "job_exit".into(),
            summary: "result".into(),
        }
    }

    #[test]
    fn failed_outbox_insert_rolls_back_terminal_state_and_recovery_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        let job = JobRow {
            id: "job".into(),
            token: "fixture".into(),
            session_key: "session-a".into(),
            kind: "test".into(),
            shell: "sh".into(),
            cwd: "/unused".into(),
            script_path: "/unused".into(),
            restart_on_boot: false,
            status: "running".into(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        };
        db.insert_job(&job).unwrap();
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_outbox BEFORE INSERT ON job_mailbox BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert!(db.finish_job(&event("job", "session-a"), true).is_err());
        assert_eq!(db.get_job("job").unwrap().unwrap().status, "running");
        assert!(db.pending_job_events().unwrap().is_empty());
        db.conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_outbox")
            .unwrap();
        assert!(db.finish_job(&event("job", "session-a"), true).unwrap());
        assert!(!db.finish_job(&event("job", "session-a"), false).unwrap());
        db.queue_job_event(&event("notify", "session-a")).unwrap();
        db.queue_job_event(&event("notify", "session-b")).unwrap();
        let source = db.job_event_source().unwrap();
        let first = db.pending_job_events().unwrap();
        assert_eq!(
            first
                .iter()
                .map(|(_, e)| e.session_key.as_str())
                .collect::<Vec<_>>(),
            ["session-a", "session-b"]
        );
        drop(db);
        let db = StationDb::open(dir.path(), &dir.path().join("workspaces")).unwrap();
        assert_eq!(db.job_event_source().unwrap(), source);
        assert_eq!(db.get_job("job").unwrap().unwrap().status, "succeeded");
        assert_eq!(db.pending_job_events().unwrap()[0].0, first[0].0);
        db.acknowledge_job_event(first[0].0).unwrap();
        let second = db.pending_job_events().unwrap();
        assert!(second[0].0 > first[0].0);
        assert_eq!(second[0].1.job_id, "notify");
    }
}
