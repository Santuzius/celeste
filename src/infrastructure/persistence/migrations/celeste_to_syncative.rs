//! One-shot migration from the old name Celeste to Syncative.
//!
//! - The data dir moves from `${XDG_DATA_HOME:-~/.local/share}/celeste` to `…/syncative` at startup, before anything opens it. After the move, the old cache dir, which only held the icon, is removed.
//! - Keyring entries move from the service `Celeste Keys` to `Syncative Keys` the first time each is read (`take_celeste_key`), so no list of accounts is needed. `secrets::delete` also removes an entry left under the old service.
//!
//! - The autostart entry `celeste.desktop` gives way to Syncative's own (`move_autostart_entry`).
//!
//! On Android the app id changed with the name, so the old app's data is out of reach and nothing moves.

use std::{fs::File, os::fd::AsRawFd, path::Path};

/// Keyring service of the entries written before the rename.
#[cfg(not(target_os = "android"))]
const CELESTE_SERVICE: &str = "Celeste Keys";

/// A Celeste process still owns the old data dir.
pub struct CelesteRunning;

/// Move the old data dir to `data_dir`, unless `data_dir` exists already; `Ok(true)` when it moved. Refuses while Celeste still runs (e.g. from the tray during an update): both would then sync the same folders from the same database.
pub fn move_data_dir(data_dir: &Path) -> Result<bool, CelesteRunning> {
    let Some(old) = data_dir.parent().map(|parent| parent.join("celeste")) else {
        return Ok(false);
    };
    if !old.is_dir() || data_dir.exists() {
        return Ok(false);
    }
    if holds_instance_lock(&old) {
        return Err(CelesteRunning);
    }
    match std::fs::rename(&old, data_dir) {
        Ok(()) => {
            eprintln!("syncative: moved {} to {}", old.display(), data_dir.display());
            Ok(true)
        }
        Err(err) => {
            eprintln!("syncative: couldn't move {} to {}: {err}", old.display(), data_dir.display());
            Ok(false)
        }
    }
}

/// Remove the old cache dir, which only held a copy of the icon.
pub fn remove_cache_dir() {
    let cache = match std::env::var_os("XDG_CACHE_HOME") {
        Some(path) if !path.is_empty() => std::path::PathBuf::from(path),
        _ => Path::new(&std::env::var_os("HOME").unwrap_or_default()).join(".cache"),
    };
    let _ = std::fs::remove_dir_all(cache.join("celeste"));
}

/// Whether another process holds the exclusive lock that `single_instance::acquire` takes on `<dir>/instance.lock`.
fn holds_instance_lock(dir: &Path) -> bool {
    let Ok(lock) = File::options().write(true).open(dir.join("instance.lock")) else {
        return false;
    };
    // SAFETY: plain syscall on a valid, owned fd; closing the file releases a lock taken here.
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
}

/// Remove the autostart entry `celeste.desktop`; switched off there, Syncative's own entry starts switched off too. An AppImage updated in place keeps its path, so both entries would start it at login, and the second start would open the window.
#[cfg(not(target_os = "android"))]
pub fn move_autostart_entry() {
    let new = crate::services::autostart::entry_path();
    move_autostart_entry_at(&new.with_file_name("celeste.desktop"), &new);
}

#[cfg(not(target_os = "android"))]
fn move_autostart_entry_at(old: &Path, new: &Path) {
    use crate::services::autostart;
    if !old.exists() {
        return;
    }
    if !new.exists() && !autostart::enabled_at(old) {
        if let Err(err) = autostart::write_at(new, false, &autostart::exec_command()) {
            eprintln!("syncative: couldn't write {}: {err}", new.display());
        }
    }
    if let Err(err) = std::fs::remove_file(old) {
        eprintln!("syncative: couldn't remove {}: {err}", old.display());
    }
}

/// The value of `account` under the old service, moved to the current one. `Ok(None)` when there is none.
#[cfg(not(target_os = "android"))]
pub fn take_celeste_key(account: &str) -> Result<Option<String>, String> {
    let old = keyring::Entry::new(CELESTE_SERVICE, account).map_err(|e| format!("keyring entry init failed: {e}"))?;
    let value = match old.get_password() {
        Ok(value) => value,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(e) => return Err(format!("keyring read failed: {e}")),
    };
    crate::services::secrets::store(account, &value)?;
    if let Err(err) = old.delete_credential() {
        eprintln!("syncative: moved the keyring entry {account}, but couldn't delete it under {CELESTE_SERVICE}: {err}");
    }
    eprintln!("syncative: moved the keyring entry {account} from {CELESTE_SERVICE}");
    Ok(Some(value))
}

/// Remove `account` under the old service, if it is there.
#[cfg(not(target_os = "android"))]
pub fn delete_celeste_key(account: &str) {
    if let Ok(old) = keyring::Entry::new(CELESTE_SERVICE, account) {
        let _ = old.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("syncative-rename-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn the_old_data_dir_moves() {
        let root = temp_root("move");
        std::fs::create_dir(root.join("celeste")).unwrap();
        std::fs::write(root.join("celeste/data.sqlite"), "db").unwrap();
        assert_eq!(move_data_dir(&root.join("syncative")).ok(), Some(true));
        assert_eq!(std::fs::read_to_string(root.join("syncative/data.sqlite")).unwrap(), "db");
        assert!(!root.join("celeste").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_existing_data_dir_is_kept() {
        let root = temp_root("keep");
        std::fs::create_dir(root.join("celeste")).unwrap();
        std::fs::create_dir(root.join("syncative")).unwrap();
        std::fs::write(root.join("syncative/data.sqlite"), "new").unwrap();
        assert_eq!(move_data_dir(&root.join("syncative")).ok(), Some(false));
        assert_eq!(std::fs::read_to_string(root.join("syncative/data.sqlite")).unwrap(), "new");
        assert!(root.join("celeste").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_old_autostart_entry_goes_and_passes_on_off() {
        use crate::services::autostart::{enabled_at, write_at};
        let root = temp_root("autostart");
        let (old, new) = (root.join("celeste.desktop"), root.join("syncative.desktop"));
        write_at(&old, false, "celeste").unwrap();
        move_autostart_entry_at(&old, &new);
        assert!(!old.exists());
        assert!(new.exists() && !enabled_at(&new));

        std::fs::remove_file(&new).unwrap();
        write_at(&old, true, "celeste").unwrap();
        move_autostart_entry_at(&old, &new);
        assert!(!old.exists());
        // Left to `autostart::refresh`, which creates the entry switched on.
        assert!(!new.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_running_celeste_blocks_the_move() {
        let root = temp_root("running");
        std::fs::create_dir(root.join("celeste")).unwrap();
        let lock = File::options().create(true).truncate(false).write(true).open(root.join("celeste/instance.lock")).unwrap();
        // flock locks belong to the open file description, so a second open in this process conflicts like another process would.
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }, 0);
        assert!(move_data_dir(&root.join("syncative")).is_err());
        assert!(!root.join("syncative").exists());
        drop(lock);
        assert_eq!(move_data_dir(&root.join("syncative")).ok(), Some(true));
        assert!(root.join("syncative").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
