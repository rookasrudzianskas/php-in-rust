//! main/streams/xp_socket.c + transports.c — socket streams (`tcp://`,
//! `udp://`, `unix://`, `udg://`) on top of `std::net` / `std::os::unix::net`.

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::time::Duration;

use php_runtime::{PResult, PhpStr, ResRef, Vm};

use super::stream::{
    self, CastAs, OptParam, StatBuf, StreamKind, StreamOps, StreamState, alloc_ex, opt, stream_warn,
};
use super::sys;

/// The OS socket behind a socket stream.
pub enum Sock {
    Tcp(TcpStream),
    Udp(UdpSocket),
    Unix(UnixStream),
    UnixDg(UnixDatagram),
    TcpListener(TcpListener),
    UnixListener(UnixListener),
    /// A socket inherited through a descriptor (`php://fd/N`, STDIN).
    Generic(File),
}

impl Sock {
    pub fn fd(&self) -> RawFd {
        match self {
            Sock::Tcp(s) => s.as_raw_fd(),
            Sock::Udp(s) => s.as_raw_fd(),
            Sock::Unix(s) => s.as_raw_fd(),
            Sock::UnixDg(s) => s.as_raw_fd(),
            Sock::TcpListener(s) => s.as_raw_fd(),
            Sock::UnixListener(s) => s.as_raw_fd(),
            Sock::Generic(f) => f.as_raw_fd(),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Sock::Tcp(s) => s.read(buf),
            Sock::Udp(s) => s.recv(buf),
            Sock::Unix(s) => s.read(buf),
            Sock::UnixDg(s) => s.recv(buf),
            Sock::Generic(f) => f.read(buf),
            _ => Err(io::Error::from_raw_os_error(57)),
        }
    }

    fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Sock::Tcp(s) => s.peek(buf),
            Sock::Udp(s) => s.peek(buf),
            _ => Err(io::Error::from(io::ErrorKind::WouldBlock)),
        }
    }

    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sock::Tcp(s) => s.write(buf),
            Sock::Udp(s) => s.send(buf),
            Sock::Unix(s) => s.write(buf),
            Sock::UnixDg(s) => s.send(buf),
            Sock::Generic(f) => f.write(buf),
            _ => Err(io::Error::from_raw_os_error(57)),
        }
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Sock::Tcp(s) => s.shutdown(how),
            Sock::Unix(s) => s.shutdown(how),
            Sock::UnixDg(s) => s.shutdown(how),
            _ => Err(io::Error::from_raw_os_error(57)),
        }
    }

    pub fn set_nonblocking(&self, nb: bool) -> io::Result<()> {
        match self {
            Sock::Tcp(s) => s.set_nonblocking(nb),
            Sock::Udp(s) => s.set_nonblocking(nb),
            Sock::Unix(s) => s.set_nonblocking(nb),
            Sock::UnixDg(s) => s.set_nonblocking(nb),
            Sock::TcpListener(s) => s.set_nonblocking(nb),
            Sock::UnixListener(s) => s.set_nonblocking(nb),
            Sock::Generic(f) => sys::sys_set_nonblock(f.as_raw_fd(), nb),
        }
    }
}

/// `php_netstream_data_t` + the socket ops tables.
pub struct SocketOps {
    pub sock: Option<Sock>,
    pub is_blocked: bool,
    /// `sock->timeout`; `None` is `tv_sec == -1` (wait forever).
    pub timeout: Option<Duration>,
    pub timeout_event: bool,
    pub label: &'static str,
}

impl SocketOps {
    pub fn new(sock: Sock, label: &'static str, timeout: Option<Duration>) -> SocketOps {
        SocketOps {
            sock: Some(sock),
            is_blocked: true,
            timeout,
            timeout_event: false,
            label,
        }
    }

