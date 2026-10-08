//! App-wide preferences dialog (as opposed to the per-remote settings): colours of the window and the tray icon, start at login — one labelled row each, like the remote settings.

use iced::{
    widget::{button, column, container, row, rule, toggler, Row, Space},
    Alignment, Element, Length,
};

use crate::{
    services::appearance::{Appearance, ThemeChoice},
    theme::{self, CAPTION, HEADING, TEXT},
    widgets::text,
};

#[derive(Debug, Clone)]
pub enum Msg {
    WindowThemeChanged(ThemeChoice),
    TrayIconChanged(ThemeChoice),
    AutostartToggled(bool),
    Close,
}

pub fn view<'a>(appearance: Appearance, autostart: bool, error: Option<&'a str>) -> Element<'a, Msg> {
    let rows = column![
        setting_row("Theme", "Colours of this window.", segmented(appearance.window, Msg::WindowThemeChanged)),
        rule::horizontal(1).style(theme::separator),
        setting_row(
            "Tray icon",
            "Light is a white icon for dark panels, Dark a black one for light panels.",
            segmented(appearance.tray_icon, Msg::TrayIconChanged),
        ),
        rule::horizontal(1).style(theme::separator),
        setting_row(
            "Start Celeste when you log in",
            "It starts hidden in the system tray and syncs in the background.",
            toggler(autostart).on_toggle(Msg::AutostartToggled).size(20).into(),
        ),
    ];

    let mut content = column![
        text("Preferences").size(HEADING + 2.0),
        container(rows).padding([2, 0]).width(Length::Fill).style(theme::card),
    ]
    .spacing(16);
    if let Some(err) = error {
        content = content.push(text(err).size(CAPTION).style(theme::danger_text));
    }
    content = content.push(row![
        Space::new().width(Length::Fill),
        button(text("Close").size(TEXT)).padding([6, 16]).style(theme::button_secondary).on_press(Msg::Close),
    ]);

    container(content).padding(22).max_width(560).style(theme::dialog).into()
}

/// System / Light / Dark as one button group; the chosen one stays highlighted.
fn segmented<'a>(current: ThemeChoice, on_pick: fn(ThemeChoice) -> Msg) -> Element<'a, Msg> {
    let buttons = ThemeChoice::ALL.into_iter().map(|choice| {
        button(text(choice.label()).size(TEXT))
            .padding([4, 12])
            .style(theme::button_toggle(choice == current))
            .on_press(on_pick(choice))
            .into()
    });
    container(Row::with_children(buttons).spacing(2)).padding(2).style(theme::well).into()
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
