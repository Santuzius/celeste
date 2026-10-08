//! App-wide preferences dialog (as opposed to the per-remote settings).

use iced::{
    widget::{button, column, container, row, toggler, Space},
    Alignment, Element, Length,
};

use crate::{
    theme::{self, CAPTION, HEADING, TEXT},
    widgets::text,
};

#[derive(Debug, Clone)]
pub enum Msg {
    AutostartToggled(bool),
    Close,
}

pub fn view<'a>(autostart: bool, error: Option<&'a str>) -> Element<'a, Msg> {
    let mut detail = column![
        text("Start Celeste when you log in").size(TEXT),
        text("It starts hidden in the system tray and syncs in the background.").size(CAPTION).style(theme::muted),
    ]
    .spacing(2)
    .width(Length::Fill);
    if let Some(err) = error {
        detail = detail.push(text(err).size(CAPTION).style(theme::danger_text));
    }
    let setting = container(
        row![detail, toggler(autostart).on_toggle(Msg::AutostartToggled).size(20)]
            .spacing(16)
            .padding([12, 14])
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .style(theme::card);

    container(
        column![
            text("Preferences").size(HEADING + 2.0),
            setting,
            row![
                Space::new().width(Length::Fill),
                button(text("Close").size(TEXT)).padding([6, 16]).style(theme::button_secondary).on_press(Msg::Close),
            ],
        ]
        .spacing(16),
    )
    .padding(22)
    .max_width(520)
    .style(theme::dialog)
    .into()
}
