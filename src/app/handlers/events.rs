//! `WorkerReady` and `SyncEventReceived` handlers.

use std::sync::atomic::Ordering;

use iced::Task;
use tokio::sync::mpsc;

use crate::domain::{
    events::SyncEvent,
    remote::RemoteId,
    run_state::RunState,
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

    /// Route a single sync event through the state machine, the log
    /// buffer, and (for auth failures) auto-pause persistence.
    pub(in crate::app) fn handle_sync_event(&mut self, event: SyncEvent) -> Task<Message> {
        let mut auto_pause: Option<(RemoteId, crate::domain::remote::SyncPolicy)> = None;
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
                // session is per remote) and auto-pause the policy so
                // the scheduler stops hammering an endpoint that can
                // only return 401 until the user signs in again.
                let auth_failure = match &error {
                    SyncError::General(_, msg) => is_auth_failure(msg),
                    SyncError::BothMoreCurrent(..) => false,
                };
                self.push_log_line(sync_dir_id, line);
                if auth_failure {
                    self.sync_state.auth_failure(remote_id);
                    if let Some(remote) =
                        self.remotes.iter_mut().find(|r| r.id == remote_id)
                        && remote.policy.enabled
                    {
                        remote.policy.enabled = false;
                        self.sync_state.set_remote_enabled(remote_id, false);
                        if let Some(flag) = self.cancel_flags.get(&remote_id) {
                            flag.store(true, Ordering::Release);
                        }
                        self.refresh_requested_after.remove(&remote_id);
                        auto_pause = Some((remote_id, remote.policy.clone()));
                    }
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
                self.sync_state.transition_dir(remote_id, sync_dir_id, state);
            }
            SyncEvent::RemoteStarted { .. }
            | SyncEvent::RemoteCompleted { .. }
            | SyncEvent::RemoteFailed { .. }
            | SyncEvent::FileProgress { .. } => {}
        }
        if let Some((id, policy)) = auto_pause {
            let repo = self.repo.clone();
            Task::perform(
                async move {
                    let _ = repo.set_policy(id, policy).await;
                },
                |_| Message::PolicySaved,
            )
        } else {
            Task::none()
        }
    }
}
