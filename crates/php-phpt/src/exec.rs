//! Child process execution with timeouts, binary-safe output capture and
//! optional stderr merging (the port of `system_with_timeout()`).
//!
//! Children run in their own process group so a timeout can kill the whole
//! tree; a reader thread drains the output pipe so a runaway child can never
//! block the runner.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Captured output is capped to protect the runner from runaway tests.
pub const MAX_OUTPUT: usize = 256 * 1024 * 1024;

/// What to connect to the child's stdin.
#[derive(Debug, Clone)]
pub enum StdinMode {
    /// Feed these bytes, then close the pipe.
    Data(Vec<u8>),
    /// Share the runner's stdin.
    Inherit,
    /// `/dev/null`.
    Null,
}

/// stdout handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdoutMode {
    Capture,
    Inherit,
}

/// stderr handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StderrMode {
    /// Merge into the captured stdout (`2>&1`).
    Merge,
    /// Capture (only meaningful when stdout is not captured).
    Capture,
    Inherit,
    Discard,
}

/// Description of one process to run.
#[derive(Debug, Clone)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The complete environment of the child (nothing is inherited).
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    pub stdin: StdinMode,
    pub stdout: StdoutMode,
    pub stderr: StderrMode,
    pub timeout: Duration,
}

/// Result of running a process.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub data: Vec<u8>,
    pub timed_out: bool,
    /// Output exceeded [`MAX_OUTPUT`] and the child was killed.
    pub overflowed: bool,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    /// Set when the process could not be started at all.
    pub spawn_error: Option<String>,
}

impl Outcome {
    /// Signal number as reported by run-tests (`Termsig=`): either a real
    /// signal or a shell-style `128 + n` exit code.
    pub fn termsig(&self) -> Option<i32> {
        if let Some(sig) = self.signal {
            return Some(sig);
        }
        match self.exit_code {
            Some(code) if code > 128 && code < 160 => Some(code - 128),
            _ => None,
        }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    safe fn kill(pid: i32, sig: i32) -> i32;
    fn signal(signum: i32, handler: usize) -> usize;
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Whether SIGINT/SIGTERM was received (see [`install_interrupt_handler`]).
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

#[cfg(unix)]
extern "C" fn on_interrupt(_signum: i32) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Make SIGINT and SIGTERM request a graceful stop: running children are
/// killed (they live in their own process groups and would otherwise be
/// orphaned) and no new tests are started.
pub fn install_interrupt_handler() {
    #[cfg(unix)]
    {
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        let handler = on_interrupt as extern "C" fn(i32) as usize;
        // SAFETY: `on_interrupt` only stores to an atomic, which is
        // async-signal-safe, and has the C ABI `signal` expects.
        unsafe {
            signal(SIGINT, handler);
            signal(SIGTERM, handler);
        }
    }
}

/// Kill the child and everything in its process group.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // The child was started as a process group leader.
            kill(-pid, 9);
        }
    }
    let _ = child.kill();
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        kill(-pid, 9);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

#[derive(Default)]
struct Collector {
    buf: Mutex<Vec<u8>>,
    done: AtomicBool,
    overflow: AtomicBool,
}

impl Collector {
    fn spawn_reader<R: Read + Send + 'static>(self: &Arc<Self>, mut reader: R) {
        let me = Arc::clone(self);
        thread::spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut buf = me.buf.lock().unwrap_or_else(PoisonError::into_inner);
                        if buf.len() + n > MAX_OUTPUT {
                            let room = MAX_OUTPUT.saturating_sub(buf.len());
                            buf.extend_from_slice(&chunk[..room]);
                            me.overflow.store(true, Ordering::SeqCst);
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            me.done.store(true, Ordering::SeqCst);
        });
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.buf.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

fn wait_until(deadline: Instant, mut cond: impl FnMut() -> bool) -> bool {
    while !cond() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    true
}

