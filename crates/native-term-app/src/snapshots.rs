//! Pictures of windows drawn off the screen (the software renderer the
//! windows use), to look at a change without opening anything on the
//! desktop: `NATIVETERM_SNAPSHOTS=<folder> cargo test -p native-term-app
//! snapshots -- --ignored` writes a PNG of each, dark and light.

use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};

/// `draw` as a window `size` large shows it, a few frames on (what is
/// laid out after being measured settles), as RGBA rows.
fn picture(size: egui::Vec2, dark: bool, mut draw: impl FnMut(&mut egui::Ui)) -> (usize, usize, Vec<u8>) {
    let ctx = egui::Context::default();
    crate::install_fonts(&ctx);
    crate::looks::Preset::from_setting(None).apply(&ctx);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..egui::RawInput::default()
    };
    let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Rgba);
    let (w, h) = (size.x as usize, size.y as usize);
    let mut pixels = vec![[0u8; 4]; w * h];
    for _ in 0..4 {
        let output = ctx.run_ui(input(), &mut draw);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut target = BufferMutRef::new(&mut pixels, w, h);
        renderer.render(&mut target, &primitives, &output.textures_delta, output.pixels_per_point);
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

        let mut delete = crate::dialogs::ConfirmDelete::new("web01", "Web server 01");
        save(
            &format!("delete-{theme}"),
            picture(egui::vec2(720.0, 420.0), dark, |ui| {
                let skin = crate::looks::skin(ui.visuals());
                egui::CentralPanel::default().frame(egui::Frame::NONE.fill(skin.palette.page)).show_inside(ui, |ui| {
                    ui.label("the main window, behind");
                });
                let _ = delete.show(ui.ctx());
            }),
        );
    }
}
