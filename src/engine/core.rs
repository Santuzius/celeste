//! The engine's state and how each input changes it. Starting a pass is the only side effect; everything else is plain state, so the tests drive this directly.

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use tokio::sync::mpsc;

use crate::{
    domain::{
        events::SyncEvent,
        ports::{BackendClient, Repository},
        remote::{ProviderKind, Remote, RemoteId},
        run_state::{AppState, RunState, SyncActivity},
        sync::{Conflict, Resolution, SyncDir, SyncDirId, SyncError},
    },
    infrastructure::{
        stderr_capture::CaptureHandle,
        tray::{self, TrayStatus},
    },
    util::fmt_home,
};

use super::{is_auth_failure, local_clock, pass::Pass, Command, Input, LogLine, Logs, PassVerdict, Snapshot, CONFLICT_LINE_PREFIXES, MAX_LOG_LINES};

pub(crate) struct Core {
    repo: Arc<dyn Repository>,
    backend: Arc<dyn BackendClient>,
    stderr: CaptureHandle,
    /// Where passes send their events and their end.
    tx: mpsc::UnboundedSender<Input>,
    remotes: Vec<Remote>,
    sync_dirs: Vec<SyncDir>,
    /// Hierarchical run-state machine: per-dir states, auth-failure bookkeeping, and backoff counters.
    state: AppState,
    syncing: HashSet<RemoteId>,
    /// When each remote's last pass ended. Drives the interval scheduler.
    last_sync_at: HashMap<RemoteId, Instant>,
    /// Remotes with a pass requested while one was running; it starts as soon as that one ends.
    refresh_requested_after: HashSet<RemoteId>,
    /// Per-remote cancel flags. Flipping `true` tells the running pass to bail out between actions — set when the user disables or deletes the remote, or the session turns out dead.
    cancel_flags: HashMap<RemoteId, Arc<AtomicBool>>,
    /// The user's conflict choices, handed to the remote's next pass. They stay until a pass reports back on them, so a cancelled pass doesn't lose them.
    resolutions: HashMap<RemoteId, HashMap<SyncDirId, Vec<Resolution>>>,
    conflicts: HashMap<SyncDirId, Vec<Conflict>>,
    pass_lines: HashMap<SyncDirId, String>,
    problems: HashMap<SyncDirId, String>,
    logs: Logs,
    log_version: u64,
    conflicts_version: u64,
    passes_finished: u64,
    last_status: Option<TrayStatus>,
}

impl Core {
    pub fn new(repo: Arc<dyn Repository>, backend: Arc<dyn BackendClient>, stderr: CaptureHandle, logs: Logs, tx: mpsc::UnboundedSender<Input>) -> Self {
        Self {
            repo,
            backend,
            stderr,
            tx,
            remotes: Vec::new(),
            sync_dirs: Vec::new(),
            state: AppState::new(),
            syncing: HashSet::new(),
            last_sync_at: HashMap::new(),
            refresh_requested_after: HashSet::new(),
            cancel_flags: HashMap::new(),
            resolutions: HashMap::new(),
            conflicts: HashMap::new(),
            pass_lines: HashMap::new(),
            problems: HashMap::new(),
            logs,
            log_version: 0,
            conflicts_version: 0,
            passes_finished: 0,
            last_status: None,
        }
    }

    pub async fn handle(&mut self, input: Input) {
        match input {
            Input::Command(command) => self.handle_command(command).await,
            Input::Event(event) => self.handle_event(event),
            Input::PassFinished(id, verdict) => self.finish_pass(id, verdict),
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::Reload => self.reload().await,
            Command::SyncNow(id) => self.sync_now(id),
            Command::SyncAll => {
                let ids: Vec<RemoteId> = self.remotes.iter().map(|r| r.id).filter(|id| self.is_schedulable(*id)).collect();
                for id in ids {
                    self.start_sync(id);
                }
            }
            Command::SyncSoon(id) => {
                self.last_sync_at.remove(&id);
            }
            Command::SignedIn(id) => {
                self.state.reauth_complete(id);
                self.state.set_remote_enabled(id, true);
                // Sync right away instead of waiting out the interval.
                self.last_sync_at.remove(&id);
                self.reload().await;
            }
            Command::Resolve { remote_id, sync_dir_id, resolution } => self.resolve(remote_id, sync_dir_id, resolution),
            Command::Log(sync_dir_id, line) => self.push_log_line(sync_dir_id, line),
        }
    }

