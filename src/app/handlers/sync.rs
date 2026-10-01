//! Sync lifecycle handlers: scheduler tick, pass start/finish, and the
//! `start_sync` worker spawn.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

use iced::Task;

use crate::{
    domain::{events::SyncEvent, remote::RemoteId},
    services::sync::Outcome,
};

use super::super::{CelesteApp, Message, PassVerdict};

impl CelesteApp {
    /// Handle [`Message::SyncStarted`] — mark the remote as in-flight.
    pub(in crate::app) fn handle_sync_started(&mut self, id: RemoteId) -> Task<Message> {
        self.syncing.insert(id);
        Task::none()
    }

    /// Handle [`Message::SyncFinished`] — record the verdict, decay
    /// counters, and kick a queued refresh if one is pending.
    pub(in crate::app) fn handle_sync_finished(
        &mut self,
        id: RemoteId,
        verdict: PassVerdict,
    ) -> Task<Message> {
        self.syncing.remove(&id);
        self.sync_state.finish_pass(id);
        self.last_sync_at.insert(id, Instant::now());
        match verdict {
            PassVerdict::Clean => {
                self.sync_state.on_clean_pass(id);
            }
            PassVerdict::Degraded => {
                // Linear backoff: skip N cycles after the N-th
                // consecutive degraded pass. N=1 the first time, N=2
                // the next, and so on — resets the moment a pass lands
                // clean. Rclone already does exponential on its side;
                // the linear layer just stops us hammering.
                self.sync_state.on_degraded_pass(id);
            }
            PassVerdict::Aborted => {
                // Intentionally leave counters as-is: an abort caused
                // by cancel / list error isn't a signal the backend is
                // overloaded.
            }
        }
        // If the user clicked Refresh now while we were already
        // syncing, honour that click now.
        if self.refresh_requested_after.remove(&id) {
            self.start_sync(id)
        } else {
            Task::none()
        }
    }

    /// Handle [`Message::Tick`] — fire syncs for every enabled remote
    /// whose interval has elapsed and that isn't already in flight.
    pub(in crate::app) fn handle_tick(&mut self) -> Task<Message> {
        // Check each enabled remote; if its interval has elapsed and
        // it's not already syncing, kick off a new pass. Remotes
        // currently inside a backoff window have `syncs_to_skip > 0` —
        // we decrement, stamp last_sync_at, and skip this cycle so the
        // next interval's tick does the same until the counter hits
        // zero.
        let now = Instant::now();
        let mut due: Vec<RemoteId> = Vec::new();
        let remote_ids: Vec<(RemoteId, std::time::Duration)> = self
            .remotes
            .iter()
            .filter(|r| r.policy.enabled && !self.syncing.contains(&r.id))
            .map(|r| (r.id, r.policy.interval.duration()))
            .collect();
        for (id, interval) in remote_ids {
            let elapsed = self
                .last_sync_at
                .get(&id)
                .map(|t| now.duration_since(*t))
                .unwrap_or(interval);
            if elapsed < interval {
                continue;
            }
            if self.sync_state.should_skip_and_decrement(id) {
                // Advance the baseline so we wait another full interval
                // before the next skip decision.
                self.last_sync_at.insert(id, now);
                continue;
            }
            due.push(id);
        }
        let cmds: Vec<Task<Message>> =
            due.into_iter().map(|id| self.start_sync(id)).collect();
        Task::batch(cmds)
    }

