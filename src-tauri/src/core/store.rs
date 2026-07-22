//! Download registry, persisted with SQLite so the queue and partial downloads
//! survive a restart. One row per task; the engine keeps the live copy in
//! memory and writes through to here on meaningful changes.

use std::path::Path;

use rusqlite::{params, Connection};

use super::task::{Task, TaskKind, TaskStatus};

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) the downloads database under the data dir.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let conn = Connection::open(data_dir.join("downloads.db")).map_err(err)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS tasks (
                 id         TEXT PRIMARY KEY,
                 kind       TEXT NOT NULL,
                 url        TEXT NOT NULL,
                 filename   TEXT NOT NULL,
                 dest       TEXT NOT NULL,
                 status     TEXT NOT NULL,
                 total      INTEGER,
                 received   INTEGER NOT NULL DEFAULT 0,
                 error      TEXT,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL,
                 archived   INTEGER NOT NULL DEFAULT 0,
                 active_ms  INTEGER NOT NULL DEFAULT 0,
                 completed_at INTEGER,
                 backend    TEXT
             );",
        )
        .map_err(err)?;
        // Migrate older databases (ignore "duplicate column" on re-run).
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN active_ms INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN completed_at INTEGER", []);
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN backend TEXT", []);
        // Backfill a completion time for rows finished before this column existed.
        let _ = conn.execute(
            "UPDATE tasks SET completed_at = updated_at
             WHERE status = 'completed' AND completed_at IS NULL",
            [],
        );
        Ok(Self { conn })
    }

    /// Every task, newest first.
    pub fn all(&self) -> Result<Vec<Task>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, url, filename, dest, status, total, received, error,
                        created_at, updated_at, archived, active_ms, completed_at, backend
                 FROM tasks ORDER BY created_at DESC",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Task {
                    id: r.get(0)?,
                    kind: parse_kind(&r.get::<_, String>(1)?),
                    url: r.get(2)?,
                    filename: r.get(3)?,
                    dest: r.get(4)?,
                    status: parse_status(&r.get::<_, String>(5)?),
                    total: r.get(6)?,
                    received: r.get(7)?,
                    error: r.get(8)?,
                    created_at: r.get(9)?,
                    updated_at: r.get(10)?,
                    archived: r.get::<_, i64>(11)? != 0,
                    active_ms: r.get(12)?,
                    completed_at: r.get(13)?,
                    backend: r.get(14)?,
                })
            })
            .map_err(err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(err)
    }

    /// Insert or replace a task.
    pub fn upsert(&self, t: &Task) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO tasks
                   (id, kind, url, filename, dest, status, total, received, error,
                    created_at, updated_at, archived, active_ms, completed_at, backend)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                 ON CONFLICT(id) DO UPDATE SET
                   status=?6, total=?7, received=?8, error=?9, updated_at=?11,
                   archived=?12, active_ms=?13, completed_at=?14, backend=?15",
                params![
                    t.id,
                    kind_str(t.kind),
                    t.url,
                    t.filename,
                    t.dest,
                    status_str(t.status),
                    t.total,
                    t.received,
                    t.error,
                    t.created_at,
                    t.updated_at,
                    t.archived as i64,
                    t.active_ms,
                    t.completed_at,
                    t.backend,
                ],
            )
            .map_err(err)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])
            .map_err(err)?;
        Ok(())
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn kind_str(k: TaskKind) -> &'static str {
    match k {
        TaskKind::Http => "http",
        TaskKind::Torrent => "torrent",
        TaskKind::Media => "media",
    }
}

fn parse_kind(s: &str) -> TaskKind {
    match s {
        "torrent" => TaskKind::Torrent,
        "media" => TaskKind::Media,
        _ => TaskKind::Http,
    }
}

fn status_str(s: TaskStatus) -> &'static str {
    match s {
        TaskStatus::Queued => "queued",
        TaskStatus::Connecting => "connecting",
        TaskStatus::Downloading => "downloading",
        TaskStatus::Paused => "paused",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed => "failed",
        TaskStatus::Canceled => "canceled",
    }
}

fn parse_status(s: &str) -> TaskStatus {
    match s {
        "queued" => TaskStatus::Queued,
        "connecting" => TaskStatus::Connecting,
        "downloading" => TaskStatus::Downloading,
        "paused" => TaskStatus::Paused,
        "completed" => TaskStatus::Completed,
        "canceled" => TaskStatus::Canceled,
        _ => TaskStatus::Failed,
    }
}
