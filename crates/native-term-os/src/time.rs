//! Local time for seconds since the Unix epoch, as text.

#[cfg(windows)]
pub use native_term_win::{local_date_time, local_time_of_day};

#[cfg(unix)]
mod unix {
    /// The broken-down local time, or `None` for a time libc can't place.
    fn local(unix: u64) -> Option<libc::tm> {
        let t = libc::time_t::try_from(unix).ok()?;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: `t` and `tm` are ours and live through the call;
        // `localtime_r` writes only into `tm` and returns null on failure.
        let done = unsafe { libc::localtime_r(&t, &mut tm) };
        (!done.is_null()).then_some(tm)
    }

    /// `YYYY-MM-DD HH:MM` in local time for seconds since the Unix epoch.
    pub fn local_date_time(unix: u64) -> String {
        match local(unix) {
            Some(t) => {
                format!("{:04}-{:02}-{:02} {:02}:{:02}", t.tm_year + 1900, t.tm_mon + 1, t.tm_mday, t.tm_hour, t.tm_min)
            }
            None => String::new(),
        }
    }

    /// `HH:MM` in local time for seconds since the Unix epoch.
    pub fn local_time_of_day(unix: u64) -> String {
        match local(unix) {
            Some(t) => format!("{:02}:{:02}", t.tm_hour, t.tm_min),
            None => String::new(),
        }
    }
}

#[cfg(unix)]
pub use unix::{local_date_time, local_time_of_day};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        // 2024-01-15 12:34:56 UTC; the local date may be a day off
        let date = local_date_time(1_705_322_096);
        assert_eq!(date.len(), 16, "{date}");
        assert!(date.starts_with("2024-01-1"), "{date}");
        let time = local_time_of_day(1_705_322_096);
        assert_eq!(time.len(), 5, "{time}");
        assert_eq!(&time[2..3], ":");
        assert_eq!(&date[11..], time);
    }
}
