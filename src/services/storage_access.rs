//! Android's "All files access", without which Celeste can read only the files it created itself in shared storage. Always granted on the desktop.

/// Whether Celeste may read and write the folders it syncs.
pub fn granted() -> bool {
    #[cfg(target_os = "android")]
    return crate::infrastructure::android::has_storage_access();
    #[cfg(not(target_os = "android"))]
    true
}

/// Opens Android's setting for All files access.
pub fn ask() {
    #[cfg(target_os = "android")]
    crate::infrastructure::android::ask_for_storage_access();
}
