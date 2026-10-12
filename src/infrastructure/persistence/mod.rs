pub mod migrations;
pub mod models;
pub mod repository;

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use self::migrations::LEGACY_MIGRATION_IDS;

/// Returns true when the connected DB has any legacy migration row —
/// a signal that the schema is from a pre-2026-04-18 version we no
/// longer understand. Startup treats this as a hard block: show the
/// popup, ask the user to remove `~/.local/share/syncative/` manually,
/// exit.
pub async fn has_legacy_migrations(db: &DatabaseConnection) -> bool {
    let table_exists = db
        .query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='seaql_migrations'"
                .to_owned(),
        ))
        .await
        .ok()
        .flatten()
        .is_some();
    if !table_exists {
        return false;
    }
    for id in LEGACY_MIGRATION_IDS {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                "SELECT 1 FROM seaql_migrations WHERE version = ? LIMIT 1",
                [(*id).into()],
            ))
            .await
            .ok()
            .flatten();
        if row.is_some() {
            return true;
        }
    }
    false
}
