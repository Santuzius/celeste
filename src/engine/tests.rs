//! Engine tests: scheduling, queueing, cancelling, sign-in problems, conflicts and the folder logs, with real passes against the fake repository and backend.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use time::OffsetDateTime;
use tokio::sync::mpsc;

use super::{core::Core, Command, Input, Logs, PassVerdict, Snapshot};
use crate::{
    domain::{
        events::SyncEvent,
        remote::RemoteId,
        run_state::RunState,
        sync::{Conflict, ConflictChoice, FileDetails, Resolution, SyncDirId, SyncError},
    },
    infrastructure::stderr_capture,
    test_support::{remote, sync_dir, FakeBackend, FakeRepo, TempDir},
};

const R1: RemoteId = RemoteId(1);
const R2: RemoteId = RemoteId(2);
const D1: SyncDirId = SyncDirId(1);
const D2: SyncDirId = SyncDirId(2);

struct Rig {
    core: Core,
    rx: mpsc::UnboundedReceiver<Input>,
    repo: Arc<FakeRepo>,
    logs: Logs,
    _dirs: Vec<TempDir>,
}

/// Two enabled remotes with one folder each, both in fresh temp dirs.
async fn rig() -> Rig {
    let repo = Arc::new(FakeRepo::new());
    let dirs = vec![TempDir::new("engine-1"), TempDir::new("engine-2")];
    *repo.remotes.lock().unwrap() = vec![remote(1, "One"), remote(2, "Two")];
    *repo.sync_dirs.lock().unwrap() = vec![sync_dir(1, 1, dirs[0].as_str(), "a"), sync_dir(2, 2, dirs[1].as_str(), "b")];
    let (tx, rx) = mpsc::unbounded_channel();
    let logs = Logs::default();
    let mut core = Core::new(repo.clone(), Arc::new(FakeBackend::default()), stderr_capture::handle(), logs.clone(), tx);
    core.reload().await;
    Rig { core, rx, repo, logs, _dirs: dirs }
}

impl Rig {
    async fn send(&mut self, command: Command) {
        self.core.handle(Input::Command(command)).await;
    }

    async fn event(&mut self, event: SyncEvent) {
        self.core.handle(Input::Event(event)).await;
    }

    /// Feed the passes' events and ends back in until no pass is running.
    async fn settle(&mut self) {
        while !self.snap().syncing.is_empty() {
            let input = tokio::time::timeout(Duration::from_secs(10), self.rx.recv()).await.expect("pass hangs").expect("channel closed");
            self.core.handle(input).await;
        }
    }

    fn snap(&self) -> Snapshot {
        self.core.snapshot()
    }

    fn log(&self, dir: SyncDirId) -> Vec<String> {
        self.logs.lock().unwrap().get(&dir).map(|lines| lines.iter().map(|l| l.text.clone()).collect()).unwrap_or_default()
    }

    fn set_enabled(&self, id: RemoteId, enabled: bool) {
        self.repo.remotes.lock().unwrap().iter_mut().find(|r| r.id == id).unwrap().policy.enabled = enabled;
    }
}

fn conflict(path: &str) -> Conflict {
    let details = FileDetails { size: Some(1), mod_time: OffsetDateTime::UNIX_EPOCH, sha1: None };
    Conflict { local_path: format!("/tmp/{path}"), remote_path: path.to_owned(), local: details.clone(), remote: details, local_stamp: 1, remote_stamp: 2, first_sync: false }
}

fn resolution(path: &str) -> Resolution {
    Resolution { remote_path: path.to_owned(), choice: ConflictChoice::KeepLocal, local_stamp: 1, remote_stamp: 2 }
}

fn conflicts_event(conflicts: Vec<Conflict>, resolutions: Vec<Resolution>) -> SyncEvent {
    SyncEvent::SyncDirConflicts { remote_id: R1, sync_dir_id: D1, conflicts, resolutions }
}

#[tokio::test]
async fn due_remotes_sync_once_and_wait_for_their_interval() {
    let mut rig = rig().await;
    rig.set_enabled(R2, false);
    rig.send(Command::Reload).await;
    rig.core.start_due(Instant::now());
    assert_eq!(rig.snap().syncing, [R1].into());
    rig.settle().await;
    let snap = rig.snap();
    assert_eq!(snap.passes_finished, 1);
    assert!(snap.last_sync_at.contains_key(&R1) && !snap.last_sync_at.contains_key(&R2));
    assert_eq!(snap.state.roll_up(R1), Some(RunState::Synced));
    assert_eq!(snap.state.roll_up(R2), Some(RunState::Paused));
    // Nothing is due before the interval (15 s by default) is up.
    rig.core.start_due(Instant::now());
    assert!(rig.snap().syncing.is_empty());
    let due = rig.core.next_due().expect("R1 is scheduled");
    assert!(due > Instant::now() + Duration::from_secs(14));
}

#[tokio::test]
async fn sync_now_while_syncing_queues_one_more_pass() {
    let mut rig = rig().await;
    rig.send(Command::SyncNow(R1)).await;
    rig.send(Command::SyncNow(R1)).await;
    assert!(rig.log(D1).iter().any(|l| l.starts_with("⟳ Refresh queued")));
    rig.settle().await;
    assert_eq!(rig.snap().passes_finished, 2);
}

