//! The window's end of the pipe to the fan helper. It starts the helper through pkexec,
//! sends it each new plan, and follows its status. While the fan daemon runs the window's
//! settings, it starts no helper and follows the daemon's status instead.
//!
//! In a sandbox that sets "no new privileges", pkexec cannot become root. On NixOS,
//! `appimage-run` runs each AppImage in such a sandbox (bubblewrap). There the user's
//! systemd manager starts pkexec instead, outside the sandbox.

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Serialize;

use thermite_core::control;
use thermite_core::helper::{Plan, Status, StatusLog, HELPER_FLAG};

/// What the UI shows about fan control.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlState {
    /// Why the fans with curves do not follow them, in a few words. `None` while they do,
    /// and while no fan has a curve.
    pub notice: Option<String>,
    /// True while the fan daemon applies the settings. The window's fan helper does not run then.
    pub daemon: bool,
    /// The last status of the fan helper, or of the fan daemon.
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
        ..ControlState::default()
    }
}

/// The state for a status that the fan helper or the fan daemon reported.
fn reported(status: Status, daemon: bool) -> ControlState {
    let notice = status.blocked_by.as_ref().map(|other| format!("{other} controls the fans"));
    ControlState { notice, daemon, status }
}

/// The window's end of the pipe to the fan helper.
pub struct Link {
    stdin: Option<ChildStdin>,
    /// The plan last sent. An unchanged plan is not sent again,
    /// so saving other settings does not show the password dialog again.
    sent: Option<Plan>,
    /// True while the fan daemon follows this window's settings file.
    daemon: bool,
    shared: Arc<Mutex<Shared>>,
    on_change: OnChange,
}

