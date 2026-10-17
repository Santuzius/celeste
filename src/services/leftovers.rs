//! Local leftovers of an excluded path: files Syncative synced before the user excluded them, still sitting unchanged on this computer.
//!
//! Only those are offered for deletion. Files added or edited after the exclusion have no unchanged DB record and stay untouched, so a deliberately local-only folder can't lose anything.

use std::{collections::HashMap, fs, path::Path, time::SystemTime};

use crate::domain::{
    ports::Repository,
    sync::{SyncDirId, SyncItem},
};

/// Leftovers under an excluded path: synced-and-unchanged files, and synced folders that are empty once those are gone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub files: usize,
    pub folders: usize,
}

impl Found {
    pub fn is_empty(self) -> bool {
        self.files == 0 && self.folders == 0
    }

    /// "3 files", "1 folder", "3 files and 1 folder".
    pub fn describe(self) -> String {
        let files = match self.files {
            0 => None,
            1 => Some("1 file".to_owned()),
            n => Some(format!("{n} files")),
        };
        let folders = match self.folders {
            0 => None,
            1 => Some("1 folder".to_owned()),
            n => Some(format!("{n} folders")),
        };
        match (files, folders) {
            (Some(f), Some(d)) => format!("{f} and {d}"),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => "nothing".to_owned(),
        }
    }
}

/// What [`clean`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cleaned {
    pub deleted: Found,
    /// Files still under the path afterwards (added or changed since the exclusion).
    pub kept: usize,
}

fn under<'a>(items: &'a [SyncItem], root: &str) -> impl Iterator<Item = &'a SyncItem> {
    let prefix = format!("{root}/");
    items.iter().filter(move |i| i.local_path == root || i.local_path.starts_with(&prefix))
}

fn mtime_secs(meta: &fs::Metadata) -> Option<i64> {
    let at = meta.modified().ok()?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(at.as_secs() as i64)
}

/// A tracked file that is still exactly what was synced.
fn is_stale_copy(item: &SyncItem) -> bool {
    fs::symlink_metadata(&item.local_path)
        .ok()
        .filter(fs::Metadata::is_file)
        .and_then(|m| mtime_secs(&m))
        .is_some_and(|secs| secs == item.last_local_timestamp)
}

/// Whether the tracked folder `path` would be removed by [`clean`]: everything below it is a stale copy or such a folder.
fn is_removable_dir(tracked: &HashMap<&Path, &SyncItem>, path: &Path) -> bool {
    if !tracked.contains_key(path) || !fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().all(|e| match e.file_type() {
        Ok(t) if t.is_dir() => is_removable_dir(tracked, &e.path()),
        Ok(_) => tracked.get(e.path().as_path()).is_some_and(|i| is_stale_copy(i)),
        Err(_) => false,
    })
}

/// The synced-and-unchanged files under `root`, and the synced folders that would be left empty without them (empty synced folders included).
pub fn count(items: &[SyncItem], root: &str) -> Found {
    let tracked: HashMap<&Path, &SyncItem> = under(items, root).map(|i| (Path::new(i.local_path.as_str()), i)).collect();
    let mut found = Found::default();
    for (path, item) in &tracked {
        if is_stale_copy(item) {
            found.files += 1;
        } else if is_removable_dir(&tracked, path) {
            found.folders += 1;
        }
    }
    found
}

/// Delete the synced-and-unchanged files under `root`, then every folder that ends up empty, and forget their DB rows. Forgetting matters: a stale row for a deleted file would read as "deleted locally" once the exclusion is lifted, and the cloud copy would go too.
pub async fn clean(repo: &dyn Repository, sync_dir: SyncDirId, root: &str) -> Result<Cleaned, String> {
    let items = repo.list_sync_items(sync_dir).await.map_err(|e| e.to_string())?;
    let mut deleted = Found::default();
    let mut dirs: Vec<&SyncItem> = Vec::new();
    for item in under(&items, root) {
        match fs::symlink_metadata(&item.local_path) {
            Ok(meta) if meta.is_dir() => {
                dirs.push(item);
                continue;
            }
            Ok(_) if is_stale_copy(item) => {
                fs::remove_file(&item.local_path).map_err(|e| format!("{}: {e}", item.local_path))?;
                deleted.files += 1;
            }
            // Changed since the last sync: keep the file and its row.
            Ok(_) => continue,
            // Already gone; the row is stale either way.
            Err(_) => {}
        }
        let _ = repo.delete_sync_item_by_paths(sync_dir, &item.local_path, &item.remote_path).await;
    }

    // Deepest first, so parents are empty by the time they come up. `remove_dir` refuses non-empty folders, which keeps whatever is left.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.local_path.len()));
    for dir in dirs {
        if fs::remove_dir(&dir.local_path).is_ok() {
            deleted.folders += 1;
            let _ = repo.delete_sync_item_by_paths(sync_dir, &dir.local_path, &dir.remote_path).await;
        }
    }
    let _ = fs::remove_dir(root);

    Ok(Cleaned { deleted, kept: count_files(Path::new(root)) })
}

