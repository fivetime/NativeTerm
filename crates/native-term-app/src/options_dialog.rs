//! Session options: more ssh settings for one host, by category. Empty
//! fields keep what ssh would use anyway (shown greyed); saving is checked
//! with `ssh -G`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use native_term_app::t;
use native_term_config::options::{self, Category, Kind, Names, Values};

use crate::dialogs::Outcome;

pub struct OptionsDialog {
    pub alias: String,
    label: String,
    category: Category,
    /// Field text per keyword; repeated options one per line.
    text: BTreeMap<&'static str, String>,
    /// What ssh uses now, by lowercase keyword.
    effective: HashMap<String, Vec<String>>,
    /// `ssh -Q` answers, asked when a list is first opened.
    names: HashMap<&'static str, Vec<String>>,
    ssh: PathBuf,
    pub error: Option<String>,
}

fn category_name(category: Category) -> String {
    match category {
        Category::Connection => t!("options-connection"),
        Category::Authentication => t!("options-authentication"),
        Category::Algorithms => t!("options-algorithms"),
        Category::HostKey => t!("options-host-key"),
        Category::Forwarding => t!("options-forwarding"),
        Category::Environment => t!("options-environment"),
    }
}

fn option_name(keyword: &str) -> String {
    match keyword {
        "ConnectTimeout" => t!("opt-connect-timeout"),
        "ServerAliveInterval" => t!("opt-server-alive-interval"),
        "ServerAliveCountMax" => t!("opt-server-alive-count-max"),
        "TCPKeepAlive" => t!("opt-tcp-keep-alive"),
        "Compression" => t!("opt-compression"),
        "AddressFamily" => t!("opt-address-family"),
        "RequestTTY" => t!("opt-request-tty"),
        "RemoteCommand" => t!("opt-remote-command"),
        "LogLevel" => t!("opt-log-level"),
        "PreferredAuthentications" => t!("opt-preferred-authentications"),
        "PubkeyAuthentication" => t!("opt-pubkey-authentication"),
        "PasswordAuthentication" => t!("opt-password-authentication"),
        "KbdInteractiveAuthentication" => t!("opt-kbd-interactive-authentication"),
        "IdentitiesOnly" => t!("opt-identities-only"),
        "ForwardAgent" => t!("opt-forward-agent"),
        "PubkeyAcceptedAlgorithms" => t!("opt-pubkey-accepted-algorithms"),
        "KexAlgorithms" => t!("opt-kex-algorithms"),
        "Ciphers" => t!("opt-ciphers"),
        "MACs" => t!("opt-macs"),
        "HostKeyAlgorithms" => t!("opt-host-key-algorithms"),
        "StrictHostKeyChecking" => t!("opt-strict-host-key-checking"),
        "UpdateHostKeys" => t!("opt-update-host-keys"),
        "CheckHostIP" => t!("opt-check-host-ip"),
        "LocalForward" => t!("opt-local-forward"),
        "RemoteForward" => t!("opt-remote-forward"),
        "DynamicForward" => t!("opt-dynamic-forward"),
        "ExitOnForwardFailure" => t!("opt-exit-on-forward-failure"),
        "GatewayPorts" => t!("opt-gateway-ports"),
        "ForwardX11" => t!("opt-forward-x11"),
        "ForwardX11Trusted" => t!("opt-forward-x11-trusted"),
        "SetEnv" => t!("opt-set-env"),
        "SendEnv" => t!("opt-send-env"),
        other => other.to_string(),
    }
}

fn lines_hint(keyword: &str) -> &'static str {
    match keyword {
        "LocalForward" => "8080 localhost:80",
        "RemoteForward" => "9000 localhost:3000",
        "DynamicForward" => "1080",
        "SetEnv" => "TERM=xterm-256color",
        "SendEnv" => "LANG LC_*",
        _ => "",
    }
}

/// The explicit list a list field stands for: its own names, or what ssh
/// uses when it is empty or only adjusts the defaults (`+`, `-`, `^`).
fn explicit_list(text: &str, effective: &[String]) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() || text.starts_with(['+', '-', '^']) {
        effective.iter().flat_map(|v| v.split(',')).map(str::to_string).filter(|s| !s.is_empty()).collect()
    } else {
        text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
    }
}

impl OptionsDialog {
    pub fn new(alias: &str, label: &str, values: &Values, effective: Vec<(String, String)>, ssh: &Path) -> OptionsDialog {
        let text = options::SPECS
            .iter()
            .map(|s| (s.keyword, values.get(s.keyword).map(|v| v.join("\n")).unwrap_or_default()))
            .collect();
        let mut by_keyword: HashMap<String, Vec<String>> = HashMap::new();
        for (k, v) in effective {
            by_keyword.entry(k).or_default().push(v);
        }
        OptionsDialog {
            alias: alias.to_string(),
            label: label.to_string(),
            category: Category::Connection,
            text,
            effective: by_keyword,
            names: HashMap::new(),
            ssh: ssh.to_path_buf(),
            error: None,
        }
    }

    pub fn values(&self) -> Values {
        self.text.iter().map(|(k, v)| (*k, v.lines().map(str::to_string).collect())).collect()
    }

