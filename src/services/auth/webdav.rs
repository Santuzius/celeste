//! WebDAV / Nextcloud / Owncloud remote provisioning.

use serde_json::json;

use crate::{
    domain::{
        ports::{BackendClient, Repository},
        remote::RemoteId,
    },
    util,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebDavVendor {
    WebDav,
    Nextcloud,
    Owncloud,
}

impl WebDavVendor {
    pub(super) fn rclone_vendor(self) -> &'static str {
        match self {
            WebDavVendor::WebDav => "webdav",
            WebDavVendor::Nextcloud => "nextcloud",
            WebDavVendor::Owncloud => "owncloud",
        }
    }
}

/// Add a new WebDAV-family remote: create the rclone config, insert the
/// DB row, and return the new `RemoteId`. Validation (trying to list the
/// root) is the caller's responsibility — mirrors the GTK flow which
/// validates via `can_login` before inserting.
pub fn add_webdav_remote(
    name: &str,
    url: &str,
    user: &str,
    pass: &str,
    vendor: WebDavVendor,
    repo: &dyn Repository,
    client: &dyn BackendClient,
) -> Result<RemoteId, String> {
    client.create_config(config_payload(name, url, user, pass, vendor))?;
    util::await_future(repo.insert_remote(name.to_owned()))
        .map_err(|e| e.to_string())
}

/// Sign in to an existing WebDAV-family remote again, e.g. one imported without its password: replace its rclone config under the same name and keep the DB row with its folders.
pub fn reauth_webdav_remote(name: &str, url: &str, user: &str, pass: &str, vendor: WebDavVendor, client: &dyn BackendClient) -> Result<(), String> {
    // rclone's `config/create` rejects a name that already exists.
    let _ = client.delete_config(name);
    client.create_config(config_payload(name, url, user, pass, vendor))
}

/// The URL the form takes back from a stored config: Nextcloud and Owncloud get their WebDAV path appended again on saving.
pub fn form_url(stored_url: &str, user: &str, vendor: WebDavVendor) -> String {
    match vendor {
        WebDavVendor::Nextcloud | WebDavVendor::Owncloud => stored_url.strip_suffix(&format!("/remote.php/dav/files/{user}")).unwrap_or(stored_url).to_owned(),
        WebDavVendor::WebDav => stored_url.to_owned(),
    }
}

