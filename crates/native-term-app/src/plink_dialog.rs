//! New or edited non-SSH session (Telnet, rlogin, raw, serial, SUPDUP),
//! kept in the folder's `.nt.toml` and run with plink.

use std::path::{Path, PathBuf};

use native_term_app::t;
use native_term_config::plink::{
    self, Flow, Parity, PlinkSession, Protocol, PuttyOption, PuttyValue, Serial, PUTTY_CONNECTION, PUTTY_LINE,
    PUTTY_LOG, PUTTY_SUPDUP, PUTTY_TELNET,
};

use crate::dialogs::Outcome;

/// Charsets offered first; any name `plink::code_page` knows, or a code
/// page number, can be typed.
pub const CHARSETS: [&str; 5] = ["UTF-8", "GBK", "Big5", "Shift_JIS", "EUC-KR"];
const SPEEDS: [u32; 10] = [300, 1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400];
const STOP_BITS: [&str; 3] = ["1", "1.5", "2"];

pub struct PlinkDialog {
    pub title: String,
    /// `Some(name)` when editing.
    pub alias: Option<String>,
    /// The folder file a new session goes to.
    pub file: Option<PathBuf>,
    /// What the edited session had that the dialog doesn't show.
    base: PlinkSession,
    label: String,
    protocol: Protocol,
    host: String,
    port: String,
    user: String,
    charset: String,
    /// What the Backspace key sends: `^h` or `^?`.
    backspace: String,
    line: String,
    speed: String,
    data_bits: u8,
    parity: Parity,
    stop_bits: String,
    flow: Flow,
    note: String,
    on_login: String,
    pre_connect: String,
    /// As many lines as they like, kept in `state.db` by the session's id.
    long_note: String,
    /// Comma separated, kept with the long note.
    tags: String,
    /// Serial ports present when the dialog opened.
    ports: Vec<String>,
    /// The PuTTY options being edited (only `putty` is used).
    options: PlinkSession,
    /// The log file used when logging is turned on without one.
    default_log: String,
    pub error: Option<String>,
}

fn opt(text: &str) -> Option<String> {
    let t = text.trim();
    (!t.is_empty()).then(|| t.to_string())
}

impl PlinkDialog {
    pub fn new_session(file: PathBuf, folder: &str) -> PlinkDialog {
        PlinkDialog::from_session(t!("plink-new-title", folder = folder), None, Some(file), &PlinkSession::default())
    }

    pub fn edit(session: &PlinkSession) -> PlinkDialog {
        let title = t!("plink-edit-title", name = session.label());
        PlinkDialog::from_session(title, Some(session.name.clone()), None, session)
    }

    fn from_session(title: String, alias: Option<String>, file: Option<PathBuf>, s: &PlinkSession) -> PlinkDialog {
        let serial = s.serial.clone().unwrap_or_else(|| Serial::new(""));
        let ports = native_term_os::registry::serial_ports();
        let line =
            if serial.line.is_empty() { ports.first().cloned().unwrap_or_default() } else { serial.line.clone() };
        PlinkDialog {
            title,
            alias,
            file,
            base: s.clone(),
            label: s.label.clone().unwrap_or_default(),
            protocol: s.protocol,
            host: s.host.clone().unwrap_or_default(),
            port: s.port.map(|p| p.to_string()).unwrap_or_default(),
            user: s.user.clone().unwrap_or_default(),
            charset: s.charset.clone().unwrap_or_else(|| CHARSETS[0].into()),
            backspace: s.backspace.clone().unwrap_or_else(|| "^h".into()),
            line,
            speed: serial.speed.to_string(),
            data_bits: serial.data_bits,
            parity: serial.parity,
            stop_bits: serial.stop_bits.clone(),
            flow: serial.flow,
            note: s.note.clone().unwrap_or_default(),
            on_login: s.on_login.clone().unwrap_or_default(),
            pre_connect: s.pre_connect.clone().unwrap_or_default(),
            long_note: String::new(),
            tags: String::new(),
            ports,
            options: s.clone(),
            default_log: String::new(),
            error: None,
        }
    }

    /// The data directory, for the default log file.
    pub fn with_data_dir(mut self, data_dir: &Path) -> PlinkDialog {
        self.default_log = plink::default_log_file(data_dir);
        self
    }

