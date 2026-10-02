//! Pictures of windows drawn off the screen (the software renderer the
//! windows use), to look at a change without opening anything on the
//! desktop: `NATIVETERM_SNAPSHOTS=<folder> cargo test -p native-term-app
//! snapshots -- --ignored` writes a PNG of each, dark and light.

use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};

/// `draw` as a window `size` large shows it, a few frames on (what is
/// laid out after being measured settles), as RGBA rows.
fn picture(size: egui::Vec2, dark: bool, draw: impl FnMut(&mut egui::Ui)) -> (usize, usize, Vec<u8>) {
    picture_at(size, dark, None, draw)
}

/// The same with the pointer resting at `pointer`.
fn picture_at(
    size: egui::Vec2,
    dark: bool,
    pointer: Option<egui::Pos2>,
    mut draw: impl FnMut(&mut egui::Ui),
) -> (usize, usize, Vec<u8>) {
    let ctx = egui::Context::default();
    crate::install_fonts(&ctx);
    crate::looks::Preset::from_setting(None).apply(&ctx);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    // (a second between frames: what fades in has)
    let input = |frame: u32| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        time: Some(f64::from(frame)),
        events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
        ..egui::RawInput::default()
    };
    let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Rgba);
    let (w, h) = (size.x as usize, size.y as usize);
    let mut pixels = vec![[0u8; 4]; w * h];
    for frame in 0..4 {
        let mut output = ctx.run_ui(input(frame), &mut draw);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut target = BufferMutRef::new(&mut pixels, w, h);
        renderer.render(&mut target, &primitives, &output.textures_delta, output.pixels_per_point);
        output.textures_delta.clear();
    }
    (w, h, pixels.into_iter().flatten().collect())
}

fn save(name: &str, (w, h, rgba): (usize, usize, Vec<u8>)) {
    let Some(dir) = std::env::var_os("NATIVETERM_SNAPSHOTS") else { return };
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    let mut encoder = png::Encoder::new(file, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
    println!("{}", path.display());
}

#[test]
#[ignore]
fn snapshots() {
    use crate::window::Ui as _;
    for dark in [true, false] {
        let theme = if dark { "dark" } else { "light" };
        let question = native_term_app::password_ask::Question {
            label: Some("web01".into()),
            user: "root".into(),
            host: "10.0.0.9".into(),
            retry: true,
            refused: false,
            can_save: true,
        };
        let mut window = crate::password_window::for_snapshot(question);
        let size = egui::vec2(460.0, 280.0 + native_term_skin::TITLE_BAR);
        save(&format!("password-{theme}"), picture(size, dark, |ui| window.ui(ui)));

        for changed in [false, true] {
            use native_term_app::host_key_ask::{OldHostKey, Question};
            let old = changed.then(|| OldHostKey {
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:Q2xKb3F3cE1wZ0xUa2l2eFZ1bXh1SGxqd3JmS2Rh".into(),
                file: "~/.ssh/known_hosts".into(),
                line: 42,
            });
            let question = Question {
                label: Some("web01".into()),
                host: "10.0.0.9".into(),
                ip: "10.0.0.9".into(),
                key_type: "ssh-ed25519".into(),
                fingerprint: "SHA256:bm90IGEgcmVhbCBrZXksIGEgcGljdHVyZSdzIG9uZQ".into(),
                old,
            };
            let mut window = crate::host_key_window::for_snapshot(question);
            let height = if changed { 350.0 } else { 245.0 } + native_term_skin::TITLE_BAR;
            let name = if changed { "hostkey-changed" } else { "hostkey-new" };
            save(&format!("{name}-{theme}"), picture(egui::vec2(560.0, height), dark, |ui| window.ui(ui)));
        }
        let mut window = crate::tab_title_window::for_snapshot("web01 — build");
        let size = egui::vec2(440.0, 170.0 + native_term_skin::TITLE_BAR);
        save(&format!("tab-title-{theme}"), picture(size, dark, |ui| window.ui(ui)));

        // the files window, made with the picture's context (it keeps one)
        for (name, names) in [("files", false), ("files-names", true)] {
            let mut files: Option<Box<dyn crate::window::Ui>> = None;
            let size = egui::vec2(1440.0, 900.0);
            let snapshot = crate::files_window::snapshot::for_snapshot;
            save(
                &format!("{name}-{theme}"),
                picture(size, dark, |ui| files.get_or_insert_with(|| snapshot(ui.ctx(), names)).ui(ui)),
            );
        }

        // the files window's "Choose Transfer Type"
        let mut asked = crate::files_window::snapshot::transfer_type();
        save(
            &format!("files-type-{theme}"),
            picture(egui::vec2(560.0, 360.0), dark, |ui| {
                let skin = crate::looks::skin(ui.visuals());
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(skin.palette.page)).show(ui, |_| {});
                asked(ui.ctx());
            }),
        );

        // dialogs inside the main window, over a page standing for it
        let over = |name: &str, show: &mut dyn FnMut(&egui::Context)| {
            save(
                &format!("{name}-{theme}"),
                picture(egui::vec2(820.0, 620.0), dark, |ui| {
                    let skin = crate::looks::skin(ui.visuals());
                    let page = egui::Frame::NONE.fill(skin.palette.page);
                    egui::CentralPanel::default().frame(page).show(ui, |ui| ui.label("the main window, behind"));
                    show(ui.ctx());
                }),
            );
        };
        let mut host = crate::dialogs::HostDialog::new_host("lab.conf".into(), "Lab");
        over("host", &mut |ctx| {
            let _ = host.show(ctx);
        });
        let mut folder = crate::dialogs::FolderDialog::new_folder();
        over("folder", &mut |ctx| {
            let _ = folder.show(ctx);
        });
        let mut mixed = crate::dialogs::ConfirmCloseMixed::new(
            vec!["a".into(), "b".into()],
            vec![("web01".into(), false), ("local".into(), true)],
        );
        over("close-mixed", &mut |ctx| {
            let _ = mixed.show(ctx);
        });

        // the session tree: chosen hosts, checkboxes, a row under the
        // pointer, the recent list, a search; and narrow
        let dir = tempfile::tempdir().unwrap();
        let fixture = crate::tree_view::snapshot::fixture(dir.path());
        use crate::tree_view::{snapshot::view, Scope};
        // (name, checkboxes, chosen, searched, scope, width, pointer)
        type Case<'a> = (&'a str, bool, &'a [&'a str], &'a str, Scope, f32, Option<egui::Pos2>);
        let cases: [Case; 6] = [
            ("tree", false, &["db1", "db2"], "", Scope::Tree, 620.0, Some(egui::pos2(200.0, 150.0))),
            ("tree-checks", true, &["db1"], "", Scope::Tree, 620.0, Some(egui::pos2(30.0, 215.0))),
            ("tree-recent", false, &["web"], "", Scope::Recent, 620.0, None),
            ("tree-search", false, &[], "db", Scope::Tree, 620.0, None),
            ("tree-narrow", false, &["db1"], "", Scope::Tree, 300.0, None),
            ("tree-narrow-checks", true, &["db1", "db2"], "", Scope::Tree, 300.0, None),
        ];
        for (name, checks, chosen, query, scope, width, pointer) in cases {
            let mut tree = view(checks, chosen, query);
            save(
                &format!("{name}-{theme}"),
                picture_at(egui::vec2(width, 420.0), dark, pointer, |ui| {
                    let skin = crate::looks::skin(ui.visuals());
                    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(skin.palette.page)).show(ui, |ui| {
                        let _ = tree.show(ui, &fixture.shown(scope));
                    });
                }),
            );
        }

        let mut delete = crate::dialogs::ConfirmDelete::new("web01", "Web server 01");
        save(
            &format!("delete-{theme}"),
            picture(egui::vec2(720.0, 420.0), dark, |ui| {
                let skin = crate::looks::skin(ui.visuals());
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(skin.palette.page)).show(ui, |ui| {
                    ui.label("the main window, behind");
                });
                let _ = delete.show(ui.ctx());
            }),
        );
    }
}

