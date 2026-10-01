//! The landing page: sidebar listing every configured remote, plus a
//! placeholder while Phase D fills in the remote detail pane.

use iced::{
    widget::{button, column, container, row, scrollable, Space},
    Alignment, Element, Length,
};

use crate::{
    domain::{
        remote::{Remote, RemoteId},
        run_state::{AppState, RunState, SyncActivity},
    },
    theme::{PAGE_PADDING, ROW_SPACING, SECTION_SPACING},
    widgets::{run_state_icon::status_icon, text},
};

/// Side length of the per-row roll-up icon.
const SIDEBAR_ICON_SIZE: f32 = 18.0;

#[derive(Debug, Clone)]
pub enum Msg {
    Selected(RemoteId),
    RefreshAll,
    AddRemote,
}

pub fn view<'a>(
    remotes: &'a [Remote],
    selected: Option<RemoteId>,
    state: &'a AppState,
) -> Element<'a, Msg> {
    let header = row![
        text("Celeste").size(24),
        Space::new().width(Length::Fill),
        button(text("Refresh all")).on_press(Msg::RefreshAll),
        button(text("Add remote")).on_press(Msg::AddRemote),
    ]
    .spacing(ROW_SPACING)
    .align_y(Alignment::Center);

    let sidebar = {
        let mut col = column![text("Remotes").size(16)].spacing(ROW_SPACING);
        for remote in remotes {
            let roll_up = state.roll_up(remote.id);
            let label = format!("{}  ({})", remote.name, status_label(roll_up, remote.policy.enabled));
            // Icon sits next to the button (not inside it) so the
            // status badge keeps its surrounding background instead of
            // inheriting the button's hover/press tint, and so the
            // icon's bounding box doesn't grow the button's clickable
            // area.
            let row_widget = row![
                status_icon(roll_up, SIDEBAR_ICON_SIZE),
                button(text(label))
                    .width(Length::Fill)
                    .on_press(Msg::Selected(remote.id)),
            ]
            .spacing(ROW_SPACING / 2.0)
            .align_y(Alignment::Center);
            col = col.push(row_widget);
        }
        scrollable(col).width(Length::Fixed(240.0))
    };

    let body: Element<Msg> = match selected {
        Some(_) => text("Remote details coming soon.").into(),
        None => text("Select a remote in the sidebar.").into(),
    };

    container(
        column![
            header,
            row![sidebar, container(body).width(Length::Fill).padding(PAGE_PADDING)]
                .spacing(PAGE_PADDING),
        ]
        .spacing(SECTION_SPACING),
    )
    .padding(PAGE_PADDING)
    .into()
}

/// Short text label paired with the row icon. Falls back to the policy's
/// `enabled` flag when the state machine has no entry yet (fresh remote
/// pre-first-tick) so a disabled-from-the-start remote still reads as paused.
fn status_label(roll_up: Option<RunState>, enabled: bool) -> &'static str {
    match (roll_up, enabled) {
        (Some(RunState::Syncing(SyncActivity::Listing)), _) => "listing…",
        (Some(RunState::Syncing(SyncActivity::Downloading)), _) => "downloading…",
        (Some(RunState::Syncing(SyncActivity::Uploading)), _) => "uploading…",
        (Some(RunState::Syncing(SyncActivity::Deleting)), _) => "deleting…",
        (Some(RunState::Syncing(SyncActivity::Resolving)), _) => "resolving…",
        (Some(RunState::AuthNeeded), _) => "needs reauth",
        (Some(RunState::Paused), _) => "paused",
        (Some(RunState::Synced), _) => "up to date",
        (Some(RunState::Warning), _) => "warning",
        (Some(RunState::Error), _) => "error",
        (Some(RunState::Waiting), false) | (None, false) => "paused",
        (Some(RunState::Waiting), true) | (None, true) => "idle",
    }
}
