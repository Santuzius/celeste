//! SeaORM-backed implementation of [`crate::domain::ports::Repository`].
//!
//! Per-entity CRUD lives in submodules; this file holds the [`Repository`]
//! impl and the shared row→domain mappers.

use sea_orm::DatabaseConnection;

use crate::domain::{
    ports::{BoxFuture, Repository, RepositoryError},
    remote::{Backend, Interval, Remote, RemoteId, SyncPolicy},
    sync::{SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId, SyncItem, SyncItemId},
};

use super::models::{RemotesModel, SyncDirsModel, SyncItemsModel};

mod exclusions;
mod remotes;
mod sync_dirs;
mod sync_items;

#[derive(Clone)]
pub struct SeaOrmRepository {
    db: DatabaseConnection,
}

impl SeaOrmRepository {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

fn map_remote(m: RemotesModel) -> Remote {
    Remote {
        id: RemoteId(m.id),
        name: m.name,
        policy: SyncPolicy {
            interval: Interval::from_seconds(m.sync_interval_seconds.max(1) as u64),
            enabled: m.enabled != 0,
        },
        // Enriched in the app layer via BackendClient::remote_type.
        provider_kind: None,
        backend: Backend::from_db_str(&m.backend),
        session_path: m.session_path,
    }
}

fn map_sync_dir(m: SyncDirsModel) -> SyncDir {
    SyncDir {
        id: SyncDirId(m.id),
        remote_id: RemoteId(m.remote_id),
        local_path: m.local_path,
        remote_path: m.remote_path,
    }
}

fn map_sync_item(m: SyncItemsModel) -> SyncItem {
    SyncItem {
        id: SyncItemId(m.id),
        sync_dir_id: SyncDirId(m.sync_dir_id),
        local_path: m.local_path,
        remote_path: m.remote_path,
        last_local_timestamp: m.last_local_timestamp as i64,
        last_remote_timestamp: m.last_remote_timestamp as i64,
    }
}

fn map_err<E: std::fmt::Display>(err: E) -> RepositoryError {
    RepositoryError::Other(err.to_string())
}

impl Repository for SeaOrmRepository {
    fn list_remotes(&self) -> BoxFuture<'_, Result<Vec<Remote>, RepositoryError>> {
        remotes::list_remotes(&self.db)
    }

    fn find_remote(
        &self,
        id: RemoteId,
    ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
        remotes::find_remote(&self.db, id)
    }

