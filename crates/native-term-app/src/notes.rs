//! `notes.toml`: what was written about the hosts, in a file that syncs.
//!
//! The notes themselves live in `state.db`, keyed by each host's
//! `NativeTermId`, because they are read and written constantly. A SQLite
//! file and a sync service do not mix, though, so the notes also live in
//! a small text file next to it: written whenever one changes, read at
//! start, merged by id.
//!
//! The merge is by time: the newer of the two sides wins for each host,
//! and a note someone cleared is an empty note rather than a missing one,
//! so clearing it reaches the other computers too. Nothing is ever merged
//! *within* a note — two people editing the same host's note on two
//! computers at once keep the later one, and the file is a plain text
//! file they can look at.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::registry::Note;

pub const FILE: &str = "notes.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "note", skip_serializing_if = "Vec::is_empty")]
    notes: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// The host's `NativeTermId`.
    id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    /// Seconds since the epoch.
    updated: i64,
}

#[must_use]
pub fn path_in(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// What a merge did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Notes the file brought that the database didn't have, or had older.
    pub taken: usize,
    /// Notes the database has that the file didn't, or had older.
    pub given: usize,
}

impl Merged {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.taken == 0 && self.given == 0
    }
}

/// Read the file, if it is there. A file that can't be parsed is an
/// error and is never overwritten.
pub fn read(path: &Path) -> io::Result<BTreeMap<String, Note>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e),
    };
    let file: File = toml::from_str(text.trim_start_matches(crate::data_lock::BOM))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(file
        .notes
        .into_iter()
        .map(|entry| (entry.id, Note { text: entry.text, tags: entry.tags, updated_at: entry.updated }))
        .collect())
}

