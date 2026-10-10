//! Monochrome icondata glyphs for the UI, tinted per theme.
//!
//! Each icon's SVG document is built once and its [`svg::Handle`] cached for the process lifetime (keyed by the static's address); colour comes from the svg widget's tint filter, so light/dark switches and status colours never allocate a new handle.

use std::{cell::RefCell, collections::HashMap};

use iced::{
    widget::{svg, Svg},
    Element, Length, Theme,
};

use crate::{domain::run_state::RunState, theme};

thread_local! {
    static HANDLES: RefCell<HashMap<usize, svg::Handle>> = RefCell::new(HashMap::new());
}

fn handle(icon: icondata::Icon) -> svg::Handle {
    let key = std::ptr::from_ref(icon) as usize;
    HANDLES.with_borrow_mut(|cache| {
        cache
            .entry(key)
            .or_insert_with(|| svg::Handle::from_memory(crate::icons::svg_document(icon, "#000").into_bytes()))
            .clone()
    })
}

/// Icon in the current text colour.
pub fn icon<'a>(icon: icondata::Icon, size: f32) -> Svg<'a, Theme> {
    svg(handle(icon))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(|theme: &Theme, _| svg::Style { color: Some(theme::tones(theme).text) })
}

/// Icon in the muted caption colour.
pub fn muted_icon<'a>(icon_data: icondata::Icon, size: f32) -> Svg<'a, Theme> {
    icon(icon_data, size).style(|theme: &Theme, _| svg::Style { color: Some(theme::tones(theme).muted) })
}

/// Icon in the accent colour (e.g. an info marker).
pub fn accent_icon<'a>(icon_data: icondata::Icon, size: f32) -> Svg<'a, Theme> {
    icon(icon_data, size).style(|theme: &Theme, _| svg::Style { color: Some(theme::tones(theme).accent) })
}

/// White icon for filled buttons (e.g. destructive ones).
pub fn on_fill_icon<'a>(icon_data: icondata::Icon, size: f32) -> Svg<'a, Theme> {
    icon(icon_data, size).style(|_: &Theme, _| svg::Style { color: Some(iced::Color::WHITE) })
}

/// Glyph for a run-state, coloured by severity.
pub fn status_icon<'a, Msg: 'a>(state: RunState, size: f32) -> Element<'a, Msg> {
    let glyph = match state {
        RunState::Waiting => icondata::TbClockOutline,
        RunState::Paused => icondata::TbPlayerPauseOutline,
        RunState::AuthNeeded => icondata::TbLockOutline,
        RunState::Syncing(_) => icondata::TbRefreshOutline,
        RunState::Synced => icondata::TbCircleCheckOutline,
        RunState::Warning => icondata::TbAlertTriangleOutline,
        RunState::Error => icondata::TbAlertCircleOutline,
        RunState::Held => crate::icons::SPEEDOMETER_OUTLINE,
    };
    icon(glyph, size)
        .style(move |theme: &Theme, _| svg::Style { color: Some(theme::status_color(theme, state)) })
        .into()
}
