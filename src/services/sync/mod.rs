//! Snapshot-based sync engine.
//!
//! Three submodules cover the three phases of one pass:
//!
//! 1. [`snapshot`] fetches the authoritative remote listing
//!    ([`BackendClient::list`] recursive), walks the local tree, and loads
//!    the DB rows. Bails out on list errors; rate-limit handling lives in
//!    [`run`]'s stderr-tap check around the build call.
//!
//! 2. [`planner`] turns the snapshot into a `Vec<Action>` in a pure function.
//!    Every destructive decision is reducible to a `(local, remote, db)`
//!    triple and lands in exactly one branch.
//!
//! 3. [`applier`] executes the actions. Upload failures re-check whether
//!    the source file raced away (user deleted between planning and the
//!    backend reading it) and silently skip in that case.

use std::time::Instant;

use crate::domain::{
    events::SyncEvent,
    ports::{BackendClient, Cancel, Repository, is_cancelled as cancel_check},
    remote::Remote,
    run_state::{RunState, SyncActivity},
    sync::{SyncDir, SyncError},
};

mod applier;
mod planner;
mod snapshot;

#[cfg(test)]
mod tests;

/// Outcome of a single [`run`] call.
///
/// - `Synced`: the plan was applied in full.
/// - `Aborted`: the pass refused to act (cancelled, or list error).
/// - `Degraded`: the pass detected provider rate-limiting (e.g. Proton
///   Drive's `status=429` retry warnings in stderr) and skipped the
///   apply step. Drives the scheduler's linear backoff so we stop
///   hammering a distressed API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Synced,
    Aborted,
    Degraded,
}

/// Entry point. Builds the snapshot, plans, applies. `cancel` is polled
/// at the start of each destructive action so the pass can bail out
/// promptly when the user disables the remote or shuts down the app —
/// in-flight client calls still run to completion (we can't interrupt
/// `copy_to_remote` cleanly), but nothing new fires.
pub fn run<FE, FD>(
    remote: &Remote,
    sync_dir: &SyncDir,
    repo: &dyn Repository,
    client: &dyn BackendClient,
    all_sync_dirs: &[SyncDir],
    emit: FE,
    cancel: &Cancel,
    rate_limit_seen_since: FD,
) -> Outcome
where
    FE: Fn(SyncEvent) + Clone,
    FD: Fn(Instant) -> bool + Clone,
{
    let is_cancelled = || cancel_check(cancel);
    let pass_start = Instant::now();
    let emit_pending = |text: String| {
        emit(SyncEvent::SyncDirPending {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            text,
        });
    };
    let emit_error = |error: SyncError| {
        emit(SyncEvent::SyncDirError {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            error,
        });
    };
    let emit_status = |text: String| {
        emit(SyncEvent::SyncDirStatus {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            text,
        });
    };
    let emit_state = |state: RunState| {
        emit(SyncEvent::SyncDirStateChanged {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            state,
        });
    };

    emit_state(RunState::Syncing(SyncActivity::Listing));
    let snapshot_result =
        snapshot::Snapshot::build(remote, sync_dir, repo, client, all_sync_dirs, cancel);

    // Classify rate-limit *before* we commit to a success/failure path:
    // list failures caused by quota exhaustion still need to route
    // through the Degraded / backoff branch, not be treated as plain
    // network errors that retry at the normal cadence.
    let rate_limited_during_list = rate_limit_seen_since(pass_start);
    if rate_limited_during_list {
        eprintln!(
            "sync: DEGRADED for '{}' — rate-limit warnings observed during listing; skipping this pass for backoff.",
            remote.name,
        );
        emit_error(SyncError::General(
            sync_dir.remote_path.clone(),
            tr::tr!("Rate-limit warnings detected during listing; skipping this pass for backoff."),
        ));
        emit_status(tr::tr!("Sync skipped — provider rate-limited."));
        emit_state(RunState::Warning);
        return Outcome::Degraded;
    }

    let snapshot = match snapshot_result {
        Ok(s) => s,
        Err(err) => {
            eprintln!("sync: list failed for {}: {err}", remote.name);
            // The error line + Error icon already tell the user what
            // happened; a separate "will retry" status was redundant
            // and made the green/red mismatch jarring when the next
            // tick succeeded but the line stuck around in the log.
            emit_error(SyncError::General(sync_dir.remote_path.clone(), err));
            emit_state(RunState::Error);
            return Outcome::Aborted;
        }
    };

    if is_cancelled() {
        emit_status(tr::tr!("Sync cancelled."));
        return Outcome::Aborted;
    }
    let actions = planner::plan(&snapshot, sync_dir);
    planner::log_plan_summary(remote, sync_dir, &snapshot, &actions);
    // Replace the "Listing remote…" pending with a phase-level
    // description of what apply() is about to do. Without this, a
    // multi-hundred-action pass (e.g. first-ever sync of a large
    // remote) leaves the user staring at "Listing…" for as long as
    // the per-action status churn takes to dominate the UI.
    if !actions.is_empty() {
        emit_pending(tr::tr!(
            "Applying {} actions (0 done)…",
            actions.len()
        ));
    }
    applier::apply(
        actions,
        &snapshot,
        remote,
        sync_dir,
        repo,
        client,
        &emit,
        cancel,
    );

    // Checkpoint any auth-token rotation that happened during listing or
    // apply so it lands in the keyring before the process can exit. A
    // no-op for rclone backends; the native Proton client re-persists a
    // rotated refresh token here. Cheap when nothing rotated.
    client.checkpoint_session(&remote.name);

    if is_cancelled() {
        emit_status(tr::tr!("Sync cancelled."));
        return Outcome::Aborted;
    }
    // Post-apply check: even if the pass reached the end cleanly, a
    // rate-limit warning at any point during upload/download means the
    // backend was stressed. Flag Degraded so the scheduler backs off
    // before the next tick — we don't undo the work we already did.
    if rate_limit_seen_since(pass_start) {
        eprintln!(
            "sync: pass for '{}' completed but rate-limit warnings surfaced during apply; flagging Degraded for backoff.",
            remote.name,
        );
        emit_status(tr::tr!(
            "Files are synced — provider rate-limited, backing off next tick."
        ));
        emit_state(RunState::Warning);
        return Outcome::Degraded;
    }
    emit_state(RunState::Synced);
    Outcome::Synced
}