fn config_payload(name: &str, url: &str, user: &str, pass: &str, vendor: WebDavVendor) -> String {
    // For Nextcloud/Owncloud the GTK flow reformats the URL to include
    // `/remote.php/dav/files/<user>`; mirror that here so configs the
    // Iced UI creates line up with configs the GTK UI creates.
    let effective_url = match vendor {
        WebDavVendor::Nextcloud | WebDavVendor::Owncloud => {
            let trimmed = url.trim_end_matches('/');
            format!("{trimmed}/remote.php/dav/files/{user}")
        }
        WebDavVendor::WebDav => url.to_owned(),
    };

    json!({
        "name": name,
        "parameters": {
            "url": effective_url,
            "vendor": vendor.rclone_vendor(),
            "user": user,
            "pass": pass,
        },
        "type": "webdav",
        "opt": { "obscure": true },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::domain::{
        ports::{BoxFuture, Repository, RepositoryError},
        remote::{Remote, RemoteId, SyncPolicy},
        sync::{
            ListFilter, RemoteItem, SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId,
            SyncItem, SyncItemId,
        },
    };

    #[derive(Default)]
    struct FakeRepo {
        inserted: Mutex<Vec<String>>,
        next_id: Mutex<i32>,
    }

    impl Repository for FakeRepo {
        fn list_remotes(&self) -> BoxFuture<'_, Result<Vec<Remote>, RepositoryError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn find_remote(
            &self,
            _id: RemoteId,
        ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
            Box::pin(async { Ok(None) })
        }
        fn find_remote_by_name(
            &self,
            _name: &str,
        ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
            Box::pin(async { Ok(None) })
        }
        fn insert_remote(
            &self,
            name: String,
        ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
            let mut inserted = self.inserted.lock().unwrap();
            inserted.push(name);
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            let id = RemoteId(*next);
            Box::pin(async move { Ok(id) })
        }
        fn insert_native_proton_remote(
            &self,
            name: String,
            _session_path: String,
        ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
            let mut inserted = self.inserted.lock().unwrap();
            inserted.push(name);
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            let id = RemoteId(*next);
            Box::pin(async move { Ok(id) })
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
            _id: RemoteId,
            _p: SyncPolicy,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
            Box::pin(async { Ok(()) })
        }
        fn list_sync_dirs(
            &self,
            _r: RemoteId,
        ) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn sync_dir_exists(
            &self,
            _l: &str,
            _r: &str,
        ) -> BoxFuture<'_, Result<bool, RepositoryError>> {
            Box::pin(async { Ok(false) })
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
            _sd: SyncDirId,
        ) -> BoxFuture<'_, Result<Vec<SyncItem>, RepositoryError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn find_sync_item_by_paths(
            &self,
            _sd: SyncDirId,
            _l: &str,
            _r: &str,
        ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
            Box::pin(async { Ok(None) })
        }
        fn find_sync_item_by_local(
            &self,
            _sd: SyncDirId,
            _l: &str,
        ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
            Box::pin(async { Ok(None) })
        }
        fn find_sync_item_by_remote(
            &self,
            _sd: SyncDirId,
            _r: &str,
        ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
            Box::pin(async { Ok(None) })
        }
        fn insert_sync_item(
            &self,
            _sd: SyncDirId,
            _l: String,
            _r: String,
            _lt: i64,
            _rt: i64,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
            Box::pin(async { Ok(()) })
        }
        fn update_sync_item_timestamps(
            &self,
            _id: SyncItemId,
            _lt: i64,
            _rt: i64,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
            Box::pin(async { Ok(()) })
        }
        fn delete_sync_item(
            &self,
            _id: SyncItemId,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
            Box::pin(async { Ok(()) })
        }
        fn delete_sync_item_by_paths(
            &self,
            _sd: SyncDirId,
            _l: &str,
            _r: &str,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
            Box::pin(async { Ok(()) })
        }
        fn list_all_sync_dirs(
            &self,
        ) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
            Box::pin(async { Ok(vec![]) })
        }
        fn delete_sync_items_with_local_prefix(
            &self,
            _sd: SyncDirId,
            _prefix: &str,
        ) -> BoxFuture<'_, Result<(), RepositoryError>> {
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
    }

    #[derive(Default)]
    struct FakeRclone {
        created: Mutex<Vec<String>>,
    }

    impl crate::domain::ports::BackendClient for FakeRclone {
        fn stat(
            &self,
            _r: &str,
            _p: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<Option<RemoteItem>, String> {
            Ok(None)
        }
        fn list(
            &self,
            _r: &str,
            _p: &str,
            _rec: bool,
            _f: ListFilter,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<Vec<RemoteItem>, String> {
            Ok(vec![])
        }
        fn mkdir(
            &self,
            _r: &str,
            _p: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<(), String> {
            Ok(())
        }
        fn delete_file(
            &self,
            _r: &str,
            _p: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<(), String> {
            Ok(())
        }
        fn purge(
            &self,
            _r: &str,
            _p: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<(), String> {
            Ok(())
        }
        fn copy_to_remote(
            &self,
            _l: &str,
            _r: &str,
            _rp: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<(), String> {
            Ok(())
        }
        fn copy_to_local(
            &self,
            _l: &str,
            _r: &str,
            _rp: &str,
            _c: &crate::domain::ports::Cancel,
        ) -> Result<(), String> {
            Ok(())
        }
        fn delete_config(&self, _r: &str) -> Result<(), String> {
            Ok(())
        }
        fn create_config(&self, payload: String) -> Result<(), String> {
            self.created.lock().unwrap().push(payload);
            Ok(())
        }
        fn remote_type(&self, _r: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    #[test]
    fn webdav_creates_rclone_config_and_inserts_row() {
        let repo = FakeRepo::default();
        let client = FakeRclone::default();

        let id = add_webdav_remote(
            "Home NAS",
            "https://nas.example.org/webdav",
            "alex",
            "hunter2",
            WebDavVendor::WebDav,
            &repo,
            &client,
        )
        .expect("add should succeed against fakes");

        assert_eq!(id.0, 1);
        assert_eq!(repo.inserted.lock().unwrap().as_slice(), &["Home NAS".to_owned()]);
        let created = client.created.lock().unwrap();
        assert_eq!(created.len(), 1);
        assert!(created[0].contains("\"vendor\":\"webdav\""));
        assert!(created[0].contains("\"user\":\"alex\""));
    }

    #[test]
    fn nextcloud_reformats_the_url() {
        let repo = FakeRepo::default();
        let client = FakeRclone::default();

        add_webdav_remote(
            "Work Nextcloud",
            "https://cloud.example.org",
            "alex",
            "hunter2",
            WebDavVendor::Nextcloud,
            &repo,
            &client,
        )
        .unwrap();

        let created = client.created.lock().unwrap();
        assert!(
            created[0].contains("/remote.php/dav/files/alex"),
            "expected Nextcloud URL to be rewritten, got: {}",
            created[0]
        );
    }
}
