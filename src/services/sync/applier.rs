//! Execute a `Vec<Action>` produced by the planner. Surfaces per-action
//! status / error events through the caller's `emit` callback; bails out
//! between actions when the cancel token trips.

use std::{collections::HashMap, fs, io::Read, path::Path, time::SystemTime};

use time::OffsetDateTime;

use crate::{
    domain::{
        events::SyncEvent,
        ports::{BackendClient, Cancel, Repository, is_cancelled as cancel_check},
        remote::Remote,
        run_state::{RunState, SyncActivity},
        sync::{Conflict, ConflictChoice, FileDetails, Resolution, SyncDir, SyncError, SyncItem},
    },
    util,
};

use super::planner::{Action, ConflictKind};
use super::snapshot::Snapshot;

/// What one apply step left behind.
pub(super) struct Applied {
    pub failures: usize,
    /// Conflicts still waiting for the user. Only complete when the pass wasn't cancelled.
    pub conflicts: Vec<Conflict>,
    pub cancelled: bool,
}

pub(super) fn apply<FE>(
    actions: Vec<Action>,
    snapshot: &Snapshot,
    remote: &Remote,
    sync_dir: &SyncDir,
    repo: &dyn Repository,
    client: &dyn BackendClient,
    resolutions: &[Resolution],
    emit: &FE,
    cancel: &Cancel,
) -> Applied
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
    let mut conflicts = Vec::new();
    for (idx, action) in actions.into_iter().enumerate() {
        if is_cancelled() {
            return Applied { failures: failures.get(), conflicts, cancelled: true };
        }
        // The listing may be a few seconds old (cached until the provider's change log reports something), so re-check the target right before anything gets overwritten or deleted.
        let mut action = preflight(action, &snapshot.db, remote, client, cancel);
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
        // A resolved conflict turns into a transfer: `continue 'action` runs that transfer, `break 'action` moves on to the next planned action.
        'action: loop {
        match std::mem::replace(&mut action, Action::ClearDbRow { local_path: String::new(), remote_path: String::new() }) {
            Action::Upload {
                local_path,
                remote_path,
                is_dir,
            } => {
                if !Path::new(&local_path).exists() {
                    // Raced with a local delete between planning and now.
                    break 'action;
                }
                if is_dir {
                    if let Err(err) = client.mkdir(&remote.name, &remote_path, cancel) {
                        emit_error(SyncError::General(remote_path.clone(), err));
                        break 'action;
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
                            break 'action;
                        }
                        emit_error(SyncError::General(local_path.clone(), err));
                        break 'action;
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
                        break 'action;
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
                        break 'action;
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
                    break 'action;
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
                    break 'action;
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
            Action::RecordDbRow {
                local_path,
                remote_path,
            } => {
                eprintln!("sync: RECORD '{remote_path}' — present on both sides but untracked; tracking it from now on.");
                record_upsert(repo, sync_dir, &local_path, &remote_path, client, &remote.name, cancel);
            }
            Action::Conflict {
                local_path,
                remote_path,
                kind,
            } => {
                let conflict = match inspect_conflict(&local_path, &remote_path, kind, remote, client, cancel) {
                    Inspected::Gone => break 'action,
                    Inspected::Failed(err) => {
                        emit_error(SyncError::General(remote_path.clone(), err));
                        break 'action;
                    }
                    Inspected::Identical => {
                        eprintln!("sync: '{remote_path}' is identical on both sides — recording it as synced.");
                        record_upsert(repo, sync_dir, &local_path, &remote_path, client, &remote.name, cancel);
                        break 'action;
                    }
                    Inspected::Differs(conflict) => conflict,
                };
                let chosen = resolutions
                    .iter()
                    .find(|r| r.remote_path == remote_path && r.local_stamp == conflict.local_stamp && r.remote_stamp == conflict.remote_stamp)
                    .map(|r| r.choice.clone())
                    // Never-synced pair on a backend without content digests: the newer copy wins, as before.
                    .or_else(|| {
                        (kind == ConflictKind::BothNew && conflict.remote.sha1.is_none()).then(|| {
                            if conflict.local_stamp > conflict.remote_stamp { ConflictChoice::KeepLocal } else { ConflictChoice::KeepRemote }
                        })
                    });
                match chosen {
                    None => {
                        eprintln!("sync: CONFLICT '{remote_path}' changed on both sides — waiting for the user.");
                        emit(SyncEvent::SyncDirStateChanged {
                            remote_id: remote.id,
                            sync_dir_id: sync_dir.id,
                            state: RunState::Warning,
                        });
                        conflicts.push(conflict);
                    }
                    Some(ConflictChoice::KeepLocal) => {
                        action = Action::Upload { local_path, remote_path, is_dir: false };
                        continue 'action;
                    }
                    Some(ConflictChoice::KeepRemote) => {
                        action = Action::Download { local_path, remote_path, is_dir: false };
                        continue 'action;
                    }
                    Some(ConflictChoice::KeepBoth { local_name }) => {
                        let renamed = Path::new(&local_path).with_file_name(&local_name);
                        if local_name.is_empty() || local_name.contains('/') || renamed.exists() {
                            emit_error(SyncError::General(local_path.clone(), tr::tr!("Can't keep both: '{}' is not a free file name.", local_name)));
                            break 'action;
                        }
                        if let Err(err) = fs::rename(&local_path, &renamed) {
                            emit_error(SyncError::General(local_path.clone(), err.to_string()));
                            break 'action;
                        }
                        emit_status(tr::tr!("Kept both versions: the local one is now '{}'.", util::fmt_home(&renamed.to_string_lossy())));
                        // The renamed copy is a new local file and goes up on the next pass.
                        action = Action::Download { local_path, remote_path, is_dir: false };
                        continue 'action;
                    }
                }
            }
        }
        break 'action;
        }
    }
    Applied { failures: failures.get(), conflicts, cancelled: false }
}

