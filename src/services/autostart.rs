//! Start Syncative in the tray when the desktop session starts, through an XDG autostart entry in `${XDG_CONFIG_HOME:-~/.config}/autostart/syncative.desktop`.
//!
//! The entry itself is the setting: missing means "never chosen" and counts as on, so the first start creates it; switching off rewrites it with `Hidden=true` instead of deleting it. The file name matches the system-wide entry of the NixOS module (`/etc/xdg/autostart/syncative.desktop`), which a user entry of the same name overrides. In the Snap, `XDG_CONFIG_HOME` lies inside `~/snap/syncative/current`, where snapd's `autostart:` looks for it.

use std::{
    io,
    path::{Path, PathBuf},
};

const FILE_NAME: &str = "syncative.desktop";

/// Where the entry lives.
pub fn entry_path() -> PathBuf {
    let mut base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    base.push("autostart");
    base.push(FILE_NAME);
    base
}

/// Whether Syncative starts at login.
pub fn enabled() -> bool {
    enabled_at(&entry_path())
}

/// Switch autostart on or off.
pub fn set(enabled: bool) -> io::Result<()> {
    write_at(&entry_path(), enabled, &exec_command())
}

/// At startup: create the entry the first time (on by default) and keep its command current, e.g. after the binary moved.
pub fn refresh() {
    let path = entry_path();
    if enabled_at(&path)
        && let Err(err) = write_at(&path, true, &exec_command())
    {
        eprintln!("autostart: could not write {}: {err}", path.display());
    }
}

fn enabled_at(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return true;
    };
    !content.lines().map(str::trim).any(|line| line.eq_ignore_ascii_case("Hidden=true") || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false"))
}

fn write_at(path: &Path, enabled: bool, exec: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Syncative\nComment=Sync local folders with Google Drive or Proton Drive\nExec={exec}\nIcon=syncative-icon\nTerminal=false\nHidden={hidden}\nX-GNOME-Autostart-enabled={enabled}\n",
        hidden = !enabled,
    );
    // Write next to it and rename, so a crash never leaves a half-written entry that the desktop ignores.
    let tmp = path.with_extension("desktop.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// `syncative` when that name on `PATH` is this very binary (installed packages, the Snap), so the entry survives updates; the `.AppImage` file for an AppImage, whose binary lies in a mount point that changes with every start; otherwise the absolute path, e.g. for a development build.
fn exec_command() -> String {
    if std::env::var_os("SNAP").is_some() {
        return "syncative".to_owned();
    }
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|path| !path.is_empty()) {
        return quote(&appimage.to_string_lossy());
    }
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return "syncative".to_owned();
    };
    let exe = nix_wrapper(&exe).unwrap_or(exe);
    let on_path = std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("syncative").canonicalize().is_ok_and(|p| p == exe)))
        .unwrap_or(false);
    if on_path { "syncative".to_owned() } else { quote(&exe.to_string_lossy()) }
}

/// The script `bin/syncative` that Nix's `wrapProgram` puts in front of the real binary `bin/.syncative-wrapped`. Only the script sets `LD_LIBRARY_PATH`, so started directly the binary finds no Wayland library and panics.
fn nix_wrapper(exe: &Path) -> Option<PathBuf> {
    let name = exe.file_name()?.to_str()?.strip_prefix('.')?.strip_suffix("-wrapped")?;
    let wrapper = exe.with_file_name(name);
    wrapper.is_file().then_some(wrapper)
}

/// Quote an `Exec` argument as the Desktop Entry spec asks when it contains anything but plain path characters.
fn quote(arg: &str) -> String {
    if arg.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_entry(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("syncative-autostart-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("autostart").join(FILE_NAME)
    }

    #[test]
    fn missing_entry_counts_as_on() {
        assert!(enabled_at(&temp_entry("missing")));
    }

    #[test]
    fn switching_off_keeps_a_hidden_entry() {
        let path = temp_entry("toggle");
        write_at(&path, false, "syncative").unwrap();
        assert!(!enabled_at(&path));
        assert!(std::fs::read_to_string(&path).unwrap().contains("Hidden=true"));
        write_at(&path, true, "syncative").unwrap();
        assert!(enabled_at(&path));
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn nix_binary_is_replaced_by_its_wrapper() {
        let dir = std::env::temp_dir().join(format!("syncative-autostart-wrapper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join(".syncative-wrapped");
        assert_eq!(nix_wrapper(&binary), None);
        std::fs::write(dir.join("syncative"), "").unwrap();
        assert_eq!(nix_wrapper(&binary), Some(dir.join("syncative")));
        assert_eq!(nix_wrapper(&dir.join("syncative")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exec_paths_with_spaces_are_quoted() {
        assert_eq!(quote("/usr/bin/syncative"), "/usr/bin/syncative");
        assert_eq!(quote("/home/a b/syncative"), "\"/home/a b/syncative\"");
    }
}
