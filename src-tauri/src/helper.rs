//! The fan helper: a small process that runs as root and drives fans by their fan curves.
//!
//! The window starts it through pkexec and sends it a plan over a pipe. The helper gives
//! every fan back to its automatic mode when the pipe closes, so a closed or crashed
//! window cannot leave a fan at a low duty.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::control::{self, FanControl};
use crate::curve::FanCurve;
use crate::fans;
use crate::monitor::{INTERVAL, RESCAN_EVERY};
use crate::sensors::{self, Input, Sensor, SysPaths};
use crate::settings::Settings;

/// One fan and the curve that drives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanPlan {
    pub fan: String,
    pub curve: FanCurve,
    /// °C. At or above it, the fan runs at full speed whatever the curve says.
    pub failsafe: f64,
}

/// Every fan the helper must drive. Fans that are not in it stay in automatic mode.
/// It names fans and sensors by id only: the helper writes only to files it found itself.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub fans: Vec<FanPlan>,
}

impl Plan {
    /// The plan for the saved fan curves. Each failsafe is the "Throttling likely at"
    /// temperature of the curve's sensor, the custom one when the user set it.
    /// A curve whose sensor is not found drops out, so its fan stays in automatic mode.
    pub fn new(settings: &Settings, sensors: &[Sensor]) -> Self {
        let fans = settings.fan_curves.iter().filter_map(|(fan, curve)| {
            let sensor = sensors.iter().find(|s| s.id == curve.sensor)?;
            let failsafe = settings
                .overrides
                .get(&sensor.id)
                .and_then(|o| o.throttle)
                .unwrap_or(sensor.thresholds.throttle);
            Some(FanPlan {
                fan: fan.clone(),
                curve: curve.clone(),
                failsafe,
            })
        });
        Self { fans: fans.collect() }
    }
}

/// What happened to one fan of the plan in the last tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanStatus {
    pub fan: String,
    /// The duty set, in percent. `None` when the fan did not take it.
    pub duty: Option<f64>,
    /// Full speed, because the sensor reached the failsafe temperature or stopped reading.
    pub failsafe: bool,
    /// Why the fan did not take the duty.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Another fan controller that runs now. While it runs, the helper sets no fan.
    pub blocked_by: Option<String>,
    pub fans: Vec<FanStatus>,
}

/// A fan in manual mode, and the automatic mode to give back.
struct Held {
    control: FanControl,
    restore: Option<String>,
    /// The duty last set, in percent.
    duty: Option<f64>,
}

/// Applies a plan to the hardware, one tick at a time.
/// Dropping it gives every fan back to its automatic mode, also when a panic unwinds.
pub struct Controller {
    paths: SysPaths,
    plan: Plan,
    /// Input file of each hwmon sensor, by sensor id.
    sensors: HashMap<String, PathBuf>,
    /// Each fan whose speed the app can set, by fan id.
    fans: HashMap<String, FanControl>,
    /// Keyed by fan id.
    held: BTreeMap<String, Held>,
    blocked_by: Option<String>,
    ticks: u64,
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.release_unless(|_| false);
    }
}

impl Controller {
    pub fn new(paths: SysPaths) -> Self {
        Self {
            paths,
            plan: Plan::default(),
            sensors: HashMap::new(),
            fans: HashMap::new(),
            held: BTreeMap::new(),
            blocked_by: None,
            ticks: 0,
        }
    }

    pub fn set_plan(&mut self, mut plan: Plan) {
        for fan_plan in &mut plan.fans {
            fan_plan.curve.points.sort_by(|a, b| a.temp.total_cmp(&b.temp));
        }
        self.release_unless(|id| plan.fans.iter().any(|f| f.fan == id));
        self.plan = plan;
        self.discover();
    }

    /// Gives every held fan that `keep` rejects back to its automatic mode.
    fn release_unless(&mut self, keep: impl Fn(&str) -> bool) {
        self.held.retain(|id, held| {
            if keep(id) {
                return true;
            }
            if let Err(e) = held.control.release(held.restore.as_deref()) {
                eprintln!("thermite: could not give {id} back to automatic mode: {e}");
            }
            false
        });
    }