    fn poll_for(&self, events: i16, timeout: Option<Duration>) -> io::Result<i32> {
        let Some(s) = &self.sock else {
            return Ok(-1);
        };
        let ms = match timeout {
            None => -1,
            Some(d) => d.as_millis().min(i32::MAX as u128) as i32,
        };
        loop {
            match sys::sys_poll(&[(s.fd(), events)], ms) {
                Ok((n, _)) => return Ok(n as i32),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    pub fn readable_now(&self) -> bool {
        matches!(self.poll_for(sys::POLLIN | sys::POLLPRI, Some(Duration::ZERO)), Ok(n) if n > 0)
    }
}

impl StreamOps for SocketOps {
    fn label(&self) -> &'static str {
        self.label
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Socket
    }

    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        if self.sock.is_none() {
            return Ok(0);
        }
        loop {
            let r = self.sock.as_mut().expect("checked").write(buf);
            match r {
                Ok(n) => return Ok(n as isize),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if !self.is_blocked {
                        return Ok(0);
                    }
                    self.timeout_event = false;
                    match self.poll_for(sys::POLLOUT, self.timeout) {
                        Ok(0) => {
                            self.timeout_event = true;
                            return Ok(-1);
                        }
                        Ok(n) if n > 0 => continue,
                        _ => return Ok(-1),
                    }
                }
                Err(e) => {
                    if st.flags & stream::flags::SUPPRESS_ERRORS == 0 {
                        let n = sys::errno_of(&e);
                        stream_warn(
                            vm,
                            st,
                            &format!(
                                "Send of {} bytes failed with errno={n} {}",
                                buf.len(),
                                sys::strerror(n)
                            ),
                        )?;
                    }
                    return Ok(-1);
                }
            }
        }
    }

    fn read(&mut self, _vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        if self.sock.is_none() {
            return Ok(-1);
        }
        if self.is_blocked {
            let dont_wait = self.timeout == Some(Duration::ZERO);
            if !dont_wait {
                self.timeout_event = false;
                if let Ok(0) =
                    self.poll_for(sys::POLLIN | sys::POLLPRI | sys::POLLHUP, self.timeout)
                {
                    self.timeout_event = true;
                    return Ok(-1);
                }
            } else if !self.readable_now() {
                return Ok(0);
            }
        }
        let r = self.sock.as_mut().expect("checked").read(buf);
        Ok(match r {
            Ok(0) => {
                st.eof = true;
                0
            }
            Ok(n) => n as isize,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => 0,
            Err(_) => {
                st.eof = true;
                -1
            }
        })
    }

    fn close(&mut self, _vm: &mut Vm, _st: &mut StreamState, close_handle: bool) -> PResult<i32> {
        if close_handle {
            if let Some(s) = &self.sock {
                let _ = s.shutdown(Shutdown::Read);
                let _ = self.poll_for(sys::POLLOUT, Some(Duration::from_millis(500)));
            }
        }
        self.sock = None;
        Ok(0)
    }

    fn cast_fd(&mut self, _as: CastAs) -> Option<RawFd> {
        self.sock.as_ref().map(Sock::fd)
    }

    fn stat(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        Ok(None)
    }

    fn set_option(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        match option {
            opt::CHECK_LIVENESS => {
                let Some(_) = &self.sock else {
                    return Ok(opt::RETURN_ERR);
                };
                let tv = if value == -1 {
                    match self.timeout {
                        None => Some(Duration::from_secs(
                            vm.ini_int("default_socket_timeout").max(0) as u64,
                        )),
                        t => t,
                    }
                } else {
                    Some(Duration::from_secs(value.max(0) as u64))
                };
                let mut alive = true;
                let check = (value == 0 && st.flags & stream::flags::NO_IO == 0)
                    || matches!(self.poll_for(sys::POLLIN | sys::POLLPRI, tv), Ok(n) if n > 0);
                if check && self.readable_now() {
                    let mut b = [0u8; 1];
                    match self.sock.as_mut().expect("checked").peek(&mut b) {
                        Ok(0) => alive = false,
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(_) => alive = false,
                    }
                }
                Ok(if alive {
                    opt::RETURN_OK
                } else {
                    opt::RETURN_ERR
                })
            }
            opt::BLOCKING => {
                let old = self.is_blocked as i32;
                match &self.sock {
                    Some(s) if s.set_nonblocking(value == 0).is_ok() => {
                        self.is_blocked = value != 0;
                        Ok(old)
                    }
                    _ => Ok(opt::RETURN_ERR),
                }
            }
            opt::READ_TIMEOUT => {
                if let OptParam::Timeout(sec, usec) = param {
                    self.timeout = if *sec < 0 {
                        None
                    } else {
                        Some(Duration::from_secs(*sec as u64) + Duration::from_micros(*usec as u64))
                    };
                    self.timeout_event = false;
                }
                Ok(opt::RETURN_OK)
            }
            opt::META_DATA_API => {
                if let OptParam::Meta(a) = param {
                    a.insert_str("timed_out", self.timeout_event);
                    a.insert_str("blocked", self.is_blocked);
                    a.insert_str("eof", st.eof);
                }
                Ok(opt::RETURN_OK)
            }
            _ => Ok(opt::RETURN_NOTIMPL),
        }
    }
}

