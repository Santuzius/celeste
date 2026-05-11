//! Iced application root. The Phase D entry point alongside the existing
//! GTK `launch::launch`. Runs the pure-Rust UI against the already-extracted
//! service layer.
//!
//! `update()` is a thin dispatch shell — every message variant routes to a
//! handler method defined in `app::handlers::*` (or `app::log` for the
//! per-sync_dir log buffer). Handler files own private fields of `CelesteApp`
//! because they are descendants of this module.

use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

use iced::{stream, theme as iced_theme, window, Element, Subscription, Task, Theme};
use tokio::sync::mpsc;

use crate::{
    domain::{
        events::SyncEvent,
        ports::Repository,
        remote::{ProviderKind, Remote, RemoteId},
        run_state::AppState,
        sync::{SyncDir, SyncDirExclusion, SyncDirId},
    },
    infrastructure::{
        client_router::ClientRouter,
        stderr_capture::{self, CaptureHandle},
        tray::{self, TrayAction, TrayUpdate},
    },
    screens::{add_remote, main_page, remote_page, settings},
    theme,
};

mod handlers;
mod log;

/// Messages the root application dispatches. Screen-level messages are
/// wrapped by variants; service results fire their own.
#[derive(Debug, Clone)]
pub enum Message {
    Main(main_page::Msg),
    Remote(remote_page::Msg),
    Settings(settings::Msg),
    AddRemote(add_remote::Msg),
    AddRemoteResult(Result<RemoteId, String>),
    RemotesLoaded(Vec<Remote>),
    SyncDirsLoaded(RemoteId, Vec<SyncDir>),
    AllSyncDirsRefreshed(Vec<SyncDir>),
    ExclusionsLoaded(SyncDirId, Vec<SyncDirExclusion>),
    PolicySaved,
    SyncStarted(RemoteId),
    SyncFinished(RemoteId, PassVerdict),
    WorkerReady(mpsc::Sender<SyncEvent>),
    SyncEventReceived(SyncEvent),
    Tick,
    /// Delivered once when the ksni service is live — carries the
    /// sender the app uses to push status / theme updates back into
    /// the tray task.
    TrayReady(mpsc::Sender<TrayUpdate>),
    /// A tray action from the user (menu click or left-click on the
    /// icon).
    TrayClick(TrayAction),
    /// The iced runtime detected (or was just told about) a system
    /// colour-scheme change. Forwarded to the tray so its rasterised
    /// glyphs flip tone with the panel.
    SystemThemeChanged(iced_theme::Mode),
    /// User-initiated quit — only the tray "Quit Celeste" entry. Handled
    /// by an immediate `std::process::exit` so we don't wait for
    /// in-flight FFI calls (notably librclone listings, which expose no
    /// cancellation handle) to return.
    Quit,
    /// A window was destroyed (X button, Alt-F4, or our own
    /// `iced::window::close`). Lets us clear the cached window id so
    /// the next "Open Celeste" creates a fresh window instead of
    /// targeting the dead one.
    WindowClosed(window::Id),
}

/// Aggregate outcome across every sync_dir of one remote's pass. The
/// scheduler uses this to drive linear backoff on provider rate-limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassVerdict {
    /// Every sync_dir finished cleanly.
    Clean,
    /// At least one sync_dir detected rate-limiting (stderr tap fired).
    /// Scheduler bumps `consecutive_degraded` and skips more cycles.
    Degraded,
    /// Pass aborted for a non-rate-limit reason (cancel, list error).
    /// Backoff counter is left alone.
    Aborted,
}

