pub mod app;
pub mod branding;
pub mod domain;
pub mod icons;
pub mod infrastructure;
pub mod screens;
pub mod services;
#[cfg(test)]
pub mod test_support;
pub mod theme;
pub mod util;
pub mod widgets;

use std::sync::Arc;

use sea_orm::Database;
use serde_json::json;

use crate::{
    app::run as iced_run,
    domain::{
        ports::{BackendClient, Repository},
        remote::Backend,
    },
    infrastructure::{
        client_router::ClientRouter,
        persistence::{
            self,
            migrations::{legacy_config_dir, Migrator, MigratorTrait},
            repository::SeaOrmRepository,
        },
        proton::client::NativeProtonClient,
        rclone::LibrcloneClient,
        stderr_capture,
    },
    services::secrets,
};

fn main() {
    // Tap stderr before the Go runtime can grab it — that's the
    // only way to catch the `WARN[...] Too many requests` lines rclone's
    // backends emit when they silently retry a 429. Falls back to a no-op
    // if the platform can't hand us a pipe; sync keeps working, we just
    // lose rate-limit detection for the run.
    let _stderr = stderr_capture::install();

    // SQLite DB lives under ${XDG_DATA_HOME:-~/.local/share}/celeste.
    // The rclone config file (which holds OAuth tokens librclone writes
    // and reads as plaintext) lives in $XDG_RUNTIME_DIR — a per-session
    // tmpfs that's RAM-backed and wiped on logout — so the durable
    // copy is the keyring entry, not anything on rotational storage.
    // When XDG_RUNTIME_DIR isn't set we fall back to the data dir with
    // a warning; that's the only path where tokens can hit disk.
    let data_dir = util::get_data_dir();
    std::fs::create_dir_all(&data_dir).expect("failed to create data dir");

    legacy_config_dir::run(&data_dir);
    fold_stale_rclone_into_keyring(&data_dir);

    let rclone_config = match util::get_runtime_dir() {
        Some(dir) => {
            std::fs::create_dir_all(&dir).expect("failed to create runtime dir");
            dir.join("rclone.conf")
        }
        None => {
            eprintln!(
                "celeste: $XDG_RUNTIME_DIR is unset; rclone tokens will land on disk at {}",
                data_dir.display(),
            );
            data_dir.join("rclone.conf")
        }
    };
    hydrate_rclone_config(&rclone_config);

    celeste_go::initialize();
    // Prove the combined Go archive loaded — cheap (no network).
    eprintln!("celeste: native-go identity = {}", celeste_go::proton_drive_version());
    celeste_go::rpc(
        "config/setpath",
        json!({ "path": rclone_config }).to_string(),
    )
    .expect("failed to set rclone config path");

    let mut db_path = data_dir.clone();
    db_path.push("data.sqlite");
    if !db_path.exists() {
        std::fs::File::create(&db_path).expect("failed to create db file");
    }
    let db = util::await_future(Database::connect(format!(
        "sqlite://{}",
        db_path.display()
    )))
    .expect("failed to connect to the database");

    if util::await_future(persistence::has_legacy_migrations(&db)) {
        show_legacy_config_popup(&data_dir);
        std::process::exit(0);
    }

    util::await_future(Migrator::up(&db, None))
        .expect("failed to run database migrations");

    let repo: Arc<dyn Repository> = Arc::new(SeaOrmRepository::new(db));

    // Per-remote client router. celeste_go's librclone surface is the default — Celeste's
    // existing rclone-backed remotes keep working unchanged. Each
    // native-backend remote resumes its saved session up front so
    // the UID is registered before the first sync tick fires.
    let default_client: Arc<dyn BackendClient> = Arc::new(LibrcloneClient::new(rclone_config));
    let router = Arc::new(ClientRouter::new(default_client));
    resume_native_sessions(&*repo, &router);
    iced_run(repo, router).expect("iced app exited with error");
}

/// Sweep an `<data_dir>/rclone.conf` left behind by a prior version
/// (which kept the rclone config in the data dir) into the keyring,
/// then delete it. The keyring is canonical; the file is the leak.
fn fold_stale_rclone_into_keyring(data_dir: &std::path::Path) {
    let stale = data_dir.join("rclone.conf");
    if !stale.exists() {
        return;
    }
    match std::fs::read_to_string(&stale) {
        Ok(body) => {
            if !body.is_empty() {
                if let Err(err) = secrets::store(secrets::RCLONE_ACCOUNT, &body) {
                    eprintln!(
                        "celeste: keyring sync of stale rclone.conf failed: {err}; leaving file in place",
                    );
                    return;
                }
            }
            if let Err(err) = std::fs::remove_file(&stale) {
                eprintln!(
                    "celeste: couldn't delete stale rclone.conf at {}: {err}",
                    stale.display(),
                );
            } else {
                eprintln!(
                    "celeste: cleared stale rclone.conf from {}",
                    data_dir.display(),
                );
            }
        }
        Err(err) => eprintln!(
            "celeste: couldn't read stale rclone.conf at {}: {err}",
            stale.display(),
        ),
    }
}