/// `php_stream_sock_open_from_socket`.
pub fn from_fd(vm: &mut Vm, fd: OwnedFd, mode: &[u8]) -> ResRef {
    let timeout = default_timeout(vm);
    alloc_ex(
        vm,
        Box::new(SocketOps::new(
            Sock::Generic(File::from(fd)),
            "tcp_socket",
            timeout,
        )),
        mode,
        false,
    )
}

/// `FG(default_socket_timeout)` as a duration (`None` when negative).
pub fn default_timeout(vm: &Vm) -> Option<Duration> {
    let t = vm.ini_int("default_socket_timeout");
    (t >= 0).then(|| Duration::from_secs(t as u64))
}

/// Allocates a socket stream resource.
pub fn alloc_socket(
    vm: &mut Vm,
    sock: Sock,
    label: &'static str,
    timeout: Option<Duration>,
    persistent: bool,
) -> ResRef {
    alloc_ex(
        vm,
        Box::new(SocketOps::new(sock, label, timeout)),
        b"r+",
        persistent,
    )
}

/// Parsed transport address (`tcp://host:port`, `unix:///path`, ...).
pub struct XportTarget {
    pub transport: PhpStr,
    pub name: Vec<u8>,
}

/// Splits `scheme://rest` (default transport `tcp`), like
/// `php_stream_xport_create`.
pub fn split_transport(address: &[u8]) -> XportTarget {
    let n = address
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
        .count();
    if address.get(n) == Some(&b':') && address[n + 1..].starts_with(b"//") {
        XportTarget {
            transport: PhpStr::from_bytes(&address[..n]),
            name: address[n + 3..].to_vec(),
        }
    } else {
        XportTarget {
            transport: PhpStr::from("tcp"),
            name: address.to_vec(),
        }
    }
}

/// `parse_ip_address_ex`.
pub fn parse_ip_address(s: &[u8]) -> Result<(String, i32), String> {
    if s.contains(&0) {
        return Err("The hostname must not contain null bytes".into());
    }
    if s.first() == Some(&b'[') && s.len() > 1 {
        let inner = &s[1..s.len() - 1];
        match inner.iter().position(|&c| c == b']') {
            Some(p) if s.get(p + 2) == Some(&b':') => {
                let port = atoi(&s[p + 3..]);
                return Ok((String::from_utf8_lossy(&s[1..p + 1]).into_owned(), port));
            }
            _ => {
                return Err(format!(
                    "Failed to parse IPv6 address \"{}\"",
                    String::from_utf8_lossy(s)
                ));
            }
        }
    }
    let colon = if s.is_empty() {
        None
    } else {
        s[..s.len() - 1].iter().position(|&c| c == b':')
    };
    match colon {
        Some(c) => Ok((
            String::from_utf8_lossy(&s[..c]).into_owned(),
            atoi(&s[c + 1..]),
        )),
        None => Err(format!(
            "Failed to parse address \"{}\"",
            String::from_utf8_lossy(s)
        )),
    }
}

fn atoi(s: &[u8]) -> i32 {
    php_runtime::ini::strtol(s, 10).0 as i32
}

/// Resolves `host:port` (`php_network_getaddresses`).
pub fn resolve(host: &str, port: i32) -> Result<Vec<SocketAddr>, String> {
    let port = (port as u32 & 0xffff) as u16;
    match (host, port).to_socket_addrs() {
        Ok(it) => {
            let v: Vec<_> = it.collect();
            if v.is_empty() {
                Err(format!(
                    "php_network_getaddresses: getaddrinfo for {host} failed: nodename nor servname provided, or not known"
                ))
            } else {
                Ok(v)
            }
        }
        Err(e) => Err(format!(
            "php_network_getaddresses: getaddrinfo for {host} failed: {}",
            gai_message(&e)
        )),
    }
}

