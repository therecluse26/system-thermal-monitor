//! The window's end of the pipe to the fan helper. It starts the helper through pkexec,
//! sends it each new plan, and follows its status.
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
            let notice = status.blocked_by.as_ref().map(|other| format!("{other} controls the fans"));
            update(&mut shared, ControlState { notice, status }, on_change);
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
