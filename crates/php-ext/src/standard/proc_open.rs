//! ext/standard/proc_open.c — `proc_open()`, `proc_close()`,
//! `proc_get_status()`, `proc_terminate()` and the `"process"` resource.
//!
//! This is php-src's POSIX branch as built with `USE_POSIX_SPAWN` (glibc ≥
//! 2.29, macOS): a child that cannot be started is reported by the parent as
//! "posix_spawn() failed: ...". Array commands are looked up through the
//! parent's `PATH` like `posix_spawnp()`; string commands run as
//! `/bin/sh -c command` with `argv[0]` = `sh`.
//!
//! The child is started with `std::process::Command`. A `pre_exec` hook
//! performs the file actions of `close_parentends_of_pipes()` (close every
//! parent end, `dup2()` every child end onto its descriptor number), ignores
//! `SIGPIPE` the way children of the PHP CLI do, and installs the
//! `$env_vars` environment. Every descriptor this module opens is
//! close-on-exec until the hook moves it into place, so children that other
//! threads spawn concurrently do not inherit it.
//!
//! As in php-src, the process resource holds its pipe streams: closing it
//! (`proc_close()`, or dropping the last reference) closes them too, and only
//! `proc_close()` waits for the child.
//!
//! Not modelled: the Windows branch (all of `$options` is Windows-only and
//! ignored, as on POSIX) and the fork/exec fallback for systems without
//! `posix_spawn_file_actions_addchdir`.

use std::ffi::{CString, OsStr, c_char, c_int};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus, Stdio};

use php_runtime::{
    Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpArrayRef, PhpStr, Registry, ResRef,
    Value, Vm,
};

use super::streams::socket::{self, Sock};
use super::streams::stream::{self, CastAs, Stream, flags, handle_of, with_stream};
use super::streams::{self, plain, sys as ssys, wrappers};

/// Resource type name registered by `PHP_MINIT_FUNCTION(proc_open)`.
const LE_PROC_OPEN: &str = "process";

// ---------------------------------------------------------------------------
// The "process" resource
// ---------------------------------------------------------------------------

/// `php_process_handle`.
struct ProcHandle {
    child: i32,
    command: PhpStr,
    /// The pipe streams handed out in `$pipes` (`proc->pipes`).
    pipes: Vec<ResRef>,
    /// `has_cached_exit_wait_status` / `cached_exit_wait_status_value`.
    cached_exit_wait_status: Option<c_int>,
    /// The destructor already ran (`proc_close()`).
    released: bool,
}

impl ProcHandle {
    /// `waitpid_cached`: once the child has exited, its final status is
    /// remembered, because the kernel reports it only once.
    fn waitpid_cached(&mut self, options: c_int) -> io::Result<(i32, c_int)> {
        if let Some(status) = self.cached_exit_wait_status {
            return Ok((self.child, status));
        }
        let (pid, status) = ffi::waitpid(self.child, options)?;
        // Only the exit status is final: a stopped child may continue.
        if pid > 0 && ExitStatus::from_raw(status).code().is_some() {
            self.cached_exit_wait_status = Some(status);
        }
        Ok((pid, status))
    }

    /// `proc_open_rsrc_dtor`: closes the pipes ("to avoid a deadlock"), then
    /// reaps the child, blocking only for `proc_close()` (`FG(pclose_wait)`).
    /// Returns `FG(pclose_ret)`.
    fn release(&mut self, mut vm: Option<&mut Vm>, wait: bool) -> PResult<i64> {
        self.released = true;
        let mut closed = Ok(());
        for p in std::mem::take(&mut self.pipes) {
            if let (Some(vm), Ok(())) = (vm.as_deref_mut(), &closed) {
                closed = streams::close(vm, &p).map(|_| ());
            }
            if !p.is_closed() {
                p.close();
            }
        }
        closed?;
        let options = if wait { 0 } else { ffi::WNOHANG };
        let waited = loop {
            match self.waitpid_cached(options) {
                Err(e) if e.raw_os_error() == Some(ssys::EINTR) => continue,
                r => break r,
            }
        };
        Ok(match waited {
            Ok((pid, status)) if pid > 0 => match ExitStatus::from_raw(status).code() {
                Some(code) => i64::from(code),
                None => i64::from(status),
            },
            _ => -1,
        })
    }
}

impl Drop for ProcHandle {
    /// The resource destructor run when the last reference goes away.
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release(None, false);
        }
    }
}

/// Runs `f` on the process handle behind an open `"process"` resource.
fn with_process<R>(res: &ResRef, f: impl FnOnce(&mut ProcHandle) -> R) -> Option<R> {
    if res.is_closed() || res.type_name() != LE_PROC_OPEN {
        return None;
    }
    let mut data = res.resource().data.try_borrow_mut().ok()?;
    data.as_mut()?.downcast_mut::<ProcHandle>().map(f)
}

/// `Z_PARAM_RESOURCE` + `zend_fetch_resource(..., "process", le_proc_open)`.
fn process_arg(vm: &mut Vm, v: &Value) -> PResult<ResRef> {
    let v = v.unref();
    let Value::Resource(r) = v else {
        let msg = format!("must be of type resource, {} given", v.value_name());
        return Err(vm.argument_type_error(1, &msg));
    };
    if with_process(&r, |_| ()).is_some() {
        return Ok(r);
    }
    let fname = vm.current_function_name();
    Err(vm.throw_type_error(format!(
        "{fname}(): supplied resource is not a valid {LE_PROC_OPEN} resource"
    )))
}

/// `proc_terminate(resource $process, int $signal = 15): bool`.
fn proc_terminate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = process_arg(vm, args.get(0))?;
    // kill() takes an int: the zend_long is truncated as in the C call.
    let sig = args.int(1) as i32;
    let pid = with_process(&res, |p| p.child).unwrap_or(-1);
    Ok(Value::Bool(ssys::sys_kill(pid as u32, sig).is_ok()))
}

/// `proc_close(resource $process): int`.
fn proc_close(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = process_arg(vm, args.get(0))?;
    // zend_list_close(): the resource reads as "Unknown" before its
    // destructor runs.
    let Some(Ok(mut proc)) = res.close().map(|d| d.downcast::<ProcHandle>()) else {
        return Ok(Value::Int(-1));
    };
    Ok(Value::Int(proc.release(Some(vm), true)?))
}

/// `proc_get_status(resource $process): array`.
fn proc_get_status(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = process_arg(vm, args.get(0))?;
    let Some((command, child, waited, cached)) = with_process(&res, |p| {
        let waited = p.waitpid_cached(ffi::WNOHANG | ffi::WUNTRACED);
        let cached = p.cached_exit_wait_status.is_some();
        (p.command.clone(), p.child, waited, cached)
    }) else {
        return Ok(Value::from(PhpArray::new()));
    };

    let (mut running, mut signaled, mut stopped) = (true, false, false);
    let (mut exitcode, mut termsig, mut stopsig) = (-1i64, 0i64, 0i64);
    match waited {
        Ok((pid, status)) if pid == child => {
            let st = ExitStatus::from_raw(status);
            if let Some(code) = st.code() {
                running = false;
                exitcode = i64::from(code);
            }
            if let Some(sig) = st.signal() {
                running = false;
                signaled = true;
                termsig = i64::from(sig);
            }
            if let Some(sig) = st.stopped_signal() {
                stopped = true;
                stopsig = i64::from(sig);
            }
        }
        Ok(_) => {}
        // The only possible error is ECHILD: the PID does not exist or is
        // not a child of this process.
        Err(_) => running = false,
    }

    let mut a = PhpArray::new();
    a.insert_str("command", command);
    a.insert_str("pid", i64::from(child));
    a.insert_str("cached", cached);
    a.insert_str("running", running);
    a.insert_str("signaled", signaled);
    a.insert_str("stopped", stopped);
    a.insert_str("exitcode", exitcode);
    a.insert_str("termsig", termsig);
    a.insert_str("stopsig", stopsig);
    Ok(Value::from(a))
}

// ---------------------------------------------------------------------------
// Descriptor specs
// ---------------------------------------------------------------------------

/// `descriptor_type`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DescriptorType {
    Std,
    Pipe,
    Socket,
}

