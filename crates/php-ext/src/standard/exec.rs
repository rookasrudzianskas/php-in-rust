//! ext/standard/exec.c — program execution: `exec`, `system`, `passthru`,
//! `shell_exec`, `escapeshellarg`, `escapeshellcmd` and `proc_nice`.
//!
//! Commands run the way `popen(cmd, "r")` runs them: `/bin/sh -c cmd` (with
//! `argv[0]` = `sh`), stdout piped back to PHP, stdin and stderr inherited.
//! The child ignores `SIGPIPE`, as children of the PHP CLI do (php_cli.c
//! ignores it and the disposition survives `exec`; Rust's `Command` would
//! reset it). The result code is what `pclose()` reports through
//! `php_stdiop_close`: `WEXITSTATUS` for a normal exit, the raw wait status
//! otherwise (e.g. 15 for a shell killed by `SIGTERM`), -1 if waiting fails.
//!
//! Not modelled: the `auto_detect_line_endings` EOL detection of the stream
//! layer (lines always end at `\n`), `--enable-sigchild`, and the Windows
//! branches of php-src.

use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, ExitStatus};
use std::rc::Rc;
use std::sync::OnceLock;

use php_runtime::{Args, NativeFunction, PResult, PhpRef, Registry, Value, Vm};

/// `EXEC_INPUT_BUF` (main/php.h).
const EXEC_INPUT_BUF: usize = 4096;

/// `_POSIX_ARG_MAX`: fallback when `sysconf(_SC_ARG_MAX)` fails.
#[cfg(unix)]
const POSIX_ARG_MAX: usize = 4096;

/// `cmd_max_len` from `PHP_MINIT_FUNCTION(exec)`: `sysconf(_SC_ARG_MAX)`.
pub fn cmd_max_len() -> usize {
    static CMD_MAX_LEN: OnceLock<usize> = OnceLock::new();
    *CMD_MAX_LEN.get_or_init(|| {
        #[cfg(unix)]
        {
            usize::try_from(sys::arg_max()).unwrap_or(POSIX_ARG_MAX)
        }
        #[cfg(not(unix))]
        {
            // Commands run through cmd.exe, which has a constant limit.
            8192
        }
    })
}

// ---------------------------------------------------------------------------
// Running commands
// ---------------------------------------------------------------------------

/// What [`php_exec`] does with the command's output (the `type` argument of
/// php-src's `php_exec()`).
pub enum ExecMode {
    /// Type 0 (`exec()` without `$output`): only the last line is returned.
    LastLine,
    /// Type 1 (`system()`): every line is echoed, the last one returned.
    Print,
    /// Type 2 (`exec()` with `$output`): every line, stripped of trailing
    /// whitespace, is appended to the array held by the reference.
    Lines(PhpRef),
    /// Type 3 (`passthru()`): the raw output is echoed, nothing is returned.
    Passthru,
}

/// `popen(cmd, "r")`.
#[cfg(unix)]
fn popen(cmd: &[u8]) -> io::Result<Child> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut command = Command::new("/bin/sh");
    command
        .arg0("sh")
        .arg("-c")
        .arg(OsStr::from_bytes(cmd))
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    // SAFETY: the hook runs in the forked child right before `exec` and only
    // calls signal(2), which is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            sys::ignore_sigpipe();
            Ok(())
        });
    }
    command.spawn()
}

#[cfg(not(unix))]
fn popen(_cmd: &[u8]) -> io::Result<Child> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// `pclose()` as `php_stdiop_close` reports it: closes the pipe, waits for
/// the child and returns its exit code (or the raw wait status if it did not
/// exit normally); -1 if waiting failed.
fn pclose(mut child: Child) -> i64 {
    drop(child.stdout.take());
    match child.wait() {
        Ok(status) => exit_status_code(status),
        Err(_) => -1,
    }
}

fn exit_status_code(status: ExitStatus) -> i64 {
    match status.code() {
        Some(code) => i64::from(code),
        #[cfg(unix)]
        None => {
            use std::os::unix::process::ExitStatusExt;
            i64::from(status.into_raw())
        }
        #[cfg(not(unix))]
        None => -1,
    }
}

