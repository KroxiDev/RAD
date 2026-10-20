//! Native application launcher. Acceptance is not application readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherError {
    Clock,
    Io,
    Unavailable,
    Capacity,
    InvalidRequest,
    Protocol,
}
/// Linux CLOCK_MONOTONIC epoch shared with the Engine and helper.
pub fn monotonic_ms() -> Result<u64, LauncherError> {
    let mut ts = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: valid clock and writable storage, initialized only after success.
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, ts.as_mut_ptr()) } != 0 {
        return Err(LauncherError::Clock);
    }
    // SAFETY: clock_gettime returned success.
    let ts = unsafe { ts.assume_init() };
    let seconds = u64::try_from(ts.tv_sec).map_err(|_| LauncherError::Clock)?;
    let nanos = u64::try_from(ts.tv_nsec).map_err(|_| LauncherError::Clock)?;
    if nanos >= 1_000_000_000 {
        return Err(LauncherError::Clock);
    }
    seconds
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add(nanos / 1_000_000))
        .ok_or(LauncherError::Clock)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOutcome {
    Accepted,
    Failed,
    Expired,
    HelperLost,
}
fn resolve_before_deadline<T>(
    deadline: u64,
    clock: impl Fn() -> Result<u64, LauncherError>,
    resolve: impl FnOnce() -> Option<T>,
) -> Result<T, LaunchOutcome> {
    if clock().map_err(|_| LaunchOutcome::Failed)? >= deadline {
        return Err(LaunchOutcome::Expired);
    }
    let candidate = resolve().ok_or(LaunchOutcome::Failed)?;
    if clock().map_err(|_| LaunchOutcome::Failed)? >= deadline {
        return Err(LaunchOutcome::Expired);
    }
    Ok(candidate)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolver_blocked_past_deadline_cannot_return_a_launch_candidate() {
        let now = std::cell::Cell::new(100);
        let result = resolve_before_deadline(
            105,
            || Ok(now.get()),
            || {
                now.set(105);
                Some(())
            },
        );
        assert_eq!(result, Err(LaunchOutcome::Expired));
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: rad_core::configuration::DesktopEntry,
    pub name: String,
}
mod adapter;
pub use adapter::Launcher;
#[doc(hidden)]
pub mod helper;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconPreview {
    pub id: rad_core::configuration::DesktopEntry,
    pub image: rad_media::NormalizedImage,
    pub generic: bool,
}
mod icons;
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}
fn unhex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let digit = |c: u8| match c {
                b'0'..=b'9' => Some(c - b'0'),
                b'a'..=b'f' => Some(c - b'a' + 10),
                _ => None,
            };
            Some(digit(p[0])? * 16 + digit(p[1])?)
        })
        .collect()
}
