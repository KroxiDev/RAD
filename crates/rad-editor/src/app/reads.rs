//! Choose the next read for the main worker, preserving its rotating order.
use super::{App, ahead, launcher};
use crate::{Request, native::Job};

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
            match self.schedule.choose([
                normal && (self.force || self.overview_due),
                normal && self.result_due && self.control.result_request().is_some(),
                self.transfer_active,
                normal && self.inspect_needed,
                normal && self.catalog_due.is_some_and(|due| now >= due),
                normal && self.plugins.has_work(),
                normal && !pictures.is_empty(),
                normal && self.machine_due,
                self.recovery_due,
                normal && self.app_picker.is_due(now),
                normal && inspect_ahead.is_some(),
                normal && folder_ahead.is_some(),
            ]) {
                Some(0) => {
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
                Some(1) => {
                    self.result_due = false;
                    let _ = self
                        .worker
                        .submit(Job::Request(self.control.result_request().unwrap()));
                }
                Some(2) => {
                    let _ = self.worker.submit(Job::TransferStep);
                }
                Some(3) => {
                    self.inspect_needed = false;
                    if let Some(id) = self.selected() {
                        let _ = self.worker.submit(Job::Inspect(id));
                    }
                }
                Some(4) => {
                    self.catalog_due = None;
                    self.request_application_catalog(launcher::CatalogTarget::Actions);
                }
                Some(5) => {
                    self.poll_plugins();
                }
                Some(6) => {
                    self.pictures.asked(&pictures);
                    let _ = self.worker.submit(Job::Pictures(pictures));
                }
                Some(7) => {
                    self.machine_due = false;
                    let _ = self
                        .worker
                        .submit(Job::Request(Request::MachineSettings {}));
                }
                Some(8) => {
                    self.recovery_due = false;
                    let _ = self.worker.submit(Job::Request(Request::Recovery {}));
                }
                Some(9) => {
                    self.request_application_catalog(launcher::CatalogTarget::Picker);
                }
                Some(10) => {
                    let id = inspect_ahead.expect("ready");
                    self.ahead.asked_inspection(&id);
                    let _ = self.worker.submit(Job::InspectAhead(id));
                }
                Some(11) => {
                    let id = folder_ahead.expect("ready");
                    self.ahead.asked_folder(&id);
                    let _ = self.worker.submit(Job::FolderBoard(id));
                }
                _ => {}
            }
        }
    }
}
