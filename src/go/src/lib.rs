//! Celeste's combined Go archive — Rust side.
//!
//! Raw bindings live in `ffi` (bindgen-generated at build time from
//! `wrapper.go`'s `//export`s). The crate root also exposes small safe
//! wrappers matching the upstream `librclone` crate's API (initialize,
//! finalize, rpc) so the rest of Celeste can swap its `librclone` dep
//! for this one with no code changes in the rclone path. New surface
//! area (`proton_drive_version` and future native Drive calls) sits
//! alongside.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use std::{
    ffi::{CStr, CString},
    os::raw::c_char,
    path::Path,
};

use serde::{Deserialize, Serialize};

/// Initialize the Go runtime, rclone's librclone, and prepare the
/// native Proton client for use. Must be called once at process
/// startup before any other function in this crate.
pub fn initialize() {
    unsafe { ffi::RcloneInitialize() };
}

/// Finalize the Go runtime. Currently triggers a Go GC. Safe to skip
/// at process exit — the OS reclaims everything.
pub fn finalize() {
    unsafe { ffi::RcloneFinalize() };
}

/// Perform a single rclone RPC call. Identical signature to the
/// upstream `librclone::rpc` so existing Celeste code needn't change.
///
/// - `method`: e.g. `operations/list` — see <https://rclone.org/rc/>.
/// - `input`: a serialised JSON object.
/// - returns `Ok(json)` for HTTP 200, otherwise `Err(json)`.
pub fn rpc<S1: Into<String>, S2: Into<String>>(method: S1, input: S2) -> Result<String, String> {
    let mut method_cstr: Vec<c_char> = method
        .into()
        .into_bytes()
        .into_iter()
        .map(|b| b as c_char)
        .collect();
    method_cstr.push(0);
    let mut input_cstr: Vec<c_char> = input
        .into()
        .into_bytes()
        .into_iter()
        .map(|b| b as c_char)
        .collect();
    input_cstr.push(0);

    let result = unsafe { ffi::RcloneRPC(method_cstr.as_mut_ptr(), input_cstr.as_mut_ptr()) };
    let output = unsafe { CStr::from_ptr(result.Output) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::RcloneFreeString(result.Output) };
    if result.Status == 200 {
        Ok(output)
    } else {
        Err(output)
    }
}

/// Run rclone's OAuth flow for the backend `kind` (`drive`, `dropbox`, `pcloud`) in-process, like `rclone authorize`, with the given client ID and secret unless empty. Blocks until the browser has come back to rclone's local web server or [`authorize_cancel`] is called; returns the token JSON. Instead of opening a browser, rclone leaves the link to [`authorize_url`].
pub fn authorize(kind: &str, client_id: &str, client_secret: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct Reply {
        token: Option<String>,
        error: Option<String>,
    }
    let (Ok(kind), Ok(id), Ok(secret)) = (CString::new(kind), CString::new(client_id), CString::new(client_secret)) else {
        return Err("invalid client ID or secret".to_owned());
    };
    let reply = unsafe { take_string(ffi::CelesteAuthorize(kind.as_ptr() as *mut c_char, id.as_ptr() as *mut c_char, secret.as_ptr() as *mut c_char)) };
    match serde_json::from_str::<Reply>(&reply) {
        Ok(Reply { token: Some(token), .. }) => Ok(token),
        Ok(Reply { error: Some(error), .. }) => Err(error),
        _ => Err(format!("unexpected reply from rclone: {reply}")),
    }
}

/// The authorization link of the running [`authorize`], once rclone has made it.
pub fn authorize_url() -> Option<String> {
    let url = unsafe { ffi::CelesteAuthorizeURL() };
    (!url.is_null()).then(|| unsafe { take_string(url) })
}

/// Make a running [`authorize`] return with an error.
pub fn authorize_cancel() {
    unsafe { ffi::CelesteAuthorizeCancel() };
}

/// Make rclone encrypt its config file with `password` (rclone's own config encryption) and decrypt it on loading. Call before anything reads the config.
pub fn set_config_password(password: &str) -> Result<(), String> {
    let password = CString::new(password).map_err(|_| "invalid password".to_owned())?;
    let err = unsafe { ffi::CelesteSetConfigPassword(password.as_ptr() as *mut c_char) };
    if err.is_null() { Ok(()) } else { Err(unsafe { take_string(err) }) }
}

