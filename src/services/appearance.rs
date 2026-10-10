//! Colour choices for the window and the tray icon and the size of the window's content, kept in `appearance.conf` in the data dir as `key=value` lines. A missing or unreadable file means "follow the system" for all of them.

use std::{io, path::Path};

const FILE_NAME: &str = "appearance.conf";

/// Follow the system colour scheme or force one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    fn key(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == value.trim())
    }
}

/// Colour of the tray icon. Panels don't always follow the system colour scheme, hence the reversed and fixed variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrayIconChoice {
    /// White on a dark system scheme, black on a light one.
    #[default]
    System,
    /// The other way round, for a light panel in a dark scheme or vice versa.
    Reversed,
    White,
    Black,
}

impl TrayIconChoice {
    pub const ALL: [TrayIconChoice; 4] = [TrayIconChoice::System, TrayIconChoice::Reversed, TrayIconChoice::White, TrayIconChoice::Black];

    pub fn label(self) -> &'static str {
        match self {
            TrayIconChoice::System => "System",
            TrayIconChoice::Reversed => "Reversed system",
            TrayIconChoice::White => "White",
            TrayIconChoice::Black => "Black",
        }
    }

    fn key(self) -> &'static str {
        match self {
            TrayIconChoice::System => "system",
            TrayIconChoice::Reversed => "reversed",
            TrayIconChoice::White => "white",
            TrayIconChoice::Black => "black",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            // Written by the first version of this setting.
            "light" => Some(TrayIconChoice::White),
            "dark" => Some(TrayIconChoice::Black),
            other => Self::ALL.into_iter().find(|c| c.key() == other),
        }
    }
}

/// Size of everything in the window, text and spacing alike, relative to the system's: on Android its font size, elsewhere the desktop's scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeChoice {
    Smaller,
    #[default]
    System,
    Larger,
    Largest,
}

impl SizeChoice {
    pub const ALL: [SizeChoice; 4] = [SizeChoice::Smaller, SizeChoice::System, SizeChoice::Larger, SizeChoice::Largest];

    pub fn label(self) -> &'static str {
        match self {
            SizeChoice::Smaller => "Smaller",
            SizeChoice::System => "System",
            SizeChoice::Larger => "Larger",
            SizeChoice::Largest => "Largest",
        }
    }

    /// Factor on top of the system's size.
    pub fn factor(self) -> f32 {
        match self {
            SizeChoice::Smaller => 0.9,
            SizeChoice::System => 1.0,
            SizeChoice::Larger => 1.15,
            SizeChoice::Largest => 1.3,
        }
    }

    fn key(self) -> &'static str {
        match self {
            SizeChoice::Smaller => "smaller",
            SizeChoice::System => "system",
            SizeChoice::Larger => "larger",
            SizeChoice::Largest => "largest",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == value.trim())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    /// Colour scheme of the window.
    pub window: ThemeChoice,
    pub tray_icon: TrayIconChoice,
    pub size: SizeChoice,
}

impl Appearance {
    pub fn load(data_dir: &Path) -> Self {
        let mut appearance = Self::default();
        let Ok(content) = std::fs::read_to_string(data_dir.join(FILE_NAME)) else {
            return appearance;
        };
        for line in content.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim() {
                "window" => appearance.window = ThemeChoice::parse(value).unwrap_or_default(),
                "tray_icon" => appearance.tray_icon = TrayIconChoice::parse(value).unwrap_or_default(),
                "size" => appearance.size = SizeChoice::parse(value).unwrap_or_default(),
                _ => {}
            }
        }
        appearance
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, format!("window={}\ntray_icon={}\nsize={}\n", self.window.key(), self.tray_icon.key(), self.size.key()))?;
        std::fs::rename(&tmp, data_dir.join(FILE_NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_follows_the_system_and_saved_choices_load_back() {
        let dir = std::env::temp_dir().join(format!("celeste-appearance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Appearance::load(&dir), Appearance::default());
        let chosen = Appearance { window: ThemeChoice::Dark, tray_icon: TrayIconChoice::Reversed, size: SizeChoice::Larger };
        chosen.save(&dir).unwrap();
        assert_eq!(Appearance::load(&dir), chosen);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
