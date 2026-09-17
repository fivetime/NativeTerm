//! egui shell: sidebar session tree (from `native_term_config`) + list of
//! currently open sessions (from `native_term_session`). See
//! `docs/ROADMAP.md` Phase 0 for the first slice being built.

fn main() -> eframe::Result<()> {
    eframe::run_simple_native("NativeTerm", eframe::NativeOptions::default(), |ctx, _frame| {
        egui::SidePanel::left("session_tree").show(ctx, |ui| {
            ui.label("Session tree (TODO: load from ~/.ssh/config)");
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("Open sessions (TODO)");
        });
    })
}
