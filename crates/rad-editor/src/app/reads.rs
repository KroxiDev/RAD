//! Choose the next read for the main worker, preserving its rotating order.
use super::{App, ahead, launcher};
use crate::{Request, native::Job};

#[derive(Clone, Copy)]
enum Read {
    Refresh,
    ExecutionResult,
    Transfer,
    Inspection,
    ActionCatalog,
    Plugins,
    Pictures,
    MachineSettings,
    Recovery,
    ApplicationPicker,
    InspectAhead,
    FolderAhead,
}

impl App {
    pub(super) fn pump_reads(&mut self) {
        if !self.worker.busy() && self.connected {
            let now = self.now_ms();
            let pictures = match &self.board {
                Some(board) => self.pictures.next(
                    &ahead::board_pictures(board),
                    &self.ahead.nearby_pictures(board),
                ),
                None => Vec::new(),
            };
            let selected = self.selected();
            let inspect_ahead = self
                .board
                .as_ref()
                .and_then(|board| self.ahead.due_inspection(board, selected.as_deref()));
            let folder_ahead = self
                .board
                .as_ref()
                .and_then(|board| self.ahead.due_folder(board));
            let normal = self.recovery.is_normal();
            let reads = [
                Read::Refresh,
                Read::ExecutionResult,
                Read::Transfer,
                Read::Inspection,
                Read::ActionCatalog,
                Read::Plugins,
                Read::Pictures,
                Read::MachineSettings,
                Read::Recovery,
                Read::ApplicationPicker,
                Read::InspectAhead,
                Read::FolderAhead,
            ];
            let ready = reads.map(|read| {
                let ready = match read {
                    Read::Refresh => normal && (self.force || self.overview_due),
                    Read::ExecutionResult => {
                        normal && self.result_due && self.control.result_request().is_some()
                    }
                    Read::Transfer => self.transfer_active,
                    Read::Inspection => normal && self.inspect_needed,
                    Read::ActionCatalog => normal && self.catalog_due.is_some_and(|due| now >= due),
                    Read::Plugins => normal && self.plugins.has_work(),
                    Read::Pictures => normal && !pictures.is_empty(),
                    Read::MachineSettings => normal && self.machine_due,
                    Read::Recovery => self.recovery_due,
                    Read::ApplicationPicker => normal && self.app_picker.is_due(now),
                    Read::InspectAhead => normal && inspect_ahead.is_some(),
                    Read::FolderAhead => normal && folder_ahead.is_some(),
                };
                (read, ready)
            });
            match self.schedule.choose(ready) {
                Some(Read::Refresh) => {
                    let force = std::mem::take(&mut self.force);
                    let overview = std::mem::take(&mut self.overview_due)
                        || self.status.is_null()
                        || self.resolving_unknown;
                    // A board of a newer revision brings the selected key's inspector along.
                    let selected = force.then(|| self.selected()).flatten().map(|id| {
                        crate::session::Selected {
                            id,
                            revision: self
                                .inspection
                                .as_ref()
                                .and_then(|i| i.button["view_revision"].as_str())
                                .map(str::to_owned),
                        }
                    });
                    let _ = self.worker.submit(Job::Refresh {
                        board: force,
                        overview,
                        selected,
                    });
                }
                Some(Read::ExecutionResult) => {
                    self.result_due = false;
                    let _ = self
                        .worker
                        .submit(Job::Request(self.control.result_request().unwrap()));
                }
                Some(Read::Transfer) => {
                    let _ = self.worker.submit(Job::TransferStep);
                }
                Some(Read::Inspection) => {
                    self.inspect_needed = false;
                    if let Some(id) = self.selected() {
                        let _ = self.worker.submit(Job::Inspect(id));
                    }
                }
                Some(Read::ActionCatalog) => {
                    self.catalog_due = None;
                    self.request_application_catalog(launcher::CatalogTarget::Actions);
                }
                Some(Read::Plugins) => {
                    self.poll_plugins();
                }
                Some(Read::Pictures) => {
                    self.pictures.asked(&pictures);
                    let _ = self.worker.submit(Job::Pictures(pictures));
                }
                Some(Read::MachineSettings) => {
                    self.machine_due = false;
                    let _ = self
                        .worker
                        .submit(Job::Request(Request::MachineSettings {}));
                }
                Some(Read::Recovery) => {
                    self.recovery_due = false;
                    let _ = self.worker.submit(Job::Request(Request::Recovery {}));
                }
                Some(Read::ApplicationPicker) => {
                    self.request_application_catalog(launcher::CatalogTarget::Picker);
                }
                Some(Read::InspectAhead) => {
                    let id = inspect_ahead.expect("ready");
                    self.ahead.asked_inspection(&id);
                    let _ = self.worker.submit(Job::InspectAhead(id));
                }
                Some(Read::FolderAhead) => {
                    let id = folder_ahead.expect("ready");
                    self.ahead.asked_folder(&id);
                    let _ = self.worker.submit(Job::FolderBoard(id));
                }
                None => {}
            }
        }
    }
}
