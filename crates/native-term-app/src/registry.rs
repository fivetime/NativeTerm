//! `state.db`: the open-session registry (so NativeTerm finds its tabs
//! again after a restart and can replace restored placeholders) and usage
//! counts. SQLite in rollback-journal mode: the data directory may be on a
//! network share, where WAL's shared memory doesn't work.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

pub type Result<T> = rusqlite::Result<T>;

const SCHEMA_VERSION: i64 = 7;

/// What someone wrote about a host: as many lines as they like, and
/// tags. Kept by the host's `NativeTermId`, so renaming it loses
/// nothing (`notes.rs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    /// In the order they were written, without empties.
    pub tags: Vec<String>,
    /// When it was last written, as seconds since the epoch: what
    /// decides between two computers' copies.
    pub updated_at: i64,
}

/// What is between two tags in a line of them.
const SEPARATORS: [char; 4] = [',', '\u{ff0c}', '\n', ';'];

impl Note {
    /// Nothing written: the row is kept so that clearing a note reaches
    /// the other computers too.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty() && self.tags.is_empty()
    }

    /// Tags as they are stored and shown: separated by commas.
    #[must_use]
    pub fn tag_line(&self) -> String {
        self.tags.join(", ")
    }

    /// Whether it has `tag` (as tags are told apart: `Prod` is `prod`).
    #[must_use]
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }

    /// The line of tags `line` with `tag` in it (`on`) or not, as
    /// choosing among the tags there are changes what was typed.
    #[must_use]
    pub fn line_with(line: &str, tag: &str, on: bool) -> String {
        let mut tags = Note::tags_from(line);
        tags.retain(|t| !t.eq_ignore_ascii_case(tag));
        if on {
            tags.extend(Note::tags_from(tag));
        }
        tags.join(", ")
    }

    /// What is typed of a tag at the end of `line` (after the last of
    /// what is between tags), and what is before it.
    #[must_use]
    pub fn being_typed(line: &str) -> (&str, &str) {
        let start = line.char_indices().rev().find(|(_, c)| SEPARATORS.contains(c)).map(|(i, c)| i + c.len_utf8());
        let (before, typed) = line.split_at(start.unwrap_or(0));
        (before, typed.trim())
    }

    /// The tags of `known` to choose among while `line` is typed: those
    /// that have in them what is typed of a tag at the line's end; all
    /// of them where that is nothing, or a whole tag, or in none.
    #[must_use]
    pub fn offered<'a>(line: &str, known: &'a [String]) -> Vec<&'a String> {
        let typed = Note::being_typed(line).1.to_lowercase();
        let whole = known.iter().any(|tag| tag.to_lowercase() == typed);
        let narrowed: Vec<&String> = known.iter().filter(|tag| tag.to_lowercase().contains(&typed)).collect();
        if typed.is_empty() || whole || narrowed.is_empty() {
            known.iter().collect()
        } else {
            narrowed
        }
    }

    /// `line` with `tag` chosen among those `offered`: in it, in the
    /// place of what was typed of it where that is the tag's beginning
    /// or another part of it.
    #[must_use]
    pub fn line_choosing(line: &str, tag: &str) -> String {
        let (before, typed) = Note::being_typed(line);
        let part = !typed.is_empty() && tag.to_lowercase().contains(&typed.to_lowercase());
        Note::line_with(if part { before } else { line }, tag, true)
    }

    /// Tags from a line someone typed (commas, no empties, no repeats;
    /// a tag may have spaces in it, and whatever a server's own labels
    /// have: `ceph-osd=enabled`, `openpe.fivetime.io/rack=c2r6`).
    #[must_use]
    pub fn tags_from(line: &str) -> Vec<String> {
        let mut tags: Vec<String> = Vec::new();
        for tag in line.split(SEPARATORS) {
            let tag = tag.trim();
            if !tag.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_string());
            }
        }
        tags
    }
}

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
    /// The shim last linked: process id and start time. If that process
    /// is gone at the next start, so is the tab.
    pub shim: Option<(u32, u64)>,
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
        if version < 4 {
            conn.execute_batch(
                "BEGIN;
                 ALTER TABLE sessions ADD COLUMN shim_pid INTEGER;
                 ALTER TABLE sessions ADD COLUMN shim_started INTEGER;
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
        if version < 5 {
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE notes (
                     nt_id TEXT PRIMARY KEY,
                     text TEXT NOT NULL,
                     tags TEXT NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 5;
                 COMMIT;",
            )?;
        }
        if version < 6 {
            // the tags there are, whether a host has them or not: what a
            // new host's tags are chosen among
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE tags (
                     name TEXT PRIMARY KEY COLLATE NOCASE,
                     created_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 6;
                 COMMIT;",
            )?;
            // (those the notes have are the first ones known)
            let lines: Vec<String> = {
                let mut statement = conn.prepare("SELECT tags FROM notes ORDER BY updated_at")?;
                let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<Result<_>>()?
            };
            for tag in lines.iter().flat_map(|line| Note::tags_from(line)) {
                conn.execute("INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)", params![tag, now()])?;
            }
        }
        if version < 7 {
            // what each host's server said it is, the last time it was
            // logged in to (`server.rs`)
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE servers (
                     alias TEXT PRIMARY KEY,
                     said TEXT NOT NULL,
                     seen_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 7;
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

    /// A consistent copy of the whole database in a new file.
    pub fn copy_to(&self, path: &Path) -> Result<()> {
        self.with(|c| {
            c.execute("VACUUM INTO ?1", [path.to_string_lossy()])?;
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

    /// The shim now linked for a session (see `Record::shim`).
    pub fn set_shim(&self, id: &str, pid: u32, started: u64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET shim_pid = ?2, shim_started = ?3 WHERE id = ?1",
                params![id, pid, started as i64],
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
            c.execute(
                "UPDATE sessions SET closed_at = COALESCE(closed_at, ?2), restorable = 1 WHERE id = ?1",
                params![id, now()],
            )?;
            Ok(())
        })
    }

    /// No longer worth restoring: its session was opened again by hand,
    /// so a pane Terminal brings back would only be a duplicate.
    pub fn not_restorable(&self, id: &str) -> Result<()> {
        self.with(|c| {
            c.execute("UPDATE sessions SET restorable = 0 WHERE id = ?1", [id])?;
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
                     no_forwards, locked, shim_pid, shim_started
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
                    shim: match (r.get::<_, Option<u32>>(10)?, r.get::<_, Option<i64>>(11)?) {
                        (Some(pid), Some(started)) => Some((pid, started as u64)),
                        _ => None,
                    },
                })
            })?;
            rows.collect()
        })
    }

    /// A per-machine setting (`settings` table).
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional())
    }

    /// What was written about the host with this id.
    pub fn note(&self, nt_id: &str) -> Result<Option<Note>> {
        self.with(|c| {
            c.query_row("SELECT text, tags, updated_at FROM notes WHERE nt_id = ?1", [nt_id], |r| {
                Ok(Note { text: r.get(0)?, tags: Note::tags_from(&r.get::<_, String>(1)?), updated_at: r.get(2)? })
            })
            .optional()
        })
    }

    /// Every note, by host id.
    pub fn notes(&self) -> Result<Vec<(String, Note)>> {
        self.with(|c| {
            let mut statement = c.prepare("SELECT nt_id, text, tags, updated_at FROM notes ORDER BY nt_id")?;
            let rows = statement.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Note { text: r.get(1)?, tags: Note::tags_from(&r.get::<_, String>(2)?), updated_at: r.get(3)? },
                ))
            })?;
            rows.collect()
        })
    }

    /// Write what someone said about a host. An empty note keeps its row:
    /// clearing one has to reach the other computers too. Its tags are
    /// tags there are from then on.
    pub fn set_note(&self, nt_id: &str, note: &Note) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO notes (nt_id, text, tags, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (nt_id) DO UPDATE SET text = ?2, tags = ?3, updated_at = ?4",
                params![nt_id, note.text, note.tag_line(), note.updated_at],
            )?;
            for tag in &note.tags {
                c.execute("INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)", params![tag, now()])?;
            }
            Ok(())
        })
    }

    /// The tags there are, by name: those a host has, and those that
    /// were some host's and have not been deleted since.
    pub fn tags(&self) -> Result<Vec<String>> {
        self.with(|c| {
            let mut statement = c.prepare("SELECT name FROM tags ORDER BY name COLLATE NOCASE")?;
            let rows = statement.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    /// `tag` is one of the tags there are.
    pub fn add_tag(&self, tag: &str) -> Result<()> {
        self.with(|c| {
            c.execute("INSERT OR IGNORE INTO tags (name, created_at) VALUES (?1, ?2)", params![tag, now()])?;
            Ok(())
        })
    }

    /// `tag` is none of the tags there are any more (the notes that have
    /// it are the caller's to change: `notes::retag`).
    pub fn delete_tag(&self, tag: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM tags WHERE name = ?1", [tag])?;
            Ok(())
        })
    }

    /// Every setting row (for taking them over into `settings.toml`).
    pub fn all_settings(&self) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut statement = c.prepare("SELECT key, value FROM settings ORDER BY key")?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
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

    /// `alias`' server said this of itself, now.
    pub fn set_server(&self, alias: &str, said: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO servers (alias, said, seen_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (alias) DO UPDATE SET said = ?2, seen_at = ?3",
                params![alias, said, now()],
            )?;
            Ok(())
        })
    }

    /// What the hosts' servers said of themselves: (alias, what).
    pub fn servers(&self) -> Result<Vec<(String, String)>> {
        self.with(|c| {
            let mut statement = c.prepare("SELECT alias, said FROM servers ORDER BY alias")?;
            let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    /// Most recently opened hosts first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Usage>> {
        self.with(|c| {
            let mut stmt =
                c.prepare("SELECT alias, count, last_opened FROM usage ORDER BY last_opened DESC, alias LIMIT ?1")?;
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
            shim: None,
        }
    }

    #[test]
    fn shim_is_remembered() {
        let reg = Registry::in_memory().unwrap();
        reg.opened(&record("a", "web01")).unwrap();
        assert_eq!(reg.open_sessions().unwrap()[0].shim, None, "not linked yet");
        reg.set_shim("a", 4242, 133_900_000_000_000_000).unwrap();
        assert_eq!(reg.open_sessions().unwrap()[0].shim, Some((4242, 133_900_000_000_000_000)));
        // closed with its window: still known, and restorable
        reg.closed_with_window("a").unwrap();
        assert!(reg.open_sessions().unwrap().is_empty());
        assert_eq!(reg.restorable_sessions(Duration::from_secs(3600)).unwrap()[0].id, "a");
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
    fn the_tags_the_notes_had_are_the_first_ones_known() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        {
            // as the schema before the tags' own table left it
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE notes (nt_id TEXT PRIMARY KEY, text TEXT NOT NULL, tags TEXT NOT NULL,
                     updated_at INTEGER NOT NULL);
                 INSERT INTO notes VALUES ('a', '', 'prod, ceph', 1);
                 INSERT INTO notes VALUES ('b', 'x', 'Prod, 生产', 2);
                 INSERT INTO notes VALUES ('c', '', '', 3);
                 PRAGMA user_version = 5;",
            )
            .unwrap();
        }
        let reg = Registry::open(&path).unwrap();
        assert_eq!(reg.tags().unwrap(), ["ceph", "prod", "生产"], "each once, as it was written first");
        // a note's tags are tags there are; taking one from the note leaves it one
        let note = Note { text: String::new(), tags: vec!["lab".into()], updated_at: 4 };
        reg.set_note("a", &note).unwrap();
        assert_eq!(reg.tags().unwrap(), ["ceph", "lab", "prod", "生产"]);
        reg.delete_tag("PROD").unwrap();
        reg.add_tag("Staging").unwrap();
        reg.add_tag("staging").unwrap();
        assert_eq!(reg.tags().unwrap(), ["ceph", "lab", "Staging", "生产"]);
        drop(reg);
        assert_eq!(Registry::open(&path).unwrap().tags().unwrap().len(), 4, "kept");
    }

    #[test]
    fn what_is_typed_of_a_tag_narrows_those_to_choose_among() {
        let known: Vec<String> =
            ["ceph-mon=enabled", "ceph-osd=enabled", "openpe.fivetime.io/rack=c2r6", "openvswitch=enabled", "prod"]
                .iter()
                .map(|t| t.to_string())
                .collect();
        let offered = |line: &str| Note::offered(line, &known).into_iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(offered("").len(), 5);
        assert_eq!(offered("prod, CEPH"), ["ceph-mon=enabled", "ceph-osd=enabled"]);
        assert_eq!(offered("rack"), ["openpe.fivetime.io/rack=c2r6"], "any part of it");
        assert_eq!(offered("prod，").len(), 5, "the next one is not begun");
        assert_eq!(offered("ceph-osd=enabled, prod").len(), 5, "a whole tag");
        assert_eq!(offered("a new one").len(), 5, "in none of them");
        // the one chosen takes the place of what was typed of it
        assert_eq!(Note::line_choosing("prod, CEPH", "ceph-osd=enabled"), "prod, ceph-osd=enabled");
        assert_eq!(Note::line_choosing("rack", "openpe.fivetime.io/rack=c2r6"), "openpe.fivetime.io/rack=c2r6");
        assert_eq!(Note::line_choosing("prod, ", "ceph-mon=enabled"), "prod, ceph-mon=enabled");
        assert_eq!(Note::line_choosing("a new one", "prod"), "a new one, prod", "and of nothing else");
        assert_eq!(Note::line_choosing("prod, ceph-mon=enabled", "prod"), "ceph-mon=enabled, prod", "once");
        // a server's labels are tags as they are
        let labels = "ceph-osd=enabled, openpe.fivetime.io/rack=c2r6; node-role.kubernetes.io/storage=";
        assert_eq!(
            Note::tags_from(labels),
            ["ceph-osd=enabled", "openpe.fivetime.io/rack=c2r6", "node-role.kubernetes.io/storage="]
        );
    }

    #[test]
    fn what_a_server_said_is_kept_per_host() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.db");
        {
            let reg = Registry::open(&path).unwrap();
            assert!(reg.servers().unwrap().is_empty());
            reg.set_server("web01", "SSH-2.0-OpenSSH_9.2p1 Debian-2+deb12u10").unwrap();
            reg.set_server("db01", "SSH-2.0-OpenSSH_10.2").unwrap();
            // (the server was set up anew)
            reg.set_server("web01", "SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.6").unwrap();
        }
        let kept = Registry::open(&path).unwrap().servers().unwrap();
        let said = |alias: &str, said: &str| (alias.to_string(), said.to_string());
        assert_eq!(
            kept,
            [said("db01", "SSH-2.0-OpenSSH_10.2"), said("web01", "SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.6")]
        );
    }

    #[test]
    fn a_tag_chosen_is_in_the_line_and_one_chosen_again_is_not() {
        assert_eq!(Note::line_with("", "prod", true), "prod");
        assert_eq!(Note::line_with("ceph，lab", "prod", true), "ceph, lab, prod");
        assert_eq!(Note::line_with("ceph, Prod, lab", "prod", true), "ceph, lab, prod", "once");
        assert_eq!(Note::line_with("ceph, Prod, lab", "prod", false), "ceph, lab");
        assert_eq!(Note::line_with("ceph", "prod", false), "ceph");
        let note = Note { tags: vec!["Prod".into()], ..Note::default() };
        assert!(note.has_tag("prod") && !note.has_tag("pro"));
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
