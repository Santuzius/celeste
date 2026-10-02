//! Iced application root. The Phase D entry point alongside the existing
//! GTK `launch::launch`. Runs the pure-Rust UI against the already-extracted
//! service layer.
//!
//! `update()` is a thin dispatch shell — every message variant routes to a
//! handler method defined in `app::handlers::*` (or `app::log` for the
//! per-sync_dir log buffer). Handler files own private fields of `CelesteApp`
//! because they are descendants of this module.

use std::{
    collections::{HashMap, VecDeque},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

use iced::{
    stream,
    theme as iced_theme,
    widget::{container, row, rule, stack},
    window, Element, Length, Size, Subscription, Task, Theme,
};
use tokio::sync::mpsc;

use crate::{
    domain::{
        events::SyncEvent,
        ports::Repository,
        remote::{ProviderKind, Remote, RemoteId},
        run_state::{AppState, RunState},
        sync::{Conflict, Resolution, SyncDir, SyncDirExclusion, SyncDirId},
    },
    infrastructure::{
        client_router::ClientRouter,
        single_instance,
        stderr_capture::{self, CaptureHandle},
        tray::{self, TrayAction, TrayUpdate},
    },
    screens::{about, add_remote, conflict, main_page, remote_page, settings},
    services::leftovers,
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
    About(about::Msg),
    Conflict(conflict::Msg),
    AddRemoteResult(Result<RemoteId, String>),
    RemotesLoaded(Vec<Remote>),
    SyncDirsLoaded(RemoteId, Vec<SyncDir>),
    AllSyncDirsRefreshed(Vec<SyncDir>),
    /// Exclusions of a sync_dir plus, per exclusion, how many synced files are still on this computer.
    ExclusionsLoaded(SyncDirId, Vec<SyncDirExclusion>, HashMap<String, usize>),
    /// Leftovers of an excluded path were deleted (or not); carries the path relative to the sync_dir for the log.
    LeftoversCleaned(SyncDirId, String, Result<leftovers::Cleaned, String>),
    /// Result of the desktop folder chooser (`None` = cancelled / no portal).
    LocalPathPicked(Option<String>),
    /// An add-sync-dir attempt finished; `Err` carries the message shown under the form.
    SyncDirAdded(RemoteId, Result<(), String>),
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
    /// Escape pressed outside a text field — closes the topmost dialog / panel.
    Escape,
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
    sync_dir_log_lines: HashMap<SyncDirId, VecDeque<log::LogLine>>,
    /// Read-only [`text_editor::Content`] mirror of the log lines — only
    /// for logs the user has expanded, since each one holds a fully
    /// shaped text buffer. Dropped again on collapse / window close.
    sync_dir_log_content: HashMap<SyncDirId, iced::widget::text_editor::Content>,
    /// Newest log line of the pass currently running per sync_dir, shown under the folder while it syncs. Cleared when a pass starts, so the previous pass's "Done" line doesn't linger.
    sync_dir_pass_line: HashMap<SyncDirId, String>,
    /// Google Drive remotes still on rclone's retiring shared OAuth client.
    shared_oauth_client: std::collections::HashSet<RemoteId>,
    /// Error from the last add-sync-dir attempt, shown under the form.
    add_sync_dir_error: Option<String>,
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
    /// Synced files still on this computer under an excluded path, by sync_dir and path relative to it; only non-zero counts.
    exclusion_leftovers: HashMap<SyncDirId, HashMap<String, usize>>,
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
    /// The remote page shows its settings instead of its folders.
    settings_open: bool,
    /// The About dialog is shown.
    about_open: bool,
    /// Files per sync dir that changed on both sides, as reported by the last pass.
    conflicts: HashMap<SyncDirId, Vec<Conflict>>,
    /// The user's choices, handed to the remote's next pass.
    resolutions: HashMap<RemoteId, HashMap<SyncDirId, Vec<Resolution>>>,
    /// Open conflict dialog.
    conflict_dialog: Option<conflict::Dialog>,
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
    /// Last status pushed to the tray, to skip redundant pushes.
    last_tray_status: Option<tray::TrayStatus>,
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
        show_window: bool,
    ) -> (Self, Task<Message>) {
        let mut state = Self {
            repo: repo.clone(),
            rclone,
            remotes: Vec::new(),
            sync_dirs: HashMap::new(),
            selected: None,
            syncing: std::collections::HashSet::new(),
            sync_dir_log_lines: HashMap::new(),
            sync_dir_log_content: HashMap::new(),
            sync_dir_pass_line: HashMap::new(),
            add_sync_dir_error: None,
            shared_oauth_client: std::collections::HashSet::new(),
            sync_state: AppState::new(),
            all_known_sync_dirs: Vec::new(),
            exclusion_panel: None,
            sync_dir_exclusions: HashMap::new(),
            exclusion_leftovers: HashMap::new(),
            draft_exclusion: HashMap::new(),
            last_sync_at: HashMap::new(),
            refresh_requested_after: std::collections::HashSet::new(),
            sync_dir_drafts: HashMap::new(),
            settings_open: false,
            about_open: false,
            conflicts: HashMap::new(),
            resolutions: HashMap::new(),
            conflict_dialog: None,
            add_remote_draft: None,
            events_tx: None,
            cancel_flags: HashMap::new(),
            stderr_capture: stderr_capture::handle(),
            tray_tx: None,
            last_tray_status: None,
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
        let open = if show_window {
            let (id, opened) = window::open(main_window_settings());
            state.window_id = Some(id);
            opened.discard()
        } else {
            Task::none()
        };
        (state, Task::batch([load, initial_theme, open]))
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
        // A second launch of the binary asks us (over the single-instance socket) to surface the window.
        let show_requests = single_instance::show_requests().map(|()| Message::TrayClick(TrayAction::Open));
        let escape = iced::keyboard::listen().filter_map(|event| match event {
            iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            } => Some(Message::Escape),
            _ => None,
        });
        Subscription::batch([events, ticker, tray, window_close, system_theme, show_requests, escape])
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
            Message::Remote(remote_page::Msg::RefreshNow(id)) => self.handle_refresh_now(id),
            Message::Remote(remote_page::Msg::OpenSettings) => {
                self.settings_open = true;
                Task::none()
            }
            Message::Remote(remote_page::Msg::CloseSettings) => {
                self.settings_open = false;
                Task::none()
            }
            Message::Main(main_page::Msg::OpenAbout) => {
                self.about_open = true;
                Task::none()
            }
            Message::About(about::Msg::Close) => {
                self.about_open = false;
                Task::none()
            }
            Message::About(about::Msg::Open(url)) => {
                handlers::remotes::open_in_browser(url);
                Task::none()
            }
            Message::Remote(remote_page::Msg::RequestDeleteRemote(id, name)) => {
                self.handle_request_delete_remote(id, name)
            }
            Message::Remote(remote_page::Msg::ConfirmDelete) => self.handle_confirm_delete(),
            Message::Remote(remote_page::Msg::CancelDelete) => self.handle_cancel_delete(),
            Message::Remote(remote_page::Msg::Reauthenticate(id, name)) => {
                self.settings_open = false;
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
            Message::Remote(remote_page::Msg::RequestDeleteSyncDir(local, remote, label)) => {
                self.handle_request_delete_sync_dir(local, remote, label)
            }
            Message::Remote(remote_page::Msg::BrowseLocalPath) => self.handle_browse_local_path(),
            Message::LocalPathPicked(path) => self.handle_local_path_picked(path),
            Message::SyncDirAdded(id, result) => self.handle_sync_dir_added(id, result),
            Message::Remote(remote_page::Msg::ToggleLog(sd_id)) => self.handle_toggle_log(sd_id),
            Message::Remote(remote_page::Msg::Settings(sub)) | Message::Settings(sub) => {
                self.handle_settings(sub)
            }
            Message::PolicySaved => Task::none(),
            Message::ExclusionsLoaded(sd_id, excls, leftovers) => {
                self.handle_exclusions_loaded(sd_id, excls, leftovers)
            }
            Message::Remote(remote_page::Msg::RequestCleanLeftovers(sd_id, relative)) => self.handle_request_clean_leftovers(sd_id, relative),
            Message::LeftoversCleaned(sd_id, relative, res) => self.handle_leftovers_cleaned(sd_id, relative, res),
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
            Message::Remote(remote_page::Msg::OpenConflict(sd_id, remote_path)) => self.handle_open_conflict(sd_id, remote_path),
            Message::Conflict(sub) => self.handle_conflict_msg(sub),

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
            Message::Escape => self.handle_escape(),
        };
        self.push_tray_status();
        cmd
    }

    fn view(&self, _id: window::Id) -> Element<'_, Message> {
        let nav = main_page::nav(
            self.remotes
                .iter()
                .map(|remote| main_page::NavEntry {
                    remote,
                    state: self.display_state(remote),
                    has_folders: self.all_known_sync_dirs.iter().any(|d| d.remote_id == remote.id),
                })
                .collect(),
            self.selected,
        )
        .map(Message::Main);

        let content: Element<'_, Message> = match self
            .selected
            .and_then(|id| self.remotes.iter().find(|r| r.id == id))
        {
            Some(remote) => remote_page::view(self.remote_page(remote)).map(Message::Remote),
            None => container(main_page::empty_state().map(Message::Main))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::page)
                .into(),
        };

        let base: Element<'_, Message> = row![nav, rule::vertical(1).style(theme::separator), content].into();
        if let Some(draft) = self.add_remote_draft.as_ref() {
            let dismiss = draft.can_cancel().then_some(Message::AddRemote(add_remote::Msg::Cancel));
            stack![base, remote_page::modal(add_remote::view(draft).map(Message::AddRemote), dismiss)].into()
        } else if let Some(dialog) = self.conflict_dialog.as_ref() {
            stack![base, remote_page::modal(conflict::view(dialog).map(Message::Conflict), None)].into()
        } else if let Some(pending) = self.pending_delete.as_ref() {
            stack![base, remote_page::confirm_delete_overlay(pending).map(Message::Remote)].into()
        } else if let Some(remote) = self.settings_open.then(|| self.selected_remote()).flatten() {
            let auth_needed = self.display_state(remote) == RunState::AuthNeeded;
            stack![base, remote_page::settings_dialog(remote, auth_needed).map(Message::Remote)].into()
        } else if self.about_open {
            stack![base, remote_page::modal(about::view().map(Message::About), Some(Message::About(about::Msg::Close)))].into()
        } else {
            base
        }
    }
}