pub struct CelesteApp {
    repo: Arc<dyn Repository>,
    /// Client router — dispatches BackendClient calls per-remote.
    /// `Arc<ClientRouter>` rather than `Arc<dyn BackendClient>` so the
    /// add-/delete-remote paths can register / unregister native
    /// sessions on it; sync code downcasts on the fly (ClientRouter
    /// implements BackendClient).
    rclone: Arc<ClientRouter>,
    remotes: Vec<Remote>,
    sync_dirs: HashMap<RemoteId, Vec<SyncDir>>,
    selected: Option<RemoteId>,
    /// Remotes whose sync pass is currently running.
    syncing: std::collections::HashSet<RemoteId>,
    /// Accumulated log lines per sync_dir, capped at
    /// [`remote_page::MAX_LOG_LINES`] so a long-running session doesn't
    /// grow unbounded.
    sync_dir_log_lines: HashMap<SyncDirId, Vec<String>>,
    /// Read-only [`text_editor::Content`] mirror of the log lines, kept
    /// in sync so the remote-page editor can borrow it directly.
    sync_dir_log_content: HashMap<SyncDirId, iced::widget::text_editor::Content>,
    /// Hierarchical run-state machine: per-dir states, auth-failure
    /// bookkeeping, and backoff counters. Replaces the former flat
    /// `sync_dir_status`, `auth_failed_remotes`, `consecutive_degraded`,
    /// and `syncs_to_skip` fields.
    sync_state: AppState,
    /// All sync_dirs across every remote, refreshed on navigation changes.
    /// Used to compute auto-exclusions in the UI.
    all_known_sync_dirs: Vec<SyncDir>,
    /// The sync_dir whose exclusion panel is currently open (at most one).
    exclusion_panel: Option<SyncDirId>,
    /// Loaded user-defined exclusions per sync_dir.
    sync_dir_exclusions: HashMap<SyncDirId, Vec<SyncDirExclusion>>,
    /// Draft remote sub-path for the "add exclusion" form per sync_dir.
    draft_exclusion: HashMap<SyncDirId, String>,
    /// Wall-clock timestamp of the last sync completion per remote. Drives
    /// the interval scheduler.
    last_sync_at: HashMap<RemoteId, Instant>,
    /// Remote ids with a refresh request queued while the current pass is
    /// still running — as soon as SyncFinished lands we kick another pass.
    refresh_requested_after: std::collections::HashSet<RemoteId>,
    /// In-progress (local_path, remote_path) inputs for the Add sync_dir form
    /// on each remote page.
    sync_dir_drafts: HashMap<RemoteId, (String, String)>,
    /// In-progress Add Remote form. Some(...) while the screen is shown.
    add_remote_draft: Option<add_remote::Draft>,
    /// Sender handed to us by the subscription worker; sync code clones this
    /// to emit events back into the event loop.
    events_tx: Option<mpsc::Sender<SyncEvent>>,
    /// Per-remote cancel flags. Flipping `true` tells the in-flight
    /// sync pass to bail out between actions — the app sets it when
    /// the user disables a remote (or the app shuts down).
    cancel_flags: HashMap<RemoteId, Arc<AtomicBool>>,
    /// Stderr ring-buffer handle — shared by every sync pass so each
    /// can ask "did any provider rate-limit warning fire since my
    /// pass_start?". Installed once at process startup.
    stderr_capture: CaptureHandle,
    /// Sender into the ksni subscription task. `Some` once the tray
    /// handshake has landed; remains `None` if the session has no
    /// StatusNotifier host.
    tray_tx: Option<mpsc::Sender<TrayUpdate>>,
    /// Last system colour-scheme value reported by iced. Cached so
    /// the [`Message::TrayReady`] handshake can seed the tray with
    /// the current value before the next change fires.
    system_theme: iced_theme::Mode,
    /// What the user is about to delete, set while the confirmation
    /// dialog is open. `None` when no dialog is showing.
    pending_delete: Option<remote_page::PendingDelete>,
    /// Id of the live main window, or `None` when hidden-to-tray. We
    /// run as an `iced::daemon`: the runtime stays alive with no
    /// windows, and the tray's "Open Celeste" entry opens (or focuses)
    /// the window on demand. Tracked here so `Hide` knows what to
    /// close and `Open` can tell "no window" from "minimised".
    window_id: Option<window::Id>,
}

