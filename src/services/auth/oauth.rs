//! OAuth2 remote provisioning via rclone's `authorize` flow, run in-process (Dropbox, Google Drive, pCloud).

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};

use serde_json::json;

use crate::domain::{
    ports::{BackendClient, Repository},
    remote::RemoteId,
};
use crate::util;

/// OAuth providers that use `rclone authorize` for token capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthProvider {
    Dropbox,
    GDrive,
    PCloud,
}

impl OAuthProvider {
    pub(super) fn rclone_type(self) -> &'static str {
        match self {
            OAuthProvider::Dropbox => "dropbox",
            OAuthProvider::GDrive => "drive",
            OAuthProvider::PCloud => "pcloud",
        }
    }
}

/// Shared between a running authorization and the UI: the UI can cancel it and shows the authorization link, so the user can open it in a browser of their choice when the default one doesn't work.
#[derive(Debug, Default)]
pub struct AuthorizeHandle {
    cancelled: AtomicBool,
    url: Mutex<Option<String>>,
}

impl AuthorizeHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// The local authorization link, once rclone has made it.
    pub fn url(&self) -> Option<String> {
        self.url.lock().ok()?.clone()
    }
}

/// Error returned when the user cancelled the browser flow.
pub const AUTHORIZE_CANCELLED: &str = "Authorization cancelled.";

/// OAuth2 like `rclone authorize`. Blocks until the user finishes the
/// browser flow or cancels it through `handle`; meant to run on a
/// blocking tokio task. `client_id`/`client_secret` override rclone's
/// built-in defaults.
pub fn add_oauth_remote(
    name: &str,
    provider: OAuthProvider,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    handle: &AuthorizeHandle,
    repo: &dyn Repository,
    client: &dyn BackendClient,
) -> Result<RemoteId, String> {
    let token = run_rclone_authorize(provider, client_id, client_secret, handle)?;

    let payload = json!({
        "name": name,
        "parameters": {
            "client_id": client_id.unwrap_or_default(),
            "client_secret": client_secret.unwrap_or_default(),
            "token": token,
            "config_refresh_token": false,
        },
        "type": provider.rclone_type(),
    })
    .to_string();

    client.create_config(payload)?;
    util::await_future(repo.insert_remote(name.to_owned()))
        .map_err(|e| e.to_string())
}

/// Reauthenticate an existing OAuth-backed remote (Dropbox / Google
/// Drive / pCloud). Re-runs the authorization for fresh tokens, then
/// replaces the rclone config entry under the same name so the new
/// token takes effect. Leaves the DB row untouched — same id, same
/// name, same sync_dirs — so the user's configuration survives the
/// reauth unchanged.
pub fn reauth_oauth_remote(
    name: &str,
    provider: OAuthProvider,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    handle: &AuthorizeHandle,
    client: &dyn BackendClient,
) -> Result<(), String> {
    let token = run_rclone_authorize(provider, client_id, client_secret, handle)?;

    let payload = json!({
        "name": name,
        "parameters": {
            "client_id": client_id.unwrap_or_default(),
            "client_secret": client_secret.unwrap_or_default(),
            "token": token,
            "config_refresh_token": false,
        },
        "type": provider.rclone_type(),
    })
    .to_string();

    // rclone's `config/create` rejects a name that already exists, so
    // drop the stale entry first. Failure to delete is non-fatal — the
    // create step will surface the real error if anything's wrong.
    let _ = client.delete_config(name);
    client.create_config(payload)
}

fn run_rclone_authorize(
    provider: OAuthProvider,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    handle: &AuthorizeHandle,
) -> Result<String, String> {
    let (client_id, client_secret) = match (client_id, client_secret) {
        (Some(id), Some(secret)) if !id.is_empty() && !secret.is_empty() => (id, secret),
        _ => ("", ""),
    };
    // rclone's flow runs in-process (there is no rclone binary on Android) and waits for the browser on its own thread; this one opens the link and passes on a cancel.
    std::thread::scope(|scope| {
        let flow = scope.spawn(|| celeste_go::authorize(provider.rclone_type(), client_id, client_secret));
        let mut cancelled = false;
        while !flow.is_finished() {
            if handle.cancelled.load(Ordering::Acquire) {
                // Repeated until it lands: rclone may not be listening yet.
                celeste_go::authorize_cancel();
                cancelled = true;
            } else if handle.url().is_none()
                && let Some(url) = celeste_go::authorize_url()
            {
                util::open_in_browser(&url);
                if let Ok(mut slot) = handle.url.lock() {
                    *slot = Some(url);
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let token = flow.join().unwrap_or_else(|_| Err("rclone's authorization crashed".to_owned()));
        if cancelled { Err(AUTHORIZE_CANCELLED.to_owned()) } else { token }
    })
}