    /// What was written about this session (`notes.rs`).
    pub fn with_note(mut self, note: Option<&native_term_app::registry::Note>) -> PlinkDialog {
        if let Some(note) = note {
            self.long_note = note.text.clone();
            self.tags = note.tag_line();
        }
        self
    }

    /// The note as it stands now, stamped with the time it was written.
    pub fn note_now(&self) -> native_term_app::registry::Note {
        native_term_app::registry::Note {
            text: self.long_note.trim_end().to_string(),
            tags: native_term_app::registry::Note::tags_from(&self.tags),
            updated_at: native_term_app::registry::now(),
        }
    }

    /// Log type and file; a log turned on without a file gets the default.
    fn log_ui(&mut self, ui: &mut egui::Ui) {
        let on = self.options.log_file().is_some() || self.options.putty.contains_key(PUTTY_LOG[0].key());
        egui::CollapsingHeader::new(t!("plink-log")).id_salt("plink-log").default_open(on).show(ui, |ui| {
            egui::Grid::new("plink-log-fields").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.label(t!("plink-log-type"));
                let mut n = match self.options.putty_value(PUTTY_LOG[0]) {
                    PuttyValue::Number(n) if n <= 2 => n,
                    _ => 0,
                };
                let choices = [(0u32, t!("plink-log-off")), (1, t!("plink-log-text")), (2, t!("plink-log-raw"))];
                let shown = choices.iter().find(|(v, _)| *v == n).map(|(_, text)| text.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("plink-log-type").selected_text(shown).show_ui(ui, |ui| {
                    for (v, text) in &choices {
                        ui.selectable_value(&mut n, *v, text.clone());
                    }
                });
                self.options.set_putty_value(PUTTY_LOG[0], PuttyValue::Number(n));
                ui.end_row();
                ui.label(t!("plink-log-file"));
                let mut file = match self.options.putty_value(PUTTY_LOG[1]) {
                    PuttyValue::Text(t) => t,
                    PuttyValue::Number(n) => n.to_string(),
                };
                if n != 0 && file.is_empty() && !self.default_log.is_empty() {
                    file = self.default_log.clone();
                }
                ui.add_enabled(n != 0, egui::TextEdit::singleline(&mut file).desired_width(360.0));
                self.options.set_putty_value(PUTTY_LOG[1], PuttyValue::Text(file.clone()));
                ui.end_row();
                // the folder, once there is one without codes to fill in
                let folder = Path::new(&file).parent().filter(|f| f.is_dir()).map(Path::to_path_buf);
                if let (true, Some(folder)) = (n != 0, folder) {
                    ui.label("");
                    if ui.button(t!("plink-log-open-folder")).clicked() {
                        let _ = std::process::Command::new("explorer.exe").arg(&folder).spawn();
                    }
                    ui.end_row();
                }
            });
            ui.weak(t!("plink-log-note"));
        });
    }

    /// The PuTTY option pages for the chosen protocol.
    fn putty_pages(&self) -> Vec<(String, Vec<PuttyOption>)> {
        let mut pages = vec![(t!("putty-page-line"), PUTTY_LINE.to_vec())];
        if self.protocol != Protocol::Serial {
            pages.push((t!("putty-page-connection"), PUTTY_CONNECTION.to_vec()));
        }
        match self.protocol {
            Protocol::Telnet => pages.push(("Telnet".to_string(), PUTTY_TELNET.to_vec())),
            Protocol::Supdup => pages.push(("SUPDUP".to_string(), PUTTY_SUPDUP.to_vec())),
            _ => {}
        }
        pages
    }

    fn putty_ui(&mut self, ui: &mut egui::Ui) {
        let pages = self.putty_pages();
        if pages.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(t!("putty-options")).id_salt("putty-options").show(ui, |ui| {
            // one grid, so every page's fields line up
            egui::Grid::new("putty-pages").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                for (title, options) in pages {
                    ui.strong(title);
                    ui.end_row();
                    for option in options {
                        putty_row(ui, &mut self.options, option);
                    }
                }
            });
            ui.weak(t!("putty-options-note"));
        });
    }

    /// The session as entered; `name` is the alias (kept when editing,
    /// made by the app for a new one).
    pub fn session(&self, name: &str) -> Result<PlinkSession, String> {
        let port = match self.port.trim() {
            "" => None,
            p => Some(p.parse::<u16>().map_err(|_| t!("host-bad-port", port = p))?),
        };
        let serial = (self.protocol == Protocol::Serial)
            .then(|| -> Result<Serial, String> {
                let speed =
                    self.speed.trim().parse::<u32>().map_err(|_| t!("plink-bad-speed", speed = self.speed.trim()))?;
                Ok(Serial {
                    line: self.line.trim().to_uppercase(),
                    speed,
                    data_bits: self.data_bits,
                    parity: self.parity,
                    stop_bits: self.stop_bits.clone(),
                    flow: self.flow,
                })
            })
            .transpose()?;
        let network = self.protocol != Protocol::Serial;
        let charset = opt(&self.charset).filter(|c| !c.eq_ignore_ascii_case("utf-8"));
        let mut session = PlinkSession {
            name: name.to_string(),
            label: opt(&self.label),
            protocol: self.protocol,
            host: if network { opt(&self.host) } else { None },
            port: if network { port } else { None },
            user: if matches!(self.protocol, Protocol::Telnet | Protocol::Rlogin) { opt(&self.user) } else { None },
            charset,
            serial,
            note: opt(&self.note),
            on_login: opt(&self.on_login),
            pre_connect: opt(&self.pre_connect),
            // only the one that isn't the default is worth writing
            backspace: opt(&self.backspace).filter(|b| b != "^h"),
            ..self.base.clone()
        };
        // options of other protocols' pages don't apply any more
        let shown: Vec<PuttyOption> = self.putty_pages().into_iter().flat_map(|(_, options)| options).collect();
        for option in PUTTY_TELNET.iter().chain(&PUTTY_SUPDUP) {
            if !shown.contains(option) {
                session.putty.remove(option.key());
            }
        }
        for option in shown.into_iter().chain(PUTTY_LOG) {
            session.set_putty_value(option, self.options.putty_value(option));
        }
        // no log without a file (none is turned on without one)
        if session.log_file().is_none() {
            for option in PUTTY_LOG {
                session.putty.remove(option.key());
            }
        }
        session.check()?;
        Ok(session)
    }

    /// What a new session's alias is made from.
    pub fn name_base(&self) -> String {
        [&self.label, &self.host, &self.line]
            .into_iter()
            .map(|s| s.trim())
            .find(|s| !s.is_empty())
            .unwrap_or("session")
            .to_string()
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome<()> {
        let mut outcome = Outcome::Open;
        let mut open = true;
        egui::Window::new(self.title.clone())
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Grid::new("plink-fields").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    let field = |ui: &mut egui::Ui, name: String, value: &mut String, hint: String| {
                        ui.label(name);
                        ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(280.0));
                        ui.end_row();
                    };
                    field(ui, t!("field-name"), &mut self.label, t!("plink-name-hint"));
                    ui.label(t!("field-protocol"));
                    egui::ComboBox::from_id_salt("plink-protocol").selected_text(protocol_text(self.protocol)).show_ui(
                        ui,
                        |ui| {
                            for p in Protocol::ALL {
                                ui.selectable_value(&mut self.protocol, p, protocol_text(p));
                            }
                        },
                    );
                    ui.end_row();
                    if self.protocol == Protocol::Serial {
                        ui.label(t!("field-serial-line"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.line).hint_text("COM3").desired_width(120.0));
                            egui::ComboBox::from_id_salt("plink-ports")
                                .selected_text(if self.ports.is_empty() {
                                    t!("plink-no-ports")
                                } else {
                                    t!("plink-ports")
                                })
                                .show_ui(ui, |ui| {
                                    for port in &self.ports {
                                        ui.selectable_value(&mut self.line, port.clone(), port);
                                    }
                                });
                        });
                        ui.end_row();
                        ui.label(t!("field-serial-speed"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.speed).desired_width(120.0));
                            egui::ComboBox::from_id_salt("plink-speeds").selected_text(t!("plink-common")).show_ui(
                                ui,
                                |ui| {
                                    for speed in SPEEDS {
                                        ui.selectable_value(&mut self.speed, speed.to_string(), speed.to_string());
                                    }
                                },
                            );
                        });
                        ui.end_row();
                        ui.label(t!("field-serial-format"));
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("plink-data")
                                .selected_text(self.data_bits.to_string())
                                .width(48.0)
                                .show_ui(ui, |ui| {
                                    for bits in 5..=8u8 {
                                        ui.selectable_value(&mut self.data_bits, bits, bits.to_string());
                                    }
                                });
                            egui::ComboBox::from_id_salt("plink-parity")
                                .selected_text(parity_text(self.parity))
                                .show_ui(ui, |ui| {
                                    for p in Parity::ALL {
                                        ui.selectable_value(&mut self.parity, p, parity_text(p));
                                    }
                                });
                            egui::ComboBox::from_id_salt("plink-stop")
                                .selected_text(self.stop_bits.clone())
                                .width(48.0)
                                .show_ui(ui, |ui| {
                                    for s in STOP_BITS {
                                        ui.selectable_value(&mut self.stop_bits, s.to_string(), s);
                                    }
                                });
                        });
                        ui.end_row();
                        ui.label(t!("field-serial-flow"));
                        egui::ComboBox::from_id_salt("plink-flow").selected_text(flow_text(self.flow)).show_ui(
                            ui,
                            |ui| {
                                for f in Flow::ALL {
                                    ui.selectable_value(&mut self.flow, f, flow_text(f));
                                }
                            },
                        );
                        ui.end_row();
                    } else {
                        field(ui, t!("field-host"), &mut self.host, t!("field-host-hint"));
                        let default_port = self
                            .protocol
                            .default_port()
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| t!("plink-port-needed"));
                        field(ui, t!("field-port"), &mut self.port, default_port);
                        if matches!(self.protocol, Protocol::Telnet | Protocol::Rlogin) {
                            field(ui, t!("field-user"), &mut self.user, t!("plink-user-hint"));
                        }
                    }
                    ui.label(t!("field-charset"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.charset).desired_width(120.0));
                        egui::ComboBox::from_id_salt("plink-charsets").selected_text(t!("plink-common")).show_ui(
                            ui,
                            |ui| {
                                for c in CHARSETS {
                                    ui.selectable_value(&mut self.charset, c.to_string(), c);
                                }
                            },
                        );
                    });
                    ui.end_row();
                    ui.label(t!("field-backspace")).on_hover_text(t!("field-backspace-hint"));
                    egui::ComboBox::from_id_salt("plink-backspace")
                        .selected_text(backspace_text(&self.backspace))
                        .show_ui(ui, |ui| {
                            for value in ["^h", "^?"] {
                                ui.selectable_value(&mut self.backspace, value.to_string(), backspace_text(value));
                            }
                        });
                    ui.end_row();
                    field(ui, t!("field-note"), &mut self.note, t!("field-note-hint"));
                    field(ui, t!("field-on-login"), &mut self.on_login, t!("plink-on-login-hint"));
                    field(ui, t!("field-pre-connect"), &mut self.pre_connect, t!("field-pre-connect-hint"));
                    ui.label(t!("field-tags")).on_hover_text(t!("field-tags-hint"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.tags)
                            .hint_text(t!("field-tags-hint-short"))
                            .desired_width(280.0),
                    );
                    ui.end_row();
                    ui.label(t!("field-long-note")).on_hover_text(t!("field-long-note-hint"));
                    ui.add(
                        egui::TextEdit::multiline(&mut self.long_note)
                            .hint_text(t!("field-long-note-hint-short"))
                            .desired_rows(3)
                            .desired_width(280.0),
                    );
                    ui.end_row();
                });
                self.putty_ui(ui);
                self.log_ui(ui);
                ui.weak(t!("plink-note"));
                if let Some(alias) = &self.alias {
                    ui.weak(t!("plink-alias-kept", alias = alias.as_str()));
                }
                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x3a, 0x3a), error);
                }
                ui.horizontal(|ui| {
                    if ui.button(t!("button-save")).clicked() {
                        outcome = Outcome::Submit(());
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

/// One option: its name and an editor for its kind.
fn putty_row(ui: &mut egui::Ui, session: &mut PlinkSession, option: PuttyOption) {
    let key = option.key();
    let value = session.putty_value(option);
    // a switch carries its own text (readable by screen readers)
    if let PuttyOption::Flag { .. } = option {
        ui.label("");
    } else {
        ui.label(option_text(key));
    }
    match option {
        PuttyOption::Flag { .. } => {
            let mut on = value == PuttyValue::Number(1);
            if ui.checkbox(&mut on, option_text(key)).changed() {
                session.set_putty_value(option, PuttyValue::Number(u32::from(on)));
            }
        }
        PuttyOption::Number { .. } if matches!(key, "LocalEcho" | "LocalEdit") => {
            // stored 0 = on, 1 = off, 2 = automatic
            let choices = [(2u32, t!("putty-auto")), (0, t!("putty-on")), (1, t!("putty-off"))];
            let mut n = match value {
                PuttyValue::Number(n) if n <= 2 => n,
                _ => 2,
            };
            let shown = choices.iter().find(|(v, _)| *v == n).map(|(_, text)| text.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt(key).selected_text(shown).show_ui(ui, |ui| {
                for (v, text) in &choices {
                    ui.selectable_value(&mut n, *v, text.clone());
                }
            });
            session.set_putty_value(option, PuttyValue::Number(n));
        }
        PuttyOption::Number { .. } if key == "SUPDUPCharset" => {
            let names = ["None", "ITS", "WAITS"];
            let mut n = match value {
                PuttyValue::Number(n) => n.min(2),
                PuttyValue::Text(_) => 0,
            };
            egui::ComboBox::from_id_salt(key).selected_text(names[n as usize]).show_ui(ui, |ui| {
                for (i, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut n, i as u32, *name);
                }
            });
            session.set_putty_value(option, PuttyValue::Number(n));
        }
        PuttyOption::Number { .. } => {
            let mut text = match value {
                PuttyValue::Number(n) => n.to_string(),
                PuttyValue::Text(t) => t,
            };
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(80.0)).changed() {
                if let Ok(n) = text.trim().parse::<u32>() {
                    session.set_putty_value(option, PuttyValue::Number(n));
                } else if text.trim().is_empty() {
                    session.set_putty_value(option, option.default_value());
                }
            }
        }
        PuttyOption::Text { .. } => {
            let mut text = match value {
                PuttyValue::Text(t) => t,
                PuttyValue::Number(n) => n.to_string(),
            };
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0)).changed() {
                session.set_putty_value(option, PuttyValue::Text(text));
            }
        }
    }
    ui.end_row();
}

