//! User choices that survive a restart: custom thresholds, hidden sensors, time window,
//! the fastest speed seen for each fan, and fan curves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::curve::FanCurve;

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
    /// Keyed by fan id. A fan without a curve stays in automatic mode.
    pub fan_curves: BTreeMap<String, FanCurve>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            hidden: BTreeSet::new(),
            window_secs: 300,
            fan_peaks: BTreeMap::new(),
            fan_curves: BTreeMap::new(),
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

    /// Moves saved choices from old sensor and fan ids to new ones.
    /// Returns whether anything moved, so the caller knows to save.
    pub fn migrate(&mut self, aliases: &[(String, String)]) -> bool {
        let mut changed = false;
        for (old, new) in aliases {
            if let Some(o) = self.overrides.remove(old) {
                self.overrides.entry(new.clone()).or_insert(o);
                changed = true;
            }
            if self.hidden.remove(old) {
                self.hidden.insert(new.clone());
                changed = true;
            }
            if let Some(peak) = self.fan_peaks.remove(old) {
                let kept = self.fan_peaks.entry(new.clone()).or_insert(0);
                *kept = (*kept).max(peak);
                changed = true;
            }
            if let Some(curve) = self.fan_curves.remove(old) {
                self.fan_curves.entry(new.clone()).or_insert(curve);
                changed = true;
            }
            for curve in self.fan_curves.values_mut().filter(|c| c.sensor == *old) {
                curve.sensor = new.clone();
                changed = true;
            }
        }
        changed
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
    use crate::curve::CurvePoint;

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

        let aliases = [(
            "k10temp/0000:00:18.3/temp1".to_string(),
            "k10temp/new/temp1".to_string(),
        )];
        let mut moved = s.clone();
        assert!(moved.migrate(&aliases));
        assert!(moved.overrides.contains_key("k10temp/new/temp1"));
        assert!(!moved.overrides.contains_key("k10temp/0000:00:18.3/temp1"));
        assert!(!moved.migrate(&aliases));

        fs::write(&path, r#"{"windowSecs": 60}"#).unwrap();
        let partial = Settings::load(&path);
        assert_eq!(partial.window_secs, 60);
        assert!(partial.overrides.is_empty());
    }

    #[test]
    fn fan_curves_move_with_fan_and_sensor_ids() {
        let mut s = Settings::default();
        let curve = FanCurve {
            sensor: "k10temp/old/temp1".into(),
            points: vec![CurvePoint { temp: 50.0, duty: 0.0 }],
        };
        s.fan_curves.insert("cros_ec/cros-ec-hwmon.6.auto/fan1".into(), curve);
        let aliases = [
            ("cros_ec/cros-ec-hwmon.6.auto/fan1".to_string(), "cros_ec/cros-ec-hwmon/fan1".to_string()),
            ("k10temp/old/temp1".to_string(), "k10temp/new/temp1".to_string()),
        ];
        assert!(s.migrate(&aliases));
        let moved: Vec<(&str, &str)> = s.fan_curves.iter().map(|(fan, c)| (fan.as_str(), c.sensor.as_str())).collect();
        assert_eq!(moved, [("cros_ec/cros-ec-hwmon/fan1", "k10temp/new/temp1")]);
    }
}
