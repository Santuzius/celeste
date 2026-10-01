//! Iced theme, colour tones and widget styles.
//!
//! Deliberately plain: a neutral background, slightly raised cards with a hairline border, neutral buttons by default, the accent colour only for the one main action of a view and red only for destructive ones — roughly the visual language of KDE Breeze and Windows 11 settings. All styles are plain functions so nothing is allocated per frame.

use std::sync::LazyLock;

use iced::{
    border, theme,
    widget::{button, container, overlay, pick_list as pick, rule, scrollable, text, text_input},
    Background, Border, Color, Shadow, Theme, Vector,
};

use crate::domain::run_state::RunState;

/// Outer padding of the content area.
pub const PAGE_PADDING: f32 = 24.0;
/// Spacing between stacked sections within a page.
pub const SECTION_SPACING: f32 = 20.0;
/// Spacing between items inside a row or a tight column.
pub const ROW_SPACING: f32 = 8.0;
/// Corner radius of cards, dialogs and buttons.
pub const RADIUS: f32 = 6.0;
/// Width of the left navigation pane.
pub const NAV_WIDTH: f32 = 248.0;

/// Body text size.
pub const TEXT: f32 = 14.0;
/// Secondary / caption text size.
pub const CAPTION: f32 = 12.0;
/// Page title size.
pub const TITLE: f32 = 24.0;
/// Section heading size.
pub const HEADING: f32 = 16.0;

/// Fixed colours the widgets draw with, one set per light/dark mode.
pub struct Tones {
    pub background: Color,
    pub nav: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
    pub danger_hover: Color,
    /// Background of destructive buttons (white text on it), darker than `danger` in dark mode for contrast.
    pub danger_fill: Color,
    pub danger_fill_hover: Color,
    pub selection: Color,
}

const LIGHT: Tones = Tones {
    background: rgb(0xf3f3f4),
    nav: rgb(0xebebed),
    surface: rgb(0xffffff),
    surface_hover: rgb(0xf5f6f7),
    border: rgb(0xdcdde0),
    text: rgb(0x1c1c1f),
    muted: rgb(0x606368),
    accent: rgb(0x2a73c5),
    accent_hover: rgb(0x2464ad),
    on_accent: rgb(0xffffff),
    success: rgb(0x2a8a4f),
    warning: rgb(0xb7790f),
    danger: rgb(0xc4372f),
    danger_hover: rgb(0xa92e27),
    danger_fill: rgb(0xc4372f),
    danger_fill_hover: rgb(0xa92e27),
    selection: rgb(0xdde7f4),
};

const DARK: Tones = Tones {
    background: rgb(0x1f2023),
    nav: rgb(0x18191b),
    surface: rgb(0x2a2b2f),
    surface_hover: rgb(0x323337),
    border: rgb(0x3a3b40),
    text: rgb(0xe6e6e8),
    muted: rgb(0xa0a2a8),
    accent: rgb(0x4c9be8),
    accent_hover: rgb(0x63a9ee),
    on_accent: rgb(0x0e1520),
    success: rgb(0x4cbb73),
    warning: rgb(0xe3a93a),
    danger: rgb(0xe5584f),
    danger_hover: rgb(0xec7068),
    danger_fill: rgb(0xc0392f),
    danger_fill_hover: rgb(0xd24a40),
    selection: rgb(0x2c3b4f),
};

const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

fn palette(t: &Tones) -> theme::Palette {
    theme::Palette {
        background: t.background,
        text: t.text,
        primary: t.accent,
        success: t.success,
        warning: t.warning,
        danger: t.danger,
    }
}

static LIGHT_THEME: LazyLock<Theme> = LazyLock::new(|| Theme::custom("Celeste Light", palette(&LIGHT)));
static DARK_THEME: LazyLock<Theme> = LazyLock::new(|| Theme::custom("Celeste Dark", palette(&DARK)));

/// Pick the theme that mirrors the system colour-scheme iced reports (freedesktop `color-scheme` portal). `Mode::None` falls back to dark, matching the libadwaita default the GTK build inherited from upstream.
pub fn celeste_theme(mode: theme::Mode) -> Theme {
    match mode {
        theme::Mode::Light => LIGHT_THEME.clone(),
        theme::Mode::Dark | theme::Mode::None => DARK_THEME.clone(),
    }
}

pub fn tones(theme: &Theme) -> &'static Tones {
    if theme.extended_palette().is_dark { &DARK } else { &LIGHT }
}

