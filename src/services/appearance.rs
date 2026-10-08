//! Colour choices for the window and the tray icon, kept in `appearance.conf` in the data dir as `key=value` lines. A missing or unreadable file means "follow the system" for both.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    /// Colour scheme of the window.
    pub window: ThemeChoice,
    /// Colour of the tray icon itself: Light is a white icon for dark panels, Dark a black one for light panels.
    pub tray_icon: ThemeChoice,
}

impl Appearance {
    pub fn load(data_dir: &Path) -> Self {
        let mut appearance = Self::default();
        let Ok(content) = std::fs::read_to_string(data_dir.join(FILE_NAME)) else {
            return appearance;
        };
        for line in content.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match (key.trim(), ThemeChoice::parse(value)) {
                ("window", Some(choice)) => appearance.window = choice,
                ("tray_icon", Some(choice)) => appearance.tray_icon = choice,
                _ => {}
            }
        }
        appearance
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, format!("window={}\ntray_icon={}\n", self.window.key(), self.tray_icon.key()))?;
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
        let chosen = Appearance { window: ThemeChoice::Dark, tray_icon: ThemeChoice::Light };
        chosen.save(&dir).unwrap();
        assert_eq!(Appearance::load(&dir), chosen);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
