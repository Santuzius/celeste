//! Unit tests for the new `services::sync` algorithm. Covers:
//!
//! - plan() decisions on every (local, remote, db) tuple.
//! - apply() mirror-deletes, race-swallow on upload, conflict emission.

#![cfg(test)]

use std::{
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::Mutex,
};

use super::{planner::ancestor_in_set, run, Outcome};
use crate::{
    domain::{
        events::SyncEvent,
        ports::cancel_never,
        sync::{Conflict, ConflictChoice, Resolution, SyncDirId, SyncError},
    },
    test_support::{remote, remote_item, sync_dir, touch_mtime, FakeBackend, FakeRepo, TempDir},
};

fn run_full(
    tmp: &TempDir,
    repo: &FakeRepo,
    client: &FakeBackend,
) -> (Outcome, Vec<SyncEvent>) {
    run_resolving(tmp, repo, client, &[])
}

fn run_resolving(
    tmp: &TempDir,
    repo: &FakeRepo,
    client: &FakeBackend,
    resolutions: &[Resolution],
) -> (Outcome, Vec<SyncEvent>) {
    let r = remote(1, "TestRemote");
    let sd = sync_dir(1, 1, tmp.as_str(), "");
    let captured: Mutex<Vec<SyncEvent>> = Mutex::new(Vec::new());
    let cancel = cancel_never();
    let outcome = run(
        &r,
        &sd,
        repo,
        client,
        &[],
        resolutions,
        |e| captured.lock().unwrap().push(e),
        &cancel,
        |_| false,
    );
    let events = captured.lock().unwrap().clone();
    (outcome, events)
}

fn errors(events: &[SyncEvent]) -> Vec<SyncError> {
    events
        .iter()
        .filter_map(|e| match e {
            SyncEvent::SyncDirError { error, .. } => Some(error.clone()),
            _ => None,
        })
        .collect()
}

/// Snapshot abort path when `client.list` itself errors out (e.g.
/// network loss). Nothing destructive runs.
#[test]
fn snapshot_aborts_when_list_fails() {
    let tmp = TempDir::new("sync_list_err");
    let local = tmp.write_file("a.txt", b"hi");
    touch_mtime(&local, 1_700_000_000);
    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        local.to_str().unwrap(),
        "a.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list("", Err("connection reset".to_owned()));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Aborted);
    assert!(local.exists());
    assert_eq!(repo.item_count(), 1);
}

/// Everything in sync → no destructive actions, no transfers.
#[test]
fn steady_state_produces_no_actions() {
    let tmp = TempDir::new("sync_steady");
    let local = tmp.write_file("a.txt", b"v1");
    touch_mtime(&local, 1_700_000_000);
    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        local.to_str().unwrap(),
        "a.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("a.txt", false, 1_700_000_000)]));

    let (outcome, events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(errors(&events).is_empty());
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
    assert!(client.copy_to_local_calls.lock().unwrap().is_empty());
    assert!(client.delete_file_calls.lock().unwrap().is_empty());
    assert!(local.exists());
}

/// Local deleted between syncs → mirror on remote. DB row cleared.
#[test]
fn local_deleted_mirrors_to_remote() {
    let tmp = TempDir::new("sync_local_del");
    let repo = FakeRepo::new();
    let local_path = format!("{}/gone.txt", tmp.as_str());
    repo.insert_item(
        SyncDirId(1),
        &local_path,
        "gone.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("gone.txt", false, 1_700_000_000)]));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert_eq!(client.delete_file_calls.lock().unwrap().len(), 1);
    assert!(!repo.has_item(&local_path, "gone.txt"));
}

/// Remote deleted between syncs → mirror locally. DB row cleared.
#[test]
fn remote_deleted_mirrors_locally() {
    let tmp = TempDir::new("sync_remote_del");
    let local = tmp.write_file("gone.txt", b"v1");
    touch_mtime(&local, 1_700_000_000);
    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        local.to_str().unwrap(),
        "gone.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list("", Ok(vec![]));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(!local.exists());
    assert!(!repo.has_item(local.to_str().unwrap(), "gone.txt"));
}

fn conflicts(events: &[SyncEvent]) -> Vec<Conflict> {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            SyncEvent::SyncDirConflicts { conflicts, .. } => Some(conflicts.clone()),
            _ => None,
        })
        .expect("pass reported no conflict list")
}

