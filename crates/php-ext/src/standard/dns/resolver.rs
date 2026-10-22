//! A small stub resolver: the part of the BIND resolver library
//! (`res_nsearch`, `dn_expand`, `dn_skipname`) that `dns.c` uses, in pure
//! Rust (UDP with a TCP retry for truncated answers).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use crate::standard::basic::sys;

pub const T_A: u16 = 1;
pub const C_IN: u16 = 1;

/// `h_errno` values reported by a failed search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HErrno {
    HostNotFound,
    TryAgain,
    NoRecovery,
    NoData,
}

/// Resolver configuration (`resolv.conf`).
#[derive(Clone, Debug)]
pub struct Resolver {
    pub servers: Vec<SocketAddr>,
    /// The search list (`search` / `domain`).
    pub search: Vec<String>,
    pub ndots: usize,
    pub timeout: Duration,
    pub attempts: usize,
}

impl Default for Resolver {
    fn default() -> Resolver {
        Resolver {
            servers: Vec::new(),
            search: Vec::new(),
            ndots: 1,
            timeout: Duration::from_secs(5),
            attempts: 2,
        }
    }
}

impl Resolver {
    /// The system configuration from `/etc/resolv.conf`.
    pub fn system() -> Resolver {
        let text = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        let hostname = sys::gethostname()
            .map(|h| String::from_utf8_lossy(&h).into_owned())
            .unwrap_or_default();
        Resolver::from_resolv_conf(&text, &hostname)
    }