fn option_text(key: &str) -> String {
    match key {
        "TerminalType" => t!("putty-terminaltype"),
        "PingIntervalSecs" => t!("putty-pingintervalsecs"),
        "TCPNoDelay" => t!("putty-tcpnodelay"),
        "TCPKeepalives" => t!("putty-tcpkeepalives"),
        "PassiveTelnet" => t!("putty-passivetelnet"),
        "LocalEcho" => t!("putty-localecho"),
        "LocalEdit" => t!("putty-localedit"),
        "RFCEnviron" => t!("putty-rfcenviron"),
        "SUPDUPLocation" => t!("putty-supduplocation"),
        "SUPDUPCharset" => t!("putty-supdupcharset"),
        "SUPDUPMoreProcessing" => t!("putty-supdupmoreprocessing"),
        "SUPDUPScrolling" => t!("putty-supdupscrolling"),
        other => other.to_string(),
    }
}

/// The Backspace choice, in words.
fn backspace_text(value: &str) -> String {
    match native_term_config::plink::backspace_code(value) {
        Some("^?") => t!("backspace-delete"),
        _ => t!("backspace-control-h"),
    }
}

pub fn protocol_text(p: Protocol) -> String {
    match p {
        Protocol::Telnet => "Telnet".into(),
        Protocol::Rlogin => "rlogin".into(),
        Protocol::Raw => t!("protocol-raw"),
        Protocol::Serial => t!("protocol-serial"),
        Protocol::Supdup => "SUPDUP".into(),
    }
}