    /// Read remotes and folders from the database and align the state with them.
    pub async fn reload(&mut self) {
        let Ok(mut remotes) = self.repo.list_remotes().await else { return };
        let Ok(sync_dirs) = self.repo.list_all_sync_dirs().await else { return };
        // The provider decides whether a pass watches stderr for rate-limit warnings; a failed lookup only loses that.
        for r in &mut remotes {
            if let Ok(Some(t)) = self.backend.remote_type(&r.name) {
                r.provider_kind = ProviderKind::from_rclone_type(&t);
            }
        }
        self.apply(remotes, sync_dirs);
    }

    fn apply(&mut self, remotes: Vec<Remote>, sync_dirs: Vec<SyncDir>) {
        for gone in self.remotes.iter().filter(|old| !remotes.iter().any(|r| r.id == old.id)) {
            if let Some(flag) = self.cancel_flags.remove(&gone.id) {
                flag.store(true, Ordering::Release);
            }
            self.state.remove_remote(gone.id);
            self.last_sync_at.remove(&gone.id);
            self.refresh_requested_after.remove(&gone.id);
            self.resolutions.remove(&gone.id);
        }
        for r in &remotes {
            let was_enabled = self.remotes.iter().find(|old| old.id == r.id).map(|old| old.policy.enabled);
            // Disabled while syncing: the running pass bails out between actions. Re-enabling waits for the scheduler.
            if was_enabled == Some(true) && !r.policy.enabled {
                if let Some(flag) = self.cancel_flags.get(&r.id) {
                    flag.store(true, Ordering::Release);
                }
                self.refresh_requested_after.remove(&r.id);
            }
            self.state.ensure_remote(r.id, r.policy.enabled);
            // A session that already failed to resume at startup needs reauth even if the remote is (auto-)paused and so never gets a pass that would discover it.
            if self.backend.needs_reauth(&r.name) {
                self.state.auth_failure(r.id);
            }
            let ids: Vec<SyncDirId> = sync_dirs.iter().filter(|d| d.remote_id == r.id).map(|d| d.id).collect();
            self.state.set_dirs(r.id, &ids);
        }
        // Forget everything about deleted folders, so a stale Error can't keep colouring the remote's roll-up.
        let gone: Vec<SyncDirId> = self.sync_dirs.iter().map(|d| d.id).filter(|id| !sync_dirs.iter().any(|d| d.id == *id)).collect();
        if !gone.is_empty() {
            let mut logs = self.logs.lock().unwrap();
            for id in &gone {
                logs.remove(id);
                self.pass_lines.remove(id);
                self.problems.remove(id);
                if self.conflicts.remove(id).is_some() {
                    self.conflicts_version += 1;
                }
                for dirs in self.resolutions.values_mut() {
                    dirs.remove(id);
                }
            }
            self.log_version += 1;
        }
        self.remotes = remotes;
        self.sync_dirs = sync_dirs;
    }

    /// Enabled by the user and not blocked on reauthentication.
    fn is_schedulable(&self, id: RemoteId) -> bool {
        self.remotes.iter().any(|r| r.id == id && r.policy.enabled) && !self.state.needs_reauth(id)
    }

    /// Start a pass now, or queue one behind the running pass.
    fn sync_now(&mut self, id: RemoteId) {
        if !self.syncing.contains(&id) {
            self.start_sync(id);
            return;
        }
        // Leave a note on each folder so the click doesn't look lost.
        self.refresh_requested_after.insert(id);
        let queued: Vec<SyncDirId> = self.sync_dirs.iter().filter(|d| d.remote_id == id).map(|d| d.id).collect();
        for sd in queued {
            self.push_log_line(sd, "⟳ Refresh queued — starts after the current pass finishes.".to_owned());
        }
    }

    /// Start every remote whose interval is up. Remotes inside a backoff window skip the turn instead: the counter goes down and the interval starts over.
    pub fn start_due(&mut self, now: Instant) {
        let candidates: Vec<(RemoteId, std::time::Duration)> = self
            .remotes
            .iter()
            .filter(|r| self.is_schedulable(r.id) && !self.syncing.contains(&r.id))
            .map(|r| (r.id, r.policy.interval.duration()))
            .collect();
        for (id, interval) in candidates {
            if self.last_sync_at.get(&id).is_some_and(|t| now.duration_since(*t) < interval) {
                continue;
            }
            if self.state.should_skip_and_decrement(id) {
                self.last_sync_at.insert(id, now);
                continue;
            }
            self.start_sync(id);
        }
    }

