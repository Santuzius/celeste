//! Generic run-state badge: maps a [`RunState`] (or `None`) to a 24-px
//! coloured icon. Re-used by every screen that renders per-dir or per-remote
//! status next to a label.

use iced::{
    widget::{svg, Space},
    Element, Length,
};

use crate::domain::run_state::RunState;

/// Default side length used by callers that don't override the size.
pub const DEFAULT_SIZE: f32 = 24.0;

/// Render the per-row status icon, or a same-sized blank when there's no
/// run-state yet so labels keep horizontal alignment.
pub fn status_icon<'a, Msg: 'a>(state: Option<RunState>, size: f32) -> Element<'a, Msg> {
    let placeholder = || -> Element<'a, Msg> { Space::new().width(Length::Fixed(size)).into() };
    match state {
        None | Some(RunState::Waiting) => placeholder(),
        Some(RunState::Paused) => icon_svg(icondata::AiPauseCircleOutlined, "#6b7280", size),
        Some(RunState::AuthNeeded) => icon_svg(icondata::AiLockOutlined, "#f97316", size),
        Some(RunState::Syncing(_)) => icon_svg(icondata::TbRefreshOutline, "#3b82f6", size),
        Some(RunState::Synced) => icon_svg(icondata::AiCheckCircleOutlined, "#22c55e", size),
        Some(RunState::Warning) => icon_svg(icondata::AiWarningOutlined, "#eab308", size),
        Some(RunState::Error) => icon_svg(icondata::BiErrorAltRegular, "#ef4444", size),
    }
}

/// Wrap an [`icondata::Icon`] (raw inner SVG path data plus a viewBox) in a
/// real `<svg>` document and hand it to iced's SVG widget. The outer `fill`
/// cascades into any `<path>` that doesn't set its own, which gives us a
/// one-call recolour for the monochrome icons we use.
/// real `<svg>` document and hand it to iced's SVG widget. The outer paint
/// attributes cascade into any `<path>` that doesn't set its own, so a
/// `currentColor` placeholder in the icon's metadata becomes our chosen
/// tone in one shot. Mirrors `infrastructure::tray::icons::rasterise_icon`
/// — we honour both fill-based (Ant Design, BoxIcons) and stroke-based
/// (Tabler outlines, our custom cloud-sync glyph) icondata at once.
fn icon_svg<'a, Msg: 'a>(icon: icondata::Icon, color: &str, size: f32) -> Element<'a, Msg> {
    let view_box = icon.view_box.unwrap_or("0 0 24 24");
    let fill = icon.fill.unwrap_or("currentColor").replace("currentColor", color);
    let stroke = icon.stroke.unwrap_or("none").replace("currentColor", color);
    let stroke_width = icon.stroke_width.unwrap_or("1");
    let stroke_linecap = icon.stroke_linecap.unwrap_or("butt");
    let stroke_linejoin = icon.stroke_linejoin.unwrap_or("miter");
    let data = icon.data;
    let svg_doc = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}" stroke-linecap="{stroke_linecap}" stroke-linejoin="{stroke_linejoin}">{data}</svg>"##,
    );
    svg(svg::Handle::from_memory(svg_doc.into_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .into()
}
