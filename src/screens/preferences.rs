//! App-wide preferences dialog (as opposed to the per-remote settings): colours of the window and the tray icon, the size of the window's content, start at login — one labelled row each, like the remote settings.

use iced::{
    widget::{button, column, container, row, rule, toggler, Row, Space},
    Alignment, Element, Length,
};

use crate::{
    services::appearance::{Appearance, SizeChoice, ThemeChoice, TrayIconChoice},
    theme::{self, CAPTION, HEADING, TEXT},
    widgets::text,
};

#[derive(Debug, Clone)]
pub enum Msg {
    WindowThemeChanged(ThemeChoice),
    TrayIconChanged(TrayIconChoice),
    SizeChanged(SizeChoice),
    AutostartToggled(bool),
    Close,
}

/// `compact`: the phone layout, where the button groups go below their labels.
pub fn view<'a>(appearance: Appearance, autostart: bool, error: Option<&'a str>, compact: bool) -> Element<'a, Msg> {
    let android = cfg!(target_os = "android");
    let size_hint = if android { "Text and everything else in this window, relative to the system's font and display size." } else { "Text and everything else in this window." };
    let mut rows = column![
        choice_row(compact, "Theme", "Colours of this window.", segmented(&ThemeChoice::ALL, ThemeChoice::label, appearance.window, Msg::WindowThemeChanged)),
        rule::horizontal(1).style(theme::separator),
        choice_row(compact, "Size", size_hint, segmented(&SizeChoice::ALL, SizeChoice::label, appearance.size, Msg::SizeChanged)),
    ];
    // Android has no tray.
    if !android {
        rows = rows.push(rule::horizontal(1).style(theme::separator)).push(choice_row(
            compact,
            "Tray icon",
            "Colour of the icon in the panel.",
            segmented(&TrayIconChoice::ALL, TrayIconChoice::label, appearance.tray_icon, Msg::TrayIconChanged),
        ));
    }
    let (autostart_label, autostart_hint) = if android {
        ("Start Celeste when the device starts", "It syncs in the background, with its status in a silent notification.")
    } else {
        ("Start Celeste when you log in", "It starts hidden in the system tray and syncs in the background.")
    };
    rows = rows.push(rule::horizontal(1).style(theme::separator)).push(setting_row(autostart_label, autostart_hint, toggler(autostart).on_toggle(Msg::AutostartToggled).size(20).into()));

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

/// The options as one button group; the chosen one stays highlighted.
fn segmented<'a, T: Copy + PartialEq + 'a>(options: &[T], label: fn(T) -> &'static str, current: T, on_pick: fn(T) -> Msg) -> Element<'a, Msg> {
    let buttons = options.iter().map(|&choice| {
        button(text(label(choice)).size(TEXT))
            .padding([4, 12])
            .style(theme::button_toggle(choice == current))
            .on_press(on_pick(choice))
            .into()
    });
    // Wraps rather than cutting off the last options when the window is narrow or its content large.
    container(Row::with_children(buttons).spacing(2).wrap().vertical_spacing(2)).padding(2).style(theme::well).into()
}

/// A row with a button group, which needs the whole width of a phone.
fn choice_row<'a>(compact: bool, title: &'a str, detail: &'a str, control: Element<'a, Msg>) -> Element<'a, Msg> {
    if !compact {
        return setting_row(title, detail, control);
    }
    column![column![text(title).size(TEXT), text(detail).size(CAPTION).style(theme::muted)].spacing(2), control].spacing(10).padding([12, 14]).into()
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
