//! Thin, safe wrappers over the few libc services ext/standard needs and the
//! Rust standard library does not expose (`getrusage`, `getuid`, `uname`,
//! `getloadavg`, the netdb databases, `getifaddrs`, `syslog`, `strptime`,
//! `gethostbyaddr`, `clock_gettime`).
//!
//! This is the only place in the crate that uses `unsafe`. Every block calls a
//! POSIX function with pointers to live local buffers, and converts the result
//! into owned Rust data before returning. All of it is `cfg(unix)`; other
//! targets get conservative fallbacks.

#![allow(non_camel_case_types, clippy::missing_safety_doc)]

#[cfg(unix)]
mod ffi {
    use std::os::raw::{c_char, c_int, c_long, c_uint, c_void};

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct timespec {
        pub tv_sec: i64,
        pub tv_nsec: c_long,
    }

    #[cfg(target_os = "macos")]
    pub type suseconds_t = i32;
    #[cfg(not(target_os = "macos"))]
    pub type suseconds_t = i64;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct timeval {
        pub tv_sec: i64,
        pub tv_usec: suseconds_t,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct rusage {
        pub ru_utime: timeval,
        pub ru_stime: timeval,
        pub ru_maxrss: c_long,
        pub ru_ixrss: c_long,
        pub ru_idrss: c_long,
        pub ru_isrss: c_long,
        pub ru_minflt: c_long,
        pub ru_majflt: c_long,
        pub ru_nswap: c_long,
        pub ru_inblock: c_long,
        pub ru_oublock: c_long,
        pub ru_msgsnd: c_long,
        pub ru_msgrcv: c_long,
        pub ru_nsignals: c_long,
        pub ru_nvcsw: c_long,
        pub ru_nivcsw: c_long,
    }

    #[repr(C)]
    pub struct servent {
        pub s_name: *mut c_char,
        pub s_aliases: *mut *mut c_char,
        pub s_port: c_int,
        pub s_proto: *mut c_char,
    }

    #[repr(C)]
    pub struct protoent {
        pub p_name: *mut c_char,
        pub p_aliases: *mut *mut c_char,
        pub p_proto: c_int,
    }

    #[repr(C)]
    pub struct hostent {
        pub h_name: *mut c_char,
        pub h_aliases: *mut *mut c_char,
        pub h_addrtype: c_int,
        pub h_length: c_int,
        pub h_addr_list: *mut *mut c_char,
    }

    #[repr(C)]
    pub struct ifaddrs {
        pub ifa_next: *mut ifaddrs,
        pub ifa_name: *mut c_char,
        pub ifa_flags: c_uint,
        pub ifa_addr: *mut c_void,
        pub ifa_netmask: *mut c_void,
        pub ifa_dstaddr: *mut c_void,
        pub ifa_data: *mut c_void,
    }

    /// `struct tm` (glibc/BSD layout: nine ints, `tm_gmtoff`, `tm_zone`).
    #[repr(C)]
    pub struct tm {
        pub tm_sec: c_int,
        pub tm_min: c_int,
        pub tm_hour: c_int,
        pub tm_mday: c_int,
        pub tm_mon: c_int,
        pub tm_year: c_int,
        pub tm_wday: c_int,
        pub tm_yday: c_int,
        pub tm_isdst: c_int,
        pub tm_gmtoff: c_long,
        pub tm_zone: *const c_char,
    }

    #[cfg(target_os = "macos")]
    pub const UTS_LEN: usize = 256;
    #[cfg(not(target_os = "macos"))]
    pub const UTS_LEN: usize = 65;

    #[repr(C)]
    pub struct utsname {
        pub sysname: [c_char; UTS_LEN],
        pub nodename: [c_char; UTS_LEN],
        pub release: [c_char; UTS_LEN],
        pub version: [c_char; UTS_LEN],
        pub machine: [c_char; UTS_LEN],
        /// glibc's `domainname` (absent on macOS; harmless slack there).
        pub domainname: [c_char; UTS_LEN],
    }

    unsafe extern "C" {
        pub fn clock_gettime(clk_id: c_int, tp: *mut timespec) -> c_int;
        pub fn getuid() -> u32;
        pub fn getgid() -> u32;
        pub fn getrusage(who: c_int, usage: *mut rusage) -> c_int;
        pub fn gethostname(name: *mut c_char, len: usize) -> c_int;
        pub fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
        pub fn uname(buf: *mut utsname) -> c_int;
        pub fn getservbyname(name: *const c_char, proto: *const c_char) -> *mut servent;
        pub fn getservbyport(port: c_int, proto: *const c_char) -> *mut servent;
        pub fn getprotobyname(name: *const c_char) -> *mut protoent;
        pub fn getprotobynumber(proto: c_int) -> *mut protoent;
        pub fn gethostbyaddr(addr: *const c_void, len: u32, ty: c_int) -> *mut hostent;
        pub fn getifaddrs(ifap: *mut *mut ifaddrs) -> c_int;
        pub fn freeifaddrs(ifa: *mut ifaddrs);
        pub fn openlog(ident: *const c_char, option: c_int, facility: c_int);
        pub fn closelog();
        pub fn syslog(priority: c_int, format: *const c_char, ...);
        pub fn strptime(s: *const c_char, format: *const c_char, tm: *mut tm) -> *mut c_char;
        pub fn umask(mask: u32) -> u32;
        pub fn getpwuid(uid: u32) -> *mut *mut c_char;
    }

    #[cfg(target_os = "macos")]
    pub const CLOCK_MONOTONIC: c_int = 6;
    #[cfg(not(target_os = "macos"))]
    pub const CLOCK_MONOTONIC: c_int = 1;
}

#[cfg(unix)]
use std::ffi::{CStr, CString};
#[cfg(unix)]
use std::os::raw::c_char;

/// Copies a NUL-terminated C string into owned bytes.
#[cfg(unix)]
unsafe fn c_bytes(p: *const c_char) -> Vec<u8> {
    // SAFETY: callers pass a valid NUL-terminated string from libc.
    unsafe { CStr::from_ptr(p).to_bytes().to_vec() }
}

/// Builds a C string, truncating at the first NUL like C callers do.
#[cfg(unix)]
fn cstring(s: &[u8]) -> CString {
    let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
    CString::new(&s[..end]).expect("no interior NUL")
}

/// Monotonic clock in nanoseconds (`zend_hrtime`).
pub fn hrtime_ns() -> u64 {
    #[cfg(unix)]
    {
        let mut ts = ffi::timespec::default();
        // SAFETY: `ts` is a valid out-pointer for the duration of the call.
        let rc = unsafe { ffi::clock_gettime(ffi::CLOCK_MONOTONIC, &mut ts) };
        if rc == 0 {
            return (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64;
        }
    }
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// `getuid()`.
pub fn getuid() -> i64 {
    #[cfg(unix)]
    {
        // SAFETY: no arguments, cannot fail.
        unsafe { ffi::getuid() as i64 }
    }
    #[cfg(not(unix))]
    {
        1
    }
}

/// `getgid()`.
pub fn getgid() -> i64 {
    #[cfg(unix)]
    {
        // SAFETY: no arguments, cannot fail.
        unsafe { ffi::getgid() as i64 }
    }
    #[cfg(not(unix))]
    {
        1
    }
}

/// The login name of `uid` (`getpwuid(uid)->pw_name`).
pub fn getpwuid_name(uid: i64) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        // SAFETY: `struct passwd` starts with `char *pw_name` on every
        // supported unix; the result is copied before any other libc call.
        unsafe {
            let pw = ffi::getpwuid(uid as u32);
            if pw.is_null() || (*pw).is_null() {
                return None;
            }
            Some(c_bytes(*pw))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = uid;
        None
    }
}

/// Fills `buf` with random bytes (`php_random_bytes_silent`); `false` when no
/// source is available.
pub fn random_bytes(buf: &mut [u8]) -> bool {
    use std::io::Read;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(buf).is_ok() {
            return true;
        }
    }
    false
}

/// Random bytes with a std-only fallback (`php_random_generate_fallback_seed`).
pub fn random_bytes_or_fallback(buf: &mut [u8]) {
    if random_bytes(buf) {
        return;
    }
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let state = RandomState::new();
    for (i, chunk) in buf.chunks_mut(8).enumerate() {
        let mut h = state.build_hasher();
        h.write_usize(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let b = h.finish().to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
}

/// `umask(0)` followed by restoring the previous mask: reads the current umask.
pub fn current_umask() -> u32 {
    #[cfg(unix)]
    {
        // SAFETY: umask never fails; the previous value is restored right away.
        unsafe {
            let old = ffi::umask(0o022);
            ffi::umask(old);
            old
        }
    }
    #[cfg(not(unix))]
    {
        0o022
    }
}

/// The fields `getrusage()` reports, in php-src's order.
#[derive(Default, Debug, Clone, Copy)]
pub struct Rusage {
    pub ru_oublock: i64,
    pub ru_inblock: i64,
    pub ru_msgsnd: i64,
    pub ru_msgrcv: i64,
    pub ru_maxrss: i64,
    pub ru_ixrss: i64,
    pub ru_idrss: i64,
    pub ru_minflt: i64,
    pub ru_majflt: i64,
    pub ru_nsignals: i64,
    pub ru_nvcsw: i64,
    pub ru_nivcsw: i64,
    pub ru_nswap: i64,
    pub utime_usec: i64,
    pub utime_sec: i64,
    pub stime_usec: i64,
    pub stime_sec: i64,
}

/// `getrusage(RUSAGE_SELF | RUSAGE_CHILDREN)`; `None` on failure.
pub fn getrusage(children: bool) -> Option<Rusage> {
    #[cfg(unix)]
    {
        let mut u = ffi::rusage::default();
        // RUSAGE_SELF = 0, RUSAGE_CHILDREN = -1 on every supported unix.
        let who = if children { -1 } else { 0 };
        // SAFETY: `u` is a valid, writable `struct rusage`.
        if unsafe { ffi::getrusage(who, &mut u) } == -1 {
            return None;
        }
        Some(Rusage {
            ru_oublock: u.ru_oublock as i64,
            ru_inblock: u.ru_inblock as i64,
            ru_msgsnd: u.ru_msgsnd as i64,
            ru_msgrcv: u.ru_msgrcv as i64,
            ru_maxrss: u.ru_maxrss as i64,
            ru_ixrss: u.ru_ixrss as i64,
            ru_idrss: u.ru_idrss as i64,
            ru_minflt: u.ru_minflt as i64,
            ru_majflt: u.ru_majflt as i64,
            ru_nsignals: u.ru_nsignals as i64,
            ru_nvcsw: u.ru_nvcsw as i64,
            ru_nivcsw: u.ru_nivcsw as i64,
            ru_nswap: u.ru_nswap as i64,
            utime_usec: u.ru_utime.tv_usec as i64,
            utime_sec: u.ru_utime.tv_sec,
            stime_usec: u.ru_stime.tv_usec as i64,
            stime_sec: u.ru_stime.tv_sec,
        })
    }
    #[cfg(not(unix))]
    {
        let _ = children;
        None
    }
}

/// `gethostname()`.
pub fn gethostname() -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        let mut buf = [0 as c_char; 256];
        // SAFETY: `buf` is writable for `buf.len()` bytes.
        if unsafe { ffi::gethostname(buf.as_mut_ptr(), buf.len()) } != 0 {
            return None;
        }
        buf[255] = 0;
        // SAFETY: NUL-terminated just above.
        Some(unsafe { c_bytes(buf.as_ptr()) })
    }
    #[cfg(not(unix))]
    {
        std::env::var_os("COMPUTERNAME").map(|v| v.to_string_lossy().into_owned().into_bytes())
    }
}

/// `getloadavg(3)` (three samples); `None` when unavailable.
pub fn getloadavg() -> Option<[f64; 3]> {
    #[cfg(unix)]
    {
        let mut a = [0f64; 3];
        // SAFETY: `a` holds three doubles.
        if unsafe { ffi::getloadavg(a.as_mut_ptr(), 3) } != 3 {
            return None;
        }
        Some(a)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `uname(2)` fields: sysname, nodename, release, version, machine.
pub fn uname() -> Option<[Vec<u8>; 5]> {
    #[cfg(unix)]
    {
        // SAFETY: `utsname` is plain old data, zero is a valid bit pattern.
        let mut u: ffi::utsname = unsafe { std::mem::zeroed() };
        // SAFETY: `u` is a valid out-pointer.
        if unsafe { ffi::uname(&mut u) } < 0 {
            return None;
        }
        u.sysname[ffi::UTS_LEN - 1] = 0;
        u.nodename[ffi::UTS_LEN - 1] = 0;
        u.release[ffi::UTS_LEN - 1] = 0;
        u.version[ffi::UTS_LEN - 1] = 0;
        u.machine[ffi::UTS_LEN - 1] = 0;
        // SAFETY: every field was NUL-terminated above.
        unsafe {
            Some([
                c_bytes(u.sysname.as_ptr()),
                c_bytes(u.nodename.as_ptr()),
                c_bytes(u.release.as_ptr()),
                c_bytes(u.version.as_ptr()),
                c_bytes(u.machine.as_ptr()),
            ])
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// `getservbyname()`: the port in host byte order.
pub fn getservbyname(name: &[u8], proto: &[u8]) -> Option<i64> {
    #[cfg(unix)]
    {
        let (n, p) = (cstring(name), cstring(proto));
        // SAFETY: both arguments are valid C strings; the returned pointer is
        // read immediately and never retained.
        unsafe {
            let s = ffi::getservbyname(n.as_ptr(), p.as_ptr());
            if s.is_null() {
                return None;
            }
            Some(u16::from_be((*s).s_port as u16) as i64)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (name, proto);
        None
    }
}

/// `getservbyport()`: the service name.
pub fn getservbyport(port: u16, proto: &[u8]) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        let p = cstring(proto);
        // SAFETY: valid C string; result copied before returning.
        unsafe {
            let s = ffi::getservbyport(port.to_be() as i32, p.as_ptr());
            if s.is_null() {
                return None;
            }
            Some(c_bytes((*s).s_name))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (port, proto);
        None
    }
}

/// `getprotobyname()`: the protocol number.
pub fn getprotobyname(name: &[u8]) -> Option<i64> {
    #[cfg(unix)]
    {
        let n = cstring(name);
        // SAFETY: valid C string; result read immediately.
        unsafe {
            let p = ffi::getprotobyname(n.as_ptr());
            if p.is_null() {
                return None;
            }
            Some((*p).p_proto as i64)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        None
    }
}

/// `getprotobynumber()`: the protocol name.
pub fn getprotobynumber(num: i32) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        // SAFETY: result copied before returning.
        unsafe {
            let p = ffi::getprotobynumber(num);
            if p.is_null() {
                return None;
            }
            Some(c_bytes((*p).p_name))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = num;
        None
    }
}

/// `gethostbyaddr()` for a raw IPv4 (4 bytes) or IPv6 (16 bytes) address.
pub fn gethostbyaddr(addr: &[u8]) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        let family = match addr.len() {
            4 => 2,
            16 => {
                if cfg!(target_os = "macos") {
                    30
                } else {
                    10
                }
            }
            _ => return None,
        };
        // SAFETY: `addr` is valid for `addr.len()` bytes; result copied.
        unsafe {
            let h = ffi::gethostbyaddr(addr.as_ptr().cast(), addr.len() as u32, family);
            if h.is_null() || (*h).h_name.is_null() {
                return None;
            }
            Some(c_bytes((*h).h_name))
        }
    }
    #[cfg(not(unix))]
    {
        let _ = addr;
        None
    }
}

/// One entry of `getifaddrs()`.
#[derive(Debug, Clone, Default)]
pub struct IfAddr {
    pub name: Vec<u8>,
    pub flags: u32,
    pub addr: Option<SockAddr>,
    pub netmask: Option<SockAddr>,
    pub dstaddr: Option<SockAddr>,
}

/// A decoded `struct sockaddr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SockAddr {
    /// `sa_family` (platform numbering: `AF_INET` = 2, `AF_INET6` = 10 or 30...).
    pub family: i64,
    /// The IPv4/IPv6 address bytes when the family is an IP one.
    pub ip: Option<std::net::IpAddr>,
}

/// `getifaddrs()`; `Err` carries the `errno` value.
pub fn getifaddrs() -> Result<Vec<IfAddr>, i32> {
    #[cfg(unix)]
    {
        let mut head: *mut ffi::ifaddrs = std::ptr::null_mut();
        // SAFETY: `head` is a valid out-pointer.
        if unsafe { ffi::getifaddrs(&mut head) } != 0 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
        let mut out = Vec::new();
        let mut p = head;
        while !p.is_null() {
            // SAFETY: `p` walks the list returned by getifaddrs, which stays
            // valid until `freeifaddrs` below.
            unsafe {
                let e = &*p;
                out.push(IfAddr {
                    name: c_bytes(e.ifa_name),
                    flags: e.ifa_flags,
                    addr: decode_sockaddr(e.ifa_addr),
                    netmask: decode_sockaddr(e.ifa_netmask),
                    dstaddr: decode_sockaddr(e.ifa_dstaddr),
                });
                p = e.ifa_next;
            }
        }
        // SAFETY: `head` came from getifaddrs and is freed exactly once.
        unsafe { ffi::freeifaddrs(head) };
        Ok(out)
    }
    #[cfg(not(unix))]
    {
        Err(0)
    }
}

#[cfg(unix)]
unsafe fn decode_sockaddr(p: *const std::ffi::c_void) -> Option<SockAddr> {
    if p.is_null() {
        return None;
    }
    let b = p.cast::<u8>();
    // SAFETY: every sockaddr is at least `sa_family` + 14 bytes of data; the
    // IPv6 variant is 28 bytes and is only read when the family says so.
    unsafe {
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        let (family, v4_off, v6_off) = (*b.add(1) as i64, 4usize, 8usize);
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        let (family, v4_off, v6_off) = (u16::from_ne_bytes([*b, *b.add(1)]) as i64, 4usize, 8usize);
        let af_inet6: i64 = if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
            30
        } else if cfg!(target_os = "freebsd")
            || cfg!(target_os = "netbsd")
            || cfg!(target_os = "openbsd")
        {
            28
        } else {
            10
        };
        let ip = if family == 2 {
            let mut a = [0u8; 4];
            std::ptr::copy_nonoverlapping(b.add(v4_off), a.as_mut_ptr(), 4);
            Some(std::net::IpAddr::V4(std::net::Ipv4Addr::from(a)))
        } else if family == af_inet6 {
            let mut a = [0u8; 16];
            std::ptr::copy_nonoverlapping(b.add(v6_off), a.as_mut_ptr(), 16);
            Some(std::net::IpAddr::V6(std::net::Ipv6Addr::from(a)))
        } else {
            None
        };
        Some(SockAddr { family, ip })
    }
}

/// `openlog(3)`. `ident` must stay alive while the log is open, so the caller
/// passes ownership of a leaked-until-closed buffer through [`SyslogIdent`].
pub struct SyslogIdent {
    #[cfg(unix)]
    ident: CString,
}

impl SyslogIdent {
    pub fn new(ident: &[u8]) -> SyslogIdent {
        #[cfg(unix)]
        {
            SyslogIdent {
                ident: cstring(ident),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = ident;
            SyslogIdent {}
        }
    }

    /// Calls `openlog(ident, option, facility)`; `self` must outlive the log.
    pub fn open(&self, option: i64, facility: i64) {
        #[cfg(unix)]
        {
            // SAFETY: `self.ident` is a valid C string that the caller keeps
            // alive until `closelog` (it is stored in the Vm's syslog state).
            unsafe { ffi::openlog(self.ident.as_ptr(), option as i32, facility as i32) }
        }
        #[cfg(not(unix))]
        {
            let _ = (option, facility);
        }
    }
}

/// `closelog(3)`.
pub fn closelog() {
    #[cfg(unix)]
    {
        // SAFETY: no arguments.
        unsafe { ffi::closelog() }
    }
}

/// `syslog(priority, "%s", message)`.
pub fn syslog(priority: i64, message: &[u8]) {
    #[cfg(unix)]
    {
        let m = cstring(message);
        // SAFETY: the format string is the constant "%s" with one C-string argument.
        unsafe { ffi::syslog(priority as i32, c"%s".as_ptr(), m.as_ptr()) }
    }
    #[cfg(not(unix))]
    {
        let _ = (priority, message);
    }
}

/// Result of `strptime(3)`.
pub struct Strptime {
    pub tm_sec: i64,
    pub tm_min: i64,
    pub tm_hour: i64,
    pub tm_mday: i64,
    pub tm_mon: i64,
    pub tm_year: i64,
    pub tm_wday: i64,
    pub tm_yday: i64,
    pub unparsed: Vec<u8>,
}

/// `strptime(3)`; `None` when the input does not match the format.
pub fn strptime(ts: &[u8], format: &[u8]) -> Option<Strptime> {
    #[cfg(unix)]
    {
        let (s, f) = (cstring(ts), cstring(format));
        // SAFETY: `tm` is plain old data and zero is a valid value.
        let mut tm: ffi::tm = unsafe { std::mem::zeroed() };
        // SAFETY: all pointers are valid; the returned pointer, when non-null,
        // points into `s`, which is alive here.
        unsafe {
            let rest = ffi::strptime(s.as_ptr(), f.as_ptr(), &mut tm);
            if rest.is_null() {
                return None;
            }
            Some(Strptime {
                tm_sec: tm.tm_sec as i64,
                tm_min: tm.tm_min as i64,
                tm_hour: tm.tm_hour as i64,
                tm_mday: tm.tm_mday as i64,
                tm_mon: tm.tm_mon as i64,
                tm_year: tm.tm_year as i64,
                tm_wday: tm.tm_wday as i64,
                tm_yday: tm.tm_yday as i64,
                unparsed: c_bytes(rest),
            })
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (ts, format);
        None
    }
}

/// Sets an environment variable (`setenv`).
///
/// `std::env::set_var` is `unsafe` since the 2024 edition because other
/// threads may read the environment through libc concurrently. The
/// interpreter runs PHP code on a single thread and never calls `getenv`
/// from elsewhere, which is the invariant required here.
pub fn set_env(name: &[u8], value: &[u8]) {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(unix)]
    {
        // SAFETY: see the function documentation.
        unsafe { std::env::set_var(OsStr::from_bytes(name), OsStr::from_bytes(value)) }
    }
    #[cfg(not(unix))]
    {
        let (n, v) = (
            String::from_utf8_lossy(name).into_owned(),
            String::from_utf8_lossy(value).into_owned(),
        );
        // SAFETY: see the function documentation.
        unsafe { std::env::set_var(OsStr::new(&n), OsStr::new(&v)) }
    }
}

/// Removes an environment variable (`unsetenv`); see [`set_env`] for safety.
pub fn unset_env(name: &[u8]) {
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    #[cfg(unix)]
    {
        // SAFETY: see `set_env`.
        unsafe { std::env::remove_var(OsStr::from_bytes(name)) }
    }
    #[cfg(not(unix))]
    {
        let n = String::from_utf8_lossy(name).into_owned();
        // SAFETY: see `set_env`.
        unsafe { std::env::remove_var(OsStr::new(&n)) }
    }
}

/// The process environment as `(name, value)` byte pairs.
pub fn env_vars() -> Vec<(Vec<u8>, Vec<u8>)> {
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;
    std::env::vars_os()
        .map(|(k, v)| {
            #[cfg(unix)]
            {
                (k.as_bytes().to_vec(), v.as_bytes().to_vec())
            }
            #[cfg(not(unix))]
            {
                (
                    k.to_string_lossy().into_owned().into_bytes(),
                    v.to_string_lossy().into_owned().into_bytes(),
                )
            }
        })
        .collect()
}

/// One environment variable's value.
pub fn env_var(name: &[u8]) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        if name.is_empty() || name.contains(&b'=') || name.contains(&0) {
            return None;
        }
        std::env::var_os(OsStr::from_bytes(name)).map(OsStringExt::into_vec)
    }
    #[cfg(not(unix))]
    {
        std::env::var(String::from_utf8_lossy(name).as_ref())
            .ok()
            .map(String::into_bytes)
    }
}