impl CelesteApp {
    /// Close the topmost dialog or open panel.
    fn handle_escape(&mut self) -> Task<Message> {
        if self.pending_delete.take().is_some() {
            return Task::none();
        }
        if self.conflict_dialog.take().is_some() {
            return Task::none();
        }
        if std::mem::take(&mut self.about_open) {
            return Task::none();
        }
        if let Some(draft) = &self.add_remote_draft {
            if draft.can_cancel() {
                return self.handle_add_remote_msg(add_remote::Msg::Cancel);
            }
            return Task::none();
        }
        if std::mem::take(&mut self.settings_open) {
            return Task::none();
        }
        self.exclusion_panel = None;
        Task::none()
    }

    fn selected_remote(&self) -> Option<&Remote> {
        self.selected.and_then(|id| self.remotes.iter().find(|r| r.id == id))
    }

    /// Roll-up shown for a remote; falls back to the policy before the
    /// state machine knows the remote.
    fn display_state(&self, remote: &Remote) -> RunState {
        self.sync_state.roll_up(remote.id).unwrap_or(if remote.policy.enabled {
            RunState::Waiting
        } else {
            RunState::Paused
        })
    }

    /// Collect everything the remote page renders.
    fn remote_page<'a>(&'a self, remote: &'a Remote) -> remote_page::Page<'a> {
        let dirs: &[SyncDir] = self.sync_dirs.get(&remote.id).map_or(&[], |v| v.as_slice());
        let (draft_local, draft_remote) = self
            .sync_dir_drafts
            .get(&remote.id)
            .map_or(("", ""), |(l, r)| (l.as_str(), r.as_str()));
        let folders = dirs
            .iter()
            .map(|dir| {
                let lines = self.sync_dir_log_lines.get(&dir.id);
                remote_page::Folder {
                    dir,
                    state: self.sync_state.dir_state(remote.id, dir.id),
                    latest_line: self.sync_dir_pass_line.get(&dir.id).map(String::as_str),
                    latest_problem: lines
                        .and_then(|l| l.iter().rev().find(|line| line.text.starts_with('⚠')))
                        .map(|l| l.text.as_str()),
                    log: self.sync_dir_log_content.get(&dir.id),
                    exclusions_open: self.exclusion_panel == Some(dir.id),
                    auto_excluded: remote_page_auto_excluded(dir, &self.all_known_sync_dirs),
                    custom_excluded: self.sync_dir_exclusions.get(&dir.id).map_or(&[], |v| v.as_slice()),
                    leftovers: self.exclusion_leftovers.get(&dir.id),
                    conflicts: self.conflicts.get(&dir.id).map_or(&[], |v| v.as_slice()),
                    draft_exclusion: self.draft_exclusion.get(&dir.id).map_or("", |s| s.as_str()),
                }
            })
            .collect();
        remote_page::Page {
            remote,
            state: self.display_state(remote),
            syncing: self.syncing.contains(&remote.id),
            next_sync: self.next_sync_eta(remote.id),
            dirs: folders,
            draft_local,
            draft_remote,
            add_error: self.add_sync_dir_error.as_deref(),
            shared_oauth_client: self.shared_oauth_client.contains(&remote.id),
        }
    }

