//! Folder and file choosers on Android: the system's document picker (`ACTION_OPEN_DOCUMENT_TREE`, turned into a file path) and its open and save dialogs. Blocking — call from `spawn_blocking`.

/// Ask the user for a directory. `None` when cancelled or when the picker's choice has no file path.
pub fn pick_folder(_title: &str) -> Option<String> {
    super::android::folders::pick()
}

/// Ask where to save a file, suggesting `name`, and write `contents` there. `Ok(false)` when cancelled.
pub fn save_file(_title: &str, name: &str, contents: &[u8]) -> Result<bool, String> {
    super::android::documents::save(name, contents)
}

/// Ask for a file and read it. `Ok(None)` when cancelled.
pub fn open_file(_title: &str) -> Result<Option<Vec<u8>>, String> {
    super::android::documents::open()
}