    /// Parses `resolv.conf` text; `hostname` supplies the default domain.
    pub fn from_resolv_conf(text: &str, hostname: &str) -> Resolver {
        let mut r = Resolver::default();
        let mut domain: Option<String> = None;
        let mut search: Option<Vec<String>> = None;
        for line in text.lines() {
            let line = line.split(['#', ';']).next().unwrap_or("").trim();
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("nameserver") => {
                    if let Some(a) = parts.next() {
                        let a = a.split('%').next().unwrap_or(a);
                        if let Ok(ip) = a.parse::<std::net::IpAddr>() {
                            if r.servers.len() < 3 {
                                r.servers.push(SocketAddr::new(ip, 53));
                            }
                        }
                    }
                }
                Some("domain") => domain = parts.next().map(str::to_string),
                Some("search") => search = Some(parts.map(str::to_string).collect()),
                Some("options") => {
                    for opt in parts {
                        if let Some(v) = opt.strip_prefix("ndots:") {
                            if let Ok(n) = v.parse::<usize>() {
                                r.ndots = n.min(15);
                            }
                        } else if let Some(v) = opt.strip_prefix("timeout:") {
                            if let Ok(n) = v.parse::<u64>() {
                                r.timeout = Duration::from_secs(n.clamp(1, 30));
                            }
                        } else if let Some(v) = opt.strip_prefix("attempts:") {
                            if let Ok(n) = v.parse::<usize>() {
                                r.attempts = n.clamp(1, 5);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if r.servers.is_empty() {
            r.servers
                .push(SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), 53));
        }
        r.search = match (search, domain) {
            (Some(s), _) => s,
            (None, Some(d)) => vec![d],
            (None, None) => match hostname.split_once('.') {
                Some((_, d)) if !d.is_empty() => vec![d.to_string()],
                _ => Vec::new(),
            },
        };
        r
    }

    /// `res_nsearch(name, C_IN, qtype)`: the full answer message, or the
    /// `h_errno` of the failure.
    pub fn search(&self, name: &str, qtype: u16) -> Result<Vec<u8>, HErrno> {
        let dots = name.bytes().filter(|&c| c == b'.').count();
        let trailing_dot = name.ends_with('.');
        let mut saved: Option<HErrno> = None;
        let mut tried_as_is = false;
        let (mut got_nodata, mut got_servfail) = (false, false);

        if dots >= self.ndots || trailing_dot {
            match self.query_domain(name, None, qtype) {
                Ok(a) => return Ok(a),
                Err((e, _)) => {
                    if trailing_dot {
                        return Err(e);
                    }
                    saved = Some(e);
                }
            }
            tried_as_is = true;
        }
        if (dots == 0) || (dots > 0 && !trailing_dot) {
            for domain in &self.search {
                match self.query_domain(name, Some(domain), qtype) {
                    Ok(a) => return Ok(a),
                    Err((HErrno::NoData, _)) => got_nodata = true,
                    Err((HErrno::HostNotFound, _)) => {}
                    Err((HErrno::TryAgain, Some(2))) => got_servfail = true,
                    Err(_) => break,
                }
            }
        }
        if !tried_as_is {
            match self.query_domain(name, None, qtype) {
                Ok(a) => return Ok(a),
                Err((e, _)) => saved = saved.or(Some(e)),
            }
        }
        Err(match saved {
            Some(e) => e,
            None if got_nodata => HErrno::NoData,
            None if got_servfail => HErrno::TryAgain,
            None => HErrno::HostNotFound,
        })
    }

    /// `res_nquerydomain()` + `res_nquery()`: one name, one query.
    fn query_domain(
        &self,
        name: &str,
        domain: Option<&str>,
        qtype: u16,
    ) -> Result<Vec<u8>, (HErrno, Option<u8>)> {
        let full = match domain {
            None => name.trim_end_matches('.').to_string(),
            Some(d) => format!("{}.{}", name.trim_end_matches('.'), d.trim_end_matches('.')),
        };
        let Some(qname) = encode_name(&full) else {
            return Err((HErrno::NoRecovery, None));
        };
        let mut id = [0u8; 2];
        sys::random_bytes_or_fallback(&mut id);
        let mut packet = Vec::with_capacity(32);
        packet.extend_from_slice(&id);
        packet.extend_from_slice(&[0x01, 0x00]); // RD
        packet.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        packet.extend_from_slice(&qname);
        packet.extend_from_slice(&qtype.to_be_bytes());
        packet.extend_from_slice(&C_IN.to_be_bytes());

        let answer = match self.send(&packet) {
            Ok(a) => a,
            Err(_) => return Err((HErrno::TryAgain, None)),
        };
        if answer.len() < 12 {
            return Err((HErrno::NoRecovery, None));
        }
        let rcode = answer[3] & 0x0f;
        let ancount = u16::from_be_bytes([answer[6], answer[7]]);
        if rcode != 0 || ancount == 0 {
            let e = match rcode {
                3 => HErrno::HostNotFound,
                2 => HErrno::TryAgain,
                0 => HErrno::NoData,
                _ => HErrno::NoRecovery,
            };
            return Err((e, Some(rcode)));
        }
        Ok(answer)
    }

    /// `res_nsend()`: tries every server up to `attempts` times.
    fn send(&self, packet: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut last = std::io::Error::new(std::io::ErrorKind::TimedOut, "no answer");
        for _ in 0..self.attempts {
            for server in &self.servers {
                match self.exchange_udp(*server, packet) {
                    Ok(a) => {
                        // truncated: ask again over TCP
                        if a.len() >= 4 && a[2] & 0x02 != 0 {
                            if let Ok(t) = self.exchange_tcp(*server, packet) {
                                return Ok(t);
                            }
                        }
                        return Ok(a);
                    }
                    Err(e) => last = e,
                }
            }
        }
        Err(last)
    }

    fn exchange_udp(&self, server: SocketAddr, packet: &[u8]) -> std::io::Result<Vec<u8>> {
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0".parse().expect("valid")
        } else {
            "[::]:0".parse().expect("valid")
        };
        let sock = UdpSocket::bind(bind)?;
        sock.connect(server)?;
        sock.set_read_timeout(Some(self.timeout))?;
        sock.send(packet)?;
        let mut buf = vec![0u8; 65535];
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let n = sock.recv(&mut buf)?;
            // ignore datagrams that are not the answer to this query
            if n >= 12 && buf[..2] == packet[..2] && buf[2] & 0x80 != 0 {
                buf.truncate(n);
                return Ok(buf);
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout"));
            }
            sock.set_read_timeout(Some(left))?;
        }
    }

    fn exchange_tcp(&self, server: SocketAddr, packet: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut s = TcpStream::connect_timeout(&server, self.timeout)?;
        s.set_read_timeout(Some(self.timeout))?;
        s.set_write_timeout(Some(self.timeout))?;
        let mut framed = (packet.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(packet);
        s.write_all(&framed)?;
        let mut len = [0u8; 2];
        s.read_exact(&mut len)?;
        let mut buf = vec![0u8; u16::from_be_bytes(len) as usize];
        s.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Encodes a domain name in wire format; `None` when a label or the whole
/// name is too long.
pub fn encode_name(name: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let name = name.trim_end_matches('.');
    if !name.is_empty() {
        for label in name.split('.') {
            let l = label.as_bytes();
            if l.is_empty() || l.len() > 63 {
                return None;
            }
            out.push(l.len() as u8);
            out.extend_from_slice(l);
        }
    }
    out.push(0);
    if out.len() > 255 {
        return None;
    }
    Some(out)
}

/// `dn_expand()`: the text of the name at `pos` and the offset of the
/// first byte after it (following compression pointers).
pub fn dn_expand(msg: &[u8], pos: usize) -> Option<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut p = pos;
    let mut next: Option<usize> = None;
    let mut hops = 0;
    loop {
        let len = *msg.get(p)? as usize;
        match len & 0xc0 {
            0x00 => {
                p += 1;
                if len == 0 {
                    break;
                }
                let label = msg.get(p..p + len)?;
                if !out.is_empty() {
                    out.push(b'.');
                }
                for &c in label {
                    if matches!(c, b'.' | b';' | b'\\' | b'(' | b')' | b'@' | b'$' | b'"') {
                        out.push(b'\\');
                        out.push(c);
                    } else if c <= 0x20 || c >= 0x7f {
                        out.extend_from_slice(format!("\\{c:03}").as_bytes());
                    } else {
                        out.push(c);
                    }
                }
                p += len;
                if out.len() > 1024 {
                    return None;
                }
            }
            0xc0 => {
                let lo = *msg.get(p + 1)? as usize;
                if next.is_none() {
                    next = Some(p + 2);
                }
                p = ((len & 0x3f) << 8) | lo;
                hops += 1;
                if hops > 128 || p >= msg.len() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if out.is_empty() {
        out.push(b'.');
    }
    Some((out, next.unwrap_or(p)))
}

/// `dn_skipname()`: the offset after the (possibly compressed) name.
pub fn dn_skipname(msg: &[u8], pos: usize) -> Option<usize> {
    let mut p = pos;
    loop {
        let len = *msg.get(p)? as usize;
        match len & 0xc0 {
            0x00 => {
                p += 1;
                if len == 0 {
                    return Some(p);
                }
                p += len;
                if p > msg.len() {
                    return None;
                }
            }
            0xc0 => {
                msg.get(p + 1)?;
                return Some(p + 2);
            }
            _ => return None,
        }
    }
}

/// `gethostbyname()` for IPv4: every address the system resolver knows.
pub fn gethostbyname_v4(name: &str) -> Option<Vec<std::net::Ipv4Addr>> {
    let addrs = (name, 0).to_socket_addrs().ok()?;
    let mut out: Vec<std::net::Ipv4Addr> = Vec::new();
    for a in addrs {
        if let SocketAddr::V4(v4) = a {
            if !out.contains(v4.ip()) {
                out.push(*v4.ip());
            }
        }
    }
    (!out.is_empty()).then_some(out)
}
