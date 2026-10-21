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
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn recording() -> (Events, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        (
            Events::with(
                move |line| sink.lock().unwrap().push(line.to_owned()),
                || 1_760_000_000_000,
            ),
            lines,
        )
    }

    #[test]
    fn the_ring_keeps_the_newest_128_and_counts_what_it_drops() {
        let (mut events, _) = recording();
        for _ in 0..CAPACITY + 2 {
            events.record(EventCode::NoticeDropped, None);
        }
        let view = events.view();
        assert_eq!(view.entries.len(), CAPACITY);
        assert_eq!(view.dropped, "2");
        // Sequence numbers are never reused: the oldest two (1 and 2) are the ones gone.
        assert_eq!(view.entries.first().unwrap().seq, "3");
        assert_eq!(view.entries.last().unwrap().seq, (CAPACITY + 2).to_string());
        let seqs: Vec<u64> = view
            .entries
            .iter()
            .map(|e| e.seq.parse().unwrap())
            .collect();
        assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }

    #[test]
    fn an_event_carries_its_code_subject_and_time() {
        let (mut events, _) = recording();
        events.record(EventCode::PluginFailed, Some("org.rad.audio"));
        events.record(EventCode::Started, None);
        let view = events.view();
        assert_eq!(view.dropped, "0");
        assert_eq!(
            view.entries,
            vec![
                RuntimeEvent {
                    seq: "1".into(),
                    at_unix_ms: "1760000000000".into(),
                    code: EventCode::PluginFailed,
                    subject: Some("org.rad.audio".into()),
                },
                RuntimeEvent {
                    seq: "2".into(),
                    at_unix_ms: "1760000000000".into(),
                    code: EventCode::Started,
                    subject: None,
                },
            ]
        );
    }

    #[test]
    fn a_subject_is_bounded_on_a_character_boundary_and_stays_on_one_line() {
        let (mut events, lines) = recording();
        events.record(EventCode::PluginFailed, Some(&"é".repeat(300)));
        events.record(EventCode::PluginFailed, Some("a\nb\u{1b}[31mc\r"));
        let view = events.view();
        let long = view.entries[0].subject.as_deref().unwrap();
        assert!(long.len() <= SUBJECT_LIMIT, "{}", long.len());
        assert!(long.chars().all(|c| c == 'é'));
        // A subject cannot forge a second journal line or move the terminal.
        let odd = view.entries[1].subject.as_deref().unwrap();
        assert!(!odd.chars().any(char::is_control), "{odd:?}");
        for line in lines.lock().unwrap().iter() {
            assert!(
                !line.contains('\n') && !line.chars().any(char::is_control),
                "{line:?}"
            );
        }
    }

    #[test]
    fn every_event_is_one_stable_stderr_line() {
        assert_eq!(
            Events::line(EventCode::Started, None),
            "rad-runtime: started"
        );
        assert_eq!(
            Events::line(EventCode::PluginFailed, Some("org.rad.audio")),
            "rad-runtime: plugin_failed org.rad.audio"
        );
        let (mut events, lines) = recording();
        events.record(EventCode::DeckSpawnFailed, None);
        events.record(EventCode::PreviousExit, Some("storage"));
        assert_eq!(
            *lines.lock().unwrap(),
            [
                "rad-runtime: deck_spawn_failed",
                "rad-runtime: previous_exit storage"
            ]
        );
    }

    #[test]
    fn the_full_ring_fits_in_a_response() {
        let (mut events, _) = recording();
        for _ in 0..CAPACITY {
            events.record(EventCode::StorageRecoveryRequired, Some(&"é".repeat(300)));
        }
        let response = rad_api::Response {
            v: 1,
            id: Some(u64::MAX.to_string()),
            session_id: Some("f".repeat(32)),
            status: rad_api::Status::Ok,
            data: rad_api::ResponseData::RuntimeLog(events.view()),
            error: None,
        };
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(bytes.len() < 48 * 1024, "{}", bytes.len());
    }
    #[test]
    fn connection_failures_are_reported_at_most_once_a_minute_with_their_count() {
        let mut failures = ConnectionFailures::default();
        assert_eq!(failures.note(1, 1_000), Some(1));
        assert_eq!(failures.note(2, 2_000), None);
        assert_eq!(failures.note(1, 30_000), None);
        assert_eq!(failures.note(0, 60_999), None);
        assert_eq!(failures.note(0, 61_000), Some(3));
        assert_eq!(failures.note(0, 200_000), None);
        assert_eq!(failures.note(4, 200_001), Some(4));
    }
}
