//! Router that dispatches [`BackendClient`] calls to the appropriate
//! adapter per-remote. Holds a default (rclone-backed) client plus a
//! name-keyed override map that points native-backend remotes at a
//! `NativeProtonClient`. Sync code keeps talking to
//! `Arc<dyn BackendClient>` — the router is transparent.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use crate::domain::{
    ports::{BackendClient, Cancel},
    sync::{ListFilter, RemoteItem},
};

pub struct ClientRouter {
    default: Arc<dyn BackendClient>,
    overrides: RwLock<HashMap<String, Arc<dyn BackendClient>>>,
}

impl ClientRouter {
    /// Create a router whose fallback is `default`. No overrides
    /// registered initially — call [`register`] for each native
    /// remote at startup.
    pub fn new(default: Arc<dyn BackendClient>) -> Self {
        Self {
            default,
            overrides: RwLock::new(HashMap::new()),
        }
    }

    /// Register a per-remote override. Subsequent calls with that
    /// `remote` name route to `client` instead of the default.
    /// Replaces any existing override for the same name.
    pub fn register(&self, remote: String, client: Arc<dyn BackendClient>) {
        self.overrides.write().unwrap().insert(remote, client);
    }

    /// Drop an override — future calls with that name fall back to
    /// the default. No-op if the name isn't registered.
    pub fn unregister(&self, remote: &str) {
        self.overrides.write().unwrap().remove(remote);
    }

    /// Resolve which client should handle `remote`. Cheap read-lock
    /// hot path; the override map only mutates at app startup +
    /// add/remove remote.
    fn pick(&self, remote: &str) -> Arc<dyn BackendClient> {
        if let Some(client) = self.overrides.read().unwrap().get(remote) {
            return client.clone();
        }
        self.default.clone()
    }
}

impl BackendClient for ClientRouter {
    fn stat(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<Option<RemoteItem>, String> {
        self.pick(remote).stat(remote, path, cancel)
    }
    fn list(
        &self,
        remote: &str,
        path: &str,
        recursive: bool,
        filter: ListFilter,
        cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        self.pick(remote).list(remote, path, recursive, filter, cancel)
    }
    fn mkdir(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.pick(remote).mkdir(remote, path, cancel)
    }
    fn delete_file(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.pick(remote).delete_file(remote, path, cancel)
    }
    fn purge(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.pick(remote).purge(remote, path, cancel)
    }
    fn copy_to_remote(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        self.pick(remote).copy_to_remote(local_path, remote, remote_path, cancel)
    }
    fn copy_to_local(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        self.pick(remote).copy_to_local(local_path, remote, remote_path, cancel)
    }
    fn delete_config(&self, remote: &str) -> Result<(), String> {
        self.pick(remote).delete_config(remote)
    }
    fn create_config(&self, payload_json: String) -> Result<(), String> {
        // There's no remote name to route on here; create_config is
        // only called from the rclone-side add-remote flow. Always
        // route to the default (librclone) — native-backend adds
        // bypass this method entirely.
        self.default.create_config(payload_json)
    }
    fn remote_type(&self, remote: &str) -> Result<Option<String>, String> {
        self.pick(remote).remote_type(remote)
    }
    fn checkpoint_session(&self, remote: &str) {
        self.pick(remote).checkpoint_session(remote);
    }
}
