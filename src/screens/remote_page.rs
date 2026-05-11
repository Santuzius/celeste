//! Per-remote detail page: sync-dir cards with log view and exclusion panel.

use std::{collections::HashMap, time::Duration};

use iced::{
    widget::{
        button, center, column, container, opaque, row, rule, scrollable, stack,
        text_editor, text_input, Space,
    },
    Alignment, Background, Color, Element, Length,
};

use crate::{
    domain::{
        remote::{Remote, RemoteId},
        run_state::RunState,
        sync::{SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId},
    },
    screens::settings,
    theme::{PAGE_PADDING, ROW_SPACING, SECTION_SPACING},
    widgets::{run_state_icon::status_icon, text},
};

/// Fixed height of the per-sync-dir log editor.
const LOG_HEIGHT: f32 = 110.0;
/// Maximum log lines retained per sync_dir before the oldest are dropped
/// to keep memory bounded across long-running sessions.
pub const MAX_LOG_LINES: usize = 200;
/// Side length of the per-card status icon.
const STATUS_ICON_SIZE: f32 = 24.0;
/// Font size for the sync_dir path label so it reads larger than the
/// surrounding chrome (badges, log lines).
const SYNC_DIR_FONT_SIZE: f32 = 16.0;
/// Right-side breathing room reserved inside the cards scrollable so
/// the trailing buttons aren't sat on by the outer scrollbar.
const CARDS_RIGHT_GUTTER: u16 = 18;

#[derive(Debug, Clone)]
pub enum Msg {
    RefreshNow(RemoteId),
    Back,
    Settings(settings::Msg),
    DraftLocalPathChanged(String),
    DraftRemotePathChanged(String),
    AddSyncDir,
    /// User clicked the Delete button on a sync_dir card. Opens the
    /// confirmation dialog; the actual delete fires on
    /// [`Msg::ConfirmDelete`].
    RequestDeleteSyncDir(String, String),
    DeleteSyncDir(String, String),
    /// User clicked the Delete remote button in the header. Opens the
    /// confirmation dialog; the actual delete fires on
    /// [`Msg::ConfirmDelete`].
    RequestDeleteRemote(RemoteId, String),
    DeleteRemote(RemoteId, String),
    /// Dialog OK pressed — execute the pending delete.
    ConfirmDelete,
    /// Dialog Cancel pressed — drop the pending delete.
    CancelDelete,
    ToggleExclusions(SyncDirId),
    DraftExclusionChanged(SyncDirId, String),
    AddExclusion(SyncDirId),
    RemoveExclusion(SyncDirExclusionId, SyncDirId),
    Reauthenticate(RemoteId, String),
    /// Read-only log editor swallows edits but forwards scroll/select
    /// actions so users can drag through history.
    LogEditorAction(SyncDirId, text_editor::Action),
}

/// What the user is about to delete, pending OK/Cancel on the
/// confirmation dialog. Stored at the app level and threaded into
/// [`view`] so the dialog can render the right description and dispatch
/// the matching delete on confirm.
#[derive(Debug, Clone)]
pub enum PendingDelete {
    Remote(RemoteId, String),
    SyncDir { local: String, remote: String },
}

