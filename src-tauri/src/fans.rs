//! Finds and reads fan speeds that the kernel exposes through hwmon.
//! NVIDIA GPU fans come from `nvidia.rs` instead, as the same `Fan` type.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::control::{self, FanControl};
use crate::sensors::{device_key, id_chip, legacy_device_key, make_unique, numbered, read_trimmed, SysPaths};

/// Drivers that report the same physical fans as another driver: (duplicate, preferred).
/// The duplicate's fans are skipped when the preferred driver has fans.
/// When the duplicate owns the PWM files, the preferred fan of the same number gets them.
const DUPLICATE_SOURCES: &[(&str, &str)] = &[("framework_laptop", "cros_ec")];

/// A reading above this is a glitch, not a fan.
const MAX_PLAUSIBLE_RPM: u32 = 30_000;

/// Where a fan's reading comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum FanInput {
    Hwmon {
        input: PathBuf,
        target: PathBuf,
        /// `pwmN` and its full-scale value, when the file is readable.
        pwm: Option<(PathBuf, f64)>,
    },
    /// An NVIDIA GPU fan, by NVML device index and fan index.
    Nvml { gpu: u32, fan: u32 },
}

/// What the `share` of a reading measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareKind {
    /// PWM duty cycle: the share of full power sent to the fan.
    Pwm,
    /// The driver's own figure for the share of maximum speed.
    Driver,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fan {
    /// Stable across reboots where the hardware allows it. Settings are keyed by it.
    pub id: String,
    /// The id that earlier versions of the app used, so saved settings can move to `id`.
    #[serde(skip)]
    pub legacy_id: String,
    pub chip: String,
    /// Short name of what the fan belongs to, e.g. "System" or "GPU".
    pub device: String,
    /// e.g. "Fan 1", or the driver's own label.
    pub label: String,
    /// Rated maximum speed from `fanN_max`, when the hardware reports one.
    pub rated_max: Option<u32>,
    /// What `FanReading::share` measures, when this fan reports one.
    pub share_kind: Option<ShareKind>,
    /// Whether the app can set this fan's speed.
    pub controllable: bool,
    #[serde(skip)]
    pub input: FanInput,
    /// How to set the fan's speed, when the driver allows it.
    #[serde(skip)]
    pub control: Option<FanControl>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanReading {
    /// `None` when the hardware reports only a share of maximum speed.
    pub rpm: Option<u32>,
    /// Speed the controller asks for, when it reports one.
    pub target: Option<u32>,
    /// Share of maximum in percent; `Fan::share_kind` says what kind.
    pub share: Option<f64>,
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

/// Current speed of a hwmon fan, or `None` when it cannot be read right now.
pub fn read_hwmon(input: &FanInput) -> Option<FanReading> {
    let FanInput::Hwmon { input, target, pwm } = input else {
        return None;
    };
    let rpm = read_rpm(input)?;
    let target = read_rpm(target).filter(|t| *t > 0);
    let share = pwm.as_ref().and_then(|(path, full)| {
        let raw: f64 = read_trimmed(path)?.parse().ok()?;
        Some((raw / full * 100.0).clamp(0.0, 100.0))
    });
    Some(FanReading {
        rpm: Some(rpm),
        target,
        share,
    })
}

/// "fan1_input": two drivers that report the same fans give one fan the same name.
fn input_name(fan: &Fan) -> Option<&OsStr> {
    match &fan.input {
        FanInput::Hwmon { input, .. } => input.file_name(),
        FanInput::Nvml { .. } => None,
    }
}

/// Finds every hwmon fan whose speed can be read right now.
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
            let pwm = read_trimmed(&pwm_path)
                .and_then(|v| v.parse::<f64>().ok())
                .map(|_| (pwm_path, control::pwm_full(&dir, n)));
            fans.push(Fan {
                id: format!(
                    "{}/{}/fan{n}",
                    id_chip(&chip),
                    device_key(&chip, device_dir.as_deref())
                ),
                legacy_id: format!(
                    "{chip}/{}/fan{n}",
                    legacy_device_key(&chip, &dir, device_dir.as_deref())
                ),
                device: device_name(&chip),
                label: read_trimmed(&dir.join(format!("fan{n}_label")))
                    .unwrap_or_else(|| format!("Fan {n}")),
                rated_max: read_rpm(&dir.join(format!("fan{n}_max"))).filter(|m| *m > 0),
                share_kind: pwm.is_some().then_some(ShareKind::Pwm),
                controllable: false,
                chip: chip.clone(),
                input: FanInput::Hwmon {
                    input,
                    target: dir.join(format!("fan{n}_target")),
                    pwm,
                },
                control: FanControl::hwmon(&dir, &chip, n),
            });
        }
    }

    let chips: Vec<String> = fans.iter().map(|f| f.chip.clone()).collect();
    let (duplicates, mut fans): (Vec<Fan>, Vec<Fan>) = fans.into_iter().partition(|f| {
        DUPLICATE_SOURCES
            .iter()
            .any(|(dup, preferred)| f.chip == *dup && chips.iter().any(|c| c == preferred))
    });
    for dup in duplicates {
        let same_fan = fans.iter_mut().find(|f| {
            DUPLICATE_SOURCES.iter().any(|(d, p)| *d == dup.chip && *p == f.chip)
                && input_name(f).is_some()
                && input_name(f) == input_name(&dup)
        });
        if let Some(fan) = same_fan {
            fan.control = fan.control.take().or(dup.control);
        }
    }
    for fan in &mut fans {
        fan.controllable = fan.control.is_some();
    }
    make_unique(&mut fans, |f| (&mut f.id, &f.legacy_id));
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
        assert_eq!(fans[0].share_kind, Some(ShareKind::Pwm));
        assert_eq!(
            read_hwmon(&fans[0].input),
            Some(FanReading {
                rpm: Some(1500),
                target: Some(1600),
                share: Some(40.0)
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
        sys.link_device("hwmon6", "devices/platform/cros-ec-dev.0.auto/cros-ec-hwmon.6.auto");

        let fans = discover(&sys.paths());
        let names: Vec<String> = fans.iter().map(|f| format!("{} {}", f.chip, f.label)).collect();
        assert_eq!(names, ["cros_ec Fan 1", "cros_ec Fan 2"]);
        assert_eq!(fans[0].id, "cros_ec/cros-ec-hwmon/fan1");
        assert_eq!(fans[0].legacy_id, "cros_ec/cros-ec-hwmon.6.auto/fan1");
        let stopped = read_hwmon(&fans[0].input).unwrap();
        assert_eq!((stopped.rpm, stopped.target, stopped.share), (Some(0), Some(3700), None));
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
            json["reading"] = serde_json::json!(read_hwmon(&f.input));
            println!("{json}");
        }
    }
}
