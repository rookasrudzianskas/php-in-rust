//! Minimal libc bindings for the POSIX calls std does not expose
//! (`flock`, `umask`, `isatty`, `dup`, `access`, `fnmatch`, `statvfs`,
//! `kill`, `poll`, `fcntl`, `getpwnam`, `getgrnam`).
//!
//! The workspace has no `libc` dependency; std already links the C library,
//! so these are plain `extern "C"` declarations. Every wrapper below is a
//! safe function: arguments are validated (paths are NUL-checked C strings,
//! buffers are owned and large enough) before the call, and results are
//! translated into `std::io::Error`s via `errno`.

#![allow(unsafe_code)]

use std::ffi::{CString, c_char, c_int, c_short, c_uint, c_ulong};
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

#[cfg(target_os = "macos")]
type ModeT = u16;
#[cfg(not(target_os = "macos"))]
type ModeT = u32;

#[cfg(target_os = "linux")]
type NfdsT = c_ulong;
#[cfg(not(target_os = "linux"))]
type NfdsT = c_uint;

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

/// Leading fields of `struct passwd` (identical on Linux and the BSDs).
#[repr(C)]
struct PasswdPrefix {
    pw_name: *mut c_char,
    pw_passwd: *mut c_char,
    pw_uid: u32,
    pw_gid: u32,
}

/// Leading fields of `struct group`.
#[repr(C)]
struct GroupPrefix {
    gr_name: *mut c_char,
    gr_passwd: *mut c_char,
    gr_gid: u32,
}

#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Default)]
struct StatVfs {
    f_bsize: c_ulong,
    f_frsize: c_ulong,
    f_blocks: u32,
    f_bfree: u32,
    f_bavail: u32,
    f_files: u32,
    f_ffree: u32,
    f_favail: u32,
    f_fsid: c_ulong,
    f_flag: c_ulong,
    f_namemax: c_ulong,
    spare: [u64; 8],
}

#[cfg(not(target_os = "macos"))]
#[repr(C)]
#[derive(Default)]
struct StatVfs {
    f_bsize: c_ulong,
    f_frsize: c_ulong,
    f_blocks: u64,
    f_bfree: u64,
    f_bavail: u64,
    f_files: u64,
    f_ffree: u64,
    f_favail: u64,
    f_fsid: c_ulong,
    f_flag: c_ulong,
    f_namemax: c_ulong,
    spare: [u64; 8],
}

unsafe extern "C" {
    fn flock(fd: c_int, operation: c_int) -> c_int;
    fn umask(mask: ModeT) -> ModeT;
    fn isatty(fd: c_int) -> c_int;
    fn dup(fd: c_int) -> c_int;
    fn access(path: *const c_char, mode: c_int) -> c_int;
    fn fnmatch(pattern: *const c_char, string: *const c_char, flags: c_int) -> c_int;
    fn statvfs(path: *const c_char, buf: *mut StatVfs) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
    fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: c_int) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn getpwnam(name: *const c_char) -> *mut PasswdPrefix;
    fn getgrnam(name: *const c_char) -> *mut GroupPrefix;
    fn getdtablesize() -> c_int;
}

fn cstr(path: &[u8]) -> io::Result<CString> {
    CString::new(path).map_err(|_| io::Error::from_raw_os_error(ENOENT))
}

fn check(r: c_int) -> io::Result<c_int> {
    if r == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(r)
    }
}

/// `ENOENT` (identical on every supported platform).
pub const ENOENT: i32 = 2;
/// `EBADF`.
pub const EBADF: i32 = 9;
/// `EACCES`.
pub const EACCES: i32 = 13;
/// `EEXIST`.
pub const EEXIST: i32 = 17;
/// `EXDEV`.
pub const EXDEV: i32 = 18;
/// `ENOTDIR`.
pub const ENOTDIR: i32 = 20;
/// `EISDIR`.
pub const EISDIR: i32 = 21;
/// `EINVAL`.
pub const EINVAL: i32 = 22;
/// `EPERM`.
pub const EPERM: i32 = 1;
/// `EINTR`.
pub const EINTR: i32 = 4;
/// `EAGAIN`/`EWOULDBLOCK`.
#[cfg(target_os = "linux")]
pub const EWOULDBLOCK: i32 = 11;
#[cfg(not(target_os = "linux"))]
pub const EWOULDBLOCK: i32 = 35;

