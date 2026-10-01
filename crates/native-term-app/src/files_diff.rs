//! Comparing a file here with one on the server (the design's Diff):
//! the server's is copied to a folder of the window's own, then both go
//! to a diff program the computer has (WinMerge, Beyond Compare, Meld,
//! KDiff3, Kompare, FileMerge, VS Code; looked for where they install).
//! NativeTerm draws no diff of its own.

use super::*;

/// A diff program found: where, and what comes before the two files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Tool {
    pub name: &'static str,
    pub program: PathBuf,
    pub args: Vec<&'static str>,
}

/// `name` in one of the PATH folders.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The first diff program this computer has.
pub(super) fn find_tool() -> Option<Tool> {
    let found = |name: &'static str, places: Vec<Option<PathBuf>>, args: Vec<&'static str>| {
        places.into_iter().flatten().find(|p| p.is_file()).map(|program| Tool { name, program, args })
    };
    #[cfg(windows)]
    {
        let env = |v: &str| std::env::var_os(v).map(PathBuf::from);
        let under = |v: &str, rest: &str| env(v).map(|d| d.join(rest));
        found("WinMerge", vec![under("ProgramFiles", r"WinMerge\WinMergeU.exe"), on_path("WinMergeU.exe")], vec!["/u"])
            .or_else(|| {
                found(
                    "Beyond Compare",
                    vec![
                        under("ProgramFiles", r"Beyond Compare 5\BComp.exe"),
                        under("ProgramFiles", r"Beyond Compare 4\BComp.exe"),
                        on_path("BComp.exe"),
                    ],
                    vec![],
                )
            })
            .or_else(|| found("Meld", vec![under("ProgramFiles", r"Meld\Meld.exe"), on_path("Meld.exe")], vec![]))
            .or_else(|| {
                found(
                    "VS Code",
                    vec![
                        under("LOCALAPPDATA", r"Programs\Microsoft VS Code\Code.exe"),
                        under("ProgramFiles", r"Microsoft VS Code\Code.exe"),
                    ],
                    vec!["--diff"],
                )
            })
    }
    #[cfg(not(windows))]
    {
        let mac = cfg!(target_os = "macos");
        let mut tools: Vec<(&'static str, &'static str, Vec<&'static str>)> = Vec::new();
        if mac {
            tools.push(("FileMerge", "opendiff", vec![]));
        }
        tools.extend([
            ("Meld", "meld", vec![]),
            ("KDiff3", "kdiff3", vec![]),
            ("Kompare", "kompare", vec![]),
            ("VS Code", "code", vec!["--diff"]),
        ]);
        tools.into_iter().find_map(|(name, program, args)| found(name, vec![on_path(program)], args))
    }
}

impl FilesWindow {
    /// The local file and the server's file to compare: one file chosen on
    /// each side.
    pub(super) fn diff_pair(&self) -> Option<(PathBuf, RemoteRow)> {
        let tab = self.tabs.get(self.active)?;
        let local = tab.local.rows.iter().filter(|r| !r.dir && tab.local.selected.contains(&local_key(r)));
        let remote = tab.remote.rows.iter().filter(|r| !r.dir && tab.remote.selected.contains(&r.entry.name));
        let (local, remote): (Vec<_>, Vec<_>) = (local.collect(), remote.collect());
        match (local.as_slice(), remote.as_slice(), tab.local.selected.len(), tab.remote.selected.len()) {
            ([l], [r], 1, 1) => Some((l.path.clone(), (*r).clone())),
            _ => None,
        }
    }

    /// Copies the server's file aside and opens both in the diff program.
    pub(super) fn diff(&mut self, id: u64) {
        let Some((local, row)) = self.diff_pair() else { return };
        let Some(tool) = find_tool() else {
            // the ones this platform has
            let tools = if cfg!(windows) {
                "WinMerge, Beyond Compare, Meld, VS Code"
            } else if cfg!(target_os = "macos") {
                "FileMerge (Xcode), Meld, VS Code"
            } else {
                "Meld, KDiff3, Kompare, VS Code"
            };
            self.log(id, t!("files-diff-no-tool", tools = tools), true);
            return;
        };
        let folder = self.edit_dir.join("diff").join(format!("{:x}", unique()));
        let Some(tab) = self.tab(id) else { return };
        let Some(sftp) = tab.remote.sftp.clone() else { return };
        let remote = native_term_sftp::join(&tab.remote.path, &row.entry.name);
        let copy = folder.join(transfer::local_name(&tab.remote.names, &row.entry.name));
        let said =
            t!("files-diff-opened", local = local.display().to_string(), remote = row.name.as_str(), tool = tool.name);
        self.spawn(id, move || {
            let fetched = std::fs::create_dir_all(&folder)
                .map_err(native_term_sftp::Error::from)
                .and_then(|()| sftp.download(&remote, &copy, &mut |_| true));
            if let Err(e) = fetched {
                return What::Notice(e.to_string(), true);
            }
            match std::process::Command::new(&tool.program).args(&tool.args).arg(&local).arg(&copy).spawn() {
                Ok(_) => What::Notice(said, false),
                Err(e) => What::Notice(format!("{}: {e}", tool.program.display()), true),
            }
        });
    }
}
