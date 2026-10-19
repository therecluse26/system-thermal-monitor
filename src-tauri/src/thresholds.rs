//! Default "keep below" and "throttling likely" temperatures for each kind of sensor.
//!
//! The hardware limit wins when the device reports a believable one. Otherwise the
//! value comes from a per-chip profile, marked as an estimate so the UI can say so.

use serde::Serialize;

use crate::sensors::Category;

/// Limits the hardware reports for one sensor, in °C. `None` when absent or not believable.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Limits {
    /// hwmon `tempN_max`: a warning or high-alarm level. For NVMe this is WCTEMP.
    pub max: Option<f64>,
    /// hwmon `tempN_crit`, or an ACPI "critical" trip point.
    pub crit: Option<f64>,
    /// hwmon `tempN_emergency`: usually the forced-shutdown level.
    pub emergency: Option<f64>,
    /// ACPI "passive" trip point: the firmware starts throttling here.
    pub passive: Option<f64>,
}

/// A reported limit is believable when it sits where real hardware limits sit.
/// This drops sentinels such as -273.15 °C, 65261.85 °C and ACPI's 160 °C placeholders.
pub fn believable(celsius: f64) -> bool {
    (30.0..=150.0).contains(&celsius)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The throttle point comes from the device itself.
    Hardware,
    /// The device reports nothing usable; the numbers are typical values for its kind.
    Estimate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Thresholds {
    /// Keep the sensor below this for full performance and long-term health.
    pub safe: f64,
    /// At or above this, the device is likely to throttle.
    pub throttle: f64,
    pub source: Source,
    /// Plain-language reason for the numbers, shown in the threshold editor.
    pub basis: String,
}

impl Thresholds {
    fn hardware(throttle: f64, margin: f64, basis: String) -> Self {
        Self::new(throttle, margin, Source::Hardware, basis)
    }

    fn estimate(throttle: f64, margin: f64, basis: &str) -> Self {
        Self::new(throttle, margin, Source::Estimate, basis.to_string())
    }

    fn new(throttle: f64, margin: f64, source: Source, basis: String) -> Self {
        Self {
            safe: (throttle - margin).round(),
            throttle,
            source,
            basis,
        }
    }
}

/// Formats a temperature the way the UI does: one decimal, dropped when it is zero.
fn deg(celsius: f64) -> String {
    let rounded = (celsius * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0} °C")
    } else {
        format!("{rounded:.1} °C")
    }
}

pub fn resolve(chip: &str, category: Category, hw: &Limits) -> Thresholds {
    match chip {
        "nvme" => match hw.max {
            Some(warn) => {
                let crit = hw
                    .crit
                    .map(|c| format!(" Its critical temperature (CCTEMP) is {}.", deg(c)))
                    .unwrap_or_default();
                Thresholds::hardware(
                    warn,
                    10.0,
                    format!(
                        "The drive reports a warning temperature (WCTEMP) of {}. Above it, the drive slows down to cool off.{crit}",
                        deg(warn)
                    ),
                )
            }
            None => Thresholds::estimate(
                80.0,
                10.0,
                "This drive reports no warning temperature. Most NVMe drives throttle between 70 °C and 85 °C.",
            ),
        },

        "drivetemp" => match hw.max.or(hw.crit) {
            Some(limit) => Thresholds::hardware(
                limit,
                10.0,
                format!("The drive reports a maximum operating temperature of {}.", deg(limit)),
            ),
            None => Thresholds::estimate(
                60.0,
                10.0,
                "This drive reports no limit. Hard drives are usually rated for 55–60 °C.",
            ),
        },

        "spd5118" | "jc42" => {
            let basis = "DDR memory works normally up to 85 °C. Above that it refreshes twice as often, which costs performance.";
            match hw.crit {
                Some(crit) => Thresholds::hardware(crit, 15.0, basis.to_string()),
                None => Thresholds::estimate(85.0, 15.0, basis),
            }
        }

        "cros_ec" => match hw.max.or(hw.crit) {
            Some(warn) => {
                let halt = hw
                    .emergency
                    .map(|e| format!(" It forces a shutdown at {}.", deg(e)))
                    .unwrap_or_default();
                Thresholds::hardware(
                    warn,
                    15.0,
                    format!(
                        "The embedded controller reports a warning limit of {}. Above it, the EC throttles the CPU.{halt}",
                        deg(warn)
                    ),
                )
            }
            None => generic(category, hw),
        },

        "k10temp" | "zenpower" => Thresholds::estimate(
            95.0,
            10.0,
            "AMD does not report a limit here. Ryzen CPUs pull their clocks back as Tctl nears Tjmax, which is 95–100 °C on most models.",
        ),

        "coretemp" => match hw.crit.or(hw.max) {
            Some(tjmax) => Thresholds::hardware(
                tjmax,
                15.0,
                format!(
                    "The CPU reports TjMax as {}. It throttles when it reaches this temperature.",
                    deg(tjmax)
                ),
            ),
            None => Thresholds::estimate(
                100.0,
                15.0,
                "This CPU reports no TjMax. Most Intel CPUs throttle at 100 °C.",
            ),
        },

        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => match hw.crit {
            Some(crit) => Thresholds::hardware(
                crit,
                15.0,
                format!(
                    "The GPU reports a critical temperature of {}. The driver throttles the GPU as it nears that point.",
                    deg(crit)
                ),
            ),
            None => Thresholds::estimate(
                95.0,
                10.0,
                "This GPU reports no limit. Integrated graphics share the CPU die, so they throttle with it near 95–100 °C.",
            ),
        },

        _ => generic(category, hw),
    }
}

