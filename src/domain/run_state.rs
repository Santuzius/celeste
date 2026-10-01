//! Hierarchical sync run-state: Dir → Remote → App.
//!
//! `RunState` is the single enum for all levels. The sync engine only ever reports per-dir `Syncing` / `Synced` / `Warning` / `Error`; `Paused` and `AuthNeeded` are remote-level conditions (policy disabled, session dead) that are layered on top when a state is *displayed*, so they can't be clobbered by a late engine event and never need restoring afterwards.

use std::collections::HashMap;

use super::{remote::RemoteId, sync::SyncDirId};

// ---------------------------------------------------------------------------
// SyncActivity / RunState
// ---------------------------------------------------------------------------

/// What the sync engine is currently doing inside a `Syncing` pass.
/// Carried inside `RunState::Syncing` so the UI can show a structured activity label instead of parsing log strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncActivity {
    Listing,
    Downloading,
    Uploading,
    Deleting,
    Resolving,
}

/// Coarse run-state for a sync directory (or its roll-up at remote/app level).
///
/// Severity ordering (low → high): Waiting < Paused < Synced < Syncing < Warning < Error < AuthNeeded. Roll-up at the remote level takes the maximum over the children; the remote-level conditions (auth, disabled) override it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Fresh before the first pass, or a pass was interrupted.
    Waiting,
    /// The remote is disabled (by the user, or auto-paused on an auth failure).
    Paused,
    /// The remote's session is dead; the user must reauthenticate.
    AuthNeeded,
    /// A pass just completed with no errors.
    Synced,
    /// A sync pass is currently running.
    Syncing(SyncActivity),
    /// Last pass degraded (rate-limited, per-file errors, conflicts).
    /// Sticky within a pass — a later clean `Synced` does not clear it; the next pass's `Syncing` does.
    Warning,
    /// Last pass aborted (listing failed, etc.). Sticky like `Warning`.
    Error,
}

impl RunState {
    fn severity(self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Paused => 1,
            Self::Synced => 2,
            Self::Syncing(_) => 3,
            Self::Warning => 4,
            Self::Error => 5,
            Self::AuthNeeded => 6,
        }
    }

    /// Whether `self` should remain instead of being replaced by `next`.
    /// Warning and Error survive a later Synced — a clean apply step does not erase a per-file error that fired earlier in the same pass.
    fn is_sticky_over(self, next: RunState) -> bool {
        matches!((self, next), (RunState::Warning | RunState::Error, RunState::Synced))
    }

    /// Something the user should look at.
    pub fn is_problem(self) -> bool {
        matches!(self, Self::Warning | Self::Error | Self::AuthNeeded)
    }
}

// ---------------------------------------------------------------------------
// RemoteState
// ---------------------------------------------------------------------------

/// Per-remote state: per-dir engine states plus the remote-level conditions and backoff counters.
#[derive(Clone, Debug)]
pub struct RemoteState {
    /// Last state the engine reported per dir.
    pub dirs: HashMap<SyncDirId, RunState>,
    /// Mirrors `remote.policy.enabled`.
    pub enabled: bool,
    /// The remote's session is dead; only a reauthentication clears this.
    pub auth_needed: bool,
    /// Consecutive degraded-pass count; drives the linear backoff schedule.
    pub consecutive_degraded: u32,
    /// Scheduler cycles remaining to skip before the next pass attempt.
    pub syncs_to_skip: u32,
}

impl RemoteState {
    pub fn new(enabled: bool) -> Self {
        Self {
            dirs: HashMap::new(),
            enabled,
            auth_needed: false,
            consecutive_degraded: 0,
            syncs_to_skip: 0,
        }
    }

    /// Remote-level condition that overrides every dir state, if any.
    fn override_state(&self) -> Option<RunState> {
        if self.auth_needed {
            Some(RunState::AuthNeeded)
        } else if !self.enabled {
            Some(RunState::Paused)
        } else {
            None
        }
    }

    /// Aggregate child dir states into a single `RunState` for this remote.
    pub fn roll_up(&self) -> RunState {
        self.override_state().unwrap_or_else(|| {
            self.dirs
                .values()
                .copied()
                .max_by_key(|s| s.severity())
                .unwrap_or(RunState::Waiting)
        })
    }

    /// The state to display for one dir.
    pub fn dir_display_state(&self, id: SyncDirId) -> RunState {
        self.override_state()
            .unwrap_or_else(|| self.dirs.get(&id).copied().unwrap_or(RunState::Waiting))
    }

    /// Transition dir `id` to `next`, respecting the stickiness rule.
    pub fn transition_dir(&mut self, id: SyncDirId, next: RunState) {
        let entry = self.dirs.entry(id).or_insert(RunState::Waiting);
        if !entry.is_sticky_over(next) {
            *entry = next;
        }
    }

    /// Keep only the given dirs (drops state of deleted sync dirs so it can't haunt the roll-up) and add missing ones as `Waiting`.
    pub fn set_dirs(&mut self, ids: &[SyncDirId]) {
        self.dirs.retain(|id, _| ids.contains(id));
        for id in ids {
            self.dirs.entry(*id).or_insert(RunState::Waiting);
        }
    }

