//! Reading a Windows service's state (for `ssh-agent`), without changing
//! anything: starting or enabling a service needs an administrator.

use windows::core::HSTRING;
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfigW, QueryServiceStatus, QUERY_SERVICE_CONFIGW,
    SC_MANAGER_CONNECT, SERVICE_DISABLED, SERVICE_QUERY_CONFIG, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Running,
    /// Stopped, but may be started.
    Stopped,
    /// Start type "Disabled" (the default for `ssh-agent`).
    Disabled,
    NotInstalled,
}

pub fn service_state(name: &str) -> ServiceState {
    unsafe {
        let Ok(manager) = OpenSCManagerW(None, None, SC_MANAGER_CONNECT) else { return ServiceState::NotInstalled };
        let state = match OpenServiceW(manager, &HSTRING::from(name), SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG) {
            Err(_) => ServiceState::NotInstalled,
            Ok(service) => {
                let mut status = SERVICE_STATUS::default();
                let running =
                    QueryServiceStatus(service, &mut status).is_ok() && status.dwCurrentState == SERVICE_RUNNING;
                let mut needed = 0u32;
                let _ = QueryServiceConfigW(service, None, 0, &mut needed);
                let mut buffer = vec![0u8; needed as usize];
                let disabled = needed > 0
                    && QueryServiceConfigW(
                        service,
                        Some(buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>()),
                        needed,
                        &mut needed,
                    )
                    .is_ok()
                    && (*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()).dwStartType == SERVICE_DISABLED;
                let _ = CloseServiceHandle(service);
                if running {
                    ServiceState::Running
                } else if disabled {
                    ServiceState::Disabled
                } else {
                    ServiceState::Stopped
                }
            }
        };
        let _ = CloseServiceHandle(manager);
        state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_and_unknown_services() {
        // the event log always runs
        assert_eq!(service_state("EventLog"), ServiceState::Running);
        assert_eq!(service_state("nativeterm-no-such-service"), ServiceState::NotInstalled);
        println!("ssh-agent: {:?}", service_state("ssh-agent"));
    }
}