pub fn view<'a>(
    remote: &'a Remote,
    sync_dirs: &'a [SyncDir],
    log: &'a HashMap<SyncDirId, text_editor::Content>,
    status: HashMap<SyncDirId, RunState>,
    all_known_sync_dirs: &'a [SyncDir],
    exclusion_panel: Option<SyncDirId>,
    exclusions: &'a HashMap<SyncDirId, Vec<SyncDirExclusion>>,
    draft_exclusion: &'a HashMap<SyncDirId, String>,
    draft: (&'a str, &'a str),
    next_sync_eta: Option<(Duration, bool)>,
    needs_reauth: bool,
    pending_delete: Option<&'a PendingDelete>,
) -> Element<'a, Msg> {
    let countdown: Element<'a, Msg> = match next_sync_eta {
        Some((remaining, in_backoff)) => {
            let label = format!("next sync in {}", format_duration(remaining));
            let countdown_text = text(label).size(12);
            if in_backoff {
                use iced::widget::tooltip;
                tooltip(
                    row![
                        countdown_text,
                        Space::new().width(Length::Fixed(4.0)),
                        text("⚠").size(14),
                    ]
                    .align_y(Alignment::Center),
                    text(
                        "Backoff active — the provider returned rate-limit \
                         warnings on the last pass, so Celeste will skip \
                         the next cycles before trying again. The more \
                         consecutive degraded passes, the more cycles are \
                         skipped. Resets on the next clean pass.",
                    )
                    .size(12),
                    tooltip::Position::Bottom,
                )
                .gap(8)
                .padding(8)
                .into()
            } else {
                countdown_text.into()
            }
        }
        None => text("paused").size(12).into(),
    };

    let header = row![
        button(text("←")).on_press(Msg::Back),
        text(&remote.name).size(22),
        Space::new().width(Length::Fixed(12.0)),
        countdown,
        Space::new().width(Length::Fill),
        button(text("Refresh now")).on_press(Msg::RefreshNow(remote.id)),
        button(text("Reauthenticate"))
            .on_press(Msg::Reauthenticate(remote.id, remote.name.clone())),
        button(text("Delete remote"))
            .on_press(Msg::RequestDeleteRemote(remote.id, remote.name.clone())),
    ]
    .spacing(ROW_SPACING)
    .align_y(Alignment::Center);

    // ── Reauth banner (native-proton session missing / expired) ───────────
    let reauth_banner: Option<Element<'a, Msg>> = if needs_reauth {
        let msg = "This remote's session isn't loaded. Sync is paused \
                   until you reauthenticate. Your sync directories, \
                   exclusions, and schedule will be preserved.";
        // Wrap the message in a Fill-width container so a long blurb
        // wraps inside the available slack instead of pushing the
        // [Reauthenticate] button off the right edge.
        Some(
            container(
                row![
                    container(text(format!("⚠ {msg}")).size(13))
                        .width(Length::Fill),
                    button(text("Reauthenticate"))
                        .on_press(Msg::Reauthenticate(remote.id, remote.name.clone())),
                ]
                .align_y(Alignment::Center)
                .spacing(ROW_SPACING),
            )
            .padding(8)
            .width(Length::Fill)
            .style(container::bordered_box)
            .into(),
        )
    } else {
        None
    };

    // ── Sync directory cards ────────────────────────────────────────────────
    let mut cards_col = column![text("Sync directories").size(16)].spacing(ROW_SPACING);

    if sync_dirs.is_empty() {
        cards_col = cards_col.push(text("No sync directories yet.").size(13));
    }

    for sd in sync_dirs {
        let remote_display = if sd.remote_path.is_empty() {
            "/"
        } else {
            &sd.remote_path
        };
        let path_label = format!("\"{}\" → \"{}\"", sd.local_path, remote_display);

        // Count auto-excluded descendants + user exclusions for the badge.
        let auto_excl = auto_excluded_for(sd, all_known_sync_dirs);
        let custom_excl_count = exclusions.get(&sd.id).map(|v| v.len()).unwrap_or(0);
        let total_excl = auto_excl.len() + custom_excl_count;
        let excl_label = format!("Excluded ({})", total_excl);

        // Reauth needed: show AuthNeeded icon on every card so the
        // user can see at a glance that no dir can progress.
        let icon_state = if needs_reauth {
            Some(RunState::AuthNeeded)
        } else {
            status.get(&sd.id).copied()
        };

        // Top row: [icon] paths | [Excluded (n)] [Delete].
        // Wrap the path label in a Fill-width container so a long
        // `"local" → "remote"` string consumes the row's slack instead
        // of pushing the trailing buttons off the right edge. iced
        // wraps the text within the container's bounds.
        let top_row = row![
            status_icon(icon_state, STATUS_ICON_SIZE),
            container(text(path_label).size(SYNC_DIR_FONT_SIZE))
                .width(Length::Fill),
            button(text(excl_label).size(12)).on_press(Msg::ToggleExclusions(sd.id)),
            button(text("Delete").size(12)).on_press(Msg::RequestDeleteSyncDir(
                sd.local_path.clone(),
                sd.remote_path.clone(),
            )),
        ]
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center);

        // Log area: read-only multi-line editor stretched to the card
        // width with a small horizontal inset so its frame doesn't merge
        // with the card edge. Lines are capped at MAX_LOG_LINES upstream
        // to keep RAM bounded. App.rs guarantees a Content entry per
        // sync_dir on load so the editor is always visible — even for a
        // sync_dir that hasn't emitted a single event yet.
        let sd_id_for_editor = sd.id;
        let log_area: Element<'a, Msg> = if let Some(content) = log.get(&sd.id) {
            container(
                text_editor(content)
                    .height(Length::Fixed(LOG_HEIGHT))
                    .padding(4)
                    .on_action(move |action| {
                        Msg::LogEditorAction(sd_id_for_editor, action)
                    }),
            )
            .padding([0, 6])
            .width(Length::Fill)
            .into()
        } else {
            // Defensive fallback: a same-sized blank so card layout
            // stays steady if a sync_dir somehow lacks a Content entry.
            container(Space::new().height(Length::Fixed(LOG_HEIGHT)))
                .padding([0, 6])
                .width(Length::Fill)
                .into()
        };

        let mut card_col = column![top_row, log_area].spacing(ROW_SPACING / 2.0);

        // ── Exclusion panel (shown when toggled) ────────────────────────────
        if exclusion_panel == Some(sd.id) {
            card_col = card_col.push(rule::horizontal(1));
            card_col = card_col.push(exclusion_panel_view(
                sd,
                &auto_excl,
                exclusions.get(&sd.id).map(|v| v.as_slice()).unwrap_or(&[]),
                draft_exclusion.get(&sd.id).map(|s| s.as_str()).unwrap_or(""),
            ));
        }

        cards_col = cards_col.push(
            container(card_col)
                .padding(8)
                .width(Length::Fill)
                .style(container::bordered_box),
        );
    }

    // Inline "Add sync dir" form.
    let (draft_local, draft_remote) = draft;
    let add_form = row![
        text_input("Local path…", draft_local)
            .on_input(Msg::DraftLocalPathChanged)
            .padding(6)
            .size(13),
        text_input("Remote path…", draft_remote)
            .on_input(Msg::DraftRemotePathChanged)
            .padding(6)
            .size(13),
        button(text("Add")).on_press(Msg::AddSyncDir),
    ]
    .spacing(ROW_SPACING);
    cards_col = cards_col.push(add_form);

    // Inset the cards by the scrollbar's footprint so the trailing
    // [Excluded] / [Delete] buttons aren't covered by the outer
    // scrollbar drawn over the right edge.
    let sync_dirs_section = scrollable(
        container(cards_col)
            .padding(iced::Padding::default().right(f32::from(CARDS_RIGHT_GUTTER)))
            .width(Length::Fill),
    )
    .height(Length::FillPortion(2));

    let settings_panel = settings::view(remote).map(Msg::Settings);

    let mut page = column![header, rule::horizontal(1)].spacing(SECTION_SPACING);
    if let Some(banner) = reauth_banner {
        page = page.push(banner);
    }
    page = page
        .push(sync_dirs_section)
        .push(rule::horizontal(1))
        .push(settings_panel);

    let base: Element<'a, Msg> = container(page).padding(PAGE_PADDING).into();

    // Overlay the confirmation dialog on top of the page when a delete
    // is pending. The dimmed backdrop is wrapped in `opaque` so clicks
    // outside the dialog don't leak through to the page underneath.
    match pending_delete {
        Some(pending) => stack![base, confirm_delete_overlay(pending)].into(),
        None => base,
    }
}

