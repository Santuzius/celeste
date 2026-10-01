//! Execute a `Vec<Action>` produced by the planner. Surfaces per-action
//! status / error events through the caller's `emit` callback; bails out
//! between actions when the cancel token trips.

use std::{fs, path::Path, time::SystemTime};

use crate::{
    domain::{
        events::SyncEvent,
        ports::{BackendClient, Cancel, Repository, is_cancelled as cancel_check},
        remote::Remote,
        run_state::{RunState, SyncActivity},
        sync::{SyncDir, SyncError},
    },
    util,
};

use super::planner::Action;
use super::snapshot::Snapshot;

/// Returns how many actions failed.
pub(super) fn apply<FE>(
    actions: Vec<Action>,
    snapshot: &Snapshot,
    remote: &Remote,
    sync_dir: &SyncDir,
    repo: &dyn Repository,
    client: &dyn BackendClient,
    emit: &FE,
    cancel: &Cancel,
) -> usize
where
    FE: Fn(SyncEvent) + Clone,
{
    let is_cancelled = || cancel_check(cancel);
    let failures = std::cell::Cell::new(0usize);
    let total = actions.len();
    let emit_status = |text: String| {
        emit(SyncEvent::SyncDirStatus {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            text,
        });
    };
    let emit_pending_local = |text: String| {
        emit(SyncEvent::SyncDirPending {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            text,
        });
    };
    let emit_error = |error: SyncError| {
        failures.set(failures.get() + 1);
        emit(SyncEvent::SyncDirError {
            remote_id: remote.id,
            sync_dir_id: sync_dir.id,
            error,
        });
    };

    // Actions arrive sorted by phase, so the activity only changes a
    // handful of times per pass — emit just those transitions.
    let mut current_activity: Option<SyncActivity> = None;
    for (idx, action) in actions.into_iter().enumerate() {
        if is_cancelled() {
            return failures.get();
        }
        if let Some(activity) = activity_of(&action)
            && current_activity != Some(activity)
        {
            current_activity = Some(activity);
            emit(SyncEvent::SyncDirStateChanged {
                remote_id: remote.id,
                sync_dir_id: sync_dir.id,
                state: RunState::Syncing(activity),
            });
        }
        // Refresh the phase-pending line roughly every 16 actions so a
        // long apply (e.g. 1800+ downloads on first sync of a large
        // remote) reports progress even when the per-action status is
        // changing faster than the eye can follow. Cheap — one event
        // per batch, same channel as everything else.
        if idx > 0 && idx.is_multiple_of(16) {
            emit_pending_local(tr::tr!(
                "Applying {} actions ({} done)…",
                total,
                idx
            ));
        }
        let pos = idx + 1;
        match action {
            Action::Upload {
                local_path,
                remote_path,
                is_dir,
            } => {
                if !Path::new(&local_path).exists() {
                    // Raced with a local delete between planning and now.
                    continue;
                }
                if is_dir {
                    if let Err(err) = client.mkdir(&remote.name, &remote_path, cancel) {
                        emit_error(SyncError::General(remote_path.clone(), err));
                        continue;
                    }
                } else {
                    emit_status(tr::tr!(
                        "[{}/{}] Uploading '{}'…",
                        pos,
                        total,
                        util::fmt_home(&local_path)
                    ));
                    if let Err(err) =
                        client.copy_to_remote(&local_path, &remote.name, &remote_path, cancel)
                    {
                        if !Path::new(&local_path).exists() {
                            eprintln!(
                                "sync: copy_to_remote raced with local delete for '{local_path}' — swallowing '{err}'.",
                            );
                            continue;
                        }
                        emit_error(SyncError::General(local_path.clone(), err));
                        continue;
                    }
                }
                record_upsert(repo, sync_dir, &local_path, &remote_path, client, &remote.name, cancel);
            }
            Action::Download {
                local_path,
                remote_path,
                is_dir,
            } => {
                if is_dir {
                    if !Path::new(&local_path).exists()
                        && let Err(err) = fs::create_dir_all(&local_path)
                    {
                        emit_error(SyncError::General(local_path.clone(), err.to_string()));
                        continue;
                    }
                } else {
                    if let Some(parent) = Path::new(&local_path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    emit_status(tr::tr!(
                        "[{}/{}] Downloading '{}'…",
                        pos,
                        total,
                        util::fmt_home(&local_path)
                    ));
                    if let Err(err) =
                        client.copy_to_local(&local_path, &remote.name, &remote_path, cancel)
                    {
                        emit_error(SyncError::General(remote_path.clone(), err));
                        continue;
                    }
                }
                record_upsert(repo, sync_dir, &local_path, &remote_path, client, &remote.name, cancel);
            }
            Action::DeleteLocal {
                local_path,
                remote_path,
                is_dir,
            } => {
                eprintln!(
                    "sync: DELETE mirror-local remote={} path={}",
                    remote.name, remote_path
                );
                emit_status(tr::tr!(
                    "[{}/{}] Removing '{}' locally…",
                    pos,
                    total,
                    util::fmt_home(&local_path)
                ));
                let res = if is_dir {
                    fs::remove_dir_all(&local_path)
                } else {
                    fs::remove_file(&local_path)
                };
                if let Err(err) = res {
                    emit_error(SyncError::General(local_path.clone(), err.to_string()));
                    continue;
                }
                let _ = util::await_future(repo.delete_sync_item_by_paths(
                    sync_dir.id,
                    &local_path,
                    &remote_path,
                ));
            }
            Action::DeleteRemote {
                local_path,
                remote_path,
                is_dir,
            } => {
                eprintln!(
                    "sync: DELETE mirror-remote remote={} path={}",
                    remote.name, remote_path
                );
                emit_status(tr::tr!(
                    "[{}/{}] Removing '{}' on remote…",
                    pos,
                    total,
                    remote_path
                ));
                let res = if is_dir {
                    client.purge(&remote.name, &remote_path, cancel)
                } else {
                    client.delete_file(&remote.name, &remote_path, cancel)
                };
                if let Err(err) = res {
                    emit_error(SyncError::General(remote_path.clone(), err));
                    continue;
                }
                let _ = util::await_future(repo.delete_sync_item_by_paths(
                    sync_dir.id,
                    &local_path,
                    &remote_path,
                ));
            }
            Action::ClearDbRow {
                local_path,
                remote_path,
            } => {
                let _ = util::await_future(repo.delete_sync_item_by_paths(
                    sync_dir.id,
                    &local_path,
                    &remote_path,
                ));
            }
            Action::Conflict {
                local_path,
                remote_path,
            } => {
                emit_error(SyncError::BothMoreCurrent(local_path, remote_path));
            }
        }
    }
    let _ = snapshot;
    failures.get()
}

fn activity_of(action: &Action) -> Option<SyncActivity> {
    match action {
        Action::Upload { .. } => Some(SyncActivity::Uploading),
        Action::Download { .. } => Some(SyncActivity::Downloading),
        Action::DeleteLocal { .. } | Action::DeleteRemote { .. } => Some(SyncActivity::Deleting),
        Action::Conflict { .. } => Some(SyncActivity::Resolving),
        Action::ClearDbRow { .. } => None,
    }
}

fn record_upsert(
    repo: &dyn Repository,
    sync_dir: &SyncDir,
    local_path: &str,
    remote_path: &str,
    client: &dyn BackendClient,
    remote_name: &str,
    cancel: &Cancel,
) {
    let Some(local_ts) = local_timestamp(Path::new(local_path)) else {
        return;
    };
    let Some(rstat) = client.stat(remote_name, remote_path, cancel).ok().flatten() else {
        return;
    };
    let remote_ts = rstat.mod_time.unix_timestamp();
    if let Some(existing) = util::await_future(
        repo.find_sync_item_by_paths(sync_dir.id, local_path, remote_path),
    )
    .unwrap_or(None)
    {
        let _ = util::await_future(repo.update_sync_item_timestamps(
            existing.id,
            local_ts as i64,
            remote_ts,
        ));
    } else {
        let _ = util::await_future(repo.insert_sync_item(
            sync_dir.id,
            local_path.to_owned(),
            remote_path.to_owned(),
            local_ts as i64,
            remote_ts,
        ));
    }
}

fn local_timestamp(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
