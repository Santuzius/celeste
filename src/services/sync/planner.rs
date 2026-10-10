//! Diff a [`Snapshot`] (remote / local / db tuple) into a `Vec<Action>`
//! the applier executes. Pure function — no I/O, no logging beyond a
//! single one-line summary on the way out.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::domain::{
    remote::Remote,
    sync::{RemoteItem, SyncDir, SyncItem},
};

use super::local_names;
use super::snapshot::{LocalEntry, Snapshot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Upload {
        local_path: String,
        remote_path: String,
        is_dir: bool,
    },
    Download {
        local_path: String,
        remote_path: String,
        is_dir: bool,
    },
    DeleteLocal {
        local_path: String,
        remote_path: String,
        is_dir: bool,
    },
    DeleteRemote {
        local_path: String,
        remote_path: String,
        is_dir: bool,
    },
    ClearDbRow {
        local_path: String,
        remote_path: String,
    },
    /// Present on both sides but not tracked yet (a folder created on both sides): only the DB row is missing. Without it, deleting the folder on one side would bring it back from the other.
    /// The timestamps come from this pass's walk and listing, so recording needs no further call to the remote.
    RecordDbRow {
        local_path: String,
        remote_path: String,
        local_mtime: i64,
        remote_mtime: i64,
    },
    /// A file exists on both sides and the planner can't tell which copy is right. The applier compares content first (identical copies just get a DB row) and otherwise asks the user — see [`ConflictKind`].
    Conflict {
        local_path: String,
        remote_path: String,
        kind: ConflictKind,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConflictKind {
    /// Both copies changed since the last sync.
    BothChanged,
    /// Both copies existed before the first sync. Without a content digest to compare (pCloud, WebDAV, Dropbox) the newer one wins as before, so a first sync of two large trees doesn't turn into hundreds of questions.
    BothNew,
}

pub(super) fn plan(snapshot: &Snapshot, sync_dir: &SyncDir) -> Vec<Action> {
    let keys: BTreeSet<&String> = snapshot
        .remote
        .keys()
        .chain(snapshot.local.keys())
        .chain(snapshot.db.keys())
        .collect();
    let db_by_parent = group_db_keys_by_parent(&snapshot.db);
    let mut out = Vec::new();
    for key in keys {
        let local = snapshot.local.get(key);
        let remote = snapshot.remote.get(key);
        let db = snapshot.db.get(key);
        if let Some(action) = plan_one(
            key,
            local,
            remote,
            db,
            sync_dir,
            snapshot,
            &db_by_parent,
        ) {
            out.push(action);
        }
    }
    // Order: directory creations first (uploads/downloads for dirs before
    // their children), file transfers next, deletes last. Within each
    // group, shorter paths first so parents precede children.
    out.sort_by_key(|a| (phase(a), a_path(a).len(), a_path(a).to_owned()));
    out
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

fn group_db_keys_by_parent(
    db: &HashMap<String, SyncItem>,
) -> HashMap<&str, Vec<&str>> {
    let mut out: HashMap<&str, Vec<&str>> = HashMap::new();
    for key in db.keys() {
        out.entry(parent_of(key)).or_default().push(key.as_str());
    }
    out
}

/// Did the listing / walk successfully enumerate `path`'s parent?
/// Used to distinguish a legitimate "parent was emptied" from a
/// rate-limit / cache-flush / concurrent-write glitch that returned
/// partial data.
///
/// Returns `true` when any of the following holds:
///   1. The DB has no other tracked siblings under that parent — this
///      is the legitimate "last file in its parent" case.
///   2. At least one DB-tracked sibling of `path` is present in
///      `items`. Proof that the listing / walk reached the parent.
///   3. `items` contains *any* entry directly under the same parent
///      — even untracked ones. A non-empty enumeration of the parent
///      is proof the call succeeded; the DB-tracked ones are genuinely
///      gone. This is the mass-replace / bulk-delete case where none
///      of the old DB-tracked items survive but new content arrived.
///
/// Returns `false` only when the DB says siblings should exist, none
/// of them appear in `items`, and the listing / walk shows nothing
/// at all under that parent — the dangerous "we saw zero where we
/// expected many" pattern.
fn parent_is_verified<T>(
    path: &str,
    items: &HashMap<String, T>,
    db_by_parent: &HashMap<&str, Vec<&str>>,
) -> bool {
    let parent = parent_of(path);
    let siblings = match db_by_parent.get(parent) {
        Some(v) => v,
        None => return true,
    };
    let mut any_expected = false;
    for sibling in siblings {
        if *sibling == path {
            continue;
        }
        any_expected = true;
        if items.contains_key(*sibling) {
            return true;
        }
    }
    if !any_expected {
        return true;
    }
    items.keys().any(|k| parent_of(k) == parent)
}

/// Emit a single stderr line summarising the snapshot shape and the
/// planned action counts. Lets the user tell at a glance whether a
/// "DELETE mirror-remote" is one-off (e.g. a manual delete mirroring
/// cleanly) or the start of a cascade they want to interrupt.
pub(super) fn log_plan_summary(
    remote: &Remote,
    sync_dir: &SyncDir,
    snapshot: &Snapshot,
    actions: &[Action],
) {
    // A pass with nothing to do is the common case; only the detailed log keeps a line for it.
    if actions.is_empty() && !crate::infrastructure::stderr_capture::detailed() {
        return;
    }
    let (mut uploads, mut downloads) = (0usize, 0usize);
    let (mut delete_local, mut delete_remote) = (0usize, 0usize);
    let (mut conflicts, mut clear_rows, mut record_rows) = (0usize, 0usize, 0usize);
    for a in actions {
        match a {
            Action::Upload { .. } => uploads += 1,
            Action::Download { .. } => downloads += 1,
            Action::DeleteLocal { .. } => delete_local += 1,
            Action::DeleteRemote { .. } => delete_remote += 1,
            Action::Conflict { .. } => conflicts += 1,
            Action::ClearDbRow { .. } => clear_rows += 1,
            Action::RecordDbRow { .. } => record_rows += 1,
        }
    }
    eprintln!(
        "sync: plan for remote='{}' dir='{}' — snapshot(db={}, listing={}, walk={}, walk_unreliable={}); actions(upload={}, download={}, delete_local={}, delete_remote={}, conflict={}, clear_db_row={}, record_db_row={}).",
        remote.name,
        sync_dir.remote_path,
        snapshot.db.len(),
        snapshot.remote.len(),
        snapshot.local.len(),
        snapshot.walk_unreliable.len(),
        uploads,
        downloads,
        delete_local,
        delete_remote,
        conflicts,
        clear_rows,
        record_rows,
    );
}

/// Does `path`, or any of its ancestor directories up to the sync-dir
/// root, appear in `unreliable`? Used to refuse `DeleteRemote` when the
/// local walk reported an I/O error anywhere along the chain — the
/// "local is missing" signal isn't trustworthy under a broken walk,
/// regardless of how healthy the siblings look.
pub(super) fn ancestor_in_set(path: &str, unreliable: &HashSet<String>) -> bool {
    if unreliable.is_empty() {
        return false;
    }
    if unreliable.contains(path) {
        return true;
    }
    let mut p = path;
    loop {
        let parent = parent_of(p);
        if unreliable.contains(parent) {
            return true;
        }
        if parent == p {
            return false;
        }
        p = parent;
    }
}

impl Action {
    /// Only touches the DB, nothing the user would count as a synced change.
    pub(super) fn is_bookkeeping(&self) -> bool {
        matches!(self, Action::ClearDbRow { .. } | Action::RecordDbRow { .. })
    }
}

fn phase(a: &Action) -> u8 {
    match a {
        Action::Upload { is_dir: true, .. } | Action::Download { is_dir: true, .. } => 0,
        Action::Upload { .. } | Action::Download { .. } => 1,
        Action::Conflict { .. } => 2,
        Action::ClearDbRow { .. } | Action::RecordDbRow { .. } => 3,
        Action::DeleteLocal { .. } | Action::DeleteRemote { .. } => 4,
    }
}

fn a_path(a: &Action) -> &str {
    match a {
        Action::Upload { remote_path, .. }
        | Action::Download { remote_path, .. }
        | Action::DeleteLocal { remote_path, .. }
        | Action::DeleteRemote { remote_path, .. }
        | Action::ClearDbRow { remote_path, .. }
        | Action::RecordDbRow { remote_path, .. }
        | Action::Conflict { remote_path, .. } => remote_path,
    }
}

fn plan_one(
    remote_path: &str,
    local: Option<&LocalEntry>,
    remote: Option<&RemoteItem>,
    db: Option<&SyncItem>,
    sync_dir: &SyncDir,
    snapshot: &Snapshot,
    db_by_parent: &HashMap<&str, Vec<&str>>,
) -> Option<Action> {
    match (local, remote, db) {
        (None, None, None) => None,
        // Stale DB: both sides gone. Clean the row.
        (None, None, Some(_)) => Some(Action::ClearDbRow {
            local_path: db_local_path(remote_path, sync_dir),
            remote_path: remote_path.to_owned(),
        }),
        // New local, never seen — upload.
        (Some(l), None, None) => Some(Action::Upload {
            local_path: l.absolute_path.clone(),
            remote_path: remote_path.to_owned(),
            is_dir: l.is_dir,
        }),
        // New remote, never seen — download.
        (None, Some(r), None) => Some(Action::Download {
            local_path: derive_local_path(&r.path, sync_dir),
            remote_path: r.path.clone(),
            is_dir: r.is_dir,
        }),
        // Both present, never tracked — compare timestamps, upload if
        // local is strictly newer, else download. Equal timestamps: just
        // record the DB row via an upload (no-op transfer but aligns DB).
        (Some(l), Some(r), None) => {
            if l.is_dir && r.is_dir {
                // Dirs: nothing to transfer, just start tracking it.
                return Some(Action::RecordDbRow {
                    local_path: l.absolute_path.clone(),
                    remote_path: remote_path.to_owned(),
                    local_mtime: l.mtime_secs,
                    remote_mtime: r.mod_time.unix_timestamp(),
                });
            }
            if l.is_dir != r.is_dir {
                // A file on one side, a folder on the other: no content to compare, keep the old newer-wins rule.
                return Some(if l.mtime_secs > r.mod_time.unix_timestamp() {
                    Action::Upload { local_path: l.absolute_path.clone(), remote_path: remote_path.to_owned(), is_dir: l.is_dir }
                } else {
                    Action::Download { local_path: l.absolute_path.clone(), remote_path: remote_path.to_owned(), is_dir: r.is_dir }
                });
            }
            Some(Action::Conflict {
                local_path: l.absolute_path.clone(),
                remote_path: remote_path.to_owned(),
                kind: ConflictKind::BothNew,
            })
        }
        // Was tracked, remote gone — mirror delete locally. Require a
        // DB-tracked sibling of this item to also appear in the listing;
        // otherwise the listing for this parent is untrustworthy (rate-
        // limit / cache flush) and we refuse to destroy the local copy.
        // Equivalent of the GoogleDrive-era sibling verification in
        // 6117026, adapted to the snapshot algorithm.
        (Some(l), None, Some(db)) => {
            if !parent_is_verified(
                remote_path,
                &snapshot.remote,
                db_by_parent,
            ) {
                eprintln!(
                    "sync: SKIP DeleteLocal for '{remote_path}' — listing enumerated no entries under '{}' (likely rate-limited / cache flush); preserving local copy.",
                    parent_of(remote_path),
                );
                return None;
            }
            // If the local file has been modified since the last successful
            // sync, the missing-remote is more consistent with a failed
            // upload that rclone cleaned up (e.g. hash mismatch on transfer)
            // than with an intentional remote deletion. Re-upload rather
            // than destroy the newer local copy.
            if !l.is_dir && l.mtime_secs > db.last_local_timestamp {
                eprintln!(
                    "sync: SWAP DeleteLocal → Upload for '{remote_path}' — local mtime {} newer than last synced {} (likely failed upload cleanup); retrying upload.",
                    l.mtime_secs, db.last_local_timestamp,
                );
                return Some(Action::Upload {
                    local_path: l.absolute_path.clone(),
                    remote_path: remote_path.to_owned(),
                    is_dir: l.is_dir,
                });
            }
            Some(Action::DeleteLocal {
                local_path: l.absolute_path.clone(),
                remote_path: remote_path.to_owned(),
                is_dir: l.is_dir,
            })
        }
        // Was tracked, local gone — mirror delete remotely. Two
        // guards before we fire anything destructive:
        //   1. Ancestor reliability. If the local walk reported an
        //      I/O error anywhere in this path's chain, the absence
        //      isn't a deletion signal — it's a walk glitch (most
        //      commonly a concurrent writer like Syncthing racing
        //      Celeste mid-readdir). Refuse.
        //   2. Sibling presence. Weaker backstop for paths whose
        //      ancestors look healthy but whose parent holds other
        //      DB-tracked rows that didn't make the walk either.
        (None, Some(r), Some(db)) => {
            // Deleted here, edited there: the edit wins, so nothing the user changed is lost. Fetch it back instead of deleting it.
            if !r.is_dir && r.mod_time.unix_timestamp() > db.last_remote_timestamp {
                eprintln!(
                    "sync: SWAP DeleteRemote → Download for '{}' — remote changed since the last sync; restoring it locally.",
                    r.path,
                );
                return Some(Action::Download {
                    local_path: derive_local_path(&r.path, sync_dir),
                    remote_path: r.path.clone(),
                    is_dir: false,
                });
            }
            if ancestor_in_set(&r.path, &snapshot.walk_unreliable) {
                eprintln!(
                    "sync: SKIP DeleteRemote for '{}' — local walk reported an error on this path or an ancestor; preserving remote copy.",
                    r.path,
                );
                return None;
            }
            // A sub-folder the walk read without error and found empty was emptied on purpose, so its files go remotely too. The sync folder itself is never in the walk and keeps the stricter check: an empty root more likely means an unmounted disk.
            let parent_walked = snapshot.local.get(parent_of(&r.path)).is_some_and(|p| p.is_dir);
            if !parent_walked && !parent_is_verified(
                &r.path,
                &snapshot.local,
                db_by_parent,
            ) {
                eprintln!(
                    "sync: SKIP DeleteRemote for '{}' — walk found no entries under '{}' (likely concurrent-write race); preserving remote copy.",
                    r.path,
                    parent_of(&r.path),
                );
                return None;
            }
            Some(Action::DeleteRemote {
                local_path: derive_local_path(&r.path, sync_dir),
                remote_path: r.path.clone(),
                is_dir: r.is_dir,
            })
        }
        // Full triple — compare timestamps against the recorded values.
        (Some(l), Some(r), Some(db)) => {
            let local_changed = l.mtime_secs > db.last_local_timestamp;
            let remote_changed =
                r.mod_time.unix_timestamp() > db.last_remote_timestamp;
            match (local_changed, remote_changed) {
                (false, false) => None,
                (true, false) => Some(Action::Upload {
                    local_path: l.absolute_path.clone(),
                    remote_path: remote_path.to_owned(),
                    is_dir: l.is_dir,
                }),
                (false, true) => Some(Action::Download {
                    local_path: l.absolute_path.clone(),
                    remote_path: remote_path.to_owned(),
                    is_dir: r.is_dir,
                }),
                // Both sides drifted since the last recorded sync: the user decides, unless the content turns out identical. Two dirs against each other still no-op since the contents land via their children.
                (true, true) => {
                    if l.is_dir && r.is_dir {
                        None
                    } else if l.is_dir != r.is_dir {
                        // File on one side, folder on the other: nothing to compare, the newer one wins.
                        Some(if l.mtime_secs >= r.mod_time.unix_timestamp() {
                            Action::Upload { local_path: l.absolute_path.clone(), remote_path: remote_path.to_owned(), is_dir: l.is_dir }
                        } else {
                            Action::Download { local_path: l.absolute_path.clone(), remote_path: remote_path.to_owned(), is_dir: r.is_dir }
                        })
                    } else {
                        Some(Action::Conflict {
                            local_path: l.absolute_path.clone(),
                            remote_path: remote_path.to_owned(),
                            kind: ConflictKind::BothChanged,
                        })
                    }
                }
            }
        }
    }
}

fn derive_local_path(remote_path: &str, sync_dir: &SyncDir) -> String {
    let relative = if sync_dir.remote_path.is_empty() {
        remote_path.to_owned()
    } else {
        remote_path
            .strip_prefix(&format!("{}/", sync_dir.remote_path))
            .unwrap_or(remote_path)
            .to_owned()
    };
    if relative.is_empty() {
        sync_dir.local_path.clone()
    } else {
        format!("{}/{}", sync_dir.local_path, local_names::to_local(&relative))
    }
}

fn db_local_path(remote_path: &str, sync_dir: &SyncDir) -> String {
    derive_local_path(remote_path, sync_dir)
}
