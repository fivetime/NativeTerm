//! Where NativeTerm keeps its own data (see "Locating the data directory"
//! in `docs/ARCHITECTURE.md`). First match wins:
//! 1. `--data-dir`;
//! 2. `NATIVETERM_DATA_DIR`;
//! 3. `nativeterm.toml` next to the program (`data_dir = "…"`, relative to
//!    the program folder);
//! 4. `HKCU\Software\NativeTerm\DataDir`;
//! 5. `data\` next to the program if writable, else `%APPDATA%\NativeTerm`.

use std::io;
use std::path::{Path, PathBuf};

pub const ENV: &str = "NATIVETERM_DATA_DIR";
pub const POINTER_FILE: &str = "nativeterm.toml";
pub const REGISTRY_KEY: &str = r"Software\NativeTerm";
pub const REGISTRY_VALUE: &str = "DataDir";

/// Everything the decision depends on, so it can be tested.
#[derive(Default)]
pub struct Inputs {
    pub command_line: Option<PathBuf>,
    pub env: Option<PathBuf>,
    pub program_dir: PathBuf,
    pub registry: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
}

impl Inputs {
    pub fn from_system(command_line: Option<PathBuf>) -> io::Result<Inputs> {
        let exe = std::env::current_exe()?;
        Ok(Inputs {
            command_line,
            env: std::env::var_os(ENV).filter(|v| !v.is_empty()).map(PathBuf::from),
            program_dir: exe.parent().map(Path::to_path_buf).unwrap_or_default(),
            registry: registry_pointer(),
            app_data: native_term_os::home::app_data(),
        })
    }
}

/// `HKCU\Software\NativeTerm\DataDir`, where there is a registry.
fn registry_pointer() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        native_term_os::desktop::user_registry_string(REGISTRY_KEY, REGISTRY_VALUE)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// The folders inside it (see "Settings and data directory" in
/// `docs/ARCHITECTURE.md`): sent commands, copies of the ssh config files
/// NativeTerm edited, and its own log.
pub const FOLDERS: [&str; 3] = ["audit", "backups", "logs"];

/// The data directory, created if needed, and how it was chosen.
pub fn resolve(inputs: &Inputs) -> io::Result<(PathBuf, &'static str)> {
    let (dir, source) = choose(inputs)?;
    std::fs::create_dir_all(&dir)?;
    for folder in FOLDERS {
        // a folder that cannot be made is the writer's problem, not the
        // start's: the data directory itself is there
        let _ = std::fs::create_dir_all(dir.join(folder));
    }
    Ok((dir, source))
}

fn choose(inputs: &Inputs) -> io::Result<(PathBuf, &'static str)> {
    if let Some(dir) = &inputs.command_line {
        return Ok((dir.clone(), "--data-dir"));
    }
    if let Some(dir) = &inputs.env {
        return Ok((dir.clone(), ENV));
    }
    if let Some(dir) = pointer(&inputs.program_dir)? {
        return Ok((dir, POINTER_FILE));
    }
    if let Some(dir) = &inputs.registry {
        return Ok((dir.clone(), "HKCU\\Software\\NativeTerm\\DataDir"));
    }
    let portable = inputs.program_dir.join("data");
    if writable(&portable) {
        return Ok((portable, "program folder"));
    }
    let app_data = inputs
        .app_data
        .as_ref()
        .ok_or_else(|| io::Error::other(format!("{} is not set", native_term_os::home::APP_DATA_SOURCE)))?;
    Ok((app_data.join("NativeTerm"), native_term_os::home::APP_DATA_SOURCE))
}

fn pointer(program_dir: &Path) -> io::Result<Option<PathBuf>> {
    let file = program_dir.join(POINTER_FILE);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let value: toml::Table =
        text.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", file.display())))?;
    Ok(value.get("data_dir").and_then(|v| v.as_str()).map(|dir| program_dir.join(dir)))
}

/// What "Change data directory" updates, by how the current one was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// `nativeterm.toml` next to the program (portable mode).
    File,
    /// `HKCU\Software\NativeTerm\DataDir` (installed mode).
    Registry,
    /// Chosen by `--data-dir` or the environment: not NativeTerm's to change.
    Fixed,
}

