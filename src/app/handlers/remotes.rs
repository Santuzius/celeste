//! Remote-level message handlers: load list, add/reauth, delete,
//! refresh, navigation.

use iced::Task;

use crate::{
    domain::{
        ports::BackendClient,
        remote::{ProviderKind, Remote, RemoteId},
    },
    screens::{add_remote, remote_page},
};

use super::super::{map_domain_provider_to_add_remote, CelesteApp, Message};

impl CelesteApp {
    /// Handle [`Message::RemotesLoaded`] — refresh the in-memory list
    /// and sync per-remote enabled flags into the state machine so
    /// roll-ups reflect the latest policy.
    pub(in crate::app) fn handle_remotes_loaded(
        &mut self,
        mut remotes: Vec<Remote>,
    ) -> Task<Message> {
        // Ask rclone for each remote's backend type so the scheduler
        // can enforce provider-specific interval floors (see
        // `ProviderKind::min_interval`). A failure here is non-fatal —
        // the remote just loses its provider-specific floor for this
        // session.
        for r in &mut remotes {
            if let Ok(Some(t)) = self.rclone.remote_type(&r.name) {
                r.provider_kind = ProviderKind::from_rclone_type(&t);
            }
        }
        self.remotes = remotes;
        for r in &self.remotes {
            self.sync_state.ensure_remote(r.id, r.policy.enabled);
            // A session that already failed to resume at startup
            // needs reauth even if the remote is (auto-)paused and so
            // never gets a sync pass that would discover it.
            if self.rclone.needs_reauth(&r.name) {
                self.sync_state.auth_failure(r.id);
            }
        }
        self.shared_oauth_client = self
            .remotes
            .iter()
            .filter(|r| self.rclone.uses_shared_oauth_client(&r.name))
            .map(|r| r.id)
            .collect();
        // Keep a remote selected whenever there is one, so the window
        // never opens on an empty pane; (re)load its folders if needed.
        let selected = self
            .selected
            .filter(|id| self.remotes.iter().any(|r| r.id == *id))
            .or_else(|| self.remotes.first().map(|r| r.id));
        let repo = self.repo.clone();
        let all_dirs = Task::perform(
            async move { repo.list_all_sync_dirs().await.unwrap_or_default() },
            Message::AllSyncDirsRefreshed,
        );
        let select = match selected {
            Some(id) if self.selected != Some(id) || !self.sync_dirs.contains_key(&id) => {
                self.handle_remote_selected(id)
            }
            Some(_) => Task::none(),
            None => {
                self.selected = None;
                Task::none()
            }
        };
        Task::batch([all_dirs, select])
    }

    /// Handle [`main_page::Msg::Selected`] — navigate to a remote and
    /// kick off two parallel reads (this remote's sync_dirs + the
    /// global sync_dirs list for auto-exclusion).
    pub(in crate::app) fn handle_remote_selected(&mut self, id: RemoteId) -> Task<Message> {
        if self.selected != Some(id) {
            // Per-page UI state doesn't carry over to another remote;
            // expanded logs are dropped to free their text buffers.
            self.add_sync_dir_error = None;
            self.exclusion_panel = None;
            self.settings_open = false;
            self.sync_dir_log_content.clear();
        }
        self.selected = Some(id);
        let repo = self.repo.clone();
        let repo2 = self.repo.clone();
        Task::batch([
            Task::perform(
                async move { repo.list_sync_dirs(id).await.unwrap_or_default() },
                move |sd| Message::SyncDirsLoaded(id, sd),
            ),
            Task::perform(
                async move { repo2.list_all_sync_dirs().await.unwrap_or_default() },
                Message::AllSyncDirsRefreshed,
            ),
        ])
    }

    /// Handle [`main_page::Msg::RefreshAll`] — start a sync pass for
    /// every enabled, idle remote.
    pub(in crate::app) fn handle_refresh_all(&mut self) -> Task<Message> {
        let ids: Vec<RemoteId> = self
            .remotes
            .iter()
            .filter(|r| self.is_schedulable(r.id) && !self.syncing.contains(&r.id))
            .map(|r| r.id)
            .collect();
        let cmds: Vec<Task<Message>> =
            ids.into_iter().map(|id| self.start_sync(id)).collect();
        Task::batch(cmds)
    }