/// Make sure the runtime `rclone.conf` matches what's in the keyring.
/// Treats the keyring as the source of truth: if it has an entry, the
/// on-disk file is overwritten; if not, an empty stub is created so
/// librclone has something to open.
fn hydrate_rclone_config(rclone_config: &std::path::Path) {
    match secrets::load(secrets::RCLONE_ACCOUNT) {
        Ok(Some(body)) => {
            if let Err(err) = std::fs::write(rclone_config, body) {
                eprintln!(
                    "celeste: couldn't write rclone config to {}: {err}",
                    rclone_config.display(),
                );
            }
        }
        Ok(None) => {
            // No keyring entry yet. If a stub file is missing, create
            // an empty one so librclone has something to open.
            if !rclone_config.exists() {
                let _ = std::fs::File::create(rclone_config);
            }
        }
        Err(err) => eprintln!("celeste: keyring read of rclone config failed: {err}"),
    }
}

/// Load every remote from the DB, and for those flagged
/// `Backend::NativeProton` resume the saved session (if any), wrap
/// the UID in a [`NativeProtonClient`], and register it on the
/// router keyed by remote name. When the resume fails, register a
/// [`DisabledProtonClient`] instead so the sync engine surfaces a
/// clear "Reauthenticate" message rather than falling through to
/// rclone (which would error with an opaque config-lookup failure).
fn resume_native_sessions(repo: &dyn Repository, router: &ClientRouter) {
    use crate::infrastructure::proton::client::DisabledProtonClient;
    let remotes = util::await_future(repo.list_remotes()).unwrap_or_default();
    for remote in remotes {
        if remote.backend != Backend::NativeProton {
            continue;
        }
        if remote.session_path.is_none() {
            let reason = format!(
                "Proton Drive session blob missing for '{}'. Click Reauthenticate on the remote page to log in again.",
                remote.name,
            );
            eprintln!("celeste: {reason}");
            notify_reauth_needed(&remote.name);
            router.register(
                remote.name.clone(),
                Arc::new(DisabledProtonClient::new(reason)),
            );
            continue;
        }
        match crate::services::auth::resume_proton_session(&remote.name) {
            Ok(Some(cred)) => {
                router.register(
                    remote.name.clone(),
                    Arc::new(NativeProtonClient::new(cred.uid)),
                );
                eprintln!(
                    "celeste: native-proton session resumed for '{}'.",
                    remote.name,
                );
            }
            Ok(None) => {
                let reason = format!(
                    "Proton Drive session for '{}' not found in keyring. Click Reauthenticate on the remote page to log in again.",
                    remote.name,
                );
                eprintln!("celeste: {reason}");
                notify_reauth_needed(&remote.name);
                router.register(
                    remote.name.clone(),
                    Arc::new(DisabledProtonClient::new(reason)),
                );
            }
            Err(err) => {
                let reason = format!(
                    "Proton Drive session for '{}' could not be resumed ({err}). Click Reauthenticate on the remote page to log in again.",
                    remote.name,
                );
                eprintln!("celeste: {reason}");
                notify_reauth_needed(&remote.name);
                router.register(
                    remote.name.clone(),
                    Arc::new(DisabledProtonClient::new(reason)),
                );
            }
        }
    }
}

/// Best-effort OS notification when a native-proton remote can't resume
/// its session at startup. Silently swallows errors — the remote page
/// banner + button are the authoritative recovery surface; the toast
/// is just there to nudge users who've minimised Celeste to the tray.
fn notify_reauth_needed(remote_name: &str) {
    let mut notification = notify_rust::Notification::new();
    notification
        .summary("Celeste: reauthentication needed")
        .body(&format!(
            "Sync is paused for '{remote_name}'. Open Celeste and click Reauthenticate to log in again.",
        ))
        .appname("Celeste");
    if let Some(icon) = branding::icon_file_path() {
        notification.icon(icon);
    }
    let _ = notification.show();
}

fn show_legacy_config_popup(data_dir: &std::path::Path) {
    use iced::{
        widget::{button, column, text},
        Element, Length, Task, Theme,
    };

    struct LegacyPopup {
        data_dir: String,
    }

    #[derive(Debug, Clone)]
    enum Msg {
        Ack,
    }

    fn legacy_update(_state: &mut LegacyPopup, _msg: Msg) -> Task<Msg> {
        iced::window::latest().and_then(iced::window::close)
    }

    fn legacy_view(state: &LegacyPopup) -> Element<'_, Msg> {
        column![
            text("Outdated Celeste configuration detected").size(20),
            text(format!(
                "The sync algorithm was rewritten and the database schema is no longer compatible.\n\nDelete the following directory and restart Celeste:\n\n  {}",
                state.data_dir,
            ))
            .size(14),
            button(text("Close Celeste")).on_press(Msg::Ack),
        ]
        .spacing(16)
        .padding(24)
        .max_width(560)
        .width(Length::Fill)
        .into()
    }

    fn legacy_theme(_state: &LegacyPopup) -> Theme {
        Theme::Dark
    }

    let data_dir = data_dir.display().to_string();
    let _ = iced::application(
        move || LegacyPopup {
            data_dir: data_dir.clone(),
        },
        legacy_update,
        legacy_view,
    )
    .title("Celeste — outdated configuration")
    .theme(legacy_theme)
    .run();
}
