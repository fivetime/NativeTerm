//! What is chosen in the session tree, at the window's right (the
//! design's "Item Properties"): a host, several hosts, or a folder; what
//! is known about it, and what is done with it most. Everything here is
//! in the rows' menus too; this is where it can be seen without asking.

use std::collections::HashMap;

use native_term_app::{t, HostRequest};
use native_term_config::plink::Protocol;
use native_term_config::{Folder, HostEntry, SessionTree};
use native_term_platform::Target;

use crate::icons;
use crate::layout::{self, Kind, Room};
use crate::looks::Tones;
use crate::tree_view::{self, Activity, Chosen, FolderView, TreeAction, TreeView, Written};

/// What the properties are read from.
pub struct About<'a> {
    pub tree: &'a SessionTree,
    pub view: &'a TreeView,
    pub activity: &'a HashMap<String, Activity>,
    pub written: Written<'a>,
}

/// What was asked for here.
pub enum Asked {
    Tree(TreeAction),
    /// Nothing is to be chosen any more.
    Nothing,
}

/// An icon button's width, and what is between two buttons (`gap-2`).
const ICON_BUTTON: f32 = 40.0;
const GAP: f32 = 8.0;

/// What the chosen thing is, in a word, for the badge in the heading.
fn kind(about: &About, chosen: &Chosen) -> String {
    match chosen {
        Chosen::Nothing => t!("props-kind-none"),
        Chosen::Hosts(_) => t!("props-kind-group"),
        Chosen::Folder(_) => t!("props-folder"),
        Chosen::Host(alias) => match about.tree.find(alias).and_then(|(_, host)| host.plink.as_ref()) {
            None => "SSH".into(),
            Some(session) => crate::plink_dialog::protocol_text(session.protocol),
        },
    }
}

pub fn show(ui: &mut egui::Ui, about: &About, chosen: &Chosen) -> Vec<Asked> {
    let tones = crate::looks::tones(ui.visuals());
    let mut asked = Vec::new();
    // the heading (`px-5 py-4`), a line under it
    let heading = egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 14)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new(icons::INFO.to_string()).size(16.0).color(tones.accent));
            layout::caption(ui, &tones, &t!("props-title"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                layout::badge(ui, &tones, &kind(about, chosen).to_uppercase(), None);
            });
        });
    });
    let under = heading.response.rect.bottom();
    ui.painter().hline(ui.max_rect().x_range(), under, egui::Stroke::new(1.0_f32, tones.line));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 20)).show(ui, |ui| {
            // (the parts `space-y-5` from each other)
            ui.spacing_mut().item_spacing.y = 20.0;
            match chosen {
                Chosen::Nothing => layout::empty(ui, &tones, icons::POINTER, &t!("props-empty")),
                Chosen::Host(alias) => match about.tree.find(alias) {
                    Some((folder, host)) => one_host(ui, &tones, about, folder, host, &mut asked),
                    None => layout::empty(ui, &tones, icons::POINTER, &t!("props-empty")),
                },
                Chosen::Hosts(aliases) => several(ui, &tones, about, aliases, &mut asked),
                Chosen::Folder(path) => match about.view.folder(about.tree, path) {
                    Some(folder) => one_folder(ui, &tones, about, &folder, &mut asked),
                    None => layout::empty(ui, &tones, icons::POINTER, &t!("props-empty")),
                },
            }
        });
    });
    asked
}

/// The chosen thing's card: its picture, its name, and a line about it.
fn card(ui: &mut egui::Ui, tones: &Tones, icon: char, tint: egui::Color32, name: &str, under: &str, fixed: bool) {
    layout::card(ui, tones, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            layout::tile(ui, 48.0, icon, tones.bar, tones.line, tint);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(4.0);
                let name = egui::RichText::new(name).size(layout::MIDDLE).color(tones.text);
                ui.add(egui::Label::new(name).truncate());
                let mut line = egui::RichText::new(under).size(layout::SMALL).color(tones.weak);
                if fixed {
                    line = line.monospace();
                }
                ui.add(egui::Label::new(line).truncate()).on_hover_text(under);
            });
        });
    });
}