    /// When the next remote is due; `None` when nothing is scheduled.
    pub fn next_due(&self) -> Option<Instant> {
        self.remotes
            .iter()
            .filter(|r| self.is_schedulable(r.id) && !self.syncing.contains(&r.id))
            .map(|r| self.last_sync_at.get(&r.id).map_or_else(Instant::now, |t| *t + r.policy.interval.duration()))
            .min()
    }

    /// Spawn a pass for one remote. No-op if one is running or the session is dead.
    fn start_sync(&mut self, id: RemoteId) {
        if self.syncing.contains(&id) || self.state.needs_reauth(id) || !self.remotes.iter().any(|r| r.id == id) {
            return;
        }
        // Reuse the remote's flag (flip it back) rather than swapping the Arc.
        let cancel = self.cancel_flags.entry(id).or_insert_with(|| Arc::new(AtomicBool::new(false))).clone();
        cancel.store(false, Ordering::Release);
        self.syncing.insert(id);
        let events = self.tx.clone();
        let pass = Pass {
            remote_id: id,
            repo: self.repo.clone(),
            backend: self.backend.clone(),
            stderr: self.stderr.clone(),
            cancel,
            // A choice that no longer fits is dropped by the pass and the conflict reported again.
            resolutions: self.resolutions.get(&id).cloned().unwrap_or_default(),
            emit: Arc::new(move |event| {
                let _ = events.send(Input::Event(event));
            }),
        };
        let done = self.tx.clone();
        tokio::spawn(async move {
            let verdict = pass.run().await;
            let _ = done.send(Input::PassFinished(id, verdict));
        });
    }

    /// Record the verdict, update the backoff and start a queued pass.
    fn finish_pass(&mut self, id: RemoteId, verdict: PassVerdict) {
        self.syncing.remove(&id);
        self.passes_finished += 1;
        if !self.remotes.iter().any(|r| r.id == id) {
            return;
        }
        self.state.finish_pass(id);
        self.last_sync_at.insert(id, Instant::now());
        match verdict {
            PassVerdict::Clean => self.state.on_clean_pass(id),
            // Linear backoff: skip N cycles after the N-th consecutive degraded pass; resets with the first clean one. Rclone already backs off exponentially; this just stops us hammering.
            PassVerdict::Degraded => {
                self.state.on_degraded_pass(id);
            }
            // A cancel or list error isn't a sign the backend is overloaded.
            PassVerdict::Aborted => {}
        }
        if self.refresh_requested_after.remove(&id) {
            self.start_sync(id);
        }
    }

    /// Route a single sync event through the state machine and the log.
    fn handle_event(&mut self, event: SyncEvent) {
        match event {
            SyncEvent::SyncDirStatus { sync_dir_id, text, .. } => self.push_log_line(sync_dir_id, text),
            SyncEvent::SyncDirPending { sync_dir_id, text, .. } => self.push_log_line(sync_dir_id, format!("⟳ {text}")),
            SyncEvent::SyncDirError { remote_id, sync_dir_id, error } => {
                let line = match &error {
                    SyncError::General(path, msg) => format!("⚠ {path}: {msg}"),
                    SyncError::BothMoreCurrent(local, remote) => format!("⚠ Conflict: '{local}' vs '{remote}'"),
                };
                // HTTP 401 and the matching rclone phrasing mean the session is dead and only reauth fixes it. Flag the whole remote (the session is per remote) and stop the running pass. The flag alone keeps the scheduler off the remote; the user's Enabled setting stays, so a successful reauth resumes syncing without a detour through the settings.
                let auth_failure = matches!(&error, SyncError::General(_, msg) if is_auth_failure(msg));
                self.push_log_line(sync_dir_id, line);
                if auth_failure {
                    self.state.auth_failure(remote_id);
                    if let Some(flag) = self.cancel_flags.get(&remote_id) {
                        flag.store(true, Ordering::Release);
                    }
                    self.refresh_requested_after.remove(&remote_id);
                } else {
                    // Per-file errors show as Warning even if the pass ends Synced.
                    self.state.transition_dir(remote_id, sync_dir_id, RunState::Warning);
                }
            }
            SyncEvent::SyncDirStateChanged { remote_id, sync_dir_id, state } => {
                // Every pass starts by listing; from here on only this pass's lines describe what's going on.
                if state == RunState::Syncing(SyncActivity::Listing) {
                    self.pass_lines.remove(&sync_dir_id);
                }
                self.state.transition_dir(remote_id, sync_dir_id, state);
            }
            SyncEvent::SyncDirConflicts { remote_id, sync_dir_id, conflicts, resolutions } => self.conflicts_reported(remote_id, sync_dir_id, conflicts, resolutions),
            SyncEvent::RemoteStarted { .. } | SyncEvent::RemoteCompleted { .. } | SyncEvent::RemoteFailed { .. } | SyncEvent::FileProgress { .. } => {}
        }
    }

