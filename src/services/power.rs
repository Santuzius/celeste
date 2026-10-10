//! How much battery syncing may use on Android: the power mode chosen in Preferences and what it means for the sync engine's [`Cadence`] given the screen, the charger and Android's battery saver. Kept in `power.conf` in the data dir as `key=value` lines, together with whether Celeste already asked to be exempted from battery optimization.

use std::{io, path::Path, time::Duration};

use crate::engine::Cadence;

const FILE_NAME: &str = "power.conf";

/// The shortest interval in Balanced with the screen off.
const SCREEN_OFF_FLOOR: Duration = Duration::from_secs(5 * 60);
/// The shortest interval in Power save with the screen on.
const POWER_SAVE_FLOOR: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerMode {
    PowerSave,
    #[default]
    Balanced,
    SyncSpeed,
}

impl PowerMode {
    pub const ALL: [PowerMode; 3] = [PowerMode::PowerSave, PowerMode::Balanced, PowerMode::SyncSpeed];

    pub fn label(self) -> &'static str {
        match self {
            PowerMode::PowerSave => "Power save",
            PowerMode::Balanced => "Balanced",
            PowerMode::SyncSpeed => "Sync speed",
        }
    }

    /// What the mode does, as bullet points below the choice.
    pub fn points(self) -> &'static [&'static str] {
        match self {
            PowerMode::PowerSave => &["Screen on: at most once a minute", "Screen off: no syncing; it catches up when the screen turns on", "While charging: each remote's own interval"],
            PowerMode::Balanced => &[
                "Screen on: each remote's own interval",
                "Screen off: at most every 5 minutes",
                "Android's battery saver on: like Power save",
                "While charging: each remote's own interval",
            ],
            PowerMode::SyncSpeed => &["Always each remote's own interval, also with the screen off", "Uses the most battery"],
        }
    }

    /// The engine's cadence in this mode under `now`.
    pub fn cadence(self, now: Conditions) -> Cadence {
        let mode = if self == PowerMode::Balanced && now.battery_saver { PowerMode::PowerSave } else { self };
        match mode {
            _ if now.charging => Cadence::Full,
            PowerMode::SyncSpeed => Cadence::Full,
            PowerMode::Balanced if now.screen_off => Cadence::AtMost(SCREEN_OFF_FLOOR),
            PowerMode::Balanced => Cadence::Full,
            PowerMode::PowerSave if now.screen_off => Cadence::Held,
            PowerMode::PowerSave => Cadence::AtMost(POWER_SAVE_FLOOR),
        }
    }

    fn key(self) -> &'static str {
        match self {
            PowerMode::PowerSave => "power-save",
            PowerMode::Balanced => "balanced",
            PowerMode::SyncSpeed => "sync-speed",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == value.trim())
    }
}

/// The device's state as far as the power mode cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Conditions {
    pub screen_off: bool,
    /// Plugged in, whether or not the battery is full.
    pub charging: bool,
    pub battery_saver: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PowerSettings {
    pub mode: PowerMode,
    /// Celeste asked once to be exempted from battery optimization, whatever the answer.
    pub asked_battery: bool,
}

impl PowerSettings {
    pub fn load(data_dir: &Path) -> Self {
        let mut settings = Self::default();
        let Ok(content) = std::fs::read_to_string(data_dir.join(FILE_NAME)) else {
            return settings;
        };
        for line in content.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key.trim() {
                "mode" => settings.mode = PowerMode::parse(value).unwrap_or_default(),
                "asked_battery" => settings.asked_battery = value.trim() == "yes",
                _ => {}
            }
        }
        settings
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let tmp = data_dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, format!("mode={}\nasked_battery={}\n", self.mode.key(), if self.asked_battery { "yes" } else { "no" }))?;
        std::fs::rename(&tmp, data_dir.join(FILE_NAME))
    }
}

/// Whether Android leaves Celeste out of battery optimization; always elsewhere.
pub fn background_allowed() -> bool {
    #[cfg(target_os = "android")]
    return crate::infrastructure::android::ignores_battery_optimizations();
    #[cfg(not(target_os = "android"))]
    true
}

/// Opens Android's question whether Celeste may always run in the background.
pub fn allow_background() {
    #[cfg(target_os = "android")]
    crate::infrastructure::android::ask_to_ignore_battery_optimizations();
}

/// Applies a newly saved power mode to the running engine.
pub fn apply() {
    #[cfg(target_os = "android")]
    crate::infrastructure::android::apply_power_state();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_stretch_or_hold_with_the_screen_off_and_charging_lifts_them() {
        let on = Conditions::default();
        let off = Conditions { screen_off: true, ..on };
        assert_eq!(PowerMode::SyncSpeed.cadence(off), Cadence::Full);
        assert_eq!(PowerMode::Balanced.cadence(on), Cadence::Full);
        assert_eq!(PowerMode::Balanced.cadence(off), Cadence::AtMost(SCREEN_OFF_FLOOR));
        assert_eq!(PowerMode::Balanced.cadence(Conditions { battery_saver: true, ..off }), Cadence::Held);
        assert_eq!(PowerMode::PowerSave.cadence(on), Cadence::AtMost(POWER_SAVE_FLOOR));
        assert_eq!(PowerMode::PowerSave.cadence(off), Cadence::Held);
        assert_eq!(PowerMode::PowerSave.cadence(Conditions { charging: true, ..off }), Cadence::Full);
    }

    #[test]
    fn missing_file_is_balanced_and_saved_settings_load_back() {
        let dir = std::env::temp_dir().join(format!("celeste-power-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(PowerSettings::load(&dir), PowerSettings::default());
        let chosen = PowerSettings { mode: PowerMode::PowerSave, asked_battery: true };
        chosen.save(&dir).unwrap();
        assert_eq!(PowerSettings::load(&dir), chosen);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