    /// Push the current tray status to the ksni task, if it's alive.
    /// Computation lives in the tray module so the mapping rule sits
    /// next to the icon set it drives. Silently drops on a full channel
    /// — the tray catches up on the next change (within one tick).
    fn push_tray_status(&mut self) {
        if let Some(tx) = self.tray_tx.as_ref() {
            let status = tray::compute_status(
                &self.sync_state,
                &self.remotes,
                &self.syncing,
                &self.last_sync_at,
            );
            // `update` runs at least once a second (ticker); only wake
            // the tray task when something visible actually changed.
            if self.last_tray_status.as_ref() != Some(&status)
                && tx.try_send(TrayUpdate::Status(status.clone())).is_ok()
            {
                self.last_tray_status = Some(status);
            }
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

/// Sync_dirs from `all` that are auto-excluded under `sd` — another
/// sync_dir on the same remote whose remote path is nested under
/// `sd.remote_path`. Local-tree overlaps are blocked at AddSyncDir time,
/// so only the remote-tree case can occur.
fn remote_page_auto_excluded<'a>(sd: &SyncDir, all: &'a [SyncDir]) -> Vec<&'a SyncDir> {
    all.iter()
        .filter(|d| d.id != sd.id && d.remote_id == sd.remote_id)
        .filter(|d| {
            if sd.remote_path.is_empty() {
                !d.remote_path.is_empty()
            } else {
                d.remote_path.starts_with(&format!("{}/", sd.remote_path))
            }
        })
        .collect()
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
/// Boot opens no window unless `show_window` (`--show`) is set: Celeste
/// typically autostarts at login, where a pop-up would steal focus. The
/// user surfaces it through the tray or by launching the binary again.
pub fn run(
    repo: Arc<dyn Repository>,
    rclone: Arc<ClientRouter>,
    show_window: bool,
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
        move || CelesteApp::new(repo.clone(), rclone.clone(), show_window),
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
        // Wayland has no per-window icon here; KWin / GNOME look the icon up via the app id in the matching `celeste.desktop` (X11: WM_CLASS).
        platform_specific: window::settings::PlatformSpecific { application_id: "celeste".to_owned(), ..Default::default() },
        size: Size::new(1000.0, 700.0),
        min_size: Some(Size::new(720.0, 460.0)),
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
    let mut paths: Vec<String> = Vec::new();
    for query in [
        "Noto Color Emoji",
        "Noto Sans Symbols 2",
        "Noto Sans",
        "sans-serif",
        "emoji",
        "monospace",
    ] {
        // Generic aliases usually resolve to a file an earlier query
        // already found; loading it twice would just double its RAM.
        if let Some(path) = fc_match_path(query)
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .map(std::borrow::Cow::Owned)
        .collect()
}

fn fc_match_path(pattern: &str) -> Option<String> {
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}"])
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (!path.is_empty()).then_some(path)
}
