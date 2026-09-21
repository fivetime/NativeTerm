//! Session options: more ssh settings for one host, by category. Empty
//! fields keep what ssh would use anyway (shown greyed); saving is checked
//! with `ssh -G`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use native_term_app::t;
use native_term_config::options::{self, Category, Kind, Names, Values};
use native_term_config::password::REFUSED;
use native_term_config::proxy::{self, Proxy};
use native_term_win::credentials::{self, Saved};

use crate::dialogs::Outcome;

/// What the options are for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionsTarget {
    Host(String),
    /// Every host in this folder file.
    Folder(PathBuf),
}

/// The proxy field: none, one of NativeTerm's (type and address), or a
/// `ProxyCommand` of another form, kept as text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProxyChoice {
    Default,
    Kind(proxy::Kind),
    Other,
}

/// The proxy login's password in Credential Manager, as last read.
#[derive(Default)]
struct ProxySecret {
    /// The entry it was read for (it follows type, address and user).
    entry: Option<String>,
    saved: bool,
    refused: bool,
    typed: String,
    message: Option<String>,
}

impl ProxySecret {
    /// Read the state again when the entry changed.
    fn follow(&mut self, entry: Option<String>) {
        if self.entry == entry {
            return;
        }
        let saved = entry.as_deref().and_then(|e| credentials::read(e).ok().flatten());
        self.saved = saved.is_some();
        self.refused = saved.is_some_and(|s| s.comment == REFUSED);
        self.entry = entry;
        self.message = None;
    }
}

pub struct OptionsDialog {
    pub target: OptionsTarget,
    proxy: ProxyChoice,
    /// `host:port` for NativeTerm's proxy.
    proxy_address: String,
    /// The login's user name (empty: none).
    proxy_user: String,
    proxy_secret: ProxySecret,
    /// The helper written into `ProxyCommand`.
    shim: PathBuf,
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
        "User" => t!("field-user"),
        "Port" => t!("field-port"),
        "ProxyJump" => t!("field-jump"),
        "ProxyCommand" => t!("opt-proxy"),
        "IdentityFile" => t!("field-keys"),
        other => other.to_string(),
    }
}

