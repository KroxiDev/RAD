//! Editor presentation and bounded client worker; no authoritative domain state.
pub use rad_api::Request;
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
            .map(|id| Request::Run { id: id.into() })
    }
}

#[cfg(feature = "gui")]
pub mod app;
pub mod app_icon;
pub mod catalog;
pub mod commands;
pub mod desktop;
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
        self.execution.as_ref().map(|id| Request::Result {
            execution_id: id.into(),
        })
    }
    pub fn terminal(&mut self) {
        self.execution = None;
    }
}

pub mod status;

pub mod plugins;

pub mod preferences;

pub mod dependencies;

pub mod diagnostics;
pub mod machine;
pub mod notices;
pub mod official;
pub mod recovery;
pub mod schedule;
pub mod screens;
pub mod service;
pub mod texts;

pub mod activation;
