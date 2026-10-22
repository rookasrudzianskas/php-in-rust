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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::output::ob;
    use php_runtime::{ArrayKey, PhpArray};

    fn bytes(v: &Value) -> Vec<u8> {
        match v {
            Value::String(s) => s.to_vec(),
            other => panic!("expected string, got {other:?}"),
        }
    }

    fn lines(v: &Value) -> Vec<Vec<u8>> {
        match v {
            Value::Array(a) => a.values().map(bytes).collect(),
            other => panic!("expected array, got {other:?}"),
        }
    }

    fn by_ref() -> PhpRef {
        PhpRef::new(Value::Null)
    }

    fn err(class: &str, msg: &str) -> (String, String) {
        (class.to_string(), msg.to_string())
    }

    fn escapeshellarg(t: &mut TestVm, arg: &[u8]) -> Vec<u8> {
        bytes(&t.call("escapeshellarg", vec![Value::from(arg)]).unwrap())
    }

    fn escapeshellcmd(t: &mut TestVm, cmd: &[u8]) -> Vec<u8> {
        bytes(&t.call("escapeshellcmd", vec![Value::from(cmd)]).unwrap())
    }

    /// `exec($cmd, $output, $result_code)`: (return value, $output, $result_code).
    fn exec3(t: &mut TestVm, cmd: &str, output: Value) -> (Value, Value, Value) {
        let (out, code) = (PhpRef::new(output), by_ref());
        let args = vec![
            cmd.into(),
            Value::Ref(out.clone()),
            Value::Ref(code.clone()),
        ];
        let ret = t.call("exec", args).unwrap();
        (ret, out.get(), code.get())
    }

    // --- escapeshellarg ---------------------------------------------------

    #[test]
    fn escapeshellarg_basic() {
        // escapeshellarg_basic.phpt
        let mut t = TestVm::new();
        assert_eq!(escapeshellarg(&mut t, b"Mr O'Neil"), b"'Mr O'\\''Neil'");
        assert_eq!(escapeshellarg(&mut t, b"Mr O\\'Neil"), b"'Mr O\\'\\''Neil'");
        assert_eq!(escapeshellarg(&mut t, b"%FILENAME"), b"'%FILENAME'");
        assert_eq!(escapeshellarg(&mut t, b""), b"''");
    }

    #[test]
    fn escapeshellarg_variations() {
        let mut t = TestVm::new();
        assert_eq!(escapeshellarg(&mut t, b"'"), b"''\\'''");
        assert_eq!(escapeshellarg(&mut t, b"''"), b"''\\'''\\'''");
        assert_eq!(escapeshellarg(&mut t, b"a b"), b"'a b'");
        assert_eq!(
            escapeshellarg(&mut t, b"$HOME `id` \"x\" \\"),
            b"'$HOME `id` \"x\" \\'"
        );
        assert_eq!(escapeshellarg(&mut t, b"line1\nline2"), b"'line1\nline2'");
        assert_eq!(escapeshellarg(&mut t, b"-rf; rm *"), b"'-rf; rm *'");
        // Weak-mode coercion from int.
        let r = t.call("escapeshellarg", vec![Value::Int(123)]).unwrap();
        assert_eq!(bytes(&r), b"'123'");
    }

    #[test]
    fn escapeshellarg_multibyte() {
        // UTF-8 sequences are kept, ill-formed bytes dropped (C.UTF-8 mblen).
        let mut t = TestVm::new();
        assert_eq!(
            escapeshellarg(&mut t, "héllo".as_bytes()),
            "'héllo'".as_bytes()
        );
        assert_eq!(
            escapeshellarg(&mut t, "日本'語".as_bytes()),
            "'日本'\\''語'".as_bytes()
        );
        assert_eq!(escapeshellarg(&mut t, "😀".as_bytes()), "'😀'".as_bytes());
        assert_eq!(escapeshellarg(&mut t, b"caf\xe9"), b"'caf'");
        assert_eq!(escapeshellarg(&mut t, b"a\x80b\xffc"), b"'abc'");
        // Truncated, overlong, surrogate and out-of-range sequences.
        assert_eq!(escapeshellarg(&mut t, b"x\xe2\x82"), b"'x'");
        assert_eq!(escapeshellarg(&mut t, b"\xc0\xaf'"), b"''\\'''");
        assert_eq!(escapeshellarg(&mut t, b"\xed\xa0\x80z"), b"'z'");
        assert_eq!(escapeshellarg(&mut t, b"\xf4\x90\x80\x80"), b"''");
        // An invalid lead byte does not swallow the quote after it.
        assert_eq!(escapeshellarg(&mut t, b"\xe2'"), b"''\\'''");
    }

    #[test]
    fn escapeshellarg_null_bytes() {
        // escapeshellarg_bug71039.phpt
        let mut t = TestVm::new();
        let r = t.call("escapeshellarg", vec![Value::from(&b"hello\0world"[..])]);
        assert_eq!(
            thrown(r),
            err(
                "ValueError",
                "escapeshellarg(): Argument #1 ($arg) must not contain any null bytes"
            )
        );
    }

    #[test]
    fn escapeshellarg_length_limits() {
        // escapeshellarg_bug71270.phpt
        let mut t = TestVm::new();
        let max = cmd_max_len();
        let r = t.call("escapeshellarg", vec![Value::from("A".repeat(max - 2))]);
        let msg = format!("Argument exceeds the allowed length of {max} bytes");
        assert_eq!(thrown(r), err("ValueError", &msg));
        // The longest accepted argument.
        let r = t.call("escapeshellarg", vec![Value::from("A".repeat(max - 3))]);
        assert_eq!(bytes(&r.unwrap()).len(), max - 1);
        // Escaping can push it over the limit.
        let r = t.call("escapeshellarg", vec![Value::from("'".repeat(max / 2))]);
        let msg = format!("Escaped argument exceeds the allowed length of {max} bytes");
        assert_eq!(thrown(r), err("ValueError", &msg));
    }

    // --- escapeshellcmd ---------------------------------------------------

    #[test]
    fn escapeshellcmd_metacharacters() {
        let mut t = TestVm::new();
        assert_eq!(
            escapeshellcmd(&mut t, b"#&;`|*?~<>^()[]{}$\\\n"),
            b"\\#\\&\\;\\`\\|\\*\\?\\~\\<\\>\\^\\(\\)\\[\\]\\{\\}\\$\\\\\\\n"
        );
        assert_eq!(
            escapeshellcmd(&mut t, b"ls -l 'foo bar' && rm *"),
            b"ls -l 'foo bar' \\&\\& rm \\*"
        );
        assert_eq!(
            escapeshellcmd(&mut t, b"%PATH% !x @=+,.:/-_"),
            b"%PATH% !x @=+,.:/-_"
        );
        assert_eq!(escapeshellcmd(&mut t, b"echo $(id)"), b"echo \\$\\(id\\)");
        assert_eq!(escapeshellcmd(&mut t, b""), b"");
    }

    #[test]
    fn escapeshellcmd_quote_pairing() {
        let mut t = TestVm::new();
        // Paired quotes stay, unpaired ones are escaped.
        assert_eq!(escapeshellcmd(&mut t, b"\"a\""), b"\"a\"");
        assert_eq!(escapeshellcmd(&mut t, b"'a'"), b"'a'");
        assert_eq!(escapeshellcmd(&mut t, b"'a"), b"\\'a");
        assert_eq!(escapeshellcmd(&mut t, b"a\""), b"a\\\"");
        assert_eq!(escapeshellcmd(&mut t, b"'a'b'"), b"'a'b\\'");
        // A different quote inside an open quote is escaped.
        assert_eq!(escapeshellcmd(&mut t, b"'a\"b'"), b"'a\\\"b'");
        assert_eq!(escapeshellcmd(&mut t, b"\"it's\""), b"\"it\\'s\"");
        assert_eq!(escapeshellcmd(&mut t, b"'a\"b'\"c\""), b"'a\\\"b'\"c\"");
        assert_eq!(escapeshellcmd(&mut t, b"\"'\"'"), b"\"\\'\"\\'");
        assert_eq!(escapeshellcmd(&mut t, b"''''"), b"''''");
        assert_eq!(escapeshellcmd(&mut t, b"'''"), b"''\\'");
        // Metacharacters inside quotes are still escaped.
        assert_eq!(escapeshellcmd(&mut t, b"echo 'a;b'"), b"echo 'a\\;b'");
    }

    #[test]
    fn escapeshellcmd_multibyte() {
        let mut t = TestVm::new();
        assert_eq!(
            escapeshellcmd(&mut t, "écho «x»".as_bytes()),
            "écho «x»".as_bytes()
        );
        assert_eq!(
            escapeshellcmd(&mut t, "日;本".as_bytes()),
            "日\\;本".as_bytes()
        );
        // 0xff is ill-formed UTF-8, so it is dropped rather than escaped.
        assert_eq!(escapeshellcmd(&mut t, b"a\xffb"), b"ab");
        assert_eq!(escapeshellcmd(&mut t, b"\x80;\xc3"), b"\\;");
    }

    #[test]
    fn escapeshellcmd_null_bytes() {
        // escapeshellcmd_bug71039.phpt
        let mut t = TestVm::new();
        let r = t.call("escapeshellcmd", vec![Value::from(&b"hello\0world"[..])]);
        assert_eq!(
            thrown(r),
            err(
                "ValueError",
                "escapeshellcmd(): Argument #1 ($command) must not contain any null bytes"
            )
        );
    }

    #[test]
    fn escapeshellcmd_length_limits() {
        // escapeshellcmd_bug71270.phpt
        let mut t = TestVm::new();
        let max = cmd_max_len();
        let r = t.call("escapeshellcmd", vec![Value::from("A".repeat(max - 2))]);
        let msg = format!("Command exceeds the allowed length of {max} bytes");
        assert_eq!(thrown(r), err("ValueError", &msg));
        let r = t.call("escapeshellcmd", vec![Value::from("A".repeat(max - 3))]);
        assert_eq!(bytes(&r.unwrap()).len(), max - 3);
        let r = t.call("escapeshellcmd", vec![Value::from("#".repeat(max - 3))]);
        let msg = format!("Escaped command exceeds the allowed length of {max} bytes");
        assert_eq!(thrown(r), err("ValueError", &msg));
    }

    // --- exec -------------------------------------------------------------

    #[test]
    fn exec_output_array() {
        // bug50732.phpt
        let mut t = TestVm::new();
        let (ret, out, code) = exec3(&mut t, "echo x", Value::Null);
        assert_eq!(bytes(&ret), b"x");
        assert_eq!(lines(&out), [b"x"]);
        assert!(matches!(code, Value::Int(0)));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn exec_long_lines() {
        // bug49847.phpt: lines longer than EXEC_INPUT_BUF are not split.
        let mut t = TestVm::new();
        let (_, out, _) = exec3(&mut t, "printf %4098d 1", Value::Null);
        let out = lines(&out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len(), 4098);
        assert!(out[0].ends_with(b" 1"));
        let (ret, out, _) = exec3(&mut t, "printf %9000d 7; echo; echo end", Value::Null);
        let lens: Vec<usize> = lines(&out).iter().map(Vec::len).collect();
        assert_eq!(lens, [9000, 3]);
        assert_eq!(bytes(&ret), b"end");
    }

    #[test]
    fn exec_strips_trailing_whitespace() {
        // bug70018.phpt
        let mut t = TestVm::new();
        let (ret, out, _) = exec3(&mut t, r"printf 'abc\f\n \n'", Value::Null);
        assert_eq!(lines(&out), [&b"abc"[..], b""]);
        assert_eq!(bytes(&ret), b"");
        let (ret, out, _) = exec3(&mut t, r"printf '  a b \t\v\r\nlast \r'", Value::Null);
        assert_eq!(lines(&out), [&b"  a b"[..], b"last"]);
        assert_eq!(bytes(&ret), b"last");
        // Leading whitespace and blank lines are kept.
        let (_, out, _) = exec3(&mut t, r"printf '\n\n x\n'", Value::Null);
        assert_eq!(lines(&out), [&b""[..], b"", b" x"]);
    }

    #[test]
    fn exec_returns_last_line() {
        let mut t = TestVm::new();
        let r = t.call("exec", vec![r"printf 'a\nb\n'".into()]).unwrap();
        assert_eq!(bytes(&r), b"b");
        let r = t.call("exec", vec![r"printf 'a\nlast'".into()]).unwrap();
        assert_eq!(bytes(&r), b"last");
        let r = t.call("exec", vec![r"printf 'a\n\n'".into()]).unwrap();
        assert_eq!(bytes(&r), b"");
        // No output: "" rather than null.
        let r = t.call("exec", vec!["true".into()]).unwrap();
        assert_eq!(bytes(&r), b"");
        // argv[0] is "sh", as with popen().
        let r = t.call("exec", vec!["echo $0".into()]).unwrap();
        assert_eq!(bytes(&r), b"sh");
        assert_eq!(t.output(), "");
    }

    #[test]
    fn exec_appends_to_existing_array() {
        let mut t = TestVm::new();
        let mut a = PhpArray::new();
        a.insert_str("k", "v");
        a.append("pre").unwrap();
        let (_, out, _) = exec3(&mut t, r"printf 'x\ny\n'", Value::array(a));
        assert_eq!(lines(&out), [&b"v"[..], b"pre", b"x", b"y"]);
        let Value::Array(out) = out else {
            unreachable!()
        };
        assert!(out.get(&ArrayKey::Int(2)).is_some());

        // A shared array is separated, not modified in place.
        let shared = Rc::new(PhpArray::from_iter([Value::from("a")]));
        let (_, out, _) = exec3(&mut t, "echo b", Value::Array(shared.clone()));
        assert_eq!(lines(&out), [b"a", b"b"]);
        assert_eq!(shared.len(), 1);

        // Anything else is replaced by a new array.
        let (_, out, _) = exec3(&mut t, "echo z", Value::from("string"));
        assert_eq!(lines(&out), [b"z"]);
        let (_, out, _) = exec3(&mut t, "true", Value::Int(5));
        assert!(lines(&out).is_empty());
    }

    #[test]
    fn exec_result_codes() {
        let mut t = TestVm::new();
        let (ret, out, code) = exec3(&mut t, "exit 3", Value::Null);
        assert_eq!(bytes(&ret), b"");
        assert!(lines(&out).is_empty());
        assert!(matches!(code, Value::Int(3)));
        let (ret, _, code) = exec3(&mut t, "echo out; exit 255", Value::Null);
        assert_eq!(bytes(&ret), b"out");
        assert!(matches!(code, Value::Int(255)));
        let (_, _, code) = exec3(&mut t, "nonexistent-command-xyz 2>/dev/null", Value::Null);
        assert!(matches!(code, Value::Int(127)));

        // Named argument without $output.
        let code = by_ref();
        let r =
            t.vm.call_native_named(
                "exec",
                vec!["echo hi; exit 4".into()],
                vec![("result_code", Value::Ref(code.clone()))],
            )
            .unwrap();
        assert_eq!(bytes(&r), b"hi");
        assert!(matches!(code.get(), Value::Int(4)));

        // $output and $result_code may be the same variable.
        let same = by_ref();
        let args = vec![
            "echo x".into(),
            Value::Ref(same.clone()),
            Value::Ref(same.clone()),
        ];
        t.call("exec", args).unwrap();
        assert!(matches!(same.get(), Value::Int(0)));
    }

    #[cfg(unix)]
    #[test]
    fn exec_signal_status() {
        // pclose() + WIFEXITED: a signalled shell yields the raw wait status.
        let mut t = TestVm::new();
        let (_, _, code) = exec3(&mut t, "kill -TERM $$", Value::Null);
        assert!(matches!(code, Value::Int(15)), "{code:?}");
        let (_, _, code) = exec3(&mut t, "kill -KILL $$", Value::Null);
        assert!(matches!(code, Value::Int(9)), "{code:?}");
        // SIGPIPE is ignored in the child, as under the PHP CLI.
        let (ret, _, code) = exec3(&mut t, "kill -PIPE $$; echo alive", Value::Null);
        assert_eq!(bytes(&ret), b"alive");
        assert!(matches!(code, Value::Int(0)));
    }

    #[test]
    fn exec_argument_errors() {
        // exec_basic1.phpt
        let mut t = TestVm::new();
        for f in ["exec", "system", "passthru"] {
            let cmd = Value::from(&b"echo abc\n\0command"[..]);
            let r = t.call(f, vec![cmd, by_ref().into()]);
            let msg = format!("{f}(): Argument #1 ($command) must not contain any null bytes");
            assert_eq!(thrown(r), err("ValueError", &msg));
            let r = t.call(f, vec!["".into()]);
            let msg = format!("{f}(): Argument #1 ($command) must not be empty");
            assert_eq!(thrown(r), err("ValueError", &msg));
        }
        let r = t.call("exec", vec![Value::empty_array()]);
        assert_eq!(
            thrown(r),
            err(
                "TypeError",
                "exec(): Argument #1 ($command) must be of type string, array given"
            )
        );
        let r = t.call("exec", vec![]);
        assert_eq!(
            thrown(r),
            err(
                "ArgumentCountError",
                "exec() expects at least 1 argument, 0 given"
            )
        );
        let r = t.call(
            "system",
            vec!["true".into(), by_ref().into(), by_ref().into()],
        );
        assert_eq!(
            thrown(r),
            err(
                "ArgumentCountError",
                "system() expects at most 2 arguments, 3 given"
            )
        );
        // $output is left alone when validation fails.
        let out = PhpRef::new(Value::from("keep"));
        thrown(t.call("exec", vec!["".into(), Value::Ref(out.clone())]));
        assert_eq!(bytes(&out.get()), b"keep");
    }

    // --- system -----------------------------------------------------------

    #[test]
    fn system_echoes_lines() {
        let mut t = TestVm::new();
        let r = t.call("system", vec!["echo hello".into()]).unwrap();
        assert_eq!(bytes(&r), b"hello");
        assert_eq!(t.output(), "hello\n");

        let code = by_ref();
        let args = vec![r"printf 'a  \nb\t'".into(), Value::Ref(code.clone())];
        let r = t.call("system", args).unwrap();
        assert_eq!(bytes(&r), b"b");
        assert_eq!(t.output(), "a  \nb\t");
        assert!(matches!(code.get(), Value::Int(0)));

        let args = vec!["exit 5".into(), Value::Ref(code.clone())];
        let r = t.call("system", args).unwrap();
        assert_eq!(bytes(&r), b"");
        assert_eq!(t.output(), "");
        assert!(matches!(code.get(), Value::Int(5)));
    }

    #[test]
    fn system_output_buffering() {
        // bug79410.phpt: a last chunk of exactly 4095 bytes is not lost.
        let mut t = TestVm::new();
        t.vm.ob_start(None, 0, ob::STDFLAGS).unwrap();
        let cmd = "printf '%4095s' '' | tr ' ' .";
        let r = t.call("system", vec![cmd.into()]).unwrap();
        let buffered = t.vm.ob_get_clean().unwrap().unwrap();
        assert_eq!(buffered.len(), 4095);
        assert!(buffered.iter().all(|&b| b == b'.'));
        assert_eq!(bytes(&r).len(), 4095);
        assert_eq!(t.output(), "");
    }

    // --- passthru ---------------------------------------------------------

    #[test]
    fn passthru_binary_output() {
        let mut t = TestVm::new();
        let r = t.call("passthru", vec![r"printf 'a\0b\377\n\n  '".into()]);
        assert!(matches!(r, Ok(Value::Null)));
        assert_eq!(t.out.take(), b"a\0b\xff\n\n  ");

        let code = by_ref();
        let cmd = "head -c 100000 /dev/zero; exit 2";
        let r = t.call("passthru", vec![cmd.into(), Value::Ref(code.clone())]);
        assert!(matches!(r, Ok(Value::Null)));
        let out = t.out.take();
        assert_eq!(out.len(), 100000);
        assert!(out.iter().all(|&b| b == 0));
        assert!(matches!(code.get(), Value::Int(2)));
    }

    // --- shell_exec -------------------------------------------------------

    #[test]
    fn shell_exec_output() {
        let mut t = TestVm::new();
        let r = t.call("shell_exec", vec!["echo hi".into()]).unwrap();
        assert_eq!(bytes(&r), b"hi\n");
        let r = t.call("shell_exec", vec![r"printf 'a\0b\n\n'".into()]);
        assert_eq!(bytes(&r.unwrap()), b"a\0b\n\n");
        // Empty output is null, whatever the exit status.
        let r = t.call("shell_exec", vec!["true".into()]);
        assert!(matches!(r, Ok(Value::Null)));
        let r = t.call("shell_exec", vec!["printf ''; exit 1".into()]);
        assert!(matches!(r, Ok(Value::Null)));
        let r = t.call("shell_exec", vec!["echo err 2>/dev/null >&2".into()]);
        assert!(matches!(r, Ok(Value::Null)));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn shell_exec_argument_errors() {
        let mut t = TestVm::new();
        let r = t.call("shell_exec", vec!["".into()]);
        assert_eq!(
            thrown(r),
            err(
                "ValueError",
                "shell_exec(): Argument #1 ($command) must not be empty"
            )
        );
        let r = t.call("shell_exec", vec![Value::from(&b"echo\0"[..])]);
        assert_eq!(
            thrown(r),
            err(
                "ValueError",
                "shell_exec(): Argument #1 ($command) must not contain any null bytes"
            )
        );
    }

    #[test]
    fn backtick_operator() {
        let mut t = TestVm::new();
        let r = shell_exec_bytes(&mut t.vm, b"echo `echo nested`").unwrap();
        assert_eq!(bytes(&r), b"nested\n");
        let r = shell_exec_bytes(&mut t.vm, b"exit 0");
        assert!(matches!(r, Ok(Value::Null)));
        let r = shell_exec_bytes(&mut t.vm, b"");
        assert_eq!(
            thrown(r),
            err(
                "ValueError",
                "shell_exec(): Argument #1 ($command) must not be empty"
            )
        );
    }

    // --- proc_nice --------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn proc_nice_basic() {
        // proc_nice_variation2.phpt / proc_nice_variation5.phpt, limited to
        // calls that leave the test process's priority alone.
        let mut t = TestVm::new();
        let r = t.call("proc_nice", vec![Value::Int(0)]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        let r = t.call("proc_nice", vec![Value::Bool(false)]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        let uid = t.call("shell_exec", vec!["id -u".into()]).unwrap();
        if bytes(&uid) != b"0\n" {
            let r = t.call("proc_nice", vec![Value::Int(-1)]);
            assert!(matches!(r, Ok(Value::Bool(false))));
            assert_eq!(
                t.output(),
                "\nWarning: proc_nice(): Only a super user may attempt to increase the \
                 priority of a process in /t.php on line 3\n"
            );
        }
        let r = t.call("proc_nice", vec![Value::from("abc")]);
        assert_eq!(
            thrown(r),
            err(
                "TypeError",
                "proc_nice(): Argument #1 ($priority) must be of type int, string given"
            )
        );
    }

    #[test]
    fn mblen_is_utf8() {
        assert_eq!(php_mblen(b"a"), Some(1));
        assert_eq!(php_mblen("é!".as_bytes()), Some(2));
        assert_eq!(php_mblen("€".as_bytes()), Some(3));
        assert_eq!(php_mblen("😀".as_bytes()), Some(4));
        assert_eq!(php_mblen(b"\xe2\x82"), None);
        assert_eq!(php_mblen(b"\xc1\xbf"), None);
        assert_eq!(php_mblen(b"\xe0\x80\xaf"), None);
        assert_eq!(php_mblen(b"\xbf"), None);
        assert_eq!(php_mblen(b"\xf8\x88\x80\x80\x80"), None);
    }

    #[test]
    fn functions_registered() {
        let t = TestVm::new();
        for f in [
            "exec",
            "system",
            "passthru",
            "escapeshellcmd",
            "escapeshellarg",
            "shell_exec",
        ] {
            assert!(t.vm.find_function(f).is_some(), "{f}");
        }
        assert_eq!(t.vm.find_function("proc_nice").is_some(), cfg!(unix));
    }
}
