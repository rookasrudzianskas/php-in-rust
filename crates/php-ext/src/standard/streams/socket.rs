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
        sys::sys_peek_nonblocking(self.fd(), buf)
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
        // zend_fstat(sock->socket) on a duplicate of the descriptor
        let Some(s) = &self.sock else {
            return Ok(None);
        };
        Ok(sys::sys_dup(s.fd())
            .ok()
            .and_then(|fd| File::from(fd).metadata().ok())
            .map(|m| StatBuf::from_metadata(&m)))
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

/// `ETIMEDOUT`.
#[cfg(target_os = "linux")]
pub const ETIMEDOUT: i32 = 110;
#[cfg(not(target_os = "linux"))]
pub const ETIMEDOUT: i32 = 60;

fn err_pair(e: &io::Error) -> (i32, String) {
    let n = match e.raw_os_error() {
        Some(n) => n,
        None if e.kind() == io::ErrorKind::TimedOut => ETIMEDOUT,
        None => sys::errno_of(e),
    };
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

/// Persistent sockets (`pfsockopen`, `STREAM_CLIENT_PERSISTENT`), keyed by
/// their hash key.
#[derive(Default)]
pub struct PersistentSockets {
    pub map: Vec<(Vec<u8>, ResRef)>,
}

/// `php_stream_xport_create` for clients and servers (`server_flags` are
/// STREAM_SERVER_BIND/LISTEN). `Err((errno, message))` carries what PHP
/// reports as `$error_code` / `$error_message`.
pub fn xport_create(
    vm: &mut Vm,
    address: &[u8],
    server: bool,
    server_flags: i64,
    timeout: Option<Duration>,
    ctx: Option<&ResRef>,
    persistent_key: Option<&[u8]>,
) -> PResult<Result<ResRef, (i32, String)>> {
    if let Some(key) = persistent_key {
        let found = vm
            .ext_data::<PersistentSockets>()
            .map
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, r)| r.clone());
        if let Some(r) = found {
            let alive = stream::with_stream(vm, &r, |vm, s| {
                s.set_option(vm, opt::CHECK_LIVENESS, 0, &mut OptParam::None)
            })?;
            if alive == Some(opt::RETURN_OK) {
                return Ok(Ok(r));
            }
            vm.ext_data::<PersistentSockets>()
                .map
                .retain(|(k, _)| k != key);
        }
    }
    let target = split_transport(address);
    if !TRANSPORTS.contains(&target.transport.to_string_lossy().as_ref()) {
        let name: String = target
            .transport
            .to_string_lossy()
            .chars()
            .take(31)
            .collect();
        return Ok(Err((
            0,
            format!(
                "Unable to find the socket transport \"{name}\" - did you forget to enable it when you configured PHP?"
            ),
        )));
    }
    let transport = target.transport.as_bytes();
    let r = if server {
        if server_flags & 4 != 0 {
            bind(transport, &target.name, server_flags & 8 != 0)
        } else {
            Err((0, String::new()))
        }
    } else {
        connect(transport, &target.name, timeout)
    };
    match r {
        Ok((sock, label)) => {
            // `timeout` None waits forever (tv_sec == -1)
            let t = if server { None } else { timeout };
            let res = alloc_socket(vm, sock, label, t, persistent_key.is_some());
            if let Some(h) = stream::handle_of(&res) {
                let mut s = h.borrow_mut();
                s.st.context = ctx.cloned();
                s.orig_path = Some(PhpStr::from_bytes(address));
                if server {
                    s.st.flags |= stream::flags::NO_IO;
                }
            }
            if let Some(key) = persistent_key {
                vm.ext_data::<PersistentSockets>()
                    .map
                    .push((key.to_vec(), res.clone()));
            }
            Ok(Ok(res))
        }
        Err(e) => {
            // the factory allocated (and freed) a stream before connecting
            drop(ResRef::new("stream", Box::new(())));
            Ok(Err(e))
        }
    }
}

/// Formats a socket address like `php_network_populate_name_from_sockaddr`.
pub fn format_addr(a: &SocketAddr) -> String {
    match a {
        SocketAddr::V4(v) => format!("{}:{}", v.ip(), v.port()),
        SocketAddr::V6(v) => format!("[{}]:{}", v.ip(), v.port()),
    }
}

fn unix_addr_name(a: &std::os::unix::net::SocketAddr) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    a.as_pathname().map(|p| p.as_os_str().as_bytes().to_vec())
}

