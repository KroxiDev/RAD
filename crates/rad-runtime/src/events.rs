//! What the runtime reports about itself: a bounded ring of recent events for
//! Advanced → Log, and the same event as one stderr line for journald (ADR 0011).
//!
//! Events are state changes and contained failures, never per-request or per-key traffic:
//! in normal use the ring does not fill. Codes are the closed [`rad_api::EventCode`]
//! vocabulary; the subject, when there is one, is an identifier (a plugin id, an exit
//! reason), never prose or data from a failure.
use rad_api::{EventCode, RuntimeEvent, RuntimeLogView};
use std::collections::VecDeque;

/// Events kept; older ones are dropped and counted.
pub const CAPACITY: usize = 128;
/// Bytes of subject kept.
pub const SUBJECT_LIMIT: usize = 128;

struct Entry {
    seq: u64,
    at_unix_ms: u64,
    code: EventCode,
    subject: Option<String>,
}

/// An event raised below the server (the launcher, the plugin host) and carried up to be
/// recorded by whoever owns the ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occurrence {
    pub code: EventCode,
    pub subject: Option<String>,
}
impl Occurrence {
    pub fn new(code: EventCode, subject: Option<&str>) -> Self {
        Self {
            code,
            subject: subject.map(str::to_owned),
        }
    }
}

pub struct Events {
    ring: VecDeque<Entry>,
    next_seq: u64,
    dropped: u64,
    sink: Box<dyn FnMut(&str) + Send>,
    clock: fn() -> u64,
}
impl Events {
    /// Journald through stderr, with the system clock.
    pub fn new() -> Self {
        Self::with(|line| eprintln!("{line}"), crate::exit::now_unix_ms)
    }
    /// The seam for tests: where lines go and what time it is.
    pub fn with(sink: impl FnMut(&str) + Send + 'static, clock: fn() -> u64) -> Self {
        Self {
            ring: VecDeque::new(),
            next_seq: 1,
            dropped: 0,
            sink: Box::new(sink),
            clock,
        }
    }
    /// `rad-runtime: <code>[ <subject>]`: the stable form of an event on stderr.
    pub fn line(code: EventCode, subject: Option<&str>) -> String {
        match subject {
            Some(subject) => format!("rad-runtime: {} {subject}", code.as_str()),
            None => format!("rad-runtime: {}", code.as_str()),
        }
    }
    pub fn record(&mut self, code: EventCode, subject: Option<&str>) {
        let subject = subject.map(bounded).filter(|subject| !subject.is_empty());
        (self.sink)(&Self::line(code, subject.as_deref()));
        if self.ring.len() == CAPACITY {
            self.ring.pop_front();
            self.dropped += 1;
        }
        self.ring.push_back(Entry {
            seq: self.next_seq,
            at_unix_ms: (self.clock)(),
            code,
            subject,
        });
        self.next_seq += 1;
    }
    pub fn record_all(&mut self, occurrences: impl IntoIterator<Item = Occurrence>) {
        for occurrence in occurrences {
            self.record(occurrence.code, occurrence.subject.as_deref());
        }
    }
    pub fn view(&self) -> RuntimeLogView {
        RuntimeLogView {
            entries: self
                .ring
                .iter()
                .map(|entry| RuntimeEvent {
                    seq: entry.seq.to_string(),
                    at_unix_ms: entry.at_unix_ms.to_string(),
                    code: entry.code,
                    subject: entry.subject.clone(),
                })
                .collect(),
            dropped: self.dropped.to_string(),
        }
    }
}
/// A subject is one printable line of at most [`SUBJECT_LIMIT`] bytes: it also goes to the
/// journal, where a newline or an escape sequence would forge or hide lines.
fn bounded(subject: &str) -> String {
    let mut out = String::new();
    for c in subject.chars() {
        let c = if c.is_control() { '?' } else { c };
        if out.len() + c.len_utf8() > SUBJECT_LIMIT {
            break;
        }
        out.push(c);
    }
    out
}
impl Default for Events {
    fn default() -> Self {
        Self::new()
    }
}

/// Connections the transport could not take in, reported at most once a minute with
/// how many there were: running out of descriptors can last, and one event per turn
/// would push every useful entry out of the ring.
#[derive(Default)]
pub(crate) struct ConnectionFailures {
    unreported: u64,
    last_report: Option<u64>,
}
impl ConnectionFailures {
    pub const INTERVAL_MS: u64 = 60_000;
    /// Adds `count` new failures seen at monotonic `now_ms`; the total to report, if due.
    pub fn note(&mut self, count: u64, now_ms: u64) -> Option<u64> {
        self.unreported = self.unreported.saturating_add(count);
        let due = self
            .last_report
            .is_none_or(|last| now_ms >= last.saturating_add(Self::INTERVAL_MS));
        if self.unreported == 0 || !due {
            return None;
        }
        self.last_report = Some(now_ms);
        Some(std::mem::take(&mut self.unreported))
    }
}