/// `isspace()` in the C / C.UTF-8 locale.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `strip_trailing_whitespace()`.
fn strip_trailing_whitespace(buf: &mut Vec<u8>) {
    let len = buf
        .iter()
        .rposition(|&b| !is_c_space(b))
        .map_or(0, |i| i + 1);
    buf.truncate(len);
}

/// `handle_line()`.
fn handle_line(vm: &mut Vm, mode: &ExecMode, line: &mut Vec<u8>) -> PResult<()> {
    match mode {
        ExecMode::Print => {
            vm.echo(line)?;
            if vm.output.level() < 1 {
                vm.sapi_flush();
            }
        }
        ExecMode::Lines(output) => {
            strip_trailing_whitespace(line);
            if let Value::Array(a) = &mut *output.borrow_mut() {
                // `add_next_index_stringl` silently drops the line when the
                // next index is taken.
                let _ = Rc::make_mut(a).append(Value::from(line.as_slice()));
            }
        }
        ExecMode::LastLine | ExecMode::Passthru => {}
    }
    Ok(())
}

/// The line loop of `php_exec()` for types 0–2. php-src reads lines with
/// `php_stream_get_line()` in `EXEC_INPUT_BUF` chunks and glues chunks
/// without a newline together, so a line is everything up to and including
/// `\n` (or the end of the output), whatever its length.
fn read_lines(vm: &mut Vm, mode: &ExecMode, stdout: ChildStdout) -> PResult<Value> {
    let mut reader = BufReader::with_capacity(EXEC_INPUT_BUF, stdout);
    let mut last: Option<Vec<u8>> = None;
    loop {
        let mut line = Vec::new();
        // A read error ends the output like EOF does; what was read before
        // it still counts.
        let res = reader.read_until(b'\n', &mut line);
        if line.is_empty() {
            break;
        }
        handle_line(vm, mode, &mut line)?;
        last = Some(line);
        if res.is_err() {
            break;
        }
    }
    // Return the last line from the shell command; "" (not null, for BC)
    // when there was no output.
    Ok(match last {
        Some(mut line) => {
            strip_trailing_whitespace(&mut line);
            Value::from(line)
        }
        None => Value::empty_string(),
    })
}

/// The binary loop of `php_exec()` for type 3.
fn passthru_output(vm: &mut Vm, mut stdout: ChildStdout) -> PResult<()> {
    let mut buf = vec![0; EXEC_INPUT_BUF];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => vm.echo(&buf[..n])?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Ok(()),
        }
    }
}

/// `php_exec()`: runs `cmd` through the shell and handles its output
/// according to `mode`. Returns the `pclose()` status and the PHP return
/// value (`false` after the "Unable to fork" warning).
///
/// If echoing fails (an output handler threw or exited), the pipe is closed,
/// the child is waited for and the error is propagated.
pub fn php_exec(vm: &mut Vm, mode: &ExecMode, cmd: &[u8]) -> PResult<(i64, Value)> {
    let mut child = match popen(cmd) {
        Ok(child) => child,
        Err(_) => {
            vm.warning([b"Unable to fork [", cmd, b"]"].concat())?;
            return Ok((-1, Value::Bool(false)));
        }
    };
    let stdout = child.stdout.take().expect("stdout is piped");
    let result = match mode {
        ExecMode::Passthru => passthru_output(vm, stdout).map(|()| Value::Null),
        _ => read_lines(vm, mode, stdout),
    };
    let status = pclose(child);
    result.map(|ret| (status, ret))
}

/// `Z_PARAM_PATH` / `Z_PARAM_PATH_STR`: no NUL bytes allowed.
fn check_path(vm: &mut Vm, arg_num: usize, s: &[u8]) -> PResult<()> {
    if s.contains(&0) {
        return Err(vm.argument_value_error(arg_num, "must not contain any null bytes"));
    }
    Ok(())
}