/// The properties, one under the other with nothing between them.
fn list(ui: &mut egui::Ui, rows: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        rows(ui);
    });
}

/// A part with a word above it.
fn part(ui: &mut egui::Ui, tones: &Tones, name: &str, inside: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(egui::RichText::new(name).size(layout::SMALL).color(tones.weak));
        inside(ui);
    });
}

/// A button that is an icon only, with what it does said under the pointer.
fn icon_button(ui: &mut egui::Ui, tones: &Tones, kind: Kind, icon: char, hint: String) -> bool {
    layout::button(ui, tones, kind, Room::Wide(ICON_BUTTON), Some(icon), "").on_hover_text(hint).clicked()
}

/// A row of buttons: the first as wide as the others leave it.
fn first_width(ui: &egui::Ui, others: usize) -> Room {
    Room::Wide((ui.available_width() - others as f32 * (ICON_BUTTON + GAP)).max(80.0))
}

fn one_host(
    ui: &mut egui::Ui,
    tones: &Tones,
    about: &About,
    folder: &Folder,
    host: &HostEntry,
    asked: &mut Vec<Asked>,
) {
    let alias = host.alias().to_string();
    let plink = host.plink.as_ref();
    let icon = match plink.map(|p| p.protocol) {
        None => icons::HOST,
        Some(Protocol::Serial) => icons::SERIAL,
        Some(_) => icons::NETWORK,
    };
    let address = match plink {
        Some(session) => session.target(),
        None => tree_view::address(host),
    };
    card(ui, tones, icon, tones.host, host.label(), &address, true);
    let note = about.written.of(host);
    list(ui, |ui| {
        layout::property(ui, tones, &t!("props-alias"), |ui| layout::value(ui, tones, &alias, true));
        layout::property(ui, tones, &t!("props-state"), |ui| match about.activity.get(&alias) {
            Some(activity) => {
                let (text, tint) = activity.mark(tones);
                layout::badge(ui, tones, &text, Some(tint));
            }
            None => layout::badge(ui, tones, &t!("props-state-none"), None),
        });
        layout::property(ui, tones, &t!("props-folder"), |ui| {
            let name = if folder.name.is_empty() { t!("tree-main-config") } else { folder.label().to_string() };
            layout::value(ui, tones, &name, false);
        });
        match plink {
            Some(session) => {
                if let Some(charset) = &session.charset {
                    layout::property(ui, tones, &t!("props-charset"), |ui| layout::value(ui, tones, charset, false));
                }
            }
            None => {
                layout::property(ui, tones, &t!("props-host"), |ui| layout::value(ui, tones, host.target(), true));
                if let Some(user) = &host.user {
                    layout::property(ui, tones, &t!("props-user"), |ui| layout::value(ui, tones, user, true));
                }
                if let Some(port) = host.port {
                    layout::property(ui, tones, &t!("props-port"), |ui| {
                        layout::value(ui, tones, &port.to_string(), true);
                    });
                }
                if let Some(jump) = &host.proxy_jump {
                    layout::property(ui, tones, &t!("props-jump"), |ui| layout::value(ui, tones, jump, true));
                }
                if let Some(program) = native_term_config::persistent::for_host(folder, host) {
                    layout::property(ui, tones, &t!("props-kept"), |ui| {
                        layout::value(ui, tones, program.name(), false);
                    });
                }
            }
        }
        if let Some(note) = note.filter(|note| !note.tags.is_empty()) {
            layout::property(ui, tones, &t!("props-tags"), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                // (from the right: the first tag is the one at the left)
                for tag in note.tags.iter().rev() {
                    layout::badge(ui, tones, tag, None);
                }
            });
        }
    });
    // what was written about it: in its config, and in NativeTerm
    let written: Vec<&str> =
        [plink.and_then(|session| session.note.as_deref()), host.nt.get("note"), note.map(|note| note.text.trim())]
            .into_iter()
            .flatten()
            .filter(|text| !text.is_empty())
            .collect();
    part(ui, tones, &t!("props-note"), |ui| {
        let text = if written.is_empty() { t!("props-no-note") } else { written.join("\n") };
        layout::text_box(ui, tones, &text);
    });
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
        let request = tree_view::request(about.tree, host);
        ui.horizontal(|ui| {
            let wide = first_width(ui, 2);
            if layout::button(ui, tones, Kind::Primary, wide, Some(icons::CONNECT), &t!("menu-connect")).clicked() {
                asked.push(Asked::Tree(TreeAction::Open(vec![request.clone()], Target::Recent)));
            }
            if icon_button(ui, tones, Kind::Plain, icons::OPEN, t!("menu-connect-new-window")) {
                asked.push(Asked::Tree(TreeAction::Open(vec![request.clone()], Target::NewWindow)));
            }
            let (hint, on) = match host.favorite() {
                true => (t!("menu-unfavorite"), false),
                false => (t!("menu-favorite"), true),
            };
            if icon_button(ui, tones, Kind::Plain, icons::STAR_FILLED, hint) {
                asked.push(Asked::Tree(TreeAction::Favorite(alias.clone(), on)));
            }
        });
        ui.horizontal(|ui| {
            let ssh = plink.is_none();
            let wide = first_width(ui, if ssh { 3 } else { 1 });
            if layout::button(ui, tones, Kind::Plain, wide, Some(icons::EDIT), &t!("menu-edit")).clicked() {
                asked.push(Asked::Tree(TreeAction::Edit(alias.clone())));
            }
            if ssh && icon_button(ui, tones, Kind::Plain, icons::SETTINGS, t!("menu-options")) {
                asked.push(Asked::Tree(TreeAction::Options(alias.clone())));
            }
            if ssh && icon_button(ui, tones, Kind::Plain, icons::FOLDER_OPEN, t!("menu-files")) {
                asked.push(Asked::Tree(TreeAction::Files(alias.clone())));
            }
            if icon_button(ui, tones, Kind::Danger, icons::DELETE, t!("menu-delete")) {
                asked.push(Asked::Tree(TreeAction::Delete(alias.clone())));
            }
        });
    });
}