impl CelesteApp {
    fn new(
        repo: Arc<dyn Repository>,
        rclone: Arc<ClientRouter>,
    ) -> (Self, Task<Message>) {
        let state = Self {
            repo: repo.clone(),
            rclone,
            remotes: Vec::new(),
            sync_dirs: HashMap::new(),
            selected: None,
            syncing: std::collections::HashSet::new(),
            sync_dir_log_lines: HashMap::new(),
            sync_dir_log_content: HashMap::new(),
            sync_state: AppState::new(),
            all_known_sync_dirs: Vec::new(),
            exclusion_panel: None,
            sync_dir_exclusions: HashMap::new(),
            draft_exclusion: HashMap::new(),
            last_sync_at: HashMap::new(),
            refresh_requested_after: std::collections::HashSet::new(),
            sync_dir_drafts: HashMap::new(),
            add_remote_draft: None,
            events_tx: None,
            cancel_flags: HashMap::new(),
            stderr_capture: stderr_capture::handle(),
            tray_tx: None,
            system_theme: iced_theme::Mode::None,
            pending_delete: None,
            window_id: None,
        };
        let load = Task::perform(
            async move { repo.list_remotes().await.unwrap_or_default() },
            Message::RemotesLoaded,
        );
        // Seed the cached system theme with whatever iced already knows;
        // the subscription below picks up subsequent changes.
        let initial_theme = iced::system::theme().map(Message::SystemThemeChanged);
        (state, Task::batch([load, initial_theme]))
    }

    fn title(&self, _id: window::Id) -> String {
        "Celeste".to_string()
    }

    fn theme(&self, _id: window::Id) -> Theme {
        theme::celeste_theme(self.system_theme)
    }

