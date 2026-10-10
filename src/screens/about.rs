//! About dialog: version, project links, licence and credits.

use iced::{
    widget::{button, column, container, row, svg, Space},
    Alignment, Element, Length,
};

use crate::{
    branding,
    screens::add_remote::PRIVACY_POLICY,
    theme::{self, CAPTION, ROW_SPACING, TEXT, TITLE},
    widgets::{bullet, text},
};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const UPSTREAM: &str = "https://github.com/hwittenborn/celeste";
const LICENSE: &str = "https://www.gnu.org/licenses/gpl-3.0.html";

#[derive(Debug, Clone)]
pub enum Msg {
    Open(&'static str),
    Close,
}

pub fn view<'a>() -> Element<'a, Msg> {
    let link = |label: &'a str, url: &'static str| button(text(label).size(CAPTION)).padding([5, 12]).style(theme::button_secondary).on_press(Msg::Open(url));

    let mut credits = column![
        text("Credits").size(TEXT),
        bullet("Hunter Wittenborn (hwittenborn): idea and original code of Celeste"),
        bullet("Adrien Facélina: app icon, adapted for this fork"),
        bullet("Proton AG: go-proton-api"),
        bullet("Chun-Hung Tseng: Proton-API-Bridge"),
        bullet("rclone, iced, Tabler Icons, Material Design Icons and Ionicons"),
    ]
    .spacing(4);
    // The emoji font iced_android bundles for Android 15+; its licence asks for this credit.
    if cfg!(target_os = "android") {
        credits = credits.push(bullet("Emoji on Android 15+: Twemoji by Twitter, CC-BY 4.0"));
    }

    let copyright = column![
        text("© 2022–2024 Hunter Wittenborn").size(CAPTION).style(theme::muted),
        text("© 2026 Alexander Menzel").size(CAPTION).style(theme::muted),
        text("Licensed under the GNU GPL, version 3 or later.").size(CAPTION).style(theme::muted),
    ]
    .spacing(2);

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
            text(format!("Two-way file sync between folders on {} and Google Drive or Proton Drive.", crate::util::THIS_DEVICE)).size(TEXT),
            row![link("Source", REPOSITORY), link("Privacy policy", PRIVACY_POLICY), link("Original project", UPSTREAM)].spacing(ROW_SPACING),
            credits,
            copyright,
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