pub fn pointer_for(source: &str) -> Pointer {
    match source {
        "--data-dir" | ENV => Pointer::Fixed,
        POINTER_FILE | "program folder" => Pointer::File,
        _ => Pointer::Registry,
    }
}

/// Point future starts at `dir`.
pub fn set_pointer(pointer: Pointer, program_dir: &Path, dir: &Path) -> io::Result<()> {
    match pointer {
        Pointer::Fixed => Err(io::Error::other("the data directory is set on the command line or in the environment")),
        Pointer::File => {
            let file = program_dir.join(POINTER_FILE);
            let mut table: toml::Table = match std::fs::read_to_string(&file) {
                Ok(text) => text
                    .parse()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", file.display())))?,
                Err(e) if e.kind() == io::ErrorKind::NotFound => toml::Table::new(),
                Err(e) => return Err(e),
            };
            table.insert("data_dir".into(), toml::Value::String(dir.display().to_string()));
            let text = format!("# where NativeTerm keeps its data (Settings > Change data folder)\n{table}");
            let temp = file.with_extension("toml.tmp");
            std::fs::write(&temp, text)?;
            std::fs::rename(&temp, &file)
        }
        #[cfg(windows)]
        Pointer::Registry => native_term_os::registry::write_user_values(
            REGISTRY_KEY,
            &[(REGISTRY_VALUE, native_term_os::registry::RegValue::Str(dir.display().to_string()))],
        ),
        #[cfg(not(windows))]
        Pointer::Registry => Err(io::Error::new(io::ErrorKind::Unsupported, "no registry to point from")),
    }
}

