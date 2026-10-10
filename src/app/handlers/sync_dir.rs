//! Sync_dir-level handlers: list load, add/delete, exclusion CRUD,
//! per-remote settings (interval + enabled toggle), and the read-only
//! log editor's scroll/select pass-through.

use std::{collections::HashMap, sync::Arc};

use iced::{widget::text_editor, Task};

use crate::{
    domain::{
        ports::{BackendClient, Repository},
        remote::RemoteId,
        sync::{SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId},
    },
    engine::Command,
    screens::{main_page, remote_page, settings},
    services::leftovers,
};

use super::super::{local_paths_overlap, remote_page_auto_excluded, CelesteApp, Message};

impl CelesteApp {
    /// Handle [`Message::SyncDirsLoaded`] — store the list and drop the
    /// UI state of deleted folders.
    pub(in crate::app) fn handle_sync_dirs_loaded(
        &mut self,
        id: RemoteId,
        sd: Vec<SyncDir>,
    ) -> Task<Message> {
        let ids: Vec<SyncDirId> = sd.iter().map(|d| d.id).collect();
        if let Some(old) = self.sync_dirs.get(&id) {
            for gone in old.iter().map(|d| d.id).filter(|d| !ids.contains(d)) {
                self.sync_dir_log_content.remove(&gone);
                self.sync_dir_exclusions.remove(&gone);
                self.exclusion_leftovers.remove(&gone);
                self.draft_exclusion.remove(&gone);
            }
        }
        self.sync_dirs.insert(id, sd);
        self.refresh_exclusions()
    }

    /// Handle [`Message::AllSyncDirsRefreshed`] — replace the global
    /// auto-exclusion lookup table.
    pub(in crate::app) fn handle_all_sync_dirs_refreshed(
        &mut self,
        all: Vec<SyncDir>,
    ) -> Task<Message> {
        self.all_known_sync_dirs = all;
        // A new folder inside another one turns the old copy into leftovers.
        self.refresh_exclusions()
    }

    /// Reload exclusions and leftover counts of every folder on the open page, so the filter count and the broom are right without opening the panel first.
    pub(in crate::app) fn refresh_exclusions(&self) -> Task<Message> {
        let dirs = self.selected.and_then(|id| self.sync_dirs.get(&id)).map_or(&[][..], |v| v.as_slice());
        Task::batch(dirs.iter().map(|d| self.reload_exclusions(d.id, |_| async {})))
    }

    /// Handle [`remote_page::Msg::DraftLocalPathChanged`] / [`DraftRemotePathChanged`].
    pub(in crate::app) fn handle_draft_local_path_changed(&mut self, s: String) -> Task<Message> {
        self.add_sync_dir_error = None;
        if let Some(id) = self.selected {
            self.sync_dir_drafts.entry(id).or_default().0 = s;
        }
        Task::none()
    }

    pub(in crate::app) fn handle_draft_remote_path_changed(&mut self, s: String) -> Task<Message> {
        if let Some(id) = self.selected {
            self.sync_dir_drafts.entry(id).or_default().1 = s;
        }
        Task::none()
    }

