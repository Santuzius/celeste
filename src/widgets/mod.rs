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
