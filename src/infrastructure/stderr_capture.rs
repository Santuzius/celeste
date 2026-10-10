//! Process-wide stderr tap. Installs once at startup, before the Go
//! runtime is initialised, so that every warning rclone (and its backends) print
//! to fd 2 lands in a timestamped ring buffer we can query from the
//! sync code, and which Preferences can copy for a bug report. Only problems
//! are forwarded to the real stderr (the journal when autostarted, logcat on
//! Android) unless the detailed log is on: a line per folder and pass would
//! otherwise add megabytes a day.
//!
//! The motivation is provider-level rate-limiting: rclone's ProtonDrive
//! backend retries 429s internally and eventually returns `Ok(partial)`
//! from `operations/list`. The only signal we have that the call was
//! stressed is the `WARN[...] Too many requests` line emitted by
//! `go-proton-api`. Scraping stderr gives us that signal without
//! waiting for rclone to surface retry counters via its RPC.
//!
//! Unix-only. `dup2` on fd 2 is the trick; spawning a reader thread on
//! the read end of the pipe keeps the buffer warm. The original stderr
//! is `dup`-saved first so lines are forwarded back — tests, other
//! parts of Celeste, and any child process stay visible.

use std::{
    collections::VecDeque,
    io::{BufRead, BufReader},
    os::fd::{FromRawFd, IntoRawFd, OwnedFd},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::Instant,
};

/// Upper bound on the stderr ring — a few thousand lines is plenty for
/// the "did rate limits fire during this sync pass?" query and keeps
/// memory use flat.
const RING_CAPACITY: usize = 4_096;

#[derive(Clone, Debug)]
pub struct CapturedLine {
    pub received_at: Instant,
    /// Wall-clock `HH:MM:SS`, for [`dump`].
    pub at: String,
    pub text: String,
}

/// Forward every line, not only problems (see [`is_problem`]).
static DETAILED: AtomicBool = AtomicBool::new(false);

/// Switch the detailed log on or off.
pub fn set_detailed(on: bool) {
    DETAILED.store(on, Ordering::Relaxed);
}

/// Whether a line is worth the system log without the detailed log: errors, warnings, conflicts, sign-in and rate-limit trouble.
fn is_problem(line: &str) -> bool {
    const MARKERS: [&str; 11] = ["error", "warn", "fail", "could not", "couldn't", "panic", "denied", "expired", "too many requests", "rate limit", "reauth"];
    // Every pass summary counts `conflict=0`; only the conflict lines themselves are shouted.
    let lower = line.to_lowercase();
    line.contains("CONFLICT") || MARKERS.iter().any(|m| lower.contains(m))
}

/// Forget everything captured so far, e.g. before reproducing a problem.
pub fn clear() {
    handle().inner.lock().unwrap().clear();
}

/// Everything captured, oldest first, one `HH:MM:SS text` line each.
pub fn dump() -> String {
    let handle = handle();
    let guard = handle.inner.lock().unwrap();
    guard.iter().map(|l| format!("{} {}\n", l.at, l.text.trim_end())).collect()
}

#[derive(Clone)]
pub struct CaptureHandle {
    inner: Arc<Mutex<VecDeque<CapturedLine>>>,
}

impl CaptureHandle {
    /// Does any captured line arriving at or after `since` match any of
    /// the given substrings? Used by the sync layer to turn "was the
    /// pass stressed?" into a boolean. Cheap enough to call twice per
    /// pass.
    pub fn any_line_since<F>(&self, since: Instant, mut predicate: F) -> bool
    where
        F: FnMut(&str) -> bool,
    {
        let guard = self.inner.lock().unwrap();
        guard
            .iter()
            .rev()
            .take_while(|l| l.received_at >= since)
            .any(|l| predicate(&l.text))
    }
}

static GLOBAL: OnceLock<CaptureHandle> = OnceLock::new();

/// Install the stderr tap. Call exactly once, from `main`, **before**
/// any FFI layer touches fd 2 (in particular before
/// `celeste_go::initialize`). Repeat calls return the previously-installed
/// handle.
pub fn install() -> CaptureHandle {
    if let Some(existing) = GLOBAL.get() {
        return existing.clone();
    }
    let handle = match install_inner() {
        Ok(h) => h,
        Err(err) => {
            // Falling back to an empty buffer keeps the rest of the app
            // working; we just lose rate-limit detection for this run.
            eprintln!("stderr_capture: install failed: {err} — rate-limit detection disabled.");
            CaptureHandle {
                inner: Arc::new(Mutex::new(VecDeque::new())),
            }
        }
    };
    let _ = GLOBAL.set(handle.clone());
    handle
}

/// Retrieve the handle installed by [`install`]. Returns an empty-buffer
/// handle if install was never called (unit tests).
pub fn handle() -> CaptureHandle {
    GLOBAL
        .get()
        .cloned()
        .unwrap_or_else(|| CaptureHandle {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        })
}