    /// Handle [`remote_page::Msg::AddSyncDir`] — normalise input,
    /// reject overlapping local paths, auto-create the directories,
    /// and insert the DB row. Problems are shown under the form.
    pub(in crate::app) fn handle_add_sync_dir(&mut self) -> Task<Message> {
        let Some(id) = self.selected else {
            return Task::none();
        };
        let Some((local, remote)) = self.sync_dir_drafts.get(&id).cloned() else {
            return Task::none();
        };
        let Some(remote_name) =
            self.remotes.iter().find(|r| r.id == id).map(|r| r.name.clone())
        else {
            return Task::none();
        };
        let local = expand_home(local.trim());
        if local.is_empty() {
            self.add_sync_dir_error = Some(format!("Choose a folder on {}.", crate::util::THIS_DEVICE));
            return Task::none();
        }
        if !local.starts_with('/') {
            let example = if cfg!(target_os = "android") { "/storage/emulated/0/Documents" } else { "/home/you/Documents or ~/Documents" };
            self.add_sync_dir_error = Some(format!("Use an absolute path, e.g. {example}."));
            return Task::none();
        }
        // Normalise to match the on-disk contract: the local path is
        // absolute (leading `/`) and has no trailing `/`; the remote
        // path has no leading or trailing `/`. The sync loop assumes
        // this shape when stripping prefixes off listed items.
        let local_norm = format!("/{}", crate::util::strip_slashes(&local));
        let remote_norm = crate::util::strip_slashes(remote.trim());
        // Sync_dirs must be local siblings — overlapping local trees
        // would have the engine walking the same files twice with
        // conflicting tracking.
        if let Some(conflict) = self
            .all_known_sync_dirs
            .iter()
            .find(|d| local_paths_overlap(&d.local_path, &local_norm))
        {
            self.add_sync_dir_error = Some(format!(
                "{} overlaps the already synced folder {}. Synced folders can't contain each other.",
                crate::util::fmt_home(&local_norm),
                crate::util::fmt_home(&conflict.local_path),
            ));
            return Task::none();
        }
        self.add_sync_dir_error = None;
        let repo = self.repo.clone();
        let rclone = self.rclone.clone();
        Task::perform(
            async move {
                // Auto-create local + remote directory if missing so
                // the next sync pass doesn't immediately trip
                // "directory not found" on the listing call.
                let local_for_mk = local_norm.clone();
                let remote_for_mk = remote_norm.clone();
                let created = tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&local_for_mk)
                        .map_err(|e| format!("Couldn't create {local_for_mk}: {e}"))?;
                    let _ = rclone.mkdir(
                        &remote_name,
                        &remote_for_mk,
                        &crate::domain::ports::cancel_never(),
                    );
                    Ok::<(), String>(())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                let result = match created {
                    Ok(()) => repo
                        .insert_sync_dir(id, local_norm, remote_norm)
                        .await
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                (id, result)
            },
            |(id, result)| Message::SyncDirAdded(id, result),
        )
    }

    /// Handle [`Message::SyncDirAdded`] — clear the form and reload on
    /// success, keep the input and show the reason otherwise.
    pub(in crate::app) fn handle_sync_dir_added(
        &mut self,
        id: RemoteId,
        result: Result<(), String>,
    ) -> Task<Message> {
        match result {
            Ok(()) => {
                self.sync_dir_drafts.remove(&id);
                self.add_sync_dir_error = None;
                self.add_folder_open = false;
                // Start syncing the new folder right away.
                self.engine.send(Command::Reload);
                self.engine.send(Command::SyncSoon(id));
                self.handle_remote_selected(id)
            }
            Err(err) => {
                self.add_sync_dir_error = Some(err);
                Task::none()
            }
        }
    }

