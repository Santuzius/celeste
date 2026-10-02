//! Conflict list per sync dir and the conflict dialog.

use iced::Task;

use crate::{
    domain::{
        remote::RemoteId,
        run_state::RunState,
        sync::{Conflict, ConflictChoice, Resolution, SyncDirId},
    },
    screens::conflict,
    util::fmt_home,
};

use super::super::{CelesteApp, Message};

impl CelesteApp {
    /// A pass reported the complete conflict list of a sync dir. New entries are logged once (not on every pass); an open dialog follows its file or closes when the file is no longer in conflict.
    pub(in crate::app) fn handle_conflicts_reported(&mut self, remote_id: RemoteId, sync_dir_id: SyncDirId, conflicts: Vec<Conflict>) {
        let known = self.conflicts.remove(&sync_dir_id).unwrap_or_default();
        for c in &conflicts {
            if !known.iter().any(|k| k.remote_path == c.remote_path) {
                let what = if c.first_sync { "Different on both sides" } else { "Changed on both sides" };
                self.push_log_line(sync_dir_id, format!("⚠ {what}: {}", fmt_home(&c.local_path)));
            }
        }
        if !conflicts.is_empty() {
            self.sync_state.transition_dir(remote_id, sync_dir_id, RunState::Warning);
        }
        if let Some(dialog) = self.conflict_dialog.as_mut().filter(|d| d.sync_dir_id == sync_dir_id) {
            match conflicts.iter().find(|c| c.remote_path == dialog.conflict.remote_path) {
                Some(current) => dialog.conflict = current.clone(),
                None => self.conflict_dialog = None,
            }
        }
        if !conflicts.is_empty() {
            self.conflicts.insert(sync_dir_id, conflicts);
        }
    }

    pub(in crate::app) fn handle_open_conflict(&mut self, sync_dir_id: SyncDirId, remote_path: String) -> Task<Message> {
        let Some(conflict) = self.conflicts.get(&sync_dir_id).and_then(|list| list.iter().find(|c| c.remote_path == remote_path)) else {
            return Task::none();
        };
        let Some(remote) = self.remotes.iter().find(|r| self.sync_dirs.get(&r.id).is_some_and(|dirs| dirs.iter().any(|d| d.id == sync_dir_id))) else {
            return Task::none();
        };
        self.conflict_dialog = Some(conflict::Dialog::new(remote.id, remote.name.clone(), sync_dir_id, conflict.clone()));
        Task::none()
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
        let pending = self.resolutions.entry(dialog.remote_id).or_default().entry(dialog.sync_dir_id).or_default();
        pending.retain(|r| r.remote_path != resolution.remote_path);
        pending.push(resolution);
        // Hide it right away; the pass applies the choice, or reports the file again if it changed in the meantime.
        if let Some(list) = self.conflicts.get_mut(&dialog.sync_dir_id) {
            list.retain(|c| c.remote_path != dialog.conflict.remote_path);
        }
        self.handle_refresh_now(dialog.remote_id)
    }
}
