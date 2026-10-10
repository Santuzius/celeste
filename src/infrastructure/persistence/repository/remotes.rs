//! Remote-row CRUD against `remotes`.

use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, ModelTrait, QueryFilter,
};

use crate::domain::{
    ports::{BoxFuture, RepositoryError},
    remote::{Backend, Remote, RemoteId, SyncPolicy},
};

use super::super::models::{
    RemotesActiveModel, RemotesColumn, RemotesEntity, SyncDirExclusionsColumn, SyncDirExclusionsEntity, SyncDirsColumn, SyncDirsEntity,
    SyncItemsColumn, SyncItemsEntity,
};
use super::{map_err, map_remote};

pub(super) fn list_remotes(
    db: &DatabaseConnection,
) -> BoxFuture<'_, Result<Vec<Remote>, RepositoryError>> {
    Box::pin(async move {
        let rows = RemotesEntity::find().all(db).await.map_err(map_err)?;
        Ok(rows.into_iter().map(map_remote).collect())
    })
}

pub(super) fn find_remote(
    db: &DatabaseConnection,
    id: RemoteId,
) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
    Box::pin(async move {
        let row = RemotesEntity::find_by_id(id.0)
            .one(db)
            .await
            .map_err(map_err)?;
        Ok(row.map(map_remote))
    })
}

pub(super) fn find_remote_by_name(
    db: &DatabaseConnection,
    name: String,
) -> BoxFuture<'_, Result<Option<Remote>, RepositoryError>> {
    Box::pin(async move {
        let row = RemotesEntity::find()
            .filter(RemotesColumn::Name.eq(name))
            .one(db)
            .await
            .map_err(map_err)?;
        Ok(row.map(map_remote))
    })
}

pub(super) fn insert_remote(
    db: &DatabaseConnection,
    name: String,
) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
    Box::pin(async move {
        let active = RemotesActiveModel {
            name: ActiveValue::Set(name),
            ..Default::default()
        };
        let res = RemotesEntity::insert(active)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(RemoteId(res.last_insert_id))
    })
}

pub(super) fn insert_native_proton_remote(
    db: &DatabaseConnection,
    name: String,
    session_path: String,
) -> BoxFuture<'_, Result<RemoteId, RepositoryError>> {
    Box::pin(async move {
        let active = RemotesActiveModel {
            name: ActiveValue::Set(name),
            backend: ActiveValue::Set(Backend::NativeProton.as_db_str().to_owned()),
            session_path: ActiveValue::Set(Some(session_path)),
            ..Default::default()
        };
        let res = RemotesEntity::insert(active)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(RemoteId(res.last_insert_id))
    })
}

pub(super) fn delete_remote(
    db: &DatabaseConnection,
    id: RemoteId,
) -> BoxFuture<'_, Result<(), RepositoryError>> {
    Box::pin(async move {
        RemotesEntity::delete_by_id(id.0)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(())
    })
}

pub(super) fn cascade_delete_remote(
    db: &DatabaseConnection,
    id: RemoteId,
) -> BoxFuture<'_, Result<(), RepositoryError>> {
    Box::pin(async move {
        let sync_dirs = SyncDirsEntity::find()
            .filter(SyncDirsColumn::RemoteId.eq(id.0))
            .all(db)
            .await
            .map_err(map_err)?;
        for sd in sync_dirs {
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
        RemotesEntity::delete_by_id(id.0)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(())
    })
}

pub(super) fn set_policy(
    db: &DatabaseConnection,
    id: RemoteId,
    policy: SyncPolicy,
) -> BoxFuture<'_, Result<(), RepositoryError>> {
    Box::pin(async move {
        let active = RemotesActiveModel {
            id: ActiveValue::Unchanged(id.0),
            sync_interval_seconds: ActiveValue::Set(policy.interval.seconds() as i32),
            enabled: ActiveValue::Set(policy.enabled as i32),
            ..Default::default()
        };
        RemotesEntity::update(active)
            .exec(db)
            .await
            .map_err(map_err)?;
        Ok(())
    })
}
