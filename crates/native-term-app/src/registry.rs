//! `state.db`: the open-session registry (so NativeTerm finds its tabs
//! again after a restart and can replace restored placeholders) and usage
//! counts. SQLite in rollback-journal mode: the data directory may be on a
//! network share, where WAL's shared memory doesn't work.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

pub type Result<T> = rusqlite::Result<T>;

const SCHEMA_VERSION: i64 = 3;

/// One session that was open when last seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    /// The GUID of the tab NativeTerm opened last for this session.
    pub terminal_session: String,
    /// The shim's current `WT_SESSION` if it differs ("Restart connection").
    pub current_terminal_session: Option<String>,
    pub label: String,
    pub alias: String,
    pub opened_at: i64,
    /// Last known position: window number and tab index.
    pub window_number: Option<i64>,
    pub tab_index: Option<i64>,
    /// A clone opened without port forwards.
    pub no_forwards: bool,
    /// Kept out of batch closes and group sends (not changed by `opened`).
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    pub alias: String,
    pub count: i64,
    pub last_opened: i64,
}

pub struct Registry {
    conn: Mutex<Connection>,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

impl Registry {
    pub fn open(path: &Path) -> Result<Registry> {
        Registry::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Registry> {
        Registry::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Registry> {
        conn.busy_timeout(Duration::from_secs(3))?;
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "state.db is from a newer NativeTerm (schema {version}, this one knows {SCHEMA_VERSION})"
            )));
        }
        if version < 1 {
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE sessions (
                     id TEXT PRIMARY KEY,
                     terminal_session TEXT NOT NULL,
                     current_terminal_session TEXT,
                     label TEXT NOT NULL,
                     alias TEXT NOT NULL,
                     opened_at INTEGER NOT NULL,
                     closed_at INTEGER,
                     window_number INTEGER,
                     tab_index INTEGER,
                     -- closed together with its window: Terminal may restore it
                     restorable INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX sessions_open ON sessions (closed_at);
                 CREATE TABLE usage (
                     alias TEXT PRIMARY KEY,
                     count INTEGER NOT NULL,
                     last_opened INTEGER NOT NULL
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE sessions ADD COLUMN no_forwards INTEGER NOT NULL DEFAULT 0;
                 CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 PRAGMA user_version = 2;
                 COMMIT;",
            )?;
        }
        if version < 3 {
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE sessions ADD COLUMN locked INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 3;
                 COMMIT;",
            )?;
        }
        Ok(Registry { conn: Mutex::new(conn) })
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        f(&self.conn.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// A session was opened (or reopened as a replacement tab).
    pub fn opened(&self, r: &Record) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions (id, terminal_session, current_terminal_session, label, alias, opened_at, closed_at, no_forwards)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, NULL, ?6)
                 ON CONFLICT (id) DO UPDATE SET terminal_session = ?2, current_terminal_session = NULL,
                     label = ?3, alias = ?4, closed_at = NULL, restorable = 0, no_forwards = ?6",
                params![r.id, r.terminal_session, r.label, r.alias, r.opened_at, r.no_forwards],
            )?;
            Ok(())
        })
    }

    pub fn set_locked(&self, id: &str, locked: bool) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE sessions SET locked = ?2 WHERE id = ?1", params![id, locked])?;
            Ok(())
        })
    }

    pub fn count_use(&self, alias: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO usage (alias, count, last_opened) VALUES (?1, 1, ?2)
                 ON CONFLICT (alias) DO UPDATE SET count = count + 1, last_opened = ?2",
                params![alias, now()],
            )?;
            Ok(())
        })
    }

    pub fn seen_terminal_session(&self, id: &str, guid: Option<&str>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET current_terminal_session = ?2, closed_at = NULL, restorable = 0 WHERE id = ?1",
                params![id, guid],
            )?;
            Ok(())
        })
    }

    pub fn moved(&self, id: &str, window_number: Option<i64>, tab_index: Option<i64>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET window_number = ?2, tab_index = ?3 WHERE id = ?1",
                params![id, window_number, tab_index],
            )?;
            Ok(())
        })
    }

    pub fn closed(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE sessions SET closed_at = ?2 WHERE id = ?1 AND closed_at IS NULL", params![id, now()])?;
            Ok(())
        })
    }

    /// Closed because its whole window closed: Terminal's session restore
    /// may bring its pane back.
    pub fn closed_with_window(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE sessions SET closed_at = COALESCE(closed_at, ?2), restorable = 1 WHERE id = ?1", params![id, now()])?;
            Ok(())
        })
    }

    pub fn open_sessions(&self) -> Result<Vec<Record>> {
        self.query("closed_at IS NULL", [])
    }

    /// Sessions closed with their window within `max_age`, newest first
    /// per position; older ones are forgotten.
    pub fn restorable_sessions(&self, max_age: Duration) -> Result<Vec<Record>> {
        let oldest = now() - max_age.as_secs() as i64;
        self.with(|c| {
            c.execute("UPDATE sessions SET restorable = 0 WHERE restorable = 1 AND closed_at < ?1", [oldest])?;
            Ok(())
        })?;
        self.query("closed_at IS NOT NULL AND restorable = 1", [])
    }

    fn query(&self, condition: &str, args: impl rusqlite::Params) -> Result<Vec<Record>> {
        self.with(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT id, terminal_session, current_terminal_session, label, alias, opened_at, window_number, tab_index,
                     no_forwards, locked
                 FROM sessions WHERE {condition} ORDER BY window_number, tab_index, opened_at"
            ))?;
            let rows = stmt.query_map(args, |r| {
                Ok(Record {
                    id: r.get(0)?,
                    terminal_session: r.get(1)?,
                    current_terminal_session: r.get(2)?,
                    label: r.get(3)?,
                    alias: r.get(4)?,
                    opened_at: r.get(5)?,
                    window_number: r.get(6)?,
                    tab_index: r.get(7)?,
                    no_forwards: r.get(8)?,
                    locked: r.get(9)?,
                })
            })?;
            rows.collect()
        })
    }

    /// A per-machine setting (`settings` table).
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2",
                params![key, value],
            )?;
            Ok(())
        })
    }

    pub fn usage(&self, alias: &str) -> Result<Option<Usage>> {
        self.with(|c| {
            c.query_row("SELECT alias, count, last_opened FROM usage WHERE alias = ?1", [alias], |r| {
                Ok(Usage { alias: r.get(0)?, count: r.get(1)?, last_opened: r.get(2)? })
            })
            .optional()
        })
    }

    /// Most recently opened hosts first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Usage>> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT alias, count, last_opened FROM usage ORDER BY last_opened DESC, alias LIMIT ?1")?;
            let rows = stmt.query_map([limit as i64], |r| {
                Ok(Usage { alias: r.get(0)?, count: r.get(1)?, last_opened: r.get(2)? })
            })?;
            rows.collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, label: &str) -> Record {
        Record {
            id: id.into(),
            terminal_session: format!("guid-{id}"),
            current_terminal_session: None,
            label: label.into(),
            alias: "web01".into(),
            opened_at: 100,
            window_number: None,
            tab_index: None,
            no_forwards: false,
            locked: false,
        }
    }

    #[test]
    fn session_lifecycle() {
        let reg = Registry::in_memory().unwrap();
        reg.opened(&record("a", "web01")).unwrap();
        reg.opened(&record("b", "web01 (2)")).unwrap();
        reg.moved("b", Some(1), Some(0)).unwrap();
        reg.moved("a", Some(1), Some(3)).unwrap();
        reg.seen_terminal_session("a", Some("restarted")).unwrap();
        let open = reg.open_sessions().unwrap();
        assert_eq!(open.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["b", "a"], "in tab order");
        assert_eq!(open[1].current_terminal_session.as_deref(), Some("restarted"));

        reg.closed("a").unwrap();
        assert_eq!(reg.open_sessions().unwrap().len(), 1);
        // its shim turned up after all
        reg.seen_terminal_session("a", None).unwrap();
        assert_eq!(reg.open_sessions().unwrap().len(), 2);

        // a replacement tab: new GUID, same session
        let mut replaced = record("a", "web01");
        replaced.terminal_session = "guid-new".into();
        reg.opened(&replaced).unwrap();
        let a = reg.open_sessions().unwrap().into_iter().find(|r| r.id == "a").unwrap();
        assert_eq!(a.terminal_session, "guid-new");
        assert_eq!(a.current_terminal_session, None);
        assert_eq!(a.window_number, Some(1), "position hint kept");
    }

    #[test]
    fn restorable_after_the_window_closed() {
        let reg = Registry::in_memory().unwrap();
        reg.opened(&record("a", "web01")).unwrap();
        reg.opened(&record("b", "db01")).unwrap();
        reg.closed("b").unwrap();
        reg.closed_with_window("a").unwrap();
        assert!(reg.open_sessions().unwrap().is_empty());
        let restorable = reg.restorable_sessions(Duration::from_secs(3600)).unwrap();
        assert_eq!(restorable.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
        // replaced: open again, no longer restorable
        reg.opened(&record("a", "web01")).unwrap();
        assert!(reg.restorable_sessions(Duration::from_secs(3600)).unwrap().is_empty());
        assert_eq!(reg.open_sessions().unwrap().len(), 1);
        // too old
        reg.closed_with_window("a").unwrap();
        reg.with(|c| c.execute("UPDATE sessions SET closed_at = 5", []).map(|_| ())).unwrap();
        assert!(reg.restorable_sessions(Duration::from_secs(3600)).unwrap().is_empty());
    }

    #[test]
    fn usage_counts() {
        let reg = Registry::in_memory().unwrap();
        assert_eq!(reg.usage("web01").unwrap(), None);
        reg.count_use("web01").unwrap();
        reg.count_use("db01").unwrap();
        reg.count_use("web01").unwrap();
        assert_eq!(reg.usage("web01").unwrap().unwrap().count, 2);
        assert_eq!(reg.recent(10).unwrap().len(), 2);
    }

    #[test]
    fn settings() {
        let reg = Registry::in_memory().unwrap();
        assert_eq!(reg.setting("auto_reconnect").unwrap(), None);
        reg.set_setting("auto_reconnect", "1").unwrap();
        reg.set_setting("auto_reconnect", "0").unwrap();
        assert_eq!(reg.setting("auto_reconnect").unwrap().as_deref(), Some("0"));
    }

    #[test]
    fn schema_1_is_upgraded() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE sessions (id TEXT PRIMARY KEY, terminal_session TEXT NOT NULL,
                     current_terminal_session TEXT, label TEXT NOT NULL, alias TEXT NOT NULL,
                     opened_at INTEGER NOT NULL, closed_at INTEGER, window_number INTEGER,
                     tab_index INTEGER, restorable INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE usage (alias TEXT PRIMARY KEY, count INTEGER NOT NULL, last_opened INTEGER NOT NULL);
                 INSERT INTO sessions (id, terminal_session, label, alias, opened_at) VALUES ('old', 'g', 'l', 'a', 1);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let reg = Registry::open(&path).unwrap();
        let open = reg.open_sessions().unwrap();
        assert_eq!(open.len(), 1);
        assert!(!open[0].no_forwards);
        let mut clone = record("c", "web01 (2)");
        clone.no_forwards = true;
        reg.opened(&clone).unwrap();
        assert!(reg.open_sessions().unwrap().iter().any(|r| r.id == "c" && r.no_forwards));
        reg.set_locked("c", true).unwrap();
        // a replacement tab keeps the lock
        reg.opened(&clone).unwrap();
        assert!(reg.open_sessions().unwrap().iter().any(|r| r.id == "c" && r.locked));
    }

    #[test]
    fn file_survives_reopen_and_refuses_newer_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        Registry::open(&path).unwrap().opened(&record("a", "web01")).unwrap();
        assert_eq!(Registry::open(&path).unwrap().open_sessions().unwrap().len(), 1);
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(Registry::open(&path).is_err());
    }
}
