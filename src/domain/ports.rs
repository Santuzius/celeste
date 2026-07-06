//! Port traits: the contract services use to reach the outside world.
//!
//! Implementations live in `crate::infrastructure`. Services depend only on
//! these traits, so adapter swaps never touch domain or service code.
//!
//! Boxed-future return types are used instead of `async fn in trait` to stay
//! object-safe (`dyn Repository`) without pulling in `async-trait` yet. This
//! is revisited when the orchestrator actually needs `Arc<dyn Port>`s.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use super::{
    remote::{Remote, RemoteId, SyncPolicy},
    sync::{
        ListFilter, RemoteItem, SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId,
        SyncItem, SyncItemId,
    },
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Cancellation token for backend operations. A shared `AtomicBool` that
/// callers flip to `true` to request cancellation; adapters check it at
/// action boundaries and return early when set.
///
/// The type alias is intentionally thin so a future workstream can swap
/// the implementation to `tokio_util::sync::CancellationToken` in one
/// commit without touching every call site.
pub type Cancel = Arc<AtomicBool>;

/// A `Cancel` token that is never set — used when a call site has no
/// meaningful cancellation requirement (e.g. auth / config operations).
#[inline]
pub fn cancel_never() -> Cancel {
    Arc::new(AtomicBool::new(false))
}

/// Check whether `cancel` has been tripped.
#[inline]
pub fn is_cancelled(cancel: &Cancel) -> bool {
    cancel.load(Ordering::Acquire)
}

#[derive(Debug)]
pub enum RepositoryError {
    NotFound,
    Other(String),
}

impl std::fmt::Display for RepositoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for RepositoryError {}

pub trait Repository: Send + Sync {
    fn list_remotes(&self) -> BoxFuture<'_, Result<Vec<Remote>, RepositoryError>>;
    fn find_remote(&self, id: RemoteId)
        -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>>;
    fn find_remote_by_name(
        &self,
        name: &str,
    ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>>;
    fn insert_remote(
        &self,
        name: String,
    ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>>;
    /// Insert a remote whose adapter is the native ProtonDrive client.
    /// `session_path` is the absolute path to the persisted credential
    /// blob the app-start flow will later feed to
    /// `ProtonDrive_ResumeSession`.
    fn insert_native_proton_remote(
        &self,
        name: String,
        session_path: String,
    ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>>;
    fn delete_remote(&self, id: RemoteId) -> BoxFuture<'_, Result<(), RepositoryError>>;
    /// Delete a remote, all of its sync_dirs, and all of their sync_items.
    fn cascade_delete_remote(
        &self,
        id: RemoteId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    /// Delete a sync_dir (by `(local_path, remote_path)`) and all of its
    /// sync_items.
    fn cascade_delete_sync_dir(
        &self,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn set_policy(
        &self,
        id: RemoteId,
        policy: SyncPolicy,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;

    fn list_sync_dirs(
        &self,
        remote: RemoteId,
    ) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>>;
    fn list_all_sync_dirs(&self) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>>;
    fn sync_dir_exists(
        &self,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<bool, RepositoryError>>;
    fn insert_sync_dir(
        &self,
        remote: RemoteId,
        local_path: String,
        remote_path: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn list_sync_items(
        &self,
        sync_dir: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncItem>, RepositoryError>>;
    fn find_sync_item_by_paths(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>>;
    fn find_sync_item_by_local(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>>;
    fn find_sync_item_by_remote(
        &self,
        sync_dir: SyncDirId,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>>;
    fn delete_sync_item(&self, id: SyncItemId)
        -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn insert_sync_item(
        &self,
        sync_dir: SyncDirId,
        local_path: String,
        remote_path: String,
        last_local_timestamp: i64,
        last_remote_timestamp: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn update_sync_item_timestamps(
        &self,
        id: SyncItemId,
        last_local_timestamp: i64,
        last_remote_timestamp: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn delete_sync_item_by_paths(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    /// Delete all sync_items in `sync_dir` whose `local_path` equals
    /// `local_prefix` or starts with `local_prefix/`.
    fn delete_sync_items_with_local_prefix(
        &self,
        sync_dir: SyncDirId,
        local_prefix: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;

    fn list_exclusions(
        &self,
        sync_dir: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncDirExclusion>, RepositoryError>>;
    fn insert_exclusion(
        &self,
        sync_dir: SyncDirId,
        remote_path: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
    fn delete_exclusion(
        &self,
        id: SyncDirExclusionId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>>;
}

pub trait BackendClient: Send + Sync {
    fn stat(&self, remote: &str, path: &str, cancel: &Cancel)
        -> Result<Option<RemoteItem>, String>;
    fn list(
        &self,
        remote: &str,
        path: &str,
        recursive: bool,
        filter: ListFilter,
        cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String>;
    fn mkdir(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String>;
    fn delete_file(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String>;
    fn purge(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String>;
    fn copy_to_remote(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String>;
    fn copy_to_local(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String>;
    fn delete_config(&self, remote: &str) -> Result<(), String>;
    /// Create a new rclone config from a JSON body (rclone's
    /// `config/create` RPC payload, including `name`, `type`, `parameters`
    /// and optional `opt`).
    fn create_config(&self, payload_json: String) -> Result<(), String>;
    /// rclone backend type for a configured remote — `"drive"`,
    /// `"dropbox"`, `"protondrive"`, `"webdav"`, etc. Returns `Ok(None)`
    /// when the remote name isn't in rclone's config.
    fn remote_type(&self, remote: &str) -> Result<Option<String>, String>;

    /// Persist any session state that mutated during a sync pass (e.g.
    /// a rotated auth token) so it survives a restart. Called by the
    /// sync engine at the end of each pass. Default is a no-op —
    /// rclone-backed remotes keep their credentials in `rclone.conf`
    /// and have nothing per-pass to checkpoint; the native Proton
    /// client overrides this to re-persist rotated refresh tokens.
    fn checkpoint_session(&self, _remote: &str) {}
}

