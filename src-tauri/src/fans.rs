//! Finds and reads fan speeds that the kernel exposes through hwmon.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::sensors::{device_key, numbered, read_trimmed, SysPaths};

/// Drivers that report the same physical fans as another driver: (duplicate, preferred).
/// The duplicate's fans are skipped when the preferred driver has fans.
const DUPLICATE_SOURCES: &[(&str, &str)] = &[("framework_laptop", "cros_ec")];

/// A reading above this is a glitch, not a fan.
const MAX_PLAUSIBLE_RPM: u32 = 30_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fan {
    /// Stable across reboots where the hardware allows it. Settings are keyed by it.
    pub id: String,
    pub chip: String,
    /// Short name of what the fan belongs to, e.g. "System" or "GPU".
    pub device: String,
    /// e.g. "Fan 1", or the driver's own label.
    pub label: String,
    /// Rated maximum speed from `fanN_max`, when the hardware reports one.
    pub rated_max: Option<u32>,
    #[serde(skip)]
    input: PathBuf,
    #[serde(skip)]
    target: PathBuf,
    /// `pwmN` and its full-scale value, when the file is readable.
    #[serde(skip)]
    pwm: Option<(PathBuf, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanReading {
    pub rpm: u32,
    /// Speed the controller asks for, when it reports one.
    pub target: Option<u32>,
    /// PWM duty cycle in percent, when it is readable.
    pub duty: Option<f64>,
}

fn read_rpm(path: &Path) -> Option<u32> {
    read_trimmed(path)?
        .parse()
        .ok()
        .filter(|rpm| *rpm <= MAX_PLAUSIBLE_RPM)
}

fn device_name(chip: &str) -> String {
    match chip {
        "cros_ec" | "framework_laptop" | "thinkpad" | "dell_smm" | "hp_wmi" | "asus_wmi_sensors" => "System",
        "amdgpu" | "radeon" | "nouveau" => "GPU",
        _ if ["nct", "it8", "f71", "w83", "asus"].iter().any(|p| chip.starts_with(p)) => "Board",
        _ => chip,
    }
    .to_string()
}

/// Current speed, or `None` when the fan cannot be read right now.
pub fn read(fan: &Fan) -> Option<FanReading> {
    let rpm = read_rpm(&fan.input)?;
    let target = read_rpm(&fan.target).filter(|t| *t > 0);
    let duty = fan.pwm.as_ref().and_then(|(path, full)| {
        let raw: f64 = read_trimmed(path)?.parse().ok()?;
        Some((raw / full * 100.0).clamp(0.0, 100.0))
    });
    Some(FanReading { rpm, target, duty })
}

/// Finds every fan whose speed can be read right now.
pub fn discover(paths: &SysPaths) -> Vec<Fan> {
    let mut fans = Vec::new();
    for index in numbered(&paths.hwmon, "hwmon", "") {
        let dir = paths.hwmon.join(format!("hwmon{index}"));
        let Some(chip) = read_trimmed(&dir.join("name")) else {
            continue;
        };
        let device_dir = fs::canonicalize(dir.join("device")).ok();
        for n in numbered(&dir, "fan", "_input") {
            let input = dir.join(format!("fan{n}_input"));
            if read_rpm(&input).is_none() {
                continue;
            }
            let pwm_path = dir.join(format!("pwm{n}"));
            let pwm = read_trimmed(&pwm_path).and_then(|v| v.parse::<f64>().ok()).map(|_| {
                let full = read_trimmed(&dir.join(format!("pwm{n}_max")))
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|m| *m > 0.0)
                    .unwrap_or(255.0);
                (pwm_path, full)
            });
            fans.push(Fan {
                id: format!(
                    "{chip}/{}/fan{n}",
                    device_key(&chip, &dir, device_dir.as_deref())
                ),
                device: device_name(&chip),
                label: read_trimmed(&dir.join(format!("fan{n}_label")))
                    .unwrap_or_else(|| format!("Fan {n}")),
                rated_max: read_rpm(&dir.join(format!("fan{n}_max"))).filter(|m| *m > 0),
                chip: chip.clone(),
                input,
                target: dir.join(format!("fan{n}_target")),
                pwm,
            });
        }
    }

    let chips: Vec<String> = fans.iter().map(|f| f.chip.clone()).collect();
    fans.retain(|f| {
        !DUPLICATE_SOURCES
            .iter()
            .any(|(dup, preferred)| f.chip == *dup && chips.iter().any(|c| c == preferred))
    });
    fans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsys::FakeSys;

    #[test]
    fn reads_speed_target_and_duty() {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon3/name", "amdgpu\n");
        sys.write("class/hwmon/hwmon3/fan1_input", "1500\n");
        sys.write("class/hwmon/hwmon3/fan1_max", "3000\n");
        sys.write("class/hwmon/hwmon3/fan1_target", "1600\n");
        sys.write("class/hwmon/hwmon3/pwm1", "102\n");
        sys.link_device("hwmon3", "devices/pci/0000:03:00.0");

        let fans = discover(&sys.paths());
        assert_eq!(fans.len(), 1);
        assert_eq!(fans[0].id, "amdgpu/0000:03:00.0/fan1");
        assert_eq!((fans[0].device.as_str(), fans[0].label.as_str()), ("GPU", "Fan 1"));
        assert_eq!(fans[0].rated_max, Some(3000));
        assert_eq!(
            read(&fans[0]),
            Some(FanReading {
                rpm: 1500,
                target: Some(1600),
                duty: Some(40.0)
            })
        );
    }

    #[test]
    fn stopped_fan_is_kept_and_framework_duplicates_are_dropped() {
        let sys = FakeSys::new();
        for hw in ["hwmon6", "hwmon10"] {
            sys.write(&format!("class/hwmon/{hw}/fan1_input"), "0\n");
            sys.write(&format!("class/hwmon/{hw}/fan2_input"), "5173\n");
        }
        sys.write("class/hwmon/hwmon6/name", "cros_ec\n");
        sys.write("class/hwmon/hwmon10/name", "framework_laptop\n");
        sys.write("class/hwmon/hwmon6/fan1_target", "3700\n");

        let fans = discover(&sys.paths());
        let names: Vec<String> = fans.iter().map(|f| format!("{} {}", f.chip, f.label)).collect();
        assert_eq!(names, ["cros_ec Fan 1", "cros_ec Fan 2"]);
        let stopped = read(&fans[0]).unwrap();
        assert_eq!((stopped.rpm, stopped.target, stopped.duty), (0, Some(3700), None));
    }

    #[test]
    fn glitch_readings_are_ignored() {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon1/name", "nct6798\n");
        sys.write("class/hwmon/hwmon1/fan1_input", "65535\n");
        sys.write("class/hwmon/hwmon1/fan2_input", "900\n");
        let fans = discover(&sys.paths());
        assert_eq!(fans.len(), 1);
        assert_eq!(fans[0].device, "Board");
    }

    /// Shows what this machine exposes: `cargo test real_fans -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real /sys of the machine running the test"]
    fn real_fans() {
        for f in discover(&SysPaths::default()) {
            let mut json = serde_json::to_value(&f).unwrap();
            json["reading"] = serde_json::json!(read(&f));
            println!("{json}");
        }
    }
}
