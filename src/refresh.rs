//! The auto-refresh pace: which intervals we offer, how the keyboard cycles
//! through them, and how the chosen one is saved between runs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// How often the app reloads on its own. `Off` — the default — means never:
/// the absence of an interval IS the "disabled" state, so there is no separate
/// on/off flag to keep in sync.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum AutoRefresh {
    #[default]
    Off,
    M1,
    M5,
    M10,
    M30,
    H1,
}

impl AutoRefresh {
    /// How long to wait between two reloads, or `None` when disabled.
    pub fn interval(self) -> Option<Duration> {
        let secs = match self {
            AutoRefresh::Off => return None,
            AutoRefresh::M1 => 60,
            AutoRefresh::M5 => 5 * 60,
            AutoRefresh::M10 => 10 * 60,
            AutoRefresh::M30 => 30 * 60,
            AutoRefresh::H1 => 60 * 60,
        };
        Some(Duration::from_secs(secs))
    }

    /// The label shown in the header and in the help screen.
    pub fn label(self) -> &'static str {
        match self {
            AutoRefresh::Off => "off",
            AutoRefresh::M1 => "1mn",
            AutoRefresh::M5 => "5mn",
            AutoRefresh::M10 => "10mn",
            AutoRefresh::M30 => "30mn",
            AutoRefresh::H1 => "1h",
        }
    }

    /// The next pace, wrapping back to `Off` after the longest one.
    pub fn next(self) -> Self {
        match self {
            AutoRefresh::Off => AutoRefresh::M1,
            AutoRefresh::M1 => AutoRefresh::M5,
            AutoRefresh::M5 => AutoRefresh::M10,
            AutoRefresh::M10 => AutoRefresh::M30,
            AutoRefresh::M30 => AutoRefresh::H1,
            AutoRefresh::H1 => AutoRefresh::Off,
        }
    }
}

/// What we persist about the auto-refresh. A struct rather than the bare enum
/// so the file stays a JSON object, and so a later setting can join it without
/// breaking the configs already written.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct RefreshSettings {
    #[serde(default)]
    pub auto_refresh: AutoRefresh,
}

impl RefreshSettings {
    /// Path of the config file: ~/.config/gh-ui/refresh.json (Linux/macOS).
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("gh-ui").join("refresh.json"))
    }

    /// Reloads the settings, falling back to the defaults if the file is
    /// missing or unreadable — the same lenient behaviour as `Filters::load`.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Saves the settings as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_walks_every_pace_then_wraps_to_off() {
        let mut pace = AutoRefresh::Off;
        let mut seen = Vec::new();
        for _ in 0..6 {
            pace = pace.next();
            seen.push(pace);
        }
        assert_eq!(
            seen,
            vec![
                AutoRefresh::M1,
                AutoRefresh::M5,
                AutoRefresh::M10,
                AutoRefresh::M30,
                AutoRefresh::H1,
                AutoRefresh::Off, // a full tour brings us back to the start
            ]
        );
    }

    #[test]
    fn off_has_no_interval_and_the_others_grow() {
        assert_eq!(AutoRefresh::Off.interval(), None);
        assert_eq!(
            AutoRefresh::M1.interval(),
            Some(Duration::from_secs(60)) // the pace gh-ui used to hard-code
        );
        assert_eq!(AutoRefresh::H1.interval(), Some(Duration::from_secs(3600)));

        // Cycling never goes backwards in time: each pace is longer than the
        // previous one.
        let mut pace = AutoRefresh::M1;
        while pace != AutoRefresh::H1 {
            let next = pace.next();
            assert!(
                next.interval() > pace.interval(),
                "{} should be longer than {}",
                next.label(),
                pace.label()
            );
            pace = next;
        }
    }

    #[test]
    fn serde_round_trip_preserves_the_pace() {
        let original = RefreshSettings {
            auto_refresh: AutoRefresh::M10,
        };
        let json = serde_json::to_string(&original).unwrap();
        let round_trip: RefreshSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, round_trip);
    }

    #[test]
    fn an_empty_or_broken_config_falls_back_to_off() {
        // `#[serde(default)]` on the field: an object without it still parses.
        let empty: RefreshSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.auto_refresh, AutoRefresh::Off);
        // And a malformed file is what `load` feeds to `unwrap_or_default`.
        assert!(serde_json::from_str::<RefreshSettings>("nonsense").is_err());
    }
}