    /// Finds new sensors and fans, and any other fan controller. Ids come from the same
    /// code the window uses. Known ones stay even when a rescan skips them because they
    /// do not read: a sensor that stops reading must give full speed, not be forgotten.
    fn discover(&mut self) {
        self.blocked_by = control::other_controller(&self.paths.proc);
        let found_sensors = sensors::discover(&self.paths)
            .into_iter()
            .filter_map(|s| match s.input {
                Input::Hwmon(path) => Some((s.id, path)),
                Input::Nvml(_) => None,
            });
        self.sensors.extend(found_sensors);
        let found_fans = fans::discover(&self.paths)
            .into_iter()
            .filter_map(|f| Some((f.id, f.control?)));
        self.fans.extend(found_fans);
    }

    /// Reads each curve's sensor and sets each fan's duty.
    pub fn tick(&mut self) -> Status {
        if self.ticks > 0 && self.ticks.is_multiple_of(RESCAN_EVERY) {
            self.discover();
        }
        self.ticks += 1;
        if let Some(other) = &self.blocked_by {
            let blocked_by = Some(other.clone());
            self.release_unless(|_| false);
            return Status {
                blocked_by,
                fans: Vec::new(),
            };
        }

        let plan = std::mem::take(&mut self.plan);
        let fans = plan.fans.iter().map(|fan_plan| {
            let mut status = FanStatus {
                fan: fan_plan.fan.clone(),
                duty: None,
                failsafe: false,
                error: None,
            };
            match self.apply(fan_plan) {
                Ok((duty, failsafe)) => (status.duty, status.failsafe) = (Some(duty), failsafe),
                Err(e) => status.error = Some(e),
            }
            status
        });
        let status = Status {
            blocked_by: None,
            fans: fans.collect(),
        };
        self.plan = plan;
        status
    }

    /// Sets one fan's duty. Returns the duty and whether the failsafe set it.
    fn apply(&mut self, fan_plan: &FanPlan) -> Result<(f64, bool), String> {
        let id = &fan_plan.fan;
        let control = self
            .fans
            .get(id)
            .ok_or_else(|| format!("fan {id} not found, or its speed cannot be set"))?;
        let input = self
            .sensors
            .get(&fan_plan.curve.sensor)
            .ok_or_else(|| format!("sensor {} not found", fan_plan.curve.sensor))?;
        if !self.held.contains_key(id) {
            let restore = control
                .hold()
                .map_err(|e| format!("could not put {id} in manual mode: {e}"))?;
            let held = Held {
                control: control.clone(),
                restore,
                duty: None,
            };
            self.held.insert(id.clone(), held);
        }
        let held = self.held.get_mut(id).expect("held above");
        // A sensor that stops reading could hide a fire.
        let (duty, failsafe) = match sensors::read_hwmon(input) {
            Some(temp) if temp < fan_plan.failsafe => (fan_plan.curve.next_duty(temp, held.duty), false),
            _ => (100.0, true),
        };
        held.control
            .set(duty)
            .map_err(|e| format!("could not set the duty of {id}: {e}"))?;
        held.duty = Some(duty);
        Ok((duty, failsafe))
    }
}

/// Runs the fan helper. Reads plans from `input`, one JSON line each, and writes each
/// new status to `output` as one JSON line. Returns when `input` ends or `stop` is set.
/// The controller is then dropped, which gives every fan back to its automatic mode.
pub fn serve(
    mut controller: Controller,
    input: impl BufRead + Send + 'static,
    mut output: impl Write,
    stop: &AtomicBool,
) {
    let (plans, received) = mpsc::channel();
    thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Plan>(&line) {
                Ok(plan) => {
                    if plans.send(plan).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("thermite: fan helper ignored a plan it could not read: {e}"),
            }
        }
    });

    // Before the first plan there is nothing to report.
    let mut last = Some(Status::default());
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match received.recv_timeout(next.saturating_duration_since(Instant::now())) {
            Ok(plan) => controller.set_plan(plan),
            Err(RecvTimeoutError::Timeout) => {}
            // The window closed the pipe, or died.
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let status = controller.tick();
        next = Instant::now() + INTERVAL;
        if last.as_ref() != Some(&status) {
            let line = serde_json::to_string(&status).expect("status is plain data");
            if writeln!(output, "{line}").and_then(|()| output.flush()).is_err() {
                break;
            }
            last = Some(status);
        }
    }
}

