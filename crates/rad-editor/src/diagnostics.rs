//! Technical facts the runtime reports about itself, worded for Advanced and «View details».
use serde_json::Value;

/// `YYYY-MM-DD HH:MM:SS` for a Unix time in milliseconds shifted by `offset_s` seconds.
pub fn format_time(unix_ms: u64, offset_s: i64) -> String {
    let seconds = (unix_ms / 1000) as i64 + offset_s;
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let m = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * m + 2) / 5 + 1;
    let month = if m < 10 { m + 3 } else { m - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Seconds east of UTC that the system's time zone has at that moment; 0 if it cannot say.
pub fn local_offset(unix_ms: u64) -> i64 {
    /// The head of glibc's and musl's `struct tm`; only `tm_gmtoff` is read.
    #[repr(C)]
    struct Tm {
        fields: [i32; 9],
        gmtoff: i64,
        zone: *const u8,
    }
    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let time = (unix_ms / 1000) as i64;
    let mut tm = Tm {
        fields: [0; 9],
        gmtoff: 0,
        zone: std::ptr::null(),
    };
    // SAFETY: `time` and `tm` are valid for the call, and `Tm` has the layout of the C
    // struct on Linux; `localtime_r` writes only into `tm`.
    let filled = unsafe { localtime_r(&time, &mut tm) };
    if filled.is_null() { 0 } else { tm.gmtoff }
}

/// The user's local time for a Unix time in milliseconds sent as a decimal string.
pub fn local_time(unix_ms: &str) -> String {
    let Ok(ms) = unix_ms.parse::<u64>() else {
        return unix_ms.into();
    };
    format_time(ms, local_offset(ms))
}

/// `code · time` for the previous run's unexpected end, if the overview reports one.
pub fn previous_exit(overview: &Value) -> Option<(String, String)> {
    let exit = overview.get("previous_exit").filter(|v| v.is_object())?;
    Some((
        exit["code"].as_str()?.to_owned(),
        exit["at_unix_ms"].as_str()?.to_owned(),
    ))
}
pub fn previous_exit_details(code: &str, at_unix_ms: &str) -> String {
    format!("{code} · {}", local_time(at_unix_ms))
}

/// One read-only line per runtime event: `time · code subject`.
pub fn log_line(entry: &rad_api::RuntimeEvent) -> String {
    let line = format!(
        "{} · {}",
        local_time(&entry.at_unix_ms),
        entry.code.as_str()
    );
    match &entry.subject {
        Some(subject) => format!("{line} {subject}"),
        None => line,
    }
}
