//! The sync engine: schedules and runs the sync passes, tracks their state and keeps the folder logs. It runs on its own thread with its own small tokio runtime, independent of the GUI, so syncing goes on without a window (tray only on the desktop, a background service on Android).
//!
//! The GUI is a client: it sends [`Command`]s and watches [`Snapshot`]s. The engine itself only reads the database; the GUI writes it and then says [`Command::Reload`].

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

use tokio::sync::{mpsc, watch};

use crate::{
    domain::{
        events::SyncEvent,
        ports::{BackendClient, Repository},
        remote::RemoteId,
        run_state::AppState,
        sync::{Conflict, Resolution, SyncDirId},
    },
    infrastructure::stderr_capture::CaptureHandle,
};

mod core;
mod pass;
#[cfg(test)]
mod tests;

use self::core::Core;
pub use self::pass::PassVerdict;

/// Maximum log lines retained per folder before the oldest are dropped to keep memory bounded across long-running sessions.
pub const MAX_LOG_LINES: usize = 200;

/// Starts of the log lines for new conflicts (first sync / changed since the last one).
pub const CONFLICT_LINE_PREFIXES: [&str; 2] = ["⚠ Different on both sides", "⚠ Changed on both sides"];

/// What the GUI asks the engine to do.
#[derive(Debug, Clone)]
pub enum Command {
    /// Remotes, their settings or their folders changed in the database.
    Reload,
    /// Sync this remote now; queued if a pass is running.
    SyncNow(RemoteId),
    /// Sync every enabled, idle remote now.
    SyncAll,
    /// Sync this remote at the next opportunity without queueing behind a running pass (a folder was added).
    SyncSoon(RemoteId),
    /// The remote was added or signed in again (and its setting enabled in the database): clear the sign-in problem and sync it.
    SignedIn(RemoteId),
    /// The user's choice for a conflict, handed to the remote's next pass, which starts right away.
    Resolve { remote_id: RemoteId, sync_dir_id: SyncDirId, resolution: Resolution },
    /// Add a line to a folder's log.
    Log(SyncDirId, String),
}

/// Everything the engine feeds into its loop.
#[derive(Debug)]
pub(crate) enum Input {
    Command(Command),
    Event(SyncEvent),
    PassFinished(RemoteId, PassVerdict),
}

/// The engine's state as the GUI sees it. Published after every change.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub state: AppState,
    /// Remotes with a pass running.
    pub syncing: HashSet<RemoteId>,
    /// When each remote's last pass ended.
    pub last_sync_at: HashMap<RemoteId, Instant>,
    /// Files per folder that changed on both sides and wait for the user.
    pub conflicts: HashMap<SyncDirId, Vec<Conflict>>,
    /// Newest log line of the pass running (or last run) per folder.
    pub pass_lines: HashMap<SyncDirId, String>,
    /// Newest problem (⚠) line per folder, apart from conflicts, which have their own row.
    pub problems: HashMap<SyncDirId, String>,
    /// Bumped with every log line; the folder logs themselves are in [`Handle::logs`].
    pub log_version: u64,
    /// Bumped when the conflict lists change.
    pub conflicts_version: u64,
    /// Bumped when a pass ends.
    pub passes_finished: u64,
}

/// One log entry: local `HH:MM:SS` plus the message.
#[derive(Debug, Clone)]
pub struct LogLine {
    pub at: String,
    pub text: String,
}

/// The folder logs, shared with the GUI, which only reads them while a log is expanded.
pub type Logs = Arc<Mutex<HashMap<SyncDirId, VecDeque<LogLine>>>>;

/// The GUI's connection to the engine. Cheap to clone.
#[derive(Clone)]
pub struct Handle {
    tx: mpsc::UnboundedSender<Input>,
    snapshot: watch::Receiver<Arc<Snapshot>>,
    logs: Logs,
}

impl Handle {
    pub fn send(&self, command: Command) {
        let _ = self.tx.send(Input::Command(command));
    }

