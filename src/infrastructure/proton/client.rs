//! [`BackendClient`] implementation that talks directly to Proton via
//! the native Go archive. The sync engine sees this behind
//! `&dyn BackendClient` and doesn't know anything has changed; all
//! it ever passes are string paths, which this client translates to
//! ProtonDrive link IDs by walking the tree from the session's root.
//!
//! Scope of this initial impl:
//!   - One remote per Celeste instance. `remote` arg is accepted but only
//!     logged — every call routes to the single native session this client was
//!     constructed with.
//!   - No cache. Each call walks root → target, listing each intermediate
//!     folder. First-pass good-enough for a known small sync tree; later
//!     optimise with a linkID cache keyed by remote_path.
//!   - `ListFilter` is honoured (`All` / `Dirs` / `Files`). `recursive` is
//!     implemented by in-Rust recursion over the native API's non-recursive
//!     listings — the FFI surface doesn't yet carry a `recursive` flag.
//!   - `copy_to_remote` needs a parent link ID + basename; we split
//!     `remote_path` on the trailing slash and resolve the parent portion.
//!   - `delete_config` is a no-op at this layer — the session blob is owned by
//!     the higher-level auth flow that constructed us.
//!   - `create_config` is unused; the native path takes a typed credential blob
//!     rather than an rclone JSON body.
//!   - `remote_type` returns the fixed string `"native-proton"`.

use std::path::Path;

use time::OffsetDateTime;

use celeste_go::proton as proton_ffi;

use crate::domain::{
    ports::{BackendClient, Cancel, is_cancelled},
    sync::{ListFilter, RemoteItem},
};

/// Native ProtonDrive adapter. Owns the session UID the native-go
/// layer returns from `ProtonDrive_Login` / `ProtonDrive_ResumeSession`.
#[derive(Clone, Debug)]
pub struct NativeProtonClient {
    uid: String,
}

impl NativeProtonClient {
    /// Wrap a UID the caller already got from a `login` / `resume`
    /// call. The session must be registered on the Go side.
    pub fn new(uid: String) -> Self {
        Self { uid }
    }

    pub fn uid(&self) -> &str {
        &self.uid
    }

    /// Resolve a Proton-relative path (e.g. `"Foo/bar.txt"`,
    /// possibly empty for the root) to its link ID. Walks from the
    /// session root, listing each intermediate folder and matching
    /// by decrypted `Entry.name`. Returns `Ok(None)` when any
    /// segment is absent.
    ///
    /// Checks `cancel` between each `list_directory` FFI call so a
    /// deep path (many nested folders) doesn't block past a cancel
    /// request indefinitely.
    fn resolve_path(&self, path: &str, cancel: &Cancel) -> Result<Option<String>, String> {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Ok(Some(proton_ffi::root_link_id(&self.uid)?));
        }
        let mut current = proton_ffi::root_link_id(&self.uid)?;
        for segment in trimmed.split('/') {
            if is_cancelled(cancel) {
                return Err("cancelled".to_owned());
            }
            let entries = proton_ffi::list_directory(&self.uid, &current)?;
            match entries.into_iter().find(|e| e.name == segment) {
                Some(entry) => current = entry.link_id,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// Resolve a path to `(parent_link_id, basename)` — needed for
    /// create / upload / mkdir where we only have the full remote
    /// path. An empty path resolves to (root, "") which is invalid
    /// for those callers; surface as an error.
    fn resolve_parent(
        &self,
        path: &str,
        cancel: &Cancel,
    ) -> Result<(String, String), String> {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Err("cannot operate on root itself".to_owned());
        }
        let (parent_path, basename) = match trimmed.rsplit_once('/') {
            Some((p, b)) => (p, b),
            None => ("", trimmed),
        };
        let parent_id = match self.resolve_path(parent_path, cancel)? {
            Some(id) => id,
            None => return Err(format!("parent path '{parent_path}' not found")),
        };
        Ok((parent_id, basename.to_owned()))
    }
}

fn entry_to_remote_item(entry: proton_ffi::Entry, path_prefix: &str) -> RemoteItem {
    let full_path = if path_prefix.is_empty() {
        entry.name.clone()
    } else {
        format!("{path_prefix}/{}", entry.name)
    };
    RemoteItem {
        is_dir: entry.is_dir,
        path: full_path,
        name: entry.name,
        mod_time: OffsetDateTime::from_unix_timestamp(entry.mod_time_unix)
            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH),
    }
}