    fn effective_of(&self, keyword: &str) -> &[String] {
        self.effective.get(&keyword.to_ascii_lowercase()).map(Vec::as_slice).unwrap_or(&[])
    }

    fn names_of(&mut self, names: Names) -> Vec<String> {
        match names {
            Names::Fixed(list) => list.iter().map(|s| s.to_string()).collect(),
            Names::Query(what) => {
                let ssh = self.ssh.clone();
                self.names.entry(what).or_insert_with(|| options::query(&ssh, what)).clone()
            }
        }
    }

    fn field(&mut self, ui: &mut egui::Ui, keyword: &'static str, kind: Kind) {
        ui.label(option_name(keyword)).on_hover_text(keyword);
        let effective = self.effective_of(keyword).to_vec();
        let current = effective.join(", ");
        match kind {
            Kind::Text => {
                let value = self.text.entry(keyword).or_default();
                ui.add(egui::TextEdit::singleline(value).hint_text(current).desired_width(320.0));
            }
            Kind::Choice(choices) => {
                let value = self.text.entry(keyword).or_default();
                let shown = if value.is_empty() { t!("options-default", value = current.clone()) } else { value.clone() };
                egui::ComboBox::from_id_salt(keyword).selected_text(shown).width(320.0).show_ui(ui, |ui| {
                    ui.selectable_value(value, String::new(), t!("options-default", value = current.clone()));
                    for choice in choices {
                        let selected = value.eq_ignore_ascii_case(choice);
                        if ui.selectable_label(selected, *choice).clicked() {
                            *value = choice.to_string();
                        }
                    }
                });
            }
            Kind::List(names) => {
                ui.horizontal(|ui| {
                    let value = self.text.entry(keyword).or_default();
                    ui.add(egui::TextEdit::singleline(value).hint_text(current.clone()).desired_width(240.0));
                    let config = egui::containers::menu::MenuConfig::new()
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                    egui::containers::menu::MenuButton::new(t!("options-choose")).config(config).ui(ui, |ui| {
                        let all = self.names_of(names);
                        let value = self.text.entry(keyword).or_default();
                        let mut list = explicit_list(value, &effective);
                        if all.is_empty() {
                            ui.weak(t!("options-no-names"));
                        }
                        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                            for name in &all {
                                let mut on = list.contains(name);
                                if ui.checkbox(&mut on, name.as_str()).changed() {
                                    if on {
                                        list.push(name.clone());
                                    } else {
                                        list.retain(|n| n != name);
                                    }
                                    *value = list.join(",");
                                }
                            }
                        });
                        if ui.button(t!("options-use-default")).clicked() {
                            value.clear();
                            ui.close();
                        }
                    });
                });
            }
            Kind::Lines => {
                let value = self.text.entry(keyword).or_default();
                let hint = if current.is_empty() { lines_hint(keyword).to_string() } else { current };
                ui.add(egui::TextEdit::multiline(value).hint_text(hint).desired_rows(2).desired_width(320.0));
            }
        }
        ui.end_row();
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<Values> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(t!("options-title", label = self.label.as_str()))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(120.0);
                        for category in Category::ALL {
                            if ui.selectable_label(self.category == category, category_name(category)).clicked() {
                                self.category = category;
                            }
                        }
                    });
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.set_width(500.0);
                        ui.set_min_height(300.0);
                        egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| {
                            egui::Grid::new("session-options").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                let category = self.category;
                                for spec in options::SPECS.iter().filter(|s| s.category == category) {
                                    self.field(ui, spec.keyword, spec.kind);
                                }
                            });
                            match self.category {
                                Category::Authentication => {
                                    ui.weak(t!("options-forward-agent-note"));
                                }
                                Category::Algorithms => {
                                    ui.weak(t!("options-algorithms-note"));
                                }
                                Category::Forwarding => {
                                    ui.weak(t!("options-forwarding-note"));
                                }
                                _ => {}
                            }
                        });
                    });
                });
                ui.separator();
                ui.weak(t!("options-note", alias = self.alias.as_str()));
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("button-save")).clicked() {
                        outcome = Outcome::Submit(self.values());
                    }
                    if ui.button(t!("button-cancel")).clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if !open {
            outcome = Outcome::Cancel;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_fields() {
        let effective = vec!["aes256-ctr,aes128-ctr".to_string()];
        assert_eq!(explicit_list("", &effective), ["aes256-ctr", "aes128-ctr"]);
        assert_eq!(explicit_list("+3des-cbc", &effective), ["aes256-ctr", "aes128-ctr"]);
        assert_eq!(explicit_list(" chacha20-poly1305@openssh.com , aes256-ctr", &effective).len(), 2);
    }

    #[test]
    fn every_option_has_a_name() {
        for spec in options::SPECS {
            assert_ne!(option_name(spec.keyword), spec.keyword, "{}", spec.keyword);
        }
    }

    #[test]
    fn values_round_trip() {
        let mut values = options::empty();
        values.insert("LocalForward", vec!["1 a:1".into(), "2 b:2".into()]);
        values.insert("Ciphers", vec!["aes256-ctr".into()]);
        let dialog = OptionsDialog::new("web", "Web", &values, Vec::new(), Path::new("ssh"));
        assert_eq!(dialog.values(), values);
    }
}
