//! Key clipboard state belongs to this editor; the runtime remains the source of the copy.
use super::{App, Job, Request, tr};
use eframe::egui;

pub(super) struct PendingPaste {
    folder: String,
    position: usize,
    pub(super) applied: bool,
}

impl App {
    fn selected_copyable(&self) -> Option<String> {
        let id = self.selected()?;
        let inspection = self.inspection.as_ref()?;
        let board = self.board.as_ref()?;
        if inspection.button["id"] != id
            || !crate::session::coherent(board, inspection)
            || inspection
                .actions
                .iter()
                .any(|action| action["fields"]["type"] == "return")
        {
            return None;
        }
        Some(id)
    }

    fn paste_key(&mut self, text: &str) {
        let Some(id) = self
            .copied_key
            .clone()
            .filter(|id| text == format!("rad-key:{id}"))
        else {
            return;
        };
        let Some((folder_id, position)) = self.folder().zip(self.selected_slot) else {
            return;
        };
        if self.slot_id(position).is_some()
            && (self.slot_id(position).as_ref() == Some(&id) || self.selected_copyable().is_none())
        {
            return;
        }
        if self.pending_paste.is_some() {
            self.warn(tr(self.es, "Busy — not sent"));
            return;
        }
        if self.command(Job::Request(Request::Duplicate {
            id,
            folder_id: folder_id.clone(),
            position,
            replace: true,
        })) {
            self.pending_paste = Some(PendingPaste {
                folder: folder_id,
                position,
                applied: false,
            });
        }
    }

    /// A poll already in flight before the paste must not consume its destination.
    pub(super) fn follow_paste(&mut self) {
        if self
            .pending_paste
            .as_ref()
            .is_some_and(|pending| pending.applied)
            && let Some(pending) = self.pending_paste.take()
            && self.folder().as_deref() == Some(pending.folder.as_str())
            && let Some(id) = self.slot_id(pending.position)
        {
            self.select(pending.position, Some(id));
        }
    }

    pub(super) fn clipboard_shortcuts(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                egui::Event::Copy => {
                    if let Some(id) = self.selected_copyable() {
                        ctx.copy_text(format!("rad-key:{id}"));
                        self.copied_key = Some(id);
                        self.say(tr(self.es, "Key copied"));
                    }
                }
                egui::Event::Paste(text) => self.paste_key(&text),
                _ => {}
            }
        }
    }
}