    fn find_remote_by_name(
        &self,
        name: &str,
    ) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
        remotes::find_remote_by_name(&self.db, name.to_owned())
    }

    fn insert_remote(&self, name: String) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
        remotes::insert_remote(&self.db, name)
    }

    fn insert_native_proton_remote(
        &self,
        name: String,
        session_path: String,
    ) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
        remotes::insert_native_proton_remote(&self.db, name, session_path)
    }

    fn delete_remote(&self, id: RemoteId) -> BoxFuture<'_, Result<(), RepositoryError>> {
        remotes::delete_remote(&self.db, id)
    }

    fn cascade_delete_remote(
        &self,
        id: RemoteId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        remotes::cascade_delete_remote(&self.db, id)
    }

    fn cascade_delete_sync_dir(
        &self,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_dirs::cascade_delete_sync_dir(&self.db, local_path.to_owned(), remote_path.to_owned())
    }

    fn set_policy(
        &self,
        id: RemoteId,
        policy: SyncPolicy,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        remotes::set_policy(&self.db, id, policy)
    }

    fn list_sync_dirs(
        &self,
        remote: RemoteId,
    ) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
        sync_dirs::list_sync_dirs(&self.db, remote)
    }

    fn list_all_sync_dirs(&self) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
        sync_dirs::list_all_sync_dirs(&self.db)
    }

    fn insert_sync_dir(
        &self,
        remote: RemoteId,
        local_path: String,
        remote_path: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_dirs::insert_sync_dir(&self.db, remote, local_path, remote_path)
    }

    fn sync_dir_exists(
        &self,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<bool, RepositoryError>> {
        sync_dirs::sync_dir_exists(&self.db, local_path.to_owned(), remote_path.to_owned())
    }

    fn list_sync_items(
        &self,
        sync_dir: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncItem>, RepositoryError>> {
        sync_items::list_sync_items(&self.db, sync_dir)
    }

    fn find_sync_item_by_paths(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        sync_items::find_sync_item_by_paths(
            &self.db,
            sync_dir,
            local_path.to_owned(),
            remote_path.to_owned(),
        )
    }

    fn find_sync_item_by_local(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        sync_items::find_sync_item_by_local(&self.db, sync_dir, local_path.to_owned())
    }

    fn find_sync_item_by_remote(
        &self,
        sync_dir: SyncDirId,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<Option<SyncItem>, RepositoryError>> {
        sync_items::find_sync_item_by_remote(&self.db, sync_dir, remote_path.to_owned())
    }

    fn delete_sync_item(
        &self,
        id: SyncItemId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_items::delete_sync_item(&self.db, id)
    }

    fn insert_sync_item(
        &self,
        sync_dir: SyncDirId,
        local_path: String,
        remote_path: String,
        last_local_timestamp: i64,
        last_remote_timestamp: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_items::insert_sync_item(
            &self.db,
            sync_dir,
            local_path,
            remote_path,
            last_local_timestamp,
            last_remote_timestamp,
        )
    }

    fn update_sync_item_timestamps(
        &self,
        id: SyncItemId,
        last_local_timestamp: i64,
        last_remote_timestamp: i64,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_items::update_sync_item_timestamps(
            &self.db,
            id,
            last_local_timestamp,
            last_remote_timestamp,
        )
    }

    fn delete_sync_item_by_paths(
        &self,
        sync_dir: SyncDirId,
        local_path: &str,
        remote_path: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_items::delete_sync_item_by_paths(
            &self.db,
            sync_dir,
            local_path.to_owned(),
            remote_path.to_owned(),
        )
    }

    fn list_exclusions(
        &self,
        sync_dir: SyncDirId,
    ) -> BoxFuture<'_, Result<Vec<SyncDirExclusion>, RepositoryError>> {
        exclusions::list_exclusions(&self.db, sync_dir)
    }

    fn insert_exclusion(
        &self,
        sync_dir: SyncDirId,
        remote_path: String,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        exclusions::insert_exclusion(&self.db, sync_dir, remote_path)
    }

    fn delete_exclusion(
        &self,
        id: SyncDirExclusionId,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        exclusions::delete_exclusion(&self.db, id)
    }

    fn delete_sync_items_with_local_prefix(
        &self,
        sync_dir: SyncDirId,
        local_prefix: &str,
    ) -> BoxFuture<'_, Result<(), RepositoryError>> {
        sync_items::delete_sync_items_with_local_prefix(
            &self.db,
            sync_dir,
            local_prefix.to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::Database;

    use super::SeaOrmRepository;
    use crate::{
        domain::ports::Repository,
        infrastructure::persistence::migrations::{Migrator, MigratorTrait},
        util::await_future,
    };

    #[test]
    fn folders_and_remotes_with_exclusions_can_be_removed() {
        let db = await_future(Database::connect("sqlite::memory:")).unwrap();
        await_future(Migrator::up(&db, None)).unwrap();
        let repo = SeaOrmRepository::new(db);
        let id = await_future(repo.insert_remote("R".to_owned())).unwrap();
        for local in ["/a", "/b"] {
            await_future(repo.insert_sync_dir(id, local.to_owned(), String::new())).unwrap();
        }
        for dir in await_future(repo.list_sync_dirs(id)).unwrap() {
            await_future(repo.insert_exclusion(dir.id, "Big".to_owned())).unwrap();
        }

        await_future(repo.cascade_delete_sync_dir("/a", "")).unwrap();
        assert_eq!(await_future(repo.list_sync_dirs(id)).unwrap().len(), 1);
        await_future(repo.cascade_delete_remote(id)).unwrap();
        assert!(await_future(repo.list_remotes()).unwrap().is_empty());
    }
}
