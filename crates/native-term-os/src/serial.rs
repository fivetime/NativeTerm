//! The serial ports this computer has.

/// Port names to offer (`COM3`; `/dev/ttyUSB0`, `/dev/cu.usbserial-…`).
#[must_use]
pub fn ports() -> Vec<String> {
    #[cfg(windows)]
    {
        native_term_win::registry::serial_ports()
    }
    #[cfg(unix)]
    {
        let Ok(entries) = std::fs::read_dir("/dev") else { return Vec::new() };
        let mut ports: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                n.starts_with("ttyUSB")
                    || n.starts_with("ttyACM")
                    || n.starts_with("ttyAMA")
                    || (n.starts_with("cu.") && !n.starts_with("cu.Bluetooth"))
            })
            .map(|n| format!("/dev/{n}"))
            .collect();
        ports.sort();
        ports
    }
}