/// Local copy at 1_700_000_500 and remote copy at 1_700_000_700, both edited since the sync at 1_700_000_000.
fn both_changed(name: &str) -> (TempDir, PathBuf, FakeRepo, FakeBackend) {
    let tmp = TempDir::new(name);
    let local = tmp.write_file("a.txt", b"v2");
    touch_mtime(&local, 1_700_000_500);
    let repo = FakeRepo::new();
    repo.insert_item(SyncDirId(1), local.to_str().unwrap(), "a.txt", 1_700_000_000, 1_700_000_000);
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("a.txt", false, 1_700_000_700)]));
    client.set_stat("a.txt", Ok(Some(remote_item("a.txt", false, 1_700_000_700))));
    (tmp, local, repo, client)
}

/// Both sides changed since the last sync: nothing is transferred and the file is reported as a conflict. The pass itself doesn't set Warning; the app does, for the conflicts it still shows.
#[test]
fn both_sides_changed_is_a_conflict() {
    let (tmp, local, repo, client) = both_changed("sync_conflict_reported");
    let (_outcome, events) = run_full(&tmp, &repo, &client);
    let reported = conflicts(&events);
    assert_eq!(reported.len(), 1);
    assert_eq!(reported[0].local_path, local.to_str().unwrap());
    assert_eq!((reported[0].local_stamp, reported[0].remote_stamp), (1_700_000_500, 1_700_000_700));
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
    assert!(client.copy_to_local_calls.lock().unwrap().is_empty());
    assert!(!events.iter().any(|e| matches!(e, SyncEvent::SyncDirStateChanged { state: crate::domain::run_state::RunState::Warning, .. })));
}

/// A file waiting for the user doesn't hold up the rest of the folder: other changes in the same pass still go through.
#[test]
fn conflict_does_not_block_other_changes() {
    let (tmp, _local, repo, client) = both_changed("sync_conflict_others");
    let new_local = tmp.write_file("b.txt", b"b");
    client.set_list("", Ok(vec![remote_item("a.txt", false, 1_700_000_700), remote_item("c.txt", false, 1_700_000_000)]));
    let (_outcome, events) = run_full(&tmp, &repo, &client);
    assert_eq!(conflicts(&events).len(), 1);
    assert_eq!(*client.copy_to_remote_calls.lock().unwrap(), vec![(new_local.to_str().unwrap().to_owned(), "b.txt".to_owned())]);
    assert_eq!(client.copy_to_local_calls.lock().unwrap().len(), 1);
}

/// The user's choice is applied when the file still looks the way the dialog showed it.
#[test]
fn resolution_keep_local_uploads() {
    let (tmp, _local, repo, client) = both_changed("sync_conflict_keep_local");
    let resolution = Resolution { remote_path: "a.txt".into(), choice: ConflictChoice::KeepLocal, local_stamp: 1_700_000_500, remote_stamp: 1_700_000_700 };
    let (_outcome, events) = run_resolving(&tmp, &repo, &client, &[resolution]);
    assert!(conflicts(&events).is_empty());
    assert_eq!(client.copy_to_remote_calls.lock().unwrap().len(), 1);
    assert!(client.copy_to_local_calls.lock().unwrap().is_empty());
}

/// A choice made for an older state of the file is not applied; the conflict is reported again.
#[test]
fn stale_resolution_is_ignored() {
    let (tmp, _local, repo, client) = both_changed("sync_conflict_stale");
    let resolution = Resolution { remote_path: "a.txt".into(), choice: ConflictChoice::KeepRemote, local_stamp: 1_700_000_400, remote_stamp: 1_700_000_700 };
    let (_outcome, events) = run_resolving(&tmp, &repo, &client, &[resolution]);
    assert_eq!(conflicts(&events).len(), 1);
    assert!(client.copy_to_local_calls.lock().unwrap().is_empty());
}