    /// A receiver that sees every new snapshot.
    pub fn watch(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.snapshot.clone()
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    pub fn logs(&self) -> &Logs {
        &self.logs
    }
}

static GLOBAL: OnceLock<Handle> = OnceLock::new();

/// Start the process-wide engine, or return the running one.
pub fn start(repo: Arc<dyn Repository>, backend: Arc<dyn BackendClient>, stderr: CaptureHandle) -> &'static Handle {
    GLOBAL.get_or_init(|| spawn(repo, backend, stderr))
}

/// The engine started by [`start`], if any.
pub fn get() -> Option<&'static Handle> {
    GLOBAL.get()
}

fn spawn(repo: Arc<dyn Repository>, backend: Arc<dyn BackendClient>, stderr: CaptureHandle) -> Handle {
    let (tx, rx) = mpsc::unbounded_channel();
    let (snapshot_tx, snapshot) = watch::channel(Arc::new(Snapshot::default()));
    let logs = Logs::default();
    let core = Core::new(repo, backend, stderr, logs.clone(), tx.clone());
    std::thread::Builder::new()
        .name("celeste-engine".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("failed to start the engine's runtime");
            runtime.block_on(run(core, rx, snapshot_tx));
        })
        .expect("failed to start the engine thread");
    Handle { tx, snapshot, logs }
}

/// The engine loop: sleeps until an input arrives or a remote is due, handles everything that is ready, starts due passes and publishes one snapshot per round.
async fn run(mut core: Core, mut rx: mpsc::UnboundedReceiver<Input>, snapshot_tx: watch::Sender<Arc<Snapshot>>) {
    core.reload().await;
    let mut busy = false;
    loop {
        core.start_due(Instant::now());
        let snapshot = Arc::new(core.snapshot());
        if busy != !snapshot.syncing.is_empty() {
            busy = !busy;
            busy_changed(busy);
        }
        snapshot_tx.send_replace(snapshot);
        if let Some(status) = core.take_status_change() {
            status_changed(&status);
        }
        let input = match core.next_due() {
            Some(at) => tokio::select! {
                input = rx.recv() => input,
                () = tokio::time::sleep_until(at.into()) => continue,
            },
            None => rx.recv().await,
        };
        let Some(input) = input else { return };
        core.handle(input).await;
        while let Ok(input) = rx.try_recv() {
            core.handle(input).await;
        }
    }
}

/// Shows the summary that changed where the platform wants it: the desktop tray reads the snapshots itself, Android shows it in the sync service's notification.
#[cfg(target_os = "android")]
fn status_changed(status: &crate::infrastructure::tray::TrayStatus) {
    crate::infrastructure::android::show_sync_status(&crate::infrastructure::tray::description_for(status));
}

#[cfg(not(target_os = "android"))]
fn status_changed(_status: &crate::infrastructure::tray::TrayStatus) {}

/// A pass started while none ran (`true`), or the last one ended. On Android the device stays awake in between, so a pass that started finishes even when the screen goes off.
#[cfg(target_os = "android")]
fn busy_changed(busy: bool) {
    crate::infrastructure::android::keep_awake(busy);
}

#[cfg(not(target_os = "android"))]
fn busy_changed(_busy: bool) {}

/// True when an error message indicates an auth failure across any backend. Delegates to the per-backend translators so the classification lives exactly once.
pub(crate) fn is_auth_failure(msg: &str) -> bool {
    use crate::domain::backend_events::EventTranslator;
    use crate::infrastructure::translators::{proton::ProtonTranslator, rclone::RcloneTranslator};
    RcloneTranslator.is_auth_failure(msg) || ProtonTranslator.is_auth_failure(msg)
}

/// Local wall-clock time as `HH:MM:SS` for log prefixes. Uses libc's `localtime_r` because the `time` crate refuses local offsets in multi-threaded processes.
fn local_clock() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: `localtime_r` only writes into the zeroed `tm` we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