#[tokio::test]
async fn disabling_cancels_and_unschedules() {
    let mut rig = rig().await;
    rig.send(Command::SyncNow(R1)).await;
    rig.send(Command::SyncNow(R1)).await;
    rig.set_enabled(R1, false);
    rig.set_enabled(R2, false);
    rig.send(Command::Reload).await;
    rig.settle().await;
    // The queued pass was dropped with the disable.
    assert_eq!(rig.snap().passes_finished, 1);
    assert_eq!(rig.core.next_due(), None);
    rig.send(Command::SyncAll).await;
    assert!(rig.snap().syncing.is_empty());
}

#[tokio::test]
async fn auth_failure_holds_the_remote_until_signed_in() {
    let mut rig = rig().await;
    rig.set_enabled(R2, false);
    rig.send(Command::Reload).await;
    let msg = "couldn't fetch token: invalid_grant: maybe token expired? - try refreshing with \"rclone config reconnect gdrive:\"";
    rig.event(SyncEvent::SyncDirError { remote_id: R1, sync_dir_id: D1, error: SyncError::General("a".into(), msg.into()) }).await;
    assert!(rig.snap().state.needs_reauth(R1));
    assert_eq!(rig.snap().problems.get(&D1).map(String::as_str), Some(format!("⚠ a: {msg}").as_str()));
    assert_eq!(rig.core.next_due(), None);
    rig.send(Command::SyncNow(R1)).await;
    assert!(rig.snap().syncing.is_empty());
    rig.send(Command::SignedIn(R1)).await;
    assert!(!rig.snap().state.needs_reauth(R1));
    rig.core.start_due(Instant::now());
    assert_eq!(rig.snap().syncing, [R1].into());
    rig.settle().await;
}

#[tokio::test]
async fn degraded_passes_skip_turns() {
    let mut rig = rig().await;
    rig.set_enabled(R2, false);
    rig.send(Command::Reload).await;
    rig.core.handle(Input::PassFinished(R1, PassVerdict::Degraded)).await;
    assert!(rig.snap().state.in_backoff(R1));
    let later = Instant::now() + Duration::from_secs(16);
    rig.core.start_due(later);
    assert!(rig.snap().syncing.is_empty(), "the first due turn is skipped");
    assert_eq!(rig.snap().last_sync_at.get(&R1), Some(&later));
    assert!(!rig.snap().state.in_backoff(R1));
}

#[tokio::test]
async fn conflicts_are_logged_once_and_hidden_once_decided() {
    let mut rig = rig().await;
    rig.event(conflicts_event(vec![conflict("x"), conflict("y")], vec![])).await;
    rig.event(conflicts_event(vec![conflict("x"), conflict("y")], vec![])).await;
    let logged = rig.log(D1).iter().filter(|l| l.starts_with("⚠ Changed on both sides")).count();
    assert_eq!(logged, 2, "a conflict is logged when it shows up, not on every pass");
    assert!(rig.snap().problems.is_empty(), "conflicts have their own row");
    assert_eq!(rig.snap().state.dir_state(R1, D1), RunState::Warning);

    rig.send(Command::Resolve { remote_id: R1, sync_dir_id: D1, resolution: resolution("x") }).await;
    assert_eq!(rig.snap().conflicts[&D1].len(), 1);
    assert!(rig.snap().syncing.contains(&R1), "a choice starts a pass");
    // A pass that wasn't given the choice still reports x; it stays hidden.
    rig.event(conflicts_event(vec![conflict("x"), conflict("y")], vec![])).await;
    assert_eq!(rig.snap().conflicts[&D1].iter().map(|c| c.remote_path.as_str()).collect::<Vec<_>>(), ["y"]);

    rig.send(Command::Resolve { remote_id: R1, sync_dir_id: D1, resolution: resolution("y") }).await;
    assert!(!rig.snap().conflicts.contains_key(&D1));
    assert_eq!(rig.snap().state.dir_state(R1, D1), RunState::Waiting);
    rig.settle().await;
    // The pass that got both choices reports back; x changed again in between and shows up anew.
    rig.event(conflicts_event(vec![conflict("x")], vec![resolution("x"), resolution("y")])).await;
    assert_eq!(rig.snap().conflicts[&D1].len(), 1);
}

#[tokio::test]
async fn removed_folders_and_remotes_are_forgotten() {
    let mut rig = rig().await;
    rig.send(Command::Log(D1, "⚠ something".into())).await;
    rig.send(Command::Log(D2, "hello".into())).await;
    rig.event(conflicts_event(vec![conflict("x")], vec![])).await;
    rig.repo.sync_dirs.lock().unwrap().retain(|d| d.id != D1);
    rig.repo.remotes.lock().unwrap().retain(|r| r.id != R2);
    rig.send(Command::Reload).await;
    let snap = rig.snap();
    assert!(rig.log(D1).is_empty() && !snap.problems.contains_key(&D1) && !snap.conflicts.contains_key(&D1));
    assert_eq!(snap.state.roll_up(R2), None);
    assert_eq!(snap.pass_lines.get(&D2).map(String::as_str), Some("hello"));
}

#[tokio::test]
async fn logs_keep_the_newest_lines() {
    let mut rig = rig().await;
    for i in 0..super::MAX_LOG_LINES + 5 {
        rig.send(Command::Log(D1, format!("line {i}"))).await;
    }
    let log = rig.log(D1);
    assert_eq!(log.len(), super::MAX_LOG_LINES);
    assert_eq!(log[0], "line 5");
    assert_eq!(rig.snap().pass_lines[&D1], format!("line {}", super::MAX_LOG_LINES + 4));
}
