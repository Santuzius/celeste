use super::{
    remote::RemoteId,
    run_state::RunState,
    sync::{Conflict, Resolution, SyncDirId, SyncError},
};

/// Events the sync services emit. UI adapters subscribe and translate into
/// their native render calls.
#[derive(Clone, Debug)]
pub enum SyncEvent {
    /// A remote's full sync pass has begun.
    RemoteStarted {
        remote_id: RemoteId,
    },
    /// A remote's full sync pass finished successfully.
    RemoteCompleted {
        remote_id: RemoteId,
        at_unix: i64,
    },
    /// A remote's sync pass failed at the remote level (not a single file).
    RemoteFailed {
        remote_id: RemoteId,
        message: String,
    },
    /// Primary per-sync-dir status text (e.g. "Synchronizing '/foo'…" or
    /// "Files are synced."). Renders on the sync_dir's main row.
    SyncDirStatus {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        text: String,
    },
    /// Secondary per-sync-dir "pending event" text (e.g. "Checking for
    /// changes…" or "Refresh queued…"). Renders on a second line under
    /// the main row, and is cleared once SyncDirStatus advances.
    SyncDirPending {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        text: String,
    },
    /// Per-sync-dir error that should be surfaced in the error list.
    SyncDirError {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        error: SyncError,
    },
    /// The files of a sync dir that changed on both sides and wait for the user, as of the pass that just ran. Replaces the previous list; empty when there are none.
    SyncDirConflicts {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        conflicts: Vec<Conflict>,
        /// The user's choices this pass was given (applied, or dropped because the file changed again). Choices made while the pass ran aren't in here, so the app keeps hiding those files.
        resolutions: Vec<Resolution>,
    },
    /// Per-sync-dir coarse run-state transition. Drives the status icon on
    /// the remote page. The `RunState` variants the sync engine emits are
    /// `Syncing`, `Synced`, `Warning`, and `Error`; the state machine in
    /// `AppState` fills in `Waiting`, `Paused`, and `AuthNeeded` internally.
    SyncDirStateChanged {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        state: RunState,
    },
    /// Per-file progress (reserved for future streaming).
    #[allow(dead_code)]
    FileProgress {
        remote_id: RemoteId,
        sync_dir_id: SyncDirId,
        file: String,
        pct: f32,
    },
}

#[derive(Clone, Debug)]
pub enum FsEvent {
    Changed { path: String },
}