/// Where the CJK font's ideographs sit against Inter's baseline: the
/// bottom of "H" (the baseline) and of "国" at 40 points, as pixel rows
/// (`cargo test -p native-term-app cjk_baseline -- --ignored --nocapture`).
#[test]
#[ignore]
fn cjk_baseline() {
    let size = egui::vec2(200.0, 80.0);
    let (w, h, rgba) = picture(size, false, |ui| {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::WHITE)).show(ui, |ui| {
            ui.label(egui::RichText::new("H国（").size(40.0).color(egui::Color32::BLACK));
        });
    });
    // the columns each glyph is in: H first, then 国, then the bracket
    let dark = |x: usize, y: usize| rgba[(y * w + x) * 4] < 128;
    let columns: Vec<bool> = (0..w).map(|x| (0..h).any(|y| dark(x, y))).collect();
    let mut runs = Vec::new();
    let mut start = None;
    for (x, &on) in columns.iter().enumerate() {
        match (on, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                runs.push((s, x));
                start = None;
            }
            _ => {}
        }
    }
    let rows = |(a, b): (usize, usize)| {
        let ys: Vec<usize> = (0..h).filter(|&y| (a..b).any(|x| dark(x, y))).collect();
        (ys[0], *ys.last().unwrap())
    };
    let (h_top, h_bottom) = rows(runs[0]);
    // 国 may be several runs of columns: from the second run to the last but one
    let guo = (runs[1].0, runs[runs.len() - 2].1);
    let (g_top, g_bottom) = rows(guo);
    println!(
        "CJK H top {h_top} bottom {h_bottom}; 国 top {g_top} bottom {g_bottom}; below the baseline {}",
        g_bottom as i64 - h_bottom as i64
    );
}
