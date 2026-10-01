//! Interval picker — presents the full set of supported [`Interval`]
//! choices. When a `warn_below` threshold is supplied, options
//! shorter than the threshold get a trailing ⚠ glyph to flag them
//! (provider-specific rate-limit tripwires, typically). The glyph
//! renders via the fallback fonts loaded at startup (see
//! `app::fallback_fonts`). The companion tooltip lives in the
//! caller's layout.

use std::fmt;

use iced::{
    widget::{pick_list, text::Shaping},
    Element,
};

use crate::domain::remote::Interval;

impl fmt::Display for Interval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Interval::FiveSeconds => "5 s",
            Interval::FifteenSeconds => "15 s",
            Interval::ThirtySeconds => "30 s",
            Interval::OneMinute => "1 min",
            Interval::FiveMinutes => "5 min",
            Interval::FifteenMinutes => "15 min",
            Interval::ThirtyMinutes => "30 min",
            Interval::OneHour => "1 hour",
        };
        f.write_str(label)
    }
}

/// Wrapper that carries a warn flag through `pick_list`'s Display-based
/// rendering. The flag decides whether the ⚠ glyph gets tacked on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Option {
    interval: Interval,
    warn: bool,
}

impl fmt::Display for Option {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.interval.fmt(f)?;
        if self.warn {
            f.write_str("  ⚠")?;
        }
        Ok(())
    }
}

pub fn view<Msg: 'static + Clone>(
    current: Interval,
    warn_below: std::option::Option<Interval>,
    on_change: impl Fn(Interval) -> Msg + 'static,
) -> Element<'static, Msg> {
    let options: Vec<Option> = Interval::ALL
        .iter()
        .map(|i| Option {
            interval: *i,
            warn: warn_below
                .map(|t| i.seconds() < t.seconds())
                .unwrap_or(false),
        })
        .collect();
    let current_opt = options
        .iter()
        .find(|o| o.interval == current)
        .copied()
        .unwrap_or(Option {
            interval: current,
            warn: false,
        });
    pick_list(options, Some(current_opt), move |o| on_change(o.interval))
        .text_shaping(Shaping::Advanced)
        .text_size(crate::theme::TEXT)
        .padding([6, 10])
        .style(crate::theme::pick_list)
        .menu_style(crate::theme::menu)
        .into()
}
