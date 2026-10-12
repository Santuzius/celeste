pub use sea_orm_migration::prelude::*;

pub mod celeste_to_syncative;
pub mod legacy_config_dir;

mod m20260419_000001_initial;
mod m20260421_000001_exclusions;

/// Legacy migration IDs we refuse to cohabit with. If any of these show
/// up in `seaql_migrations` at startup, the old-config popup fires and
/// the app exits — see `infrastructure::persistence::detect_legacy_db`.
///
/// `m20260418_000001_initial` is the previous schema (pre-`backend` +
/// `session_path` columns on remotes). We bump the migration ID every
/// time we reshape the core schema rather than patching via ALTERs —
/// the fork is single-user, and wiping ~/.local/share/syncative/ is
/// cheaper than accreting migration state.
pub const LEGACY_MIGRATION_IDS: &[&str] = &[
    "m20220101_000001_create_table",
    "m20230207_204909_sync_dirs_remove_slash_suffix",
    "m20230220_215840_remote_sync_items_fix",
    "m20260417_000001_add_sync_policy",
    "m20260418_000001_initial",
];

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260419_000001_initial::Migration),
            Box::new(m20260421_000001_exclusions::Migration),
        ]
    }
}