/// Keep both renames the local copy and fetches the remote one under the original name.
#[test]
fn resolution_keep_both_renames_and_downloads() {
    let (tmp, local, repo, client) = both_changed("sync_conflict_keep_both");
    let resolution = Resolution {
        remote_path: "a.txt".into(),
        choice: ConflictChoice::KeepBoth { local_name: "a (1).txt".into() },
        local_stamp: 1_700_000_500,
        remote_stamp: 1_700_000_700,
    };
    let (_outcome, _events) = run_resolving(&tmp, &repo, &client, &[resolution]);
    assert!(local.with_file_name("a (1).txt").exists());
    let downloads = client.copy_to_local_calls.lock().unwrap();
    assert_eq!(downloads.len(), 1);
    assert_eq!(downloads[0].1, "a.txt");
}

/// Identical content on both sides is no conflict, only a DB update.
#[test]
fn identical_content_is_no_conflict() {
    let (tmp, _local, repo, client) = both_changed("sync_conflict_identical");
    let expected = {
        use sha1::{Digest, Sha1};
        Sha1::digest(b"v2").iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    client.set_sha1("a.txt", &expected);
    let (_outcome, events) = run_full(&tmp, &repo, &client);
    assert!(conflicts(&events).is_empty());
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
    assert!(client.copy_to_local_calls.lock().unwrap().is_empty());
}

/// Deleted locally but edited remotely: the edit wins and is downloaded instead of deleted.
#[test]
fn remote_edit_beats_local_delete() {
    let tmp = TempDir::new("sync_edit_beats_delete");
    let local = tmp.path.join("a.txt");
    let repo = FakeRepo::new();
    repo.insert_item(SyncDirId(1), local.to_str().unwrap(), "a.txt", 1_700_000_000, 1_700_000_000);
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("a.txt", false, 1_700_000_700)]));
    let (_outcome, _events) = run_full(&tmp, &repo, &client);
    assert!(client.delete_file_calls.lock().unwrap().is_empty());
    assert_eq!(client.copy_to_local_calls.lock().unwrap().len(), 1);
}

/// The listing still shows the old remote copy (cached), but the remote changed meanwhile: the upload turns into a conflict instead of overwriting it.
#[test]
fn upload_rechecks_remote_before_overwriting() {
    let tmp = TempDir::new("sync_preflight_upload");
    let local = tmp.write_file("a.txt", b"v2");
    touch_mtime(&local, 1_700_000_500);
    let repo = FakeRepo::new();
    repo.insert_item(SyncDirId(1), local.to_str().unwrap(), "a.txt", 1_700_000_000, 1_700_000_000);
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("a.txt", false, 1_700_000_000)]));
    client.set_stat("a.txt", Ok(Some(remote_item("a.txt", false, 1_700_000_800))));
    let (_outcome, events) = run_full(&tmp, &repo, &client);
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
    assert_eq!(conflicts(&events).len(), 1);
}

/// Upload that fails with "source gone" after planning — raced with a
/// local delete — is silently swallowed. No DB row written, no error
/// surfaced.
#[test]
fn upload_swallows_source_gone_race() {
    let tmp = TempDir::new("sync_upload_race");
    let local = tmp.write_file("a.txt", b"hi");
    touch_mtime(&local, 1_700_000_000);
    let repo = FakeRepo::new();
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![]));
    // Simulate the race by removing the source before apply() runs —
    // our `!Path::new(local).exists()` early-exit short-circuits the
    // copy call entirely.
    let client_check = &client;
    fs::remove_file(&local).unwrap();
    let (outcome, events) = run_full(&tmp, &repo, client_check);
    assert_eq!(outcome, Outcome::Synced);
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
    assert!(errors(&events).is_empty());
}