/// The command-line flag that makes the binary run as the fan helper.
pub const HELPER_FLAG: &str = "--fan-helper";

/// Entry point of the fan helper, which the window starts as root through pkexec.
pub fn main() {
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }
    // Logout, shutdown and Ctrl+C in a terminal end the helper through `serve`,
    // so the fans go back to automatic mode first.
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
        unsafe { libc::signal(signal, on_signal as *const () as libc::sighandler_t) };
    }
    let input = BufReader::new(io::stdin());
    serve(Controller::new(SysPaths::default()), input, io::stdout(), &STOP);
}

/// What the UI shows about fan control.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    /// Why the fans with curves do not follow them, in a few words. `None` while they do,
    /// and while no fan has a curve.
    pub notice: Option<String>,
    /// The fan helper's last status.
    pub status: Status,
}

/// What the window's link shares with the thread that reads the helper's output.
#[derive(Default)]
struct Shared {
    /// Bumps each time the window starts or stops a helper. A reader thread changes
    /// the state only while its generation is current, so an old helper cannot.
    generation: u64,
    running: bool,
    state: ControlState,
}

type OnChange = Arc<dyn Fn(&ControlState) + Send + Sync>;

fn update(shared: &mut Shared, state: ControlState, on_change: &OnChange) {
    if shared.state != state {
        shared.state = state;
        on_change(&shared.state);
    }
}

fn notice(text: impl Into<String>) -> ControlState {
    ControlState {
        notice: Some(text.into()),
        status: Status::default(),
    }
}

/// The window's end of the pipe to the fan helper.
pub struct Link {
    stdin: Option<ChildStdin>,
    /// The plan last sent. An unchanged plan is not sent again,
    /// so saving other settings does not show the password dialog again.
    sent: Option<Plan>,
    shared: Arc<Mutex<Shared>>,
    on_change: OnChange,
}

impl Link {
    /// `on_change` gets each new state, for the UI.
    pub fn new(on_change: impl Fn(&ControlState) + Send + Sync + 'static) -> Self {
        Self {
            stdin: None,
            sent: None,
            shared: Arc::default(),
            on_change: Arc::new(on_change),
        }
    }

    pub fn state(&self) -> ControlState {
        self.shared.lock().unwrap().state.clone()
    }

    /// Sends `plan` to the fan helper when it changed. When the helper does not run,
    /// this starts it through pkexec, which shows the desktop's password dialog.
    /// `retry` also starts it for an unchanged plan: after a cancelled dialog, or a block.
    pub fn apply(&mut self, plan: Plan, proc: &Path, retry: bool) {
        let running = self.shared.lock().unwrap().running;
        if self.sent.as_ref() == Some(&plan) && (running || !retry) {
            return;
        }
        self.sent = Some(plan.clone());
        if running && !plan.fans.is_empty() {
            if let Some(stdin) = &mut self.stdin {
                if send(stdin, &plan).is_ok() {
                    return;
                }
            }
        }
        self.stop();
        if plan.fans.is_empty() {
            return;
        }
        if let Some(other) = control::other_controller(proc) {
            let mut shared = self.shared.lock().unwrap();
            update(&mut shared, notice(format!("{other} controls the fans")), &self.on_change);
            return;
        }
        self.start(&plan);
    }

    /// Closes the pipe, so the helper gives every fan back and exits.
    fn stop(&mut self) {
        self.stdin = None;
        let mut shared = self.shared.lock().unwrap();
        shared.generation += 1;
        shared.running = false;
        update(&mut shared, ControlState::default(), &self.on_change);
    }