/// Render the dimmed backdrop + centered OK/Cancel confirmation card
/// for a pending delete.
fn confirm_delete_overlay<'a>(pending: &'a PendingDelete) -> Element<'a, Msg> {
    let (title, body) = match pending {
        PendingDelete::Remote(_id, name) => (
            "Delete remote?",
            format!(
                "Are you sure you want to delete the remote \"{name}\"? \
                 All sync directories for this remote will be removed and \
                 syncing will stop. Local files are preserved."
            ),
        ),
        PendingDelete::SyncDir { local, remote } => {
            let remote_display = if remote.is_empty() { "/" } else { remote.as_str() };
            (
                "Delete sync directory?",
                format!(
                    "Are you sure you want to delete the sync directory \
                     \"{local}\" → \"{remote_display}\"? Local and remote \
                     files are preserved; only the link between them is \
                     removed."
                ),
            )
        }
    };

    let dialog = container(
        column![
            text(title).size(18),
            text(body).size(13),
            row![
                Space::new().width(Length::Fill),
                button(text("Cancel")).on_press(Msg::CancelDelete),
                button(text("OK")).on_press(Msg::ConfirmDelete),
            ]
            .spacing(ROW_SPACING),
        ]
        .spacing(SECTION_SPACING),
    )
    .padding(16)
    .max_width(420.0)
    .style(container::bordered_box);

    let backdrop_style = |_theme: &iced::Theme| container::Style {
        background: Some(Background::Color(Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.5,
        })),
        ..container::Style::default()
    };

    opaque(
        container(center(dialog))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(backdrop_style),
    )
}

