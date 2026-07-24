//! Download registry, persisted with SQLite so the queue and partial downloads
//! survive a restart. One row per task; the engine keeps the live copy in
//! memory and writes through to here on meaningful changes.

use std::path::Path;

use rusqlite::{params, Connection};

use super::task::{Task, TaskKind, TaskStatus, TorrentSource};

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
                 backend    TEXT,
                 category   TEXT,
                 headers    TEXT,
                 info_hash      TEXT,
                 torrent_source TEXT,
                 uploaded       INTEGER NOT NULL DEFAULT 0,
                 torrent_files  TEXT,
                 own_dir        INTEGER NOT NULL DEFAULT 0
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
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN category TEXT", []);
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN headers TEXT", []);
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN info_hash TEXT", []);
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN torrent_source TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN uploaded INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute("ALTER TABLE tasks ADD COLUMN torrent_files TEXT", []);
        let _ = conn.execute(
            "ALTER TABLE tasks ADD COLUMN own_dir INTEGER NOT NULL DEFAULT 0",
            [],
        );
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
                        created_at, updated_at, archived, active_ms, completed_at, backend,
                        category, headers, info_hash, torrent_source, uploaded, torrent_files,
                        own_dir
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
                    category: r.get(15)?,
                    headers: r
                        .get::<_, Option<String>>(16)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default(),
                    info_hash: r.get(17)?,
                    torrent_source: r
                        .get::<_, Option<String>>(18)?
                        .and_then(|s| parse_source(&s)),
                    uploaded: r.get(19)?,
                    files: r
                        .get::<_, Option<String>>(20)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default(),
                    // Live swarm readings aren't persisted — they resume at 0
                    // and the engine refreshes them once the task runs again.
                    seeders: 0,
                    leechers: 0,
                    peers: 0,
                    up_speed: 0,
                    own_dir: r.get::<_, i64>(21)? != 0,
                    force_seed: false,
                    force_start: false,
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
                    created_at, updated_at, archived, active_ms, completed_at, backend,
                    category, headers, info_hash, torrent_source, uploaded, torrent_files,
                    own_dir)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
                         ?18,?19,?20,?21,?22)
                 ON CONFLICT(id) DO UPDATE SET
                   status=?6, total=?7, received=?8, error=?9, updated_at=?11,
                   archived=?12, active_ms=?13, completed_at=?14, backend=?15,
                   category=?16, headers=?17, info_hash=?18, torrent_source=?19,
                   uploaded=?20, torrent_files=?21, own_dir=?22",
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
                    t.category,
                    headers_json(&t.headers),
                    t.info_hash,
                    t.torrent_source.map(source_str),
                    t.uploaded,
                    files_json(&t.files),
                    t.own_dir as i64,
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

/// Serialize a task's captured headers for storage: `None` when empty (so the
/// column stays NULL for the common hand-added download), else a JSON object.
fn headers_json(headers: &std::collections::BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        None
    } else {
        serde_json::to_string(headers).ok()
    }
}

/// Serialize a torrent's file list for storage: `None` when empty (the common
/// case for HTTP tasks, so the column stays NULL), else a JSON array.
fn files_json(files: &[super::task::TorrentFile]) -> Option<String> {
    if files.is_empty() {
        None
    } else {
        serde_json::to_string(files).ok()
    }
}

fn source_str(s: TorrentSource) -> &'static str {
    match s {
        TorrentSource::Magnet => "magnet",
        TorrentSource::File => "file",
    }
}

fn parse_source(s: &str) -> Option<TorrentSource> {
    match s {
        "magnet" => Some(TorrentSource::Magnet),
        "file" => Some(TorrentSource::File),
        _ => None,
    }
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
        TaskStatus::Checking => "checking",
        TaskStatus::Downloading => "downloading",
        TaskStatus::Paused => "paused",
        TaskStatus::Moving => "moving",
        TaskStatus::Completed => "completed",
        TaskStatus::Seeding => "seeding",
        TaskStatus::Failed => "failed",
        TaskStatus::Stalled => "stalled",
        TaskStatus::Canceled => "canceled",
    }
}