    /// Starts this binary as the fan helper, as root through pkexec, and sends it `plan`.
    fn start(&mut self, plan: &Plan) {
        let generation = {
            let mut shared = self.shared.lock().unwrap();
            shared.generation += 1;
            shared.running = true;
            update(&mut shared, notice("Waiting for your password"), &self.on_change);
            shared.generation
        };
        match spawn() {
            Ok((mut stdin, stdout, child)) => {
                // If the helper is already gone, `watch` reports why.
                let _ = send(&mut stdin, plan);
                self.stdin = Some(stdin);
                let shared = Arc::clone(&self.shared);
                let on_change = Arc::clone(&self.on_change);
                thread::spawn(move || watch(stdout, child, generation, &shared, &on_change));
            }
            Err(e) => {
                eprintln!("thermite: could not start the fan helper: {e}");
                let mut shared = self.shared.lock().unwrap();
                shared.running = false;
                update(&mut shared, notice(format!("Could not start the fan helper: {e}")), &self.on_change);
            }
        }
    }
}

fn send(stdin: &mut ChildStdin, plan: &Plan) -> io::Result<()> {
    let line = serde_json::to_string(plan).expect("plan is plain data");
    writeln!(stdin, "{line}")?;
    stdin.flush()
}

fn spawn() -> io::Result<(ChildStdin, ChildStdout, Child)> {
    // Root cannot read the files inside the user's AppImage mount, so run the AppImage file.
    let program = match env::var_os("APPIMAGE") {
        Some(appimage) => PathBuf::from(appimage),
        None => env::current_exe()?,
    };
    let mut child = Command::new("pkexec")
        .arg(program)
        .arg(HELPER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    Ok((stdin, stdout, child))
}

/// Follows one helper's output until it exits, and keeps the shared state up to date.
fn watch(stdout: ChildStdout, mut child: Child, generation: u64, shared: &Mutex<Shared>, on_change: &OnChange) {
    let mut logged = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(status) = serde_json::from_str::<Status>(&line) else {
            continue;
        };
        log_status(&line, &status, &mut logged);
        let mut shared = shared.lock().unwrap();
        if shared.generation == generation {
            let notice = status.blocked_by.as_ref().map(|other| format!("{other} controls the fans"));
            update(&mut shared, ControlState { notice, status }, on_change);
        }
    }
    let exit = child.wait();
    let mut shared = shared.lock().unwrap();
    if shared.generation != generation {
        return;
    }
    shared.running = false;
    let reason = match exit {
        // pkexec: 126 when the dialog was dismissed, 127 when not authorized.
        Ok(exit) if matches!(exit.code(), Some(126 | 127)) => "Password dialog cancelled".to_string(),
        Ok(exit) => format!("Fan helper stopped ({exit})"),
        Err(e) => format!("Fan helper stopped: {e}"),
    };
    eprintln!("thermite: fan curves are off: {reason}");
    update(&mut shared, notice(reason), on_change);
}

/// The parts of a status worth a log line: a fan taken or lost, a failsafe, an error,
/// or another fan controller. Duty changes are not.
type LogKey = (Option<String>, Vec<(String, bool, bool, Option<String>)>);

