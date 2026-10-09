//! Folder chooser on Android: the system's document picker (`ACTION_OPEN_DOCUMENT_TREE`), turned into a file path. Blocking — call from `spawn_blocking`.

/// Ask the user for a directory. `None` when cancelled or when the picker's choice has no file path.
pub fn pick_folder(_title: &str) -> Option<String> {
    super::android::folders::pick()
}