    fn subscription(&self) -> Subscription<Message> {
        // Worker channel: the sync engine pushes `SyncEvent`s through
        // a tokio mpsc; we forward them to the iced runtime as
        // Messages. The first event the subscription emits is
        // `WorkerReady(tx)` so the app captures the sender.
        let events = Subscription::run(|| {
            stream::channel(128, async move |mut output| {
                use iced::futures::SinkExt;
                let (tx, mut rx) = mpsc::channel::<SyncEvent>(128);
                let _ = output.send(Message::WorkerReady(tx)).await;
                while let Some(event) = rx.recv().await {
                    let _ = output.send(Message::SyncEventReceived(event)).await;
                }
                std::future::pending::<()>().await;
            })
        });
        // Single ticker at 1 Hz — interval checks are cheap, and the
        // shortest allowed sync cadence is 5 s.
        let ticker = iced::time::every(Duration::from_secs(1)).map(|_| Message::Tick);
        let tray = tray::subscription().map(|signal| match signal {
            tray::TraySignal::Ready(tx) => Message::TrayReady(tx),
            tray::TraySignal::Action(action) => Message::TrayClick(action),
        });
        // Notice when the window is destroyed (X button, Alt-F4, or
        // our own `Hide` action) so we can drop the cached id. The
        // daemon keeps running with no window — the next "Open
        // Celeste" allocates a fresh one. We don't intercept
        // `CloseRequested`: iced's default `exit_on_close_request`
        // already destroys the window for us, and daemon mode doesn't
        // exit on the last-window destruction.
        let window_close = window::close_events().map(Message::WindowClosed);
        // Iced reads the freedesktop `org.freedesktop.appearance.color-scheme`
        // portal via `mundy` and emits a `Mode` whenever it changes. Forward
        // those into the tray so the rasterised glyphs follow the panel.
        let system_theme = iced::system::theme_changes().map(Message::SystemThemeChanged);
        Subscription::batch([events, ticker, tray, window_close, system_theme])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let cmd = match message {
            // Remote / dialog flow ----------------------------------
            Message::RemotesLoaded(remotes) => self.handle_remotes_loaded(remotes),
            Message::Main(main_page::Msg::Selected(id)) => self.handle_remote_selected(id),
            Message::Main(main_page::Msg::RefreshAll) => self.handle_refresh_all(),
            Message::Main(main_page::Msg::AddRemote) => self.handle_open_add_remote(),
            Message::AddRemote(sub) => self.handle_add_remote_msg(sub),
            Message::AddRemoteResult(Ok(id)) => self.handle_add_remote_result_ok(id),
            Message::AddRemoteResult(Err(msg)) => self.handle_add_remote_result_err(msg),
            Message::Remote(remote_page::Msg::Back) => self.handle_remote_back(),
            Message::Remote(remote_page::Msg::RefreshNow(id)) => self.handle_refresh_now(id),
            Message::Remote(remote_page::Msg::RequestDeleteRemote(id, name)) => {
                self.handle_request_delete_remote(id, name)
            }
            Message::Remote(remote_page::Msg::DeleteRemote(id, name)) => {
                self.handle_delete_remote(id, name)
            }
            Message::Remote(remote_page::Msg::ConfirmDelete) => self.handle_confirm_delete(),
            Message::Remote(remote_page::Msg::CancelDelete) => self.handle_cancel_delete(),
            Message::Remote(remote_page::Msg::Reauthenticate(id, name)) => {
                self.handle_reauthenticate(id, name)
            }

            // Sync_dir CRUD + exclusions + settings ------------------
            Message::SyncDirsLoaded(id, sd) => self.handle_sync_dirs_loaded(id, sd),
            Message::AllSyncDirsRefreshed(all) => self.handle_all_sync_dirs_refreshed(all),
            Message::Remote(remote_page::Msg::DraftLocalPathChanged(s)) => {
                self.handle_draft_local_path_changed(s)
            }
            Message::Remote(remote_page::Msg::DraftRemotePathChanged(s)) => {
                self.handle_draft_remote_path_changed(s)
            }
            Message::Remote(remote_page::Msg::AddSyncDir) => self.handle_add_sync_dir(),
            Message::Remote(remote_page::Msg::RequestDeleteSyncDir(local, remote)) => {
                self.handle_request_delete_sync_dir(local, remote)
            }
            Message::Remote(remote_page::Msg::DeleteSyncDir(local, remote)) => {
                self.handle_delete_sync_dir(local, remote)
            }
            Message::Remote(remote_page::Msg::Settings(sub)) | Message::Settings(sub) => {
                self.handle_settings(sub)
            }
            Message::PolicySaved => Task::none(),
            Message::ExclusionsLoaded(sd_id, excls) => {
                self.handle_exclusions_loaded(sd_id, excls)
            }
            Message::Remote(remote_page::Msg::ToggleExclusions(sd_id)) => {
                self.handle_toggle_exclusions(sd_id)
            }
            Message::Remote(remote_page::Msg::DraftExclusionChanged(sd_id, s)) => {
                self.handle_draft_exclusion_changed(sd_id, s)
            }
            Message::Remote(remote_page::Msg::AddExclusion(sd_id)) => {
                self.handle_add_exclusion(sd_id)
            }
            Message::Remote(remote_page::Msg::RemoveExclusion(excl_id, sd_id)) => {
                self.handle_remove_exclusion(excl_id, sd_id)
            }
            Message::Remote(remote_page::Msg::LogEditorAction(sd_id, action)) => {
                self.handle_log_editor_action(sd_id, action)
            }

            // Sync lifecycle ----------------------------------------
            Message::SyncStarted(id) => self.handle_sync_started(id),
            Message::SyncFinished(id, verdict) => self.handle_sync_finished(id, verdict),
            Message::Tick => self.handle_tick(),

            // Worker + sync events ----------------------------------
            Message::WorkerReady(tx) => self.handle_worker_ready(tx),
            Message::SyncEventReceived(event) => self.handle_sync_event(event),

            // Tray --------------------------------------------------
            Message::TrayReady(tx) => self.handle_tray_ready(tx),
            Message::TrayClick(action) => self.handle_tray_click(action),
            Message::SystemThemeChanged(mode) => self.handle_system_theme_changed(mode),
            Message::Quit => self.handle_quit(),
            Message::WindowClosed(id) => self.handle_window_closed(id),
        };
        self.push_tray_status();
        cmd
    }

