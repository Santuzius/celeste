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

/// Starts of the log lines for new conflicts (first sync / changed since the last one).
pub(in crate::app) const CONFLICT_LINE_PREFIXES: [&str; 2] = ["⚠ Different on both sides", "⚠ Changed on both sides"];

impl CelesteApp {
    /// A pass reported the complete conflict list of a sync dir. New entries are logged once (not on every pass); files the user already decided on stay hidden until a pass that was given that choice reports back; an open dialog follows its file or moves on to the one now at its position.
    pub(in crate::app) fn handle_conflicts_reported(&mut self, remote_id: RemoteId, sync_dir_id: SyncDirId, mut conflicts: Vec<Conflict>, handled: Vec<Resolution>) {
        let pending = self.resolutions.get_mut(&remote_id).and_then(|dirs| dirs.get_mut(&sync_dir_id));
        if let Some(pending) = pending {
            pending.retain(|r| !handled.contains(r));
            conflicts.retain(|c| !pending.iter().any(|r| r.remote_path == c.remote_path));
        }
        let known = self.conflicts.remove(&sync_dir_id).unwrap_or_default();
        for c in &conflicts {
            if !known.iter().any(|k| k.remote_path == c.remote_path) {
                let prefix = CONFLICT_LINE_PREFIXES[usize::from(!c.first_sync)];
                self.push_log_line(sync_dir_id, format!("{prefix}: {}", fmt_home(&c.local_path)));
            }
        }
        if !conflicts.is_empty() {
            self.sync_state.transition_dir(remote_id, sync_dir_id, RunState::Warning);
            self.conflicts.insert(sync_dir_id, conflicts);
        }
        let Some(dialog) = self.conflict_dialog.as_mut().filter(|d| d.sync_dir_id == sync_dir_id) else {
            return;
        };
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
        let pending = self.resolutions.entry(dialog.remote_id).or_default().entry(dialog.sync_dir_id).or_default();
        pending.retain(|r| r.remote_path != resolution.remote_path);
        pending.push(resolution);
        // Hide it right away; the pass applies the choice, or reports the file again if it changed in the meantime.
        if let Some(list) = self.conflicts.get_mut(&dialog.sync_dir_id) {
            list.retain(|c| c.remote_path != dialog.conflict.remote_path);
            // Nothing left to decide: drop the Warning the conflicts caused (it would outlast the next Synced) until the pass that applies the choices reports.
            if list.is_empty() {
                self.sync_state.transition_dir(dialog.remote_id, dialog.sync_dir_id, RunState::Waiting);
            }
        }
        // The next conflict moves up to the same position (2/9 → 2/8).
        self.show_conflict(dialog.sync_dir_id, dialog.position);
        self.handle_refresh_now(dialog.remote_id)
    }
}
