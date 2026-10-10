//! Snapshot building: pull the authoritative remote listing, walk the
//! local tree, load DB rows, and combine into one structure the planner
//! can decide against.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
    time::SystemTime,
};

use super::local_names;
use crate::{
    domain::{
        ports::{BackendClient, Cancel, Repository},
        remote::Remote,
        sync::{ListFilter, RemoteItem, SyncDir, SyncItem},
    },
    util,
};

/// One file or directory observed during the local walk.
#[derive(Clone, Debug)]
pub(super) struct LocalEntry {
    pub absolute_path: String,
    pub is_dir: bool,
    pub mtime_secs: i64,
}

/// Combined snapshot of the three views the planner reasons against:
/// remote listing, local walk, and tracked DB rows.
pub(super) struct Snapshot {
    pub remote: HashMap<String, RemoteItem>,
    pub local: HashMap<String, LocalEntry>,
    pub db: HashMap<String, SyncItem>,
    /// Remote-key paths whose local walk hit an I/O error (read_dir,
    /// file_type, or filename decoding failed). "Missing from `local`"
    /// under any of these ancestors is *not* a deletion signal — it's
    /// a walk glitch (most often a concurrent writer, e.g. Syncthing
    /// racing Celeste on the same tree). The planner refuses to fire
    /// `DeleteRemote` for anything whose ancestor chain lands here.
    pub walk_unreliable: HashSet<String>,
}

impl Snapshot {
    pub(super) fn build(
        remote: &Remote,
        sync_dir: &SyncDir,
        repo: &dyn Repository,
        client: &dyn BackendClient,
        all_sync_dirs: &[SyncDir],
        cancel: &Cancel,
    ) -> Result<Self, String> {
        // Auto-exclusion is keyed off remote-tree descendancy only: when
        // another sync_dir on the same provider sits inside this one's
        // remote subtree, skip it here so the two passes don't fight.
        // (Local-tree overlaps are blocked at AddSyncDir time, so they
        // can't exist by the time we get here.)
        //
        // Provider listings disagree on shape: librclone's
        // `operations/list` yields paths *relative* to the listed root
        // ("bar/baz.txt"), while the native Proton client builds
        // *absolute* remote paths ("Foo/bar/baz.txt"). Push both forms
        // so the filter matches either way — the extra string only
        // costs one `starts_with` per listed item. The matching local
        // sub-tree is also excluded to keep the walk from picking up a
        // coincidentally-named folder under this sync_dir's root.
        let mut excluded_local_prefixes: Vec<String> = Vec::new();
        let mut excluded_remote_prefixes: Vec<String> = Vec::new();
        for d in all_sync_dirs
            .iter()
            .filter(|d| d.id != sync_dir.id && d.remote_id == sync_dir.remote_id)
        {
            if let Some(rel) = remote_descendant_relative(sync_dir, &d.remote_path) {
                excluded_remote_prefixes
                    .push(absolute_remote_path(sync_dir, &rel));
                excluded_remote_prefixes.push(rel.clone());
                excluded_local_prefixes
                    .push(format!("{}/{}", sync_dir.local_path, local_names::to_local(&rel)));
            }
        }

        // User-defined exclusions (stored as remote sub-paths relative to
        // this sync_dir's remote root). Push both the relative and the
        // absolute form for the same provider-shape reason as above; the
        // local form is always absolute and only needs one entry.
        let user_excls = util::await_future(repo.list_exclusions(sync_dir.id))
            .unwrap_or_default();
        for excl in &user_excls {
            excluded_remote_prefixes
                .push(absolute_remote_path(sync_dir, &excl.remote_path));
            excluded_remote_prefixes.push(excl.remote_path.clone());
            excluded_local_prefixes.push(format!("{}/{}", sync_dir.local_path, local_names::to_local(&excl.remote_path)));
        }

        // 1. DB (cheap, authoritative for "what we last saw"). Drop any
        //    tracked rows that now fall inside an excluded subtree — they
        //    may linger from before the descendant sync_dir was created.
        let db_rows = util::await_future(repo.list_sync_items(sync_dir.id))
            .unwrap_or_default();
        let db: HashMap<String, SyncItem> = db_rows
            .into_iter()
            .filter(|r| !path_is_excluded(&r.local_path, &excluded_local_prefixes))
            .map(|r| (r.remote_path.clone(), r))
            .collect();

        // 2. Remote listing — single authoritative call. When the
        //    sync_dir's root doesn't exist on the remote (user or a
        //    prior incident trashed it), treat the listing as empty
        //    and mkdir the root so the upload phase can recreate the
        //    tree. Without this, the sync aborts every cycle and the
        //    user has to delete + re-add the sync_dir just to force
        //    a fresh mkdir.
        let remote_items = match client.list(
            &remote.name,
            &sync_dir.remote_path,
            true,
            ListFilter::All,
            cancel,
        ) {
            Ok(items) => items,
            Err(err) if is_directory_missing_error(&err) => {
                eprintln!(
                    "sync: remote dir '{}' missing for remote='{}' ({err}); recreating and proceeding with empty listing.",
                    sync_dir.remote_path, remote.name,
                );
                if !sync_dir.remote_path.is_empty() {
                    let _ = client.mkdir(&remote.name, &sync_dir.remote_path, cancel);
                }
                Vec::new()
            }
            Err(err) => return Err(err),
        };

        let remote: HashMap<String, RemoteItem> = remote_items
            .into_iter()
            .filter(|i| !path_is_excluded(&i.path, &excluded_remote_prefixes))
            .map(|i| (i.path.clone(), i))
            .collect();

        // 3. Local walk — skip subtrees managed by descendant sync_dirs.
        let (local, walk_unreliable) = walk_local(sync_dir, &excluded_local_prefixes);

        Ok(Snapshot {
            remote,
            local,
            db,
            walk_unreliable,
        })
    }
}