    fn view(&self, _id: window::Id) -> Element<'_, Message> {
        if let Some(draft) = self.add_remote_draft.as_ref() {
            return add_remote::view(draft).map(Message::AddRemote);
        }

        match self
            .selected
            .and_then(|id| self.remotes.iter().find(|r| r.id == id))
        {
            Some(remote) => {
                let dirs: &[SyncDir] = self
                    .sync_dirs
                    .get(&remote.id)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let (draft_local, draft_remote) = self
                    .sync_dir_drafts
                    .get(&remote.id)
                    .map(|(l, r)| (l.as_str(), r.as_str()))
                    .unwrap_or(("", ""));
                let eta = self.next_sync_eta(remote.id);
                let needs_reauth = self.sync_state.needs_reauth(remote.id);
                remote_page::view(
                    remote,
                    dirs,
                    &self.sync_dir_log_content,
                    self.sync_state.dir_states(remote.id),
                    &self.all_known_sync_dirs,
                    self.exclusion_panel,
                    &self.sync_dir_exclusions,
                    &self.draft_exclusion,
                    (draft_local, draft_remote),
                    eta,
                    needs_reauth,
                    self.pending_delete.as_ref(),
                )
                .map(Message::Remote)
            }
            None => main_page::view(&self.remotes, self.selected, &self.sync_state)
                .map(Message::Main),
        }
    }
}

impl CelesteApp {
    /// Push the current tray status to the ksni task, if it's alive.
    /// Computation lives in the tray module so the mapping rule sits
    /// next to the icon set it drives. Silently drops on a full channel
    /// — the tray catches up on the next change (within one tick).
    fn push_tray_status(&self) {
        if let Some(tx) = self.tray_tx.as_ref() {
            let status = tray::compute_status(
                &self.sync_state,
                &self.remotes,
                &self.syncing,
                &self.last_sync_at,
            );
            let _ = tx.try_send(TrayUpdate::Status(status));
        }
    }

    /// Push the cached system colour-scheme into the tray. Used both
    /// on the [`Message::TrayReady`] handshake (to seed the initial
    /// tone) and on every subsequent `SystemThemeChanged`.
    pub(in crate::app) fn push_tray_theme(&self) {
        if let Some(tx) = self.tray_tx.as_ref() {
            let _ = tx.try_send(TrayUpdate::Theme(self.system_theme));
        }
    }
}

/// True when an error message indicates an auth failure across any backend.
/// Delegates to the per-backend translators so the classification logic
/// lives exactly once, in the translator, not scattered across the call site
/// and here.
pub(crate) fn is_auth_failure(msg: &str) -> bool {
    use crate::domain::backend_events::EventTranslator;
    use crate::infrastructure::translators::{
        proton::ProtonTranslator, rclone::RcloneTranslator,
    };
    RcloneTranslator.is_auth_failure(msg) || ProtonTranslator.is_auth_failure(msg)
}

/// True when two local paths overlap — equal, or one is a strict
/// descendant of the other. Used to reject AddSyncDir requests so all
/// sync_dirs stay on disjoint subtrees.
pub(crate) fn local_paths_overlap(a: &str, b: &str) -> bool {
    a == b || b.starts_with(&format!("{a}/")) || a.starts_with(&format!("{b}/"))
}

/// Bridge the domain `ProviderKind` (persisted on `Remote`) to the
/// add-remote screen's own enum. Returns `None` for providers the
/// add-remote UI doesn't currently expose, so reauth falls back to
/// the picker rather than locking onto a wrong backend.
pub(crate) fn map_domain_provider_to_add_remote(
    p: ProviderKind,
) -> Option<add_remote::ProviderKind> {
    match p {
        ProviderKind::ProtonDrive => Some(add_remote::ProviderKind::ProtonDrive),
        ProviderKind::GDrive => Some(add_remote::ProviderKind::GDrive),
        ProviderKind::Dropbox => Some(add_remote::ProviderKind::Dropbox),
        ProviderKind::PCloud => Some(add_remote::ProviderKind::PCloud),
        ProviderKind::WebDav => Some(add_remote::ProviderKind::WebDav),
        ProviderKind::Nextcloud => Some(add_remote::ProviderKind::Nextcloud),
        ProviderKind::Owncloud => Some(add_remote::ProviderKind::Owncloud),
    }
}