/// Run a process to completion (or timeout) and capture its output.
pub fn run(spec: &Spec) -> Outcome {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&spec.cwd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.stdin(match &spec.stdin {
        StdinMode::Data(_) => Stdio::piped(),
        StdinMode::Inherit => Stdio::inherit(),
        StdinMode::Null => Stdio::null(),
    });

    // Wire up stdout/stderr; `reader` is the pipe we drain.
    let mut reader = None;
    let pipe = || std::io::pipe().map_err(|e| format!("cannot create pipe: {e}"));
    let setup = (|| -> Result<(), String> {
        match (spec.stdout, spec.stderr) {
            (StdoutMode::Capture, StderrMode::Merge) => {
                let (r, w) = pipe()?;
                let w2 = w
                    .try_clone()
                    .map_err(|e| format!("cannot clone pipe: {e}"))?;
                cmd.stdout(Stdio::from(w)).stderr(Stdio::from(w2));
                reader = Some(r);
            }
            (StdoutMode::Capture, other) => {
                let (r, w) = pipe()?;
                cmd.stdout(Stdio::from(w));
                cmd.stderr(if other == StderrMode::Inherit {
                    Stdio::inherit()
                } else {
                    Stdio::null()
                });
                reader = Some(r);
            }
            (StdoutMode::Inherit, StderrMode::Capture) => {
                let (r, w) = pipe()?;
                cmd.stdout(Stdio::inherit()).stderr(Stdio::from(w));
                reader = Some(r);
            }
            (StdoutMode::Inherit, StderrMode::Discard) => {
                cmd.stdout(Stdio::inherit()).stderr(Stdio::null());
            }
            (StdoutMode::Inherit, _) => {
                cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            }
        }
        Ok(())
    })();
    if let Err(e) = setup {
        return Outcome {
            spawn_error: Some(e),
            ..Outcome::default()
        };
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Outcome {
                spawn_error: Some(format!("{}: {e}", spec.program.display())),
                ..Outcome::default()
            };
        }
    };
    // Drop our copies of the pipe write ends so EOF can be observed.
    drop(cmd);

    if let StdinMode::Data(data) = &spec.stdin
        && let Some(mut stdin) = child.stdin.take()
    {
        let data = data.clone();
        if !data.is_empty() {
            thread::spawn(move || {
                // A child that exits without reading its input is fine.
                let _ = stdin.write_all(&data);
            });
        }
    }

    let collector = Arc::new(Collector::default());
    if let Some(r) = reader {
        collector.spawn_reader(r);
    } else {
        collector.done.store(true, Ordering::SeqCst);
    }

    let deadline = Instant::now() + spec.timeout;
    let mut timed_out = false;
    let mut overflowed = false;
    let mut nap = Duration::from_micros(100);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if collector.overflow.load(Ordering::SeqCst) {
            overflowed = true;
            kill_tree(&mut child);
            break child.wait().ok();
        }
        if interrupted() {
            kill_tree(&mut child);
            break child.wait().ok();
        }
        if Instant::now() >= deadline {
            timed_out = true;
            kill_tree(&mut child);
            break child.wait().ok();
        }
        thread::sleep(nap);
        nap = (nap * 2).min(Duration::from_millis(5));
    };

    // Let the reader drain what is left.  If the pipe stays open (a leaked
    // grandchild holds it), stop it after a grace period.
    let grace = if timed_out {
        Duration::from_secs(1)
    } else {
        Duration::from_secs(2)
    };
    let drained = wait_until(Instant::now() + grace, || collector.is_done());
    if !drained {
        kill_group(child.id());
        wait_until(Instant::now() + Duration::from_millis(500), || {
            collector.is_done()
        });
    }
    if collector.overflow.load(Ordering::SeqCst) {
        overflowed = true;
    }

    #[cfg(unix)]
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.and_then(|s| s.signal())
    };
    #[cfg(not(unix))]
    let signal = None;

    Outcome {
        data: collector.take(),
        timed_out,
        overflowed,
        exit_code: status.and_then(|s| s.code()),
        signal,
        spawn_error: None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Spec {
        Spec {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cwd: std::env::temp_dir(),
            stdin: StdinMode::Null,
            stdout: StdoutMode::Capture,
            stderr: StderrMode::Merge,
            timeout: Duration::from_secs(10),
        }
    }

    #[test]
    fn captures_stdout_and_merges_stderr_in_order() {
        let out = run(&sh("echo one; echo two >&2; echo three"));
        assert_eq!(out.data, b"one\ntwo\nthree\n");
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out);
    }

    #[test]
    fn stderr_is_not_captured_unless_asked() {
        let mut spec = sh("echo out; echo err >&2");
        spec.stderr = StderrMode::Discard;
        assert_eq!(run(&spec).data, b"out\n");
    }

    #[test]
    fn stderr_only_capture() {
        let mut spec = sh("echo out; echo err >&2");
        spec.stdout = StdoutMode::Inherit;
        spec.stderr = StderrMode::Capture;
        assert_eq!(run(&spec).data, b"err\n");
    }

    #[test]
    fn binary_output_is_preserved() {
        let out = run(&sh(r"printf 'a\377\376\000b'"));
        assert_eq!(out.data, b"a\xff\xfe\x00b");
    }

    #[test]
    fn stdin_data_is_delivered() {
        let mut spec = sh("cat");
        spec.stdin = StdinMode::Data(b"hello\nstdin".to_vec());
        assert_eq!(run(&spec).data, b"hello\nstdin");
    }

    #[test]
    fn large_stdin_to_a_child_that_ignores_it_does_not_deadlock() {
        let mut spec = sh("echo done");
        spec.stdin = StdinMode::Data(vec![b'x'; 4 * 1024 * 1024]);
        assert_eq!(run(&spec).data, b"done\n");
    }

    #[test]
    fn environment_is_exactly_what_was_given() {
        let mut spec = sh("echo \"[$FOO][$HOME]\"");
        spec.env.push(("FOO".into(), "bar".into()));
        assert_eq!(run(&spec).data, b"[bar][]\n");
    }

    #[test]
    fn timeout_kills_the_child_promptly() {
        // The timeout leaves the shell time to print even on a loaded machine;
        // the kill must still come long before the 30 s sleep ends.
        let mut spec = sh("echo started; sleep 30");
        spec.timeout = Duration::from_secs(2);
        let t = Instant::now();
        let out = run(&spec);
        assert!(out.timed_out);
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        assert_eq!(out.data, b"started\n");
    }

    #[test]
    fn timeout_kills_grandchildren_holding_the_pipe() {
        // The grandchild inherits stdout; without a process-group kill the
        // reader would block until it exits.
        let mut spec = sh("(sleep 30; echo late) & wait");
        spec.timeout = Duration::from_millis(300);
        let t = Instant::now();
        let out = run(&spec);
        assert!(out.timed_out);
        assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    }

    #[test]
    fn leaked_background_process_does_not_hang_after_exit() {
        let spec = sh("(sleep 30) & echo parent-done");
        let t = Instant::now();
        let out = run(&spec);
        assert!(!out.timed_out);
        assert_eq!(out.data, b"parent-done\n");
        assert!(t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
    }

    #[test]
    fn signals_and_exit_codes_are_reported() {
        let out = run(&sh("kill -SEGV $$"));
        assert_eq!(out.termsig(), Some(11));
        let out = run(&sh("exit 139"));
        assert_eq!(out.termsig(), Some(11));
        let out = run(&sh("exit 3"));
        assert_eq!(out.termsig(), None);
        assert_eq!(out.exit_code, Some(3));
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let mut spec = sh("true");
        spec.program = PathBuf::from("/nonexistent/php");
        let out = run(&spec);
        assert!(out.spawn_error.is_some());
    }

    #[test]
    fn working_directory_is_applied() {
        let mut spec = sh("pwd -P");
        let dir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        spec.cwd = dir.clone();
        let out = run(&spec);
        assert_eq!(
            String::from_utf8_lossy(&out.data).trim(),
            dir.to_string_lossy()
        );
    }
}