/// Re-check an overwrite or delete against the live state. Turns
/// - an upload over a remote copy that changed since the last sync into a conflict,
/// - a download over a local copy that changed since the last sync into a conflict,
/// - a remote delete of a copy edited remotely into a download (the edit wins),
/// - a local delete of a copy edited locally into an upload.
fn preflight(action: Action, db: &HashMap<String, SyncItem>, remote: &Remote, client: &dyn BackendClient, cancel: &Cancel) -> Action {
    let remote_changed = |remote_path: &str, row: &SyncItem| {
        matches!(client.stat(&remote.name, remote_path, cancel), Ok(Some(item)) if !item.is_dir && item.mod_time.unix_timestamp() > row.last_remote_timestamp)
    };
    let local_changed = |local_path: &str, row: &SyncItem| {
        fs::metadata(local_path).is_ok_and(|m| m.is_file()) && local_timestamp(Path::new(local_path)).is_some_and(|ts| ts as i64 > row.last_local_timestamp)
    };
    match action {
        Action::Upload { local_path, remote_path, is_dir: false } => match db.get(&remote_path) {
            Some(row) if remote_changed(&remote_path, row) => Action::Conflict { local_path, remote_path, kind: ConflictKind::BothChanged },
            _ => Action::Upload { local_path, remote_path, is_dir: false },
        },
        Action::Download { local_path, remote_path, is_dir: false } => match db.get(&remote_path) {
            Some(row) if local_changed(&local_path, row) => Action::Conflict { local_path, remote_path, kind: ConflictKind::BothChanged },
            _ => Action::Download { local_path, remote_path, is_dir: false },
        },
        Action::DeleteRemote { local_path, remote_path, is_dir: false } => match db.get(&remote_path) {
            Some(row) if remote_changed(&remote_path, row) => Action::Download { local_path, remote_path, is_dir: false },
            _ => Action::DeleteRemote { local_path, remote_path, is_dir: false },
        },
        Action::DeleteLocal { local_path, remote_path, is_dir: false } => match db.get(&remote_path) {
            Some(row) if local_changed(&local_path, row) => Action::Upload { local_path, remote_path, is_dir: false },
            _ => Action::DeleteLocal { local_path, remote_path, is_dir: false },
        },
        other => other,
    }
}

enum Inspected {
    /// One side disappeared since planning; the next pass sorts it out.
    Gone,
    Failed(String),
    Identical,
    Differs(Conflict),
}

/// Gather both sides of a conflict fresh and compare content where both SHA-1s are known.
fn inspect_conflict(local_path: &str, remote_path: &str, kind: ConflictKind, remote: &Remote, client: &dyn BackendClient, cancel: &Cancel) -> Inspected {
    let Ok(meta) = fs::metadata(local_path) else {
        return Inspected::Gone;
    };
    if !meta.is_file() {
        return Inspected::Gone;
    }
    let Some(local_stamp) = local_timestamp(Path::new(local_path)) else {
        return Inspected::Gone;
    };
    let remote_stamp = match client.stat(&remote.name, remote_path, cancel) {
        Ok(Some(item)) if !item.is_dir => item.mod_time.unix_timestamp(),
        Ok(_) => return Inspected::Gone,
        Err(err) => return Inspected::Failed(err),
    };
    let remote_details = match client.details(&remote.name, remote_path, cancel) {
        Ok(Some(details)) => details,
        Ok(None) => return Inspected::Gone,
        Err(err) => return Inspected::Failed(err),
    };
    let size_may_match = remote_details.size.is_none_or(|size| size == meta.len());
    let local_sha1 = if remote_details.sha1.is_some() && size_may_match { sha1_of(Path::new(local_path)) } else { None };
    if local_sha1.is_some() && local_sha1 == remote_details.sha1 {
        return Inspected::Identical;
    }
    Inspected::Differs(Conflict {
        local_path: local_path.to_owned(),
        remote_path: remote_path.to_owned(),
        local: FileDetails {
            size: Some(meta.len()),
            mod_time: OffsetDateTime::from_unix_timestamp(local_stamp as i64).unwrap_or(OffsetDateTime::UNIX_EPOCH),
            sha1: local_sha1,
        },
        remote: remote_details,
        local_stamp: local_stamp as i64,
        remote_stamp,
        first_sync: kind == ConflictKind::BothNew,
    })
}

fn sha1_of(path: &Path) -> Option<String> {
    use sha1::{Digest, Sha1};
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn activity_of(action: &Action) -> Option<SyncActivity> {
    match action {
        Action::Upload { .. } => Some(SyncActivity::Uploading),
        Action::Download { .. } => Some(SyncActivity::Downloading),
        Action::DeleteLocal { .. } | Action::DeleteRemote { .. } => Some(SyncActivity::Deleting),
        Action::Conflict { .. } => Some(SyncActivity::Resolving),
        Action::ClearDbRow { .. } | Action::RecordDbRow { .. } => None,
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
