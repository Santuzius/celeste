//! Shared scaffolding for unit tests across `services/`.
//!
//! - [`TempDir`]: RAII-managed scratch directory under the system temp.
//! - [`FakeRepo`]: in-memory [`Repository`] honouring the real CRUD
//!   semantics on sync_items.
//! - [`FakeBackend`]: programmable [`BackendClient`] where every op's
//!   outcome is configurable per-path — the only way to exercise
//!   interruption scenarios (network error, stat cache race) against
//!   the sync algorithm without actually talking to rclone.

#![cfg(test)]
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use time::OffsetDateTime;

use crate::domain::{
    remote::{Backend, Remote, RemoteId, SyncPolicy},
    sync::{RemoteItem, SyncDir, SyncDirId},
};

mod fake_backend;
mod fake_repository;

pub use fake_backend::FakeBackend;
pub use fake_repository::FakeRepo;

/// Scratch directory rooted under `$TMPDIR/syncative_test_*`. Dropped
/// automatically; collisions across tests use the process ID + caller tag.
pub struct TempDir {
    pub path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> Self {
        let mut counter = 0u32;
        loop {
            let mut p = std::env::temp_dir();
            p.push(format!(
                "syncative_test_{}_{}_{}",
                tag,
                std::process::id(),
                counter
            ));
            if !p.exists() {
                fs::create_dir_all(&p).expect("create temp dir");
                return Self { path: p };
            }
            counter += 1;
        }
    }

    pub fn write_file(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, contents).unwrap();
        full
    }

    pub fn as_str(&self) -> &str {
        self.path.to_str().unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).ok();
    }
}

/// Build a test Remote with the given id and name.
pub fn remote(id: i32, name: &str) -> Remote {
    Remote {
        id: RemoteId(id),
        name: name.to_owned(),
        policy: SyncPolicy::default(),
        provider_kind: None,
        backend: Backend::Rclone,
        session_path: None,
    }
}

/// Build a test SyncDir. `local_path` is typically the TempDir path.
pub fn sync_dir(id: i32, remote_id: i32, local_path: &str, remote_path: &str) -> SyncDir {
    SyncDir {
        id: SyncDirId(id),
        remote_id: RemoteId(remote_id),
        local_path: local_path.to_owned(),
        remote_path: remote_path.to_owned(),
    }
}

/// Build a RemoteItem at the given path with the given Unix timestamp.
pub fn remote_item(path: &str, is_dir: bool, unix_ts: i64) -> RemoteItem {
    let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path);
    RemoteItem {
        is_dir,
        path: path.to_owned(),
        name: name.to_owned(),
        mod_time: OffsetDateTime::from_unix_timestamp(unix_ts).unwrap(),
    }
}

/// Set the mtime of a local file to `unix_ts` so tests can control the
/// local_utc_timestamp the sync algorithm reads.
pub fn touch_mtime(path: &Path, unix_ts: i64) {
    let _ = filetime_set(path, unix_ts);
}

fn filetime_set(path: &Path, unix_ts: i64) -> std::io::Result<()> {
    use std::fs::File;
    use std::time::{Duration, UNIX_EPOCH};
    let file = File::open(path)?;
    let time = UNIX_EPOCH + Duration::from_secs(unix_ts as u64);
    file.set_modified(time)?;
    Ok(())
}