impl BackendClient for NativeProtonClient {
    fn stat(
        &self,
        _remote: &str,
        path: &str,
        cancel: &Cancel,
    ) -> Result<Option<RemoteItem>, String> {
        let Some(link_id) = self.resolve_path(path, cancel)? else {
            return Ok(None);
        };
        let Some(entry) = proton_ffi::stat(&self.uid, &link_id)? else {
            return Ok(None);
        };
        // Bridge leaves the root's name empty; callers pass the
        // requested path so we preserve that in the returned item.
        let name = if entry.name.is_empty() {
            path.trim_matches('/')
                .rsplit_once('/')
                .map_or(path.trim_matches('/').to_owned(), |(_, base)| {
                    base.to_owned()
                })
        } else {
            entry.name.clone()
        };
        Ok(Some(RemoteItem {
            is_dir: entry.is_dir,
            path: path.trim_matches('/').to_owned(),
            name,
            mod_time: OffsetDateTime::from_unix_timestamp(entry.mod_time_unix)
                .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH),
        }))
    }

    fn list(
        &self,
        _remote: &str,
        path: &str,
        recursive: bool,
        filter: ListFilter,
        cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        fn keep(filter: ListFilter, is_dir: bool) -> bool {
            match filter {
                ListFilter::All => true,
                ListFilter::Dirs => is_dir,
                ListFilter::Files => !is_dir,
            }
        }
        let trimmed = path.trim_matches('/').to_owned();
        let Some(link_id) = self.resolve_path(&trimmed, cancel)? else {
            return Ok(Vec::new());
        };

        if recursive {
            // Fast path: single FFI call, Go side parallelises the API calls.
            let entries = proton_ffi::list_recursive(&self.uid, &link_id)?;
            let out = entries
                .into_iter()
                .filter(|e| keep(filter, e.is_dir))
                .map(|e| {
                    // e.name is the full relative path from the listed root.
                    // Build the absolute remote path and extract the basename.
                    let full_path = if trimmed.is_empty() {
                        e.name.clone()
                    } else {
                        format!("{trimmed}/{}", e.name)
                    };
                    let basename = e
                        .name
                        .rsplit_once('/')
                        .map_or(e.name.clone(), |(_, b)| b.to_owned());
                    RemoteItem {
                        is_dir: e.is_dir,
                        path: full_path,
                        name: basename,
                        mod_time: OffsetDateTime::from_unix_timestamp(e.mod_time_unix)
                            .unwrap_or_else(|_| OffsetDateTime::UNIX_EPOCH),
                    }
                })
                .collect();
            return Ok(out);
        }

        // Non-recursive: list just one level.
        let entries = proton_ffi::list_directory(&self.uid, &link_id)?;
        let out = entries
            .into_iter()
            .filter(|e| keep(filter, e.is_dir))
            .map(|e| entry_to_remote_item(e, &trimmed))
            .collect();
        Ok(out)
    }

    fn mkdir(&self, _remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        use crate::domain::backend_events::{BackendEvent, EventTranslator, Operation};
        use crate::infrastructure::translators::proton::ProtonTranslator;
        let (parent, name) = self.resolve_parent(path, cancel)?;
        match proton_ffi::create_folder(&self.uid, &parent, &name) {
            Ok(_) => Ok(()),
            Err(e) => match ProtonTranslator.classify(Operation::Mkdir, &e) {
                BackendEvent::AlreadyExists => Ok(()),
                _ => Err(e),
            },
        }
    }

    fn delete_file(&self, _remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        let Some(link_id) = self.resolve_path(path, cancel)? else {
            return Err(format!("path '{path}' not found on remote"));
        };
        proton_ffi::trash_link(&self.uid, &link_id)
    }

    fn purge(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        // Proton's TrashChildren on a folder cascades server-side,
        // so `purge` and `delete_file` collapse to the same call.
        self.delete_file(remote, path, cancel)
    }

    fn copy_to_remote(
        &self,
        local_path: &str,
        _remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        let (parent, name) = self.resolve_parent(remote_path, cancel)?;
        proton_ffi::upload_file(&self.uid, &parent, &name, Path::new(local_path))?;
        Ok(())
    }

    fn copy_to_local(
        &self,
        local_path: &str,
        _remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        let Some(link_id) = self.resolve_path(remote_path, cancel)? else {
            return Err(format!("path '{remote_path}' not found on remote"));
        };
        proton_ffi::download_file(&self.uid, &link_id, Path::new(local_path))
    }

    fn delete_config(&self, _remote: &str) -> Result<(), String> {
        // Session lifecycle is owned by the auth layer that
        // constructed this client — nothing to clean up here.
        Ok(())
    }

    fn create_config(&self, _payload_json: String) -> Result<(), String> {
        Err("native-proton client does not accept rclone-style create_config payloads".to_owned())
    }

    fn remote_type(&self, _remote: &str) -> Result<Option<String>, String> {
        Ok(Some("native-proton".to_owned()))
    }

    /// Re-persist the session blob if the access/refresh tokens rotated
    /// during this pass. The upstream client silently refreshes on a
    /// 401 and Proton issues a new one-time-use refresh token; writing
    /// it back to the keyring here is what lets the session survive a
    /// restart instead of demanding a fresh 2FA login the next day.
    /// `remote` is the keyring account name the client is registered
    /// under. Non-fatal on error: the live session already holds the
    /// new tokens, so we only log and let the next rotation retry.
    fn checkpoint_session(&self, remote: &str) {
        match crate::services::auth::persist_proton_session_if_rotated(remote, &self.uid) {
            Ok(true) => eprintln!(
                "celeste: native-proton session for '{remote}' re-persisted after token rotation.",
            ),
            Ok(false) => {}
            Err(err) => eprintln!(
                "celeste: failed to re-persist rotated proton session for '{remote}': {err}",
            ),
        }
    }
}