fn parity_text(p: Parity) -> String {
    match p {
        Parity::None => t!("parity-none"),
        Parity::Odd => t!("parity-odd"),
        Parity::Even => t!("parity-even"),
        Parity::Mark => t!("parity-mark"),
        Parity::Space => t!("parity-space"),
    }
}

fn flow_text(f: Flow) -> String {
    match f {
        Flow::None => t!("flow-none"),
        Flow::XonXoff => "XON/XOFF".into(),
        Flow::RtsCts => "RTS/CTS".into(),
        Flow::DsrDtr => "DSR/DTR".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_become_a_session() {
        let mut d = PlinkDialog::new_session(PathBuf::from("lab.conf"), "Lab");
        d.label = "核心交换机".into();
        d.host = "10.0.0.1".into();
        d.charset = "GBK".into();
        let s = d.session("core-sw").unwrap();
        assert_eq!(
            (s.protocol, s.host.as_deref(), s.port, s.charset.as_deref()),
            (Protocol::Telnet, Some("10.0.0.1"), None, Some("GBK"))
        );
        assert_eq!(d.name_base(), "核心交换机");

        d.protocol = Protocol::Serial;
        d.line = "com7".into();
        d.speed = "115200".into();
        let s = d.session("console").unwrap();
        assert_eq!(s.host, None, "a serial session has no host");
        assert_eq!(s.serial.unwrap().sercfg(), "115200,8,n,1,X");

        d.speed = "fast".into();
        assert!(d.session("console").is_err());
        d.protocol = Protocol::Raw;
        assert!(d.session("r").is_err(), "raw needs a port");
        d.port = "4001".into();
        assert!(d.session("r").is_ok());
    }

    #[test]
    fn putty_options_follow_the_protocol() {
        let mut d = PlinkDialog::new_session(PathBuf::from("lab.conf"), "Lab");
        d.host = "h".into();
        d.options.set_putty_value(PUTTY_TELNET[0], PuttyValue::Number(1));
        d.options.set_putty_value(PUTTY_CONNECTION[1], PuttyValue::Number(30));
        let s = d.session("sw").unwrap();
        assert_eq!(s.putty.len(), 2, "{:?}", s.putty);
        d.protocol = Protocol::Raw;
        d.port = "4001".into();
        let s = d.session("sw").unwrap();
        assert_eq!(s.putty.keys().collect::<Vec<_>>(), ["PingIntervalSecs"], "the Telnet page doesn't apply to raw");
    }

    #[test]
    fn a_log_needs_a_type_and_a_file() {
        let mut d = PlinkDialog::new_session(PathBuf::from("lab.conf"), "Lab").with_data_dir(Path::new(r"D:\NT"));
        d.host = "h".into();
        assert!(d.session("sw").unwrap().putty.is_empty(), "no log by default");
        d.options.set_putty_value(PUTTY_LOG[0], PuttyValue::Number(1));
        assert!(d.session("sw").unwrap().putty.is_empty(), "no file: not logged");
        d.options.set_putty_value(PUTTY_LOG[1], PuttyValue::Text(plink::default_log_file(Path::new(r"D:\NT"))));
        let s = d.session("sw").unwrap();
        assert_eq!(s.log_file().as_deref(), Some(r"D:\NT\logs\&H-&Y&M&D.log"));
        d.protocol = Protocol::Serial;
        d.line = "COM3".into();
        assert!(d.session("console").unwrap().log_file().is_some(), "every protocol");
        d.options.set_putty_value(PUTTY_LOG[0], PuttyValue::Number(0));
        assert!(d.session("console").unwrap().putty.is_empty(), "off: the file goes too");
    }

    #[test]
    fn editing_keeps_what_the_dialog_does_not_show() {
        let mut original =
            PlinkSession { name: "sw".into(), host: Some("h".into()), favorite: true, ..Default::default() };
        original.putty.insert("PassiveTelnet".into(), native_term_config::plink::PuttyValue::Number(1));
        original.id = Some("id-1".into());
        let d = PlinkDialog::edit(&original);
        let s = d.session("sw").unwrap();
        assert!(s.favorite);
        assert_eq!(s.id.as_deref(), Some("id-1"));
        assert_eq!(s.putty.len(), 1);
        assert_eq!(s.charset, None, "UTF-8 is the default");
    }
}