/// Validates the `$command` argument of the exec family.
fn check_command(vm: &mut Vm, cmd: &[u8]) -> PResult<()> {
    check_path(vm, 1, cmd)?;
    if cmd.is_empty() {
        // zend_argument_must_not_be_empty_error()
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExecFunc {
    Exec,
    System,
    Passthru,
}

/// `php_exec_ex()`.
fn php_exec_ex(vm: &mut Vm, args: &mut Args, func: ExecFunc) -> PResult<Value> {
    let cmd = args.str(0);
    check_command(vm, cmd)?;
    let code_arg = if func == ExecFunc::Exec { 2 } else { 1 };

    let mode = if func == ExecFunc::Exec && args.is_passed(1) {
        // An array in `$output` is appended to (separated on first write);
        // anything else is replaced by a new array (`zend_try_array_init`).
        let output = args.by_ref(1);
        {
            let mut v = output.borrow_mut();
            if !matches!(*v, Value::Array(_)) {
                *v = Value::empty_array();
            }
        }
        ExecMode::Lines(output)
    } else {
        match func {
            ExecFunc::Exec => ExecMode::LastLine,
            ExecFunc::System => ExecMode::Print,
            ExecFunc::Passthru => ExecMode::Passthru,
        }
    };

    let (status, ret) = php_exec(vm, &mode, cmd)?;
    if args.is_passed(code_arg) {
        args.by_ref(code_arg).set(Value::Int(status));
    }
    Ok(ret)
}

fn exec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_exec_ex(vm, args, ExecFunc::Exec)
}

fn system(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_exec_ex(vm, args, ExecFunc::System)
}

fn passthru(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_exec_ex(vm, args, ExecFunc::Passthru)
}

fn shell_exec(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cmd = args.str(0);
    check_command(vm, cmd)?;
    let Ok(mut child) = popen(cmd) else {
        vm.warning([b"Unable to execute '", cmd.as_bytes(), b"'"].concat())?;
        return Ok(Value::Bool(false));
    };
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        // `php_stream_copy_to_mem`: a read error keeps what was read.
        let _ = stdout.read_to_end(&mut out);
    }
    pclose(child);
    Ok(if out.is_empty() {
        Value::Null
    } else {
        Value::from(out)
    })
}

/// The backtick operator. php-src compiles `` `cmd` `` into a plain call to
/// `shell_exec()` (`zend_compile_shell_exec`, which also emits the
/// compile-time deprecation), so this calls the registered function: frames,
/// error messages and a disabled `shell_exec` behave exactly alike.
pub fn shell_exec_bytes(vm: &mut Vm, cmd: &[u8]) -> PResult<Value> {
    vm.call_native("shell_exec", vec![Value::from(cmd)])
}

// ---------------------------------------------------------------------------
// Escaping
// ---------------------------------------------------------------------------

