//! Account-provisioning workflows. Per-backend submodules:
//!
//! - [`webdav`] — Generic WebDAV / Nextcloud / Owncloud (raw username +
//!   password; just a config/create + DB insert).
//! - [`oauth`] — Dropbox / Google Drive / pCloud (shells out to
//!   `rclone authorize`, which opens the default browser, runs its own
//!   OAuth callback listener, and prints the token JSON).
//! - [`proton`] — Proton Drive (username + password + optional 2FA, no
//!   browser step; native Go client, no rclone in the path).

pub mod oauth;
pub mod proton;
pub mod webdav;

// Public re-exports so call sites at `crate::services::auth::add_webdav_remote`
// keep working without forcing every caller to spell out the submodule.
pub use oauth::{add_oauth_remote, reauth_oauth_remote, OAuthProvider};
pub use proton::{
    add_proton_drive_remote, forget_session as forget_proton_session,
    persist_session_if_rotated as persist_proton_session_if_rotated, reauth_proton_drive_remote,
    resume_session_from_keyring as resume_proton_session,
};
pub use webdav::{add_webdav_remote, WebDavVendor};
