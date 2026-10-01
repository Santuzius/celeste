//! OAuth2 remote provisioning via `rclone authorize` (Dropbox, Google Drive, pCloud).

use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
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

/// Shared between a running `rclone authorize` and the UI: the UI can
/// cancel it and shows the authorization link rclone prints, so the
/// user can open it in a browser of their choice when the default one
/// doesn't work.
#[derive(Debug, Default)]
pub struct AuthorizeHandle {
    cancelled: AtomicBool,
    url: Mutex<Option<String>>,
}

impl AuthorizeHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// The local authorization link, once rclone has printed it.
    pub fn url(&self) -> Option<String> {
        self.url.lock().ok()?.clone()
    }
}

/// Error returned when the user cancelled the browser flow.
pub const AUTHORIZE_CANCELLED: &str = "Authorization cancelled.";

/// OAuth2 via `rclone authorize`. Blocks until the user finishes the
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
/// Drive / pCloud). Re-runs `rclone authorize` for fresh tokens, then
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
    let mut cmd = Command::new("rclone");
    cmd.arg("authorize").arg(provider.rclone_type());
    if let (Some(id), Some(secret)) = (client_id, client_secret) {
        if !id.is_empty() && !secret.is_empty() {
            cmd.arg(id).arg(secret);
        }
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run `rclone authorize`: {e}"))?;

    // rclone logs the local auth link on stderr ("…go to the following
    // link: http://127.0.0.1:53682/auth?state=…"); pick it up for the
    // UI while keeping the rest for the error message.
    let stderr = child.stderr.take().expect("piped stderr");
    let stdout = child.stdout.take().expect("piped stdout");
    let token_output = std::thread::scope(|scope| {
        let stderr_reader = scope.spawn(|| {
            let mut text = String::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Some(url) = auth_link(&line)
                    && let Ok(mut slot) = handle.url.lock()
                {
                    *slot = Some(url);
                }
                text.push_str(&line);
                text.push('\n');
            }
            text
        });
        let stdout_reader = scope.spawn(|| {
            let mut text = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut text);
            text
        });

        // Wait for rclone, but give up promptly when the user cancels —
        // otherwise `rclone authorize` waits for a browser callback
        // forever.
        let status = loop {
            if handle.cancelled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AUTHORIZE_CANCELLED.to_owned());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(e) => return Err(format!("couldn't wait for `rclone authorize`: {e}")),
            }
        };
        let stdout_text = stdout_reader.join().unwrap_or_default();
        let stderr_text = stderr_reader.join().unwrap_or_default();
        if !status.success() {
            return Err(format!("rclone authorize exited with {status}: {}", stderr_text.trim()));
        }
        Ok(stdout_text)
    })?;
    extract_token(&token_output)
}

/// The local authorization URL in one of rclone's log lines, if any.
fn auth_link(line: &str) -> Option<String> {
    let start = line.find("http://127.0.0.1:")?;
    Some(line[start..].split_whitespace().next()?.to_owned())
}

/// Pull the token out of rclone's `authorize` stdout. rclone prints
/// a banner around the token (either a JSON object or a base-hex blob)
/// so we scan for the first line that looks like one.
fn extract_token(stdout: &str) -> Result<String, String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            return Ok(trimmed.to_owned());
        }
    }
    Err(format!(
        "couldn't parse rclone authorize output (expected a JSON token line). Raw:\n{}",
        stdout.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::auth_link;

    #[test]
    fn finds_rclone_auth_link() {
        let line = "2026/10/01 15:00:00 NOTICE: If your browser doesn't open automatically go to the following link: http://127.0.0.1:53682/auth?state=abc";
        assert_eq!(auth_link(line).as_deref(), Some("http://127.0.0.1:53682/auth?state=abc"));
        assert_eq!(auth_link("NOTICE: Waiting for code..."), None);
    }
}