    /// Handle [`remote_page::Msg::BrowseLocalPath`] — open the desktop's
    /// folder chooser off the UI thread.
    pub(in crate::app) fn handle_browse_local_path(&mut self) -> Task<Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(|| {
                    crate::infrastructure::portal::pick_folder("Choose a folder to sync")
                })
                .await
                .ok()
                .flatten()
            },
            Message::LocalPathPicked,
        )
    }

    /// Handle [`Message::LocalPathPicked`] — fill the local field. The remote field stays as it is: empty means the whole drive, which is what a folder named after the drive is usually meant for.
    pub(in crate::app) fn handle_local_path_picked(&mut self, path: Option<String>) -> Task<Message> {
        let (Some(path), Some(id)) = (path, self.selected) else {
            return Task::none();
        };
        self.sync_dir_drafts.entry(id).or_default().0 = path;
        self.add_sync_dir_error = None;
        Task::none()
    }

    /// Handle [`remote_page::Msg::ToggleLog`] — expand / collapse a
    /// folder's activity log. The shaped editor content only exists
    /// while expanded.
    pub(in crate::app) fn handle_toggle_log(&mut self, sd_id: SyncDirId) -> Task<Message> {
        if self.sync_dir_log_content.remove(&sd_id).is_none() {
            self.sync_dir_log_content.insert(sd_id, self.build_log_content(sd_id));
        }
        Task::none()
    }

    /// Handle [`remote_page::Msg::DeleteSyncDir`].
    pub(in crate::app) fn handle_delete_sync_dir(
        &mut self,
        local: String,
        remote: String,
    ) -> Task<Message> {
        let Some(id) = self.selected else {
            return Task::none();
        };
        let repo = self.repo.clone();
        let engine = self.engine.clone();
        Task::perform(
            async move {
                if let Err(err) = repo.cascade_delete_sync_dir(&local, &remote).await {
                    eprintln!("celeste: could not remove the folder {local}: {err}");
                }
                engine.send(Command::Reload);
                id
            },
            |id| Message::Main(main_page::Msg::Selected(id)),
        )
    }

    /// Handle [`remote_page::Msg::Settings`] / [`Message::Settings`] —
    /// update the in-memory policy, persist it asynchronously and then
    /// tell the engine, which stops a running pass when the user disables
    /// the remote.
    pub(in crate::app) fn handle_settings(&mut self, sub: settings::Msg) -> Task<Message> {
        let Some(id) = self.selected else {
            return Task::none();
        };
        let Some(remote) = self.remotes.iter_mut().find(|r| r.id == id) else {
            return Task::none();
        };
        let Some(new_policy) = settings::policy_from(&sub, &remote.policy) else {
            // Account / removal actions are routed through the remote
            // page's own messages; nothing to persist here.
            return Task::none();
        };
        remote.policy = new_policy.clone();
        let repo = self.repo.clone();
        let engine = self.engine.clone();
        Task::perform(
            async move {
                let _ = repo.set_policy(id, new_policy).await;
                engine.send(Command::Reload);
            },
            |_| Message::PolicySaved,
        )
    }

    /// Handle [`Message::ExclusionsLoaded`].
    pub(in crate::app) fn handle_exclusions_loaded(
        &mut self,
        sd_id: SyncDirId,
        excls: Vec<SyncDirExclusion>,
        leftovers: HashMap<String, usize>,
    ) -> Task<Message> {
        self.exclusion_leftovers.insert(sd_id, leftovers);
        self.sync_dir_exclusions.insert(sd_id, excls);
        Task::none()
    }

    /// Run `change` against the repository, then reload the sync_dir's exclusions and count the local leftovers under each, including sub-folders now synced as their own folder.
    fn reload_exclusions<F, Fut>(&self, sd_id: SyncDirId, change: F) -> Task<Message>
    where
        F: FnOnce(Arc<dyn Repository>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let repo = self.repo.clone();
        let sd = self.all_known_sync_dirs.iter().find(|d| d.id == sd_id);
        let local_root = sd.map(|d| d.local_path.clone());
        let auto: Vec<String> = sd
            .map(|sd| remote_page_auto_excluded(sd, &self.all_known_sync_dirs).into_iter().map(|d| remote_page::relative_to(sd, d).to_owned()).collect())
            .unwrap_or_default();
        Task::perform(
            async move {
                change(repo.clone()).await;
                let excls = repo.list_exclusions(sd_id).await.unwrap_or_default();
                let mut counts = HashMap::new();
                if let Some(root) = local_root {
                    let items = repo.list_sync_items(sd_id).await.unwrap_or_default();
                    for relative in excls.iter().map(|e| &e.remote_path).chain(&auto) {
                        let n = leftovers::count(&items, &format!("{root}/{relative}"));
                        if n > 0 {
                            counts.insert(relative.clone(), n);
                        }
                    }
                }
                (excls, counts)
            },
            move |(excls, counts)| Message::ExclusionsLoaded(sd_id, excls, counts),
        )
    }

    /// Handle [`remote_page::Msg::RequestCleanLeftovers`] — ask before deleting anything.
    pub(in crate::app) fn handle_request_clean_leftovers(&mut self, sd_id: SyncDirId, relative: String) -> Task<Message> {
        let Some(sd) = self.all_known_sync_dirs.iter().find(|d| d.id == sd_id) else {
            return Task::none();
        };
        let Some(count) = self.exclusion_leftovers.get(&sd_id).and_then(|m| m.get(&relative)).copied() else {
            return Task::none();
        };
        let remote_name = self.remotes.iter().find(|r| r.id == sd.remote_id).map(|r| r.name.clone()).unwrap_or_default();
        self.pending_delete = Some(remote_page::PendingDelete::Leftovers {
            sync_dir: sd_id,
            root: format!("{}/{}", sd.local_path, relative),
            relative,
            remote_name,
            count,
        });
        Task::none()
    }

    /// Confirmed: delete the leftovers off the UI thread.
    pub(in crate::app) fn handle_clean_leftovers(&mut self, sd_id: SyncDirId, root: String, relative: String) -> Task<Message> {
        let repo = self.repo.clone();
        Task::perform(async move { leftovers::clean(repo.as_ref(), sd_id, &root).await }, move |res| {
            Message::LeftoversCleaned(sd_id, relative.clone(), res)
        })
    }

    /// Handle [`Message::LeftoversCleaned`] — report in the folder's log and refresh the button.
    pub(in crate::app) fn handle_leftovers_cleaned(&mut self, sd_id: SyncDirId, relative: String, res: Result<leftovers::Cleaned, String>) -> Task<Message> {
        let line = match res {
            Ok(leftovers::Cleaned { deleted, kept: 0 }) => tr::tr!("Deleted {} leftover files of '{}'.", deleted, relative),
            Ok(leftovers::Cleaned { deleted, kept }) => tr::tr!("Deleted {} leftover files of '{}'; kept {} added or changed since.", deleted, relative, kept),
            Err(err) => format!("⚠ {}", tr::tr!("Couldn't delete the leftovers of '{}': {}", relative, err)),
        };
        self.engine.send(Command::Log(sd_id, line));
        self.reload_exclusions(sd_id, |_| async {})
    }

    /// Handle [`remote_page::Msg::ToggleExclusions`] — toggle the panel
    /// open/closed; loads the exclusions list when opening.
    pub(in crate::app) fn handle_toggle_exclusions(&mut self, sd_id: SyncDirId) -> Task<Message> {
        if self.exclusion_panel == Some(sd_id) {
            self.exclusion_panel = None;
            Task::none()
        } else {
            self.exclusion_panel = Some(sd_id);
            self.reload_exclusions(sd_id, |_| async {})
        }
    }

    pub(in crate::app) fn handle_draft_exclusion_changed(
        &mut self,
        sd_id: SyncDirId,
        s: String,
    ) -> Task<Message> {
        self.draft_exclusion.insert(sd_id, s);
        Task::none()
    }

    pub(in crate::app) fn handle_add_exclusion(&mut self, sd_id: SyncDirId) -> Task<Message> {
        let raw = self
            .draft_exclusion
            .get(&sd_id)
            .cloned()
            .unwrap_or_default();
        let path = crate::util::strip_slashes(raw.trim());
        if path.is_empty() {
            return Task::none();
        }
        self.draft_exclusion.insert(sd_id, String::new());
        self.reload_exclusions(sd_id, move |repo| async move {
            let _ = repo.insert_exclusion(sd_id, path).await;
        })
    }

    pub(in crate::app) fn handle_remove_exclusion(
        &mut self,
        excl_id: SyncDirExclusionId,
        sd_id: SyncDirId,
    ) -> Task<Message> {
        self.reload_exclusions(sd_id, move |repo| async move {
            let _ = repo.delete_exclusion(excl_id).await;
        })
    }

    /// Handle [`remote_page::Msg::LogEditorAction`] — drop edit
    /// actions, but feed scroll / select / click / drag through so the
    /// user can still navigate the history pane.
    pub(in crate::app) fn handle_log_editor_action(
        &mut self,
        sd_id: SyncDirId,
        action: text_editor::Action,
    ) -> Task<Message> {
        if !action.is_edit()
            && let Some(content) = self.sync_dir_log_content.get_mut(&sd_id)
        {
            content.perform(action);
        }
        Task::none()
    }
}

/// Expand a leading `~` to `$HOME`, so typed paths can use the same shorthand the UI displays.
fn expand_home(path: &str) -> String {
    match (path.strip_prefix('~'), crate::util::user_home()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_owned(),
    }
}