    /// A pass ended (cleanly, cancelled, or crashed). Any dir still marked `Syncing` never got its final state — don't leave it spinning.
    pub fn finish_pass(&mut self) {
        for state in self.dirs.values_mut() {
            if matches!(state, RunState::Syncing(_)) {
                *state = RunState::Waiting;
            }
        }
    }

    /// Reauthentication succeeded. The `Error`s left by the failed attempts are stale now.
    pub fn reauth_complete(&mut self) {
        self.auth_needed = false;
        for state in self.dirs.values_mut() {
            if *state == RunState::Error {
                *state = RunState::Waiting;
            }
        }
    }

    /// Record a degraded pass. Increments `consecutive_degraded` and sets `syncs_to_skip = consecutive_degraded` for linear backoff.
    /// Returns the new `consecutive_degraded` value.
    pub fn on_degraded_pass(&mut self) -> u32 {
        self.consecutive_degraded = self.consecutive_degraded.saturating_add(1);
        self.syncs_to_skip = self.consecutive_degraded;
        self.consecutive_degraded
    }

    /// Record a clean pass. Resets both backoff counters.
    pub fn on_clean_pass(&mut self) {
        self.consecutive_degraded = 0;
        self.syncs_to_skip = 0;
    }

    /// Check whether the scheduler should skip this tick. If so, decrements `syncs_to_skip` and returns `true`.
    pub fn should_skip_and_decrement(&mut self) -> bool {
        if self.syncs_to_skip == 0 {
            return false;
        }
        self.syncs_to_skip -= 1;
        true
    }

    pub fn any_degraded(&self) -> bool {
        self.consecutive_degraded > 0
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Application-level sync state. The single source of truth for all per-dir and per-remote run-states.
#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub remotes: HashMap<RemoteId, RemoteState>,
}

impl AppState {
    pub fn new() -> Self {
        Self::default()
    }

    fn remote_mut(&mut self, id: RemoteId) -> &mut RemoteState {
        self.remotes.entry(id).or_insert_with(|| RemoteState::new(true))
    }

    /// Create a `RemoteState` entry for `id` if one does not yet exist, and synchronise `enabled`.
    pub fn ensure_remote(&mut self, id: RemoteId, enabled: bool) {
        self.remote_mut(id).enabled = enabled;
    }

    /// Replace the known dir set of `remote_id` (see [`RemoteState::set_dirs`]).
    pub fn set_dirs(&mut self, remote_id: RemoteId, ids: &[SyncDirId]) {
        self.remote_mut(remote_id).set_dirs(ids);
    }

    /// Transition `dir_id` inside `remote_id` to `next`, respecting the stickiness rule.
    pub fn transition_dir(&mut self, remote_id: RemoteId, dir_id: SyncDirId, next: RunState) {
        self.remote_mut(remote_id).transition_dir(dir_id, next);
    }

    /// Mark `remote_id`'s session as dead.
    pub fn auth_failure(&mut self, remote_id: RemoteId) {
        self.remote_mut(remote_id).auth_needed = true;
    }

    /// Reauthentication for `remote_id` succeeded.
    pub fn reauth_complete(&mut self, remote_id: RemoteId) {
        if let Some(rs) = self.remotes.get_mut(&remote_id) {
            rs.reauth_complete();
        }
    }

    /// A pass for `remote_id` ended.
    pub fn finish_pass(&mut self, remote_id: RemoteId) {
        if let Some(rs) = self.remotes.get_mut(&remote_id) {
            rs.finish_pass();
        }
    }

    /// Update the `enabled` flag for a remote (mirrors policy changes).
    pub fn set_remote_enabled(&mut self, id: RemoteId, enabled: bool) {
        if let Some(rs) = self.remotes.get_mut(&id) {
            rs.enabled = enabled;
        }
    }

    /// Remove all state for a deleted remote.
    pub fn remove_remote(&mut self, id: RemoteId) {
        self.remotes.remove(&id);
    }

    /// Record a degraded pass for `id`. Returns the new `consecutive_degraded` (for logging).
    pub fn on_degraded_pass(&mut self, id: RemoteId) -> u32 {
        self.remote_mut(id).on_degraded_pass()
    }

    /// Record a clean pass for `id`.
    pub fn on_clean_pass(&mut self, id: RemoteId) {
        if let Some(rs) = self.remotes.get_mut(&id) {
            rs.on_clean_pass();
        }
    }

    /// Returns `true` and decrements the skip counter if this remote's scheduler tick should be skipped.
    pub fn should_skip_and_decrement(&mut self, id: RemoteId) -> bool {
        self.remotes
            .get_mut(&id)
            .is_some_and(|rs| rs.should_skip_and_decrement())
    }

    /// `true` when `id` is inside a backoff window.
    pub fn in_backoff(&self, id: RemoteId) -> bool {
        self.remotes.get(&id).is_some_and(|rs| rs.syncs_to_skip > 0)
    }

    /// `true` when `remote_id` needs reauthentication.
    pub fn needs_reauth(&self, remote_id: RemoteId) -> bool {
        self.remotes.get(&remote_id).is_some_and(|rs| rs.auth_needed)
    }

