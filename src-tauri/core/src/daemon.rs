//! The fan daemon, `thermite-fand <user>`. systemd starts it as root at boot. It runs the
//! fan settings of one user without the window, so it needs no password dialog.
//!
//! It reads the user's settings file once per second, so a change that the window saves
//! reaches the fans within a second. Only that user and root can write the file, so the
//! file is the permission. Each new status goes to `STATUS`, where the window can read it.

use std::ffi::{CStr, CString, OsStr};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use crate::helper::{self, Controller, Plan, Status, StatusLog};
use crate::sensors::{self, Sensor, SysPaths};
use crate::settings::Settings;
use crate::{INTERVAL, RESCAN_EVERY};

/// The daemon's last status. systemd removes `/run/thermite` when the service stops.
pub const STATUS: &str = "/run/thermite/status.json";

/// Where the window saves the settings, below the user's home directory.
/// The daemon does not follow `XDG_CONFIG_HOME`.
const SETTINGS: &str = ".config/local.thermite/settings.json";

/// Builds the plan from a settings file, again on each poll.
pub struct Follower {
    settings: PathBuf,
    paths: SysPaths,
    /// Every sensor found so far. A sensor that stops reading stays, so its fan gets
    /// full speed. Without it the curve would drop out, and the fan go to automatic mode.
    sensors: Vec<Sensor>,
    polls: u64,
    /// The plan last returned.
    sent: Option<Plan>,
    /// Why the settings could not be read last time, so each reason is logged once.
    error: Option<String>,
}

impl Follower {
    pub fn new(settings: PathBuf, paths: SysPaths) -> Self {
        Self {
            settings,
            paths,
            sensors: Vec::new(),
            polls: 0,
            sent: None,
            error: None,
        }
    }

    /// The plan from the settings file now, when it is not the plan last returned.
    /// Settings that cannot be read give an empty plan, so every fan goes back to
    /// automatic mode. Every `RESCAN_EVERY` polls it also looks for new sensors.
    pub fn poll(&mut self) -> Option<Plan> {
        if self.polls.is_multiple_of(RESCAN_EVERY) {
            for sensor in sensors::discover(&self.paths) {
                if self.sensors.iter().all(|s| s.id != sensor.id) {
                    self.sensors.push(sensor);
                }
            }
        }
        self.polls += 1;
        let settings = match Settings::read(&self.settings) {
            Ok(settings) => {
                self.error = None;
                settings
            }
            Err(e) => {
                if self.error.as_ref() != Some(&e) {
                    eprintln!("thermite-fand: every fan stays in automatic mode: {e}");
                    self.error = Some(e);
                }
                Settings::default()
            }
        };
        let plan = Plan::new(&settings, &self.sensors);
        if self.sent.as_ref() == Some(&plan) {
            return None;
        }
        self.sent = Some(plan.clone());
        Some(plan)
    }
}

/// Replaces the status file in one step, so a reader never sees half a status.
fn write_status(path: &Path, status: &Status) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string(status).expect("status is plain data"))?;
    fs::rename(tmp, path)
}

/// Runs the plan from `settings` until `stop` is set, and keeps `status` up to date.
/// Returns why it stopped early. Every fan is back in automatic mode by then,
/// and the status file is gone.
pub fn run(paths: SysPaths, settings: PathBuf, status: &Path, stop: &AtomicBool) -> Result<(), String> {
    let cannot_write = |e: &io::Error| format!("cannot write {}: {e}", status.display());
    write_status(status, &Status::default()).map_err(|e| cannot_write(&e))?;
    let (plans, received) = mpsc::channel();
    let mut follower = Follower::new(settings, paths.clone());
    let done = AtomicBool::new(false);
    let mut failed = None;
    thread::scope(|scope| {
        let done = &done;
        // The settings reader has its own thread: a slow read must not hold up the failsafe.
        // It owns the sender, so if it panics, `helper::run` returns and gives the fans back.
        scope.spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if let Some(plan) = follower.poll() {
                    if plans.send(plan).is_err() {
                        break;
                    }
                }
                thread::sleep(INTERVAL);
            }
        });
        let mut log = StatusLog::default();
        let report = |s: &Status| {
            if log.worth_a_line(s) {
                eprintln!("thermite-fand: {}", serde_json::to_string(s).expect("status is plain data"));
            }
            write_status(status, s).inspect_err(|e| failed = Some(cannot_write(e)))
        };
        helper::run(Controller::new(paths), &received, report, stop);
        done.store(true, Ordering::Relaxed);
    });
    // The window must not take a status from a daemon that has stopped.
    let _ = fs::remove_file(status);
    match failed {
        Some(e) => Err(e),
        None if stop.load(Ordering::Relaxed) => Ok(()),
        None => Err("the settings reader stopped".to_string()),
    }
}