/// Result of a successful `accept`: the client socket, its ops label and
/// the peer name.
pub type Accepted = (Sock, &'static str, Option<Vec<u8>>);

impl SocketOps {
    /// `STREAM_XPORT_OP_GET_NAME` / `GET_PEER_NAME`.
    pub fn name(&self, peer: bool) -> Option<Vec<u8>> {
        let s = self.sock.as_ref()?;
        let inet = |r: io::Result<SocketAddr>| r.ok().map(|a| format_addr(&a).into_bytes());
        match s {
            Sock::Tcp(t) => inet(if peer { t.peer_addr() } else { t.local_addr() }),
            Sock::Udp(u) => inet(if peer { u.peer_addr() } else { u.local_addr() }),
            Sock::TcpListener(l) => {
                if peer {
                    None
                } else {
                    inet(l.local_addr())
                }
            }
            Sock::Unix(u) => (if peer { u.peer_addr() } else { u.local_addr() })
                .ok()
                .and_then(|a| unix_addr_name(&a)),
            Sock::UnixDg(u) => (if peer { u.peer_addr() } else { u.local_addr() })
                .ok()
                .and_then(|a| unix_addr_name(&a)),
            Sock::UnixListener(l) => {
                if peer {
                    None
                } else {
                    l.local_addr().ok().and_then(|a| unix_addr_name(&a))
                }
            }
            Sock::Generic(_) => None,
        }
    }

    /// `php_network_accept_incoming`: waits up to `timeout` for a client.
    pub fn accept(&mut self, timeout: Option<Duration>) -> Result<Accepted, String> {
        if self.sock.is_none() {
            return Err("Unknown error".into());
        }
        match self.poll_for(sys::POLLIN, timeout) {
            Ok(0) => return Err(sys::strerror(ETIMEDOUT)),
            Ok(_) => {}
            Err(e) => return Err(sys::strerror(sys::errno_of(&e))),
        }
        match self.sock.as_ref().expect("checked") {
            Sock::TcpListener(l) => match l.accept() {
                Ok((s, a)) => Ok((
                    Sock::Tcp(s),
                    "tcp_socket",
                    Some(format_addr(&a).into_bytes()),
                )),
                Err(e) => Err(sys::strerror(sys::errno_of(&e))),
            },
            Sock::UnixListener(l) => match l.accept() {
                Ok((s, a)) => Ok((Sock::Unix(s), "unix_socket", unix_addr_name(&a))),
                Err(e) => Err(sys::strerror(sys::errno_of(&e))),
            },
            _ => Err(sys::strerror(sys::EINVAL)),
        }
    }

    /// `STREAM_XPORT_OP_RECV` (with the sender's address for datagrams).
    pub fn recvfrom(&mut self, buf: &mut [u8], peek: bool) -> io::Result<(usize, Option<Vec<u8>>)> {
        match self.sock.as_mut() {
            Some(Sock::Udp(u)) => {
                let (n, a) = if peek {
                    u.peek_from(buf)?
                } else {
                    u.recv_from(buf)?
                };
                Ok((n, Some(format_addr(&a).into_bytes())))
            }
            Some(Sock::UnixDg(u)) => {
                let (n, a) = u.recv_from(buf)?;
                Ok((n, unix_addr_name(&a)))
            }
            Some(Sock::Tcp(t)) => {
                let n = if peek { t.peek(buf)? } else { t.read(buf)? };
                Ok((n, t.peer_addr().ok().map(|a| format_addr(&a).into_bytes())))
            }
            Some(s) => Ok((s.read(buf)?, None)),
            None => Err(io::Error::from_raw_os_error(sys::EBADF)),
        }
    }

    /// `STREAM_XPORT_OP_SEND`.
    pub fn sendto(&mut self, buf: &[u8], addr: Option<SocketAddr>) -> io::Result<usize> {
        match (self.sock.as_mut(), addr) {
            (Some(Sock::Udp(u)), Some(a)) => u.send_to(buf, a),
            (Some(s), _) => s.write(buf),
            (None, _) => Err(io::Error::from_raw_os_error(sys::EBADF)),
        }
    }

    pub fn shutdown(&self, how: Shutdown) -> bool {
        self.sock.as_ref().is_some_and(|s| s.shutdown(how).is_ok())
    }
}

/// `socketpair(2)` for the supported domain/type combinations; `Err(errno)`.
pub fn socket_pair(domain: i64, ty: i64) -> Result<(Sock, Sock), i32> {
    if domain != 1 {
        return Err(if cfg!(target_os = "linux") { 95 } else { 45 });
    }
    match ty {
        1 => UnixStream::pair()
            .map(|(a, b)| (Sock::Unix(a), Sock::Unix(b)))
            .map_err(|e| sys::errno_of(&e)),
        2 => UnixDatagram::pair()
            .map(|(a, b)| (Sock::UnixDg(a), Sock::UnixDg(b)))
            .map_err(|e| sys::errno_of(&e)),
        _ => Err(if cfg!(target_os = "linux") { 94 } else { 44 }),
    }
}