fn gai_message(e: &io::Error) -> String {
    let s = e.to_string();
    // std formats getaddrinfo failures as "failed to lookup address information: <msg>"
    match s.split_once(": ") {
        Some((_, m)) => m.to_string(),
        None => s,
    }
}

/// Connects a client socket; `Err((errno, message))` on failure.
pub fn connect(
    transport: &[u8],
    name: &[u8],
    timeout: Option<Duration>,
) -> Result<(Sock, &'static str), (i32, String)> {
    match transport {
        b"tcp" | b"udp" => {
            let (host, port) = parse_ip_address(name).map_err(|m| (0, m))?;
            let addrs = resolve(&host, port).map_err(|m| (0, m))?;
            if transport == b"udp" {
                let bind: SocketAddr = if addrs[0].is_ipv6() {
                    "[::]:0".parse().expect("valid")
                } else {
                    "0.0.0.0:0".parse().expect("valid")
                };
                let s = UdpSocket::bind(bind).map_err(|e| err_pair(&e))?;
                s.connect(addrs[0]).map_err(|e| err_pair(&e))?;
                return Ok((Sock::Udp(s), "udp_socket"));
            }
            let mut last = None;
            for a in addrs {
                let r = match timeout {
                    Some(t) if !t.is_zero() => TcpStream::connect_timeout(&a, t),
                    _ => TcpStream::connect(a),
                };
                match r {
                    Ok(s) => return Ok((Sock::Tcp(s), "tcp_socket")),
                    Err(e) => last = Some(e),
                }
            }
            let e = last.expect("at least one address");
            Err(err_pair(&e))
        }
        b"unix" => UnixStream::connect(super::plain::os(name))
            .map(|s| (Sock::Unix(s), "unix_socket"))
            .map_err(|e| err_pair(&e)),
        b"udg" => {
            let s = UnixDatagram::unbound().map_err(|e| err_pair(&e))?;
            s.connect(super::plain::os(name))
                .map_err(|e| err_pair(&e))?;
            Ok((Sock::UnixDg(s), "udg_socket"))
        }
        _ => Err((0, String::new())),
    }
}

fn err_pair(e: &io::Error) -> (i32, String) {
    if e.kind() == io::ErrorKind::TimedOut {
        return (sys::errno_of(e).max(60), "Connection timed out".to_string());
    }
    let n = sys::errno_of(e);
    (n, sys::strerror(n))
}

/// Binds (and for stream transports listens on) a server socket.
pub fn bind(
    transport: &[u8],
    name: &[u8],
    listen: bool,
) -> Result<(Sock, &'static str), (i32, String)> {
    match transport {
        b"tcp" => {
            let (host, port) = parse_ip_address(name).map_err(|m| (0, m))?;
            let addrs = resolve(&host, port).map_err(|m| (0, m))?;
            let _ = listen;
            TcpListener::bind(&addrs[..])
                .map(|l| (Sock::TcpListener(l), "tcp_socket"))
                .map_err(|e| err_pair(&e))
        }
        b"udp" => {
            let (host, port) = parse_ip_address(name).map_err(|m| (0, m))?;
            let addrs = resolve(&host, port).map_err(|m| (0, m))?;
            UdpSocket::bind(&addrs[..])
                .map(|s| (Sock::Udp(s), "udp_socket"))
                .map_err(|e| err_pair(&e))
        }
        b"unix" => UnixListener::bind(super::plain::os(name))
            .map(|l| (Sock::UnixListener(l), "unix_socket"))
            .map_err(|e| err_pair(&e)),
        b"udg" => UnixDatagram::bind(super::plain::os(name))
            .map(|s| (Sock::UnixDg(s), "udg_socket"))
            .map_err(|e| err_pair(&e)),
        _ => Err((0, String::new())),
    }
}

/// The registered transports (`stream_get_transports`).
pub const TRANSPORTS: &[&str] = &["tcp", "udp", "unix", "udg"];
