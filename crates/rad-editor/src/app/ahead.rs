//! What the editor reads ahead while idle, all at the board's revision: the inspector of
//! each key on the board, so selecting one shows it at once, and the folders one press away,
//! so their pictures are read before they are needed.
use super::string;
use crate::{
    native::PictureRead,
    session::{Board, Inspection, coherent},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Ahead {
    /// Revision of the board everything here was read at.
    revision: String,
    inspections: BTreeMap<String, Inspection>,
    folders: BTreeMap<String, Board>,
    asked_keys: BTreeSet<String>,
    asked_folders: BTreeSet<String>,
}
impl Ahead {
    /// A board arrived; what was read at another revision is let go.
    pub(super) fn follow(&mut self, board: &Board) {
        let revision = string(&board.folder["view_revision"]);
        if revision != self.revision {
            *self = Self {
                revision,
                ..Self::default()
            };
        }
    }
    /// Plugins changed what an inspector says without a new revision: read them again.
    pub(super) fn forget_inspections(&mut self) {
        self.inspections.clear();
        self.asked_keys.clear();
    }
    pub(super) fn inspection(&self, board: &Board, id: &str) -> Option<&Inspection> {
        self.inspections
            .get(id)
            .filter(|inspection| coherent(board, inspection))
    }
    pub(super) fn keep_inspection(&mut self, board: &Board, inspection: &Inspection) {
        if coherent(board, inspection) {
            self.inspections
                .insert(string(&inspection.button["id"]), inspection.clone());
        }
    }
    pub(super) fn keep_folder(&mut self, folder: Board) {
        if folder.folder["view_revision"] == self.revision.as_str() {
            self.folders.insert(string(&folder.folder["id"]), folder);
        }
    }
    /// The next key on the board whose inspector is not read yet, besides the selected one.
    pub(super) fn due_inspection(&self, board: &Board, selected: Option<&str>) -> Option<String> {
        board
            .buttons
            .iter()
            .map(|key| string(&key.entity["id"]))
            .find(|id| {
                Some(id.as_str()) != selected
                    && !self.inspections.contains_key(id)
                    && !self.asked_keys.contains(id)
            })
    }
    /// This key's inspector is being read; if refused, it waits for another revision.
    pub(super) fn asked_inspection(&mut self, id: &str) {
        self.asked_keys.insert(id.to_owned());
    }
    /// The next folder one press away that is not read yet.
    pub(super) fn due_folder(&self, board: &Board) -> Option<String> {
        self.nearby_folders(board)
            .into_iter()
            .find(|id| !self.folders.contains_key(id) && !self.asked_folders.contains(id))
    }
    pub(super) fn asked_folder(&mut self, id: &str) {
        self.asked_folders.insert(id.to_owned());
    }
    /// The parent folder, then those the keys enter, in the keys' order.
    fn nearby_folders(&self, board: &Board) -> Vec<String> {
        let entered = board.buttons.iter().filter_map(|key| {
            let inspection = self.inspections.get(key.entity["id"].as_str()?)?;
            crate::commands::folder_destination(&inspection.actions)
        });
        let mut folders: Vec<String> = Vec::new();
        for folder in board.folder["fields"]["parent_id"]
            .as_str()
            .into_iter()
            .chain(entered)
        {
            if folder != board.folder["id"] && !folders.iter().any(|f| f == folder) {
                folders.push(folder.to_owned());
            }
        }
        folders
    }
    /// Pictures of the folders one press away that have been read.
    pub(super) fn nearby_pictures(&self, board: &Board) -> Vec<PictureRead> {
        self.nearby_folders(board)
            .iter()
            .filter_map(|id| self.folders.get(id))
            .flat_map(board_pictures)
            .collect()
    }
}
/// The pictures of a board's keys, in reading order.
pub(super) fn board_pictures(board: &Board) -> Vec<PictureRead> {
    board
        .buttons
        .iter()
        .filter_map(|key| {
            Some(PictureRead {
                button: string(&key.entity["id"]),
                reference: key.appearance["image"].as_str()?.to_owned(),
                revision: string(&key.entity["view_revision"]),
            })
        })
        .collect()
}
