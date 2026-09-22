//! A system service's state (for `ssh-agent`), read without changing
//! anything.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    /// Stopped, but may be started.
    Stopped,
    /// Not allowed to start (Windows: start type "Disabled", the default
    /// for `ssh-agent`).
    Disabled,
    NotInstalled,
    /// This system has no service of that kind to ask about.
    Unavailable,
}

#[cfg(windows)]
pub fn service_state(name: &str) -> ServiceState {
    use native_term_win::service::ServiceState as Win;
    match native_term_win::service::service_state(name) {
        Win::Running => ServiceState::Running,
        Win::Stopped => ServiceState::Stopped,
        Win::Disabled => ServiceState::Disabled,
        Win::NotInstalled => ServiceState::NotInstalled,
    }
}

#[cfg(unix)]
pub fn service_state(_name: &str) -> ServiceState {
    ServiceState::Unavailable
}