fn lines_hint(keyword: &str) -> &'static str {
    match keyword {
        "LocalForward" => "8080 localhost:80",
        "RemoteForward" => "9000 localhost:3000",
        "DynamicForward" => "1080",
        "SetEnv" => "TERM=xterm-256color",
        "IdentityFile" => "~/.ssh/id_ed25519",
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
    pub fn new(
        target: OptionsTarget,
        label: &str,
        values: &Values,
        effective: Vec<(String, String)>,
        ssh: &Path,
    ) -> OptionsDialog {
        let text = options::SPECS
            .iter()
            .map(|s| (s.keyword, values.get(s.keyword).map(|v| v.join("\n")).unwrap_or_default()))
            .collect();
        let mut by_keyword: HashMap<String, Vec<String>> = HashMap::new();
        for (k, v) in effective {
            by_keyword.entry(k).or_default().push(v);
        }
        let written = values.get("ProxyCommand").and_then(|v| v.first()).map(String::as_str).unwrap_or("");
        let (proxy, proxy_address, proxy_user) = match Proxy::from_command(written) {
            _ if written.trim().is_empty() => (ProxyChoice::Default, String::new(), String::new()),
            Some(p) => (ProxyChoice::Kind(p.kind), p.address(), p.user.clone().unwrap_or_default()),
            None => (ProxyChoice::Other, String::new(), String::new()),
        };
        OptionsDialog {
            target,
            proxy,
            proxy_address,
            proxy_user,
            proxy_secret: ProxySecret::default(),
            shim: native_term_app::default_shim_path().unwrap_or_default(),
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
        let mut values: Values = self.text.iter().map(|(k, v)| (*k, v.lines().map(str::to_string).collect())).collect();
        let proxy = match self.proxy {
            ProxyChoice::Default => Vec::new(),
            ProxyChoice::Kind(_) => self.chosen_proxy().map(|p| vec![p.command(&self.shim)]).unwrap_or_default(),
            ProxyChoice::Other => values.get("ProxyCommand").cloned().unwrap_or_default(),
        };
        values.insert("ProxyCommand", proxy);
        values
    }

    /// NativeTerm's proxy as typed: type, address and user name.
    fn chosen_proxy(&self) -> Result<Proxy, String> {
        match self.proxy {
            ProxyChoice::Kind(kind) => Proxy::from_address(kind, &self.proxy_address)?.with_user(&self.proxy_user),
            _ => Err(String::new()),
        }
    }

    /// What is wrong with the proxy's address, if a proxy is chosen.
    fn proxy_error(&self) -> Option<String> {
        match self.proxy {
            ProxyChoice::Kind(_) => self.chosen_proxy().err().map(|e| t!("proxy-invalid", error = e)),
            _ => None,
        }
    }

    /// The login: a user name, and the password saved in Credential
    /// Manager right away (never in the config).
    fn proxy_login(&mut self, ui: &mut egui::Ui) {
        let ProxyChoice::Kind(kind) = self.proxy else { return };
        ui.horizontal(|ui| {
            ui.label(t!("proxy-user"));
            let hint = if kind == proxy::Kind::Socks4 { t!("proxy-user-id-hint") } else { t!("proxy-user-hint") };
            let field = egui::TextEdit::singleline(&mut self.proxy_user).hint_text(hint).desired_width(200.0);
            let field = ui.add(field);
            crate::dialogs::no_ime(&field);
        });
        let chosen = self.chosen_proxy().ok();
        self.proxy_secret.follow(chosen.as_ref().and_then(Proxy::password_entry));
        let Some(entry) = self.proxy_secret.entry.clone() else { return };
        let secret = &mut self.proxy_secret;
        let red = egui::Color32::from_rgb(0xd0, 0x3a, 0x3a);
        match (secret.saved, secret.refused) {
            (true, true) => ui.colored_label(red, t!("proxy-password-refused")),
            (true, false) => ui.weak(t!("proxy-password-saved")),
            (false, _) => ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), t!("proxy-password-none")),
        };
        ui.horizontal(|ui| {
            let field = egui::TextEdit::singleline(&mut secret.typed)
                .password(true)
                .hint_text(t!("proxy-password-hint"))
                .desired_width(200.0);
            let field = ui.add(field);
            crate::dialogs::no_ime(&field);
            if ui.add_enabled(!secret.typed.is_empty(), egui::Button::new(t!("password-save"))).clicked() {
                let user = self.proxy_user.trim().to_string();
                let saved = Saved { user, secret: std::mem::take(&mut secret.typed), comment: String::new() };
                let result = credentials::write(&entry, &saved);
                drop(saved);
                secret.message = Some(match result {
                    Ok(()) => {
                        (secret.saved, secret.refused) = (true, false);
                        t!("password-stored")
                    }
                    Err(e) => e.to_string(),
                });
            }
            if secret.saved && ui.button(t!("password-remove")).clicked() {
                secret.message = Some(match credentials::delete(&entry) {
                    Ok(_) => {
                        (secret.saved, secret.refused) = (false, false);
                        t!("password-removed")
                    }
                    Err(e) => e.to_string(),
                });
            }
        });
        if let Some(message) = &secret.message {
            ui.weak(message);
        }
        if kind == proxy::Kind::Http {
            ui.weak(t!("proxy-basic-note"));
            ui.weak(t!("proxy-windows-note"));
        }
        ui.weak(t!("password-warning"));
    }

    /// The proxy: a type, then its address (or another command as text).
    fn proxy_field(&mut self, ui: &mut egui::Ui) {
        ui.label(option_name("ProxyCommand")).on_hover_text("ProxyCommand");
        let effective = self.effective_of("ProxyCommand").join(" ");
        let inherited = match Proxy::from_command(&effective) {
            Some(p) => format!("{} {}", p.kind.name(), p.address()),
            None if effective.is_empty() || effective.eq_ignore_ascii_case("none") => t!("proxy-none"),
            None => effective.clone(),
        };
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let name = |choice: ProxyChoice| match choice {
                    ProxyChoice::Default => t!("options-default", value = inherited.clone()),
                    ProxyChoice::Kind(kind) => kind.name().to_string(),
                    ProxyChoice::Other => t!("proxy-other"),
                };
                egui::ComboBox::from_id_salt("proxy-kind").selected_text(name(self.proxy)).width(140.0).show_ui(
                    ui,
                    |ui| {
                        let mut choices = vec![ProxyChoice::Default];
                        choices.extend(proxy::Kind::ALL.map(ProxyChoice::Kind));
                        if self.proxy == ProxyChoice::Other {
                            choices.push(ProxyChoice::Other);
                        }
                        for choice in choices {
                            ui.selectable_value(&mut self.proxy, choice, name(choice));
                        }
                    },
                );
                match self.proxy {
                    ProxyChoice::Kind(kind) => {
                        let hint = format!("proxy.example.com:{}", kind.default_port());
                        let field = egui::TextEdit::singleline(&mut self.proxy_address).hint_text(hint);
                        ui.add(field.desired_width(172.0));
                    }
                    ProxyChoice::Other => {
                        let value = self.text.entry("ProxyCommand").or_default();
                        ui.add(egui::TextEdit::singleline(value).desired_width(172.0));
                    }
                    ProxyChoice::Default => {}
                }
            });
            if let Some(error) = self.proxy_error().filter(|_| !self.proxy_address.trim().is_empty()) {
                ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
            }
            self.proxy_login(ui);
            // ssh uses whichever of the two it reads first
            let jump = self.text.get("ProxyJump").map(|v| v.trim().to_string()).unwrap_or_default();
            let jump = if jump.is_empty() { self.effective_of("ProxyJump").join(",") } else { jump };
            let jumping = !jump.is_empty() && !jump.eq_ignore_ascii_case("none");
            if jumping && matches!(self.proxy, ProxyChoice::Kind(_)) {
                ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), t!("proxy-and-jump", jump = jump));
            }
        });
        ui.end_row();
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
        if keyword == "ProxyCommand" {
            return self.proxy_field(ui);
        }
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
                let shown =
                    if value.is_empty() { t!("options-default", value = current.clone()) } else { value.clone() };
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
        let (title, note) = match &self.target {
            OptionsTarget::Host(alias) => {
                (t!("options-title", label = self.label.as_str()), t!("options-note", alias = alias.as_str()))
            }
            OptionsTarget::Folder(_) => {
                (t!("options-folder-title", label = self.label.as_str()), t!("options-folder-note"))
            }
        };
        let folder = matches!(self.target, OptionsTarget::Folder(_));
        egui::Window::new(title)
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
                                for spec in options::SPECS
                                    .iter()
                                    .filter(|s| s.category == category && (folder || !s.folder_only))
                                {
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
                                Category::Connection => {
                                    ui.weak(t!("proxy-note"));
                                }
                                _ => {}
                            }
                        });
                    });
                });
                ui.separator();
                ui.weak(note);
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("button-save")).clicked() {
                        match self.proxy_error() {
                            Some(error) => self.error = Some(error),
                            None => outcome = Outcome::Submit(self.values()),
                        }
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
    fn proxy_as_type_and_address() {
        let shim = Path::new(r"C:\nt\nativeterm-shim.exe");
        let written = Proxy::from_address(proxy::Kind::Http, "squid:3128").unwrap().command(shim);
        let mut values = options::empty();
        values.insert("ProxyCommand", vec![written.clone()]);
        let host = || OptionsTarget::Host("web".into());
        let mut dialog = OptionsDialog::new(host(), "Web", &values, Vec::new(), Path::new("ssh"));
        assert_eq!(dialog.proxy, ProxyChoice::Kind(proxy::Kind::Http));
        assert_eq!(dialog.proxy_address, "squid:3128");
        // another type: this install's shim is written
        dialog.proxy = ProxyChoice::Kind(proxy::Kind::Socks5);
        dialog.proxy_address = "gw".into();
        let command = dialog.values()["ProxyCommand"][0].clone();
        assert!(command.ends_with(" --proxy socks5://gw:1080 %h %p"), "{command}");
        dialog.proxy_address = "g w".into();
        assert!(dialog.proxy_error().is_some());
        dialog.proxy = ProxyChoice::Default;
        assert!(dialog.values()["ProxyCommand"].is_empty());
        // a login: the user name in the URL
        dialog.proxy = ProxyChoice::Kind(proxy::Kind::Http);
        dialog.proxy_address = "squid:3128".into();
        dialog.proxy_user = "alice".into();
        let command = dialog.values()["ProxyCommand"][0].clone();
        assert!(command.ends_with(" --proxy http://alice@squid:3128 %h %p"), "{command}");
        let dialog = OptionsDialog::new(host(), "Web", &dialog.values(), Vec::new(), Path::new("ssh"));
        assert_eq!(dialog.proxy_user, "alice");
        // someone else's command is kept as it is
        values.insert("ProxyCommand", vec!["connect -S gw:1080 %h %p".into()]);
        let dialog = OptionsDialog::new(host(), "Web", &values, Vec::new(), Path::new("ssh"));
        assert_eq!(dialog.proxy, ProxyChoice::Other);
        assert_eq!(dialog.values()["ProxyCommand"], ["connect -S gw:1080 %h %p"]);
    }

    #[test]
    fn values_round_trip() {
        let mut values = options::empty();
        values.insert("LocalForward", vec!["1 a:1".into(), "2 b:2".into()]);
        values.insert("Ciphers", vec!["aes256-ctr".into()]);
        let dialog =
            OptionsDialog::new(OptionsTarget::Host("web".into()), "Web", &values, Vec::new(), Path::new("ssh"));
        assert_eq!(dialog.values(), values);
    }
}