/// Colour of a run-state glyph.
pub fn status_color(theme: &Theme, state: RunState) -> Color {
    let t = tones(theme);
    match state {
        RunState::Waiting | RunState::Paused => t.muted,
        RunState::Synced => t.success,
        RunState::Syncing(_) => t.accent,
        RunState::Warning | RunState::AuthNeeded => t.warning,
        RunState::Error => t.danger,
    }
}

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

pub fn page(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        background: Some(t.background.into()),
        text_color: Some(t.text),
        ..Default::default()
    }
}

pub fn nav_pane(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        background: Some(t.nav.into()),
        text_color: Some(t.text),
        ..Default::default()
    }
}

/// Raised card holding a group of rows.
pub fn card(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        background: Some(t.surface.into()),
        text_color: Some(t.text),
        border: Border { width: 1.0, radius: RADIUS.into(), color: t.border },
        ..Default::default()
    }
}

/// Inset well, e.g. behind the log or the exclusion list inside a card.
pub fn well(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        background: Some(t.background.into()),
        text_color: Some(t.text),
        border: Border { width: 1.0, radius: (RADIUS - 2.0).into(), color: t.border },
        ..Default::default()
    }
}

/// Modal dialog card.
pub fn dialog(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        shadow: Shadow { color: Color { a: 0.35, ..Color::BLACK }, offset: Vector::new(0.0, 8.0), blur_radius: 28.0 },
        ..card(theme)
    }
    .border(Border { width: 1.0, radius: (RADIUS + 2.0).into(), color: t.border })
}

pub fn backdrop(_theme: &Theme) -> container::Style {
    container::Style::default().background(Color { a: 0.45, ..Color::BLACK })
}

/// Top bar of the content area (title, status and page actions), set off from the scrolling body below by [`separator`] like a KDE page header.
pub fn header_bar(theme: &Theme) -> container::Style {
    let t = tones(theme);
    container::Style {
        background: Some(t.nav.into()),
        text_color: Some(t.text),
        ..Default::default()
    }
}

/// Tinted message bar (KDE `KMessageWidget` style) in the warning tone.
pub fn warning_bar(theme: &Theme) -> container::Style {
    let t = tones(theme);
    let tint = |a| Color { a, ..t.warning };
    container::Style {
        background: Some(tint(0.14).into()),
        text_color: Some(t.text),
        border: Border { width: 1.0, radius: RADIUS.into(), color: tint(0.55) },
        ..Default::default()
    }
}

/// Thin accent pill marking the selected navigation entry.
pub fn nav_indicator(theme: &Theme) -> container::Style {
    container::Style::default()
        .background(tones(theme).accent)
        .border(border::rounded(2))
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

fn button_base(bg: Color, fg: Color, border_color: Color) -> button::Style {
    button::Style {
        background: Some(Background::Color(bg)),
        text_color: fg,
        border: Border { width: 1.0, radius: RADIUS.into(), color: border_color },
        ..Default::default()
    }
}

fn disabled(style: button::Style) -> button::Style {
    button::Style {
        background: style.background.map(|b| b.scale_alpha(0.5)),
        text_color: style.text_color.scale_alpha(0.45),
        border: Border { color: style.border.color.scale_alpha(0.5), ..style.border },
        ..style
    }
}

/// Neutral default button.
pub fn button_secondary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tones(theme);
    let base = button_base(t.surface, t.text, t.border);
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style { background: Some(t.surface_hover.into()), ..base },
        button::Status::Pressed => button::Style { background: Some(t.background.into()), ..base },
        button::Status::Disabled => disabled(base),
    }
}

/// The one main action of a view.
pub fn button_primary(theme: &Theme, status: button::Status) -> button::Style {
    let t = tones(theme);
    let base = button_base(t.accent, t.on_accent, t.accent);
    match status {
        button::Status::Active | button::Status::Pressed => base,
        button::Status::Hovered => button_base(t.accent_hover, t.on_accent, t.accent_hover),
        button::Status::Disabled => disabled(base),
    }
}

/// Destructive action.
pub fn button_danger(theme: &Theme, status: button::Status) -> button::Style {
    let t = tones(theme);
    let base = button_base(t.danger_fill, Color::WHITE, t.danger_fill);
    match status {
        button::Status::Active | button::Status::Pressed => base,
        button::Status::Hovered => button_base(t.danger_fill_hover, Color::WHITE, t.danger_fill_hover),
        button::Status::Disabled => disabled(base),
    }
}

