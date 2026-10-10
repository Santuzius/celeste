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

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::domain::{
    ports::{BackendClient, Cancel},
    sync::{FileDetails, ListFilter, RemoteItem},
};
use crate::services::secrets;

use super::rpc::{self, BackendListFilter, BackendRemoteItem};

#[derive(Clone)]
pub struct LibrcloneClient {
    rclone_config_path: PathBuf,
    listings: Arc<Mutex<HashMap<String, Listings>>>,
}

/// Forces a fresh listing now and then, in case a change notification was missed.
const LISTING_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Listings of one remote, reused while the provider's change log (polled by the Go side, see `rclonewatch`) reports no change. Saves the full tree walk on every sync pass; backends without change notification never hit this cache.
#[derive(Default)]
struct Listings {
    filled_at: Option<Instant>,
    entries: HashMap<(String, bool, u8), Vec<RemoteItem>>,
}

fn filter_key(filter: ListFilter) -> u8 {
    match filter {
        ListFilter::All => 0,
        ListFilter::Dirs => 1,
        ListFilter::Files => 2,
    }
}

impl LibrcloneClient {
    /// `rclone_config_path` is the path librclone writes its config
    /// to. After every mutation we read the file back from this path
    /// and sync the contents to the keyring.
    pub fn new(rclone_config_path: PathBuf) -> Self {
        Self { rclone_config_path, listings: Arc::default() }
    }

    /// Drop the cached listings of `remote` after Celeste changed it itself, so the rest of the pass sees the change right away.
    fn invalidate(&self, remote: &str) {
        if let Ok(mut listings) = self.listings.lock() {
            listings.remove(remote);
        }
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
        let key = (path.to_owned(), recursive, filter_key(filter));
        let changed = celeste_go::remote_changed(remote);
        if let Ok(mut listings) = self.listings.lock() {
            let cached = listings.entry(remote.to_owned()).or_default();
            if changed || cached.filled_at.is_some_and(|at| at.elapsed() > LISTING_MAX_AGE) {
                *cached = Listings::default();
            }
            if let Some(items) = cached.entries.get(&key) {
                return Ok(items.clone());
            }
        }

        let items: Vec<RemoteItem> = rpc::sync::list(remote, path, recursive, map_filter(filter))
            .map(|items| items.into_iter().map(map_item).collect())
            .map_err(|err| err.error)?;
        // A change landing during the listing raises the change flag again, so the next call drops this entry.
        if let Ok(mut listings) = self.listings.lock() {
            let cached = listings.entry(remote.to_owned()).or_default();
            cached.filled_at.get_or_insert_with(Instant::now);
            cached.entries.insert(key, items.clone());
        }
        Ok(items)
    }

    fn details(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<Option<FileDetails>, String> {
        let item = rpc::sync::details(remote, path).map_err(|err| err.error)?;
        Ok(item.filter(|item| !item.is_dir).map(|item| FileDetails {
            size: u64::try_from(item.size).ok(),
            mod_time: item.mod_time,
            sha1: item.hashes.get("sha1").filter(|h| !h.is_empty()).map(|h| h.to_lowercase()),
        }))
    }

    fn mkdir(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        let result = rpc::sync::mkdir(remote, path).map_err(|err| err.error);
        self.invalidate(remote);
        result
    }

    fn delete_file(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        let result = rpc::sync::delete(remote, path).map_err(|err| err.error);
        self.invalidate(remote);
        result
    }

    fn purge(&self, remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        let result = rpc::sync::purge(remote, path).map_err(|err| err.error);
        self.invalidate(remote);
        result
    }

    fn copy_to_remote(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        let result = rpc::sync::copy_to_remote(local_path, remote, remote_path).map_err(|err| err.error);
        self.invalidate(remote);
        result
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
        self.invalidate(remote);
        celeste_go::forget_remote(remote);
        if result.is_ok() {
            self.sync_to_keyring();
        }
        result
    }

    fn create_config(&self, payload_json: String) -> Result<(), String> {
        // Reauthentication replaces an existing remote; start its change tracking over with the new credentials.
        if let Some(name) = serde_json::from_str::<serde_json::Value>(&payload_json).ok().and_then(|v| v.get("name")?.as_str().map(str::to_owned)) {
            self.invalidate(&name);
            celeste_go::forget_remote(&name);
        }
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
        let Some(config) = self.config(remote) else {
            return false;
        };
        config.get("type").and_then(|v| v.as_str()) == Some("drive")
            && config.get("client_id").and_then(|v| v.as_str()).is_none_or(str::is_empty)
    }

    /// A remote without its token or password, e.g. one imported from a settings file, waits for the user to sign in.
    fn needs_reauth(&self, remote: &str) -> bool {
        let Some(config) = self.config(remote) else {
            return false;
        };
        let missing = |key: &str| config.get(key).and_then(|v| v.as_str()).is_none_or(str::is_empty);
        match config.get("type").and_then(|v| v.as_str()) {
            Some("drive" | "dropbox" | "pcloud") => missing("token"),
            Some("webdav") => missing("pass"),
            _ => false,
        }
    }

    fn config(&self, remote: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
        let payload = serde_json::json!({ "name": remote }).to_string();
        let body = celeste_go::rpc("config/get", payload).ok()?;
        // `config/get` returns {} for unknown remotes.
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&body).ok().filter(|m| !m.is_empty())
    }
}
