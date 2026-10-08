//! Left navigation pane (one entry per remote with its roll-up status) and the empty state shown before any remote exists.

use iced::{
    widget::{button, center, column, container, row, rule, scrollable, text::Wrapping, Space},
    Alignment, Element, Length,
};

use crate::{
    domain::{
        remote::{Remote, RemoteId},
        run_state::{RunState, SyncActivity},
    },
    theme::{self, CAPTION, NAV_WIDTH, ROW_SPACING, TEXT},
    widgets::{
        icon::{icon, muted_icon, status_icon},
        text,
    },
};

#[derive(Debug, Clone)]
pub enum Msg {
    Selected(RemoteId),
    RefreshAll,
    AddRemote,
    OpenPreferences,
    OpenAbout,
}

/// One navigation entry.
pub struct NavEntry<'a> {
    pub remote: &'a Remote,
    pub state: RunState,
    pub has_folders: bool,
}

pub fn nav<'a>(entries: Vec<NavEntry<'a>>, selected: Option<RemoteId>) -> Element<'a, Msg> {
    let mut list = column![].spacing(2);
    for entry in entries {
        list = list.push(nav_item(entry, selected));
    }

    let footer = column![
        container(rule::horizontal(1).style(theme::separator)).padding([4, 0]),
        flat_row(icondata::TbPlusOutline, "Add remote", Msg::AddRemote),
        flat_row(icondata::TbRefreshOutline, "Sync all now", Msg::RefreshAll),
        flat_row(icondata::TbAdjustmentsHorizontalOutline, "Preferences", Msg::OpenPreferences),
        flat_row(icondata::TbInfoCircleOutline, "About Celeste", Msg::OpenAbout),
    ]
    .spacing(2);

    container(
        column![
            scrollable(list)
                .height(Length::Fill)
                .direction(theme::slim_scrollbar())
                .style(theme::scrollbar),
            footer,
        ]
        .spacing(ROW_SPACING),
    )
    .padding(10)
    .width(Length::Fixed(NAV_WIDTH))
    .height(Length::Fill)
    .style(theme::nav_pane)
    .into()
}

fn nav_item<'a>(entry: NavEntry<'a>, selected: Option<RemoteId>) -> Element<'a, Msg> {
    let is_selected = selected == Some(entry.remote.id);
    // Accent pill on the selected entry, an equally wide gap otherwise so labels don't shift.
    let indicator: Element<'a, Msg> = if is_selected {
        container(Space::new())
            .width(Length::Fixed(3.0))
            .height(Length::Fixed(18.0))
            .style(theme::nav_indicator)
            .into()
    } else {
        Space::new().width(Length::Fixed(3.0)).into()
    };
    let label = column![
        text(&entry.remote.name).size(TEXT).wrapping(Wrapping::WordOrGlyph),
        text(if entry.has_folders || entry.state.is_problem() || entry.state == RunState::Paused {
            status_label(entry.state)
        } else {
            NO_FOLDERS
        })
        .size(CAPTION)
        .style(theme::muted),
    ];
    button(
        row![indicator, status_icon(entry.state, 18.0), label]
            .spacing(10)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([6, 6])
    .style(theme::nav_item(is_selected))
    .on_press(Msg::Selected(entry.remote.id))
    .into()
}

fn flat_row<'a>(glyph: icondata::Icon, label: &'a str, msg: Msg) -> Element<'a, Msg> {
    button(row![icon(glyph, 16.0), text(label).size(TEXT)].spacing(10).align_y(Alignment::Center))
        .width(Length::Fill)
        .padding([7, 12])
        .style(theme::button_flat)
        .on_press(msg)
        .into()
}

/// Label of a remote that has nothing to sync yet.
pub const NO_FOLDERS: &str = "No folders yet";

/// Short status label for a remote or a sync folder.
pub fn status_label(state: RunState) -> &'static str {
    match state {
        RunState::Syncing(SyncActivity::Listing) => "Checking for changes…",
        RunState::Syncing(SyncActivity::Downloading) => "Downloading…",
        RunState::Syncing(SyncActivity::Uploading) => "Uploading…",
        RunState::Syncing(SyncActivity::Deleting) => "Removing files…",
        RunState::Syncing(SyncActivity::Resolving) => "Resolving conflicts…",
        RunState::AuthNeeded => "Not signed in",
        RunState::Paused => "Paused",
        RunState::Synced => "Up to date",
        RunState::Warning => "Synced with problems",
        RunState::Error => "Sync failed",
        RunState::Waiting => "Waiting",
    }
}

/// Content shown while no remote is configured.
pub fn empty_state<'a>() -> Element<'a, Msg> {
    center(
        column![
            muted_icon(icondata::TbCloudOutline, 56.0),
            text("No remotes yet").size(theme::TITLE),
            text("Connect a Proton Drive or Google Drive account, then pick the folders to keep in sync.")
                .size(TEXT)
                .style(theme::muted)
                .align_x(Alignment::Center),
            Space::new().height(Length::Fixed(4.0)),
            button(text("Add remote").size(TEXT))
                .padding([8, 18])
                .style(theme::button_primary)
                .on_press(Msg::AddRemote),
        ]
        .spacing(ROW_SPACING)
        .max_width(380)
        .align_x(Alignment::Center),
    )
    .into()
}