fn count_files(path: &Path) -> usize {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => count_files(&e.path()),
            _ => 1,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{touch_mtime, FakeRepo, TempDir};

    #[test]
    fn deletes_only_unchanged_synced_files() {
        let tmp = TempDir::new("leftovers");
        let root = format!("{}/Raw", tmp.as_str());
        let repo = FakeRepo::new();
        let synced = tmp.write_file("Raw/sub/old.mov", b"x");
        touch_mtime(&synced, 1_700_000_000);
        repo.insert_item(SyncDirId(1), synced.to_str().unwrap(), "Raw/sub/old.mov", 1_700_000_000, 1_700_000_000);
        repo.insert_item(SyncDirId(1), &format!("{root}/sub"), "Raw/sub", 1_700_000_000, 1_700_000_000);
        let edited = tmp.write_file("Raw/edited.mov", b"y");
        touch_mtime(&edited, 1_800_000_000);
        repo.insert_item(SyncDirId(1), edited.to_str().unwrap(), "Raw/edited.mov", 1_700_000_000, 1_700_000_000);
        let local_only = tmp.write_file("Raw/new.mov", b"z");
        // A sibling outside the excluded path must not be touched.
        let outside = tmp.write_file("Rawer/keep.mov", b"k");
        touch_mtime(&outside, 1_700_000_000);
        repo.insert_item(SyncDirId(1), outside.to_str().unwrap(), "Rawer/keep.mov", 1_700_000_000, 1_700_000_000);

        let items = crate::util::await_future(repo.list_sync_items(SyncDirId(1))).unwrap();
        assert_eq!(count(&items, &root), Found { files: 1, folders: 1 });

        let cleaned = crate::util::await_future(clean(&repo, SyncDirId(1), &root)).unwrap();
        assert_eq!(cleaned, Cleaned { deleted: Found { files: 1, folders: 1 }, kept: 2 });
        assert!(!synced.exists() && !tmp.path.join("Raw/sub").exists());
        assert!(edited.exists() && local_only.exists() && outside.exists());
        assert!(!repo.has_item(synced.to_str().unwrap(), "Raw/sub/old.mov"));
        assert!(repo.has_item(edited.to_str().unwrap(), "Raw/edited.mov"));
        assert!(repo.has_item(outside.to_str().unwrap(), "Rawer/keep.mov"));
    }

    #[test]
    fn removes_the_emptied_folder_itself() {
        let tmp = TempDir::new("leftovers_root");
        let root = format!("{}/Raw", tmp.as_str());
        let repo = FakeRepo::new();
        let synced = tmp.write_file("Raw/a.txt", b"a");
        touch_mtime(&synced, 1_700_000_000);
        repo.insert_item(SyncDirId(1), synced.to_str().unwrap(), "Raw/a.txt", 1_700_000_000, 1_700_000_000);

        let cleaned = crate::util::await_future(clean(&repo, SyncDirId(1), &root)).unwrap();
        assert_eq!(cleaned, Cleaned { deleted: Found { files: 1, folders: 0 }, kept: 0 });
        assert!(!Path::new(&root).exists());
    }

    #[test]
    fn offers_and_removes_empty_synced_folders() {
        let tmp = TempDir::new("leftovers_empty");
        let root = format!("{}/Raw", tmp.as_str());
        let repo = FakeRepo::new();
        for rel in ["Raw", "Raw/a", "Raw/a/b"] {
            std::fs::create_dir_all(tmp.path.join(rel)).unwrap();
            repo.insert_item(SyncDirId(1), &format!("{}/{rel}", tmp.as_str()), rel, 1_700_000_000, 1_700_000_000);
        }
        // An empty folder made after the exclusion has no row and keeps its parent.
        std::fs::create_dir_all(tmp.path.join("Raw/c/new")).unwrap();
        repo.insert_item(SyncDirId(1), &format!("{root}/c"), "Raw/c", 1_700_000_000, 1_700_000_000);

        let items = crate::util::await_future(repo.list_sync_items(SyncDirId(1))).unwrap();
        assert_eq!(count(&items, &root), Found { files: 0, folders: 2 });

        let cleaned = crate::util::await_future(clean(&repo, SyncDirId(1), &root)).unwrap();
        assert_eq!(cleaned.deleted, Found { files: 0, folders: 2 });
        assert!(!tmp.path.join("Raw/a").exists());
        assert!(tmp.path.join("Raw/c/new").exists());
    }
}