fn log_status(line: &str, status: &Status, logged: &mut Option<LogKey>) {
    let fans = status
        .fans
        .iter()
        .map(|f| (f.fan.clone(), f.duty.is_some(), f.failsafe, f.error.clone()))
        .collect();
    let key = Some((status.blocked_by.clone(), fans));
    if key != *logged {
        eprintln!("thermite: fan helper: {line}");
        *logged = key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::AtomicBool;

    use crate::curve::{CurvePoint, FanCurve};
    use crate::settings::Override;
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

    /// 20% at 40 °C, rising to 100% at 80 °C: 2% per °C.
    fn plan(fan: &str, sensor: &str) -> Plan {
        Plan {
            fans: vec![FanPlan {
                fan: fan.to_string(),
                curve: FanCurve {
                    sensor: sensor.to_string(),
                    points: vec![
                        CurvePoint { temp: 40.0, duty: 20.0 },
                        CurvePoint { temp: 80.0, duty: 100.0 },
                    ],
                },
                failsafe: 95.0,
            }],
        }
    }

    #[test]
    fn curve_sets_the_duty_in_manual_mode() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        controller.tick();
        // 60% of the ABI's full scale of 255.
        assert_eq!(
            (sys.read("class/hwmon/hwmon2/pwm1_enable"), sys.read("class/hwmon/hwmon2/pwm1")),
            ("1".to_string(), "153".to_string())
        );
    }

    /// A Framework laptop: `cros_ec` reports the fans, `framework_laptop` reports them
    /// again and owns the PWM files. Its `pwm1_enable` hands control back to the EC.
    fn framework(temp_milli: u32) -> FakeSys {
        let sys = FakeSys::new();
        sys.write("class/hwmon/hwmon6/name", "cros_ec\n");
        sys.write("class/hwmon/hwmon6/temp4_input", &format!("{temp_milli}\n"));
        sys.write("class/hwmon/hwmon6/temp4_label", "cpu@4c\n");
        sys.write("class/hwmon/hwmon6/fan1_input", "3000\n");
        sys.link_device("hwmon6", "devices/platform/cros-ec-dev.0.auto/cros-ec-hwmon.6.auto");
        sys.write("class/hwmon/hwmon10/name", "framework_laptop\n");
        sys.write("class/hwmon/hwmon10/fan1_input", "3000\n");
        sys.write("class/hwmon/hwmon10/pwm1", "");
        sys.write("class/hwmon/hwmon10/pwm1_enable", "untouched");
        sys.write("class/hwmon/hwmon10/pwm1_max", "100\n");
        sys
    }

    const FRAMEWORK_FAN: &str = "cros_ec/cros-ec-hwmon/fan1";
    const FRAMEWORK_SENSOR: &str = "cros_ec/cros-ec-hwmon/temp4";

    #[test]
    fn framework_fan_takes_manual_mode_from_the_duty_write_alone() {
        let sys = framework(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FRAMEWORK_FAN, FRAMEWORK_SENSOR));
        controller.tick();
        assert_eq!(
            (sys.read("class/hwmon/hwmon10/pwm1_enable"), sys.read("class/hwmon/hwmon10/pwm1")),
            ("untouched".to_string(), "60".to_string())
        );
    }

    #[test]
    fn dropping_the_controller_gives_a_framework_fan_back_to_the_ec() {
        let sys = framework(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FRAMEWORK_FAN, FRAMEWORK_SENSOR));
        controller.tick();
        drop(controller);
        assert_eq!(sys.read("class/hwmon/hwmon10/pwm1_enable"), "2");
    }

    #[test]
    fn failsafe_temperature_forces_full_speed() {
        let sys = board(70_000);
        let mut controller = Controller::new(sys.paths());
        let mut p = plan(FAN, SENSOR);
        p.fans[0].failsafe = 70.0;
        controller.set_plan(p);
        controller.tick();
        // The curve alone says 80%.
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1"), "255");
    }

    #[test]
    fn sensor_that_stops_reading_forces_full_speed() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        controller.tick();
        sys.write("class/hwmon/hwmon2/temp1_input", "garbage\n");
        controller.tick();
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1"), "255");
    }

    /// Ticks once per temperature and returns what `pwm1` holds after the last one.
    fn board_duty_after(temps_milli: &[u32]) -> String {
        let sys = board(temps_milli[0]);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        for t in temps_milli {
            sys.write("class/hwmon/hwmon2/temp1_input", &format!("{t}\n"));
            controller.tick();
        }
        sys.read("class/hwmon/hwmon2/pwm1")
    }

    #[test]
    fn duty_holds_while_the_temperature_dips_less_than_the_hysteresis() {
        assert_eq!(board_duty_after(&[60_000, 58_000]), "153");
    }

    #[test]
    fn duty_falls_once_the_temperature_drops_past_the_hysteresis() {
        // 55 °C is 5 °C down; the duty follows the curve at 58 °C: 56%, raw 142.8.
        assert_eq!(board_duty_after(&[60_000, 55_000]), "143");
    }

    #[test]
    fn duty_rises_at_once() {
        // 62 °C: 64%, raw 163.2.
        assert_eq!(board_duty_after(&[60_000, 62_000]), "163");
    }

    #[test]
    fn unknown_sensor_leaves_the_fan_in_automatic_mode_and_says_why() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, "gone/gone/temp1"));
        let status = controller.tick();
        assert_eq!(
            (sys.read("class/hwmon/hwmon2/pwm1_enable"), status.fans[0].error.as_deref()),
            ("5".to_string(), Some("sensor gone/gone/temp1 not found"))
        );
    }

    #[test]
    fn fan_that_leaves_the_plan_goes_back_to_automatic_mode() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        controller.tick();
        controller.set_plan(Plan::default());
        controller.tick();
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1_enable"), "5");
    }

    #[test]
    fn curve_that_starts_at_zero_stops_the_fan_below_its_first_point() {
        let sys = board(45_000);
        let mut controller = Controller::new(sys.paths());
        let mut p = plan(FAN, SENSOR);
        p.fans[0].curve.points = vec![
            CurvePoint { temp: 50.0, duty: 0.0 },
            CurvePoint { temp: 70.0, duty: 100.0 },
        ];
        controller.set_plan(p);
        controller.tick();
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1"), "0");
    }

    #[test]
    fn another_fan_controller_blocks_the_plan() {
        let sys = board(60_000);
        sys.write("proc/77/cmdline", "/usr/bin/thinkfan\0-n\0");
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        let status = controller.tick();
        assert_eq!(
            (sys.read("class/hwmon/hwmon2/pwm1_enable"), status.blocked_by.as_deref()),
            ("5".to_string(), Some("thinkfan"))
        );
    }

    #[test]
    fn fan_controller_that_starts_later_gets_the_fans_within_the_rescan() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        controller.tick();
        sys.write("proc/77/cmdline", "/usr/bin/thinkfan\0-n\0");
        for _ in 0..RESCAN_EVERY {
            controller.tick();
        }
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1_enable"), "5");
    }

    #[test]
    fn closing_the_pipe_gives_the_fans_back() {
        let sys = board(60_000);
        let line = serde_json::to_string(&plan(FAN, SENSOR)).unwrap() + "\n";
        let mut output = Vec::new();
        serve(Controller::new(sys.paths()), Cursor::new(line), &mut output, &AtomicBool::new(false));
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1_enable"), "5");
    }

    #[test]
    fn helper_reports_each_new_status_as_a_json_line() {
        let sys = board(60_000);
        let line = serde_json::to_string(&plan(FAN, SENSOR)).unwrap() + "\n";
        let mut output = Vec::new();
        serve(Controller::new(sys.paths()), Cursor::new(line), &mut output, &AtomicBool::new(false));
        let duties: Vec<Option<f64>> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str::<Status>(l).unwrap())
            .flat_map(|s| s.fans.into_iter().map(|f| f.duty))
            .collect();
        assert_eq!(duties, [Some(60.0)]);
    }

    #[test]
    fn plan_uses_the_custom_throttle_point_as_the_failsafe() {
        let sys = board(60_000);
        let mut settings = Settings::default();
        settings.fan_curves.insert(FAN.into(), plan(FAN, SENSOR).fans[0].curve.clone());
        let custom = Override {
            safe: None,
            throttle: Some(72.0),
        };
        settings.overrides.insert(SENSOR.into(), custom);
        let failsafes: Vec<f64> = Plan::new(&settings, &sensors::discover(&sys.paths()))
            .fans
            .iter()
            .map(|f| f.failsafe)
            .collect();
        assert_eq!(failsafes, [72.0]);
    }

    #[test]
    fn curve_points_may_come_in_any_order() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        let mut p = plan(FAN, SENSOR);
        p.fans[0].curve.points.reverse();
        controller.set_plan(p);
        controller.tick();
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1"), "153");
    }

    #[test]
    fn sensor_that_stops_reading_at_a_rescan_still_forces_full_speed() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        for _ in 0..RESCAN_EVERY {
            controller.tick();
        }
        // The next tick rescans, and the rescan skips sensors that do not read.
        sys.write("class/hwmon/hwmon2/temp1_input", "garbage\n");
        controller.tick();
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1"), "255");
    }

    #[test]
    fn dropping_the_controller_restores_the_firmware_mode() {
        let sys = board(60_000);
        let mut controller = Controller::new(sys.paths());
        controller.set_plan(plan(FAN, SENSOR));
        controller.tick();
        drop(controller);
        assert_eq!(sys.read("class/hwmon/hwmon2/pwm1_enable"), "5");
    }
}
