//! Fresh schema. The old migrations were deleted wholesale — if an
//! existing data dir has any of the legacy `seaql_migrations` rows the
//! startup check refuses to boot and tells the user to delete
//! `~/.local/share/syncative/` manually. See
//! `infrastructure::persistence::mod`.

use sea_orm::{ConnectionTrait, Statement};
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = r#"
            CREATE TABLE remotes (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                name TEXT NOT NULL,
                sync_interval_seconds INTEGER NOT NULL DEFAULT 15,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_sync_at INTEGER NULL,
                last_sync_status TEXT NULL,
                -- Which adapter drives this remote: "rclone" (default,
                -- via librclone's backends) or "native-proton" (our
                -- own ProtonDrive client in native-go/). Stamped when
                -- the remote is added; never mutated on an existing
                -- row.
                backend TEXT NOT NULL DEFAULT 'rclone',
                -- Marker that a native-backend remote has a session
                -- saved to the OS keyring (lookup uses the remote's
                -- name). NULL for rclone-backed remotes (rclone owns
                -- their config) and for native remotes still pending
                -- reauthentication. Column is named `session_path` for
                -- backward compatibility with the prior file-based
                -- layout; values are no longer filesystem paths.
                session_path TEXT NULL
            );

            CREATE TABLE sync_dirs (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                remote_id INTEGER NOT NULL,
                local_path TEXT NOT NULL,
                remote_path TEXT NOT NULL,
                FOREIGN KEY(remote_id) REFERENCES remotes(id)
            );

            CREATE TABLE sync_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
                sync_dir_id INTEGER NOT NULL,
                local_path TEXT NOT NULL,
                remote_path TEXT NOT NULL,
                last_local_timestamp INTEGER NOT NULL,
                last_remote_timestamp INTEGER NOT NULL,
                FOREIGN KEY(sync_dir_id) REFERENCES sync_dirs(id)
            );
        "#;
        let stmt = Statement::from_string(manager.get_database_backend(), sql.to_owned());
        manager.get_connection().execute(stmt).await.map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = "
            DROP TABLE sync_items;
            DROP TABLE sync_dirs;
            DROP TABLE remotes;
        ";
        let stmt = Statement::from_string(manager.get_database_backend(), sql.to_owned());
        manager.get_connection().execute(stmt).await.map(|_| ())
    }
}