/// `is_cancelled` returning true after the snapshot but before apply
/// bails out with `Outcome::Aborted` and no destructive op fires.
#[test]
fn cancellation_between_snapshot_and_apply_stops_the_pass() {
    let tmp = TempDir::new("sync_cancel");
    let repo = FakeRepo::new();
    // Seed a DB row and no local file → plan would normally emit a
    // DeleteRemote, but we cancel first.
    let local_path = format!("{}/doomed.txt", tmp.as_str());
    repo.insert_item(
        SyncDirId(1),
        &local_path,
        "doomed.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list(
        "",
        Ok(vec![remote_item("doomed.txt", false, 1_700_000_000)]),
    );

    let r = remote(1, "TestRemote");
    let sd = sync_dir(1, 1, tmp.as_str(), "");
    let captured: Mutex<Vec<SyncEvent>> = Mutex::new(Vec::new());

    // Cancel-on-first-call: the cancel check fires true right after
    // Snapshot::build, before plan/apply.
    use std::sync::atomic::AtomicBool;
    let cancel = std::sync::Arc::new(AtomicBool::new(true));
    let outcome = run(
        &r,
        &sd,
        &repo,
        &client,
        &[],
        &[],
        |e| captured.lock().unwrap().push(e),
        &cancel,
        |_| false,
    );

    assert_eq!(outcome, Outcome::Aborted);
    assert!(
        client.delete_file_calls.lock().unwrap().is_empty(),
        "no remote deletes must fire when cancelled",
    );
    assert!(
        repo.has_item(&local_path, "doomed.txt"),
        "DB row must survive a cancelled pass",
    );
}

/// The rate-limit probe firing during the list step short-circuits the
/// pass to `Outcome::Degraded`. No plan runs, no destructive actions
/// fire, DB rows are preserved.
#[test]
fn rate_limit_probe_short_circuits_to_degraded() {
    let tmp = TempDir::new("sync_rate_limit_probe");
    let local = tmp.write_file("a.txt", b"hi");
    touch_mtime(&local, 1_700_000_000);
    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        local.to_str().unwrap(),
        "a.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    // Listing is fine on its own — it's the probe that flags the pass.
    client.set_list("", Ok(vec![]));

    let r = remote(1, "TestRemote");
    let sd = sync_dir(1, 1, tmp.as_str(), "");
    let captured: Mutex<Vec<SyncEvent>> = Mutex::new(Vec::new());
    let cancel = cancel_never();
    let outcome = run(
        &r,
        &sd,
        &repo,
        &client,
        &[],
        &[],
        |e| captured.lock().unwrap().push(e),
        &cancel,
        |_| true,
    );

    assert_eq!(outcome, Outcome::Degraded);
    assert!(local.exists(), "local copy must survive degraded pass");
    assert!(
        repo.has_item(local.to_str().unwrap(), "a.txt"),
        "DB row must survive degraded pass",
    );
    assert!(
        client.delete_file_calls.lock().unwrap().is_empty(),
        "no remote deletes may fire on a degraded pass",
    );
}

/// A remote listing that's missing multiple tracked files under the
/// same parent (ProtonDrive rate-limit: partial-but-not-empty result)
/// must not cascade into mirror-delete-locally. Regression for the
/// 2026-04-18 ProtonDrive incident.
#[test]
fn delete_local_skipped_when_parent_listing_has_no_db_siblings() {
    let tmp = TempDir::new("sync_pd_listing_partial");
    // Five tracked siblings under "dir/" — all present locally, all
    // present in the DB, but the listing returns none of them (but
    // still enough root-level items to pass the global 2/3 threshold).
    let mut locals: Vec<PathBuf> = Vec::new();
    for i in 0..5 {
        let p = tmp.write_file(&format!("dir/file_{i}.txt"), b"x");
        touch_mtime(&p, 1_700_000_000);
        locals.push(p);
    }
    // Root-level padding so the global threshold (listing >= 2/3 of db)
    // still passes — isolates the per-parent check.
    let mut roots: Vec<PathBuf> = Vec::new();
    for i in 0..20 {
        let p = tmp.write_file(&format!("root_{i}.txt"), b"r");
        touch_mtime(&p, 1_700_000_000);
        roots.push(p);
    }

    let repo = FakeRepo::new();
    for (i, p) in locals.iter().enumerate() {
        repo.insert_item(
            SyncDirId(1),
            p.to_str().unwrap(),
            &format!("dir/file_{i}.txt"),
            1_700_000_000,
            1_700_000_000,
        );
    }
    for (i, p) in roots.iter().enumerate() {
        repo.insert_item(
            SyncDirId(1),
            p.to_str().unwrap(),
            &format!("root_{i}.txt"),
            1_700_000_000,
            1_700_000_000,
        );
    }

    let client = FakeBackend::default();
    // Listing: all root files present, NONE of the dir/ siblings
    // present (rate-limit returned an empty subfolder).
    let mut listing = Vec::new();
    for i in 0..20 {
        listing.push(remote_item(&format!("root_{i}.txt"), false, 1_700_000_000));
    }
    client.set_list("", Ok(listing));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced, "global threshold should pass");
    // None of the dir/ files should have been removed locally.
    for p in &locals {
        assert!(
            p.exists(),
            "local file must survive a listing missing all DB siblings: {}",
            p.display(),
        );
    }
    // Their DB rows must stick around for the next pass.
    for (i, p) in locals.iter().enumerate() {
        assert!(
            repo.has_item(p.to_str().unwrap(), &format!("dir/file_{i}.txt")),
            "DB row for dir/file_{i}.txt must survive",
        );
    }
}

