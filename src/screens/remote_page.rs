//! Per-remote page: status header, sync folders (each with an optional activity log and exclusion panel), the add-folder form and the remote's settings.

use std::time::Duration;

use iced::{
    widget::{
        button, center, column, container, mouse_area, opaque, row, scrollable, stack, text::Wrapping,
        text_editor, text_input, tooltip, Space,
    },
    Alignment, Element, Length,
};

use crate::{
    domain::{
        remote::{Remote, RemoteId},
        run_state::RunState,
        sync::{SyncDir, SyncDirExclusion, SyncDirExclusionId, SyncDirId},
    },
    screens::{main_page::status_label, settings},
    theme::{self, CAPTION, HEADING, PAGE_PADDING, ROW_SPACING, SECTION_SPACING, TEXT, TITLE},
    util::fmt_home,
    widgets::{
        icon::{icon, muted_icon, status_icon},
        text,
    },
};

/// Height of an expanded activity log.
const LOG_HEIGHT: f32 = 170.0;
/// Maximum log lines retained per sync_dir before the oldest are dropped to keep memory bounded across long-running sessions.
pub const MAX_LOG_LINES: usize = 200;
/// Content column stops growing beyond this so rows stay readable on wide windows.
const MAX_CONTENT_WIDTH: f32 = 980.0;
/// Width of the field labels in the add-folder form.
const FORM_LABEL_WIDTH: f32 = 150.0;

#[derive(Debug, Clone)]
pub enum Msg {
    RefreshNow(RemoteId),
    Settings(settings::Msg),
    DraftLocalPathChanged(String),
    DraftRemotePathChanged(String),
    /// Open the desktop's folder chooser for the local path.
    BrowseLocalPath,
    AddSyncDir,
    /// User clicked the remove button on a sync_dir. Opens the confirmation dialog; the actual delete fires on [`Msg::ConfirmDelete`].
    RequestDeleteSyncDir(String, String, String),
    /// User clicked Remove remote. Opens the confirmation dialog; the actual delete fires on [`Msg::ConfirmDelete`].
    RequestDeleteRemote(RemoteId, String),
    /// Dialog confirm pressed — execute the pending delete.
    ConfirmDelete,
    /// Dialog Cancel pressed — drop the pending delete.
    CancelDelete,
    ToggleExclusions(SyncDirId),
    ToggleLog(SyncDirId),
    DraftExclusionChanged(SyncDirId, String),
    AddExclusion(SyncDirId),
    RemoveExclusion(SyncDirExclusionId, SyncDirId),
    Reauthenticate(RemoteId, String),
    /// Read-only log editor swallows edits but forwards scroll/select actions so users can drag through history.
    LogEditorAction(SyncDirId, text_editor::Action),
}

/// What the user is about to delete, pending confirmation. Stored at the app level and rendered by [`confirm_delete_overlay`].
#[derive(Debug, Clone)]
pub enum PendingDelete {
    Remote(RemoteId, String),
    /// `remote_label` is the display form (`Name:/path`) for the dialog text.
    SyncDir { local: String, remote: String, remote_label: String },
}

/// Everything the page renders, prepared by the app.
pub struct Page<'a> {
    pub remote: &'a Remote,
    pub state: RunState,
    pub syncing: bool,
    /// Time to the next scheduled pass and whether a backoff is active; `None` while paused / signed out.
    pub next_sync: Option<(Duration, bool)>,
    pub dirs: Vec<Folder<'a>>,
    pub draft_local: &'a str,
    pub draft_remote: &'a str,
    pub add_error: Option<&'a str>,
}

/// One sync folder row.
pub struct Folder<'a> {
    pub dir: &'a SyncDir,
    pub state: RunState,
    /// Newest log line, shown under the paths while a pass runs.
    pub latest_line: Option<&'a str>,
    /// Newest problem line, shown while the folder is in Warning / Error.
    pub latest_problem: Option<&'a str>,
    /// `Some` while the activity log is expanded.
    pub log: Option<&'a text_editor::Content>,
    pub exclusions_open: bool,
    pub auto_excluded: Vec<&'a SyncDir>,
    pub custom_excluded: &'a [SyncDirExclusion],
    pub draft_exclusion: &'a str,
}