    /// Roll-up for one remote; `None` before the remote is known.
    pub fn roll_up(&self, remote_id: RemoteId) -> Option<RunState> {
        self.remotes.get(&remote_id).map(RemoteState::roll_up)
    }

    /// Display state for one dir of `remote_id`.
    pub fn dir_state(&self, remote_id: RemoteId, dir_id: SyncDirId) -> RunState {
        self.remotes
            .get(&remote_id)
            .map_or(RunState::Waiting, |rs| rs.dir_display_state(dir_id))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(id: i32) -> SyncDirId {
        SyncDirId(id)
    }

    #[test]
    fn severity_ordering_is_monotone() {
        let states = [
            RunState::Waiting,
            RunState::Paused,
            RunState::Synced,
            RunState::Syncing(SyncActivity::Listing),
            RunState::Warning,
            RunState::Error,
            RunState::AuthNeeded,
        ];
        for w in states.windows(2) {
            assert!(w[0].severity() < w[1].severity(), "{w:?} — severity not strictly increasing");
        }
    }

    #[test]
    fn warning_and_error_are_sticky_over_synced() {
        assert!(RunState::Warning.is_sticky_over(RunState::Synced));
        assert!(RunState::Error.is_sticky_over(RunState::Synced));
        assert!(!RunState::Syncing(SyncActivity::Listing).is_sticky_over(RunState::Synced));
        assert!(!RunState::Warning.is_sticky_over(RunState::Error));
        assert!(!RunState::Error.is_sticky_over(RunState::Syncing(SyncActivity::Listing)));
    }

    #[test]
    fn roll_up_takes_max_child() {
        let mut rs = RemoteState::new(true);
        rs.transition_dir(dir(1), RunState::Synced);
        rs.transition_dir(dir(2), RunState::Warning);
        assert_eq!(rs.roll_up(), RunState::Warning);
    }

    #[test]
    fn disabled_remote_rolls_up_and_displays_paused() {
        let mut rs = RemoteState::new(false);
        rs.transition_dir(dir(1), RunState::Error);
        assert_eq!(rs.roll_up(), RunState::Paused);
        assert_eq!(rs.dir_display_state(dir(1)), RunState::Paused);
    }

    #[test]
    fn auth_needed_survives_later_engine_events() {
        // The engine reports `Error` right after the auth failure (the listing failed) and possibly `Synced` for a sibling that was mid-pass. Neither may hide the reauth requirement.
        let mut rs = RemoteState::new(true);
        rs.auth_needed = true;
        rs.enabled = false; // auto-pause
        rs.transition_dir(dir(1), RunState::Error);
        rs.transition_dir(dir(2), RunState::Synced);
        assert_eq!(rs.roll_up(), RunState::AuthNeeded);
        assert_eq!(rs.dir_display_state(dir(2)), RunState::AuthNeeded);
    }

    #[test]
    fn reauth_clears_flag_and_stale_errors_only() {
        let mut rs = RemoteState::new(true);
        rs.auth_needed = true;
        rs.transition_dir(dir(1), RunState::Error);
        rs.transition_dir(dir(2), RunState::Warning);
        rs.reauth_complete();
        assert!(!rs.auth_needed);
        assert_eq!(rs.dirs[&dir(1)], RunState::Waiting);
        assert_eq!(rs.dirs[&dir(2)], RunState::Warning);
    }

    #[test]
    fn deleted_dirs_stop_affecting_roll_up() {
        let mut rs = RemoteState::new(true);
        rs.transition_dir(dir(1), RunState::Synced);
        rs.transition_dir(dir(2), RunState::Error);
        rs.set_dirs(&[dir(1), dir(3)]);
        assert_eq!(rs.roll_up(), RunState::Synced);
        assert_eq!(rs.dirs[&dir(3)], RunState::Waiting);
    }

    #[test]
    fn interrupted_pass_does_not_leave_dirs_syncing() {
        let mut rs = RemoteState::new(true);
        rs.transition_dir(dir(1), RunState::Syncing(SyncActivity::Uploading));
        rs.transition_dir(dir(2), RunState::Warning);
        rs.finish_pass();
        assert_eq!(rs.dirs[&dir(1)], RunState::Waiting);
        assert_eq!(rs.dirs[&dir(2)], RunState::Warning);
    }

    #[test]
    fn backoff_counters_increment_and_reset() {
        let mut rs = RemoteState::new(true);
        assert!(!rs.should_skip_and_decrement());

        rs.on_degraded_pass();
        assert_eq!(rs.consecutive_degraded, 1);
        assert!(rs.should_skip_and_decrement()); // skip 1
        assert!(!rs.should_skip_and_decrement()); // 0 left

        rs.on_degraded_pass();
        rs.on_degraded_pass();
        assert_eq!(rs.consecutive_degraded, 3);
        for _ in 0..3 {
            assert!(rs.should_skip_and_decrement());
        }
        assert!(!rs.should_skip_and_decrement());

        rs.on_clean_pass();
        assert_eq!(rs.consecutive_degraded, 0);
        assert!(!rs.should_skip_and_decrement());
    }
}