/// Adapter for a native-proton remote whose session couldn't be resumed
/// at startup for a *recoverable* reason — no network yet (Celeste
/// autostarting before DNS is up, or a suspend/resume), or a transient
/// keyring failure. Resuming needs an HTTPS round-trip (`GetUser`), so
/// those conditions are indistinguishable from a dead session at the
/// call site unless we classify the error.
///
/// Rather than parking the remote behind a reauth prompt the user can't
/// act on, this retries the resume on demand (rate-limited by
/// [`RESUME_RETRY_COOLDOWN`]) and promotes itself to a live
/// [`NativeProtonClient`] the moment it succeeds. A genuine auth
/// failure discovered on retry latches into the reauth message instead.
pub struct PendingProtonClient {
    remote_name: String,
    state: std::sync::Mutex<PendingState>,
}

/// Minimum gap between resume attempts. A failing pass calls into the
/// client repeatedly; without a cooldown each one would re-dial Proton
/// and re-read the keyring.
const RESUME_RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

enum PendingState {
    /// Not resumed yet. `reason` is what callers see until the next
    /// attempt is due; `last_attempt` is None before the first retry.
    Waiting {
        last_attempt: Option<std::time::Instant>,
        reason: String,
    },
    Live(NativeProtonClient),
}

impl PendingProtonClient {
    /// `reason` is the initial startup failure, surfaced to callers
    /// until the first retry is due.
    pub fn new(remote_name: String, reason: String) -> Self {
        Self {
            remote_name,
            state: std::sync::Mutex::new(PendingState::Waiting {
                last_attempt: None,
                reason,
            }),
        }
    }

    /// Return the live client, attempting a resume if one is due.
    /// Errors carry the reason callers should see. The lock is held
    /// across the resume so concurrent callers don't each open a
    /// duplicate Go-side session.
    fn resolve(&self) -> Result<NativeProtonClient, String> {
        let mut state = self.state.lock().map_err(|_| {
            format!("Proton Drive session state for '{}' is poisoned; restart Celeste.", self.remote_name)
        })?;

        match &*state {
            PendingState::Live(client) => return Ok(client.clone()),
            PendingState::Waiting { last_attempt, reason } => {
                if let Some(at) = last_attempt
                    && at.elapsed() < RESUME_RETRY_COOLDOWN
                {
                    return Err(reason.clone());
                }
            }
        }

        let now = std::time::Instant::now();
        match crate::services::auth::resume_proton_session(&self.remote_name) {
            Ok(Some(cred)) => {
                eprintln!(
                    "celeste: native-proton session for '{}' resumed on retry.",
                    self.remote_name,
                );
                let client = NativeProtonClient::new(cred.uid);
                *state = PendingState::Live(client.clone());
                Ok(client)
            }
            // Blob genuinely absent, or the retry surfaced a real auth
            // failure — no amount of waiting fixes either.
            Ok(None) => {
                let reason = format!(
                    "Proton Drive session for '{}' not found in keyring. Click Reauthenticate on the remote page to log in again.",
                    self.remote_name,
                );
                *state = PendingState::Waiting {
                    last_attempt: Some(now),
                    reason: reason.clone(),
                };
                Err(reason)
            }
            Err(err) if crate::app::is_auth_failure(&err) => {
                let reason = format!(
                    "Proton Drive session for '{}' has expired ({err}). Click Reauthenticate on the remote page to log in again.",
                    self.remote_name,
                );
                *state = PendingState::Waiting {
                    last_attempt: Some(now),
                    reason: reason.clone(),
                };
                Err(reason)
            }
            Err(err) => {
                let reason = format!(
                    "Proton Drive session for '{}' not resumed yet ({err}). Retrying automatically — no action needed.",
                    self.remote_name,
                );
                *state = PendingState::Waiting {
                    last_attempt: Some(now),
                    reason: reason.clone(),
                };
                Err(reason)
            }
        }
    }
}