/// `flock(2)` operations.
pub const LOCK_SH: i32 = 1;
pub const LOCK_EX: i32 = 2;
pub const LOCK_NB: i32 = 4;
pub const LOCK_UN: i32 = 8;

/// `access(2)` modes.
pub const F_OK: i32 = 0;
pub const X_OK: i32 = 1;
pub const W_OK: i32 = 2;
pub const R_OK: i32 = 4;

/// `fnmatch(3)` flags of the C library PHP is linked against.
#[cfg(target_os = "linux")]
pub mod fnm {
    pub const NOESCAPE: i64 = 2;
    pub const PATHNAME: i64 = 1;
    pub const PERIOD: i64 = 4;
    pub const CASEFOLD: i64 = 16;
}
#[cfg(not(target_os = "linux"))]
pub mod fnm {
    pub const NOESCAPE: i64 = 1;
    pub const PATHNAME: i64 = 2;
    pub const PERIOD: i64 = 4;
    pub const CASEFOLD: i64 = 16;
}

const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
#[cfg(target_os = "linux")]
const O_NONBLOCK: c_int = 0o4000;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: c_int = 0x0004;

/// Raw `O_NONBLOCK` for `OpenOptionsExt::custom_flags`.
pub const OPEN_NONBLOCK: i32 = O_NONBLOCK;

pub const POLLIN: i16 = 0x1;
pub const POLLPRI: i16 = 0x2;
pub const POLLOUT: i16 = 0x4;
pub const POLLERR: i16 = 0x8;
pub const POLLHUP: i16 = 0x10;
pub const POLLNVAL: i16 = 0x20;

/// `flock(fd, op)`.
pub fn sys_flock(fd: RawFd, op: i32) -> io::Result<()> {
    // SAFETY: flock only takes integers; an invalid fd yields EBADF.
    check(unsafe { flock(fd, op) }).map(|_| ())
}

/// `umask(mask)`: returns the previous mask.
pub fn sys_umask(mask: u32) -> u32 {
    // SAFETY: umask has no failure mode and only takes an integer.
    unsafe { umask(mask as ModeT) as u32 }
}

/// `isatty(fd)`.
pub fn sys_isatty(fd: RawFd) -> bool {
    // SAFETY: isatty only takes an integer; invalid fds return 0.
    unsafe { isatty(fd) == 1 }
}

/// `dup(fd)` as an owned descriptor.
pub fn sys_dup(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: dup only takes an integer; on success the returned descriptor
    // is a fresh one that nothing else owns, so OwnedFd may take it.
    let new = check(unsafe { dup(fd) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(new) })
}

/// `access(path, mode) == 0`.
pub fn sys_access(path: &[u8], mode: i32) -> io::Result<()> {
    let p = cstr(path)?;
    // SAFETY: `p` is a valid NUL-terminated string for the duration of the call.
    check(unsafe { access(p.as_ptr(), mode) }).map(|_| ())
}

/// `fnmatch(pattern, string, flags) == 0`.
pub fn sys_fnmatch(pattern: &[u8], string: &[u8], flags: i32) -> bool {
    let (Ok(p), Ok(s)) = (CString::new(pattern), CString::new(string)) else {
        return false;
    };
    // SAFETY: both arguments are valid NUL-terminated strings.
    unsafe { fnmatch(p.as_ptr(), s.as_ptr(), flags) == 0 }
}

/// `statvfs(path)`: `(f_frsize or f_bsize, f_blocks, f_bavail)`.
pub fn sys_statvfs(path: &[u8]) -> io::Result<(f64, f64, f64)> {
    let p = cstr(path)?;
    let mut buf = StatVfs::default();
    // SAFETY: `buf` is a writable struct at least as large as the C
    // `struct statvfs` (trailing spare space), `p` is NUL-terminated.
    check(unsafe { statvfs(p.as_ptr(), &mut buf) })?;
    let unit = if buf.f_frsize != 0 {
        buf.f_frsize
    } else {
        buf.f_bsize
    };
    Ok((unit as f64, buf.f_blocks as f64, buf.f_bavail as f64))
}

