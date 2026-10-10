//! The detailed log: whether every captured line goes to the system log (the journal, logcat) or only problems. Kept as the file `detailed-log` in the data dir; present means on.

use std::{io, path::PathBuf};

use crate::infrastructure::stderr_capture;

fn flag_path() -> PathBuf {
    crate::util::get_data_dir().join("detailed-log")
}

pub fn detailed_log() -> bool {
    flag_path().exists()
}

/// Switch the detailed log, for now and later runs.
pub fn set_detailed_log(on: bool) -> io::Result<()> {
    let path = flag_path();
    if on {
        std::fs::write(&path, "")?;
    } else {
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    stderr_capture::set_detailed(on);
    apply_to_rclone();
    Ok(())
}

/// Applies the saved switch; call once the capture is installed.
pub fn apply() {
    stderr_capture::set_detailed(detailed_log());
}

/// rclone names every file it copies or deletes from level INFO on; by default it only reports problems (NOTICE). Call once rclone is initialised.
pub fn apply_to_rclone() {
    let level = if detailed_log() { "INFO" } else { "NOTICE" };
    if let Err(err) = celeste_go::rpc("options/set", serde_json::json!({ "main": { "LogLevel": level } }).to_string()) {
        eprintln!("celeste: could not set rclone's log level: {err}");
    }
}
