//! The expanded folder logs. The lines themselves live in the engine.

use iced::widget::text_editor::Content;

use crate::domain::sync::SyncDirId;

use super::CelesteApp;

impl CelesteApp {
    /// Materialise a log newest-first, so the freshest entry sits on the editor's top line — `text_editor` can't be pinned to the bottom.
    pub(in crate::app) fn build_log_content(&self, sync_dir_id: SyncDirId) -> Content {
        let joined = self
            .engine
            .logs()
            .lock()
            .unwrap()
            .get(&sync_dir_id)
            .map(|lines| lines.iter().rev().map(|l| format!("{}  {}", l.at, l.text)).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        Content::with_text(&joined)
    }
}
