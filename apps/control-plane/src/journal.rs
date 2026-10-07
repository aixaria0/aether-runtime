use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{evidence::ExecutionReceipt, scheduler::ExecutorStats};

#[derive(Clone, Debug)]
pub struct Journal {
    path: PathBuf,
}

type ExecutionRow = (String, Option<String>, String, String, String, String, i64);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredExecution {
    pub task_id: String,
    pub parent_task_id: Option<String>,
    pub operation: String,
    pub payload: String,
    pub output: String,
    pub receipt: ExecutionReceipt,
    pub created_at_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEvent {
    pub sequence: i64,
    pub event_id: String,
    pub task_id: String,
    pub event_type: String,
    pub timestamp_ns: u64,
    pub payload_json: String,
    pub previous_hash: Option<String>,
    pub event_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalVerification {
    pub valid: bool,
    pub event_count: u64,
    pub head_hash: Option<String>,
    pub first_invalid_sequence: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct ExecutionSnapshot {
    pub execution: StoredExecution,
    pub parent: Option<StoredExecution>,
    pub events: Vec<JournalEvent>,
    pub chain: JournalVerification,
}

impl Journal {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let journal = Self { path: path.into() };
        journal.initialize()?;
        Ok(journal)
    }

    fn connect(&self) -> Result<Connection, String> {
        let conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
        conn.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(|e| e.to_string())?;
        Ok(conn)
    }

    fn initialize(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() && parent != Path::new(".") {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
        }
        let conn = self.connect()?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        conn.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS executions (
                task_id TEXT PRIMARY KEY,
                parent_task_id TEXT,
                operation TEXT NOT NULL,
                payload TEXT NOT NULL,
                output TEXT NOT NULL,
                receipt_json TEXT NOT NULL,
                created_at_ns INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS execution_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                event_id TEXT NOT NULL UNIQUE,
                task_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                timestamp_ns INTEGER NOT NULL,
                payload_json TEXT NOT NULL,
                previous_hash TEXT,
                event_hash TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_execution_events_task
                ON execution_events(task_id, sequence);

            CREATE TABLE IF NOT EXISTS executor_stats (
                executor_id TEXT PRIMARY KEY,
                successes INTEGER NOT NULL,
                failures INTEGER NOT NULL,
                consecutive_failures INTEGER NOT NULL,
                ewma_latency_ms REAL NOT NULL,
                quarantined_until_ns INTEGER NOT NULL
            );
            "#,
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn append_event(
        &self,
        task_id: &str,
        event_type: &str,
        payload: BTreeMap<String, String>,
    ) -> Result<JournalEvent, String> {
        let mut conn = self.connect()?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let previous_hash: Option<String> = tx
            .query_row(
                "SELECT event_hash FROM execution_events ORDER BY sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let timestamp_ns = now_ns();
        let event_id = Uuid::new_v4().to_string();
        let payload_json = serde_json::to_string(&payload).map_err(|e| e.to_string())?;
        let event_hash = hash_event(
            previous_hash.as_deref(),
            &event_id,
            task_id,
            event_type,
            timestamp_ns,
            &payload_json,
        );

        tx.execute(
            "INSERT INTO execution_events(event_id, task_id, event_type, timestamp_ns, payload_json, previous_hash, event_hash) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event_id,
                task_id,
                event_type,
                timestamp_ns as i64,
                payload_json,
                previous_hash,
                event_hash,
            ],
        )
        .map_err(|e| e.to_string())?;
        let sequence = tx.last_insert_rowid();
        tx.commit().map_err(|e| e.to_string())?;

        Ok(JournalEvent {
            sequence,
            event_id,
            task_id: task_id.to_string(),
            event_type: event_type.to_string(),
            timestamp_ns,
            payload_json,
            previous_hash,
            event_hash,
        })
    }

    pub fn save_execution(&self, record: &StoredExecution) -> Result<(), String> {
        let conn = self.connect()?;
        let receipt_json = serde_json::to_string(&record.receipt).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO executions(task_id, parent_task_id, operation, payload, output, receipt_json, created_at_ns) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.task_id,
                record.parent_task_id,
                record.operation,
                record.payload,
                record.output,
                receipt_json,
                record.created_at_ns as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn load_execution(&self, task_id: &str) -> Result<Option<StoredExecution>, String> {
        let conn = self.connect()?;
        Self::read_execution(&conn, task_id)
    }

    fn read_execution(conn: &Connection, task_id: &str) -> Result<Option<StoredExecution>, String> {
        let row: Option<ExecutionRow> = conn
            .query_row(
                "SELECT task_id, parent_task_id, operation, payload, output, receipt_json, created_at_ns FROM executions WHERE task_id = ?1",
                [task_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;

        row.map(
            |(task_id, parent_task_id, operation, payload, output, receipt_json, created_at_ns)| {
                let receipt = serde_json::from_str(&receipt_json).map_err(|e| e.to_string())?;
                Ok(StoredExecution {
                    task_id,
                    parent_task_id,
                    operation,
                    payload,
                    output,
                    receipt,
                    created_at_ns: created_at_ns as u64,
                })
            },
        )
        .transpose()
    }

    pub fn events_for_task(&self, task_id: &str) -> Result<Vec<JournalEvent>, String> {
        let conn = self.connect()?;
        Self::read_events(&conn, task_id)
    }

    fn read_events(conn: &Connection, task_id: &str) -> Result<Vec<JournalEvent>, String> {
        let mut stmt = conn
            .prepare(
                "SELECT sequence, event_id, task_id, event_type, timestamp_ns, payload_json, previous_hash, event_hash FROM execution_events WHERE task_id = ?1 ORDER BY sequence ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([task_id], |row| {
                Ok(JournalEvent {
                    sequence: row.get(0)?,
                    event_id: row.get(1)?,
                    task_id: row.get(2)?,
                    event_type: row.get(3)?,
                    timestamp_ns: row.get::<_, i64>(4)? as u64,
                    payload_json: row.get(5)?,
                    previous_hash: row.get(6)?,
                    event_hash: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn load_executor_stats(&self) -> Result<Vec<ExecutorStats>, String> {
        let conn = self.connect()?;
        let mut stmt = conn
            .prepare("SELECT executor_id, successes, failures, consecutive_failures, ewma_latency_ms, quarantined_until_ns FROM executor_stats ORDER BY executor_id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ExecutorStats {
                    executor_id: row.get(0)?,
                    successes: row.get::<_, i64>(1)? as u64,
                    failures: row.get::<_, i64>(2)? as u64,
                    consecutive_failures: row.get::<_, i64>(3)? as u32,
                    ewma_latency_ms: row.get(4)?,
                    quarantined_until_ns: row.get::<_, i64>(5)? as u64,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn save_executor_stats(&self, stats: &ExecutorStats) -> Result<(), String> {
        let conn = self.connect()?;
        conn.execute(
            "INSERT INTO executor_stats(executor_id, successes, failures, consecutive_failures, ewma_latency_ms, quarantined_until_ns) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(executor_id) DO UPDATE SET successes=excluded.successes, failures=excluded.failures, consecutive_failures=excluded.consecutive_failures, ewma_latency_ms=excluded.ewma_latency_ms, quarantined_until_ns=excluded.quarantined_until_ns",
            params![
                stats.executor_id,
                stats.successes as i64,
                stats.failures as i64,
                stats.consecutive_failures as i64,
                stats.ewma_latency_ms,
                stats.quarantined_until_ns as i64,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn verify_chain(&self) -> Result<JournalVerification, String> {
        let conn = self.connect()?;
        Self::verify_chain_from(&conn)
    }

    /// All lineage evidence is read from one SQLite snapshot, including the
    /// parent execution and the hash chain. No semantic graph is persisted.
    pub fn execution_snapshot(&self, task_id: &str) -> Result<Option<ExecutionSnapshot>, String> {
        let mut conn = self.connect()?;
        let transaction = conn.transaction().map_err(|e| e.to_string())?;
        let Some(execution) = Self::read_execution(&transaction, task_id)? else {
            return Ok(None);
        };
        let parent = execution
            .receipt
            .parent_task_id
            .as_deref()
            .map(|id| Self::read_execution(&transaction, id))
            .transpose()?
            .flatten();
        let events = Self::read_events(&transaction, task_id)?;
        let chain = Self::verify_chain_from(&transaction)?;
        transaction.commit().map_err(|e| e.to_string())?;
        Ok(Some(ExecutionSnapshot {
            execution,
            parent,
            events,
            chain,
        }))
    }

    fn verify_chain_from(conn: &Connection) -> Result<JournalVerification, String> {
        let mut stmt = conn
            .prepare(
                "SELECT sequence, event_id, task_id, event_type, timestamp_ns, payload_json, previous_hash, event_hash FROM execution_events ORDER BY sequence ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(JournalEvent {
                    sequence: row.get(0)?,
                    event_id: row.get(1)?,
                    task_id: row.get(2)?,
                    event_type: row.get(3)?,
                    timestamp_ns: row.get::<_, i64>(4)? as u64,
                    payload_json: row.get(5)?,
                    previous_hash: row.get(6)?,
                    event_hash: row.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let events = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut previous: Option<String> = None;
        for event in &events {
            let expected = hash_event(
                previous.as_deref(),
                &event.event_id,
                &event.task_id,
                &event.event_type,
                event.timestamp_ns,
                &event.payload_json,
            );
            if event.previous_hash != previous || event.event_hash != expected {
                return Ok(JournalVerification {
                    valid: false,
                    event_count: events.len() as u64,
                    head_hash: previous,
                    first_invalid_sequence: Some(event.sequence),
                });
            }
            previous = Some(event.event_hash.clone());
        }

        Ok(JournalVerification {
            valid: true,
            event_count: events.len() as u64,
            head_hash: previous,
            first_invalid_sequence: None,
        })
    }
}

pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn hash_event(
    previous_hash: Option<&str>,
    event_id: &str,
    task_id: &str,
    event_type: &str,
    timestamp_ns: u64,
    payload_json: &str,
) -> String {
    let mut hasher = Sha256::new();
    for part in [
        previous_hash.unwrap_or(""),
        event_id,
        task_id,
        event_type,
        &timestamp_ns.to_string(),
        payload_json,
    ] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        evidence::ExecutionReceipt,
        kernel::{authorize_for_executor, ExecuteInput},
    };

    fn path() -> PathBuf {
        std::env::temp_dir().join(format!("aether-journal-test-{}.sqlite3", Uuid::new_v4()))
    }

    fn sample_receipt() -> ExecutionReceipt {
        let permit = authorize_for_executor(
            &ExecuteInput {
                operation: "echo".into(),
                payload: "abc".into(),
                task_id: None,
                deadline_ms: 1_000,
            },
            "cpp-grpc-v1",
        )
        .unwrap();
        ExecutionReceipt::new(&permit, None, "abc", 1, true)
    }

    #[test]
    fn journal_survives_reopen_and_chain_verifies() {
        let path = path();
        let journal = Journal::open(&path).unwrap();
        journal
            .append_event("task", "TASK_ACCEPTED", BTreeMap::new())
            .unwrap();
        journal
            .append_event("task", "VERIFICATION_PASSED", BTreeMap::new())
            .unwrap();
        let receipt = sample_receipt();
        journal
            .save_execution(&StoredExecution {
                task_id: receipt.task_id.clone(),
                parent_task_id: None,
                operation: "echo".into(),
                payload: "abc".into(),
                output: "abc".into(),
                receipt: receipt.clone(),
                created_at_ns: now_ns(),
            })
            .unwrap();
        drop(journal);

        let reopened = Journal::open(&path).unwrap();
        assert!(reopened.verify_chain().unwrap().valid);
        assert!(reopened.load_execution(&receipt.task_id).unwrap().is_some());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn executor_stats_survive_reopen() {
        let path = path();
        let journal = Journal::open(&path).unwrap();
        let stats = ExecutorStats {
            executor_id: "cpp-grpc-v1".into(),
            successes: 7,
            failures: 2,
            consecutive_failures: 1,
            ewma_latency_ms: 4.5,
            quarantined_until_ns: 99,
        };
        journal.save_executor_stats(&stats).unwrap();
        drop(journal);

        let reopened = Journal::open(&path).unwrap();
        let loaded = reopened.load_executor_stats().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].executor_id, stats.executor_id);
        assert_eq!(loaded[0].successes, 7);
        assert_eq!(loaded[0].failures, 2);
        assert_eq!(loaded[0].consecutive_failures, 1);
        assert_eq!(loaded[0].ewma_latency_ms, 4.5);
        assert_eq!(loaded[0].quarantined_until_ns, 99);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tampering_is_detected() {
        let path = path();
        let journal = Journal::open(&path).unwrap();
        journal
            .append_event("task", "TASK_ACCEPTED", BTreeMap::new())
            .unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE execution_events SET payload_json = '{\"tampered\":\"yes\"}' WHERE sequence = 1",
            [],
        )
        .unwrap();
        assert!(!journal.verify_chain().unwrap().valid);
        let _ = fs::remove_file(path);
    }
}
