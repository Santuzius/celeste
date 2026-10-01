//! Per-remote settings card: automatic sync on/off, interval, account status and removal — one labelled row each, in the style of KDE / Windows 11 settings pages.

use iced::{
    widget::{button, column, container, row, rule, toggler, tooltip},
    Alignment, Element, Length,
};

use crate::{
    domain::remote::{Interval, Remote, RemoteId, SyncPolicy},
    theme::{self, CAPTION, TEXT},
    widgets::{duration_picker, icon::icon, text},
};

#[derive(Debug, Clone)]
pub enum Msg {
    EnabledToggled(bool),
    IntervalChanged(Interval),
    Reauthenticate(RemoteId, String),
    RemoveRemote(RemoteId, String),
}

/// The updated policy for a policy-changing message, `None` for the others.
pub fn policy_from(msg: &Msg, current: &SyncPolicy) -> Option<SyncPolicy> {
    let mut policy = current.clone();
    match msg {
        Msg::EnabledToggled(v) => policy.enabled = *v,
        Msg::IntervalChanged(i) => policy.interval = *i,
        Msg::Reauthenticate(..) | Msg::RemoveRemote(..) => return None,
    }
    Some(policy)
}

pub fn view(remote: &Remote, auth_needed: bool) -> Element<'_, Msg> {
    let warn_below = remote.provider_kind.and_then(|k| k.short_interval_threshold());
    let mut interval_control = row![duration_picker::view(remote.policy.interval, warn_below, Msg::IntervalChanged)]
        .spacing(8)
        .align_y(Alignment::Center);
    // Providers with a short-interval tripwire get a hoverable explanation next to the picker.
    if let Some(warning) = remote.provider_kind.and_then(|k| k.short_interval_warning()) {
        interval_control = interval_control.push(tooltip(
            icon(icondata::TbAlertTriangleOutline, 16.0),
            container(text(warning).size(CAPTION)).padding(10).max_width(320).style(theme::card),
            tooltip::Position::Left,
        ));
    }

    let (account_detail, account_button) = if auth_needed {
        ("Not signed in — syncing is on hold until you sign in again.", theme::button_primary as fn(&_, _) -> _)
    } else {
        ("Signed in. Sign in again to switch the account or refresh the session.", theme::button_secondary as fn(&_, _) -> _)
    };

    let rows = column![
        setting_row(
            "Automatic sync",
            "Sync this remote in the background at the interval below.",
            toggler(remote.policy.enabled).on_toggle(Msg::EnabledToggled).size(20).into(),
        ),
        rule::horizontal(1).style(theme::separator),
        setting_row("Interval", "How often Celeste checks for changes on both sides.", interval_control.into()),
        rule::horizontal(1).style(theme::separator),
        setting_row(
            "Account",
            account_detail,
            button(text("Sign in again").size(TEXT))
                .padding([6, 14])
                .style(account_button)
                .on_press(Msg::Reauthenticate(remote.id, remote.name.clone()))
                .into(),
        ),
        rule::horizontal(1).style(theme::separator),
        setting_row(
            "Remove remote",
            "Stops syncing and forgets this remote's folders. No files are deleted.",
            button(text("Remove…").size(TEXT))
                .padding([6, 14])
                .style(theme::button_danger)
                .on_press(Msg::RemoveRemote(remote.id, remote.name.clone()))
                .into(),
        ),
    ];

    container(rows).padding([2, 0]).width(Length::Fill).style(theme::card).into()
}

fn setting_row<'a>(title: &'a str, detail: &'a str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        column![text(title).size(TEXT), text(detail).size(CAPTION).style(theme::muted)]
            .spacing(2)
            .width(Length::Fill),
        control,
    ]
    .spacing(16)
    .padding([12, 14])
    .align_y(Alignment::Center)
    .into()
}
