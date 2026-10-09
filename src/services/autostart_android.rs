//! Start Celeste's sync service when the phone has booted. The setting is the file `autostart-off` in the data dir: missing means on. The app's boot receiver (android/…/BootReceiver.java) checks for the same file.

use std::{io, path::PathBuf};

/// Where the switch lives; shown when changing it fails.
pub fn entry_path() -> PathBuf {
    crate::util::get_data_dir().join("autostart-off")
}

/// Whether Celeste starts after booting.
pub fn enabled() -> bool {
    !entry_path().exists()
}

/// Switch autostart on or off.
pub fn set(enabled: bool) -> io::Result<()> {
    let path = entry_path();
    if enabled {
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        }
    } else {
        std::fs::write(&path, "")
    }
}

/// Nothing to keep current on Android; the boot receiver is part of the app.
pub fn refresh() {}
