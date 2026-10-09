//! Conflict list per sync dir and the conflict dialog.

use iced::Task;

use crate::{
    domain::sync::{ConflictChoice, Resolution, SyncDirId},
    engine::Command,
    screens::conflict,
};

use super::super::{CelesteApp, Message};

impl CelesteApp {
    /// The conflict lists changed: an open dialog follows its file or moves on to the one now at its position.
    pub(in crate::app) fn follow_conflict_dialog(&mut self) {
        let Some(dialog) = self.conflict_dialog.as_mut() else {
            return;
        };
        let sync_dir_id = dialog.sync_dir_id;
        let list = self.conflicts.get(&sync_dir_id).map_or(&[][..], |v| v.as_slice());
        match list.iter().position(|c| c.remote_path == dialog.conflict.remote_path) {
            Some(i) => {
                dialog.conflict = list[i].clone();
                dialog.position = i;
                dialog.total = list.len();
            }
            None => {
                let position = dialog.position;
                self.show_conflict(sync_dir_id, position);
            }
        }
    }

    pub(in crate::app) fn handle_open_conflicts(&mut self, sync_dir_id: SyncDirId) -> Task<Message> {
        self.show_conflict(sync_dir_id, 0);
        Task::none()
    }

    /// Open the dialog on the folder's conflict at `position` (or the last one), close it when none is left.
    fn show_conflict(&mut self, sync_dir_id: SyncDirId, position: usize) {
        self.conflict_dialog = None;
        let Some(list) = self.conflicts.get(&sync_dir_id).filter(|l| !l.is_empty()) else {
            return;
        };
        let Some(remote) = self.remotes.iter().find(|r| self.sync_dirs.get(&r.id).is_some_and(|dirs| dirs.iter().any(|d| d.id == sync_dir_id))) else {
            return;
        };
        let position = position.min(list.len() - 1);
        self.conflict_dialog = Some(conflict::Dialog::new(remote.id, remote.name.clone(), sync_dir_id, list[position].clone(), position, list.len()));
    }

    pub(in crate::app) fn handle_conflict_msg(&mut self, msg: conflict::Msg) -> Task<Message> {
        let Some(dialog) = self.conflict_dialog.as_mut() else {
            return Task::none();
        };
        let choice = match msg {
            conflict::Msg::NewNameChanged(name) => {
                dialog.new_name = name;
                return Task::none();
            }
            conflict::Msg::SuggestName => {
                dialog.suggest_name();
                return Task::none();
            }
            // The conflict stays and is checked again on every pass; maybe the user sorts it out by hand.
            conflict::Msg::Cancel => {
                self.conflict_dialog = None;
                return Task::none();
            }
            conflict::Msg::Previous | conflict::Msg::Next => {
                let (sync_dir_id, position) = (dialog.sync_dir_id, dialog.position);
                let position = if matches!(msg, conflict::Msg::Previous) { position.saturating_sub(1) } else { position + 1 };
                self.show_conflict(sync_dir_id, position);
                return Task::none();
            }
            conflict::Msg::KeepLocal => ConflictChoice::KeepLocal,
            conflict::Msg::KeepRemote => ConflictChoice::KeepRemote,
            conflict::Msg::KeepBoth if dialog.new_name_valid() => ConflictChoice::KeepBoth { local_name: dialog.new_name.trim().to_owned() },
            conflict::Msg::KeepBoth => return Task::none(),
        };
        let Some(dialog) = self.conflict_dialog.take() else {
            return Task::none();
        };
        let resolution = Resolution {
            remote_path: dialog.conflict.remote_path.clone(),
            choice,
            local_stamp: dialog.conflict.local_stamp,
            remote_stamp: dialog.conflict.remote_stamp,
        };
        // The engine hands the choice to a pass it starts right away; that pass applies it, or reports the file again if it changed in the meantime.
        self.engine.send(Command::Resolve { remote_id: dialog.remote_id, sync_dir_id: dialog.sync_dir_id, resolution });
        // Hide it right away rather than waiting for the engine's next state.
        if let Some(list) = self.conflicts.get_mut(&dialog.sync_dir_id) {
            list.retain(|c| c.remote_path != dialog.conflict.remote_path);
        }
        // The next conflict moves up to the same position (2/9 → 2/8).
        self.show_conflict(dialog.sync_dir_id, dialog.position);
        Task::none()
    }
}