    /// Handle [`main_page::Msg::AddRemote`] — open the Add Remote
    /// dialog with a fresh draft.
    pub(in crate::app) fn handle_open_add_remote(&mut self) -> Task<Message> {
        self.add_remote_draft = Some(add_remote::Draft::default());
        Task::none()
    }

    /// Handle [`Message::AddRemote`] — drive the add/reauth dialog
    /// state machine (form-field updates plus `Submit` dispatching to
    /// the appropriate auth flow).
    pub(in crate::app) fn handle_add_remote_msg(
        &mut self,
        sub: add_remote::Msg,
    ) -> Task<Message> {
        let Some(draft) = self.add_remote_draft.as_mut() else {
            return Task::none();
        };
        match sub {
            add_remote::Msg::NameChanged(s) => draft.name = s,
            add_remote::Msg::ProviderChanged(p) => {
                // Suggest a name (unique among the existing remotes)
                // until the user types their own.
                let suggested = |p: add_remote::ProviderKind| p.to_string().replace(' ', "");
                if draft.name.trim().is_empty() || draft.provider.is_some_and(|old| draft.name.starts_with(&suggested(old))) {
                    let base = suggested(p);
                    let taken = |n: &str| self.remotes.iter().any(|r| r.name == n);
                    draft.name = (1..)
                        .map(|i| if i == 1 { base.clone() } else { format!("{base}{i}") })
                        .find(|n| !taken(n))
                        .unwrap_or(base);
                }
                draft.provider = Some(p);
            }
            add_remote::Msg::UrlChanged(s) => draft.url = s,
            add_remote::Msg::UserChanged(s) => draft.user = s,
            add_remote::Msg::PassChanged(s) => draft.pass = s,
            add_remote::Msg::TotpChanged(s) => draft.totp = s,
            add_remote::Msg::ClientIdChanged(s) => draft.client_id = s,
            add_remote::Msg::ClientSecretChanged(s) => draft.client_secret = s,
            add_remote::Msg::OpenVerification => {
                if let Some(hv) = &draft.hv {
                    open_in_browser(&hv.url());
                }
            }
            add_remote::Msg::Cancel => {
                // Stops a pending `rclone authorize`; its result then
                // arrives as a cancellation and is ignored.
                if let Some(handle) = &draft.oauth {
                    handle.cancel();
                }
                self.add_remote_draft = None;
                return Task::none();
            }
            add_remote::Msg::OpenClientIdGuide => open_in_browser(add_remote::GDRIVE_CLIENT_ID_GUIDE),
            add_remote::Msg::CopyPrivacyLink => {
                draft.privacy_link_copied = true;
                return iced::clipboard::write(add_remote::PRIVACY_POLICY.to_owned());
            }
            add_remote::Msg::OpenAuthLink => {
                if let Some(url) = draft.oauth.as_ref().and_then(|h| h.url()) {
                    open_in_browser(&url);
                }
            }
            add_remote::Msg::CopyAuthLink => {
                if let Some(url) = draft.oauth.as_ref().and_then(|h| h.url()) {
                    return iced::clipboard::write(url);
                }
            }
            add_remote::Msg::Submit => {
                let Some(kind) = draft.provider else {
                    draft.error = Some("Pick a provider first.".to_owned());
                    return Task::none();
                };
                if draft.name.trim().is_empty() {
                    draft.error = Some("Name is required.".to_owned());
                    return Task::none();
                }
                let name = draft.name.clone();
                let repo = self.repo.clone();
                let rclone = self.rclone.clone();
                draft.error = None;

                if let Some(vendor) = kind.webdav_vendor() {
                    if draft.url.trim().is_empty() || draft.user.trim().is_empty() {
                        draft.error =
                            Some("URL and username are required.".to_owned());
                        return Task::none();
                    }
                    let url = draft.url.clone();
                    let user = draft.user.clone();
                    let pass = draft.pass.clone();
                    draft.busy = true;
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                crate::services::auth::add_webdav_remote(
                                    &name, &url, &user, &pass, vendor, &*repo, &*rclone,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        Message::AddRemoteResult,
                    );
                }

                if kind.is_proton_drive() {
                    if draft.user.trim().is_empty() {
                        draft.error = Some("Username is required.".to_owned());
                        return Task::none();
                    }
                    let user = draft.user.clone();
                    let pass = draft.pass.clone();
                    let totp = draft.totp.clone();
                    let hv = draft.hv.clone();
                    let router = self.rclone.clone();
                    let is_reauth = draft.reauth;
                    draft.busy = true;
                    if is_reauth {
                        // Reauth: keep the existing DB row, just swap
                        // the router's disabled stub for a live
                        // session. Look up the existing id so
                        // AddRemoteResult can reuse the normal refresh
                        // path.
                        let existing_id = self
                            .remotes
                            .iter()
                            .find(|r| r.name == name)
                            .map(|r| r.id);
                        return Task::perform(
                            async move {
                                let router_inner = router.clone();
                                let res = tokio::task::spawn_blocking(move || {
                                    crate::services::auth::reauth_proton_drive_remote(
                                        &name,
                                        &user,
                                        &pass,
                                        &totp,
                                        hv.as_ref(),
                                        &*router_inner,
                                    )
                                })
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()));
                                match (res, existing_id) {
                                    (Ok(()), Some(id)) => Ok(id),
                                    (Ok(()), None) => {
                                        Err("Remote not found after reauth.".to_owned())
                                    }
                                    (Err(e), _) => Err(e),
                                }
                            },
                            Message::AddRemoteResult,
                        );
                    }
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                crate::services::auth::add_proton_drive_remote(
                                    &name,
                                    &user,
                                    &pass,
                                    &totp,
                                    hv.as_ref(),
                                    &*repo,
                                    &*router,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        Message::AddRemoteResult,
                    );
                }

                if let Some(provider) = kind.oauth_provider() {
                    let client_id = draft.client_id.trim().to_owned();
                    let client_secret = draft.client_secret.trim().to_owned();
                    if kind.needs_own_client_id() && (client_id.is_empty() || client_secret.is_empty()) {
                        draft.error = Some(
                            "Enter your own client ID and secret — see \"How to create a client ID\".".to_owned(),
                        );
                        return Task::none();
                    }
                    let is_reauth = draft.reauth;
                    let handle = std::sync::Arc::new(crate::services::auth::AuthorizeHandle::default());
                    draft.oauth = Some(handle.clone());
                    draft.busy = true;
                    if is_reauth {
                        // Reauth: keep the existing DB row, just
                        // refresh the rclone config under the same
                        // name so the live token gets replaced.
                        // Inserting a new row would create a
                        // duplicate-name entry and make subsequent
                        // delete-by-name flows ambiguous.
                        let existing_id = self
                            .remotes
                            .iter()
                            .find(|r| r.name == name)
                            .map(|r| r.id);
                        let rclone_inner = rclone.clone();
                        return Task::perform(
                            async move {
                                let res = tokio::task::spawn_blocking(move || {
                                    let client_id = (!client_id.is_empty())
                                        .then_some(client_id.as_str());
                                    let client_secret = (!client_secret.is_empty())
                                        .then_some(client_secret.as_str());
                                    crate::services::auth::reauth_oauth_remote(
                                        &name,
                                        provider,
                                        client_id,
                                        client_secret,
                                        &handle,
                                        &*rclone_inner,
                                    )
                                })
                                .await
                                .unwrap_or_else(|e| Err(e.to_string()));
                                match (res, existing_id) {
                                    (Ok(()), Some(id)) => Ok(id),
                                    (Ok(()), None) => Err(
                                        "Remote not found after reauth.".to_owned(),
                                    ),
                                    (Err(e), _) => Err(e),
                                }
                            },
                            Message::AddRemoteResult,
                        );
                    }
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                let client_id = (!client_id.is_empty())
                                    .then_some(client_id.as_str());
                                let client_secret = (!client_secret.is_empty())
                                    .then_some(client_secret.as_str());
                                crate::services::auth::add_oauth_remote(
                                    &name,
                                    provider,
                                    client_id,
                                    client_secret,
                                    &handle,
                                    &*repo,
                                    &*rclone,
                                )
                            })
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                        },
                        Message::AddRemoteResult,
                    );
                }
            }
        }
        Task::none()
    }

    /// Handle [`Message::AddRemoteResult(Ok)`] — close the dialog,
    /// clear the auth flag, make sure the remote is enabled, and reload
    /// the remotes list.
    ///
    /// Re-enabling covers remotes that older versions auto-paused on an
    /// auth failure: signing in again is the user saying "sync this".
    /// The policy is persisted *before* the reload so the reload can't
    /// read the stale `enabled = false` back and re-pause the remote.
    pub(in crate::app) fn handle_add_remote_result_ok(&mut self, id: RemoteId) -> Task<Message> {
        self.add_remote_draft = None;
        self.selected = Some(id);
        self.sync_state.reauth_complete(id);
        self.sync_state.set_remote_enabled(id, true);
        // Sync right away instead of waiting out the interval.
        self.last_sync_at.remove(&id);
        let mut reenable_policy: Option<crate::domain::remote::SyncPolicy> = None;
        if let Some(remote) = self.remotes.iter_mut().find(|r| r.id == id)
            && !remote.policy.enabled
        {
            remote.policy.enabled = true;
            reenable_policy = Some(remote.policy.clone());
        }
        let repo = self.repo.clone();
        Task::perform(
            async move {
                if let Some(policy) = reenable_policy {
                    let _ = repo.set_policy(id, policy).await;
                }
                repo.list_remotes().await.unwrap_or_default()
            },
            Message::RemotesLoaded,
        )
    }

    /// Handle [`Message::AddRemoteResult(Err)`] — surface the message
    /// inside the open draft.
    ///
    /// A human-verification request isn't an error the user can fix in
    /// the form: open the challenge in the browser right away and keep
    /// it on the draft, so the next submit retries with its token.
    pub(in crate::app) fn handle_add_remote_result_err(&mut self, msg: String) -> Task<Message> {
        if msg == crate::services::auth::AUTHORIZE_CANCELLED {
            return Task::none();
        }
        if let Some(draft) = self.add_remote_draft.as_mut() {
            draft.busy = false;
            draft.oauth = None;
            match celeste_go::proton::HumanVerification::from_login_error(&msg) {
                Some(hv) => {
                    open_in_browser(&hv.url());
                    draft.hv = Some(hv);
                    draft.error = None;
                }
                None => draft.error = Some(msg),
            }
        }
        Task::none()
    }

    /// Handle [`remote_page::Msg::RefreshNow`] — kick a fresh pass
    /// (queue if one is already in flight).
    pub(in crate::app) fn handle_refresh_now(&mut self, id: RemoteId) -> Task<Message> {
        if self.syncing.contains(&id) {
            // The current pass is still running — queue a follow-up
            // so it fires as soon as the current one completes. Leave
            // a pending-event note on each sync_dir so the user gets
            // immediate feedback instead of thinking the click was
            // lost.
            self.refresh_requested_after.insert(id);
            let queued_ids: Vec<crate::domain::sync::SyncDirId> = self
                .sync_dirs
                .get(&id)
                .map(|dirs| dirs.iter().map(|sd| sd.id).collect())
                .unwrap_or_default();
            for sd_id in queued_ids {
                self.push_log_line(
                    sd_id,
                    "⟳ Refresh queued — starts after the current pass finishes."
                        .to_owned(),
                );
            }
            Task::none()
        } else {
            self.start_sync(id)
        }
    }

    /// Handle [`remote_page::Msg::RequestDeleteRemote`] — open the
    /// OK/Cancel dialog. The actual delete is deferred until the user
    /// presses OK ([`Message::Remote(remote_page::Msg::ConfirmDelete)`]).
    pub(in crate::app) fn handle_request_delete_remote(
        &mut self,
        id: RemoteId,
        name: String,
    ) -> Task<Message> {
        self.pending_delete = Some(remote_page::PendingDelete::Remote(id, name));
        Task::none()
    }

    /// Handle [`remote_page::Msg::RequestDeleteSyncDir`] — open the
    /// OK/Cancel dialog. The actual delete is deferred until the user
    /// presses OK ([`Message::Remote(remote_page::Msg::ConfirmDelete)`]).
    pub(in crate::app) fn handle_request_delete_sync_dir(
        &mut self,
        local: String,
        remote: String,
        remote_label: String,
    ) -> Task<Message> {
        self.pending_delete =
            Some(remote_page::PendingDelete::SyncDir { local, remote, remote_label });
        Task::none()
    }

    /// Handle [`remote_page::Msg::ConfirmDelete`] — execute the
    /// pending delete (if any) and clear the dialog state.
    pub(in crate::app) fn handle_confirm_delete(&mut self) -> Task<Message> {
        match self.pending_delete.take() {
            Some(remote_page::PendingDelete::Remote(id, name)) => {
                self.handle_delete_remote(id, name)
            }
            Some(remote_page::PendingDelete::SyncDir { local, remote, .. }) => {
                self.handle_delete_sync_dir(local, remote)
            }
            None => Task::none(),
        }
    }

    /// Handle [`remote_page::Msg::CancelDelete`] — drop the pending
    /// delete and close the dialog without acting.
    pub(in crate::app) fn handle_cancel_delete(&mut self) -> Task<Message> {
        self.pending_delete = None;
        Task::none()
    }

    /// Handle [`remote_page::Msg::DeleteRemote`] — drop in-memory
    /// state, unregister from the router, kick off the DB cascade and
    /// rclone-side delete, then reload the list.
    pub(in crate::app) fn handle_delete_remote(
        &mut self,
        id: RemoteId,
        name: String,
    ) -> Task<Message> {
        self.selected = None;
        self.syncing.remove(&id);
        self.sync_dirs.remove(&id);
        self.last_sync_at.remove(&id);
        self.sync_dir_drafts.remove(&id);
        self.sync_state.remove_remote(id);
        self.remotes.retain(|r| r.id != id);
        // Drop any native-proton override so the router stops routing
        // its (now-gone) name to a stale session.
        self.rclone.unregister(&name);
        let repo_blocking = self.repo.clone();
        let repo_after = self.repo.clone();
        let rclone = self.rclone.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let _ = crate::services::remote_lifecycle::delete_remote(
                        &name,
                        &*repo_blocking,
                        &*rclone,
                    );
                })
                .await
                .ok();
                repo_after.list_remotes().await.unwrap_or_default()
            },
            Message::RemotesLoaded,
        )
    }

    /// Handle [`remote_page::Msg::Reauthenticate`] — open the Add
    /// Remote dialog pre-filled for reauth (name + provider locked,
    /// the user enters fresh credentials).
    pub(in crate::app) fn handle_reauthenticate(
        &mut self,
        id: RemoteId,
        name: String,
    ) -> Task<Message> {
        let provider = self
            .remotes
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.provider_kind)
            .and_then(map_domain_provider_to_add_remote);
        let mut draft = add_remote::Draft::default();
        draft.name = name;
        draft.provider = provider;
        draft.reauth = true;
        self.add_remote_draft = Some(draft);
        Task::none()
    }
}

/// Hand a URL to the desktop's default browser.
pub(in crate::app) fn open_in_browser(url: &str) {
    if let Err(err) = std::process::Command::new("xdg-open").arg(url).spawn() {
        eprintln!("celeste: couldn't open {url} ({err}).");
    }
}