    /// A pass reported the complete conflict list of a folder. New entries are logged once (not on every pass); files the user already decided on stay hidden until a pass that was given that choice reports back.
    fn conflicts_reported(&mut self, remote_id: RemoteId, sync_dir_id: SyncDirId, mut conflicts: Vec<Conflict>, handled: Vec<Resolution>) {
        if let Some(pending) = self.resolutions.get_mut(&remote_id).and_then(|dirs| dirs.get_mut(&sync_dir_id)) {
            pending.retain(|r| !handled.contains(r));
            conflicts.retain(|c| !pending.iter().any(|r| r.remote_path == c.remote_path));
        }
        let known = self.conflicts.remove(&sync_dir_id).unwrap_or_default();
        for c in &conflicts {
            if !known.iter().any(|k| k.remote_path == c.remote_path) {
                let prefix = CONFLICT_LINE_PREFIXES[usize::from(!c.first_sync)];
                self.push_log_line(sync_dir_id, format!("{prefix}: {}", fmt_home(&c.local_path)));
            }
        }
        if !conflicts.is_empty() {
            self.state.transition_dir(remote_id, sync_dir_id, RunState::Warning);
            self.conflicts.insert(sync_dir_id, conflicts);
        }
        self.conflicts_version += 1;
    }

    /// Store the user's choice for the next pass, hide the conflict and start that pass.
    fn resolve(&mut self, remote_id: RemoteId, sync_dir_id: SyncDirId, resolution: Resolution) {
        if let Some(list) = self.conflicts.get_mut(&sync_dir_id) {
            list.retain(|c| c.remote_path != resolution.remote_path);
            // Nothing left to decide: drop the Warning the conflicts caused (it would outlast the next Synced) until the pass that applies the choices reports.
            if list.is_empty() {
                self.conflicts.remove(&sync_dir_id);
                self.state.transition_dir(remote_id, sync_dir_id, RunState::Waiting);
            }
            self.conflicts_version += 1;
        }
        let pending = self.resolutions.entry(remote_id).or_default().entry(sync_dir_id).or_default();
        pending.retain(|r| r.remote_path != resolution.remote_path);
        pending.push(resolution);
        self.sync_now(remote_id);
    }

    /// Append a line to the folder's log and drop the oldest beyond [`MAX_LOG_LINES`].
    fn push_log_line(&mut self, sync_dir_id: SyncDirId, line: String) {
        self.pass_lines.insert(sync_dir_id, line.clone());
        if line.starts_with('⚠') && !CONFLICT_LINE_PREFIXES.iter().any(|p| line.starts_with(p)) {
            self.problems.insert(sync_dir_id, line.clone());
        }
        let mut logs = self.logs.lock().unwrap();
        let lines = logs.entry(sync_dir_id).or_default();
        lines.push_back(LogLine { at: local_clock(), text: line });
        while lines.len() > MAX_LOG_LINES {
            lines.pop_front();
        }
        self.log_version += 1;
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.state.clone(),
            syncing: self.syncing.clone(),
            last_sync_at: self.last_sync_at.clone(),
            conflicts: self.conflicts.clone(),
            pass_lines: self.pass_lines.clone(),
            problems: self.problems.clone(),
            log_version: self.log_version,
            conflicts_version: self.conflicts_version,
            passes_finished: self.passes_finished,
        }
    }

    /// The one-line summary, when it differs from the last call's.
    pub fn take_status_change(&mut self) -> Option<TrayStatus> {
        let status = tray::compute_status(&self.state, &self.remotes, &self.syncing, &self.last_sync_at);
        if self.last_status.as_ref() == Some(&status) {
            return None;
        }
        self.last_status = Some(status.clone());
        Some(status)
    }
}