fn generic(category: Category, hw: &Limits) -> Thresholds {
    if let Some(passive) = hw.passive {
        return Thresholds::hardware(
            passive,
            10.0,
            format!(
                "The firmware starts passive cooling (throttling) at {}.",
                deg(passive)
            ),
        );
    }
    if let Some(max) = hw.max {
        return Thresholds::hardware(
            max,
            15.0,
            format!("The sensor reports a high limit of {}.", deg(max)),
        );
    }
    if let Some(crit) = hw.crit {
        return Thresholds::hardware(
            crit - 5.0,
            10.0,
            format!(
                "The sensor reports a critical limit of {}. Throttling is likely shortly before it.",
                deg(crit)
            ),
        );
    }
    match category {
        Category::Wireless => Thresholds::estimate(
            100.0,
            20.0,
            "This chip reports no limit. Wi-Fi chips usually cut transmit power near 100–110 °C.",
        ),
        Category::Battery => Thresholds::estimate(
            50.0,
            10.0,
            "This battery reports no limit. Lithium-ion cells stop charging or slow down near 45–60 °C.",
        ),
        _ => Thresholds::estimate(
            95.0,
            15.0,
            "This sensor reports no limit. These are general values for PC parts.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvme_uses_wctemp_as_throttle_point() {
        let hw = Limits {
            max: Some(82.9),
            crit: Some(84.9),
            ..Limits::default()
        };
        let t = resolve("nvme", Category::Storage, &hw);
        assert_eq!(t.throttle, 82.9);
        assert_eq!(t.safe, 73.0);
        assert_eq!(t.source, Source::Hardware);
        assert!(t.basis.contains("82.9 °C"));
        assert!(t.basis.contains("84.9 °C"));
    }

    #[test]
    fn ryzen_has_an_estimate() {
        let t = resolve("k10temp", Category::Cpu, &Limits::default());
        assert_eq!((t.safe, t.throttle), (85.0, 95.0));
        assert_eq!(t.source, Source::Estimate);
    }

    #[test]
    fn ec_warn_limit_is_the_throttle_point() {
        let hw = Limits {
            max: Some(104.9),
            crit: Some(104.9),
            emergency: Some(124.9),
            passive: None,
        };
        let t = resolve("cros_ec", Category::Board, &hw);
        assert_eq!(t.throttle, 104.9);
        assert_eq!(t.safe, 90.0);
        assert!(t.basis.contains("124.9 °C"));
    }

    #[test]
    fn passive_trip_wins_for_unknown_chips() {
        let hw = Limits {
            passive: Some(90.0),
            crit: Some(105.0),
            ..Limits::default()
        };
        let t = resolve("acpitz", Category::Acpi, &hw);
        assert_eq!((t.safe, t.throttle), (80.0, 90.0));
    }

    #[test]
    fn safe_is_always_below_throttle() {
        for chip in ["nvme", "drivetemp", "spd5118", "cros_ec", "k10temp", "coretemp", "amdgpu", "x"] {
            let t = resolve(chip, Category::Other, &Limits::default());
            assert!(t.safe < t.throttle, "{chip}: {t:?}");
        }
    }

    #[test]
    fn believable_drops_sentinels() {
        assert!(!believable(-273.15));
        assert!(!believable(65261.85));
        assert!(!believable(160.0));
        assert!(believable(82.85));
    }
}
