//! New or edited non-SSH session (Telnet, rlogin, raw, serial, SUPDUP),
//! kept in the folder's `.nt.toml` and run with plink.

use std::path::PathBuf;

use native_term_app::t;
use native_term_config::plink::{Flow, Parity, PlinkSession, Protocol, Serial};

use crate::dialogs::Outcome;

/// Charsets offered first; any name `plink::code_page` knows, or a code
/// page number, can be typed.
const CHARSETS: [&str; 5] = ["UTF-8", "GBK", "Big5", "Shift_JIS", "EUC-KR"];
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
    line: String,
    speed: String,
    data_bits: u8,
    parity: Parity,
    stop_bits: String,
    flow: Flow,
    note: String,
    on_login: String,
    /// Serial ports present when the dialog opened.
    ports: Vec<String>,
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
        let ports = native_term_win::registry::serial_ports();
        let line = if serial.line.is_empty() { ports.first().cloned().unwrap_or_default() } else { serial.line.clone() };
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
            line,
            speed: serial.speed.to_string(),
            data_bits: serial.data_bits,
            parity: serial.parity,
            stop_bits: serial.stop_bits.clone(),
            flow: serial.flow,
            note: s.note.clone().unwrap_or_default(),
            on_login: s.on_login.clone().unwrap_or_default(),
            ports,
            error: None,
        }
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
                let speed = self.speed.trim().parse::<u32>().map_err(|_| t!("plink-bad-speed", speed = self.speed.trim()))?;
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
        let session = PlinkSession {
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
            ..self.base.clone()
        };
        session.check()?;
        Ok(session)
    }

    /// What a new session's alias is made from.
    pub fn name_base(&self) -> String {
        [&self.label, &self.host, &self.line].into_iter().map(|s| s.trim()).find(|s| !s.is_empty()).unwrap_or("session").to_string()
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
                    egui::ComboBox::from_id_salt("plink-protocol")
                        .selected_text(protocol_text(self.protocol))
                        .show_ui(ui, |ui| {
                            for p in Protocol::ALL {
                                ui.selectable_value(&mut self.protocol, p, protocol_text(p));
                            }
                        });
                    ui.end_row();
                    if self.protocol == Protocol::Serial {
                        ui.label(t!("field-serial-line"));
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.line).hint_text("COM3").desired_width(120.0));
                            egui::ComboBox::from_id_salt("plink-ports")
                                .selected_text(if self.ports.is_empty() { t!("plink-no-ports") } else { t!("plink-ports") })
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
                            egui::ComboBox::from_id_salt("plink-speeds").selected_text(t!("plink-common")).show_ui(ui, |ui| {
                                for speed in SPEEDS {
                                    ui.selectable_value(&mut self.speed, speed.to_string(), speed.to_string());
                                }
                            });
                        });
                        ui.end_row();
                        ui.label(t!("field-serial-format"));
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("plink-data").selected_text(self.data_bits.to_string()).width(48.0).show_ui(
                                ui,
                                |ui| {
                                    for bits in 5..=8u8 {
                                        ui.selectable_value(&mut self.data_bits, bits, bits.to_string());
                                    }
                                },
                            );
                            egui::ComboBox::from_id_salt("plink-parity").selected_text(parity_text(self.parity)).show_ui(ui, |ui| {
                                for p in Parity::ALL {
                                    ui.selectable_value(&mut self.parity, p, parity_text(p));
                                }
                            });
                            egui::ComboBox::from_id_salt("plink-stop").selected_text(self.stop_bits.clone()).width(48.0).show_ui(
                                ui,
                                |ui| {
                                    for s in STOP_BITS {
                                        ui.selectable_value(&mut self.stop_bits, s.to_string(), s);
                                    }
                                },
                            );
                        });
                        ui.end_row();
                        ui.label(t!("field-serial-flow"));
                        egui::ComboBox::from_id_salt("plink-flow").selected_text(flow_text(self.flow)).show_ui(ui, |ui| {
                            for f in Flow::ALL {
                                ui.selectable_value(&mut self.flow, f, flow_text(f));
                            }
                        });
                        ui.end_row();
                    } else {
                        field(ui, t!("field-host"), &mut self.host, t!("field-host-hint"));
                        let default_port = self.protocol.default_port().map(|p| p.to_string()).unwrap_or_else(|| t!("plink-port-needed"));
                        field(ui, t!("field-port"), &mut self.port, default_port);
                        if matches!(self.protocol, Protocol::Telnet | Protocol::Rlogin) {
                            field(ui, t!("field-user"), &mut self.user, t!("plink-user-hint"));
                        }
                    }
                    ui.label(t!("field-charset"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.charset).desired_width(120.0));
                        egui::ComboBox::from_id_salt("plink-charsets").selected_text(t!("plink-common")).show_ui(ui, |ui| {
                            for c in CHARSETS {
                                ui.selectable_value(&mut self.charset, c.to_string(), c);
                            }
                        });
                    });
                    ui.end_row();
                    field(ui, t!("field-note"), &mut self.note, t!("field-note-hint"));
                    field(ui, t!("field-on-login"), &mut self.on_login, t!("plink-on-login-hint"));
                });
                ui.weak(t!("plink-note"));
                if let Some(alias) = &self.alias {
                    ui.weak(t!("host-alias-kept", alias = alias.as_str()));
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
        assert_eq!((s.protocol, s.host.as_deref(), s.port, s.charset.as_deref()), (Protocol::Telnet, Some("10.0.0.1"), None, Some("GBK")));
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
    fn editing_keeps_what_the_dialog_does_not_show() {
        let mut original = PlinkSession { name: "sw".into(), host: Some("h".into()), favorite: true, ..Default::default() };
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