/// Write every note, through a temporary file and a rename.
pub fn write(path: &Path, notes: &BTreeMap<String, Note>) -> io::Result<()> {
    let file = File {
        notes: notes
            .iter()
            .map(|(id, note)| Entry {
                id: id.clone(),
                text: note.text.clone(),
                tags: note.tags.clone(),
                updated: note.updated_at,
            })
            .collect(),
    };
    let body = toml::to_string_pretty(&file).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let text = format!("# Notes and tags, by host id. NativeTerm keeps these in\n# state.db and writes this file so they sync.\n\n{body}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Bring the file and the database together: the newer note of the two
/// wins for each host. Returns what moved, and the merged set (what the
/// file should hold now).
#[must_use]
pub fn merge(
    from_file: &BTreeMap<String, Note>,
    from_db: &BTreeMap<String, Note>,
) -> (Merged, BTreeMap<String, Note>, Vec<(String, Note)>) {
    let mut done = Merged::default();
    let mut all = from_db.clone();
    // notes to write into the database
    let mut into_db = Vec::new();
    for (id, theirs) in from_file {
        match from_db.get(id) {
            Some(ours) if ours.updated_at >= theirs.updated_at => {
                if ours != theirs {
                    done.given += 1;
                }
            }
            _ => {
                done.taken += 1;
                all.insert(id.clone(), theirs.clone());
                into_db.push((id.clone(), theirs.clone()));
            }
        }
    }
    done.given += from_db.keys().filter(|id| !from_file.contains_key(*id)).count();
    (done, all, into_db)
}

/// At start: read the file, merge it with the database, and write each
/// side what it was missing. The notes as they stand afterwards, and
/// anything that went wrong (a notice, never a failed start).
pub fn open(registry: &crate::registry::Registry, data_dir: &Path) -> (BTreeMap<String, Note>, Option<String>) {
    let path = path_in(data_dir);
    let from_db: BTreeMap<String, Note> = match registry.notes() {
        Ok(rows) => rows.into_iter().collect(),
        Err(e) => return (BTreeMap::new(), Some(e.to_string())),
    };
    let from_file = match read(&path) {
        Ok(notes) => notes,
        // a file that can't be read is left alone and said out loud
        Err(e) => return (from_db, Some(format!("{}: {e}", path.display()))),
    };
    let (moved, all, into_db) = merge(&from_file, &from_db);
    let mut problem = None;
    for (id, note) in into_db {
        if let Err(e) = registry.set_note(&id, &note) {
            problem = Some(e.to_string());
        }
    }
    if moved.given > 0 || (!all.is_empty() && !path.exists()) {
        if let Err(e) = write(&path, &all) {
            problem = Some(format!("{}: {e}", path.display()));
        }
    }
    (all, problem)
}

/// Keep what was written about one host, in the database and in the file.
pub fn save(
    registry: &crate::registry::Registry,
    data_dir: &Path,
    notes: &mut BTreeMap<String, Note>,
    id: &str,
    note: Note,
) -> io::Result<()> {
    registry.set_note(id, &note).map_err(io::Error::other)?;
    notes.insert(id.to_string(), note);
    write(&path_in(data_dir), notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str, tags: &[&str], updated: i64) -> Note {
        Note { text: text.to_string(), tags: tags.iter().map(|t| t.to_string()).collect(), updated_at: updated }
    }

    #[test]
    fn notes_are_written_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        let mut notes = BTreeMap::new();
        notes.insert("7f3c".to_string(), note("第一行\n第二行", &["生产", "ceph"], 1_758_000_000));
        notes.insert("a1b2".to_string(), note("", &[], 1_758_000_100));
        write(&path, &notes).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("id = \"7f3c\""), "{text}");
        assert!(text.contains("生产"), "{text}");
        assert_eq!(read(&path).unwrap(), notes, "including the cleared one");
    }

    #[test]
    fn the_newer_note_of_the_two_wins() {
        let file: BTreeMap<String, Note> = [
            ("same".to_string(), note("both", &[], 10)),
            ("theirs".to_string(), note("from the other computer", &["new"], 30)),
            ("only-file".to_string(), note("only there", &[], 5)),
        ]
        .into();
        let db: BTreeMap<String, Note> = [
            ("same".to_string(), note("both", &[], 10)),
            ("theirs".to_string(), note("older", &[], 20)),
            ("only-db".to_string(), note("only here", &[], 7)),
        ]
        .into();
        let (done, all, into_db) = merge(&file, &db);
        assert_eq!(done.taken, 2, "the other computer's newer note and the one only it had");
        assert_eq!(done.given, 1, "the one only this computer has");
        assert_eq!(all.len(), 4);
        assert_eq!(all["theirs"].text, "from the other computer");
        assert_eq!(all["only-db"].text, "only here");
        let taken: Vec<&str> = into_db.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(taken, ["only-file", "theirs"]);
    }

    #[test]
    fn a_cleared_note_travels_like_any_other() {
        let file: BTreeMap<String, Note> = [("7f3c".to_string(), note("", &[], 50))].into();
        let db: BTreeMap<String, Note> = [("7f3c".to_string(), note("something", &["old"], 40))].into();
        let (done, all, into_db) = merge(&file, &db);
        assert_eq!(done.taken, 1);
        assert!(all["7f3c"].is_empty(), "cleared on the other computer, cleared here");
        assert_eq!(into_db.len(), 1);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error_and_stays() {
        let dir = tempfile::tempdir().unwrap();
        let path = path_in(dir.path());
        std::fs::write(&path, "[[note]]\nid = \n").unwrap();
        assert!(read(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[[note]]\nid = \n");
        // a missing file is simply nothing
        assert!(read(&dir.path().join("nothing.toml")).unwrap().is_empty());
    }

    #[test]
    fn tags_are_read_from_whatever_someone_typed() {
        assert_eq!(Note::tags_from("prod, ceph"), ["prod", "ceph"]);
        assert_eq!(Note::tags_from("生产，测试"), ["生产", "测试"], "a Chinese comma too");
        assert_eq!(Note::tags_from(" a ;b, a "), ["a", "b"], "no repeats, no empties");
        assert!(Note::tags_from("  ").is_empty());
    }
}
