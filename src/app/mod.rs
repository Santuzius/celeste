//! Iced application root. The Phase D entry point alongside the existing
//! GTK `launch::launch`. Runs the pure-Rust UI against the already-extracted
//! service layer.
//!
//! `update()` is a thin dispatch shell — every message variant routes to a
//! handler method defined in `app::handlers::*` (or `app::log` for the
//! per-sync_dir log buffer). Handler files own private fields of `CelesteApp`
//! because they are descendants of this module.

use std::{collections::HashMap, sync::Arc, time::Duration};

use iced::{
    stream,
    theme as iced_theme,
    widget::{container, row, rule, stack},
    window, Element, Length, Size, Subscription, Task, Theme,
};
use tokio::sync::mpsc;

use crate::{
    domain::{
        ports::Repository,
        remote::{ProviderKind, Remote, RemoteId},
        run_state::RunState,
        sync::{Conflict, SyncDir, SyncDirExclusion, SyncDirId},
    },
    engine::{self, Snapshot},
    infrastructure::{
        client_router::ClientRouter,
        single_instance,
        tray::{self, TrayAction, TrayUpdate},
    },
    screens::{about, add_remote, conflict, main_page, preferences, remote_page, settings},
    services::{
        appearance::{Appearance, ThemeChoice, TrayIconChoice},
        autostart, leftovers,
    },
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
    Preferences(preferences::Msg),
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
    /// The sync engine's state changed.
    Engine(Arc<Snapshot>),
    /// Redraw the countdowns and the tray's "last sync" age.
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

pub struct CelesteApp {
    repo: Arc<dyn Repository>,
    /// Client router — dispatches BackendClient calls per-remote.
    /// `Arc<ClientRouter>` rather than `Arc<dyn BackendClient>` so the
    /// add-/delete-remote paths can register / unregister native
    /// sessions on it; sync code downcasts on the fly (ClientRouter
    /// implements BackendClient).
    rclone: Arc<ClientRouter>,
    /// The sync engine, which runs the passes and keeps their state and logs.
    engine: engine::Handle,
    /// The engine's latest state.
    snapshot: Arc<Snapshot>,
    remotes: Vec<Remote>,
    sync_dirs: HashMap<RemoteId, Vec<SyncDir>>,
    selected: Option<RemoteId>,
    /// Read-only [`text_editor::Content`] mirror of the engine's log
    /// lines — only for logs the user has expanded, since each one holds
    /// a fully shaped text buffer. Dropped again on collapse / window close.
    sync_dir_log_content: HashMap<SyncDirId, iced::widget::text_editor::Content>,
    /// Google Drive remotes still on rclone's retiring shared OAuth client.
    shared_oauth_client: std::collections::HashSet<RemoteId>,
    /// Error from the last add-sync-dir attempt, shown under the form.
    add_sync_dir_error: Option<String>,
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
    /// In-progress (local_path, remote_path) inputs for the Add sync_dir form
    /// on each remote page.
    sync_dir_drafts: HashMap<RemoteId, (String, String)>,
    /// The remote page shows its settings instead of its folders.
    settings_open: bool,
    /// The About dialog is shown.
    about_open: bool,
    /// The Preferences dialog is shown.
    preferences_open: bool,
    /// Start at login, as last read from or written to the autostart entry.
    autostart: bool,
    /// Colour choices for the window and the tray icon.
    appearance: Appearance,
    /// Why saving a preference failed, shown in the Preferences dialog.
    preferences_error: Option<String>,
    /// Files per sync dir that changed on both sides: the engine's list, minus choices made since it was published.
    conflicts: HashMap<SyncDirId, Vec<Conflict>>,
    /// Open conflict dialog.
    conflict_dialog: Option<conflict::Dialog>,
    /// In-progress Add Remote form. Some(...) while the screen is shown.
    add_remote_draft: Option<add_remote::Draft>,
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
        engine: engine::Handle,
        show_window: bool,
    ) -> (Self, Task<Message>) {
        let mut state = Self {
            repo: repo.clone(),
            rclone,
            snapshot: engine.snapshot(),
            engine,
            remotes: Vec::new(),
            sync_dirs: HashMap::new(),
            selected: None,
            sync_dir_log_content: HashMap::new(),
            add_sync_dir_error: None,
            shared_oauth_client: std::collections::HashSet::new(),
            all_known_sync_dirs: Vec::new(),
            exclusion_panel: None,
            sync_dir_exclusions: HashMap::new(),
            exclusion_leftovers: HashMap::new(),
            draft_exclusion: HashMap::new(),
            sync_dir_drafts: HashMap::new(),
            settings_open: false,
            about_open: false,
            preferences_open: false,
            autostart: autostart::enabled(),
            appearance: Appearance::load(&crate::util::get_data_dir()),
            preferences_error: None,
            conflicts: HashMap::new(),
            conflict_dialog: None,
            add_remote_draft: None,
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
        theme::celeste_theme(self.resolved_mode(self.appearance.window))
    }

    fn subscription(&self) -> Subscription<Message> {
        // Every new engine state; the watch channel hands over only the latest when several piled up.
        let engine = Subscription::run(|| {
            stream::channel(1, async move |mut output| {
                use iced::futures::SinkExt;
                let Some(mut rx) = engine::get().map(engine::Handle::watch) else {
                    return std::future::pending().await;
                };
                loop {
                    let snapshot = rx.borrow_and_update().clone();
                    if output.send(Message::Engine(snapshot)).await.is_err() || rx.changed().await.is_err() {
                        break;
                    }
                }
                std::future::pending::<()>().await;
            })
        });
        // The window counts down to the next sync each second; the tray only shows the minutes since the last one.
        let tick = if self.window_id.is_some() { Duration::from_secs(1) } else { Duration::from_secs(30) };
        let ticker = iced::time::every(tick).map(|_| Message::Tick);
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
        Subscription::batch([engine, ticker, tray, window_close, system_theme, show_requests, escape])
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
            Message::Main(main_page::Msg::OpenPreferences) => {
                self.preferences_open = true;
                self.autostart = autostart::enabled();
                self.preferences_error = None;
                Task::none()
            }
            Message::Preferences(preferences::Msg::Close) => {
                self.preferences_open = false;
                Task::none()
            }
            Message::Preferences(preferences::Msg::WindowThemeChanged(choice)) => {
                self.set_appearance(Appearance { window: choice, ..self.appearance });
                Task::none()
            }
            Message::Preferences(preferences::Msg::TrayIconChanged(choice)) => {
                self.set_appearance(Appearance { tray_icon: choice, ..self.appearance });
                Task::none()
            }
            Message::Preferences(preferences::Msg::AutostartToggled(on)) => {
                match autostart::set(on) {
                    Ok(()) => {
                        self.autostart = on;
                        self.preferences_error = None;
                    }
                    Err(err) => self.preferences_error = Some(format!("Could not change {}: {err}", autostart::entry_path().display())),
                }
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
            Message::Remote(remote_page::Msg::OpenConflicts(sd_id)) => self.handle_open_conflicts(sd_id),
            Message::Conflict(sub) => self.handle_conflict_msg(sub),

            // Sync engine -------------------------------------------
            Message::Engine(snapshot) => self.handle_engine(snapshot),
            Message::Tick => Task::none(),

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
        } else if self.preferences_open {
            let dialog = preferences::view(self.appearance, self.autostart, self.preferences_error.as_deref()).map(Message::Preferences);
            stack![base, remote_page::modal(dialog, Some(Message::Preferences(preferences::Msg::Close)))].into()
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
        if std::mem::take(&mut self.preferences_open) || std::mem::take(&mut self.about_open) {
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
        self.snapshot.state.roll_up(remote.id).unwrap_or(if remote.policy.enabled {
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
                remote_page::Folder {
                    dir,
                    state: self.snapshot.state.dir_state(remote.id, dir.id),
                    latest_line: self.snapshot.pass_lines.get(&dir.id).map(String::as_str),
                    latest_problem: self.snapshot.problems.get(&dir.id).map(String::as_str),
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
            syncing: self.snapshot.syncing.contains(&remote.id),
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
            let status = tray::compute_status(&self.snapshot.state, &self.remotes, &self.snapshot.syncing, &self.snapshot.last_sync_at);
            // `update` runs on every tick and engine change; only wake
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
    /// The colour scheme a theme choice stands for right now.
    fn resolved_mode(&self, choice: ThemeChoice) -> iced_theme::Mode {
        match choice {
            ThemeChoice::System => self.system_theme,
            ThemeChoice::Light => iced_theme::Mode::Light,
            ThemeChoice::Dark => iced_theme::Mode::Dark,
        }
    }

    /// Store a changed appearance and apply it; the window re-reads it through `theme()`.
    fn set_appearance(&mut self, appearance: Appearance) {
        self.appearance = appearance;
        self.preferences_error = appearance.save(&crate::util::get_data_dir()).err().map(|err| format!("Could not save the colour choice: {err}"));
        self.push_tray_theme();
    }

    pub(in crate::app) fn push_tray_theme(&self) {
        if let Some(tx) = self.tray_tx.as_ref() {
            // The tray picks the glyph that contrasts with the given scheme: the white icon for Dark, the black one for Light (and None).
            let dark = self.system_theme == iced_theme::Mode::Dark;
            let white = match self.appearance.tray_icon {
                TrayIconChoice::System => dark,
                TrayIconChoice::Reversed => !dark,
                TrayIconChoice::White => true,
                TrayIconChoice::Black => false,
            };
            let mode = if white { iced_theme::Mode::Dark } else { iced_theme::Mode::Light };
            let _ = tx.try_send(TrayUpdate::Theme(mode));
        }
    }
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
    engine: engine::Handle,
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
        move || CelesteApp::new(repo.clone(), rclone.clone(), engine.clone(), show_window),
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
        #[cfg(not(target_os = "android"))]
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
