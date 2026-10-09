//! In-memory [`Repository`] for tests. Keeps the sync_items table plus plain lists of remotes and sync_dirs (for the engine) and
//! exposes a few helper accessors for assertions. Ignores the remaining writes.

#![cfg(test)]
#![allow(dead_code)]

use std::sync::Mutex;

use crate::domain::{
    ports::{BoxFuture, Repository, RepositoryError},
    remote::{Remote, RemoteId, SyncPolicy},
    sync::{SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId, SyncItem, SyncItemId},
};

#[derive(Default)]
pub struct FakeRepo {
    pub items: Mutex<Vec<SyncItem>>,
    pub next_id: Mutex<i32>,
    pub sync_dir_exists: Mutex<bool>,
    pub remotes: Mutex<Vec<Remote>>,
    pub sync_dirs: Mutex<Vec<SyncDir>>,
}

impl FakeRepo {
    pub fn new() -> Self {
        Self {
            items: Mutex::new(vec![]),
            next_id: Mutex::new(0),
            sync_dir_exists: Mutex::new(true),
            remotes: Mutex::new(vec![]),
            sync_dirs: Mutex::new(vec![]),
        }
    }

    pub fn insert_item(
        &self,
        sync_dir_id: SyncDirId,
        local_path: &str,
        remote_path: &str,
        local_ts: i64,
        remote_ts: i64,
    ) -> SyncItemId {
        let mut next = self.next_id.lock().unwrap();
        *next += 1;
        let id = SyncItemId(*next);
        self.items.lock().unwrap().push(SyncItem {
            id,
            sync_dir_id,
            local_path: local_path.to_owned(),
            remote_path: remote_path.to_owned(),
            last_local_timestamp: local_ts,
            last_remote_timestamp: remote_ts,
        });
        id
    }

    pub fn item_count(&self) -> usize {
        self.items.lock().unwrap().len()
    }

    pub fn has_item(&self, local_path: &str, remote_path: &str) -> bool {
        self.items
            .lock()
            .unwrap()
            .iter()
            .any(|it| it.local_path == local_path && it.remote_path == remote_path)
    }
}

impl Repository for FakeRepo {
    fn list_remotes(&self) -> BoxFuture<'_, Result<Vec<Remote>, RepositoryError>> {
        let remotes = self.remotes.lock().unwrap().clone();
        Box::pin(async move { Ok(remotes) })
    }
    fn find_remote(
        &self,
        id: RemoteId,
    ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
        let remote = self.remotes.lock().unwrap().iter().find(|r| r.id == id).cloned();
        Box::pin(async move { Ok(remote) })
    }
    fn find_remote_by_name(
        &self,
        _name: &str,
    ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
        Box::pin(async { Ok(None) })
    }
    fn insert_remote(&self, _name: String) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
        Box::pin(async { Ok(RemoteId(1)) })
    }
    fn insert_native_proton_remote(
        &self,
        _name: String,
        _session_path: String,
    ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
        Box::pin(async { Ok(RemoteId(1)) })
    }
    fn delete_remote(&self, _id: RemoteId) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn cascade_delete_remote(
        &self,
        _id: RemoteId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn cascade_delete_sync_dir(
        &self,
        _local: &str,
        _remote: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn set_policy(
        &self,
        id: RemoteId,
        p: SyncPolicy,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        if let Some(remote) = self.remotes.lock().unwrap().iter_mut().find(|r| r.id == id) {
            remote.policy = p;
        }
        Box::pin(async { Ok(()) })
    }
    fn list_sync_dirs(
        &self,
        r: RemoteId,
    ) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
        let dirs = self.sync_dirs.lock().unwrap().iter().filter(|d| d.remote_id == r).cloned().collect();
        Box::pin(async move { Ok(dirs) })
    }
    fn list_all_sync_dirs(&self) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
        let dirs = self.sync_dirs.lock().unwrap().clone();
        Box::pin(async move { Ok(dirs) })
    }
    fn sync_dir_exists(
        &self,
        _l: &str,
        _r: &str,
    ) -> BoxFuture<'_, Result<bool, RepositoryError>> {
        let exists = *self.sync_dir_exists.lock().unwrap();
        Box::pin(async move { Ok(exists) })
    }
    fn insert_sync_dir(
        &self,
        _r: RemoteId,
        _l: String,
        _rp: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn list_sync_items(
        &self,
        sd: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncItem>, RepositoryError>> {
        let items: Vec<SyncItem> = self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|it| it.sync_dir_id == sd)
            .cloned()
            .collect();
        Box::pin(async move { Ok(items) })
    }
    fn find_sync_item_by_paths(
        &self,
        sd: SyncDirId,
        local: &str,
        remote: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        let found = self
            .items
            .lock()
            .unwrap()
            .iter()
            .find(|it| it.sync_dir_id == sd && it.local_path == local && it.remote_path == remote)
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn find_sync_item_by_local(
        &self,
        sd: SyncDirId,
        local: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        let found = self
            .items
            .lock()
            .unwrap()
            .iter()
            .find(|it| it.sync_dir_id == sd && it.local_path == local)
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn find_sync_item_by_remote(
        &self,
        sd: SyncDirId,
        remote: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        let found = self
            .items
            .lock()
            .unwrap()
            .iter()
            .find(|it| it.sync_dir_id == sd && it.remote_path == remote)
            .cloned();
        Box::pin(async move { Ok(found) })
    }
    fn insert_sync_item(
        &self,
        sd: SyncDirId,
        local: String,
        remote: String,
        lt: i64,
        rt: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        self.insert_item(sd, &local, &remote, lt, rt);
        Box::pin(async { Ok(()) })
    }
    fn update_sync_item_timestamps(
        &self,
        id: SyncItemId,
        lt: i64,
        rt: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        let mut items = self.items.lock().unwrap();
        if let Some(item) = items.iter_mut().find(|it| it.id == id) {
            item.last_local_timestamp = lt;
            item.last_remote_timestamp = rt;
        }
        Box::pin(async { Ok(()) })
    }
    fn delete_sync_item(
        &self,
        id: SyncItemId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        self.items.lock().unwrap().retain(|it| it.id != id);
        Box::pin(async { Ok(()) })
    }
    fn delete_sync_item_by_paths(
        &self,
        sd: SyncDirId,
        local: &str,
        remote: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        self.items
            .lock()
            .unwrap()
            .retain(|it| !(it.sync_dir_id == sd && it.local_path == local && it.remote_path == remote));
        Box::pin(async { Ok(()) })
    }
    fn list_exclusions(
        &self,
        _sd: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncDirExclusion>, RepositoryError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn insert_exclusion(
        &self,
        _sd: SyncDirId,
        _remote_path: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn delete_exclusion(
        &self,
        _id: SyncDirExclusionId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        Box::pin(async { Ok(()) })
    }
    fn delete_sync_items_with_local_prefix(
        &self,
        sd: SyncDirId,
        prefix: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        let prefix = prefix.to_owned();
        let child_prefix = format!("{prefix}/");
        self.items.lock().unwrap().retain(|it| {
            !(it.sync_dir_id == sd
                && (it.local_path == prefix || it.local_path.starts_with(&child_prefix)))
        });
        Box::pin(async { Ok(()) })
    }
}
