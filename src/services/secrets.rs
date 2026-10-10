//! OS keyring storage for credential blobs.
//!
//! All entries live under the `Celeste Keys` service so they group
//! together in Seahorse / KWallet / GNOME Keyring under one heading.
//! Two kinds of blob are stored:
//!
//! - one entry per native-Proton remote, keyed `proton-session-<name>`,
//!   holding the JSON `ReusableCredential` blob the Go side reads back
//!   on resume;
//! - one entry keyed `rclone-config`, holding the raw text of
//!   `rclone.conf` so its OAuth / WebDAV credentials never sit on disk
//!   in plaintext between runs.
//!
//! The entry interaction is intentionally narrow: `store`, `load` (None
//! when missing), `delete` (idempotent). Higher layers convert between
//! these and tempfiles where the Go FFI / librclone insists on a path.
//!
//! Android has no Secret Service; there each entry is a file in the app's private storage, encrypted with a key held by the Android Keystore (see `infrastructure::android::secrets`).

#[cfg(not(target_os = "android"))]
use keyring::Entry;

/// Service name shown in password-manager UIs. All Celeste keyring
/// entries share this so they appear as one group.
pub const SERVICE: &str = "Celeste Keys";

/// Account name for the rclone config blob.
pub const RCLONE_ACCOUNT: &str = "rclone-config";

/// Account name for the password rclone encrypts its config file with.
pub const RCLONE_PASSWORD_ACCOUNT: &str = "rclone-config-password";

/// Account name for a native-Proton remote's session blob.
pub fn proton_account(remote_name: &str) -> String {
    format!("proton-session-{remote_name}")
}

#[cfg(not(target_os = "android"))]
fn entry(account: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, account).map_err(|e| format!("keyring entry init failed: {e}"))
}

#[cfg(target_os = "android")]
pub use crate::infrastructure::android::secrets::{delete, load, store};

/// Persist `value` under `account`, overwriting any existing entry.
#[cfg(not(target_os = "android"))]
pub fn store(account: &str, value: &str) -> Result<(), String> {
    entry(account)?
        .set_password(value)
        .map_err(|e| format!("keyring write failed: {e}"))
}

/// Read `account`. Returns `Ok(None)` when no entry exists yet (the
/// caller treats that as "first run"); other errors propagate.
#[cfg(not(target_os = "android"))]
pub fn load(account: &str) -> Result<Option<String>, String> {
    match entry(account)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring read failed: {e}")),
    }
}

/// Remove `account`. Treats "no such entry" as success so the
/// remote-removal flow doesn't have to special-case never-saved remotes.
#[cfg(not(target_os = "android"))]
pub fn delete(account: &str) -> Result<(), String> {
    match entry(account)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete failed: {e}")),
    }
}
