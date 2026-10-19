//! User choices that survive a restart: custom thresholds, hidden sensors, time window,
//! and the fastest speed seen for each fan.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A custom threshold. A `None` field keeps the default for that threshold.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Override {
    pub safe: Option<f64>,
    pub throttle: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Keyed by sensor id.
    pub overrides: BTreeMap<String, Override>,
    /// Sensor ids the user chose not to chart.
    pub hidden: BTreeSet<String>,
    /// Width of the chart time window, in seconds.
    pub window_secs: u32,
    /// Fastest speed seen per fan id, in RPM. The UI uses it as the fan's maximum
    /// when the hardware reports no rated maximum and no PWM duty.
    pub fan_peaks: BTreeMap<String, u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            hidden: BTreeSet::new(),
            window_secs: 300,
            fan_peaks: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Missing or unreadable settings fall back to the defaults; the app must still start.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes to a temporary file first so a crash never leaves half a file behind.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        assert_eq!(Settings::load(&path), Settings::default());

        let mut s = Settings {
            window_secs: 600,
            ..Settings::default()
        };
        s.hidden.insert("acpitz/thermal_zone0/temp1".into());
        s.overrides.insert(
            "k10temp/0000:00:18.3/temp1".into(),
            Override {
                safe: Some(80.0),
                throttle: None,
            },
        );
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), s);

        fs::write(&path, r#"{"windowSecs": 60}"#).unwrap();
        let partial = Settings::load(&path);
        assert_eq!(partial.window_secs, 60);
        assert!(partial.overrides.is_empty());
    }
}
