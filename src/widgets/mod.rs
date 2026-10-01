//! Reusable Iced widgets. Each module exports small building-block helpers
//! (functions returning `Element<Message>`); widgets never touch services
//! or domain state directly — they emit `Msg` values that screens map onto
//! service calls.

pub mod duration_picker;
pub mod icon;
mod text_ext;

/// Drop-in replacement for `iced::widget::text` that opts into
/// `Shaping::Advanced` so cosmic-text's font fallback layer actually
/// runs. Import this in every screen instead of `iced::widget::text`.
pub use text_ext::text;

/// Indented list item in body text size — for instructions and lists the user should read.
pub fn bullet<'a, M: 'a>(s: impl iced::widget::text::IntoFragment<'a>) -> iced::Element<'a, M> {
    iced::widget::row![text("•").size(crate::theme::TEXT), text(s).size(crate::theme::TEXT).wrapping(iced::widget::text::Wrapping::WordOrGlyph)]
        .spacing(8)
        .padding(iced::Padding::default().left(8.0))
        .into()
}