/// The home directory of `user`, from the system's user database.
fn home_of(user: &str) -> Option<PathBuf> {
    let name = CString::new(user).ok()?;
    // SAFETY: `passwd` is plain data; all zeros is a valid value for it.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf: Vec<libc::c_char> = vec![0; 16 * 1024];
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and `buf` has the length given.
    let rc = unsafe { libc::getpwnam_r(name.as_ptr(), &mut entry, buf.as_mut_ptr(), buf.len(), &mut found) };
    if rc != 0 || found.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: on success, `pw_dir` is a C string in `buf`, which is still alive.
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    Some(PathBuf::from(OsStr::from_bytes(dir.to_bytes())))
}

/// Entry point of `thermite-fand <user>`.
pub fn main(user: &str) -> ExitCode {
    let stop = helper::stop_on_signals();
    let Some(home) = home_of(user) else {
        eprintln!("thermite-fand: no user named {user}");
        return ExitCode::FAILURE;
    };
    let settings = home.join(SETTINGS);
    eprintln!("thermite-fand: fan settings from {}", settings.display());
    let status = Path::new(STATUS);
    // systemd makes the directory. This is for a start by hand.
    if let Some(dir) = status.parent() {
        if let Err(e) = fs::create_dir_all(dir) {
            eprintln!("thermite-fand: cannot make {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    }
    match run(SysPaths::default(), settings, status, stop) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("thermite-fand: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::curve::{CurvePoint, FanCurve};
    use crate::testsys::FakeSys;

    const FAN: &str = "nct6798/nct6798/fan1";
    const SENSOR: &str = "nct6798/nct6798/temp1";

    /// A desktop board chip that follows the hwmon ABI. Its firmware curve is mode 5.
    fn board(temp_milli: u32) -> FakeSys {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon2/name", "nct6798\n");
        sys.write("class/hwmon/hwmon2/temp1_input", &format!("{temp_milli}\n"));
        sys.write("class/hwmon/hwmon2/fan1_input", "900\n");
        sys.write("class/hwmon/hwmon2/pwm1", "100\n");
        sys.write("class/hwmon/hwmon2/pwm1_enable", "5\n");
        sys
    }

    /// Saves settings with one curve for `FAN`, from `sensor`, as the window does.
    fn save_curve(sys: &FakeSys, sensor: &str, points: &[(f64, f64)]) -> PathBuf {
        let mut settings = Settings::default();
        let curve = FanCurve {
            sensor: sensor.to_string(),
            points: points.iter().map(|&(temp, duty)| CurvePoint { temp, duty }).collect(),
        };
        settings.fan_curves.insert(FAN.to_string(), curve);
        let path = sys.path("home/.config/local.thermite/settings.json");
        settings.save(&path).unwrap();
        path
    }

    fn planned_fans(plan: &Plan) -> Vec<&str> {
        plan.fans.iter().map(|f| f.fan.as_str()).collect()
    }

    #[test]
    fn follower_builds_the_plan_once_per_change() {
        let sys = board(60_000);
        let path = save_curve(&sys, SENSOR, &[(40.0, 20.0), (80.0, 100.0)]);
        let mut follower = Follower::new(path, sys.paths());
        let first = follower.poll().unwrap();
        assert_eq!((planned_fans(&first), follower.poll()), (vec![FAN], None));
    }

    #[test]
    fn follower_takes_a_changed_setting() {
        let sys = board(60_000);
        let path = save_curve(&sys, SENSOR, &[(40.0, 20.0), (80.0, 100.0)]);
        let mut follower = Follower::new(path, sys.paths());
        follower.poll();
        // A fixed level of 40% is a curve with one point.
        save_curve(&sys, SENSOR, &[(50.0, 40.0)]);
        let points = follower.poll().unwrap().fans[0].curve.points.clone();
        assert_eq!(points, [CurvePoint { temp: 50.0, duty: 40.0 }]);
    }

    #[test]
    fn unreadable_settings_give_every_fan_back() {
        let sys = board(60_000);
        let path = save_curve(&sys, SENSOR, &[(40.0, 20.0), (80.0, 100.0)]);
        let mut follower = Follower::new(path.clone(), sys.paths());
        follower.poll();
        fs::write(&path, "{ half a file").unwrap();
        assert_eq!(follower.poll(), Some(Plan::default()));
    }

    #[test]
    fn sensor_that_stops_reading_keeps_its_fan_in_the_plan() {
        let sys = board(60_000);
        let path = save_curve(&sys, SENSOR, &[(40.0, 20.0), (80.0, 100.0)]);
        let mut follower = Follower::new(path, sys.paths());
        follower.poll();
        // The rescan skips sensors that do not read.
        sys.write("class/hwmon/hwmon2/temp1_input", "garbage\n");
        for _ in 0..RESCAN_EVERY {
            follower.poll();
        }
        save_curve(&sys, SENSOR, &[(50.0, 40.0)]);
        assert_eq!(planned_fans(&follower.poll().unwrap()), [FAN]);
    }

    #[test]
    fn sensor_that_appears_late_joins_the_plan_at_the_rescan() {
        let sys = board(60_000);
        let path = save_curve(&sys, "late/late/temp1", &[(40.0, 20.0), (80.0, 100.0)]);
        let mut follower = Follower::new(path, sys.paths());
        assert_eq!(follower.poll(), Some(Plan::default()));
        sys.write("class/hwmon/hwmon3/name", "late\n");
        sys.write("class/hwmon/hwmon3/temp1_input", "50000\n");
        for _ in 1..RESCAN_EVERY {
            assert_eq!(follower.poll(), None);
        }
        assert_eq!(planned_fans(&follower.poll().unwrap()), [FAN]);
    }

    /// Waits up to 10 seconds for `done`.
    fn wait_for(done: impl Fn() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn daemon_runs_the_settings_and_gives_the_fans_back_when_stopped() {
        let sys = board(60_000);
        let path = save_curve(&sys, SENSOR, &[(40.0, 20.0), (80.0, 100.0)]);
        sys.write("run/thermite/.keep", "");
        let status = sys.path("run/thermite/status.json");
        let stop = AtomicBool::new(false);
        let (set, result) = thread::scope(|scope| {
            let daemon = scope.spawn(|| run(sys.paths(), path, &status, &stop));
            // 60% of the ABI's full scale of 255.
            let set = wait_for(|| sys.read("class/hwmon/hwmon2/pwm1") == "153");
            let reported = wait_for(|| fs::read_to_string(&status).unwrap_or_default().contains(r#""duty":60.0"#));
            stop.store(true, Ordering::Relaxed);
            (set && reported, daemon.join().unwrap())
        });
        assert_eq!(
            (set, result, sys.read("class/hwmon/hwmon2/pwm1_enable"), status.exists()),
            (true, Ok(()), "5".to_string(), false)
        );
    }

    #[test]
    fn root_has_a_home() {
        assert_eq!(home_of("root"), Some(PathBuf::from("/root")));
        assert_eq!(home_of("no-such-user-thermite"), None);
    }
}