/// `descriptorspec_item`.
struct Descriptor {
    /// Desired descriptor number in the child.
    index: i32,
    ty: DescriptorType,
    /// Opened for the child (copied to `index` there).
    childend: OwnedFd,
    /// The parent's end (pipes and sockets only).
    parentend: Option<OwnedFd>,
    /// Mode of the parent end's stream (from `mode_flags`).
    mode: &'static [u8],
}

impl Descriptor {
    fn std(index: i32, childend: OwnedFd) -> Descriptor {
        Descriptor {
            index,
            ty: DescriptorType::Std,
            childend,
            parentend: None,
            mode: b"",
        }
    }
}

/// The bytes a C string API sees: everything before the first NUL.
fn until_nul(b: &[u8]) -> &[u8] {
    b.iter().position(|&c| c == 0).map_or(b, |i| &b[..i])
}

fn errno_text(e: &io::Error) -> String {
    ssys::strerror(ssys::errno_of(e))
}

/// `get_string_parameter`.
fn string_parameter(vm: &mut Vm, ht: &PhpArray, index: i64, name: &str) -> PResult<PhpStr> {
    let Some(v) = ht.get_int(index).cloned() else {
        return Err(vm.throw_value_error(format!("Missing {name}")));
    };
    vm.to_php_string(&v)
}

/// `dup_proc_descriptor`.
fn dup_proc_descriptor(vm: &mut Vm, from: RawFd, nindex: i64) -> PResult<Option<OwnedFd>> {
    match ffi::dup_cloexec(from) {
        Ok(fd) => Ok(Some(fd)),
        Err(e) => {
            let msg = format!(
                "Failed to dup() for descriptor {nindex}: {}",
                errno_text(&e)
            );
            vm.warning(msg)?;
            Ok(None)
        }
    }
}

/// `php_stream_cast(stream, PHP_STREAM_AS_FD, &fd, REPORT_ERRORS)`.
fn cast_to_fd(vm: &mut Vm, s: &mut Stream) -> PResult<Option<RawFd>> {
    // Synchronise the buffer so the descriptor's offset is the stream's.
    s.flush(vm)?;
    if s.is_filtered() {
        stream::stream_warn(vm, &s.st, "Cannot cast a filtered stream on this system")?;
        return Ok(None);
    }
    if s.ops.can_seek() && s.st.flags & flags::NO_SEEK == 0 {
        let pos = s.st.position;
        s.seek(vm, pos, 0)?;
    }
    let Some(fd) = s.cast_fd(CastAs::Fd) else {
        let msg = format!(
            "Cannot represent a stream of type {} as a File Descriptor",
            s.ops.label()
        );
        stream::stream_warn(vm, &s.st, &msg)?;
        return Ok(None);
    };
    let lost = s.buffered();
    if lost > 0 {
        let msg = format!("{lost} bytes of buffered data lost during stream conversion!");
        stream::stream_warn(vm, &s.st, &msg)?;
    }
    Ok(Some(fd))
}

/// `set_proc_descriptor_from_resource`: a stream's descriptor, duplicated.
/// (php-src reports a failed dup() with the descriptor *count*, `ndesc`.)
fn descriptor_from_resource(
    vm: &mut Vm,
    r: &ResRef,
    index: i32,
    ndesc: usize,
) -> PResult<Option<Descriptor>> {
    // zend_fetch_resource(..., "stream", php_file_le_stream()): persistent
    // streams do not qualify.
    if r.type_name() != "stream" || handle_of(r).is_none() {
        let fname = vm.current_function_name();
        return Err(vm.throw_type_error(format!(
            "{fname}(): supplied resource is not a valid stream resource"
        )));
    }
    let Some(fd) = with_stream(vm, r, cast_to_fd)?.flatten() else {
        return Ok(None);
    };
    Ok(dup_proc_descriptor(vm, fd, ndesc as i64)?.map(|fd| Descriptor::std(index, fd)))
}

/// `set_proc_descriptor_to_pipe`.
fn pipe_descriptor(vm: &mut Vm, index: i32, mode: &PhpStr) -> PResult<Option<Descriptor>> {
    let (read, write) = match ffi::pipe_cloexec() {
        Ok(p) => p,
        Err(e) => {
            vm.warning(format!("Unable to create pipe {}", errno_text(&e)))?;
            return Ok(None);
        }
    };
    // Anything but a mode starting with "w" is the child's stdin side.
    let (childend, parentend, mode): (OwnedFd, OwnedFd, &'static [u8]) =
        if !mode.as_bytes().starts_with(b"w") {
            (read, write, b"w")
        } else {
            (write, read, b"r")
        };
    Ok(Some(Descriptor {
        index,
        ty: DescriptorType::Pipe,
        childend,
        parentend: Some(parentend),
        mode,
    }))
}

/// `set_proc_descriptor_to_socket`.
fn socket_descriptor(vm: &mut Vm, index: i32) -> PResult<Option<Descriptor>> {
    match UnixStream::pair() {
        Ok((parent, child)) => Ok(Some(Descriptor {
            index,
            ty: DescriptorType::Socket,
            childend: child.into(),
            parentend: Some(parent.into()),
            mode: b"r+",
        })),
        Err(e) => {
            vm.warning(format!("Unable to create socket pair: {}", errno_text(&e)))?;
            Ok(None)
        }
    }
}

/// `set_proc_descriptor_to_file`: opened through the stream layer and cast
/// to its descriptor (`PHP_STREAM_CAST_RELEASE`).
fn file_descriptor(
    vm: &mut Vm,
    index: i32,
    path: &PhpStr,
    mode: &PhpStr,
) -> PResult<Option<Descriptor>> {
    let opts = stream::open::REPORT_ERRORS | stream::open::STREAM_WILL_CAST;
    let path = until_nul(path.as_bytes());
    let mode = until_nul(mode.as_bytes());
    let Some(res) = wrappers::open_wrapper(vm, path, mode, opts, None, None)? else {
        return Ok(None);
    };
    let Some(fd) = with_stream(vm, &res, cast_to_fd)?.flatten() else {
        return Ok(None);
    };
    // Releasing the stream keeps its descriptor: take a copy, then free it.
    let fd = dup_proc_descriptor(vm, fd, i64::from(index))?;
    streams::close(vm, &res)?;
    Ok(fd.map(|fd| Descriptor::std(index, fd)))
}

/// `redirect_proc_descriptor`.
fn redirect_descriptor(
    vm: &mut Vm,
    ht: &PhpArray,
    descriptors: &[Descriptor],
    index: i32,
) -> PResult<Option<Descriptor>> {
    let target = match ht.get_int(1).map(Value::unref) {
        None => return Err(vm.throw_value_error("Missing redirection target")),
        Some(Value::Int(t)) => t as i32,
        Some(other) => {
            return Err(vm.throw_value_error(format!(
                "Redirection target must be of type int, {} given",
                other.value_name()
            )));
        }
    };
    let found = descriptors
        .iter()
        .find(|d| d.index == target)
        .map(|d| d.childend.as_raw_fd());
    let redirect_to = match found {
        Some(fd) => fd,
        // A stdin/stdout/stderr inherited from the parent.
        None if (0..=2).contains(&target) => target,
        None => {
            vm.warning(format!("Redirection target {target} not found"))?;
            return Ok(None);
        }
    };
    Ok(
        dup_proc_descriptor(vm, redirect_to, i64::from(index))?
            .map(|fd| Descriptor::std(index, fd)),
    )
}

/// `set_proc_descriptor_to_blackhole`.
fn null_descriptor(vm: &mut Vm, index: i32) -> PResult<Option<Descriptor>> {
    match OpenOptions::new().read(true).write(true).open("/dev/null") {
        Ok(f) => Ok(Some(Descriptor::std(index, f.into()))),
        Err(e) => {
            vm.warning(format!("Failed to open /dev/null: {}", errno_text(&e)))?;
            Ok(None)
        }
    }
}

/// `set_proc_descriptor_to_pty`: every "pty" descriptor shares one
/// pseudoterminal — the child gets its slave end, `$pipes` its master end.
fn pty_descriptor(
    vm: &mut Vm,
    index: i32,
    pty: &mut Option<(OwnedFd, OwnedFd)>,
) -> PResult<Option<Descriptor>> {
    if pty.is_none() {
        match ffi::open_pty() {
            Ok(p) => *pty = Some(p),
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                vm.warning("PTY (pseudoterminal) not supported on this system")?;
                return Ok(None);
            }
            Err(e) => {
                let msg = format!("Could not open PTY (pseudoterminal): {}", errno_text(&e));
                vm.warning(msg)?;
                return Ok(None);
            }
        }
    }
    let Some((master, slave)) = pty.as_ref() else {
        return Ok(None);
    };
    match (
        ffi::dup_cloexec(slave.as_raw_fd()),
        ffi::dup_cloexec(master.as_raw_fd()),
    ) {
        (Ok(childend), Ok(parentend)) => Ok(Some(Descriptor {
            index,
            ty: DescriptorType::Pipe,
            childend,
            parentend: Some(parentend),
            mode: b"r+",
        })),
        (Err(e), _) | (_, Err(e)) => {
            let msg = format!("Could not open PTY (pseudoterminal): {}", errno_text(&e));
            vm.warning(msg)?;
            Ok(None)
        }
    }
}