impl Link {
    /// `on_change` gets each new state, for the UI.
    pub fn new(on_change: impl Fn(&ControlState) + Send + Sync + 'static) -> Self {
        Self {
            stdin: None,
            sent: None,
            daemon: false,
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
    /// While the fan daemon follows the settings file, this only keeps `plan`.
    pub fn apply(&mut self, plan: Plan, proc: &Path, retry: bool) {
        if self.daemon {
            // The daemon reads the saved settings itself.
            self.sent = Some(plan);
            return;
        }
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

    /// Takes the status of the fan daemon that follows this window's settings file, or
    /// `None` when none runs. While one runs, the window's fan helper does not: two
    /// controllers on one fan fight each other. When the daemon stops, the window does not
    /// start the helper by itself, because the password dialog would come without warning.
    /// Apply in Fan control starts it, through `retry`.
    pub fn watch_daemon(&mut self, status: Option<Status>) {
        let state = match status {
            Some(status) => {
                if !self.daemon {
                    eprintln!("thermite: the fan daemon applies the fan settings");
                    self.stop();
                    self.daemon = true;
                }
                reported(status, true)
            }
            None if self.daemon => {
                eprintln!("thermite: the fan daemon stopped");
                self.daemon = false;
                let curves = self.sent.as_ref().is_some_and(|plan| !plan.fans.is_empty());
                if curves {
                    notice("The fan daemon stopped")
                } else {
                    ControlState::default()
                }
            }
            None => return,
        };
        let mut shared = self.shared.lock().unwrap();
        update(&mut shared, state, &self.on_change);
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
            Ok(mut child) => {
                let mut stdin = child.stdin.take().expect("stdin is piped");
                // If the helper is already gone, `watch` reports why.
                let _ = send(&mut stdin, plan);
                self.stdin = Some(stdin);
                let shared = Arc::clone(&self.shared);
                let on_change = Arc::clone(&self.on_change);
                thread::spawn(move || watch(child, generation, &shared, &on_change));
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

fn spawn() -> io::Result<Child> {
    // Root cannot read the files inside the user's AppImage mount, so run the AppImage file.
    let program = match env::var_os("APPIMAGE") {
        Some(appimage) => PathBuf::from(appimage),
        None => env::current_exe()?,
    };
    session_command("pkexec", no_new_privs())
        .arg(program)
        .arg(HELPER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

/// True in a sandbox that sets "no new privileges", such as bubblewrap.
/// There a setuid program such as pkexec does not become root.
fn no_new_privs() -> bool {
    fs::read_to_string("/proc/self/status")
        .is_ok_and(|status| status.lines().any(|l| l.split_whitespace().eq(["NoNewPrivs:", "1"])))
}

/// A command for `program`. With `no_new_privs`, the user's systemd manager starts it,
/// outside the sandbox; `--pipe` passes stdin, stdout, stderr and the exit code through.
fn session_command(program: impl AsRef<OsStr>, no_new_privs: bool) -> Command {
    if !no_new_privs {
        return Command::new(program);
    }
    let mut command = Command::new("systemd-run");
    command.args(["--user", "--pipe", "--quiet", "--collect", "--"]).arg(program);
    command
}

/// Follows one helper's output until it exits, and keeps the shared state up to date.
fn watch(mut child: Child, generation: u64, shared: &Mutex<Shared>, on_change: &OnChange) {
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    // pkexec and the helper write their errors here. Pass them on to the log, and keep the first.
    let errors = thread::spawn(move || {
        let mut first = None;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            eprintln!("{line}");
            if first.is_none() && !line.trim().is_empty() {
                first = Some(line);
            }
        }
        first
    });
    let mut log = StatusLog::default();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(status) = serde_json::from_str::<Status>(&line) else {
            continue;
        };
        if log.worth_a_line(&status) {
            eprintln!("thermite: fan helper: {line}");
        }
        let mut shared = shared.lock().unwrap();
        if shared.generation == generation {
            update(&mut shared, reported(status, false), on_change);
        }
    }
    let exit = child.wait();
    let error = errors.join().ok().flatten();
    let mut shared = shared.lock().unwrap();
    if shared.generation != generation {
        return;
    }
    shared.running = false;
    let reason = match (exit, error) {
        // pkexec: 126 when the password dialog was dismissed.
        (Ok(exit), _) if exit.code() == Some(126) => "Password dialog cancelled".to_string(),
        // 127 when not authorized, and also when pkexec fails, e.g. "pkexec must be setuid root".
        (Ok(exit), Some(error)) if !exit.success() => error,
        (Ok(exit), _) => format!("Fan helper stopped ({exit})"),
        (Err(e), _) => format!("Fan helper stopped: {e}"),
    };
    eprintln!("thermite: fan curves are off: {reason}");
    update(&mut shared, notice(reason), on_change);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use tempfile::TempDir;
    use thermite_core::curve::{CurvePoint, FanCurve};
    use thermite_core::helper::FanPlan;

    /// One fan at a fixed level of 40%.
    fn plan() -> Plan {
        let curve = FanCurve {
            sensor: "k10temp/0000:00:18.3/temp1".into(),
            points: vec![CurvePoint { temp: 0.0, duty: 40.0 }],
        };
        Plan {
            fans: vec![FanPlan {
                fan: "cros_ec/cros-ec-hwmon/fan1".into(),
                curve,
                failsafe: 95.0,
            }],
        }
    }

    /// A `/proc` where thinkfan runs. A link that tries to start the fan helper
    /// stops at thinkfan, so a test cannot start pkexec.
    fn proc_with_thinkfan() -> TempDir {
        let proc = tempfile::tempdir().unwrap();
        fs::create_dir(proc.path().join("77")).unwrap();
        fs::write(proc.path().join("77/cmdline"), "/usr/bin/thinkfan\0-n\0").unwrap();
        proc
    }

    #[test]
    fn the_window_starts_no_fan_helper_while_the_daemon_runs() {
        let proc = proc_with_thinkfan();
        let mut link = Link::new(|_| {});
        link.watch_daemon(Some(Status::default()));
        link.apply(plan(), proc.path(), true);
        let state = link.state();
        assert_eq!((state.daemon, state.notice), (true, None));
    }

    #[test]
    fn the_window_closes_its_fan_helper_when_the_daemon_starts() {
        // `cat` stands in for the helper: it also exits when its stdin closes.
        let mut helper = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut link = Link::new(|_| {});
        link.stdin = helper.stdin.take();
        link.shared.lock().unwrap().running = true;
        link.watch_daemon(Some(Status::default()));
        let exited = (0..500).any(|_| {
            thread::sleep(Duration::from_millis(10));
            helper.try_wait().unwrap().is_some()
        });
        let _ = helper.kill();
        assert!(exited);
    }

    #[test]
    fn the_window_says_so_when_the_daemon_stops() {
        let proc = proc_with_thinkfan();
        let mut link = Link::new(|_| {});
        link.watch_daemon(Some(Status::default()));
        link.apply(plan(), proc.path(), false);
        link.watch_daemon(None);
        let state = link.state();
        assert_eq!(
            (state.daemon, state.notice.as_deref()),
            (false, Some("The fan daemon stopped"))
        );
    }

    #[test]
    fn a_failed_pkexec_shows_its_own_message() {
        // pkexec exits 127 both when the user is not authorized and when it fails.
        let child = Command::new("sh")
            .args(["-c", "echo 'pkexec must be setuid root' >&2; exit 127"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let shared = Mutex::new(Shared { generation: 1, running: true, ..Shared::default() });
        let on_change: OnChange = Arc::new(|_| {});
        watch(child, 1, &shared, &on_change);
        assert_eq!(shared.lock().unwrap().state.notice.as_deref(), Some("pkexec must be setuid root"));
    }

    /// `cargo test leaves_a_sandbox -- --ignored`.
    #[test]
    #[ignore = "needs bwrap and a systemd user session"]
    fn leaves_a_sandbox_that_blocks_setuid() {
        // bwrap sets "no new privileges", as appimage-run does for an AppImage on NixOS.
        let inner = session_command("grep", true);
        let out = Command::new("bwrap")
            .args(["--dev-bind", "/", "/", "--"])
            .arg(inner.get_program())
            .args(inner.get_args())
            .args(["NoNewPrivs", "/proc/self/status"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "NoNewPrivs:\t0");
    }
}
