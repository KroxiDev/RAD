//! Per-process bounded diagnostic lines. No unbounded line or flood can retain memory.
use std::collections::VecDeque;
use zeroize::Zeroize;
pub const MAX_LINES: usize = 32;
pub const MAX_LINE_BYTES: usize = 1024;
#[derive(Default)]
pub(super) struct Stderr {
    partial: Vec<u8>,
    dropping: bool,
    scan_tail: Vec<u8>,
    window: u64,
    emitted: usize,
    pub lines: VecDeque<String>,
    pub credential_leak_detected: bool,
}
impl Stderr {
    pub fn ingest(&mut self, bytes: &[u8], now: u64, secrets: &[&str]) -> Vec<String> {
        // Scan even discarded/rate-limited bytes, including secrets split over read boundaries.
        let mut scan = zeroize::Zeroizing::new(self.scan_tail.clone());
        scan.extend_from_slice(bytes);
        self.credential_leak_detected |= secrets.iter().any(|secret| {
            !secret.is_empty()
                && scan
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
        });
        self.scan_tail.zeroize();
        self.scan_tail.clear();
        if !secrets.is_empty() {
            let keep = scan.len().min(4095);
            self.scan_tail.extend_from_slice(&scan[scan.len() - keep..]);
        }
        if now.saturating_sub(self.window) >= 1000 {
            self.window = now;
            self.emitted = 0;
        }
        let mut accepted = vec![];
        for &byte in bytes {
            if byte == b'\n' {
                if !self.dropping && !self.partial.is_empty() && self.emitted < 8 {
                    let raw = zeroize::Zeroizing::new(
                        String::from_utf8_lossy(&self.partial)
                            .chars()
                            .filter(|c| !c.is_control())
                            .collect::<String>(),
                    );
                    // Match the representation we publish, including each separately
                    // emitted component of a multiline credential. Mark against the
                    // original line so overlapping secrets cannot hide each other.
                    let mut masked = vec![false; raw.len()];
                    for secret in secrets {
                        for part in secret.split('\n') {
                            let part = zeroize::Zeroizing::new(
                                part.chars().filter(|c| !c.is_control()).collect::<String>(),
                            );
                            if !part.is_empty() {
                                for (start, _) in raw.char_indices() {
                                    if raw[start..].starts_with(part.as_str()) {
                                        masked[start..start + part.len()].fill(true);
                                        self.credential_leak_detected = true;
                                    }
                                }
                            }
                        }
                    }
                    let mut line = String::new();
                    let mut redacting = false;
                    for (offset, ch) in raw.char_indices() {
                        if masked[offset] {
                            if !redacting && line.len() + 10 <= MAX_LINE_BYTES {
                                line.push_str("[redacted]");
                            }
                            redacting = true;
                        } else {
                            if line.len() + ch.len_utf8() > MAX_LINE_BYTES {
                                break;
                            }
                            line.push(ch);
                            redacting = false;
                        }
                    }
                    if self.lines.len() == MAX_LINES {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(line.clone());
                    accepted.push(line);
                    self.emitted += 1;
                }
                self.partial.clear();
                self.dropping = false;
            } else if !self.dropping {
                if self.partial.len() == MAX_LINE_BYTES {
                    self.partial.clear();
                    self.dropping = true;
                } else {
                    self.partial.push(byte);
                }
            }
        }
        accepted
    }
}

/// Journald native datagram transport: never block the host on a stalled journal.
pub(super) fn journal(id: &str, line: &str) {
    if let Ok(socket) = std::os::unix::net::UnixDatagram::unbound()
        && socket.set_nonblocking(true).is_ok()
    {
        let message = format!("PRIORITY=6\nSYSLOG_IDENTIFIER=rad-plugin-{id}\nMESSAGE={line}");
        let _ = socket.send_to(message.as_bytes(), "/run/systemd/journal/socket");
    }
}

impl Drop for Stderr {
    fn drop(&mut self) {
        self.partial.zeroize();
        self.scan_tail.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_sanitized_and_multiline_credentials_across_reads() {
        for secret in [
            "topSECRET\tcanary",
            "first-secret\nsecond-secret",
            "firstcanary\r\n\tsecond\u{0085}secret",
        ] {
            for width in [1, 2, 7, 4096] {
                let mut capture = Stderr::default();
                let input = format!("before\n{secret}\nafter\n");
                let mut emitted = vec![];
                for chunk in input.as_bytes().chunks(width) {
                    emitted.extend(capture.ingest(chunk, 0, &[secret]));
                }
                assert!(capture.credential_leak_detected);
                assert_eq!(emitted.first().unwrap(), "before");
                assert_eq!(emitted.last().unwrap(), "after");
                for part in secret.split('\n') {
                    let normalized: String = part.chars().filter(|c| !c.is_control()).collect();
                    if !normalized.is_empty() {
                        assert!(emitted.iter().all(|line| !line.contains(&normalized)));
                        assert!(capture.lines.iter().all(|line| !line.contains(&normalized)));
                    }
                }
            }
        }
    }

    #[test]
    fn redaction_handles_overlaps_invalid_utf8_and_expansion_within_quota() {
        let mut capture = Stderr::default();
        assert_eq!(
            capture.ingest(b"ABABA\n", 0, &["ABA", "BAB"]),
            ["[redacted]"]
        );
        assert_eq!(capture.ingest(b"clean\n", 0, &["\t\n\r", ""]), ["clean"]);
        let mut invalid = vec![0xff; MAX_LINE_BYTES - 3];
        invalid.extend_from_slice(b"KEY\n");
        let output = capture.ingest(&invalid, 0, &["KEY"]);
        assert_eq!(output.len(), 1);
        assert!(output[0].len() <= MAX_LINE_BYTES);
        assert!(!output[0].contains("KEY"));
        let mut repeated = b"x".repeat(MAX_LINE_BYTES);
        repeated.push(b'\n');
        let output = capture.ingest(&repeated, 0, &["x"]);
        assert_eq!(output, ["[redacted]"]);
        let output = capture.ingest(b"topSECRETcanary\n", 0, &["topSECRET\tcanary"]);
        assert_eq!(output, ["[redacted]"]);
        assert!(
            capture
                .lines
                .iter()
                .all(|line| line.len() <= MAX_LINE_BYTES)
        );
    }

    #[test]
    fn bounded_stderr_redacts_and_detects_leaks_even_after_rate_limit() {
        let mut capture = Stderr::default();
        capture.ingest(&b"ordinary\n".repeat(100), 0, &["CANARY"]);
        assert_eq!(capture.lines.len(), 8);
        capture.ingest(b"CA", 0, &["CANARY"]);
        capture.ingest(b"NARY\n", 0, &["CANARY"]);
        assert!(capture.credential_leak_detected);
        capture.ingest(b"password CANARY\n", 1000, &["CANARY"]);
        assert_eq!(capture.lines.back().unwrap(), "password [redacted]");
        capture.ingest(&vec![b'x'; 100_000], 1000, &[]);
        capture.ingest(b"\n", 1000, &[]);
        assert!(
            capture
                .lines
                .iter()
                .all(|line| line.len() <= MAX_LINE_BYTES)
        );
        for now in 2..20 {
            capture.ingest(&b"ordinary\n".repeat(8), now * 1000, &[]);
        }
        assert_eq!(capture.lines.len(), MAX_LINES);
    }
}