/// Launch the Iced daemon. Blocks until `Message::Quit` calls
/// `std::process::exit`.
///
/// We use [`iced::daemon`] (not `iced::application`) so the runtime
/// stays alive even when no window is open — the user closes the
/// window, the X11/Wayland surface is fully destroyed, the taskbar
/// entry disappears, and the tray icon remains as the sole UI surface
/// (matching Signal / Telegram / WhatsApp behaviour). The tray's "Open
/// Celeste" entry then opens a fresh window via [`window::open`].
///
/// Boot opens no window: Celeste typically autostarts at login, where a
/// pop-up would steal focus. The user surfaces it through the tray.
pub fn run(
    repo: Arc<dyn Repository>,
    rclone: Arc<ClientRouter>,
) -> iced::Result {
    // Bias iced's default glyph lookup to the sans-serif family so
    // cosmic-text's fallback layer resolves against the fonts we just
    // loaded instead of a bare built-in. Without this the ⚠ and
    // anything beyond basic Latin still falls through to tofu.
    let default_font = iced::Font {
        family: iced::font::Family::Name("Noto Sans"),
        ..iced::Font::DEFAULT
    };

    let mut builder = iced::daemon(
        move || CelesteApp::new(repo.clone(), rclone.clone()),
        CelesteApp::update,
        CelesteApp::view,
    )
    .title(CelesteApp::title)
    .theme(CelesteApp::theme)
    .subscription(CelesteApp::subscription)
    .default_font(default_font);

    for font in fallback_fonts() {
        builder = builder.font(font);
    }

    builder.run()
}

/// Settings for the main Celeste window. Plants the brand icon on
/// every freshly-opened surface (boot path and tray "Open Celeste")
/// so the compositor's titlebar / Wayland xdg-toplevel and the
/// taskbar entry both pick up the bundled `assets/celeste-icon.svg`
/// — sidesteps relying on a freedesktop hicolor install that may
/// not exist outside the packaged build.
pub(crate) fn main_window_settings() -> window::Settings {
    window::Settings {
        icon: crate::branding::window_icon(),
        ..window::Settings::default()
    }
}

/// Discover fallback fonts via fontconfig at startup and hand them to
/// iced as `Settings::fonts`. iced 0.12's bundled default only covers
/// Latin — without fallbacks, anything past ASCII (emoji, ⚠, Cyrillic,
/// CJK, …) silently drops from the render.
///
/// Queries cover three tiers of glyph coverage:
/// - emoji (color, e.g. Noto Color Emoji) for actual emoji;
/// - a dedicated symbols font for ⚠ / arrows / checkmarks;
/// - a general-purpose sans-serif for wide script coverage;
/// - a monospace for the rare places that want it.
///
/// Failures (no `fc-match`, missing fonts, unreadable files) degrade
/// gracefully — the app still runs, just without the extra coverage.
/// We log each load/miss to stderr so the first "I see boxes" report
/// is traceable.
fn fallback_fonts() -> Vec<std::borrow::Cow<'static, [u8]>> {
    [
        "Noto Color Emoji",
        "Noto Sans Symbols 2",
        "Noto Sans",
        "sans-serif",
        "emoji",
        "monospace",
    ]
    .iter()
    .filter_map(|q| fc_match_read(q))
    .map(std::borrow::Cow::Owned)
    .collect()
}

fn fc_match_read(pattern: &str) -> Option<Vec<u8>> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}"])
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?;
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    std::fs::read(path).ok()
}
