//! The files window with made-up sessions, folders and transfers, for
//! the pictures drawn off the screen (`snapshots`): nothing connects,
//! nothing is read from the disk.

use super::*;

/// A server's entry: name, folder or not, size, mode, modified (day of
/// September 2026).
fn remote(name: &str, dir: bool, size: u64, mode: u32, day: u32) -> RemoteRow {
    let kind = if dir { 0o040000 } else { 0o100000 };
    let mtime = 1_788_220_800 + (day - 1) * 86_400;
    RemoteRow {
        entry: Entry {
            name: name.as_bytes().to_vec(),
            long_name: Vec::new(),
            attrs: Attrs {
                size: Some(size),
                uid_gid: Some((0, 0)),
                permissions: Some(kind | mode),
                atime_mtime: Some((mtime, mtime)),
            },
        },
        name: name.to_string(),
        dir,
    }
}

fn local(folder: &Path, name: &str, dir: bool, size: u64, day: u64) -> LocalRow {
    LocalRow {
        name: name.to_string(),
        path: folder.join(name),
        dir,
        size: (!dir).then_some(size),
        modified: Some(1_788_220_800 + (day - 1) * 86_400),
    }
}

fn spec(alias: &str) -> Spec {
    Spec {
        alias: alias.into(),
        label: alias.into(),
        ssh: PathBuf::from("ssh"),
        config: None,
        names: Names::default(),
        shim: PathBuf::from("nativeterm-shim"),
        tmux_session: None,
        memory: None,
        credential: None,
    }
}

fn tab(window: &mut FilesWindow, alias: &str, up: bool) -> Tab {
    let id = window.next_id;
    window.next_id += 1;
    Tab {
        id,
        spec: spec(alias),
        remote: Remote {
            sftp: None,
            slots: Arc::new(native_term_sftp::transfer::Slots::new(3)),
            failed: (!up).then(|| "ssh: connect to host 10.32.16.3 port 22: Connection timed out".into()),
            path: b"/root".to_vec(),
            path_text: "/root".into(),
            rows: Vec::new(),
            listing: false,
            error: None,
            selected: Selection::default(),
            sort: NAME_SORT,
            view: files_list::View::Details,
            filter: String::new(),
            renaming: None,
            names: Names::default(),
            pictured: up,
        },
        local: Local {
            path: None,
            path_text: String::new(),
            rows: Vec::new(),
            error: None,
            selected: Selection::default(),
            sort: NAME_SORT,
            view: files_list::View::Details,
            filter: String::new(),
            renaming: None,
        },
        remote_tree: Tree::default(),
        local_tree: Tree::default(),
        edits: Vec::new(),
        log: Vec::new(),
        kept: (None, None),
    }
}

/// Opens `path` in `tree` with these subfolders.
fn opened<P: Clone + Eq + Hash>(tree: &mut Tree<P>, path: P, children: Vec<(String, P)>) {
    let node = tree.node(&path);
    node.open = true;
    node.children = Some(children);
}

fn sub(path: &[u8], name: &str) -> (String, Vec<u8>) {
    let mut p = path.to_vec();
    if p.last() != Some(&b'/') {
        p.push(b'/');
    }
    p.extend_from_slice(name.as_bytes());
    (name.to_string(), p)
}

fn job(window: &mut FilesWindow, tab: u64, kind: Kind, title: &str, done: u64, total: u64, state: JobState) -> Job {
    let id = window.next_id;
    window.next_id += 1;
    let progress = Arc::new(Progress::default());
    progress.done.store(done, Ordering::Relaxed);
    progress.total.store(total, Ordering::Relaxed);
    let started = Instant::now();
    Job {
        id,
        tab,
        host: "control1".into(),
        kind,
        title: title.into(),
        progress,
        state,
        started,
        // (finished: no speed that changes from one picture to the next)
        finished: Some(started),
        work: None,
        plan: Arc::new(Mutex::new(None)),
    }
}

