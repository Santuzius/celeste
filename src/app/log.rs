//! Per-sync_dir log buffer.

use iced::widget::text_editor::Content;

use crate::{domain::sync::SyncDirId, screens::remote_page};

use super::CelesteApp;

/// One log entry: local `HH:MM:SS` plus the message.
pub(in crate::app) struct LogLine {
    pub at: String,
    pub text: String,
}

impl CelesteApp {
    /// Append a line to the per-sync_dir log and drop the oldest entries once the buffer exceeds [`remote_page::MAX_LOG_LINES`]. The shaped editor content is only rebuilt while that log is expanded — collapsed logs cost nothing but the strings.
    pub(in crate::app) fn push_log_line(&mut self, sync_dir_id: SyncDirId, line: String) {
        let lines = self.sync_dir_log_lines.entry(sync_dir_id).or_default();
        lines.push_back(LogLine { at: local_clock(), text: line });
        while lines.len() > remote_page::MAX_LOG_LINES {
            lines.pop_front();
        }
        if self.sync_dir_log_content.contains_key(&sync_dir_id) {
            let content = self.build_log_content(sync_dir_id);
            self.sync_dir_log_content.insert(sync_dir_id, content);
        }
    }

    /// Materialise a log newest-first, so the freshest entry sits on the editor's top line — `text_editor` can't be pinned to the bottom.
    pub(in crate::app) fn build_log_content(&self, sync_dir_id: SyncDirId) -> Content {
        let joined = self
            .sync_dir_log_lines
            .get(&sync_dir_id)
            .map(|lines| {
                lines
                    .iter()
                    .rev()
                    .map(|l| format!("{}  {}", l.at, l.text))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Content::with_text(&joined)
    }
}

/// Local wall-clock time as `HH:MM:SS` for log prefixes. Uses libc's `localtime_r` because the `time` crate refuses local offsets in multi-threaded processes.
fn local_clock() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: `localtime_r` only writes into the zeroed `tm` we own.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