pub fn view<'a>(page: Page<'a>) -> Element<'a, Msg> {
    let remote = page.remote;
    let auth_needed = page.state == RunState::AuthNeeded;

    let header = row![
        column![
            text(&remote.name).size(TITLE),
            row![status_icon(page.state, 14.0), text(status_line(&page)).size(CAPTION).style(theme::muted)]
                .spacing(6)
                .align_y(Alignment::Center),
        ]
        .spacing(2)
        .width(Length::Fill),
        backoff_hint(page.next_sync),
        button(row![icon(icondata::TbRefreshOutline, 16.0), text("Sync now").size(TEXT)].spacing(6).align_y(Alignment::Center))
            .padding([6, 12])
            .style(theme::button_secondary)
            .on_press_maybe((!auth_needed && !page.syncing).then_some(Msg::RefreshNow(remote.id))),
    ]
    .spacing(ROW_SPACING)
    .align_y(Alignment::Center);

    let mut body = column![].spacing(SECTION_SPACING);

    if auth_needed {
        body = body.push(
            container(
                row![
                    icon(icondata::TbLockOutline, 20.0),
                    text(format!(
                        "Celeste can no longer access {}. Sign in again to resume syncing — folders, exclusions and schedule are kept.",
                        remote.name
                    ))
                    .size(TEXT)
                    .width(Length::Fill),
                    button(text("Sign in again").size(TEXT))
                        .padding([6, 14])
                        .style(theme::button_primary)
                        .on_press(Msg::Reauthenticate(remote.id, remote.name.clone())),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .padding([10, 14])
            .width(Length::Fill)
            .style(theme::warning_bar),
        );
    }

    // ── Folders ─────────────────────────────────────────────────────────
    let mut folders = column![section_heading("Folders")].spacing(6);
    if page.dirs.is_empty() {
        folders = folders.push(
            container(text("No folders yet. Add one below to start syncing.").size(TEXT).style(theme::muted))
                .padding(16)
                .width(Length::Fill)
                .style(theme::card),
        );
    }
    for folder in page.dirs {
        folders = folders.push(folder_card(remote, folder));
    }
    body = body.push(folders);
    body = body.push(add_folder_card(remote, page.draft_local, page.draft_remote, page.add_error));

    // ── Settings ────────────────────────────────────────────────────────
    body = body.push(column![section_heading("Settings"), settings::view(remote, auth_needed).map(Msg::from_settings)].spacing(6));

    let content = column![
        header,
        scrollable(container(body).padding(iced::Padding::default().right(14.0).bottom(PAGE_PADDING)))
            .height(Length::Fill)
            .direction(theme::slim_scrollbar())
            .style(theme::scrollbar)
            .spacing(2),
    ]
    .spacing(SECTION_SPACING)
    .max_width(MAX_CONTENT_WIDTH);

    container(content)
        .padding(iced::Padding { top: PAGE_PADDING, left: PAGE_PADDING, right: PAGE_PADDING - 10.0, bottom: 0.0 })
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::page)
        .into()
}

impl Msg {
    fn from_settings(msg: settings::Msg) -> Self {
        match msg {
            settings::Msg::Reauthenticate(id, name) => Msg::Reauthenticate(id, name),
            settings::Msg::RemoveRemote(id, name) => Msg::RequestDeleteRemote(id, name),
            other => Msg::Settings(other),
        }
    }
}

fn section_heading<'a>(label: &'a str) -> Element<'a, Msg> {
    container(text(label).size(HEADING)).padding([4, 2]).into()
}

