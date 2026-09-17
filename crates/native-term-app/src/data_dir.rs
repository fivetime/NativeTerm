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
            registry: native_term_win::desktop::user_registry_string(REGISTRY_KEY, REGISTRY_VALUE)
                .ok()
                .flatten()
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            app_data: std::env::var_os("APPDATA").map(PathBuf::from),
        })
    }
}

/// The data directory, created if needed, and how it was chosen.
pub fn resolve(inputs: &Inputs) -> io::Result<(PathBuf, &'static str)> {
    let (dir, source) = choose(inputs)?;
    std::fs::create_dir_all(&dir)?;
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
    let app_data = inputs.app_data.as_ref().ok_or_else(|| io::Error::other("APPDATA is not set"))?;
    Ok((app_data.join("NativeTerm"), "%APPDATA%"))
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
        Inputs { program_dir: program_dir.to_path_buf(), app_data: Some(PathBuf::from(r"C:\AppData")), ..Default::default() }
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
    fn read_only_program_folder_falls_back_to_app_data() {
        // a path that can't be created: under an existing file
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-folder");
        std::fs::write(&file, "").unwrap();
        let i = inputs(&file);
        assert_eq!(choose(&i).unwrap(), (PathBuf::from(r"C:\AppData\NativeTerm"), "%APPDATA%"));
    }
}