/// Load rclone's config file and write it back, encrypted after [`set_config_password`]. A file rclone cannot read stays as it is.
pub fn save_config() -> Result<(), String> {
    let err = unsafe { ffi::CelesteSaveConfig() };
    if err.is_null() { Ok(()) } else { Err(unsafe { take_string(err) }) }
}

/// Copy a string the Go side allocated, and free it.
unsafe fn take_string(ptr: *mut c_char) -> String {
    let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { ffi::RcloneFreeString(ptr) };
    text
}

/// How often remotes with change notification (Google Drive) ask their provider's change log; `None` restores the default of 5 s.
pub fn set_change_poll_interval(interval: Option<std::time::Duration>) {
    let seconds = interval.map_or(0, |d| d.as_secs().clamp(1, i32::MAX as u64) as i32);
    unsafe { ffi::RcloneSetChangePollSeconds(seconds) };
}

/// Whether the rclone remote `remote` (name without colon) may have changed on the provider's side since the previous call. Cheap: no network, the backend polls its change log in the background. Always `true` for backends without change notification.
pub fn remote_changed(remote: &str) -> bool {
    let Ok(c_remote) = CString::new(remote) else {
        return true;
    };
    unsafe { ffi::RcloneRemoteChanged(c_remote.as_ptr() as *mut c_char) != 0 }
}

/// Stop change tracking for `remote`, e.g. after its config was replaced or deleted.
pub fn forget_remote(remote: &str) {
    if let Ok(c_remote) = CString::new(remote) {
        unsafe { ffi::RcloneForgetRemote(c_remote.as_ptr() as *mut c_char) };
    }
}

/// Smoke ping across the cgo boundary to `go-proton-api`. Constructs
/// a Manager on the Go side and throws it away; no network work. Used
/// during startup to verify the combined Go archive linked correctly.
pub fn proton_drive_version() -> String {
    read_c_string(unsafe { ffi::ProtonDrive_Version() })
}

/// Native ProtonDrive client — safe wrappers around the `ProtonDrive_*`
/// cgo entry points.
pub mod proton {
    use super::{call_json, ffi, invoke_raw};
    use std::path::Path;