/// Does `err` look like a "remote directory doesn't exist" failure?
/// Providers word this differently: rclone's GDrive backend bubbles up
/// `error in ListJSON: directory not found`, the native Proton client
/// returns our own `resolve_path` miss, WebDAV can reply with a 404
/// body. Match broadly — a false positive here costs us one speculative
/// mkdir, a miss costs the user a broken sync cycle.
fn is_directory_missing_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("directory not found")
        || lower.contains("not found on remote")
        || lower.contains("no such file or directory")
}

/// Returns the candidate's path *relative to* `ancestor.remote_path` when
/// the candidate is a strict descendant of the ancestor in remote space
/// (or a non-empty path under an empty/root ancestor). Returns `None`
/// otherwise — including when the two are equal or unrelated.
fn remote_descendant_relative(ancestor: &SyncDir, candidate_remote: &str) -> Option<String> {
    if ancestor.remote_path.is_empty() {
        if candidate_remote.is_empty() {
            None
        } else {
            Some(candidate_remote.to_owned())
        }
    } else {
        let sep = format!("{}/", ancestor.remote_path);
        candidate_remote.strip_prefix(&sep).map(str::to_owned)
    }
}

/// Glue an ancestor's remote root onto a relative remote sub-path, producing
/// the absolute form that providers like the native Proton client return.
fn absolute_remote_path(sync_dir: &SyncDir, relative: &str) -> String {
    if sync_dir.remote_path.is_empty() {
        relative.to_owned()
    } else {
        format!("{}/{}", sync_dir.remote_path, relative)
    }
}

/// Returns true when `path` equals one of `excluded_prefixes` or starts
/// with one of them followed by `/`.
fn path_is_excluded(path: &str, excluded_prefixes: &[String]) -> bool {
    excluded_prefixes
        .iter()
        .any(|ex| path == ex || path.starts_with(&format!("{ex}/")))
}

fn walk_local(
    sync_dir: &SyncDir,
    excluded_local_prefixes: &[String],
) -> (HashMap<String, LocalEntry>, HashSet<String>) {
    let root = Path::new(&sync_dir.local_path);
    let mut out: HashMap<String, LocalEntry> = HashMap::new();
    let mut unreliable: HashSet<String> = HashSet::new();
    walk_dir(
        root,
        sync_dir,
        &mut out,
        &mut unreliable,
        &sync_dir.remote_path,
        excluded_local_prefixes,
    );
    (out, unreliable)
}

/// Walks `dir`, populating `out` with every entry and recording any I/O
/// failure in `unreliable`. `current_dir_key` is the remote-key path of
/// `dir` itself (empty string for the root when `sync_dir.remote_path`
/// is empty). All errors are logged to stderr so the next incident is
/// traceable without having to reproduce it under a debugger.
fn walk_dir(
    dir: &Path,
    sync_dir: &SyncDir,
    out: &mut HashMap<String, LocalEntry>,
    unreliable: &mut HashSet<String>,
    current_dir_key: &str,
    excluded_local_prefixes: &[String],
) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "sync: walk read_dir failed for '{}' (key='{current_dir_key}'): {err}; marking subtree unreliable.",
                dir.display(),
            );
            unreliable.insert(current_dir_key.to_owned());
            return;
        }
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                eprintln!(
                    "sync: walk entry iteration failed under '{}' (key='{current_dir_key}'): {err}; marking subtree unreliable.",
                    dir.display(),
                );
                unreliable.insert(current_dir_key.to_owned());
                continue;
            }
        };
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(raw) => {
                eprintln!(
                    "sync: walk file_name non-UTF8 under '{}' (bytes={:?}); marking subtree unreliable.",
                    dir.display(),
                    raw,
                );
                unreliable.insert(current_dir_key.to_owned());
                continue;
            }
        };
        if crate::services::editor_temp::is_editor_temp(&name) {
            continue;
        }
        let remote_name = local_names::from_local(&name);
        let remote_key = if current_dir_key.is_empty() {
            remote_name.into_owned()
        } else {
            format!("{current_dir_key}/{remote_name}")
        };
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(err) => {
                eprintln!(
                    "sync: walk file_type failed for '{}' (key='{remote_key}'): {err}; marking entry unreliable.",
                    entry.path().display(),
                );
                unreliable.insert(remote_key);
                continue;
            }
        };
        let path = entry.path();
        let Some(local_path_str) = path.to_str() else {
            eprintln!(
                "sync: walk path non-UTF8 for '{}' (key='{remote_key}'); marking entry unreliable.",
                path.display(),
            );
            unreliable.insert(remote_key);
            continue;
        };
        if path_is_excluded(local_path_str, excluded_local_prefixes) {
            continue;
        }
        let mtime_secs = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.insert(
            remote_key.clone(),
            LocalEntry {
                absolute_path: local_path_str.to_owned(),
                is_dir: file_type.is_dir(),
                mtime_secs,
            },
        );
        if file_type.is_dir() {
            walk_dir(&path, sync_dir, out, unreliable, &remote_key, excluded_local_prefixes);
        }
    }
}
