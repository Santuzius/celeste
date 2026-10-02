//! Live Proton login + Drive round-trip (read AND write).
//!
//! Reads credentials from env vars — keeps them out of shell history
//! and argv — then exercises: login → save → resume → list root →
//! stat first child (if any) → download first file (if any) →
//! create a scratch folder under the root → upload a small scratch
//! file into it → logout.
//!
//! Set `PROTON_WRITE_SMOKE=1` to enable the write portion (folder +
//! upload). Off by default so it doesn't litter the user's Drive on
//! every invocation.
//!
//!     PROTON_USERNAME=alice@proton.me \
//!     PROTON_PASSWORD='…' \
//!     PROTON_TOTP=123456 \            # optional
//!     PROTON_MAILBOX_PASSWORD='…' \   # optional, two-password mode only
//!     nix-shell --run "cargo run --manifest-path src/go/Cargo.toml \
//!         --example proton_login"
//!
//! On success you should see a UID, the saved credential file path,
//! a resumed UID (should match), and a clean logout line. Any error
//! surfaces as an `Err(...)` from Rust — Go side can't crash the
//! process across the FFI boundary.

use std::{env, path::PathBuf};

use celeste_go::{self as native, proton};

fn main() {
    // One-time Go runtime + rclone/native init.
    native::initialize();
    eprintln!("identity: {}", native::proton_drive_version());

    let params = proton::LoginParams {
        username: require("PROTON_USERNAME"),
        password: require("PROTON_PASSWORD"),
        two_fa: env::var("PROTON_TOTP").unwrap_or_default(),
        mailbox_password: env::var("PROTON_MAILBOX_PASSWORD").unwrap_or_default(),
        ..Default::default()
    };

    eprintln!("logging in as {}…", mask(&params.username));
    let cred = match proton::login(&params) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("login failed: {err}");
            std::process::exit(1);
        }
    };
    eprintln!("  login OK — uid={}", cred.uid);

    let session_path: PathBuf = env::temp_dir().join(format!(
        "celeste-proton-session-smoke-{}.json",
        std::process::id()
    ));
    if let Err(err) = proton::save_session(&cred.uid, &session_path) {
        eprintln!("save_session failed: {err}");
        // still attempt logout
        let _ = proton::logout(&cred.uid);
        std::process::exit(1);
    }
    eprintln!("  saved session → {}", session_path.display());

    // Resume into a *second* registered session from the same blob —
    // proves save / load works without touching the live creds again.
    let resumed = match proton::resume_session(&session_path) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("resume_session failed: {err}");
            let _ = proton::logout(&cred.uid);
            std::process::exit(1);
        }
    };
    eprintln!(
        "  resumed — uid={} (matches? {})",
        resumed.uid,
        resumed.uid == cred.uid
    );

    // Drive read smoke on the resumed session. The original UID is
    // shared so either works — we'll use the resumed one because it's
    // the one we'll still have after the original times out.
    let root_id = match proton::root_link_id(&resumed.uid) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("root_link_id failed: {err}");
            let _ = proton::logout(&resumed.uid);
            std::process::exit(1);
        }
    };
    eprintln!("  root link id = {root_id}");

    let entries = match proton::list_directory(&resumed.uid, "") {
        Ok(e) => e,
        Err(err) => {
            eprintln!("list_directory failed: {err}");
            let _ = proton::logout(&resumed.uid);
            std::process::exit(1);
        }
    };
    eprintln!("  root entries: {}", entries.len());
    for e in entries.iter().take(8) {
        eprintln!(
            "    - {:<30}  {}  size={}  mtime={}",
            e.name,
            if e.is_dir { "dir " } else { "file" },
            e.size,
            e.mod_time_unix
        );
    }
    if entries.len() > 8 {
        eprintln!("    … ({} more)", entries.len() - 8);
    }

    // Stat the first child to confirm the shape round-trips, then
    // download the first file (if any) to a temp path.
    if let Some(first) = entries.first() {
        match proton::stat(&resumed.uid, &first.link_id) {
            Ok(Some(entry)) => eprintln!("  stat({}) = {} bytes, is_dir={}", entry.name, entry.size, entry.is_dir),
            Ok(None) => eprintln!("  stat({}) returned None (non-active)", first.name),
            Err(err) => eprintln!("  stat failed: {err}"),
        }
    }
    if let Some(first_file) = entries.iter().find(|e| !e.is_dir) {
        let dl_path = env::temp_dir().join(format!(
            "celeste-proton-dl-smoke-{}",
            std::process::id()
        ));
        eprintln!(
            "  downloading '{}' ({} bytes) → {}…",
            first_file.name,
            first_file.size,
            dl_path.display()
        );
        match proton::download_file(&resumed.uid, &first_file.link_id, &dl_path) {
            Ok(()) => {
                let meta = std::fs::metadata(&dl_path);
                eprintln!(
                    "    downloaded OK, local bytes = {}",
                    meta.map(|m| m.len() as i64).unwrap_or(-1)
                );
                let _ = std::fs::remove_file(&dl_path);
            }
            Err(err) => eprintln!("    download failed: {err}"),
        }
    } else {
        eprintln!("  (no files at root to download — skipping download smoke)");
    }

    // Opt-in write smoke. Skipped by default so the Drive doesn't
    // accumulate scratch folders between runs.
    if env::var("PROTON_WRITE_SMOKE").ok().as_deref() == Some("1") {
        let folder_name = format!("celeste-native-smoke-{}", std::process::id());
        eprintln!("  creating folder '{folder_name}' under root…");
        match proton::create_folder(&resumed.uid, "", &folder_name) {
            Ok(id) => {
                eprintln!("    folder OK, link id = {id}");
                // Upload a tiny scratch file into the new folder.
                let payload = format!(
                    "celeste native proton upload smoke — pid {} — hello from cgo!\n",
                    std::process::id()
                );
                let src = env::temp_dir().join(format!(
                    "celeste-proton-upload-src-{}.txt",
                    std::process::id()
                ));
                if let Err(err) = std::fs::write(&src, &payload) {
                    eprintln!("    scratch write failed: {err}");
                } else {
                    eprintln!(
                        "  uploading '{}' ({} bytes) into folder…",
                        src.display(),
                        payload.len()
                    );
                    match proton::upload_file(
                        &resumed.uid,
                        &id,
                        src.file_name().unwrap().to_string_lossy().as_ref(),
                        &src,
                    ) {
                        Ok(file_id) => eprintln!("    upload OK, file link id = {file_id}"),
                        Err(err) => eprintln!("    upload failed: {err}"),
                    }
                }
                let _ = std::fs::remove_file(&src);
            }
            Err(err) => eprintln!("    create_folder failed: {err}"),
        }
    } else {
        eprintln!("  (PROTON_WRITE_SMOKE not set — skipping create_folder / upload smoke)");
    }

    // Clean up: log out the resumed session (which revokes the shared
    // refresh token), drop the file.
    match proton::logout(&resumed.uid) {
        Ok(()) => eprintln!("  logout OK"),
        Err(err) => eprintln!("  logout returned: {err}"),
    }
    let _ = std::fs::remove_file(&session_path);

    // The original (first) session is now also invalidated upstream
    // since it shared the refresh token. No need to log it out again.
    eprintln!("done.");
}

fn require(var: &str) -> String {
    match env::var(var) {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!("{var} is required (export it before running)");
            std::process::exit(2);
        }
    }
}

/// Redact all but the first character and the domain so the log shows
/// enough to identify the account while keeping the full email out of
/// terminal history.
fn mask(username: &str) -> String {
    let (local, at, domain) = match username.find('@') {
        Some(i) => (&username[..i], "@", &username[i + 1..]),
        None => (username, "", ""),
    };
    let first = local.chars().next().unwrap_or('?');
    format!("{first}***{at}{domain}")
}
