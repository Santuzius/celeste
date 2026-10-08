//! `WorkerReady` and `SyncEventReceived` handlers.

use std::sync::atomic::Ordering;

use iced::Task;
use tokio::sync::mpsc;

use crate::domain::{
    events::SyncEvent,
    run_state::{RunState, SyncActivity},
    sync::SyncError,
};

use super::super::{is_auth_failure, CelesteApp, Message};

impl CelesteApp {
    /// Store the sender the sync engine uses to push events back into
    /// the Iced runtime.
    pub(in crate::app) fn handle_worker_ready(
        &mut self,
        tx: mpsc::Sender<SyncEvent>,
    ) -> Task<Message> {
        self.events_tx = Some(tx);
        Task::none()
    }

    /// Route a single sync event through the state machine and the log
    /// buffer.
    pub(in crate::app) fn handle_sync_event(&mut self, event: SyncEvent) -> Task<Message> {
        match event {
            SyncEvent::SyncDirStatus {
                sync_dir_id, text, ..
            } => {
                self.push_log_line(sync_dir_id, text);
            }
            SyncEvent::SyncDirPending {
                sync_dir_id, text, ..
            } => {
                self.push_log_line(sync_dir_id, format!("⟳ {text}"));
            }
            SyncEvent::SyncDirError {
                remote_id,
                sync_dir_id,
                error,
            } => {
                let line = match &error {
                    SyncError::General(path, msg) => format!("⚠ {path}: {msg}"),
                    SyncError::BothMoreCurrent(local, remote) => {
                        format!("⚠ Conflict: '{local}' vs '{remote}'")
                    }
                };
                // Auth-failure heuristic: HTTP 401 and the matching
                // rclone phrasing both indicate the session is dead and
                // only reauth fixes it. Flag the whole remote (the
                // session is per remote) and stop the running pass. The
                // flag alone keeps the scheduler off the remote; the
                // user's Enabled setting stays untouched, so a
                // successful reauth resumes syncing without a detour
                // through the settings.
                let auth_failure = match &error {
                    SyncError::General(_, msg) => is_auth_failure(msg),
                    SyncError::BothMoreCurrent(..) => false,
                };
                self.push_log_line(sync_dir_id, line);
                if auth_failure {
                    self.sync_state.auth_failure(remote_id);
                    if let Some(flag) = self.cancel_flags.get(&remote_id) {
                        flag.store(true, Ordering::Release);
                    }
                    self.refresh_requested_after.remove(&remote_id);
                } else {
                    // Per-file errors surface as Warning so the icon
                    // mirrors the trouble even if the pass eventually
                    // ends with the engine-level Synced.
                    self.sync_state.transition_dir(
                        remote_id,
                        sync_dir_id,
                        RunState::Warning,
                    );
                }
            }
            SyncEvent::SyncDirStateChanged {
                remote_id,
                sync_dir_id,
                state,
            } => {
                // Every pass starts by listing; from here on only this pass's lines describe what's going on.
                if state == RunState::Syncing(SyncActivity::Listing) {
                    self.sync_dir_pass_line.remove(&sync_dir_id);
                }
                self.sync_state.transition_dir(remote_id, sync_dir_id, state);
            }
            SyncEvent::SyncDirConflicts {
                remote_id,
                sync_dir_id,
                conflicts,
                resolutions,
            } => self.handle_conflicts_reported(remote_id, sync_dir_id, conflicts, resolutions),
            SyncEvent::RemoteStarted { .. }
            | SyncEvent::RemoteCompleted { .. }
            | SyncEvent::RemoteFailed { .. }
            | SyncEvent::FileProgress { .. } => {}
        }
        Task::none()
    }
}
