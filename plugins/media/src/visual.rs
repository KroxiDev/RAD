use crate::{
    Timing,
    bus::{Bus, Failure},
    cover,
    player::{self, Status},
    source::Source,
};
use rad_plugin_sdk::{VisualContent, VisualState};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Instant, SystemTime},
};

const PLAYING: [u8; 4] = [40, 167, 69, 255];
const PAUSED: [u8; 4] = [255, 193, 7, 255];
const STOPPED: [u8; 4] = [108, 117, 125, 255];

#[derive(PartialEq)]
struct CoverKey {
    path: PathBuf,
    length: u64,
    modified: Option<SystemTime>,
}

pub(crate) struct Reader {
    app: String,
    converter: Arc<cover::Converter>,
    cover: Option<(CoverKey, Option<String>)>,
}

impl Reader {
    pub(crate) fn new(app: String, converter: Arc<cover::Converter>) -> Self {
        Self {
            app,
            converter,
            cover: None,
        }
    }

    pub(crate) fn read(
        &mut self,
        bus: &Bus,
        timing: &Timing,
        source: &Source,
        shown: Option<&VisualState>,
    ) -> VisualState {
        let deadline = Instant::now() + timing.refresh_deadline;
        let stale = || match shown {
            Some(VisualState::Current { content } | VisualState::Stale { content }) => {
                VisualState::Stale {
                    content: content.clone(),
                }
            }
            _ => VisualState::Unavailable,
        };
        match source.read(bus, &self.app, deadline) {
            Ok(state) => VisualState::Current {
                content: self.content(&state),
            },
            Err(Failure::Unavailable) => VisualState::Unavailable,
            Err(_) => stale(),
        }
    }

    fn content(&mut self, state: &player::State) -> VisualContent {
        if let Some(jpeg_hex) = self.cover(state.art_url.as_deref()) {
            return VisualContent {
                jpeg_hex: Some(jpeg_hex),
                ..VisualContent::default()
            };
        }
        let (text, color) = match state.status {
            Status::Playing => ("Playing", PLAYING),
            Status::Paused => ("Paused", PAUSED),
            Status::Stopped => ("Stopped", STOPPED),
        };
        VisualContent {
            state: None,
            text: Some(text.into()),
            color: Some(color),
            ..VisualContent::default()
        }
    }

    fn cover(&mut self, art_url: Option<&str>) -> Option<String> {
        let path = cover::path_of(art_url?).ok()?;
        let metadata = fs::metadata(&path).ok()?;
        let key = CoverKey {
            path,
            length: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if let Some((known, made)) = &self.cover
            && *known == key
        {
            return made.clone();
        }
        let made = self.converter.jpeg_hex(&key.path).ok();
        self.cover = Some((key, made.clone()));
        made
    }
}

#[cfg(test)]
mod tests {
    mod fault_probe {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/performance/fault_probe.rs"
        ));
    }
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn a_reader_without_cover_does_not_wait_for_busy_conversion() {
        let mut profile =
            fault_probe::Probe::begin("stalled_converter", "wall_monotonic_no_bus_or_decode");
        if let Some(p) = &mut profile {
            p.snapshot("before");
        }
        let (converter, stalled) = cover::Converter::stalled();
        if let Some(p) = &mut profile {
            p.snapshot("during");
        }
        let profile_answer = Instant::now();
        let (done, result) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let mut reader = Reader::new("text-player".into(), Arc::new(converter));
            done.send(reader.content(&player::State {
                status: Status::Playing,
                art_url: None,
            }))
            .unwrap();
        });
        let result = result.recv_timeout(Duration::from_secs(2));
        if let Some(p) = &mut profile {
            p.sample(
                "text_reader_thread_answer",
                profile_answer.elapsed().as_nanos(),
            );
        }
        drop(stalled);
        handle.join().unwrap();
        if let Some(p) = &mut profile {
            p.snapshot("after");
        }
        assert_eq!(
            result
                .expect("text reader waited for cover conversion")
                .text
                .as_deref(),
            Some("Playing")
        );
    }
}
