//! Editor and toolchain temp-file patterns that should never touch the
//! remote. Syncing these is always wrong: the file lives for seconds,
//! then the editor deletes it, and the sync layer ends up with half-
//! uploaded files, spurious "object not found" errors, and DB rows
//! that provoke later mirror-delete passes.

pub fn is_editor_temp(name: &str) -> bool {
    if name.ends_with(".kate-swp") {
        return true;
    }
    if name.starts_with('.')
        && (name.ends_with(".swp") || name.ends_with(".swo") || name.ends_with(".swn"))
    {
        return true;
    }
    if name.starts_with(".#") {
        return true;
    }
    if name.starts_with('#') && name.ends_with('#') {
        return true;
    }
    if name.ends_with('~') {
        return true;
    }
    if name.starts_with(".goutputstream-") {
        return true;
    }
    // `.partial`: rclone's download in progress; `.part` also Celeste's own for Proton (src/go/drive/download.go).
    if name.ends_with(".crdownload") || name.ends_with(".part") || name.ends_with(".partial") {
        return true;
    }
    false
}

/// A Proton download in progress, named by `os.CreateTemp(dir, ".celeste-*.part")` in src/go/drive/download.go.
pub fn is_celeste_part(name: &str) -> bool {
    name.starts_with(".celeste-") && name.ends_with(".part")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_offenders_match() {
        assert!(is_editor_temp(".foo.kate-swp"));
        assert!(is_editor_temp(".bar.swp"));
        assert!(is_editor_temp(".bar.swo"));
        assert!(is_editor_temp(".bar.swn"));
        assert!(is_editor_temp(".#baz"));
        assert!(is_editor_temp("#baz#"));
        assert!(is_editor_temp("baz~"));
        assert!(is_editor_temp(".goutputstream-abc"));
        assert!(is_editor_temp("file.crdownload"));
        assert!(is_editor_temp("file.part"));
        assert!(is_editor_temp("file.pdf.4f3a2b1c.partial"));
        assert!(is_editor_temp(".celeste-123456.part"));
    }

    #[test]
    fn only_celestes_own_part_files_count_as_unfinished_downloads() {
        assert!(is_celeste_part(".celeste-123456.part"));
        assert!(!is_celeste_part("video.part"));
        assert!(!is_celeste_part(".celeste-notes.txt"));
    }

    #[test]
    fn real_files_dont_match() {
        assert!(!is_editor_temp("real-file.txt"));
        assert!(!is_editor_temp("swapfile"));
        assert!(!is_editor_temp(".hidden"));
    }
}