/// Three sessions (the second shown, the third failed), a folder on each
/// side, the trees open down to them, and a queue.
/// `names`: both sides in their names-only view.
pub(crate) fn for_snapshot(ctx: &egui::Context, names: bool) -> Box<dyn crate::window::Ui> {
    let mut window = FilesWindow::new(ctx);
    let home = PathBuf::from(if cfg!(windows) { r"C:\Users\Projects" } else { "/home/projects" });
    window.computer = "MACBOOKPRO16".into();
    window.local_roots = ["Desktop", "Documents", "Downloads"].map(|n| (n.to_string(), home.join(n))).to_vec();

    let first = tab(&mut window, "baremetal1", true);
    let mut shown = tab(&mut window, "control1", true);
    let failed = tab(&mut window, "control2", false);

    let folder = home.join("Documents").join("OpenStack-Dev");
    shown.local.rows = vec![
        local(&folder, "baremetal", true, 0, 20),
        local(&folder, "docs", true, 0, 24),
        local(&folder, "OPCR Host 架构设计.md", false, 29_184, 25),
        local(&folder, "deploy_daemon.sh", false, 4_301, 21),
        local(&folder, "OVN_Fabric_Topology.json", false, 131_993, 18),
    ];
    shown.local.selected.set([local_key(&shown.local.rows[2])]);
    shown.local.path_text = folder.display().to_string();
    shown.local.path = Some(folder.clone());
    opened(&mut shown.local_tree, home.join("Documents"), vec![("OpenStack-Dev".into(), folder.clone())]);
    opened(
        &mut shown.local_tree,
        folder.clone(),
        vec![("baremetal".into(), folder.join("baremetal")), ("docs".into(), folder.join("docs"))],
    );

    let path = b"/boot/sys/kernel-5.15".to_vec();
    shown.remote.rows = vec![
        remote("modules", true, 4096, 0o755, 24),
        remote("vmlinuz-7.0.0-31", false, 11_744_051, 0o644, 24),
        remote("initrd.img-7.0.0-31", false, 64_277_299, 0o644, 24),
        remote("config-7.0.0-31", false, 315_802, 0o644, 12),
        remote("System.map-7.0.0-31", false, 9_012_334, 0o600, 12),
    ];
    shown.remote.selected.set([b"vmlinuz-7.0.0-31".to_vec()]);
    shown.remote.path_text = String::from_utf8_lossy(&path).into_owned();
    shown.remote.path = path.clone();
    opened(&mut shown.remote_tree, b"/".to_vec(), vec![sub(b"/", "boot"), sub(b"/", "etc"), sub(b"/", "var")]);
    opened(
        &mut shown.remote_tree,
        b"/boot".to_vec(),
        vec![sub(b"/boot", "efi"), sub(b"/boot", "grub"), sub(b"/boot", "sys")],
    );
    opened(&mut shown.remote_tree, b"/boot/sys".to_vec(), vec![sub(b"/boot/sys", "kernel-5.15")]);
    opened(&mut shown.remote_tree, path, vec![sub(b"/boot/sys/kernel-5.15", "modules")]);
    shown.log = vec![
        ("14:02:11".into(), "SSH2 10.32.16.2:22 · kex curve25519-sha256".into(), false),
        ("14:02:12".into(), "SFTP v3 · /boot/sys/kernel-5.15".into(), false),
        ("14:03:40".into(), "PUT deploy_daemon.sh → /opt/nexus/bin/ 4.2 KB".into(), false),
    ];

    if names {
        shown.local.view = files_list::View::Names;
        shown.remote.view = files_list::View::Names;
    }
    let id = shown.id;
    window.tabs = vec![first, shown, failed];
    window.active = 1;
    let downloads = home.join("Downloads");
    let from = |name: &str| {
        let item = (format!("/boot/sys/kernel-5.15/{name}").into_bytes(), Attrs::default());
        Some(Work::Download { names: Names::default(), items: vec![item], folder: downloads.clone() })
    };
    let mut jobs = vec![
        job(&mut window, id, Kind::Download, "vmlinuz-7.0.0-31", 7_985_954, 11_744_051, JobState::Paused),
        job(
            &mut window,
            id,
            Kind::Download,
            "initrd.img-7.0.0-31",
            0,
            64_277_299,
            JobState::Failed("Connection reset".into()),
        ),
        job(&mut window, id, Kind::Upload, "deploy_daemon.sh", 4_301, 4_301, JobState::Done),
    ];
    jobs[0].work = from("vmlinuz-7.0.0-31");
    jobs[1].work = from("initrd.img-7.0.0-31");
    jobs[2].work = Some(Work::Upload {
        names: Names::default(),
        files: vec![folder.join("deploy_daemon.sh")],
        into: b"/opt/nexus/bin".to_vec(),
    });
    window.jobs = jobs;
    Box::new(window)
}