fn several(ui: &mut egui::Ui, tones: &Tones, about: &About, aliases: &[String], asked: &mut Vec<Asked>) {
    let hosts: Vec<&HostEntry> = aliases.iter().filter_map(|a| about.tree.find(a).map(|(_, h)| h)).collect();
    let count = hosts.len();
    let open = hosts.iter().filter(|h| about.activity.get(h.alias()) == Some(&Activity::Connected)).count();
    let name = t!("props-group", count = count);
    card(ui, tones, icons::LAYERS, tones.accent, &name, &t!("props-group-open", count = open), false);
    part(ui, tones, &t!("props-group-hosts"), |ui| {
        // the first of them; the rest are counted
        const NAMED: usize = 12;
        let mut names: Vec<String> = hosts.iter().take(NAMED).map(|h| h.label().to_string()).collect();
        if count > NAMED {
            let more = count - NAMED;
            names.push(t!("props-group-more", count = more));
        }
        layout::text_box(ui, tones, &names.join("\n"));
    });
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
        let requests: Vec<HostRequest> = hosts.iter().map(|h| tree_view::request(about.tree, h)).collect();
        ui.horizontal(|ui| {
            let wide = first_width(ui, 1);
            let text = t!("menu-connect-selected", count = count);
            if layout::button(ui, tones, Kind::Primary, wide, Some(icons::CONNECT), &text).clicked() {
                asked.push(Asked::Tree(TreeAction::Open(requests.clone(), Target::Recent)));
            }
            let hint = t!("menu-connect-selected-new-window", count = count);
            if icon_button(ui, tones, Kind::Plain, icons::OPEN, hint) {
                asked.push(Asked::Tree(TreeAction::Open(requests.clone(), Target::NewWindow)));
            }
        });
        ui.horizontal(|ui| {
            let wide = first_width(ui, 1);
            let text = t!("menu-install-key-selected", count = count);
            if layout::button(ui, tones, Kind::Plain, wide, Some(icons::KEY), &text).clicked() {
                let list = hosts.iter().map(|h| (h.alias().to_string(), h.label().to_string())).collect();
                asked.push(Asked::Tree(TreeAction::InstallKey(list)));
            }
            if icon_button(ui, tones, Kind::Plain, icons::CLEAR, t!("menu-clear-selection")) {
                asked.push(Asked::Nothing);
            }
        });
    });
}

