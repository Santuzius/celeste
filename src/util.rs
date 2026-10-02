use std::path::PathBuf;

use futures::future::Future;

/// Block the current thread on a future. Safe from any thread;
/// `block_on` has no main-context requirements.
pub fn await_future<F: Future>(future: F) -> F::Output {
    futures::executor::block_on(future)
}

/// The user's real home directory, as the user knows it. Inside a snap `$HOME` points to the snap's private data directory, the real one is in `$SNAP_REAL_HOME`; data files stay under `$HOME` either way.
pub fn user_home() -> Option<String> {
    std::env::var("SNAP_REAL_HOME").or_else(|_| std::env::var("HOME")).ok().filter(|h| !h.is_empty())
}

/// Replace the user's home-directory prefix with `~` for friendlier UI
/// paths. Falls back to the raw path if the home directory is unknown.
pub fn fmt_home(dir: &str) -> String {
    let Some(home) = user_home() else {
        return dir.to_string();
    };
    match dir.strip_prefix(&home) {
        Some(rest) => "~".to_string() + rest,
        None => dir.to_string(),
    }
}

/// `${XDG_DATA_HOME:-$HOME/.local/share}/celeste`.
///
/// We treat the celeste config dir as user data (the SQLite DB and
/// rclone config file count as state, not human-edited config), so
/// XDG-wise it belongs under `$XDG_DATA_HOME`, not `$XDG_CONFIG_HOME`.
pub fn get_data_dir() -> PathBuf {
    let mut base = match std::env::var_os("XDG_DATA_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            let mut home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            home.push(".local");
            home.push("share");
            home
        }
    };
    base.push("celeste");
    base
}

/// Legacy `${XDG_CONFIG_HOME:-$HOME/.config}/celeste` location.
///
/// Older builds wrote the SQLite DB, rclone config, and proton-session
/// blobs here. Startup migrates anything still present into the new
/// data dir / keyring; nothing else should reach for this path.
pub fn get_legacy_config_dir() -> PathBuf {
    let mut base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => {
            let mut home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            home.push(".config");
            home
        }
    };
    base.push("celeste");
    base
}

/// `$XDG_RUNTIME_DIR/celeste`, or `None` when the variable is unset.
///
/// `$XDG_RUNTIME_DIR` is a per-session tmpfs (RAM-backed, wiped on
/// logout / reboot). It's where the on-disk `rclone.conf` lives at
/// runtime so the OAuth tokens librclone needs to read never reach
/// rotational storage; the keyring holds the durable copy and the
/// runtime file is regenerated each session.
pub fn get_runtime_dir() -> Option<PathBuf> {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(path) if !path.is_empty() => {
            let mut base = PathBuf::from(path);
            base.push("celeste");
            Some(base)
        }
        _ => None,
    }
}

/// Trim at most one leading and one trailing slash.
pub fn strip_slashes(string: &str) -> String {
    let stripped_prefix = string.strip_prefix('/').unwrap_or(string);
    stripped_prefix
        .strip_suffix('/')
        .unwrap_or(stripped_prefix)
        .to_string()
}
