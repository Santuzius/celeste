//! About dialog: version, project links, licence and credits.

use iced::{
    widget::{button, column, container, row, svg, Space},
    Alignment, Element, Length,
};

use crate::{
    branding,
    screens::add_remote::PRIVACY_POLICY,
    theme::{self, CAPTION, ROW_SPACING, TEXT, TITLE},
    widgets::text,
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const ISSUES: &str = "https://github.com/Santuzius/celeste/issues";
const UPSTREAM: &str = "https://github.com/hwittenborn/celeste";
const LICENSE: &str = "https://www.gnu.org/licenses/gpl-3.0.html";

#[derive(Debug, Clone)]
pub enum Msg {
    Open(&'static str),
    Close,
}

pub fn view<'a>() -> Element<'a, Msg> {
    let link = |label: &'a str, url: &'static str| button(text(label).size(CAPTION)).padding([5, 12]).style(theme::button_secondary).on_press(Msg::Open(url));

    let credits = column![
        text("Credits").size(TEXT),
        text("Based on the original Celeste by Hunter Wittenborn (hwittenborn) — thanks for the idea and the code this fork builds on.").size(CAPTION).style(theme::muted),
        text("App icon by Adrien Facélina, adapted for this fork.").size(CAPTION).style(theme::muted),
        text("Proton Drive support builds on go-proton-api by Proton AG and Proton-API-Bridge by Chun-Hung Tseng; Google Drive runs on rclone; the interface uses iced and Tabler Icons.").size(CAPTION).style(theme::muted),
    ]
    .spacing(4);

    let card = container(
        column![
            row![
                svg(branding::logo()).width(Length::Fixed(56.0)).height(Length::Fixed(56.0)),
                column![
                    text("Celeste").size(TITLE),
                    text(concat!("Version ", env!("CARGO_PKG_VERSION"))).size(CAPTION).style(theme::muted),
                ]
                .spacing(2),
            ]
            .spacing(14)
            .align_y(Alignment::Center),
            text("Two-way file sync between folders on this computer and Google Drive or Proton Drive.").size(TEXT),
            row![link("Website", REPOSITORY), link("Report a problem", ISSUES), link("Privacy policy", PRIVACY_POLICY), link("Original project", UPSTREAM)].spacing(ROW_SPACING),
            credits,
            text("© 2022–2024 Hunter Wittenborn, © 2026 Alexander Menzel. Licensed under the GNU GPL, version 3 or later.").size(CAPTION).style(theme::muted),
            row![
                button(text("License").size(TEXT)).padding([6, 16]).style(theme::button_secondary).on_press(Msg::Open(LICENSE)),
                Space::new().width(Length::Fill),
                button(text("Close").size(TEXT)).padding([6, 16]).style(theme::button_secondary).on_press(Msg::Close),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(14),
    )
    .padding(22)
    .max_width(520)
    .style(theme::dialog);

    card.into()
}