/// `kill(pid, sig)`.
pub fn sys_kill(pid: u32, sig: i32) -> io::Result<()> {
    // SAFETY: kill only takes integers.
    check(unsafe { kill(pid as c_int, sig) }).map(|_| ())
}

/// `poll(2)` over `(fd, events)` pairs; returns the `revents` and the count.
pub fn sys_poll(fds: &[(RawFd, i16)], timeout_ms: i32) -> io::Result<(usize, Vec<i16>)> {
    let mut v: Vec<PollFd> = fds
        .iter()
        .map(|&(fd, events)| PollFd {
            fd,
            events,
            revents: 0,
        })
        .collect();
    // SAFETY: `v` is a valid array of `v.len()` pollfd structs.
    let n = check(unsafe { poll(v.as_mut_ptr(), v.len() as NfdsT, timeout_ms) })?;
    Ok((n as usize, v.iter().map(|p| p.revents).collect()))
}

/// Whether `O_NONBLOCK` is set on `fd`.
pub fn sys_get_nonblock(fd: RawFd) -> io::Result<bool> {
    // SAFETY: F_GETFL takes no further argument.
    let fl = check(unsafe { fcntl(fd, F_GETFL) })?;
    Ok(fl & O_NONBLOCK != 0)
}

/// Sets or clears `O_NONBLOCK` on `fd`.
pub fn sys_set_nonblock(fd: RawFd, nonblock: bool) -> io::Result<()> {
    // SAFETY: F_GETFL takes no further argument; F_SETFL takes an int.
    let fl = check(unsafe { fcntl(fd, F_GETFL) })?;
    let fl = if nonblock {
        fl | O_NONBLOCK
    } else {
        fl & !O_NONBLOCK
    };
    check(unsafe { fcntl(fd, F_SETFL, fl) }).map(|_| ())
}

/// `getpwnam(name)->pw_uid`.
pub fn sys_uid_by_name(name: &[u8]) -> Option<u32> {
    let n = CString::new(name).ok()?;
    // SAFETY: `n` is NUL-terminated; the result points into static storage
    // owned by libc and is only read immediately.
    let p = unsafe { getpwnam(n.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).pw_uid })
    }
}

/// `getgrnam(name)->gr_gid`.
pub fn sys_gid_by_name(name: &[u8]) -> Option<u32> {
    let n = CString::new(name).ok()?;
    // SAFETY: as for `sys_uid_by_name`.
    let p = unsafe { getgrnam(n.as_ptr()) };
    if p.is_null() {
        None
    } else {
        Some(unsafe { (*p).gr_gid })
    }
}

/// `getdtablesize()`.
pub fn sys_dtablesize() -> i32 {
    // SAFETY: no arguments.
    unsafe { getdtablesize() }
}

/// `strerror(errno)` text as PHP prints it (std's message without the
/// " (os error N)" suffix).
pub fn strerror(errno: i32) -> String {
    let s = io::Error::from_raw_os_error(errno).to_string();
    match s.rfind(" (os error ") {
        Some(i) => s[..i].to_string(),
        None => s,
    }
}

/// The errno of an `io::Error` (EIO-like fallback for synthetic errors).
pub fn errno_of(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(match e.kind() {
        io::ErrorKind::NotFound => ENOENT,
        io::ErrorKind::PermissionDenied => EACCES,
        io::ErrorKind::AlreadyExists => EEXIST,
        io::ErrorKind::WouldBlock => EWOULDBLOCK,
        io::ErrorKind::InvalidInput => EINVAL,
        _ => 5,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strerror_texts() {
        assert_eq!(strerror(ENOENT), "No such file or directory");
        assert_eq!(strerror(EACCES), "Permission denied");
        assert_eq!(strerror(EISDIR), "Is a directory");
    }

    #[test]
    fn fnmatch_and_umask() {
        assert!(sys_fnmatch(b"*.txt", b"a.txt", 0));
        assert!(!sys_fnmatch(b"*.txt", b"a.php", 0));
        let old = sys_umask(0o022);
        assert_eq!(sys_umask(old), 0o022);
    }
}