/// Whether `new` can take the data now in `current`: an absolute path,
/// not the same folder or inside it (or around it), and empty.
pub fn check_target(current: &Path, new: &Path) -> Result<(), String> {
    if !new.is_absolute() {
        return Err("not a full path".into());
    }
    let normalize = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    let (a, b) = (normalize(current), normalize(new));
    if a == b {
        return Err("that is the current folder".into());
    }
    if b.starts_with(&format!("{a}\\")) || a.starts_with(&format!("{b}\\")) {
        return Err("one folder is inside the other".into());
    }
    match std::fs::read_dir(new) {
        Ok(mut entries) => match entries.next() {
            Some(_) => Err("the folder isn't empty".into()),
            None => Ok(()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Copy the data to `to`. `state.db` is written by `copy_db` (a
/// consistent snapshot of the open database); the other files are copied
/// as they are. Returns how many files were copied.
pub fn copy_data(from: &Path, to: &Path, copy_db: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<usize> {
    std::fs::create_dir_all(to)?;
    copy_db(&to.join("state.db"))?;
    let mut copied = 1;
    copy_tree(from, to, true, &mut copied)?;
    Ok(copied)
}

fn copy_tree(from: &Path, to: &Path, top: bool, copied: &mut usize) -> io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if top && (name.starts_with("state.db") || name.starts_with(".write-test-")) {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target, false, copied)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
            *copied += 1;
        }
    }
    Ok(())
}

/// Whether files can be created in `dir` (created on the way if missing).
fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(program_dir: &Path) -> Inputs {
        Inputs {
            program_dir: program_dir.to_path_buf(),
            app_data: Some(PathBuf::from(r"C:\AppData")),
            ..Default::default()
        }
    }

    #[test]
    fn order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut i = inputs(tmp.path());
        assert_eq!(choose(&i).unwrap(), (tmp.path().join("data"), "program folder"));
        i.registry = Some(PathBuf::from(r"D:\reg"));
        assert_eq!(choose(&i).unwrap().0, PathBuf::from(r"D:\reg"));
        std::fs::write(tmp.path().join(POINTER_FILE), "# shared\ndata_dir = '../OneDrive/NativeTerm'\n").unwrap();
        assert_eq!(choose(&i).unwrap(), (tmp.path().join("../OneDrive/NativeTerm"), POINTER_FILE));
        i.env = Some(PathBuf::from(r"E:\env"));
        assert_eq!(choose(&i).unwrap().0, PathBuf::from(r"E:\env"));
        i.command_line = Some(PathBuf::from(r"F:\cli"));
        assert_eq!(choose(&i).unwrap().0, PathBuf::from(r"F:\cli"));
    }

    #[test]
    fn pointer_without_the_key_and_broken_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(POINTER_FILE), "other = 1\n").unwrap();
        assert_eq!(choose(&inputs(tmp.path())).unwrap().1, "program folder");
        std::fs::write(tmp.path().join(POINTER_FILE), "data_dir = \n").unwrap();
        assert!(choose(&inputs(tmp.path())).is_err(), "a broken pointer is reported, not skipped");
    }

    #[test]
    fn pointers() {
        assert_eq!(pointer_for("--data-dir"), Pointer::Fixed);
        assert_eq!(pointer_for(ENV), Pointer::Fixed);
        assert_eq!(pointer_for("program folder"), Pointer::File);
        assert_eq!(pointer_for(POINTER_FILE), Pointer::File);
        assert_eq!(pointer_for(native_term_os::home::APP_DATA_SOURCE), Pointer::Registry);

        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("elsewhere");
        set_pointer(Pointer::File, tmp.path(), &target).unwrap();
        assert_eq!(choose(&inputs(tmp.path())).unwrap(), (target.clone(), POINTER_FILE));
        // other keys in the file stay
        std::fs::write(tmp.path().join(POINTER_FILE), "data_dir = 'x'\nother = 1\n").unwrap();
        set_pointer(Pointer::File, tmp.path(), &target).unwrap();
        let text = std::fs::read_to_string(tmp.path().join(POINTER_FILE)).unwrap();
        assert!(text.contains("other = 1"), "{text}");
        assert_eq!(choose(&inputs(tmp.path())).unwrap().0, target);
        assert!(set_pointer(Pointer::Fixed, tmp.path(), &target).is_err());
    }

    #[test]
    fn targets() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("data");
        std::fs::create_dir_all(&current).unwrap();
        assert!(check_target(&current, Path::new("relative")).is_err());
        assert!(check_target(&current, &current).is_err());
        assert!(check_target(&current, &tmp.path().join("DATA\\")).is_err(), "same folder, other spelling");
        assert!(check_target(&current, &current.join("sub")).is_err());
        assert!(check_target(&current, tmp.path()).is_err());
        assert!(check_target(&current, &tmp.path().join("data2")).is_ok(), "a sibling with a longer name");
        let full = tmp.path().join("full");
        std::fs::create_dir_all(&full).unwrap();
        assert!(check_target(&current, &full).is_ok(), "empty");
        std::fs::write(full.join("x"), "").unwrap();
        assert!(check_target(&current, &full).is_err());
    }

    #[test]
    fn copies_everything_with_a_database_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("from");
        std::fs::create_dir_all(from.join("backups").join("inner")).unwrap();
        std::fs::write(from.join("commands.toml"), "a").unwrap();
        std::fs::write(from.join("backups").join("inner").join("b.bak"), "b").unwrap();
        std::fs::write(from.join("state.db-journal"), "stale").unwrap();
        let registry = crate::registry::Registry::open(&from.join("state.db")).unwrap();
        registry.set_setting("theme", "dark").unwrap();
        let to = tmp.path().join("to");
        let copied = copy_data(&from, &to, |db| registry.copy_to(db).map_err(io::Error::other)).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(std::fs::read_to_string(to.join("backups").join("inner").join("b.bak")).unwrap(), "b");
        assert!(!to.join("state.db-journal").exists());
        let copy = crate::registry::Registry::open(&to.join("state.db")).unwrap();
        assert_eq!(copy.setting("theme").unwrap().as_deref(), Some("dark"));
    }

    #[test]
    fn read_only_program_folder_falls_back_to_app_data() {
        // a path that can't be created: under an existing file
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-folder");
        std::fs::write(&file, "").unwrap();
        let i = inputs(&file);
        assert_eq!(
            choose(&i).unwrap(),
            (PathBuf::from(r"C:\AppData\NativeTerm"), native_term_os::home::APP_DATA_SOURCE)
        );
    }
}