/// One-line summary under the remote's name.
fn status_line(page: &Page<'_>) -> String {
    let next = |prefix: &str| match page.next_sync {
        Some((remaining, _)) if remaining.is_zero() => format!("{prefix} · next sync now"),
        Some((remaining, _)) => format!("{prefix} · next sync in {}", format_duration(remaining)),
        None => prefix.to_owned(),
    };
    match page.state {
        RunState::AuthNeeded => "Not signed in — syncing is on hold".to_owned(),
        RunState::Paused => "Paused — automatic sync is off".to_owned(),
        RunState::Syncing(_) => status_label(page.state).to_owned(),
        _ if page.syncing => "Syncing…".to_owned(),
        RunState::Waiting => next("Waiting for the first sync"),
        state => next(status_label(state)),
    }
}

/// ⚠ with an explanation while the scheduler is backing off after rate-limit warnings.
fn backoff_hint<'a>(next_sync: Option<(Duration, bool)>) -> Element<'a, Msg> {
    if !matches!(next_sync, Some((_, true))) {
        return Space::new().into();
    }
    tooltip(
        container(row![icon(icondata::TbClockOutline, 16.0), text("Backing off").size(CAPTION)].spacing(4).align_y(Alignment::Center))
            .padding([4, 8]),
        container(
            text("The provider reported rate-limit warnings on the last pass, so Celeste skips the next cycles before trying again. More consecutive warnings mean more skipped cycles; a clean pass resets it.")
                .size(CAPTION),
        )
        .padding(10)
        .max_width(320)
        .style(theme::card),
        tooltip::Position::Bottom,
    )
    .into()
}