fn one_folder(ui: &mut egui::Ui, tones: &Tones, about: &About, folder: &FolderView, asked: &mut Vec<Asked>) {
    let count = folder.hosts.len();
    card(ui, tones, icons::FOLDER_OPEN, tones.folder, &folder.name, &t!("props-folder-hosts", count = count), false);
    let own = folder.index.and_then(|i| about.tree.folders().nth(i));
    list(ui, |ui| {
        layout::property(ui, tones, &t!("props-hosts"), |ui| layout::value(ui, tones, &count.to_string(), false));
        if folder.folders > 0 {
            layout::property(ui, tones, &t!("props-folders"), |ui| {
                layout::value(ui, tones, &folder.folders.to_string(), false);
            });
        }
        let open = folder.hosts.iter().filter(|h| about.activity.get(&h.alias) == Some(&Activity::Connected)).count();
        layout::property(ui, tones, &t!("props-state"), |ui| {
            let tint = (open > 0).then_some(tones.good);
            layout::badge(ui, tones, &t!("props-folder-open", count = open), tint);
        });
        if let Some(file) = &folder.file {
            let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            layout::property(ui, tones, &t!("props-file"), |ui| {
                let label = egui::RichText::new(&name).size(layout::SMALL).color(tones.text).monospace();
                ui.add(egui::Label::new(label).truncate()).on_hover_text(file.display().to_string());
            });
        }
        if let Some(own) = own.filter(|_| !folder.main) {
            if let Some(set) = own.defaults.get(native_term_config::password::KEY) {
                layout::property(ui, tones, &t!("props-credential"), |ui| layout::value(ui, tones, set, false));
            }
            let kept = own.defaults.get(native_term_config::persistent::KEY);
            if let Some(kept) = kept.filter(|v| native_term_config::persistent::parse(v).is_some()) {
                layout::property(ui, tones, &t!("props-kept"), |ui| layout::value(ui, tones, kept, false));
            }
            if own.no_group_send() {
                layout::property(ui, tones, &t!("props-group-send"), |ui| {
                    layout::badge(ui, tones, &t!("props-group-send-off"), Some(tones.busy));
                });
            }
        }
    });
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
        ui.add_enabled_ui(count > 0, |ui| {
            ui.horizontal(|ui| {
                let wide = first_width(ui, 1);
                let text = t!("menu-connect-all");
                if layout::button(ui, tones, Kind::Primary, wide, Some(icons::CONNECT), &text).clicked() {
                    asked.push(Asked::Tree(TreeAction::Open(folder.hosts.clone(), Target::Recent)));
                }
                if icon_button(ui, tones, Kind::Plain, icons::OPEN, t!("menu-connect-all-new-window")) {
                    asked.push(Asked::Tree(TreeAction::Open(folder.hosts.clone(), Target::NewWindow)));
                }
            });
        });
        // a folder with a file of its own can take hosts; a group of
        // folders (no file) can't
        let Some(file) = &folder.file else { return };
        ui.horizontal(|ui| {
            let wide = first_width(ui, if folder.main { 0 } else { 2 });
            if layout::button(ui, tones, Kind::Plain, wide, Some(icons::ADD), &t!("menu-new-host")).clicked() {
                asked.push(Asked::Tree(TreeAction::NewHost(file.clone())));
            }
            if !folder.main && icon_button(ui, tones, Kind::Plain, icons::RENAME, t!("menu-rename-folder")) {
                asked.push(Asked::Tree(TreeAction::RenameFolder(file.clone())));
            }
            if !folder.main && icon_button(ui, tones, Kind::Plain, icons::SETTINGS, t!("menu-folder-options")) {
                asked.push(Asked::Tree(TreeAction::FolderOptions(file.clone())));
            }
        });
    });
}