fn parse_status(s: &str) -> TaskStatus {
    match s {
        "queued" => TaskStatus::Queued,
        "connecting" => TaskStatus::Connecting,
        "checking" => TaskStatus::Checking,
        "downloading" => TaskStatus::Downloading,
        "paused" => TaskStatus::Paused,
        "moving" => TaskStatus::Moving,
        "completed" => TaskStatus::Completed,
        "seeding" => TaskStatus::Seeding,
        "stalled" => TaskStatus::Stalled,
        "canceled" => TaskStatus::Canceled,
        _ => TaskStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::task::{TorrentFile, TorrentSource};
    use std::collections::BTreeMap;

    /// A throwaway data dir under the system temp dir, cleaned on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "moin-store-test-{}-{}",
                std::process::id(),
                now_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn http_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            kind: TaskKind::Http,
            url: "https://example.com/file.zip".into(),
            filename: "file.zip".into(),
            dest: "/downloads/file.zip".into(),
            status: TaskStatus::Downloading,
            total: Some(1000),
            received: 400,
            error: None,
            created_at: 1,
            updated_at: 2,
            completed_at: None,
            archived: false,
            active_ms: 500,
            backend: Some("embedded".into()),
            category: None,
            headers: BTreeMap::new(),
            info_hash: None,
            torrent_source: None,
            files: Vec::new(),
            uploaded: 0,
            seeders: 0,
            leechers: 0,
            peers: 0,
            up_speed: 0,
            own_dir: false,
            force_seed: false,
            force_start: false,
        }
    }

    #[test]
    fn round_trips_a_torrent_task() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();

        let mut task = http_task("t1");
        task.kind = TaskKind::Torrent;
        task.dest = "/downloads/ubuntu".into(); // a folder for torrents
        task.status = TaskStatus::Seeding;
        task.info_hash = Some("deadbeef".into());
        task.torrent_source = Some(TorrentSource::Magnet);
        task.uploaded = 2048;
        task.files = vec![
            TorrentFile {
                path: "ubuntu.iso".into(),
                size: 900,
                selected: true,
                received: 900,
            },
            TorrentFile {
                path: "extras/readme.txt".into(),
                size: 100,
                selected: false,
                received: 0,
            },
        ];
        // Live swarm readings that should NOT survive the round-trip.
        task.seeders = 12;
        task.leechers = 3;
        task.peers = 15;
        task.up_speed = 999;

        store.upsert(&task).unwrap();
        let loaded = store.all().unwrap();
        assert_eq!(loaded.len(), 1);
        let got = &loaded[0];

        assert_eq!(got.kind, TaskKind::Torrent);
        assert_eq!(got.status, TaskStatus::Seeding);
        assert_eq!(got.info_hash.as_deref(), Some("deadbeef"));
        assert_eq!(got.torrent_source, Some(TorrentSource::Magnet));
        assert_eq!(got.uploaded, 2048);
        assert_eq!(got.files, task.files);
        // Transient fields reset to 0 on load.
        assert_eq!(
            (got.seeders, got.leechers, got.peers, got.up_speed),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn http_task_leaves_torrent_columns_empty() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();

        store.upsert(&http_task("h1")).unwrap();
        let got = &store.all().unwrap()[0];

        assert_eq!(got.kind, TaskKind::Http);
        assert!(got.info_hash.is_none());
        assert!(got.torrent_source.is_none());
        assert_eq!(got.uploaded, 0);
        assert!(got.files.is_empty());
    }

    #[test]
    fn upsert_updates_torrent_progress_in_place() {
        let dir = TempDir::new();
        let store = Store::open(&dir.0).unwrap();

        let mut task = http_task("t2");
        task.kind = TaskKind::Torrent;
        task.uploaded = 100;
        store.upsert(&task).unwrap();

        task.uploaded = 5000;
        task.files = vec![TorrentFile {
            path: "a.bin".into(),
            size: 10,
            selected: true,
            received: 10,
        }];
        store.upsert(&task).unwrap();

        let all = store.all().unwrap();
        assert_eq!(all.len(), 1, "upsert must update, not duplicate");
        assert_eq!(all[0].uploaded, 5000);
        assert_eq!(all[0].files.len(), 1);
    }

    /// A DB created before the torrent columns existed must gain them on open and
    /// keep its existing rows readable.
    #[test]
    fn migrates_a_pre_torrent_database() {
        let dir = TempDir::new();
        // Build a minimal old-schema DB (no torrent columns) with one row.
        {
            let conn = Connection::open(dir.0.join("downloads.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
                     id TEXT PRIMARY KEY, kind TEXT NOT NULL, url TEXT NOT NULL,
                     filename TEXT NOT NULL, dest TEXT NOT NULL, status TEXT NOT NULL,
                     total INTEGER, received INTEGER NOT NULL DEFAULT 0, error TEXT,
                     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                     archived INTEGER NOT NULL DEFAULT 0, active_ms INTEGER NOT NULL DEFAULT 0,
                     completed_at INTEGER, backend TEXT, category TEXT, headers TEXT
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO tasks (id, kind, url, filename, dest, status, received,
                                    created_at, updated_at)
                 VALUES ('old', 'http', 'https://x/y.zip', 'y.zip', '/d/y.zip',
                         'completed', 10, 1, 2)",
                [],
            )
            .unwrap();
        }

        // Opening through Store must add the new columns and still read the row.
        let store = Store::open(&dir.0).unwrap();
        let all = store.all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "old");
        assert_eq!(all[0].uploaded, 0);
        assert!(all[0].info_hash.is_none());

        // And a torrent row upserts fine into the migrated DB.
        let mut t = http_task("new");
        t.kind = TaskKind::Torrent;
        t.info_hash = Some("abc".into());
        store.upsert(&t).unwrap();
        assert_eq!(store.all().unwrap().len(), 2);
    }
}
