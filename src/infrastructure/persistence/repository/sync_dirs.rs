//! Sync-dir CRUD against `sync_dirs`.

use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter,
};

use crate::domain::{
    ports::{BoxFuture, RepositoryError},
    remote::RemoteId,
    sync::SyncDir,
};

use super::super::models::{
    SyncDirExclusionsColumn, SyncDirExclusionsEntity, SyncDirsActiveModel, SyncDirsColumn, SyncDirsEntity, SyncItemsColumn, SyncItemsEntity,
};
use super::{map_err, map_sync_dir};

pub(super) fn list_sync_dirs(
    db: &DatabaseConnection,
    remote: RemoteId,
) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
    Box::pin(async move {
        let rows = SyncDirsEntity::find()
            .filter(SyncDirsColumn::RemoteId.eq(remote.0))
            .all(db)
            .await
            .map_err(map_err)?;
        Ok(rows.into_iter().map(map_sync_dir).collect())
    })
}

pub(super) fn list_all_sync_dirs(
    db: &DatabaseConnection,
) -> BoxFuture<'_, Result<Vec<SyncDir>, RepositoryError>> {
    Box::pin(async move {
        let rows = SyncDirsEntity::find().all(db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(map_sync_dir).collect())
    })
}

pub(super) fn insert_sync_dir(
    db: &DatabaseConnection,
    remote: RemoteId,
    local_path: String,
    remote_path: String,
) -> BoxFuture<'_, Result<(), RepositoryError>> {
    Box::pin(async move {
        let active = SyncDirsActiveModel {
            remote_id: ActiveValue::Set(remote.0),
            local_path: ActiveValue::Set(local_path),
            remote_path: ActiveValue::Set(remote_path),
            ..Default::default()
        };
        SyncDirsEntity::insert(active)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(())
    })
}

pub(super) fn sync_dir_exists(
    db: &DatabaseConnection,
    local_path: String,
    remote_path: String,
) -> BoxFuture<'_, Result<bool, RepositoryError>> {
    Box::pin(async move {
        let row = SyncDirsEntity::find()
            .filter(SyncDirsColumn::LocalPath.eq(local_path))
            .filter(SyncDirsColumn::RemotePath.eq(remote_path))
            .one(db)
            .await
            .map_err(map_err)?;
        Ok(row.is_some())
    })
}

pub(super) fn cascade_delete_sync_dir(
    db: &DatabaseConnection,
    local_path: String,
    remote_path: String,
) -> BoxFuture<'_, Result<(), RepositoryError>> {
    Box::pin(async move {
        if let Some(sd) = SyncDirsEntity::find()
            .filter(SyncDirsColumn::LocalPath.eq(local_path))
            .filter(SyncDirsColumn::RemotePath.eq(remote_path))
            .one(db)
            .await
            .map_err(map_err)?
        {
            SyncItemsEntity::delete_many()
                .filter(SyncItemsColumn::SyncDirId.eq(sd.id))
                .exec(db)
                .await
                .map_err(map_err)?;
            // Its exclusions refer to it; SQLite refuses to delete it before them.
            SyncDirExclusionsEntity::delete_many()
                .filter(SyncDirExclusionsColumn::SyncDirId.eq(sd.id))
                .exec(db)
                .await
                .map_err(map_err)?;
            sd.delete(db).await.map_err(map_err)?;
        }
        Ok(())
    })
}