/// `php_mblen()` (`mblen(3)`) in the locale PHP runs with. PHP 8 sets
/// `LC_CTYPE` to "C.UTF-8" at startup (`zend_reset_lc_ctype_locale()` in
/// `php_module_startup`), so this is UTF-8 aware: `Some(len)` for a
/// well-formed sequence, `None` (-1) for an ill-formed or truncated one,
/// which the escape loops drop. Locale changes by `setlocale()` are not
/// modelled. ASCII (incl. NUL, which callers reject) is one byte.
fn php_mblen(s: &[u8]) -> Option<usize> {
    let len = match s[0] {
        0x00..=0x7f => return Some(1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    let seq = s.get(..len)?;
    std::str::from_utf8(seq).ok().map(|_| len)
}

/// One step of the `php_mblen()` loops in the escape functions.
enum MbChar<'a> {
    /// A single-byte character, subject to escaping.
    Byte(u8),
    /// A valid multibyte character, copied verbatim.
    Multi(&'a [u8]),
}

/// Iterates `s` as the escape loops do: `(offset, char)`, skipping bytes
/// that do not start a valid character.
fn mb_chars(s: &[u8]) -> impl Iterator<Item = (usize, MbChar<'_>)> {
    let mut x = 0;
    std::iter::from_fn(move || {
        while x < s.len() {
            let start = x;
            match php_mblen(&s[x..]) {
                None => x += 1,
                Some(n) if n > 1 => {
                    x += n;
                    return Some((start, MbChar::Multi(&s[start..x])));
                }
                Some(_) => {
                    x += 1;
                    return Some((start, MbChar::Byte(s[start])));
                }
            }
        }
        None
    })
}

/// `php_escape_shell_cmd()`: escapes every character that could be used to
/// break out of a shell command. Quotes are left alone when they come in
/// pairs. Throws a `ValueError` if the input or the result is too long.
/// `cmd` must not contain NUL bytes.
pub fn php_escape_shell_cmd(vm: &mut Vm, cmd: &[u8]) -> PResult<Vec<u8>> {
    let max = cmd_max_len();
    // max command line length - two single quotes - \0 byte length
    if cmd.len() > max - 2 - 1 {
        return Err(
            vm.throw_value_error(format!("Command exceeds the allowed length of {max} bytes"))
        );
    }

    let mut out = Vec::with_capacity(2 * cmd.len());
    // Offset of the quote that closes the currently open (unescaped) quote.
    let mut pending: Option<usize> = None;
    for (x, ch) in mb_chars(cmd) {
        let c = match ch {
            MbChar::Multi(seq) => {
                out.extend_from_slice(seq);
                continue;
            }
            MbChar::Byte(c) => c,
        };
        match c {
            b'"' | b'\'' => {
                match pending {
                    None => {
                        pending = cmd[x + 1..].iter().position(|&b| b == c).map(|i| x + 1 + i);
                        if pending.is_none() {
                            out.push(b'\\');
                        }
                    }
                    Some(p) if cmd[p] == c => pending = None,
                    Some(_) => out.push(b'\\'),
                }
                out.push(c);
            }
            // 0xff never gets here: it is not valid UTF-8.
            b'#' | b'&' | b';' | b'`' | b'|' | b'*' | b'?' | b'~' | b'<' | b'>' | b'^' | b'('
            | b')' | b'[' | b']' | b'{' | b'}' | b'$' | b'\\' | b'\n' | 0xff => {
                out.push(b'\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }

    if out.len() > max + 1 {
        return Err(vm.throw_value_error(format!(
            "Escaped command exceeds the allowed length of {max} bytes"
        )));
    }
    Ok(out)
}

/// `php_escape_shell_arg()`: wraps `arg` in single quotes, turning each `'`
/// into `'\''`. Throws a `ValueError` if the input or the result is too long.
/// `arg` must not contain NUL bytes.
pub fn php_escape_shell_arg(vm: &mut Vm, arg: &[u8]) -> PResult<Vec<u8>> {
    let max = cmd_max_len();
    // max command line length - two single quotes - \0 byte length
    if arg.len() > max - 2 - 1 {
        return Err(vm.throw_value_error(format!(
            "Argument exceeds the allowed length of {max} bytes"
        )));
    }

    let mut out = Vec::with_capacity(arg.len() + 2);
    out.push(b'\'');
    for (_, ch) in mb_chars(arg) {
        match ch {
            MbChar::Multi(seq) => out.extend_from_slice(seq),
            MbChar::Byte(b'\'') => out.extend_from_slice(b"'\\''"),
            MbChar::Byte(c) => out.push(c),
        }
    }
    out.push(b'\'');

    if out.len() > max + 1 {
        return Err(vm.throw_value_error(format!(
            "Escaped argument exceeds the allowed length of {max} bytes"
        )));
    }
    Ok(out)
}

fn escapeshellcmd(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let command = args.str(0);
    check_path(vm, 1, command)?;
    if command.is_empty() {
        return Ok(Value::empty_string());
    }
    Ok(Value::from(php_escape_shell_cmd(vm, command)?))
}

fn escapeshellarg(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let arg = args.str(0);
    check_path(vm, 1, arg)?;
    Ok(Value::from(php_escape_shell_arg(vm, arg)?))
}

// ---------------------------------------------------------------------------
// proc_nice
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn proc_nice(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // nice() takes an int: the zend_long is truncated as in the C call.
    let pri = args.int(0) as std::ffi::c_int;
    if sys::nice(pri).is_err() {
        vm.warning("Only a super user may attempt to increase the priority of a process")?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

/// Minimal libc bindings (std has no `sysconf`, `nice` or `signal`).
#[cfg(unix)]
mod sys {
    use std::ffi::{c_int, c_long};
    use std::io;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    const SC_ARG_MAX: c_int = 0;
    // Apple, the BSDs and Solaris.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const SC_ARG_MAX: c_int = 1;

    const SIGPIPE: c_int = 13;
    const SIG_IGN: usize = 1;

    unsafe extern "C" {
        safe fn sysconf(name: c_int) -> c_long;
        #[link_name = "nice"]
        safe fn c_nice(incr: c_int) -> c_int;
        fn signal(signum: c_int, handler: usize) -> usize;
    }

    #[cfg(any(
        target_os = "linux",
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "android",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "solaris",
        target_os = "illumos"
    ))]
    unsafe extern "C" {
        #[cfg_attr(target_os = "linux", link_name = "__errno_location")]
        #[cfg_attr(
            any(
                target_vendor = "apple",
                target_os = "freebsd",
                target_os = "dragonfly"
            ),
            link_name = "__error"
        )]
        #[cfg_attr(
            any(target_os = "android", target_os = "netbsd", target_os = "openbsd"),
            link_name = "__errno"
        )]
        #[cfg_attr(
            any(target_os = "solaris", target_os = "illumos"),
            link_name = "___errno"
        )]
        safe fn errno_location() -> *mut c_int;
    }

    /// `errno = 0`.
    fn clear_errno() {
        #[cfg(any(
            target_os = "linux",
            target_vendor = "apple",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "android",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "solaris",
            target_os = "illumos"
        ))]
        // SAFETY: the errno location of the calling thread is always valid
        // for writes.
        unsafe {
            *errno_location() = 0;
        }
    }

    /// `sysconf(_SC_ARG_MAX)`; -1 on failure.
    pub fn arg_max() -> c_long {
        sysconf(SC_ARG_MAX)
    }

    /// `errno = 0; nice(incr); if (errno) ...` as in `proc_nice()`. A
    /// conforming nice() returns -1 whenever it sets errno.
    pub fn nice(incr: c_int) -> io::Result<()> {
        clear_errno();
        let ret = c_nice(incr);
        let err = io::Error::last_os_error();
        if ret == -1 && err.raw_os_error().is_some_and(|e| e != 0) {
            return Err(err);
        }
        Ok(())
    }

    /// `signal(SIGPIPE, SIG_IGN)`; async-signal-safe.
    pub fn ignore_sigpipe() {
        // SAFETY: SIG_IGN is a valid disposition for SIGPIPE.
        unsafe {
            signal(SIGPIPE, SIG_IGN);
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static EXEC: NativeFunction = NativeFunction {
    sig: "exec(string $command, &$output = null, &$result_code = null): string|false",
    handler: exec,
};

static SYSTEM: NativeFunction = NativeFunction {
    sig: "system(string $command, &$result_code = null): string|false",
    handler: system,
};

static PASSTHRU: NativeFunction = NativeFunction {
    sig: "passthru(string $command, &$result_code = null): false|null",
    handler: passthru,
};

static ESCAPESHELLCMD: NativeFunction = NativeFunction {
    sig: "escapeshellcmd(string $command): string",
    handler: escapeshellcmd,
};

static ESCAPESHELLARG: NativeFunction = NativeFunction {
    sig: "escapeshellarg(string $arg): string",
    handler: escapeshellarg,
};

static SHELL_EXEC: NativeFunction = NativeFunction {
    sig: "shell_exec(string $command): string|false|null",
    handler: shell_exec,
};

#[cfg(unix)]
static PROC_NICE: NativeFunction = NativeFunction {
    sig: "proc_nice(int $priority): bool",
    handler: proc_nice,
};

pub fn register(r: &mut Registry) {
    // PHP_MINIT_FUNCTION(exec)
    cmd_max_len();
    r.functions(&[
        &EXEC,
        &SYSTEM,
        &PASSTHRU,
        &ESCAPESHELLCMD,
        &ESCAPESHELLARG,
        &SHELL_EXEC,
    ]);
    // HAVE_NICE
    #[cfg(unix)]
    r.function(&PROC_NICE);
}