/// A legitimate remote-side delete of a single file MUST still
/// propagate locally, even with the new sibling check — the parent
/// listing still shows other DB-tracked siblings.
#[test]
fn delete_local_still_fires_with_visible_sibling() {
    let tmp = TempDir::new("sync_pd_sibling_visible");
    let gone = tmp.write_file("dir/gone.txt", b"x");
    let sibling = tmp.write_file("dir/kept.txt", b"y");
    touch_mtime(&gone, 1_700_000_000);
    touch_mtime(&sibling, 1_700_000_000);

    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        gone.to_str().unwrap(),
        "dir/gone.txt",
        1_700_000_000,
        1_700_000_000,
    );
    repo.insert_item(
        SyncDirId(1),
        sibling.to_str().unwrap(),
        "dir/kept.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    // Listing has the sibling but not the deleted file.
    client.set_list(
        "",
        Ok(vec![remote_item("dir/kept.txt", false, 1_700_000_000)]),
    );

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(!gone.exists(), "legit delete must mirror locally");
    assert!(sibling.exists(), "unaffected sibling must survive");
    assert!(!repo.has_item(gone.to_str().unwrap(), "dir/gone.txt"));
    assert!(repo.has_item(sibling.to_str().unwrap(), "dir/kept.txt"));
}

/// A legitimate mass-delete + replace scenario: the user wiped all
/// DB-tracked files under a parent and populated it with new content.
/// The listing comes back non-empty but doesn't contain any of the
/// old DB-tracked items. The guard must NOT treat this as a rate-
/// limit glitch — a non-empty enumeration of the parent is proof the
/// list call succeeded, so the missing DB-tracked files were really
/// deleted and local copies should mirror the delete.
#[test]
fn delete_local_fires_when_listing_replaced_under_parent() {
    let tmp = TempDir::new("sync_pd_mass_replace");
    let old_a = tmp.write_file("dir/old_a.txt", b"x");
    let old_b = tmp.write_file("dir/old_b.txt", b"y");
    touch_mtime(&old_a, 1_700_000_000);
    touch_mtime(&old_b, 1_700_000_000);

    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        old_a.to_str().unwrap(),
        "dir/old_a.txt",
        1_700_000_000,
        1_700_000_000,
    );
    repo.insert_item(
        SyncDirId(1),
        old_b.to_str().unwrap(),
        "dir/old_b.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    // Listing: different content under the same parent — none of the
    // DB-tracked files survive, but two new ones are present. A true
    // rate-limit glitch would return zero items under dir/.
    client.set_list(
        "",
        Ok(vec![
            remote_item("dir/new_a.txt", false, 1_700_000_000),
            remote_item("dir/new_b.txt", false, 1_700_000_000),
        ]),
    );

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(
        !old_a.exists(),
        "old DB-tracked file must be removed locally when listing enumerates the parent",
    );
    assert!(
        !old_b.exists(),
        "second old DB-tracked file must be removed locally",
    );
    assert!(!repo.has_item(old_a.to_str().unwrap(), "dir/old_a.txt"));
    assert!(!repo.has_item(old_b.to_str().unwrap(), "dir/old_b.txt"));
}