    /// Set the app identity Proton sees in `x-pm-appversion`. Call once at startup, before any login or resume.
    pub fn set_app_version(version: &str) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_SetAppVersion(payload) },
            &serde_json::json!({ "version": version }),
        )?;
        Ok(())
    }

    /// Credentials required for a fresh login. `mailbox_password` is
    /// only needed for two-password accounts; `two_fa` only for those
    /// with TOTP enabled — when required and missing, `login` returns
    /// `Err` with a descriptive message.
    #[derive(Clone, Debug, Default, serde::Serialize)]
    pub struct LoginParams {
        pub username: String,
        pub password: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        pub mailbox_password: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        pub two_fa: String,
        /// Solved human-verification challenge from a previous attempt.
        #[serde(skip_serializing_if = "String::is_empty")]
        pub hv_token: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        pub hv_methods: Vec<String>,
    }

    /// Prefix of a login error that is really Proton asking for human
    /// verification (CAPTCHA). Mirrors `drive.HVErrorPrefix`.
    const HV_ERROR_PREFIX: &str = "HUMAN_VERIFICATION_REQUIRED ";

    /// A human-verification challenge returned by a login attempt.
    #[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
    pub struct HumanVerification {
        pub token: String,
        #[serde(default)]
        pub methods: Vec<String>,
    }

    impl HumanVerification {
        /// Parse a [`login`] error; `None` for every other error.
        pub fn from_login_error(err: &str) -> Option<Self> {
            serde_json::from_str(err.strip_prefix(HV_ERROR_PREFIX)?).ok()
        }

        /// Page where the user solves the challenge in a normal browser.
        pub fn url(&self) -> String {
            let methods = if self.methods.is_empty() { "captcha".to_owned() } else { self.methods.join(",") };
            format!("https://verify.proton.me/?methods={methods}&token={}", self.token)
        }
    }

    /// Reusable credential — the JSON shape stored on disk and handed
    /// back from `login` / `resume`. `uid` is the session handle used
    /// by subsequent operations.
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    pub struct ReusableCredential {
        pub uid: String,
        pub access_token: String,
        pub refresh_token: String,
        pub salted_key_pass: String,
    }

    /// Log in to ProtonDrive with username + password (+ optional
    /// TOTP / mailbox password). Returns the reusable credential on
    /// success; error string on failure (bad password, missing 2FA,
    /// transport error, etc.).
    pub fn login(params: &LoginParams) -> Result<ReusableCredential, String> {
        call_json::<_, ReusableCredential>(
            |payload| unsafe { ffi::ProtonDrive_Login(payload) },
            params,
        )
    }

    /// Log out — revokes the session server-side AND drops local state.
    pub fn logout(uid: &str) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_Logout(payload) },
            &serde_json::json!({ "uid": uid }),
        )?;
        Ok(())
    }

    /// Persist the named session's reusable credential blob to disk.
    /// File is written with 0600 perms (Unix) by the Go side.
    pub fn save_session(uid: &str, path: &Path) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_SaveSession(payload) },
            &serde_json::json!({
                "uid": uid,
                "path": path.to_string_lossy(),
            }),
        )?;
        Ok(())
    }

    /// Persist the named session's credential blob to `path` only when
    /// the tokens have rotated (via a background refresh) since the
    /// last save. Returns `true` when a fresh blob was written — the
    /// caller then re-stores it in the keyring; `false` means nothing
    /// changed and no file was touched. Cheap to call after every sync
    /// pass.
    pub fn save_session_if_rotated(uid: &str, path: &Path) -> Result<bool, String> {
        #[derive(serde::Deserialize)]
        struct Saved {
            saved: bool,
        }
        let out = call_json::<_, Saved>(
            |payload| unsafe { ffi::ProtonDrive_SaveSessionIfRotated(payload) },
            &serde_json::json!({
                "uid": uid,
                "path": path.to_string_lossy(),
            }),
        )?;
        Ok(out.saved)
    }

    /// Rehydrate a session from a credential blob previously written
    /// by `save_session`. Returns the credential so the caller can
    /// pick up the `uid`.
    pub fn resume_session(path: &Path) -> Result<ReusableCredential, String> {
        call_json::<_, ReusableCredential>(
            |payload| unsafe { ffi::ProtonDrive_ResumeSession(payload) },
            &serde_json::json!({ "path": path.to_string_lossy() }),
        )
    }

    // ---------------- Drive read ----------------

    /// One child of a folder (or a single stat target). Matches
    /// `drive.Entry` on the Go side. `mod_time_unix` is a seconds
    /// timestamp; `size` is the link's stored size (for files that's
    /// encrypted size, not plaintext — Proton stores plaintext size
    /// in the revision XAttr, which we don't parse yet).
    #[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
    pub struct Entry {
        pub link_id: String,
        pub parent_link_id: String,
        pub name: String,
        pub is_dir: bool,
        pub size: i64,
        pub mod_time_unix: i64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub mime_type: String,
    }

    /// Return the root folder's link ID for the logged-in session.
    pub fn root_link_id(uid: &str) -> Result<String, String> {
        call_json::<_, String>(
            |payload| unsafe { ffi::ProtonDrive_RootLinkID(payload) },
            &serde_json::json!({ "uid": uid }),
        )
    }

    /// List the active children of `link_id` (pass an empty string to
    /// list the session's root). Names are decrypted; sort order is
    /// whatever Proton returns.
    pub fn list_directory(uid: &str, link_id: &str) -> Result<Vec<Entry>, String> {
        call_json::<_, Vec<Entry>>(
            |payload| unsafe { ffi::ProtonDrive_ListDirectory(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )
    }

    /// Recursively list all active entries under `link_id` using
    /// concurrent API calls on the Go side.  Each entry's `name`
    /// field is the full relative path (e.g. "Foo/Bar/baz.txt").
    /// Pass an empty string for `link_id` to start from the root.
    pub fn list_recursive(uid: &str, link_id: &str) -> Result<Vec<Entry>, String> {
        call_json::<_, Vec<Entry>>(
            |payload| unsafe { ffi::ProtonDrive_ListRecursive(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )
    }

    /// Plaintext size, recorded modification time and SHA-1 of a file's active revision. Matches `drive.FileDetails`.
    #[derive(Clone, Debug, serde::Deserialize)]
    pub struct FileDetails {
        pub size: i64,
        pub has_size: bool,
        pub mod_time_unix: i64,
        #[serde(default)]
        pub sha1: String,
    }

    /// Fresh details of a file for conflict handling. `Ok(None)` for folders and inactive links.
    pub fn file_details(uid: &str, link_id: &str) -> Result<Option<FileDetails>, String> {
        // `data` is null for folders, which `call_json` would treat as a malformed reply.
        let data = invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_FileDetails(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )?;
        data.map(|v| serde_json::from_value(v).map_err(|e| format!("invalid response shape: {e}"))).transpose()
    }

    /// Metadata for a single link. Returns `Ok(None)` when the link
    /// exists but is not in the active state (matches the semantics
    /// the sync engine's `stat` port expects from its client trait).
    pub fn stat(uid: &str, link_id: &str) -> Result<Option<Entry>, String> {
        call_json::<_, Option<Entry>>(
            |payload| unsafe { ffi::ProtonDrive_Stat(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )
    }

    /// Download the active revision of a file link to `dest_path`,
    /// creating parent directories as needed. Blocks until complete.
    pub fn download_file(uid: &str, link_id: &str, dest_path: &Path) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_DownloadFile(payload) },
            &serde_json::json!({
                "uid": uid,
                "link_id": link_id,
                "dest_path": dest_path.to_string_lossy(),
            }),
        )?;
        Ok(())
    }

    // ---------------- Drive write ----------------

    /// Create a new folder named `name` under `parent_link_id`. Pass
    /// an empty string for `parent_link_id` to target the session
    /// root. Returns the new folder's link ID.
    pub fn create_folder(uid: &str, parent_link_id: &str, name: &str) -> Result<String, String> {
        call_json::<_, String>(
            |payload| unsafe { ffi::ProtonDrive_CreateFolder(payload) },
            &serde_json::json!({
                "uid": uid,
                "parent_link_id": parent_link_id,
                "name": name,
            }),
        )
    }

    /// Upload the local file at `src_path` as a new child of
    /// `parent_link_id`, named `name`. Returns the new file's link
    /// ID. Blocks until the upload completes — no progress callback
    /// yet.
    pub fn upload_file(
        uid: &str,
        parent_link_id: &str,
        name: &str,
        src_path: &Path,
    ) -> Result<String, String> {
        call_json::<_, String>(
            |payload| unsafe { ffi::ProtonDrive_UploadFile(payload) },
            &serde_json::json!({
                "uid": uid,
                "parent_link_id": parent_link_id,
                "name": name,
                "src_path": src_path.to_string_lossy(),
            }),
        )
    }

    // ---------------- Destructive ----------------

    /// Move the named link into Proton's Trash. Works for files and
    /// folders; Proton cascade-trashes a folder's contents
    /// server-side. One link ID per call — the cgo shim constructs
    /// the `TrashChildren` request body with exactly this ID.
    pub fn trash_link(uid: &str, link_id: &str) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_TrashLink(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )?;
        Ok(())
    }

    /// Permanently delete the named link (no Trash recovery window).
    /// Celeste's sync engine should NOT call this — mirror deletes
    /// go through `trash_link` so the user keeps a restore window.
    /// Exposed so an explicit "empty trash for this item" UX can
    /// hook it later.
    pub fn permanent_delete_link(uid: &str, link_id: &str) -> Result<(), String> {
        invoke_raw(
            |payload| unsafe { ffi::ProtonDrive_PermanentDeleteLink(payload) },
            &serde_json::json!({ "uid": uid, "link_id": link_id }),
        )?;
        Ok(())
    }

    /// Internal: the caller-visible error type for every `ProtonDrive_*`
    /// entry point is just a String, to keep the FFI boundary narrow.
    /// Errors-as-strings leaves room to add structured variants later
    /// without churning the Rust surface.
    ///
    /// (Re-exported Envelope type so downstream users can write their
    /// own FFI wrappers on top of the raw bindings if needed.)
    pub use super::Envelope as RawEnvelope;
    pub use super::call_json as raw_call_json;
    pub use super::invoke_raw as raw_invoke;
}

// ----------------- internal plumbing -----------------

/// The JSON envelope every `ProtonDrive_*` shim returns. `data` is
/// absent on failure; `error` is absent on success.
#[derive(Debug, Deserialize, Serialize)]
pub struct Envelope {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Call a `ProtonDrive_*` shim that takes a JSON payload and returns
/// a JSON envelope — parse the envelope into `T`, returning the
/// server-side error string as `Err` when the shim reports failure.
pub fn call_json<P, T>(
    shim: impl FnOnce(*mut c_char) -> *mut c_char,
    params: &P,
) -> Result<T, String>
where
    P: Serialize,
    T: for<'de> Deserialize<'de>,
{
    let value = invoke_raw(shim, params)?;
    let Some(value) = value else {
        return Err("shim returned OK but no data to deserialise".to_owned());
    };
    serde_json::from_value(value).map_err(|e| format!("invalid response shape: {e}"))
}

/// Call a `ProtonDrive_*` shim and return the raw `data` field of its
/// envelope (Some on success with payload, None on success without).
pub fn invoke_raw<P>(
    shim: impl FnOnce(*mut c_char) -> *mut c_char,
    params: &P,
) -> Result<Option<serde_json::Value>, String>
where
    P: Serialize,
{
    let json = serde_json::to_string(params).map_err(|e| e.to_string())?;
    let c_params = CString::new(json).map_err(|e| e.to_string())?;
    let raw = shim(c_params.as_ptr() as *mut c_char);
    let envelope_json = read_c_string(raw);
    let envelope: Envelope = serde_json::from_str(&envelope_json)
        .map_err(|e| format!("invalid envelope from Go: {e} — body was: {envelope_json}"))?;
    if !envelope.ok {
        return Err(envelope.error);
    }
    Ok(envelope.data)
}

/// Read a `*mut c_char` returned from the Go side and free it via
/// `RcloneFreeString` (which maps to the same allocator Go used to
/// `C.CString` the bytes). Never panics; non-UTF-8 bytes are replaced.
fn read_c_string(raw: *mut c_char) -> String {
    if raw.is_null() {
        return String::new();
    }
    let out = unsafe { CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::RcloneFreeString(raw) };
    out
}

// `Path` / `Serialize` need to be in scope somewhere or unused-imports
// screams; re-assert here.
#[allow(dead_code)]
fn _path_used_somewhere(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::proton::HumanVerification;

    /// The in-process `rclone authorize` hands over its link and stops on cancel, freeing its port for the next try. Opens no browser and needs no network.
    #[test]
    fn authorize_hands_over_link_and_cancels() {
        super::initialize();
        for _ in 0..2 {
            let flow = std::thread::spawn(|| super::authorize("drive", "id.apps.googleusercontent.com", "secret"));
            let url = loop {
                if let Some(url) = super::authorize_url() {
                    break url;
                }
                assert!(!flow.is_finished(), "authorize ended early: {:?}", flow.join());
                std::thread::sleep(std::time::Duration::from_millis(20));
            };
            assert!(url.starts_with("http://127.0.0.1:53682/auth?state="), "{url}");
            while !flow.is_finished() {
                super::authorize_cancel();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(flow.join().unwrap().is_err());
        }
    }

    #[test]
    fn parses_human_verification_errors() {
        let err = r#"HUMAN_VERIFICATION_REQUIRED {"methods":["captcha"],"token":"abc123"}"#;
        let hv = HumanVerification::from_login_error(err).expect("HV error");
        assert_eq!(hv.token, "abc123");
        assert_eq!(hv.url(), "https://verify.proton.me/?methods=captcha&token=abc123");
        assert_eq!(HumanVerification::from_login_error("422 POST …: Code=2001"), None);
    }
}