/// Borderless icon / toolbar button that only shows a background on hover.
pub fn button_flat(theme: &Theme, status: button::Status) -> button::Style {
    let t = tones(theme);
    let base = button::Style {
        background: None,
        text_color: t.text,
        border: border::rounded(RADIUS),
        ..Default::default()
    };
    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style { background: Some(Color { a: 0.08, ..t.text }.into()), ..base },
        button::Status::Pressed => button::Style { background: Some(Color { a: 0.14, ..t.text }.into()), ..base },
        button::Status::Disabled => button::Style { text_color: t.text.scale_alpha(0.4), ..base },
    }
}

/// Toggle-like flat button that stays tinted while its panel is open.
pub fn button_toggle(open: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let style = button_flat(theme, status);
        if open && matches!(status, button::Status::Active) {
            button::Style { background: Some(tones(theme).selection.into()), ..style }
        } else {
            style
        }
    }
}

/// Neutral button that stays tinted while the view it opens is shown.
pub fn button_secondary_toggle(open: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let style = button_secondary(theme, status);
        let t = tones(theme);
        if open && !matches!(status, button::Status::Disabled) {
            button::Style { background: Some(t.selection.into()), border: Border { color: t.accent, ..style.border }, ..style }
        } else {
            style
        }
    }
}

/// Entry in the navigation pane.
pub fn nav_item(selected: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let t = tones(theme);
        let bg = match (selected, status) {
            (true, _) => Some(t.selection),
            (false, button::Status::Hovered) => Some(Color { a: 0.06, ..t.text }),
            (false, button::Status::Pressed) => Some(Color { a: 0.10, ..t.text }),
            _ => None,
        };
        button::Style {
            background: bg.map(Background::Color),
            text_color: t.text,
            border: border::rounded(RADIUS),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Text & inputs
// ---------------------------------------------------------------------------

pub fn muted(theme: &Theme) -> text::Style {
    text::Style { color: Some(tones(theme).muted) }
}

pub fn danger_text(theme: &Theme) -> text::Style {
    text::Style { color: Some(tones(theme).danger) }
}

pub fn input(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let t = tones(theme);
    let base = text_input::default(theme, status);
    let border_color = match status {
        text_input::Status::Focused { .. } => t.accent,
        text_input::Status::Hovered => t.muted,
        _ => t.border,
    };
    text_input::Style {
        background: t.surface.into(),
        border: Border { width: 1.0, radius: RADIUS.into(), color: border_color },
        placeholder: t.muted,
        value: t.text,
        selection: t.selection,
        ..base
    }
}

/// Hairline separator between rows of a card.
pub fn separator(theme: &Theme) -> rule::Style {
    rule::Style { color: tones(theme).border, radius: 0.0.into(), fill_mode: rule::FillMode::Full, snap: true }
}

pub fn pick_list(theme: &Theme, status: pick::Status) -> pick::Style {
    let t = tones(theme);
    pick::Style {
        text_color: t.text,
        placeholder_color: t.muted,
        handle_color: t.muted,
        background: match status {
            pick::Status::Hovered => t.surface_hover.into(),
            _ => t.surface.into(),
        },
        border: Border {
            width: 1.0,
            radius: RADIUS.into(),
            color: if matches!(status, pick::Status::Opened { .. }) { t.accent } else { t.border },
        },
    }
}

pub fn menu(theme: &Theme) -> overlay::menu::Style {
    let t = tones(theme);
    overlay::menu::Style {
        background: t.surface.into(),
        border: Border { width: 1.0, radius: RADIUS.into(), color: t.border },
        text_color: t.text,
        selected_text_color: t.text,
        selected_background: t.selection.into(),
        shadow: Shadow { color: Color { a: 0.25, ..Color::BLACK }, offset: Vector::new(0.0, 4.0), blur_radius: 12.0 },
    }
}

/// Slim overlay-like scrollbar: no rail, a muted thumb that firms up on hover / drag.
pub fn scrollbar(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let t = tones(theme);
    let alpha = match status {
        scrollable::Status::Active { .. } => 0.30,
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered: true, .. }
        | scrollable::Status::Dragged { is_vertical_scrollbar_dragged: true, .. } => 0.65,
        _ => 0.42,
    };
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: Color { a: alpha, ..t.muted }.into(), border: border::rounded(4) },
    };
    let base = scrollable::default(theme, status);
    scrollable::Style { container: container::Style::default(), vertical_rail: rail, horizontal_rail: rail, gap: None, ..base }
}

/// Vertical scrollbar geometry matching [`scrollbar`].
pub fn slim_scrollbar() -> scrollable::Direction {
    scrollable::Direction::Vertical(scrollable::Scrollbar::new().width(6).scroller_width(6).margin(2))
}