fn folder_card<'a>(remote: &'a Remote, folder: Folder<'a>) -> Element<'a, Msg> {
    let sd = folder.dir;
    let remote_display = if sd.remote_path.is_empty() { "/".to_owned() } else { format!("/{}", sd.remote_path) };

    // Second caption line: live progress while syncing, the latest problem while in trouble, otherwise the plain state.
    let detail = match folder.state {
        RunState::Syncing(_) => folder.latest_line.unwrap_or(status_label(folder.state)),
        RunState::Warning | RunState::Error => folder.latest_problem.unwrap_or(status_label(folder.state)),
        state => status_label(state),
    };
    let detail_text = text(detail).size(CAPTION).wrapping(Wrapping::WordOrGlyph);
    let detail_text = if matches!(folder.state, RunState::Error) {
        detail_text.style(theme::danger_text)
    } else {
        detail_text.style(theme::muted)
    };

    let excluded = folder.auto_excluded.len() + folder.custom_excluded.len();
    let top = row![
        status_icon(folder.state, 20.0),
        column![
            text(fmt_home(&sd.local_path)).size(TEXT).wrapping(Wrapping::WordOrGlyph),
            row![muted_icon(icondata::TbArrowsLeftRightOutline, 12.0), text(format!("{}:{remote_display}", remote.name)).size(CAPTION).style(theme::muted)]
                .spacing(6)
                .align_y(Alignment::Center),
            detail_text,
        ]
        .spacing(2)
        .width(Length::Fill),
        with_tip(
            button(row![icon(icondata::TbFilterOutline, 16.0), text(excluded.to_string()).size(CAPTION)].spacing(4).align_y(Alignment::Center))
                .padding([5, 8])
                .style(theme::button_toggle(folder.exclusions_open))
                .on_press(Msg::ToggleExclusions(sd.id)),
            "Exclusions",
        ),
        with_tip(
            button(icon(icondata::TbFileTextOutline, 16.0))
                .padding(6)
                .style(theme::button_toggle(folder.log.is_some()))
                .on_press(Msg::ToggleLog(sd.id)),
            "Activity log",
        ),
        with_tip(
            button(icon(icondata::TbTrashOutline, 16.0))
                .padding(6)
                .style(theme::button_flat)
                .on_press(Msg::RequestDeleteSyncDir(sd.local_path.clone(), sd.remote_path.clone(), format!("{}:{remote_display}", remote.name))),
            "Stop syncing this folder",
        ),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let mut card = column![top].spacing(10);

    if let Some(content) = folder.log {
        let sd_id = sd.id;
        card = card.push(
            text_editor(content)
                .height(Length::Fixed(LOG_HEIGHT))
                .padding(8)
                .size(CAPTION)
                .placeholder("Nothing logged yet in this session.")
                .on_action(move |action| Msg::LogEditorAction(sd_id, action)),
        );
    }

    if folder.exclusions_open {
        card = card.push(exclusion_panel(sd, &remote_display, &folder.auto_excluded, folder.custom_excluded, folder.draft_exclusion));
    }

    container(card).padding([12, 14]).width(Length::Fill).style(theme::card).into()
}

fn with_tip<'a>(content: impl Into<Element<'a, Msg>>, tip: &'a str) -> Element<'a, Msg> {
    tooltip(content, container(text(tip).size(CAPTION)).padding([4, 8]).style(theme::card), tooltip::Position::Top).into()
}

/// Exclusion list + add form for one sync_dir.
fn exclusion_panel<'a>(
    sd: &'a SyncDir,
    remote_display: &str,
    auto_excl: &[&'a SyncDir],
    custom_excl: &'a [SyncDirExclusion],
    draft: &'a str,
) -> Element<'a, Msg> {
    let mut list = column![].spacing(2);

    // Sub-trees owned by another sync folder — read-only.
    for desc in auto_excl {
        let relative = if sd.remote_path.is_empty() {
            desc.remote_path.as_str()
        } else {
            desc.remote_path.strip_prefix(&format!("{}/", sd.remote_path)).unwrap_or(&desc.remote_path)
        };
        list = list.push(
            row![
                muted_icon(icondata::TbFolderOutline, 14.0),
                text(relative.to_owned()).size(CAPTION).width(Length::Fill),
                text("synced as its own folder").size(CAPTION).style(theme::muted),
            ]
            .spacing(8)
            .padding([4, 0])
            .align_y(Alignment::Center),
        );
    }

    for excl in custom_excl {
        list = list.push(
            row![
                muted_icon(icondata::TbFolderOutline, 14.0),
                text(&excl.remote_path).size(CAPTION).width(Length::Fill),
                with_tip(
                    button(icon(icondata::TbXOutline, 14.0))
                        .padding(4)
                        .style(theme::button_flat)
                        .on_press(Msg::RemoveExclusion(excl.id, sd.id)),
                    "Sync this again",
                ),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    if auto_excl.is_empty() && custom_excl.is_empty() {
        list = list.push(text("Nothing is excluded.").size(CAPTION).style(theme::muted));
    }

    let sd_id = sd.id;
    let can_add = !draft.trim().is_empty();
    let form = row![
        text_input("Sub-folder or file to exclude, e.g. Videos/Raw", draft)
            .on_input(move |s| Msg::DraftExclusionChanged(sd_id, s))
            .on_submit_maybe(can_add.then_some(Msg::AddExclusion(sd_id)))
            .padding(6)
            .size(CAPTION)
            .style(theme::input),
        button(text("Exclude").size(CAPTION))
            .padding([6, 12])
            .style(theme::button_secondary)
            .on_press_maybe(can_add.then_some(Msg::AddExclusion(sd_id))),
    ]
    .spacing(ROW_SPACING)
    .align_y(Alignment::Center);

    container(
        column![
            text(format!("Excluded from sync — paths relative to {remote_display}")).size(CAPTION).style(theme::muted),
            list,
            form,
        ]
        .spacing(ROW_SPACING),
    )
    .padding(10)
    .width(Length::Fill)
    .style(theme::well)
    .into()
}

fn add_folder_card<'a>(remote: &'a Remote, draft_local: &'a str, draft_remote: &'a str, error: Option<&'a str>) -> Element<'a, Msg> {
    let label = |s: String| text(s).size(TEXT).width(Length::Fixed(FORM_LABEL_WIDTH));
    let can_add = !draft_local.trim().is_empty();

    let mut col = column![
        text("Add a folder").size(TEXT),
        row![
            label("On this computer".to_owned()),
            text_input("/home/you/Documents", draft_local)
                .on_input(Msg::DraftLocalPathChanged)
                .on_submit_maybe(can_add.then_some(Msg::AddSyncDir))
                .padding(7)
                .size(TEXT)
                .style(theme::input),
            button(row![icon(icondata::TbFolderOpenOutline, 16.0), text("Browse…").size(TEXT)].spacing(6).align_y(Alignment::Center))
                .padding([6, 12])
                .style(theme::button_secondary)
                .on_press(Msg::BrowseLocalPath),
        ]
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center),
        row![
            label(format!("On {}", remote.name)),
            text_input("Folder path, e.g. Documents (empty = whole drive)", draft_remote)
                .on_input(Msg::DraftRemotePathChanged)
                .on_submit_maybe(can_add.then_some(Msg::AddSyncDir))
                .padding(7)
                .size(TEXT)
                .style(theme::input),
        ]
        .spacing(ROW_SPACING)
        .align_y(Alignment::Center),
    ]
    .spacing(10);

    if let Some(err) = error {
        col = col.push(text(err).size(CAPTION).style(theme::danger_text));
    }

    col = col.push(row![
        text("Missing folders are created on both sides.").size(CAPTION).style(theme::muted).width(Length::Fill),
        button(text("Add folder").size(TEXT))
            .padding([6, 14])
            .style(theme::button_primary)
            .on_press_maybe(can_add.then_some(Msg::AddSyncDir)),
    ]
    .align_y(Alignment::Center));

    container(col).padding([12, 14]).width(Length::Fill).style(theme::card).into()
}

/// Dimmed backdrop + centred confirmation card for a pending delete.
pub fn confirm_delete_overlay<'a>(pending: &'a PendingDelete) -> Element<'a, Msg> {
    let (title, body, confirm) = match pending {
        PendingDelete::Remote(_id, name) => (
            format!("Remove {name}?"),
            "Celeste stops syncing this remote and forgets its folders and exclusions. No files are deleted — neither on this computer nor in the cloud.".to_owned(),
            "Remove remote",
        ),
        PendingDelete::SyncDir { local, remote_label, .. } => {
            (
                "Stop syncing this folder?".to_owned(),
                format!(
                    "{} and {remote_label} will no longer be kept in sync. No files are deleted — neither on this computer nor in the cloud.",
                    fmt_home(local)
                ),
                "Stop syncing",
            )
        }
    };

    let card = container(
        column![
            text(title).size(HEADING + 2.0),
            text(body).size(TEXT),
            row![
                Space::new().width(Length::Fill),
                button(text("Cancel").size(TEXT)).padding([6, 14]).style(theme::button_secondary).on_press(Msg::CancelDelete),
                button(text(confirm).size(TEXT)).padding([6, 14]).style(theme::button_danger).on_press(Msg::ConfirmDelete),
            ]
            .spacing(ROW_SPACING),
        ]
        .spacing(16),
    )
    .padding(22)
    .max_width(460)
    .style(theme::dialog);

    modal(card.into(), Some(Msg::CancelDelete))
}

/// Centre `content` over a dimmed backdrop that swallows clicks; clicking the backdrop itself sends `on_dismiss`.
pub fn modal<'a, M: Clone + 'a>(content: Element<'a, M>, on_dismiss: Option<M>) -> Element<'a, M> {
    let mut backdrop = mouse_area(container(Space::new()).width(Length::Fill).height(Length::Fill).style(theme::backdrop));
    if let Some(msg) = on_dismiss {
        backdrop = backdrop.on_press(msg);
    }
    stack![opaque(backdrop), center(opaque(content))].into()
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let (hours, minutes, seconds) = (secs / 3600, secs / 60 % 60, secs % 60);
    if hours > 0 {
        format!("{hours} h {minutes:02} min")
    } else if minutes > 0 {
        format!("{minutes} min {seconds:02} s")
    } else {
        format!("{seconds} s")
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;
    use std::time::Duration;

    #[test]
    fn format_duration_shapes() {
        assert_eq!(format_duration(Duration::ZERO), "0 s");
        assert_eq!(format_duration(Duration::from_secs(9)), "9 s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1 min 05 s");
        assert_eq!(format_duration(Duration::from_secs(3_900)), "1 h 05 min");
    }
}
