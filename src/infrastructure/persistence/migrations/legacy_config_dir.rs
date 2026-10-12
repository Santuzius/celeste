//! One-shot startup migration that drains `~/.config/celeste/` into
//! the new XDG-data location and the OS keyring.
//!
//! Distinct from the SeaORM migrations next door: this runs *before*
//! the database is opened, on the filesystem and against the keyring.
//! Lives here so all "things we do once on upgrade" share a folder.

use std::path::Path;

use crate::services::secrets;
use crate::util;

/// Move legacy state out of `~/.config/celeste/` (the previous home for
/// the SQLite DB and credential blobs) into the new data dir + keyring.
///
/// Three jobs:
///
/// 1. `data.sqlite` → `<data_dir>/data.sqlite` (file move).
/// 2. `proton-session-*.json` → keyring entry per remote, file deleted.
/// 3. `rclone.conf` → keyring entry, file deleted. Hydration to the
///    runtime path happens in `main.rs` after this returns; we don't
///    write any tokens to the data dir here.
///
/// Best-effort: any individual failure is logged and skipped — the
/// most important guarantee is that the user keeps their data even if
/// the keyring move trips on a missing Secret Service daemon.
pub fn run(data_dir: &Path) {
    let legacy = util::get_legacy_config_dir();
    if !legacy.exists() {
        return;
    }

    migrate_sqlite_db(&legacy, data_dir);
    migrate_proton_sessions(&legacy);
    migrate_rclone_config(&legacy);
}

fn migrate_sqlite_db(legacy: &Path, data_dir: &Path) {
    let legacy_db = legacy.join("data.sqlite");
    let new_db = data_dir.join("data.sqlite");
    if !legacy_db.exists() || new_db.exists() {
        return;
    }
    match std::fs::rename(&legacy_db, &new_db) {
        Ok(()) => eprintln!(
            "syncative: migrated SQLite DB to {}",
            new_db.display(),
        ),
        Err(err) => eprintln!(
            "syncative: couldn't move {} → {}: {err}",
            legacy_db.display(),
            new_db.display(),
        ),
    }
}

fn migrate_proton_sessions(legacy: &Path) {
    let Ok(entries) = std::fs::read_dir(legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(name) = file_name
            .strip_prefix("proton-session-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(body) => {
                let account = secrets::proton_account(name);
                match secrets::store(&account, &body) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&path);
                        eprintln!(
                            "syncative: migrated proton session for '{name}' into the keyring",
                        );
                    }
                    Err(err) => eprintln!(
                        "syncative: keyring store of legacy proton session for '{name}' failed: {err}",
                    ),
                }
            }
            Err(err) => eprintln!(
                "syncative: couldn't read legacy proton session {}: {err}",
                path.display(),
            ),
        }
    }
}

fn migrate_rclone_config(legacy: &Path) {
    let legacy_rclone = legacy.join("rclone.conf");
    if !legacy_rclone.exists() {
        return;
    }
    let body = match std::fs::read_to_string(&legacy_rclone) {
        Ok(body) => body,
        Err(err) => {
            eprintln!(
                "syncative: couldn't read {}: {err}",
                legacy_rclone.display(),
            );
            return;
        }
    };
    match secrets::store(secrets::RCLONE_ACCOUNT, &body) {
        Ok(()) => {
            let _ = std::fs::remove_file(&legacy_rclone);
            eprintln!("syncative: migrated rclone config into the keyring");
        }
        Err(err) => eprintln!("syncative: keyring store of legacy rclone.conf failed: {err}"),
    }
}
