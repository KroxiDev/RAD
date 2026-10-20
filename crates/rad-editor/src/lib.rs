//! Editor presentation and bounded client worker; no authoritative domain state.
use serde_json::{Value, json};
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub operation: String,
    pub arguments: Value,
}
impl Request {
    pub fn new(operation: &str, arguments: Value) -> Self {
        Self {
            operation: operation.into(),
            arguments,
        }
    }
}
#[derive(Default)]
pub struct Controller {
    selected: Option<String>,
    execution: Option<String>,
}
impl Controller {
    pub fn select(&mut self, id: Option<String>) {
        self.selected = id;
    }
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn try_selected(&self) -> Option<Request> {
        self.selected
            .as_ref()
            .map(|id| Request::new("run", json!({"id":id})))
    }
}

#[derive(Default)]
pub struct PollSchedule {
    last: Option<u64>,
}
impl PollSchedule {
    pub fn due(&mut self, now_ms: u64, visible: bool, busy: bool) -> bool {
        if busy
            || self
                .last
                .is_some_and(|last| now_ms.saturating_sub(last) < if visible { 250 } else { 1000 })
        {
            return false;
        }
        self.last = Some(now_ms);
        true
    }
}

#[cfg(feature = "gui")]
pub mod app;
pub mod catalog;
pub mod commands;
pub mod icons;
pub mod native;
pub mod options;
pub mod preview;
pub mod session;
pub mod theme;
pub mod transfer;
pub mod worker;

impl Controller {
    pub fn admitted(&mut self, id: String) {
        self.execution = Some(id);
    }
    pub fn result_request(&self) -> Option<Request> {
        self.execution
            .as_ref()
            .map(|id| Request::new("result", json!({"execution_id":id})))
    }
    pub fn terminal(&mut self) {
        self.execution = None;
    }
}

pub mod status;

pub mod plugins;