fn install_inner() -> Result<CaptureHandle, String> {
    // Duplicate the current stderr so the reader thread can still write
    // through to the user's terminal.
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved < 0 {
        return Err(format!("dup(stderr) failed: {}", std::io::Error::last_os_error()));
    }
    let saved_stderr = unsafe { OwnedFd::from_raw_fd(saved) };

    // Create the pipe whose writer replaces stderr.
    let mut fds = [0i32; 2];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    if rc != 0 {
        return Err(format!("pipe() failed: {}", std::io::Error::last_os_error()));
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };

    // Redirect stderr to the write end.
    let rc = unsafe { libc::dup2(write_fd.into_raw_fd(), libc::STDERR_FILENO) };
    if rc < 0 {
        return Err(format!(
            "dup2(pipe, stderr) failed: {}",
            std::io::Error::last_os_error()
        ));
    }

    let ring = Arc::new(Mutex::new(VecDeque::with_capacity(RING_CAPACITY)));
    let ring_for_thread = ring.clone();

    thread::Builder::new()
        .name("stderr-capture".to_owned())
        .spawn(move || reader_loop(read_fd, saved_stderr, ring_for_thread))
        .map_err(|e| format!("spawn reader: {e}"))?;

    Ok(CaptureHandle { inner: ring })
}

fn reader_loop(
    read_fd: OwnedFd,
    saved_stderr: OwnedFd,
    ring: Arc<Mutex<VecDeque<CapturedLine>>>,
) {
    let file = std::fs::File::from(read_fd);
    let mut reader = BufReader::new(file);
    #[cfg(not(target_os = "android"))]
    use std::io::Write;
    #[cfg(not(target_os = "android"))]
    let mut sink = std::fs::File::from(saved_stderr);
    #[cfg(target_os = "android")]
    drop(saved_stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // writer end closed
            Ok(_) => {
                // Forward first so the real stderr stays live
                // even if the ring lock is briefly held. Android's stderr is /dev/null (or a pipe the GUI set up only later), so lines go to logcat there.
                if DETAILED.load(Ordering::Relaxed) || is_problem(&line) {
                    #[cfg(target_os = "android")]
                    log::info!(target: "stderr", "{}", line.trim_end());
                    #[cfg(not(target_os = "android"))]
                    {
                        let _ = sink.write_all(line.as_bytes());
                        let _ = sink.flush();
                    }
                }

                let received_at = Instant::now();
                let at = crate::util::local_clock();
                let mut guard = ring.lock().unwrap();
                if guard.len() == RING_CAPACITY {
                    guard.pop_front();
                }
                guard.push_back(CapturedLine {
                    received_at,
                    at,
                    text: line.clone(),
                });
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The handle returned when `install` was never called must behave
    /// like an empty ring — no false positives.
    #[test]
    fn uninstalled_handle_has_no_matches() {
        let h = CaptureHandle {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        };
        assert!(!h.any_line_since(Instant::now(), |s| s.contains("429")));
    }

    /// A line inserted before `since` must not count; one inserted
    /// after must.
    #[test]
    fn only_lines_after_the_cutoff_count() {
        let ring = Arc::new(Mutex::new(VecDeque::new()));
        let h = CaptureHandle {
            inner: ring.clone(),
        };

        let before = Instant::now();
        ring.lock().unwrap().push_back(CapturedLine {
            received_at: before,
            at: String::new(),
            text: "status=429 too many requests".to_owned(),
        });
        std::thread::sleep(Duration::from_millis(5));
        let cutoff = Instant::now();
        std::thread::sleep(Duration::from_millis(5));
        ring.lock().unwrap().push_back(CapturedLine {
            received_at: Instant::now(),
            at: String::new(),
            text: "fresh 429 warning".to_owned(),
        });

        assert!(h.any_line_since(cutoff, |s| s.contains("429")));
        assert!(!h.any_line_since(cutoff, |s| s.contains("no-such-token")));
        // Pushing the cutoff back to `before` makes the older line visible.
        assert!(h.any_line_since(before, |s| s.contains("too many requests")));
    }

    #[test]
    fn only_problems_reach_the_system_log_by_default() {
        assert!(!is_problem("sync: plan for remote='GoogleDrive' dir='' — snapshot(db=1, listing=1, walk=1, walk_unreliable=0); actions(upload=0, download=0, delete_local=0, delete_remote=0, conflict=0, clear_db_row=0, record_db_row=0)."));
        assert!(!is_problem("sync: RECORD 'a' — present on both sides but untracked; tracking it from now on."));
        assert!(is_problem("2026/10/10 ERROR : x: Failed to copy"));
        assert!(is_problem("sync: could not record 'a': not found"));
        assert!(is_problem("WARN[0003] Too many requests"));
        assert!(is_problem("sync: CONFLICT 'a' — changed on both sides."));
    }
}