/// The symmetric case: a local walk that's missing multiple tracked
/// files under the same parent (concurrent-writer race, permission
/// glitch) must not cascade into mirror-delete-remotely.
#[test]
fn delete_remote_skipped_when_parent_walk_has_no_db_siblings() {
    let tmp = TempDir::new("sync_pd_walk_partial");
    // The DB has five siblings under "dir/" and 20 at root, but the
    // walk sees none of the dir/ siblings (e.g. Syncthing just wiped
    // that subtree mid-pass). The listing still has everything.
    let repo = FakeRepo::new();
    for i in 0..5 {
        let local_path = format!("{}/dir/file_{i}.txt", tmp.as_str());
        repo.insert_item(
            SyncDirId(1),
            &local_path,
            &format!("dir/file_{i}.txt"),
            1_700_000_000,
            1_700_000_000,
        );
    }
    // Root-level padding passes the global threshold.
    let mut roots: Vec<PathBuf> = Vec::new();
    for i in 0..20 {
        let p = tmp.write_file(&format!("root_{i}.txt"), b"r");
        touch_mtime(&p, 1_700_000_000);
        repo.insert_item(
            SyncDirId(1),
            p.to_str().unwrap(),
            &format!("root_{i}.txt"),
            1_700_000_000,
            1_700_000_000,
        );
        roots.push(p);
    }

    let client = FakeBackend::default();
    let mut listing = Vec::new();
    for i in 0..20 {
        listing.push(remote_item(&format!("root_{i}.txt"), false, 1_700_000_000));
    }
    for i in 0..5 {
        listing.push(remote_item(&format!("dir/file_{i}.txt"), false, 1_700_000_000));
    }
    client.set_list("", Ok(listing));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(
        client.delete_file_calls.lock().unwrap().is_empty(),
        "no remote deletes must fire when walk has no DB siblings under the parent",
    );
    assert!(
        client.purge_calls.lock().unwrap().is_empty(),
        "no remote purges must fire either",
    );
    // DB rows under dir/ must survive for the next pass.
    for i in 0..5 {
        let local_path = format!("{}/dir/file_{i}.txt", tmp.as_str());
        assert!(
            repo.has_item(&local_path, &format!("dir/file_{i}.txt")),
            "DB row for dir/file_{i}.txt must survive",
        );
    }
}

/// A sub-folder the user emptied (it still exists, just without files) mirrors the deletes remotely; only a missing folder is a reason to doubt the walk.
#[test]
fn emptied_sub_folder_mirrors_deletes_remotely() {
    let tmp = TempDir::new("sync_emptied_dir");
    fs::create_dir_all(tmp.path.join("dir")).unwrap();
    let repo = FakeRepo::new();
    let mut listing = vec![remote_item("dir", true, 1_700_000_000)];
    for i in 0..3 {
        repo.insert_item(SyncDirId(1), &format!("{}/dir/file_{i}.txt", tmp.as_str()), &format!("dir/file_{i}.txt"), 1_700_000_000, 1_700_000_000);
        listing.push(remote_item(&format!("dir/file_{i}.txt"), false, 1_700_000_000));
    }
    let client = FakeBackend::default();
    client.set_list("", Ok(listing));

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert_eq!(client.delete_file_calls.lock().unwrap().len(), 3);
}

/// A folder that exists on both sides without a DB row gets one, so deleting it later on one side isn't undone from the other.
#[test]
fn untracked_folder_on_both_sides_is_recorded() {
    let tmp = TempDir::new("sync_record_dir");
    let dir = tmp.path.join("dir");
    fs::create_dir_all(&dir).unwrap();
    let repo = FakeRepo::new();
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("dir", true, 1_700_000_000)]));
    client.set_stat("dir", Ok(Some(remote_item("dir", true, 1_700_000_000))));

    let (outcome, events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(repo.has_item(dir.to_str().unwrap(), "dir"));
    // Bookkeeping isn't reported as a synced change.
    assert!(!events.iter().any(|e| matches!(e, SyncEvent::SyncDirStatus { text, .. } if text.starts_with("Done"))));
}