    /// Spawn a sync pass for one remote. No-op if already syncing.
    /// Marks the remote as in-flight so the sidebar shows "(syncing…)"
    /// and returns a Command that will deliver `SyncFinished(id)` when
    /// the blocking task completes.
    pub(in crate::app) fn start_sync(&mut self, id: RemoteId) -> Task<Message> {
        if self.syncing.contains(&id) {
            return Task::none();
        }
        // Fresh cancel flag for this pass. Reusing the existing Arc
        // lets any stored reference remain wired up (we flip-flop the
        // bool rather than swap the Arc).
        let flag = self
            .cancel_flags
            .entry(id)
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone();
        flag.store(false, Ordering::Release);
        self.syncing.insert(id);
        let repo = self.repo.clone();
        let rclone = self.rclone.clone();
        let events_tx = self.events_tx.clone();
        let stderr_capture = self.stderr_capture.clone();
        Task::perform(
            async move {
                let remote = match repo.find_remote(id).await {
                    Ok(Some(r)) => r,
                    _ => return (id, PassVerdict::Aborted),
                };
                let sync_dirs = repo.list_sync_dirs(id).await.unwrap_or_default();
                let all_sync_dirs = repo.list_all_sync_dirs().await.unwrap_or_default();
                let verdict = tokio::task::spawn_blocking(move || {
                    let emit = move |event: SyncEvent| {
                        if let Some(tx) = &events_tx {
                            let _ = tx.blocking_send(event);
                        }
                    };
                    // The stderr probe: only rclone-transport backends
                    // emit rate-limit warnings to stderr. Native Proton
                    // reports throttling via its own error paths. For
                    // rclone backends, any line matching the rclone
                    // translator's marker set flips the pass to Degraded.
                    use crate::infrastructure::translators::rclone::RATE_LIMIT_MARKERS;
                    let use_stderr_probe = remote
                        .provider_kind
                        .map_or(true, |k| k.uses_rclone_transport());
                    let stderr_for_probe = stderr_capture.clone();
                    let rate_limit_seen_since = move |since: Instant| -> bool {
                        if !use_stderr_probe {
                            return false;
                        }
                        stderr_for_probe.any_line_since(since, |line| {
                            RATE_LIMIT_MARKERS
                                .iter()
                                .any(|m| m.iter().all(|needle| line.contains(needle)))
                        })
                    };
                    let mut any_degraded = false;
                    let mut any_error = false;
                    let mut any_synced = false;
                    for sd in sync_dirs {
                        if flag.load(Ordering::Acquire) {
                            break;
                        }
                        match crate::services::sync::run(
                            &remote,
                            &sd,
                            &*repo,
                            &*rclone,
                            &all_sync_dirs,
                            emit.clone(),
                            &flag,
                            rate_limit_seen_since.clone(),
                        ) {
                            Outcome::Synced => any_synced = true,
                            Outcome::Degraded => any_degraded = true,
                            Outcome::Aborted => any_error = true,
                        }
                    }
                    if any_degraded {
                        PassVerdict::Degraded
                    } else if any_synced {
                        PassVerdict::Clean
                    } else if any_error {
                        PassVerdict::Aborted
                    } else {
                        // No sync_dirs to run (or everything cancelled
                        // before the first). Treat as clean-ish — no
                        // reason to accrue backoff.
                        PassVerdict::Clean
                    }
                })
                .await
                .unwrap_or(PassVerdict::Aborted);
                (id, verdict)
            },
            |(id, v)| Message::SyncFinished(id, v),
        )
    }

    /// Time until the scheduler will next attempt this remote, plus a
    /// flag telling the caller whether the remote is currently in a
    /// backoff window (next attempt will be a skip, not a real pass).
    /// Returns `None` when the remote is disabled.
    pub fn next_sync_eta(&self, id: RemoteId) -> Option<(std::time::Duration, bool)> {
        let remote = self.remotes.iter().find(|r| r.id == id)?;
        if !remote.policy.enabled {
            return None;
        }
        let interval = remote.policy.interval.duration();
        let now = Instant::now();
        let base_remaining = match self.last_sync_at.get(&id) {
            Some(t) => {
                let elapsed = now.duration_since(*t);
                interval.saturating_sub(elapsed)
            }
            None => std::time::Duration::ZERO,
        };
        Some((base_remaining, self.sync_state.in_backoff(id)))
    }
}