/// `set_proc_descriptor_from_array`.
fn descriptor_from_array(
    vm: &mut Vm,
    ht: &PhpArray,
    descriptors: &[Descriptor],
    index: i32,
    pty: &mut Option<(OwnedFd, OwnedFd)>,
) -> PResult<Option<Descriptor>> {
    let ztype = string_parameter(vm, ht, 0, "handle qualifier")?;
    match ztype.as_bytes() {
        b"pipe" => {
            let mode = string_parameter(vm, ht, 1, "mode parameter for 'pipe'")?;
            pipe_descriptor(vm, index, &mode)
        }
        b"socket" => socket_descriptor(vm, index),
        b"file" => {
            let file = string_parameter(vm, ht, 1, "file name parameter for 'file'")?;
            let mode = string_parameter(vm, ht, 2, "mode parameter for 'file'")?;
            file_descriptor(vm, index, &file, &mode)
        }
        b"redirect" => redirect_descriptor(vm, ht, descriptors, index),
        b"null" => null_descriptor(vm, index),
        b"pty" => pty_descriptor(vm, index, pty),
        other => {
            let mut msg = until_nul(other).to_vec();
            msg.extend_from_slice(b" is not a valid descriptor spec/mode");
            vm.warning(msg)?;
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Command, environment and spawning
// ---------------------------------------------------------------------------

/// `get_valid_arg_string`.
fn valid_arg_string(vm: &mut Vm, v: &Value, elem_num: usize) -> PResult<PhpStr> {
    let s = vm.to_php_string(v)?;
    if elem_num == 1 && s.is_empty() {
        return Err(vm.throw_value_error("First element must contain a non-empty program name"));
    }
    if s.as_bytes().contains(&0) {
        return Err(vm.throw_value_error(format!(
            "Command array element {elem_num} contains a null byte"
        )));
    }
    Ok(s)
}

/// `get_command_from_array`: the program name and `argv`.
fn command_from_array(vm: &mut Vm, a: &PhpArray) -> PResult<(PhpStr, Vec<Vec<u8>>)> {
    let mut argv = Vec::with_capacity(a.len());
    let mut command = PhpStr::new();
    for (i, (_, v)) in a.iter().enumerate() {
        let s = valid_arg_string(vm, v, i + 1)?;
        if i == 0 {
            command = s.clone();
        }
        argv.push(s.as_bytes().to_vec());
    }
    Ok((command, argv))
}

/// The `envp` block built by `php_array_to_envp`, owned by the `pre_exec`
/// hook that installs it as `environ`.
struct EnvBlock {
    _entries: Vec<CString>,
    /// NULL-terminated pointers into `_entries`.
    ptrs: Vec<*const c_char>,
}

// SAFETY: the pointers only refer to the heap buffers of the `CString`s
// owned by the same block, which are never mutated; the block is read only
// by the forked child.
unsafe impl Send for EnvBlock {}
// SAFETY: as above — the block is immutable once built.
unsafe impl Sync for EnvBlock {}

/// `php_array_to_envp`: values are converted to strings, empty ones are
/// skipped, string keys give `key=value` and other entries the bare value.
fn env_block(vm: &mut Vm, env: &PhpArray) -> PResult<EnvBlock> {
    let mut entries = Vec::with_capacity(env.len());
    for (key, v) in env.iter() {
        let s = vm.to_php_string(v)?;
        if s.is_empty() {
            continue;
        }
        let mut entry = Vec::new();
        match &key {
            ArrayKey::Str(k) if !k.is_empty() => {
                entry.extend_from_slice(k.as_bytes());
                entry.push(b'=');
            }
            _ => {}
        }
        entry.extend_from_slice(s.as_bytes());
        let entry = until_nul(&entry).to_vec();
        entries.push(CString::new(entry).expect("no NUL bytes left"));
    }
    let mut ptrs: Vec<*const c_char> = entries.iter().map(|e| e.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    Ok(EnvBlock {
        _entries: entries,
        ptrs,
    })
}

/// `posix_spawnp()`'s program lookup: a name without a slash is searched in
/// the parent's `PATH`. `Err(errno)` when nothing executable is found.
fn resolve_program(prog: &[u8]) -> Result<Vec<u8>, i32> {
    if prog.contains(&b'/') {
        return Ok(prog.to_vec());
    }
    #[cfg(target_vendor = "apple")]
    const DEFAULT_PATH: &[u8] = b"/usr/bin:/bin";
    #[cfg(not(target_vendor = "apple"))]
    const DEFAULT_PATH: &[u8] = b"/bin:/usr/bin";
    let path = std::env::var_os("PATH").map_or_else(|| DEFAULT_PATH.to_vec(), |p| p.into_vec());
    let mut err = ssys::ENOENT;
    for dir in path.split(|&b| b == b':') {
        let mut cand = if dir.is_empty() {
            b"./".to_vec()
        } else {
            let mut d = dir.to_vec();
            d.push(b'/');
            d
        };
        cand.extend_from_slice(prog);
        if plain::os(&cand).is_file() {
            if ssys::sys_access(&cand, ssys::X_OK).is_ok() {
                return Ok(cand);
            }
            err = ssys::EACCES;
        }
    }
    Err(err)
}

/// One `close_parentends_of_pipes` step, by raw descriptor numbers.
#[derive(Clone, Copy)]
struct ChildAction {
    index: RawFd,
    childend: RawFd,
    parentend: Option<RawFd>,
}

/// Runs in the forked child right before `exec`. Only async-signal-safe
/// calls; nothing is allocated.
fn child_setup(actions: &[ChildAction], env: Option<&EnvBlock>) -> io::Result<()> {
    for a in actions {
        if let Some(p) = a.parentend {
            ffi::close(p);
        }
        if a.childend != a.index {
            ffi::dup2(a.childend, a.index)?;
            ffi::close(a.childend);
        } else {
            // Already in place, but close-on-exec like everything we open.
            ffi::clear_cloexec(a.index)?;
        }
    }
    ffi::ignore_sigpipe();
    if let Some(env) = env {
        ffi::set_environ(env.ptrs.as_ptr());
    }
    Ok(())
}

/// The `USE_POSIX_SPAWN` block of `proc_open()`: `Some(pid)`, or `None`
/// after a warning.
fn spawn(
    vm: &mut Vm,
    command: &PhpStr,
    argv: Option<&[Vec<u8>]>,
    cwd: Option<&PhpStr>,
    env: Option<EnvBlock>,
    descriptors: &[Descriptor],
) -> PResult<Option<i32>> {
    // close_parentends_of_pipes(): posix_spawn_file_actions_adddup2()
    // rejects descriptor numbers outside the descriptor table.
    let table_size = ssys::sys_dtablesize();
    let mut actions = Vec::with_capacity(descriptors.len());
    for d in descriptors {
        let childend = d.childend.as_raw_fd();
        if childend != d.index && (d.index < 0 || d.index >= table_size) {
            vm.warning(format!(
                "Unable to copy file descriptor {childend} (for pipe) into file descriptor {}: {}",
                d.index,
                ssys::strerror(ssys::EBADF)
            ))?;
            return Ok(None);
        }
        actions.push(ChildAction {
            index: d.index,
            childend,
            parentend: d.parentend.as_ref().map(|p| p.as_raw_fd()),
        });
    }

    let mut cmd = match argv {
        Some(argv) => {
            let prog = match resolve_program(&argv[0]) {
                Ok(p) => p,
                Err(errno) => {
                    vm.warning(format!("posix_spawn() failed: {}", ssys::strerror(errno)))?;
                    return Ok(None);
                }
            };
            let mut c = Command::new(OsStr::from_bytes(&prog));
            c.arg0(OsStr::from_bytes(&argv[0]));
            c.args(argv[1..].iter().map(|a| OsStr::from_bytes(a)));
            c
        }
        None => {
            let mut c = Command::new("/bin/sh");
            c.arg0("sh")
                .arg("-c")
                .arg(OsStr::from_bytes(until_nul(command.as_bytes())));
            c
        }
    };
    if let Some(cwd) = cwd {
        cmd.current_dir(plain::os(cwd.as_bytes()));
    }
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // SAFETY: the hook runs in the forked child before exec and only makes
    // async-signal-safe calls (close, dup2, fcntl, signal) plus a store to
    // `environ`; the environment block it installs was built beforehand.
    unsafe {
        cmd.pre_exec(move || child_setup(&actions, env.as_ref()));
    }
    match cmd.spawn() {
        // The pid is reaped with waitpid(); dropping `Child` does not wait.
        Ok(child) => Ok(Some(child.id() as i32)),
        Err(e) => {
            vm.warning(format!("posix_spawn() failed: {}", errno_text(&e)))?;
            Ok(None)
        }
    }
}

/// `proc_open(array|string $command, array $descriptor_spec, &$pipes, ...)`.
fn proc_open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let command = args.get(0).unref();
    let descriptorspec: PhpArrayRef = args.array(1).clone();
    let pipes = args.by_ref(2);
    // Z_PARAM_PATH_OR_NULL
    let cwd = if args.is_null(3) {
        None
    } else {
        Some(streams::path_param(vm, args, 3)?)
    };
    let environment = args.array_opt(4).cloned();
    // $options: every option proc_open.c reads is Windows-only.

    let (command_str, argv) = match &command {
        Value::Array(a) => {
            if a.is_empty() {
                return Err(vm.argument_value_error(1, "must not be empty"));
            }
            let (c, argv) = command_from_array(vm, a)?;
            (c, Some(argv))
        }
        other => (vm.to_php_string(other)?, None),
    };

    let env = match &environment {
        Some(e) => Some(env_block(vm, e)?),
        None => None,
    };

    // Walk the descriptor spec and set up files/pipes. On failure the
    // descriptors opened so far are closed as they drop
    // (close_all_descriptors).
    let mut descriptors: Vec<Descriptor> = Vec::with_capacity(descriptorspec.len());
    let mut pty: Option<(OwnedFd, OwnedFd)> = None;
    for (key, item) in descriptorspec.iter() {
        let ArrayKey::Int(nindex) = key else {
            return Err(vm.argument_value_error(2, "must be an integer indexed array"));
        };
        let index = nindex as i32;
        let d = match item.unref() {
            Value::Resource(r) => descriptor_from_resource(vm, &r, index, descriptors.len())?,
            Value::Array(ht) => descriptor_from_array(vm, &ht, &descriptors, index, &mut pty)?,
            _ => {
                return Err(vm.argument_value_error(2, "must only contain arrays and streams"));
            }
        };
        let Some(d) = d else {
            return Ok(Value::Bool(false));
        };
        descriptors.push(d);
    }

    let Some(child) = spawn(
        vm,
        &command_str,
        argv.as_deref(),
        cwd.as_ref(),
        env,
        &descriptors,
    )?
    else {
        return Ok(Value::Bool(false));
    };
    drop(pty);

    // We spawned and this is the parent: close the child ends and open
    // streams on the parent ends.
    let mut pipes_arr = PhpArray::new();
    let mut proc_pipes = Vec::new();
    for d in descriptors {
        let Descriptor {
            index,
            ty,
            childend,
            parentend,
            mode,
        } = d;
        drop(childend);
        let Some(parentend) = parentend else {
            continue;
        };
        let res = match ty {
            DescriptorType::Pipe => plain::fopen_from_fd(vm, File::from(parentend), mode, false),
            DescriptorType::Socket => {
                let timeout = socket::default_timeout(vm);
                let sock = Sock::Unix(UnixStream::from(parentend));
                socket::alloc_socket(vm, sock, "generic_socket", timeout, false)
            }
            DescriptorType::Std => continue,
        };
        // "nasty hack; don't copy it"
        if let Some(h) = handle_of(&res) {
            h.borrow_mut().st.flags |= flags::NO_SEEK;
        }
        pipes_arr.insert(
            ArrayKey::Int(i64::from(index)),
            Value::Resource(res.clone()),
        );
        proc_pipes.push(res);
    }
    pipes.set(Value::from(pipes_arr));

    let proc = ProcHandle {
        child,
        command: command_str,
        pipes: proc_pipes,
        cached_exit_wait_status: None,
        released: false,
    };
    Ok(Value::Resource(ResRef::new(LE_PROC_OPEN, Box::new(proc))))
}

// ---------------------------------------------------------------------------
// libc bindings
// ---------------------------------------------------------------------------

/// Minimal bindings for the POSIX calls std does not expose (`pipe`,
/// `fcntl`, `dup2`, `close`, `waitpid`, `signal`, the `posix_openpt` family
/// and `environ`). std already links the C library, so these are plain
/// `extern "C"` declarations behind safe wrappers.
mod ffi {
    use std::ffi::{CStr, c_char, c_int};
    use std::io;
    use std::os::fd::{FromRawFd, OwnedFd, RawFd};
    use std::sync::Mutex;

    pub const WNOHANG: c_int = 1;
    pub const WUNTRACED: c_int = 2;

    const F_SETFD: c_int = 2;
    const FD_CLOEXEC: c_int = 1;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const F_DUPFD_CLOEXEC: c_int = 1030;
    #[cfg(target_vendor = "apple")]
    const F_DUPFD_CLOEXEC: c_int = 67;
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
    const F_DUPFD_CLOEXEC: c_int = 17;
    #[cfg(target_os = "netbsd")]
    const F_DUPFD_CLOEXEC: c_int = 12;
    #[cfg(target_os = "openbsd")]
    const F_DUPFD_CLOEXEC: c_int = 10;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_CLOEXEC: c_int = 0o2_000_000;

    const O_RDWR: c_int = 2;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const O_NOCTTY: c_int = 0o400;
    #[cfg(target_vendor = "apple")]
    const O_NOCTTY: c_int = 0x20000;
    #[cfg(any(
        target_os = "freebsd",
        target_os = "dragonfly",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    const O_NOCTTY: c_int = 0x8000;

    const SIGPIPE: c_int = 13;
    const SIG_IGN: usize = 1;

    unsafe extern "C" {
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        #[link_name = "pipe"]
        fn c_pipe(fds: *mut c_int) -> c_int;
        #[cfg(any(target_os = "linux", target_os = "android"))]
        fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        #[link_name = "dup2"]
        fn c_dup2(src: c_int, dst: c_int) -> c_int;
        #[link_name = "close"]
        fn c_close(fd: c_int) -> c_int;
        #[link_name = "waitpid"]
        fn c_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
        fn signal(signum: c_int, handler: usize) -> usize;
        fn posix_openpt(flags: c_int) -> c_int;
        fn grantpt(fd: c_int) -> c_int;
        fn unlockpt(fd: c_int) -> c_int;
        fn ptsname(fd: c_int) -> *mut c_char;
    }

    #[cfg(target_vendor = "apple")]
    unsafe extern "C" {
        fn _NSGetEnviron() -> *mut *const *const c_char;
    }

    #[cfg(not(target_vendor = "apple"))]
    unsafe extern "C" {
        static mut environ: *const *const c_char;
    }

    fn check(r: c_int) -> io::Result<c_int> {
        if r == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(r)
        }
    }

    /// Takes ownership of a descriptor a successful call just returned.
    fn owned(fd: c_int) -> OwnedFd {
        // SAFETY: `fd` was just returned by the kernel and nothing else owns it.
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    /// `FD_CLOEXEC` on `fd`.
    fn set_cloexec(fd: RawFd) -> io::Result<()> {
        // SAFETY: F_SETFD takes an int argument.
        check(unsafe { fcntl(fd, F_SETFD, FD_CLOEXEC) }).map(|_| ())
    }

    /// Clears `FD_CLOEXEC` (async-signal-safe).
    pub fn clear_cloexec(fd: RawFd) -> io::Result<()> {
        // SAFETY: F_SETFD takes an int argument.
        check(unsafe { fcntl(fd, F_SETFD, 0) }).map(|_| ())
    }

    /// `pipe()` with both ends close-on-exec: `(read end, write end)`.
    pub fn pipe_cloexec() -> io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as c_int; 2];
        #[cfg(any(target_os = "linux", target_os = "android"))]
        // SAFETY: `fds` has room for the two descriptors.
        check(unsafe { pipe2(fds.as_mut_ptr(), O_CLOEXEC) })?;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        // SAFETY: `fds` has room for the two descriptors.
        check(unsafe { c_pipe(fds.as_mut_ptr()) })?;
        let (r, w) = (owned(fds[0]), owned(fds[1]));
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            use std::os::fd::AsRawFd;
            set_cloexec(r.as_raw_fd())?;
            set_cloexec(w.as_raw_fd())?;
        }
        Ok((r, w))
    }

    /// `dup()` with the copy close-on-exec (lowest free number, like dup()).
    pub fn dup_cloexec(fd: RawFd) -> io::Result<OwnedFd> {
        // SAFETY: F_DUPFD_CLOEXEC takes an int argument; an invalid `fd`
        // yields EBADF.
        check(unsafe { fcntl(fd, F_DUPFD_CLOEXEC, 0) }).map(owned)
    }

    /// `dup2()` (async-signal-safe).
    pub fn dup2(src: RawFd, dst: RawFd) -> io::Result<()> {
        // SAFETY: dup2 only takes integers.
        check(unsafe { c_dup2(src, dst) }).map(|_| ())
    }

    /// `close()`, ignoring errors (async-signal-safe).
    pub fn close(fd: RawFd) {
        // SAFETY: only used in the forked child on descriptors it inherited.
        unsafe {
            c_close(fd);
        }
    }

    /// `waitpid(pid, &status, options)`: `(pid or 0, status)`.
    pub fn waitpid(pid: c_int, options: c_int) -> io::Result<(c_int, c_int)> {
        let mut status: c_int = 0;
        // SAFETY: `status` is a valid out-pointer for the call.
        let r = check(unsafe { c_waitpid(pid, &mut status, options) })?;
        Ok((r, status))
    }

    /// `signal(SIGPIPE, SIG_IGN)` (async-signal-safe).
    pub fn ignore_sigpipe() {
        // SAFETY: SIG_IGN is a valid disposition for SIGPIPE.
        unsafe {
            signal(SIGPIPE, SIG_IGN);
        }
    }

    /// Installs `envp` as the process environment (`environ = envp`).
    pub fn set_environ(envp: *const *const c_char) {
        // SAFETY: only called in the forked child, which has a single thread;
        // `envp` is a NULL-terminated array that outlives the exec call.
        #[cfg(target_vendor = "apple")]
        unsafe {
            *_NSGetEnviron() = envp;
        }
        // SAFETY: as above.
        #[cfg(not(target_vendor = "apple"))]
        unsafe {
            environ = envp;
        }
    }

    /// `ptsname()` returns a static buffer; serialise our calls to it.
    static PTSNAME_LOCK: Mutex<()> = Mutex::new(());

    /// `openpty()` built from `posix_openpt` (php-src's Solaris fallback):
    /// `(master, slave)`, both close-on-exec.
    pub fn open_pty() -> io::Result<(OwnedFd, OwnedFd)> {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::OpenOptionsExt;

        // SAFETY: posix_openpt only takes flags.
        let master = owned(check(unsafe { posix_openpt(O_RDWR | O_NOCTTY) })?);
        let m = master.as_raw_fd();
        set_cloexec(m)?;
        // SAFETY: grantpt/unlockpt only take the descriptor.
        check(unsafe { grantpt(m) })?;
        // SAFETY: as above.
        check(unsafe { unlockpt(m) })?;
        let name = {
            let _guard = PTSNAME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: ptsname returns NULL or a NUL-terminated static
            // buffer, copied out while the lock is held.
            let p = unsafe { ptsname(m) };
            if p.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `p` is a valid C string (checked non-NULL above).
            unsafe { CStr::from_ptr(p) }.to_bytes().to_vec()
        };
        let slave = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NOCTTY)
            .open(std::ffi::OsStr::from_bytes(&name))?;
        Ok((master, slave.into()))
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static PROC_OPEN: NativeFunction = NativeFunction {
    sig: "proc_open(array|string $command, array $descriptor_spec, &$pipes, ?string $cwd = null, ?array $env_vars = null, ?array $options = null)",
    handler: proc_open,
};

static PROC_CLOSE: NativeFunction = NativeFunction {
    sig: "proc_close($process): int",
    handler: proc_close,
};

static PROC_TERMINATE: NativeFunction = NativeFunction {
    sig: "proc_terminate($process, int $signal = 15): bool",
    handler: proc_terminate,
};

static PROC_GET_STATUS: NativeFunction = NativeFunction {
    sig: "proc_get_status($process): array",
    handler: proc_get_status,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&PROC_OPEN, &PROC_CLOSE, &PROC_TERMINATE, &PROC_GET_STATUS]);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::PhpRef;

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    fn list(items: Vec<Value>) -> Value {
        Value::from(PhpArray::from_values(items))
    }

    fn strs(items: &[&str]) -> Value {
        list(items.iter().map(|v| s(v)).collect())
    }

    fn spec(items: Vec<(i64, Value)>) -> Value {
        let mut a = PhpArray::new();
        for (k, v) in items {
            a.insert(ArrayKey::Int(k), v);
        }
        Value::from(a)
    }

    fn pipe(mode: &str) -> Value {
        strs(&["pipe", mode])
    }

    /// `proc_open($cmd, $spec, $pipes, ...$rest)`.
    fn open(t: &mut TestVm, cmd: Value, spec: Value, rest: Vec<Value>) -> (PResult<Value>, PhpRef) {
        let pipes = PhpRef::new(Value::Null);
        let mut args = vec![cmd, spec, Value::Ref(pipes.clone())];
        args.extend(rest);
        (t.call("proc_open", args), pipes)
    }

    fn pipes_of(pipes: &PhpRef) -> Vec<(i64, ResRef)> {
        let Value::Array(a) = pipes.get() else {
            panic!("$pipes is not an array: {:?}", pipes.get());
        };
        a.iter()
            .map(|(k, v)| match (k, v) {
                (ArrayKey::Int(i), Value::Resource(r)) => (i, r.clone()),
                other => panic!("unexpected $pipes entry {other:?}"),
            })
            .collect()
    }

    fn pipe_at(pipes: &PhpRef, i: i64) -> ResRef {
        pipes_of(pipes)
            .into_iter()
            .find(|(k, _)| *k == i)
            .map(|(_, r)| r)
            .unwrap_or_else(|| panic!("no pipe {i}"))
    }

    fn read_all(t: &mut TestVm, r: &ResRef) -> String {
        let data = streams::read_to_end(&mut t.vm, r)
            .unwrap()
            .unwrap_or_default();
        String::from_utf8_lossy(&data).into_owned()
    }

    fn status(t: &mut TestVm, proc: &Value) -> PhpArray {
        match t.call("proc_get_status", vec![proc.clone()]).unwrap() {
            Value::Array(a) => (*a).clone(),
            other => panic!("proc_get_status returned {other:?}"),
        }
    }

    fn field(a: &PhpArray, k: &str) -> Value {
        a.get_str(k.as_bytes()).cloned().unwrap_or(Value::Undef)
    }

    /// Polls `proc_get_status()` until the child is no longer running.
    fn wait_stopped_running(t: &mut TestVm, proc: &Value) -> PhpArray {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let st = status(t, proc);
            if matches!(field(&st, "running"), Value::Bool(false)) || Instant::now() > deadline {
                return st;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn close(t: &mut TestVm, proc: &Value) -> i64 {
        match t.call("proc_close", vec![proc.clone()]).unwrap() {
            Value::Int(n) => n,
            other => panic!("proc_close returned {other:?}"),
        }
    }

    fn temp_path(name: &str) -> String {
        let dir = std::env::temp_dir();
        format!(
            "{}/php_proc_open_{}_{name}",
            dir.display(),
            std::process::id()
        )
    }

    fn warning(msg: &str) -> String {
        format!("\nWarning: proc_open(): {msg} in /t.php on line 3\n")
    }

    // proc_open.phpt, bug44667.phpt
    #[test]
    fn cat_round_trip() {
        let mut t = TestVm::new();
        let ds = spec(vec![(0, pipe("r")), (1, pipe("w")), (2, pipe("w"))]);
        let (proc, pipes) = open(&mut t, s("/bin/cat"), ds, vec![]);
        let proc = proc.unwrap();
        assert!(matches!(&proc, Value::Resource(r) if r.type_name() == "process"));
        let keys: Vec<i64> = pipes_of(&pipes).iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![0, 1, 2]);
        let modes: Vec<Vec<u8>> = pipes_of(&pipes)
            .iter()
            .map(|(_, r)| handle_of(r).unwrap().borrow().st.mode.as_bytes().to_vec())
            .collect();
        assert_eq!(modes, vec![b"w".to_vec(), b"r".to_vec(), b"r".to_vec()]);
        for (_, r) in pipes_of(&pipes) {
            let f = handle_of(&r).unwrap().borrow().st.flags;
            assert_ne!(f & flags::NO_SEEK, 0);
        }

        let stdin = pipe_at(&pipes, 0);
        assert_eq!(
            streams::write(&mut t.vm, &stdin, b"Hello").unwrap(),
            Some(5)
        );
        assert!(streams::close(&mut t.vm, &stdin).unwrap());
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "Hello");
        assert_eq!(close(&mut t, &proc), 0);
        assert_eq!(t.output(), "");

        // 'rb' / 'wb' modes (bug #44667).
        let ds = spec(vec![(0, pipe("rb")), (1, pipe("wb"))]);
        let (proc, pipes) = open(&mut t, s("cat"), ds, vec![]);
        let proc = proc.unwrap();
        let stdin = pipe_at(&pipes, 0);
        streams::write(&mut t.vm, &stdin, b"Hello").unwrap();
        streams::close(&mut t.vm, &stdin).unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "Hello");
        assert_eq!(close(&mut t, &proc), 0);
    }

    #[test]
    fn proc_close_returns_exit_code_and_closes_pipes() {
        let mut t = TestVm::new();
        let ds = spec(vec![(0, pipe("r")), (1, pipe("w"))]);
        let (proc, pipes) = open(&mut t, s("cat >/dev/null; exit 3"), ds, vec![]);
        let proc = proc.unwrap();
        // proc_close() closes the pipes first, so cat sees EOF.
        assert_eq!(close(&mut t, &proc), 3);
        for (_, r) in pipes_of(&pipes) {
            assert_eq!(r.type_name(), "Unknown");
        }
        assert!(matches!(&proc, Value::Resource(r) if r.type_name() == "Unknown"));
        let (class, msg) = thrown(t.call("proc_close", vec![proc.clone()]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            msg,
            "proc_close(): supplied resource is not a valid process resource"
        );
        let (_, msg) = thrown(t.call("proc_get_status", vec![proc]));
        assert_eq!(
            msg,
            "proc_get_status(): supplied resource is not a valid process resource"
        );
    }

    // proc_open_pipes1.phpt: the discarded process resource closes its pipes.
    #[test]
    fn dropping_the_process_closes_its_pipes() {
        let mut t = TestVm::new();
        let ds = spec((3..=30).map(|i| (i, pipe("w"))).collect());
        let (proc, pipes) = open(&mut t, s("exit 0"), ds, vec![]);
        let proc = proc.unwrap();
        let all = pipes_of(&pipes);
        assert_eq!(all.len(), 28);
        assert!(all.iter().all(|(_, r)| r.type_name() == "stream"));
        drop(proc);
        assert!(all.iter().all(|(_, r)| r.type_name() == "Unknown"));
    }

    // proc_open_pipes2.phpt
    #[test]
    fn no_pipes() {
        let mut t = TestVm::new();
        let (proc, pipes) = open(&mut t, s("exit 0"), spec(vec![]), vec![]);
        let proc = proc.unwrap();
        assert!(pipes_of(&pipes).is_empty());
        assert_eq!(close(&mut t, &proc), 0);
    }

    // proc_open_pipes3.phpt
    #[test]
    fn invalid_pipes() {
        let mut t = TestVm::new();
        let base: Vec<(i64, Value)> = (3..=5).map(|i| (i, pipe("w"))).collect();
        let with = |last: Value| {
            let mut v = base.clone();
            v.push((6, last));
            spec(v)
        };

        let (r, pipes) = open(&mut t, s("exit 0"), with(strs(&["pi"])), vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        assert!(matches!(pipes.get(), Value::Null));
        assert_eq!(
            t.output(),
            warning("pi is not a valid descriptor spec/mode")
        );

        let (r, _) = open(&mut t, s("exit 0"), with(Value::Int(1)), vec![]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "proc_open(): Argument #2 ($descriptor_spec) must only contain arrays and streams"
                    .to_string()
            )
        );

        let (r, pipes) = open(&mut t, s("exit 0"), with(pipe("test")), vec![]);
        let proc = r.unwrap();
        assert_eq!(pipes_of(&pipes).len(), 4);
        // "test" does not start with "w": the parent writes.
        let mode = handle_of(&pipe_at(&pipes, 6))
            .unwrap()
            .borrow()
            .st
            .mode
            .clone();
        assert_eq!(mode.as_bytes(), b"w");
        drop(proc);

        let file = strs(&["file", "test", "z"]);
        let (r, _) = open(&mut t, s("exit 0"), with(file), vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        let out = t.output();
        assert!(
            out.starts_with("\nWarning: proc_open(): Failed to open stream: "),
            "{out}"
        );
    }

    // proc_open_array.phpt
    #[test]
    fn command_array_validation() {
        let mut t = TestVm::new();
        let ds = || spec(vec![(0, pipe("r")), (1, pipe("w")), (2, pipe("w"))]);
        let cases: Vec<(Value, &str)> = vec![
            (
                list(vec![]),
                "proc_open(): Argument #1 ($command) must not be empty",
            ),
            (
                strs(&["php\0oops"]),
                "Command array element 1 contains a null byte",
            ),
            (
                strs(&["php", "array\0oops"]),
                "Command array element 2 contains a null byte",
            ),
            (
                strs(&[""]),
                "First element must contain a non-empty program name",
            ),
        ];
        for (cmd, expected) in cases {
            let (r, _) = open(&mut t, cmd, ds(), vec![]);
            assert_eq!(thrown(r), ("ValueError".to_string(), expected.to_string()));
        }
    }

    #[test]
    fn command_array_arguments_are_passed_through() {
        let mut t = TestVm::new();
        let args = [
            "Simple",
            "White space\ttab\nnewline",
            "\"Quoted\"",
            "Qu\"ot\"ed",
            "\\Back\\slash\\",
            "\\\\Back\\\\slash\\\\",
            "\\\"Qu\\\"ot\\\"ed\\\"",
        ];
        let mut cmd = vec!["/bin/sh", "-c", "printf '[%s]\\n' \"$0\" \"$@\"", "zero"];
        cmd.extend(args);
        let (r, pipes) = open(&mut t, strs(&cmd), spec(vec![(1, pipe("w"))]), vec![]);
        let proc = r.unwrap();
        let expected: String = std::iter::once("zero")
            .chain(args)
            .map(|a| format!("[{a}]\n"))
            .collect();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), expected);
        assert_eq!(close(&mut t, &proc), 0);

        // The command is the first element.
        let cmd = strs(&["/bin/sh", "-c", "exit 0"]);
        let (r, _) = open(&mut t, cmd, spec(vec![]), vec![]);
        let proc = r.unwrap();
        let st = wait_stopped_running(&mut t, &proc);
        assert!(matches!(field(&st, "command"), Value::String(c) if c.as_bytes() == b"/bin/sh"));
    }

    #[test]
    fn command_array_uses_path_lookup_and_reports_spawn_failure() {
        let mut t = TestVm::new();
        let cmd = strs(&["sh", "-c", "echo $0"]);
        let (r, pipes) = open(&mut t, cmd, spec(vec![(1, pipe("w"))]), vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "sh\n");
        assert_eq!(close(&mut t, &proc), 0);

        for cmd in [
            "php-in-rust-no-such-program",
            "/php-in-rust/no/such/program",
        ] {
            let (r, pipes) = open(&mut t, strs(&[cmd]), spec(vec![(1, pipe("w"))]), vec![]);
            assert!(matches!(r.unwrap(), Value::Bool(false)));
            assert!(matches!(pipes.get(), Value::Null));
            assert_eq!(
                t.output(),
                warning("posix_spawn() failed: No such file or directory")
            );
        }
    }

    #[test]
    fn string_command_runs_through_sh() {
        let mut t = TestVm::new();
        let cmd = s("echo $0; echo hi");
        let (r, pipes) = open(&mut t, cmd, spec(vec![(1, pipe("w"))]), vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "sh\nhi\n");
        let st = wait_stopped_running(&mut t, &proc);
        assert!(
            matches!(field(&st, "command"), Value::String(c) if c.as_bytes() == b"echo $0; echo hi")
        );
        assert_eq!(close(&mut t, &proc), 0);
    }

    // php_array_to_envp: order kept, empty values skipped, int keys raw.
    #[test]
    fn explicit_environment() {
        let mut t = TestVm::new();
        let mut env = PhpArray::new();
        env.insert(ArrayKey::from("A"), s("1"));
        env.insert(ArrayKey::from("B"), s(""));
        env.insert(ArrayKey::Int(0), s("C=3"));
        env.insert(ArrayKey::from("D"), Value::Int(4));
        env.insert(ArrayKey::from("E"), Value::Bool(true));
        let (r, pipes) = open(
            &mut t,
            strs(&["/usr/bin/env"]),
            spec(vec![(1, pipe("w"))]),
            vec![Value::Null, Value::from(env)],
        );
        let proc = r.unwrap();
        assert_eq!(
            read_all(&mut t, &pipe_at(&pipes, 1)),
            "A=1\nC=3\nD=4\nE=1\n"
        );
        assert_eq!(close(&mut t, &proc), 0);

        // An empty array is an empty environment, not the inherited one.
        let (r, pipes) = open(
            &mut t,
            strs(&["/usr/bin/env"]),
            spec(vec![(1, pipe("w"))]),
            vec![Value::Null, Value::from(PhpArray::new())],
        );
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "");
        assert_eq!(close(&mut t, &proc), 0);
    }

    // proc_open_cwd_null_bytes.phpt
    #[test]
    fn cwd() {
        let mut t = TestVm::new();
        let (r, pipes) = open(&mut t, s("pwd"), spec(vec![(1, pipe("w"))]), vec![s("/")]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "/\n");
        assert_eq!(close(&mut t, &proc), 0);

        let cwd = vec![s("foo\0bar")];
        let (r, _) = open(&mut t, s("does_not_matter"), spec(vec![]), cwd);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "proc_open(): Argument #4 ($cwd) must not contain any null bytes".to_string()
            )
        );

        let cwd = vec![s("/php-in-rust/no/such/dir")];
        let (r, _) = open(&mut t, s("exit 0"), spec(vec![]), cwd);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        assert_eq!(
            t.output(),
            warning("posix_spawn() failed: No such file or directory")
        );
    }

    // proc_open_null.phpt
    #[test]
    fn null_descriptor() {
        let mut t = TestVm::new();
        let cmd = "printf Test; printf Error >&2";
        let ds = spec(vec![(1, strs(&["null"])), (2, pipe("w"))]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        let keys: Vec<i64> = pipes_of(&pipes).iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![2]);
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 2)), "Error");
        close(&mut t, &proc);

        let ds = spec(vec![(1, pipe("w")), (2, strs(&["null"]))]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "Test");
        close(&mut t, &proc);
    }

    // proc_open_redirect.phpt
    #[test]
    fn redirect() {
        let mut t = TestVm::new();
        let ds = spec(vec![(0, strs(&["redirect"]))]);
        let (r, _) = open(&mut t, s("exit 0"), ds, vec![]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "Missing redirection target".to_string()
            )
        );
        let ds = spec(vec![(0, strs(&["redirect", "foo"]))]);
        let (r, _) = open(&mut t, s("exit 0"), ds, vec![]);
        assert_eq!(
            thrown(r).1,
            "Redirection target must be of type int, string given"
        );
        let ds = spec(vec![(0, list(vec![s("redirect"), Value::Int(42)]))]);
        let (r, _) = open(&mut t, s("exit 0"), ds, vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        assert_eq!(t.output(), warning("Redirection target 42 not found"));

        let cmd = "echo Test; printf Error >&2";
        let redirect = || list(vec![s("redirect"), Value::Int(1)]);
        let ds = spec(vec![(1, pipe("w")), (2, redirect())]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        assert_eq!(pipes_of(&pipes).len(), 1);
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "Test\nError");
        close(&mut t, &proc);

        // With a file name.
        let path = temp_path("redirect_file");
        let ds = spec(vec![(1, strs(&["file", &path, "w"])), (2, redirect())]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        assert!(pipes_of(&pipes).is_empty());
        close(&mut t, &proc);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Test\nError");
        std::fs::remove_file(&path).unwrap();

        // With a stream resource.
        let file = streams::open(&mut t.vm, path.as_bytes(), b"w", false, None)
            .unwrap()
            .unwrap();
        let ds = spec(vec![(1, Value::Resource(file.clone())), (2, redirect())]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        assert!(pipes_of(&pipes).is_empty());
        close(&mut t, &proc);
        assert!(streams::close(&mut t.vm, &file).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Test\nError");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(t.output(), "");
    }

    #[test]
    fn file_descriptor_reads_from_a_file() {
        let mut t = TestVm::new();
        let path = temp_path("file_input");
        std::fs::write(&path, "from a file\n").unwrap();
        let ds = spec(vec![(0, strs(&["file", &path, "r"])), (1, pipe("w"))]);
        let (r, pipes) = open(&mut t, s("/bin/cat"), ds, vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "from a file\n");
        assert_eq!(close(&mut t, &proc), 0);
        std::fs::remove_file(&path).unwrap();
    }

    // A read-buffered stream hands the child its logical position.
    #[test]
    fn stream_resource_position_is_synchronised() {
        let mut t = TestVm::new();
        let path = temp_path("cast_position");
        std::fs::write(&path, "0123456789").unwrap();
        let file = streams::open(&mut t.vm, path.as_bytes(), b"r", false, None)
            .unwrap()
            .unwrap();
        let head = streams::read(&mut t.vm, &file, 4).unwrap().unwrap();
        assert_eq!(head, b"0123");
        let ds = spec(vec![(0, Value::Resource(file.clone())), (1, pipe("w"))]);
        let (r, pipes) = open(&mut t, s("/bin/cat"), ds, vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "456789");
        assert_eq!(close(&mut t, &proc), 0);
        assert_eq!(t.output(), "");
        streams::close(&mut t.vm, &file).unwrap();
        std::fs::remove_file(&path).unwrap();
    }

    // proc_open_with_wrong_resource_type.phpt and php_stream_cast failures.
    #[test]
    fn stream_resource_errors() {
        let mut t = TestVm::new();
        let (r, _) = open(&mut t, s("exit 0"), spec(vec![]), vec![]);
        let proc = r.unwrap();
        let (r, _) = open(&mut t, s("exit 0"), spec(vec![(0, proc.clone())]), vec![]);
        assert_eq!(
            thrown(r),
            (
                "TypeError".to_string(),
                "proc_open(): supplied resource is not a valid stream resource".to_string()
            )
        );
        close(&mut t, &proc);

        let mem = streams::open_memory(&mut t.vm, b"data");
        let ds = spec(vec![(0, Value::Resource(mem))]);
        let (r, _) = open(&mut t, s("exit 0"), ds, vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        assert_eq!(
            t.output(),
            warning("Cannot represent a stream of type MEMORY as a File Descriptor")
        );
    }

    #[test]
    fn descriptor_spec_validation() {
        let mut t = TestVm::new();
        let mut keyed = PhpArray::new();
        keyed.insert(ArrayKey::from("in"), pipe("r"));
        let (r, _) = open(&mut t, s("exit 0"), Value::from(keyed), vec![]);
        assert_eq!(
            thrown(r).1,
            "proc_open(): Argument #2 ($descriptor_spec) must be an integer indexed array"
        );

        let cases: Vec<(Value, &str)> = vec![
            (list(vec![]), "Missing handle qualifier"),
            (strs(&["pipe"]), "Missing mode parameter for 'pipe'"),
            (strs(&["file"]), "Missing file name parameter for 'file'"),
            (
                strs(&["file", "/dev/null"]),
                "Missing mode parameter for 'file'",
            ),
        ];
        for (item, expected) in cases {
            let (r, _) = open(&mut t, s("exit 0"), spec(vec![(0, item)]), vec![]);
            assert_eq!(thrown(r), ("ValueError".to_string(), expected.to_string()));
        }

        // proc_open_descriptor_0_setup_failure.phpt: the spec is rejected
        // after a pipe was already set up.
        let ds = spec(vec![(0, pipe("w")), (1, strs(&["bogus_type"]))]);
        let (r, _) = open(&mut t, s("true"), ds, vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        assert_eq!(
            t.output(),
            warning("bogus_type is not a valid descriptor spec/mode")
        );

        let (r, _) = open(&mut t, s("true"), spec(vec![(-1, pipe("w"))]), vec![]);
        assert!(matches!(r.unwrap(), Value::Bool(false)));
        let out = t.output();
        assert!(
            out.starts_with("\nWarning: proc_open(): Unable to copy file descriptor ")
                && out.contains(" (for pipe) into file descriptor -1: Bad file descriptor in "),
            "{out}"
        );
    }

    // gh12655.phpt: descriptor items held by reference.
    #[test]
    fn descriptor_items_by_reference() {
        let mut t = TestVm::new();
        let item = |m: &str| Value::Ref(PhpRef::new(pipe(m)));
        let ds = spec(vec![(0, item("r")), (1, item("w")), (2, item("w"))]);
        let (r, pipes) = open(&mut t, s("echo ok"), ds, vec![]);
        let proc = r.unwrap();
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 1)), "ok\n");
        assert_eq!(close(&mut t, &proc), 0);
    }

    // proc_open_sockets*.phpt
    #[test]
    fn socket_descriptors() {
        let mut t = TestVm::new();
        let ds = spec(vec![
            (0, strs(&["null"])),
            (1, strs(&["socket"])),
            (2, strs(&["socket"])),
        ]);
        let cmd = s("printf hello; printf 'SOME ERROR' >&2");
        let (r, pipes) = open(&mut t, cmd, ds, vec![]);
        let proc = r.unwrap();
        let out = pipe_at(&pipes, 1);
        let kind = handle_of(&out).unwrap().borrow().kind();
        assert_eq!(kind, stream::StreamKind::Socket);
        assert_eq!(read_all(&mut t, &out), "hello");
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 2)), "SOME ERROR");
        assert_eq!(close(&mut t, &proc), 0);
    }

    // proc_open02.phpt
    #[test]
    fn get_status_and_terminate() {
        let mut t = TestVm::new();
        let cmd = strs(&["/bin/sleep", "3"]);
        let (r, _) = open(&mut t, cmd, spec(vec![(0, pipe("r"))]), vec![]);
        let proc = r.unwrap();
        let ok = t.call("proc_terminate", vec![proc.clone(), Value::Int(0)]);
        assert!(matches!(ok.unwrap(), Value::Bool(true)));

        let st = status(&mut t, &proc);
        let keys: Vec<String> = st
            .keys()
            .map(|k| k.to_php_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            keys,
            [
                "command", "pid", "cached", "running", "signaled", "stopped", "exitcode",
                "termsig", "stopsig"
            ]
        );
        assert!(matches!(field(&st, "command"), Value::String(c) if c.as_bytes() == b"/bin/sleep"));
        assert!(matches!(field(&st, "pid"), Value::Int(p) if p > 0));
        assert!(matches!(field(&st, "cached"), Value::Bool(false)));
        assert!(matches!(field(&st, "running"), Value::Bool(true)));
        assert!(matches!(field(&st, "signaled"), Value::Bool(false)));
        assert!(matches!(field(&st, "exitcode"), Value::Int(-1)));

        let ok = t.call("proc_terminate", vec![proc.clone()]);
        assert!(matches!(ok.unwrap(), Value::Bool(true)));
        let st = wait_stopped_running(&mut t, &proc);
        assert!(matches!(field(&st, "running"), Value::Bool(false)));
        assert!(matches!(field(&st, "signaled"), Value::Bool(true)));
        assert!(matches!(field(&st, "cached"), Value::Bool(false)));
        assert!(matches!(field(&st, "exitcode"), Value::Int(-1)));
        assert!(matches!(field(&st, "termsig"), Value::Int(15)));
        assert!(matches!(field(&st, "stopsig"), Value::Int(0)));

        // A signal is not cached: the reaped child is gone (ECHILD).
        let st = status(&mut t, &proc);
        assert!(matches!(field(&st, "running"), Value::Bool(false)));
        assert!(matches!(field(&st, "signaled"), Value::Bool(false)));
        assert_eq!(close(&mut t, &proc), -1);
    }

    // bug39322.phpt
    #[test]
    fn terminate_with_sigkill() {
        let mut t = TestVm::new();
        let ds = spec(vec![(0, pipe("r")), (1, pipe("w")), (2, pipe("w"))]);
        let (r, _) = open(&mut t, s("/bin/sleep 3"), ds, vec![]);
        let proc = r.unwrap();
        let ok = t.call("proc_terminate", vec![proc.clone(), Value::Int(9)]);
        assert!(matches!(ok.unwrap(), Value::Bool(true)));
        let st = wait_stopped_running(&mut t, &proc);
        assert!(
            matches!(field(&st, "command"), Value::String(c) if c.as_bytes() == b"/bin/sleep 3")
        );
        assert!(matches!(field(&st, "signaled"), Value::Bool(true)));
        assert!(matches!(field(&st, "termsig"), Value::Int(9)));
    }

    // gh10239_1.phpt, gh10239_2.phpt
    #[test]
    fn exit_status_is_cached() {
        let mut t = TestVm::new();
        let (r, _) = open(&mut t, s("false"), spec(vec![]), vec![]);
        let proc = r.unwrap();
        let st = wait_stopped_running(&mut t, &proc);
        assert!(matches!(field(&st, "cached"), Value::Bool(true)));
        assert!(matches!(field(&st, "exitcode"), Value::Int(1)));
        let again = status(&mut t, &proc);
        assert!(matches!(field(&again, "cached"), Value::Bool(true)));
        assert!(matches!(field(&again, "running"), Value::Bool(false)));
        assert!(matches!(field(&again, "exitcode"), Value::Int(1)));
        assert_eq!(close(&mut t, &proc), 1);

        let (r, _) = open(&mut t, s("exit 0"), spec(vec![]), vec![]);
        let proc = r.unwrap();
        wait_stopped_running(&mut t, &proc);
        assert_eq!(close(&mut t, &proc), 0);
    }

    #[test]
    fn process_argument_validation() {
        let mut t = TestVm::new();
        let (class, msg) = thrown(t.call("proc_close", vec![s("x")]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            msg,
            "proc_close(): Argument #1 ($process) must be of type resource, string given"
        );
        let (_, msg) = thrown(t.call("proc_get_status", vec![Value::Null]));
        assert_eq!(
            msg,
            "proc_get_status(): Argument #1 ($process) must be of type resource, null given"
        );
        let mem = streams::open_memory(&mut t.vm, b"");
        let (_, msg) = thrown(t.call("proc_terminate", vec![Value::Resource(mem)]));
        assert_eq!(
            msg,
            "proc_terminate(): supplied resource is not a valid process resource"
        );
    }

    // bug69442.phpt
    #[test]
    fn pty_descriptors() {
        let mut t = TestVm::new();
        let cmd = "(echo \"foo\" ; exit 42;) 3>/dev/null; code=$?; echo $code >&3; exit $code";
        let pty = || strs(&["pty"]);
        let ds = spec(vec![(0, pty()), (1, pty()), (2, pty()), (3, pipe("w"))]);
        let (r, pipes) = open(&mut t, s(cmd), ds, vec![]);
        let proc = r.unwrap();
        let master = pipe_at(&pipes, 0);
        let mode = handle_of(&master).unwrap().borrow().st.mode.clone();
        assert_eq!(mode.as_bytes(), b"r+");
        let mut data = Vec::new();
        while !data.ends_with(b"\n") {
            match streams::read(&mut t.vm, &master, 1000).unwrap() {
                Some(chunk) if !chunk.is_empty() => data.extend(chunk),
                _ => break,
            }
        }
        assert_eq!(String::from_utf8_lossy(&data), "foo\r\n");
        assert_eq!(read_all(&mut t, &pipe_at(&pipes, 3)), "42\n");
        assert_eq!(close(&mut t, &proc), 42);
    }
}