/// Build the exclusion panel for one sync_dir card.
fn exclusion_panel_view<'a>(
    sd: &'a SyncDir,
    auto_excl: &[&'a SyncDir],
    custom_excl: &'a [SyncDirExclusion],
    draft: &'a str,
) -> Element<'a, Msg> {
    let mut col = column![].spacing(ROW_SPACING / 2.0);

    // Auto-excluded (descendant sync_dirs) — read-only. Show the
    // descendant's remote path so it lines up with what the provider
    // sees, not the local mirror.
    if !auto_excl.is_empty() {
        col = col.push(text("Auto-excluded:").size(12));
        for desc in auto_excl {
            let remote_relative = if sd.remote_path.is_empty() {
                desc.remote_path.as_str()
            } else {
                desc.remote_path
                    .strip_prefix(&format!("{}/", sd.remote_path))
                    .unwrap_or(&desc.remote_path)
            };
            col = col.push(text(format!("  \"{remote_relative}\"")).size(12));
        }
    }

    // User-defined exclusions — deletable.
    if !custom_excl.is_empty() {
        col = col.push(text("Custom excluded:").size(12));
        for excl in custom_excl {
            let excl_id = excl.id;
            let sd_id = sd.id;
            col = col.push(
                row![
                    text(format!("  \"{}\"", excl.remote_path)).size(12),
                    Space::new().width(Length::Fill),
                    button(text("×").size(11))
                        .on_press(Msg::RemoveExclusion(excl_id, sd_id)),
                ]
                .align_y(iced::Alignment::Center)
                .spacing(ROW_SPACING),
            );
        }
    }

    if auto_excl.is_empty() && custom_excl.is_empty() {
        col = col.push(text("No exclusions.").size(12));
    }

    // Add-exclusion form (remote sub-path input).
    let sd_id = sd.id;
    col = col.push(
        row![
            text_input("Remote sub-path…", draft)
                .on_input(move |s| Msg::DraftExclusionChanged(sd_id, s))
                .padding(4)
                .size(12),
            button(text("Exclude").size(12)).on_press(Msg::AddExclusion(sd_id)),
        ]
        .spacing(ROW_SPACING),
    );

    col.into()
}

/// Sync_dirs from `all` that are auto-excluded under `sd` — i.e. another
/// sync_dir on the same provider whose remote path is nested under
/// `sd.remote_path`. Local-tree overlaps are blocked at AddSyncDir time,
/// so the badge only needs to surface the remote-tree case here.
fn auto_excluded_for<'a>(sd: &SyncDir, all: &'a [SyncDir]) -> Vec<&'a SyncDir> {
    all.iter()
        .filter(|d| d.id != sd.id && d.remote_id == sd.remote_id)
        .filter(|d| is_remote_descendant(sd, d))
        .collect()
}

fn is_remote_descendant(ancestor: &SyncDir, candidate: &SyncDir) -> bool {
    if ancestor.remote_path.is_empty() {
        !candidate.remote_path.is_empty()
    } else {
        candidate
            .remote_path
            .starts_with(&format!("{}/", ancestor.remote_path))
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs == 0 {
        return "0s".to_owned();
    }
    let minutes = secs / 60;
    let seconds = secs % 60;
    let hours = minutes / 60;
    let minutes_rem = minutes % 60;
    if hours > 0 {
        format!("{hours}h{minutes_rem:02}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;
    use std::time::Duration;

    #[test]
    fn format_duration_shapes() {
        assert_eq!(format_duration(Duration::ZERO), "0s");
        assert_eq!(format_duration(Duration::from_secs(9)), "9s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1m05s");
        assert_eq!(format_duration(Duration::from_secs(3_900)), "1h05m");
    }
}