impl BackendClient for PendingProtonClient {
    fn stat(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<Option<RemoteItem>, String> {
        self.resolve()?.stat(remote, path, cancel)
    }
    fn list(
        &self,
        remote: &str,
        path: &str,
        recursive: bool,
        filter: ListFilter,
        cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        self.resolve()?.list(remote, path, recursive, filter, cancel)
    }
    fn mkdir(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.resolve()?.mkdir(remote, path, cancel)
    }
    fn delete_file(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.resolve()?.delete_file(remote, path, cancel)
    }
    fn purge(&self, remote: &str, path: &str, cancel: &Cancel) -> Result<(), String> {
        self.resolve()?.purge(remote, path, cancel)
    }
    fn copy_to_remote(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        self.resolve()?
            .copy_to_remote(local_path, remote, remote_path, cancel)
    }
    fn copy_to_local(
        &self,
        local_path: &str,
        remote: &str,
        remote_path: &str,
        cancel: &Cancel,
    ) -> Result<(), String> {
        self.resolve()?
            .copy_to_local(local_path, remote, remote_path, cancel)
    }
    fn delete_config(&self, _remote: &str) -> Result<(), String> {
        Ok(())
    }
    fn create_config(&self, _payload_json: String) -> Result<(), String> {
        Err("native-proton client does not accept rclone-style create_config payloads".to_owned())
    }
    fn remote_type(&self, _remote: &str) -> Result<Option<String>, String> {
        Ok(Some("native-proton".to_owned()))
    }
    /// Only meaningful once live — don't let a checkpoint drive a
    /// resume attempt of its own.
    fn checkpoint_session(&self, remote: &str) {
        if let Ok(state) = self.state.lock()
            && let PendingState::Live(client) = &*state
        {
            client.checkpoint_session(remote);
        }
    }
}

/// Placeholder adapter for native-proton remotes whose session couldn't
/// be resumed at startup (blob missing, refresh token expired, etc.).
/// Registered on the [`ClientRouter`] so the sync engine's calls fail
/// with a clear reauth instruction instead of falling through to the
/// default rclone client (which then errors with a cryptic
/// "didn't find section in config file" because native-proton remotes
/// never get written to rclone's config).
///
/// Every [`BackendClient`] method returns the same owned reason so the
/// UI can surface it verbatim in the sync_dir log.
#[derive(Clone, Debug)]
pub struct DisabledProtonClient {
    reason: String,
}

impl DisabledProtonClient {
    pub fn new(reason: String) -> Self {
        Self { reason }
    }
}

impl BackendClient for DisabledProtonClient {
    fn stat(
        &self,
        _remote: &str,
        _path: &str,
        _cancel: &Cancel,
    ) -> Result<Option<RemoteItem>, String> {
        Err(self.reason.clone())
    }
    fn list(
        &self,
        _remote: &str,
        _path: &str,
        _recursive: bool,
        _filter: ListFilter,
        _cancel: &Cancel,
    ) -> Result<Vec<RemoteItem>, String> {
        Err(self.reason.clone())
    }
    fn mkdir(&self, _remote: &str, _path: &str, _cancel: &Cancel) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn delete_file(&self, _remote: &str, _path: &str, _cancel: &Cancel) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn purge(&self, _remote: &str, _path: &str, _cancel: &Cancel) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn copy_to_remote(
        &self,
        _local_path: &str,
        _remote: &str,
        _remote_path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn copy_to_local(
        &self,
        _local_path: &str,
        _remote: &str,
        _remote_path: &str,
        _cancel: &Cancel,
    ) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn delete_config(&self, _remote: &str) -> Result<(), String> {
        Ok(())
    }
    fn create_config(&self, _payload_json: String) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn remote_type(&self, _remote: &str) -> Result<Option<String>, String> {
        Ok(Some("native-proton".to_owned()))
    }
}
