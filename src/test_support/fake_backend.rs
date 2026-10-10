//! Programmable [`BackendClient`]. Every op returns whatever the test sets
//! for the matching path (or a catch-all default). Call counts are
//! tracked for regression-style assertions.

#![cfg(test)]
#![allow(dead_code)]

use std::{collections::HashMap, sync::Mutex};

use crate::domain::{
    ports::{BackendClient, Cancel},
    sync::{FileDetails, ListFilter, RemoteItem},
};

pub struct FakeBackend {
    pub stat_map: Mutex<HashMap<String, Result<Option<RemoteItem>, String>>>,
    pub stat_sequence: Mutex<HashMap<String, Vec<Result<Option<RemoteItem>, String>>>>,
    pub list_map: Mutex<HashMap<String, Result<Vec<RemoteItem>, String>>>,
    /// SHA-1 reported by `details` per path; without an entry `details` falls back to `stat` without a digest.
    pub sha1_map: Mutex<HashMap<String, String>>,
    /// Modification time `details` reports per path (Proton: the one the uploader recorded); without an entry the `stat` time.
    pub details_time_map: Mutex<HashMap<String, i64>>,
    pub copy_to_remote_result: Mutex<Result<(), String>>,
    pub copy_to_local_result: Mutex<Result<(), String>>,
    pub delete_file_result: Mutex<Result<(), String>>,
    pub purge_result: Mutex<Result<(), String>>,
    pub mkdir_result: Mutex<Result<(), String>>,

    pub stat_calls: Mutex<Vec<String>>,
    pub list_calls: Mutex<Vec<String>>,
    pub copy_to_remote_calls: Mutex<Vec<(String, String)>>,
    pub copy_to_local_calls: Mutex<Vec<(String, String)>>,
    pub delete_file_calls: Mutex<Vec<String>>,
    pub purge_calls: Mutex<Vec<String>>,
    pub mkdir_calls: Mutex<Vec<String>>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            stat_map: Mutex::new(HashMap::new()),
            stat_sequence: Mutex::new(HashMap::new()),
            list_map: Mutex::new(HashMap::new()),
            sha1_map: Mutex::new(HashMap::new()),
            details_time_map: Mutex::new(HashMap::new()),
            copy_to_remote_result: Mutex::new(Ok(())),
            copy_to_local_result: Mutex::new(Ok(())),
            delete_file_result: Mutex::new(Ok(())),
            purge_result: Mutex::new(Ok(())),
            mkdir_result: Mutex::new(Ok(())),
            stat_calls: Mutex::new(Vec::new()),
            list_calls: Mutex::new(Vec::new()),
            copy_to_remote_calls: Mutex::new(Vec::new()),
            copy_to_local_calls: Mutex::new(Vec::new()),
            delete_file_calls: Mutex::new(Vec::new()),
            purge_calls: Mutex::new(Vec::new()),
            mkdir_calls: Mutex::new(Vec::new()),
        }
    }
}

impl FakeBackend {
    pub fn set_stat(&self, path: &str, resp: Result<Option<RemoteItem>, String>) {
        self.stat_map.lock().unwrap().insert(path.to_owned(), resp);
    }
    /// Queue an ordered sequence of stat responses for a path. Each
    /// call consumes one entry; once exhausted, falls back to
    /// `stat_map` / `Ok(None)`. Useful for testing flows that stat
    /// the same path before and after a mutation.
    pub fn set_stat_sequence(
        &self,
        path: &str,
        responses: Vec<Result<Option<RemoteItem>, String>>,
    ) {
        self.stat_sequence
            .lock()
            .unwrap()
            .insert(path.to_owned(), responses);
    }
    pub fn set_sha1(&self, path: &str, sha1: &str) {
        self.sha1_map.lock().unwrap().insert(path.to_owned(), sha1.to_owned());
    }
    pub fn set_list(&self, path: &str, resp: Result<Vec<RemoteItem>, String>) {
        self.list_map.lock().unwrap().insert(path.to_owned(), resp);
    }
    pub fn set_copy_to_remote(&self, resp: Result<(), String>) {
        *self.copy_to_remote_result.lock().unwrap() = resp;
    }
    pub fn set_copy_to_local(&self, resp: Result<(), String>) {
        *self.copy_to_local_result.lock().unwrap() = resp;
    }
    pub fn set_delete_file(&self, resp: Result<(), String>) {
        *self.delete_file_result.lock().unwrap() = resp;
    }
    pub fn set_purge(&self, resp: Result<(), String>) {
        *self.purge_result.lock().unwrap() = resp;
    }
}

impl BackendClient for FakeBackend {
    fn stat(
        &self,
        _remote: &str,
        path: &str,
        _cancel: &Cancel,
    ) -> Result<Option<RemoteItem>, String> {
        self.stat_calls.lock().unwrap().push(path.to_owned());
        // Sequence wins if present: pop the next response.
        if let Some(seq) = self.stat_sequence.lock().unwrap().get_mut(path)
            && !seq.is_empty()
        {
            return seq.remove(0);
        }
        match self.stat_map.lock().unwrap().get(path) {
            Some(r) => r.clone(),
            None => Ok(None),
        }
    }
    fn list(
        &self,
        _remote: &str,
        path: &str,
        _recursive: bool,
        _filter: ListFilter,
        _cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        self.list_calls.lock().unwrap().push(path.to_owned());
        match self.list_map.lock().unwrap().get(path) {
            Some(r) => r.clone(),
            None => Ok(vec![]),
        }
    }
    fn details(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<Option<FileDetails>, String> {
        let sha1 = self.sha1_map.lock().unwrap().get(path).cloned();
        let recorded = self.details_time_map.lock().unwrap().get(path).copied();
        Ok(self.stat(remote, path, cancel)?.filter(|item| !item.is_dir).map(|item| FileDetails {
            size: None,
            mod_time: recorded.map_or(item.mod_time, |t| time::OffsetDateTime::from_unix_timestamp(t).unwrap()),
            sha1,
        }))
    }
    fn mkdir(&self, _remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        self.mkdir_calls.lock().unwrap().push(path.to_owned());
        self.mkdir_result.lock().unwrap().clone()
    }
    fn delete_file(&self, _remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        self.delete_file_calls.lock().unwrap().push(path.to_owned());
        self.delete_file_result.lock().unwrap().clone()
    }
    fn purge(&self, _remote: &str, path: &str, _cancel: &Cancel) -> Result<(), String> {
        self.purge_calls.lock().unwrap().push(path.to_owned());
        self.purge_result.lock().unwrap().clone()
    }
    fn copy_to_remote(
        &self,
        local: &str,
        _remote: &str,
        path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        self.copy_to_remote_calls
            .lock()
            .unwrap()
            .push((local.to_owned(), path.to_owned()));
        self.copy_to_remote_result.lock().unwrap().clone()
    }
    fn copy_to_local(
        &self,
        local: &str,
        _remote: &str,
        path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        self.copy_to_local_calls
            .lock()
            .unwrap()
            .push((local.to_owned(), path.to_owned()));
        let result = self.copy_to_local_result.lock().unwrap().clone();
        // Like a real download: the file appears, with the time of the download.
        if result.is_ok() {
            let _ = std::fs::write(local, b"downloaded");
        }
        result
    }
    fn delete_config(&self, _remote: &str) -> Result<(), String> {
        Ok(())
    }
    fn create_config(&self, _payload: String) -> Result<(), String> {
        Ok(())
    }
    fn remote_type(&self, _remote: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
}
