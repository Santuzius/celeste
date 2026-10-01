//! Adapter implementation of [`crate::domain::ports::BackendClient`] for
//! the rclone backend (via the librclone RPC surface).
//!
//! Thin wrapper around [`super::rpc::sync`] — blocking calls underneath,
//! since that's what librclone exposes. Async-ifying is deferred until the
//! orchestrator moves onto a proper tokio runtime.
//!
//! On every call that mutates the rclone config file (`create_config`,
//! `delete_config`) we re-read the file from disk and stash its full
//! text into the OS keyring under `Celeste Keys / rclone-config`. That
//! way the on-disk file becomes a transient, regenerable artefact and
//! the source of truth lives behind libsecret.

use std::path::PathBuf;

use crate::domain::{
    ports::{BackendClient, Cancel},
    sync::{ListFilter, RemoteItem},
};
use crate::services::secrets;

use super::rpc::{self, BackendListFilter, BackendRemoteItem};

#[derive(Clone)]
pub struct LibrcloneClient {
    rclone_config_path: PathBuf,
}

impl LibrcloneClient {
    /// `rclone_config_path` is the path librclone writes its config
    /// to. After every mutation we read the file back from this path
    /// and sync the contents to the keyring.
    pub fn new(rclone_config_path: PathBuf) -> Self {
        Self { rclone_config_path }
    }

    /// Push the current on-disk rclone config into the keyring. Logged
    /// on failure but not propagated — the user-visible operation has
    /// already succeeded; failing the whole call because the keyring
    /// is unhappy would just leave them with a broken UI.
    fn sync_to_keyring(&self) {
        let body = match std::fs::read_to_string(&self.rclone_config_path) {
            Ok(body) => body,
            Err(err) => {
                eprintln!(
                    "celeste: rclone config keyring sync skipped — couldn't read {}: {err}",
                    self.rclone_config_path.display(),
                );
                return;
            }
        };
        if let Err(err) = secrets::store(secrets::RCLONE_ACCOUNT, &body) {
            eprintln!("celeste: rclone config keyring sync failed: {err}");
        }
    }
}

fn map_item(item: BackendRemoteItem) -> RemoteItem {
    RemoteItem {
        is_dir: item.is_dir,
        path: item.path,
        name: item.name,
        mod_time: item.mod_time,
    }
}

fn map_filter(filter: ListFilter) -> BackendListFilter {
    match filter {
        ListFilter::All => BackendListFilter::All,
        ListFilter::Dirs => BackendListFilter::Dirs,
        ListFilter::Files => BackendListFilter::Files,
    }
}

impl BackendClient for LibrcloneClient {
    fn stat(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<Option<RemoteItem>, String> {
        rpc::sync::stat(remote, path)
            .map(|opt| opt.map(map_item))
            .map_err(|err| err.error)
    }

    fn list(
        &self,
        remote: &str,
        path: &str,
        recursive: bool,
        filter: ListFilter,
        _cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        rpc::sync::list(remote, path, recursive, map_filter(filter))
            .map(|items| items.into_iter().map(map_item).collect())
            .map_err(|err| err.error)
    }

    fn mkdir(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        rpc::sync::mkdir(remote, path).map_err(|err| err.error)
    }

    fn delete_file(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        rpc::sync::delete(remote, path).map_err(|err| err.error)
    }

    fn purge(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        rpc::sync::purge(remote, path).map_err(|err| err.error)
    }

    fn copy_to_remote(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        rpc::sync::copy_to_remote(local_path, remote, remote_path).map_err(|err| err.error)
    }

    fn copy_to_local(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        rpc::sync::copy_to_local(local_path, remote, remote_path).map_err(|err| err.error)
    }

    fn delete_config(&self, remote: &str) -> Result<(), String> {
        let result = rpc::sync::delete_config(remote).map_err(|err| err.error);
        if result.is_ok() {
            self.sync_to_keyring();
        }
        result
    }

    fn create_config(&self, payload_json: String) -> Result<(), String> {
        let result = celeste_go::rpc("config/create", payload_json).map(|_| ());
        if result.is_ok() {
            self.sync_to_keyring();
        }
        result
    }

    fn remote_type(&self, remote: &str) -> Result<Option<String>, String> {
        let payload = serde_json::json!({ "name": remote }).to_string();
        match celeste_go::rpc("config/get", payload) {
            Ok(body) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&body).map_err(|e| e.to_string())?;
                // `config/get` returns {} for unknown remotes.
                if parsed.as_object().map(|m| m.is_empty()).unwrap_or(true) {
                    return Ok(None);
                }
                Ok(parsed.get("type").and_then(|v| v.as_str()).map(str::to_owned))
            }
            Err(err) => Err(err),
        }
    }

    fn uses_shared_oauth_client(&self, remote: &str) -> bool {
        let payload = serde_json::json!({ "name": remote }).to_string();
        let Ok(body) = celeste_go::rpc("config/get", payload) else {
            return false;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
            return false;
        };
        parsed.get("type").and_then(|v| v.as_str()) == Some("drive")
            && parsed.get("client_id").and_then(|v| v.as_str()).is_none_or(str::is_empty)
    }
}
