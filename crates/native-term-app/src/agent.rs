//! "SSH keys and ssh-agent" in Settings, and a hint when a key has a
//! passphrase while ssh-agent isn't running (then every connect asks for
//! it). Checked in the background; nothing is changed without the user.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use native_term_app::{t, Core};
use native_term_config::keys::{self, AgentKeys};
use native_term_win::service::{service_state, ServiceState};

use crate::icons;

/// What an administrator runs to turn the agent on.
pub const ENABLE_COMMAND: &str = "Set-Service ssh-agent -StartupType Automatic; Start-Service ssh-agent";
/// `state.db` setting: the hint was dismissed.
pub const HINT_SETTING: &str = "agent_hint_dismissed";

#[derive(Clone, Debug)]
pub struct Status {
    pub service: ServiceState,
    /// (private key, has a passphrase)
    pub keys: Vec<(PathBuf, Option<bool>)>,
    pub agent: AgentKeys,
    /// `SSH_AUTH_SOCK` points ssh at another agent.
    pub other_agent: bool,
}

impl Status {
    pub fn check(ssh_dir: &Path) -> Status {
        let ssh = native_term_session::ssh_program();
        let keygen = keys::tool_for(&ssh, "ssh-keygen");
        let service = service_state("ssh-agent");
        let keys = keys::private_keys(ssh_dir)
            .into_iter()
            .map(|k| {
                let protected = keys::has_passphrase(&keygen, &k);
                (k, protected)
            })
            .collect();
        let other_agent = std::env::var_os("SSH_AUTH_SOCK").is_some_and(|s| !s.is_empty());
        let agent = if service == ServiceState::Running || other_agent {
            keys::agent_keys(&keys::tool_for(&ssh, "ssh-add"))
        } else {
            AgentKeys::Unreachable
        };
        Status { service, keys, agent, other_agent }
    }

    /// A key with a passphrase and no agent: every connect asks for it.
    pub fn needs_agent(&self) -> bool {
        let agent = self.service == ServiceState::Running || (self.other_agent && self.agent != AgentKeys::Unreachable);
        !agent && self.keys.iter().any(|(_, p)| *p == Some(true))
    }
}

#[derive(Default)]
pub struct AgentCheck {
    status: Arc<Mutex<Option<Status>>>,
}

impl AgentCheck {
    /// Check now, in the background.
    pub fn refresh(&self, ssh_dir: &Path, ctx: &egui::Context) {
        let slot = Arc::clone(&self.status);
        let dir = ssh_dir.to_path_buf();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let status = Status::check(&dir);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(status);
            ctx.request_repaint();
        });
    }

    pub fn status(&self) -> Option<Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The hint line at the top (unless dismissed).
    pub fn banner(&self, ui: &mut egui::Ui, core: Option<&Core>, open_settings: &mut bool) {
        let Some(status) = self.status() else { return };
        let dismissed = core.and_then(|c| c.setting(HINT_SETTING)).as_deref() == Some("1");
        if !status.needs_agent() || dismissed {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(egui::Color32::from_rgb(0xd0, 0x9a, 0x1a), t!("agent-hint"));
            if ui.small_button(t!("agent-hint-show")).clicked() {
                *open_settings = true;
            }
            if ui.small_button(t!("agent-hint-dismiss")).clicked() {
                if let Some(core) = core {
                    core.set_setting(HINT_SETTING, "1");
                }
            }
        });
    }

    pub fn settings_ui(&self, ui: &mut egui::Ui, ssh_dir: &Path, core: Option<&Core>) {
        ui.strong(t!("agent-section"));
        let Some(status) = self.status() else {
            ui.weak(t!("agent-checking"));
            return;
        };
        let service = match status.service {
            ServiceState::Running => t!("agent-running"),
            ServiceState::Stopped => t!("agent-stopped"),
            ServiceState::Disabled => t!("agent-disabled"),
            ServiceState::NotInstalled => t!("agent-missing"),
        };
        ui.label(t!("agent-service", state = service));
        if status.keys.is_empty() {
            ui.weak(t!("agent-no-keys", dir = ssh_dir.display().to_string()));
        }
        for (key, protected) in &status.keys {
            let name = key.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let what = match protected {
                Some(true) => t!("agent-key-protected"),
                Some(false) => t!("agent-key-open"),
                None => t!("agent-key-unknown"),
            };
            ui.label(format!("  {} {name}: {what}", icons::KEY));
        }
        if status.other_agent {
            ui.weak(t!("agent-other"));
        }
        if status.service == ServiceState::Running || status.other_agent {
            let loaded = match status.agent {
                AgentKeys::Some => t!("agent-loaded"),
                AgentKeys::None => t!("agent-empty"),
                AgentKeys::Unreachable => t!("agent-unreachable"),
            };
            ui.label(loaded);
            if status.agent == AgentKeys::None && !status.keys.is_empty() {
                if let Some(core) = core {
                    if ui.button(t!("agent-add")).clicked() {
                        let _ = core.terminal().open_tool(&t!("agent-add-tab"), &["--add-keys".into()]);
                    }
                }
            }
        } else if status.needs_agent() {
            ui.label(t!("agent-why"));
            ui.horizontal(|ui| {
                ui.code(ENABLE_COMMAND);
                if ui.small_button(t!("agent-copy")).clicked() {
                    ui.ctx().copy_text(ENABLE_COMMAND.to_string());
                }
            });
            ui.weak(t!("agent-admin"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_only_for_protected_keys_without_agent() {
        let key = |p: Option<bool>| (PathBuf::from("id"), p);
        let status = |service, keys| Status { service, keys, agent: AgentKeys::Unreachable, other_agent: false };
        let other = Status {
            service: ServiceState::Disabled,
            keys: vec![key(Some(true))],
            agent: AgentKeys::None,
            other_agent: true,
        };
        assert!(!other.needs_agent(), "SSH_AUTH_SOCK names an agent that answers");
        assert!(status(ServiceState::Disabled, vec![key(Some(true))]).needs_agent());
        assert!(!status(ServiceState::Running, vec![key(Some(true))]).needs_agent());
        assert!(!status(ServiceState::Disabled, vec![key(Some(false))]).needs_agent());
        assert!(!status(ServiceState::Stopped, vec![]).needs_agent());
    }
}
