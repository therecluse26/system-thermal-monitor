//! Sets fan speeds. One `FanControl` variant per kind of hardware interface.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::sensors::read_trimmed;

/// What a driver does with a write to `pwmN_enable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnableFile {
    /// As the hwmon ABI says: "1" selects manual mode, "2" and above automatic mode.
    Abi,
    /// Any write selects automatic mode. A write to `pwmN` alone selects manual mode.
    AutoOnly,
}

/// Drivers whose `pwmN_enable` is `EnableFile::AutoOnly`.
const AUTO_ONLY_ENABLE: &[&str] = &["framework_laptop"];

/// Programs that set fan speeds themselves. Two controllers on one fan fight each other.
const OTHER_CONTROLLERS: &[&str] = &[
    "fw-fanctrl",
    "fancontrol",
    "thinkfan",
    "coolercontrold",
    "fan2go",
    "nbfc_service",
];

/// The name of another fan controller that runs now, if any.
pub fn other_controller(proc: &Path) -> Option<String> {
    fs::read_dir(proc).ok()?.flatten().find_map(|entry| {
        let pid = entry.file_name();
        if !pid.to_str()?.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let cmdline = fs::read(entry.path().join("cmdline")).ok()?;
        let mut args = cmdline
            .split(|b| *b == 0)
            .filter_map(|arg| program_name(std::str::from_utf8(arg).ok()?));
        let program = args.next()?;
        // fw-fanctrl and fancontrol are scripts: the interpreter runs first.
        let name = if program.starts_with("python") || INTERPRETERS.contains(&program.as_str()) {
            args.next()?
        } else {
            program
        };
        OTHER_CONTROLLERS.contains(&name.as_str()).then_some(name)
    })
}

/// Besides "python3.13" and the like.
const INTERPRETERS: &[&str] = &["bash", "sh", "perl"];

/// "/nix/store/…/bin/.fw-fanctrl-wrapped" → "fw-fanctrl". Nix wraps programs that way.
fn program_name(path: &str) -> Option<String> {
    let name = Path::new(path).file_name()?.to_str()?;
    let name = name.strip_prefix('.').unwrap_or(name);
    Some(name.strip_suffix("-wrapped").unwrap_or(name).to_string())
}

/// How the app sets the speed of one fan.
#[derive(Debug, Clone, PartialEq)]
pub enum FanControl {
    /// hwmon `pwmN` and `pwmN_enable`.
    Hwmon {
        pwm: PathBuf,
        enable: PathBuf,
        enable_file: EnableFile,
        /// The `pwmN` value for 100%.
        full: f64,
    },
}

/// The `pwmN` value for 100%: `pwmN_max` when the driver has it, else the ABI's 255.
pub fn pwm_full(dir: &Path, n: u32) -> f64 {
    read_trimmed(&dir.join(format!("pwm{n}_max")))
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|m| *m > 0.0)
        .unwrap_or(255.0)
}

impl FanControl {
    /// The control of hwmon fan `n`, when the driver has the files for it.
    /// Without `pwmN_enable` the app could not give the fan back, so it does not take it.
    pub fn hwmon(dir: &Path, chip: &str, n: u32) -> Option<Self> {
        let pwm = dir.join(format!("pwm{n}"));
        let enable = dir.join(format!("pwm{n}_enable"));
        let enable_file = if AUTO_ONLY_ENABLE.contains(&chip) {
            EnableFile::AutoOnly
        } else {
            EnableFile::Abi
        };
        (pwm.exists() && enable.exists()).then(|| Self::Hwmon {
            pwm,
            enable,
            enable_file,
            full: pwm_full(dir, n),
        })
    }

    /// Puts the fan in manual mode. Returns the automatic mode to restore later,
    /// when the driver lets us read it.
    pub fn hold(&self) -> io::Result<Option<String>> {
        let Self::Hwmon { enable, enable_file, .. } = self;
        if *enable_file == EnableFile::AutoOnly {
            return Ok(None);
        }
        // "1" means another program left the fan in manual mode; do not restore that.
        let restore = read_trimmed(enable).filter(|mode| mode != "1");
        fs::write(enable, "1")?;
        Ok(restore)
    }

    /// Gives the fan back to its automatic mode: `restore` from `hold`, else the ABI's "2".
    pub fn release(&self, restore: Option<&str>) -> io::Result<()> {
        let Self::Hwmon { enable, .. } = self;
        fs::write(enable, restore.unwrap_or("2"))
    }

    /// Sets the PWM duty, in percent.
    pub fn set(&self, duty: f64) -> io::Result<()> {
        let Self::Hwmon { pwm, full, .. } = self;
        let raw = (duty.clamp(0.0, 100.0) / 100.0 * full).round();
        fs::write(pwm, format!("{raw}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsys::FakeSys;

    #[test]
    fn finds_fw_fanctrl_behind_its_nix_wrapper() {
        let sys = FakeSys::new();
        sys.write("proc/1/cmdline", "/run/current-system/systemd/lib/systemd/systemd\0");
        sys.write(
            "proc/3331634/cmdline",
            "/nix/store/x-python3-3.13.13/bin/python3.13\0\
             /nix/store/y-python3.13-fw-fanctrl-1.0.4/bin/.fw-fanctrl-wrapped\0\
             --output-format\0JSON\0run\0",
        );
        assert_eq!(other_controller(&sys.paths().proc).as_deref(), Some("fw-fanctrl"));
    }

    #[test]
    fn an_editor_that_opens_a_controller_script_is_not_a_controller() {
        let sys = FakeSys::new();
        sys.write("proc/4242/cmdline", "vim\0/usr/sbin/fancontrol\0");
        assert_eq!(other_controller(&sys.paths().proc), None);
    }
}