/// A walk I/O error on a subtree (simulated via chmod 000 so read_dir
/// fails) must NOT cascade into `DeleteRemote` for items under that
/// subtree. The unreliable set is the primary guard; the pass ends
/// without destructive calls to the client.
///
/// This is the regression for the 2026-04-18 ProtonDrive incident
/// where Syncthing was racing Celeste on the same tree: an entry's
/// `file_type()` returned ENOENT mid-walk, the entry got silently
/// dropped, the child appeared "missing locally", and `DeleteRemote`
/// trashed both.
#[test]
fn walk_read_dir_error_blocks_delete_remote_under_subtree() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("sync_walk_err");
    // Layout: one healthy file at root, one unreadable subtree with a
    // tracked child. The unreadable subtree is the simulation of the
    // failure mode.
    let healthy = tmp.write_file("ok.txt", b"x");
    touch_mtime(&healthy, 1_700_000_000);
    let trapped_dir = tmp.write_file("trap/child.txt", b"x");
    touch_mtime(&trapped_dir, 1_700_000_000);
    let trap_dir_path = tmp.path.join("trap");
    fs::set_permissions(&trap_dir_path, fs::Permissions::from_mode(0o000)).unwrap();
    // Cleanup guard — restore so TempDir can be dropped even if the
    // assertion below panics.
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }
    let _restore = Restore(trap_dir_path.clone());

    let repo = FakeRepo::new();
    repo.insert_item(
        SyncDirId(1),
        healthy.to_str().unwrap(),
        "ok.txt",
        1_700_000_000,
        1_700_000_000,
    );
    repo.insert_item(
        SyncDirId(1),
        trapped_dir.to_str().unwrap(),
        "trap/child.txt",
        1_700_000_000,
        1_700_000_000,
    );

    let client = FakeBackend::default();
    client.set_list(
        "",
        Ok(vec![
            remote_item("ok.txt", false, 1_700_000_000),
            remote_item("trap", true, 1_700_000_000),
            remote_item("trap/child.txt", false, 1_700_000_000),
        ]),
    );

    let (outcome, _events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(
        client.delete_file_calls.lock().unwrap().is_empty(),
        "no file deletes may fire while the walk reported an error on the subtree",
    );
    assert!(
        client.purge_calls.lock().unwrap().is_empty(),
        "no directory purges may fire either",
    );
    assert!(
        repo.has_item(trapped_dir.to_str().unwrap(), "trap/child.txt"),
        "DB row for trapped child must survive the pass",
    );
}

/// `ancestor_in_set` walks up to the empty-string root. Regression for
/// the edge where the unreliable flag is on the root itself.
#[test]
fn ancestor_in_set_walks_up_to_root() {
    let mut set = HashSet::new();
    assert!(!ancestor_in_set("a/b/c", &set));
    set.insert("a/b".to_owned());
    assert!(ancestor_in_set("a/b/c", &set));
    assert!(ancestor_in_set("a/b", &set));
    assert!(!ancestor_in_set("a", &set));

    let mut root_set = HashSet::new();
    root_set.insert(String::new());
    assert!(ancestor_in_set("any/path", &root_set));
    assert!(ancestor_in_set("a", &root_set));
}

/// Editor swap files are filtered from the local walk — never show up
/// as new items.
#[test]
fn editor_swap_files_are_not_seen() {
    let tmp = TempDir::new("sync_swap");
    let real = tmp.write_file("doc.txt", b"v1");
    touch_mtime(&real, 1_700_000_000);
    let swap = tmp.write_file(".doc.txt.kate-swp", b"tmp");
    touch_mtime(&swap, 1_700_000_000);

    let repo = FakeRepo::new();
    let client = FakeBackend::default();
    client.set_list("", Ok(vec![remote_item("doc.txt", false, 1_700_000_000)]));
    // doc.txt is brand new (no DB row); expected action: record it.
    // Stat after upsert returns the same item.
    client.set_stat(
        "doc.txt",
        Ok(Some(remote_item("doc.txt", false, 1_700_000_000))),
    );

    let (outcome, events) = run_full(&tmp, &repo, &client);
    assert_eq!(outcome, Outcome::Synced);
    assert!(errors(&events).is_empty());
    // No upload for the swap file.
    assert!(client.copy_to_remote_calls.lock().unwrap().is_empty());
}
