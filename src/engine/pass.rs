//! One sync pass over all folders of a remote.

use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
    time::Instant,
};

use crate::{
    domain::{
        events::SyncEvent,
        ports::{BackendClient, Cancel, Repository},
        remote::RemoteId,
        sync::{Resolution, SyncDirId},
    },
    infrastructure::{stderr_capture::CaptureHandle, translators::rclone::RATE_LIMIT_MARKERS},
    services::sync::Outcome,
};

/// Aggregate outcome across every sync_dir of one remote's pass. The scheduler uses this to drive linear backoff on provider rate-limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassVerdict {
    /// Every sync_dir finished cleanly.
    Clean,
    /// At least one sync_dir detected rate-limiting (stderr tap fired). Scheduler bumps `consecutive_degraded` and skips more cycles.
    Degraded,
    /// Pass aborted for a non-rate-limit reason (cancel, list error). Backoff counter is left alone.
    Aborted,
}

/// Everything a pass needs, taken from the engine when it starts.
pub(crate) struct Pass {
    pub remote_id: RemoteId,
    pub repo: Arc<dyn Repository>,
    pub backend: Arc<dyn BackendClient>,
    pub stderr: CaptureHandle,
    pub cancel: Cancel,
    /// The user's conflict choices for this pass.
    pub resolutions: HashMap<SyncDirId, Vec<Resolution>>,
    pub emit: Arc<dyn Fn(SyncEvent) + Send + Sync>,
}

impl Pass {
    /// Load the remote and its folders, then sync them one after the other off the async runtime.
    pub async fn run(self) -> PassVerdict {
        let Pass { remote_id, repo, backend, stderr, cancel, resolutions, emit } = self;
        let remote = match repo.find_remote(remote_id).await {
            Ok(Some(r)) => r,
            _ => return PassVerdict::Aborted,
        };
        let sync_dirs = repo.list_sync_dirs(remote_id).await.unwrap_or_default();
        let all_sync_dirs = repo.list_all_sync_dirs().await.unwrap_or_default();
        tokio::task::spawn_blocking(move || {
            // The stderr probe: only rclone-transport backends emit rate-limit warnings to stderr. Native Proton reports throttling via its own error paths. For rclone backends, any line matching the rclone translator's marker set flips the pass to Degraded.
            let use_stderr_probe = remote.provider_kind.is_none_or(|k| k.uses_rclone_transport());
            let rate_limit_seen_since = move |since: Instant| -> bool {
                use_stderr_probe && stderr.any_line_since(since, |line| RATE_LIMIT_MARKERS.iter().any(|m| m.iter().all(|needle| line.contains(needle))))
            };
            let emit = move |event: SyncEvent| emit(event);
            let (mut any_degraded, mut any_error, mut any_synced) = (false, false, false);
            for sd in sync_dirs {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let outcome = crate::services::sync::run(
                    &remote,
                    &sd,
                    &*repo,
                    &*backend,
                    &all_sync_dirs,
                    resolutions.get(&sd.id).map_or(&[], |v| v.as_slice()),
                    emit.clone(),
                    &cancel,
                    rate_limit_seen_since.clone(),
                );
                match outcome {
                    Outcome::Synced => any_synced = true,
                    Outcome::Degraded => any_degraded = true,
                    Outcome::Aborted => any_error = true,
                }
            }
            if any_degraded {
                PassVerdict::Degraded
            } else if any_synced {
                PassVerdict::Clean
            } else if any_error {
                PassVerdict::Aborted
            } else {
                // No sync_dirs to run (or everything cancelled before the first): no reason to accrue backoff.
                PassVerdict::Clean
            }
        })
        .await
        .unwrap_or(PassVerdict::Aborted)
    }
}
