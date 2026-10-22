//! `basic_functions.c`: `inet_ntop`, `inet_pton`, `ip2long`, `long2ip`,
//! `getservbyname`, `getservbyport`, `getprotobyname`, `getprotobynumber`.
//!
//! The address text conversions are ports of glibc's `inet_ntop`/`inet_pton`
//! (the behaviour php-src's tests expect on Linux).

use std::net::IpAddr;

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::{path_arg, sys};

/// `inet_ntop4`: dotted-quad text of an IPv4 address.
pub fn inet_ntop4(b: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

/// `inet_ntop6` (glibc): `::` compresses the longest zero run, and the last
/// 32 bits print as dotted-quad for IPv4-compatible and IPv4-mapped addresses.
pub fn inet_ntop6(src: [u8; 16]) -> String {
    let mut words = [0u16; 8];
    for (i, w) in words.iter_mut().enumerate() {
        *w = u16::from_be_bytes([src[2 * i], src[2 * i + 1]]);
    }
    // Longest run of zero words (first one wins ties).
    let (mut best_base, mut best_len): (isize, usize) = (-1, 0);
    let (mut cur_base, mut cur_len): (isize, usize) = (-1, 0);
    for (i, &w) in words.iter().enumerate() {
        if w == 0 {
            if cur_base == -1 {
                cur_base = i as isize;
                cur_len = 1;
            } else {
                cur_len += 1;
            }
        } else if cur_base != -1 {
            if best_base == -1 || cur_len > best_len {
                best_base = cur_base;
                best_len = cur_len;
            }
            cur_base = -1;
        }
    }
    if cur_base != -1 && (best_base == -1 || cur_len > best_len) {
        best_base = cur_base;
        best_len = cur_len;
    }
    if best_base != -1 && best_len < 2 {
        best_base = -1;
    }

    let mut out = String::new();
    for i in 0..8usize {
        if best_base != -1 && (i as isize) >= best_base && i < best_base as usize + best_len {
            if i as isize == best_base {
                out.push(':');
            }
            continue;
        }
        if i != 0 {
            out.push(':');
        }
        if i == 6 && best_base == 0 && (best_len == 6 || (best_len == 5 && words[5] == 0xffff)) {
            out.push_str(&inet_ntop4([src[12], src[13], src[14], src[15]]));
            break;
        }
        out.push_str(&format!("{:x}", words[i]));
    }
    if best_base != -1 && best_base as usize + best_len == 8 {
        out.push(':');
    }
    out
}

/// Text form of an address as `inet_ntop(3)` prints it.
pub fn inet_ntop_ip(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(a) => inet_ntop4(a.octets()),
        IpAddr::V6(a) => inet_ntop6(a.octets()),
    }
}

/// glibc `inet_pton4`: exactly four decimal octets without leading zeros.
pub fn inet_pton4(src: &[u8]) -> Option<[u8; 4]> {
    let mut tmp = [0u8; 4];
    let (mut saw_digit, mut octets, mut idx) = (false, 0, 0usize);
    for &ch in src {
        if ch.is_ascii_digit() {
            let new = tmp[idx] as u32 * 10 + (ch - b'0') as u32;
            if saw_digit && tmp[idx] == 0 {
                return None;
            }
            if new > 255 {
                return None;
            }
            tmp[idx] = new as u8;
            if !saw_digit {
                octets += 1;
                if octets > 4 {
                    return None;
                }
                saw_digit = true;
            }
        } else if ch == b'.' && saw_digit {
            if octets == 4 {
                return None;
            }
            idx += 1;
            tmp[idx] = 0;
            saw_digit = false;
        } else {
            return None;
        }
    }
    if octets < 4 {
        return None;
    }
    Some(tmp)
}

/// glibc `inet_pton6`.
pub fn inet_pton6(src: &[u8]) -> Option<[u8; 16]> {
    let mut tmp = [0u8; 16];
    let mut tp = 0usize;
    let mut colonp: Option<usize> = None;
    let mut i = 0usize;
    if src.is_empty() {
        return None;
    }
    if src[0] == b':' {
        i += 1;
        if i == src.len() || src[i] != b':' {
            return None;
        }
    }
    let mut curtok = i;
    let mut xdigits_seen = 0;
    let mut val: u32 = 0;
    while i < src.len() {
        let ch = src[i];
        i += 1;
        if let Some(d) = (ch as char).to_digit(16) {
            if xdigits_seen == 4 {
                return None;
            }
            val = (val << 4) | d;
            if val > 0xffff {
                return None;
            }
            xdigits_seen += 1;
            continue;
        }
        if ch == b':' {
            curtok = i;
            if xdigits_seen == 0 {
                if colonp.is_some() {
                    return None;
                }
                colonp = Some(tp);
                continue;
            } else if i == src.len() {
                return None;
            }
            if tp + 2 > 16 {
                return None;
            }
            tmp[tp] = (val >> 8) as u8;
            tmp[tp + 1] = val as u8;
            tp += 2;
            xdigits_seen = 0;
            val = 0;
            continue;
        }
        if ch == b'.' && tp + 4 <= 16 {
            if let Some(v4) = inet_pton4(&src[curtok..]) {
                tmp[tp..tp + 4].copy_from_slice(&v4);
                tp += 4;
                xdigits_seen = 0;
                break;
            }
        }
        return None;
    }
    if xdigits_seen > 0 {
        if tp + 2 > 16 {
            return None;
        }
        tmp[tp] = (val >> 8) as u8;
        tmp[tp + 1] = val as u8;
        tp += 2;
    }
    if let Some(cp) = colonp {
        if tp == 16 {
            return None;
        }
        let n = tp - cp;
        for k in 1..=n {
            tmp[16 - k] = tmp[cp + n - k];
            tmp[cp + n - k] = 0;
        }
        tp = 16;
    }
    if tp != 16 {
        return None;
    }
    Some(tmp)
}

fn inet_ntop(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let a = args.str(0);
    Ok(match a.len() {
        16 => {
            let mut b = [0u8; 16];
            b.copy_from_slice(a);
            Value::from(inet_ntop6(b))
        }
        4 => Value::from(inet_ntop4([a[0], a[1], a[2], a[3]])),
        _ => Value::Bool(false),
    })
}

fn inet_pton(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let address = path_arg(vm, args, 0)?;
    if address.contains(&b':') {
        return Ok(match inet_pton6(&address) {
            Some(b) => Value::bytes(&b),
            None => Value::Bool(false),
        });
    }
    if !address.contains(&b'.') {
        return Ok(Value::Bool(false));
    }
    Ok(match inet_pton4(&address) {
        Some(b) => Value::bytes(&b),
        None => Value::Bool(false),
    })
}

fn ip2long(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let addr = path_arg(vm, args, 0)?;
    if addr.is_empty() {
        return Err(vm.argument_value_error(1, "cannot be empty"));
    }
    Ok(match inet_pton4(&addr) {
        Some(b) => Value::Int(u32::from_be_bytes(b) as i64),
        None => Value::Bool(false),
    })
}

fn long2ip(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ip = args.int(0) as u64 as u32;
    Ok(Value::from(inet_ntop4(ip.to_be_bytes())))
}

fn getservbyname(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = path_arg(vm, args, 0)?;
    let proto = path_arg(vm, args, 1)?;
    Ok(match sys::getservbyname(&name, &proto) {
        Some(p) => Value::Int(p),
        None => Value::Bool(false),
    })
}

fn getservbyport(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let proto = path_arg(vm, args, 1)?;
    // `htons((unsigned short) port)`
    let port = args.int(0) as u16;
    Ok(match sys::getservbyport(port, &proto) {
        Some(n) => Value::String(PhpStr::from_vec(n)),
        None => Value::Bool(false),
    })
}

fn getprotobyname(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = path_arg(vm, args, 0)?;
    Ok(match sys::getprotobyname(&name) {
        Some(p) => Value::Int(p),
        None => Value::Bool(false),
    })
}

fn getprotobynumber(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match sys::getprotobynumber(args.int(0) as i32) {
        Some(n) => Value::String(PhpStr::from_vec(n)),
        None => Value::Bool(false),
    })
}

pub static IP2LONG: NativeFunction = NativeFunction {
    sig: "ip2long(string $ip): int|false",
    handler: ip2long,
};
pub static LONG2IP: NativeFunction = NativeFunction {
    sig: "long2ip(int $ip): string",
    handler: long2ip,
};
pub static INET_NTOP: NativeFunction = NativeFunction {
    sig: "inet_ntop(string $ip): string|false",
    handler: inet_ntop,
};
pub static INET_PTON: NativeFunction = NativeFunction {
    sig: "inet_pton(string $ip): string|false",
    handler: inet_pton,
};
pub static GETSERVBYNAME: NativeFunction = NativeFunction {
    sig: "getservbyname(string $service, string $protocol): int|false",
    handler: getservbyname,
};
pub static GETSERVBYPORT: NativeFunction = NativeFunction {
    sig: "getservbyport(int $port, string $protocol): string|false",
    handler: getservbyport,
};
pub static GETPROTOBYNAME: NativeFunction = NativeFunction {
    sig: "getprotobyname(string $protocol): int|false",
    handler: getprotobyname,
};
pub static GETPROTOBYNUMBER: NativeFunction = NativeFunction {
    sig: "getprotobynumber(int $protocol): string|false",
    handler: getprotobynumber,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &IP2LONG,
        &LONG2IP,
        &INET_NTOP,
        &INET_PTON,
        &GETSERVBYNAME,
        &GETSERVBYPORT,
        &GETPROTOBYNAME,
        &GETPROTOBYNUMBER,
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn s(v: &Value) -> String {
        v.as_str()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap()
    }

    #[test]
    fn ntop6_matches_glibc() {
        let p = |t: &str| inet_pton6(t.as_bytes()).unwrap();
        for (input, want) in [
            ("::1", "::1"),
            ("::", "::"),
            ("2001:db8::ff00:42:8329", "2001:db8::ff00:42:8329"),
            ("2001:0db8:0000:0000:0000:0000:0000:0001", "2001:db8::1"),
            ("1:0:0:2:0:0:0:3", "1:0:0:2::3"),
            ("1:0:2:3:4:5:6:7", "1:0:2:3:4:5:6:7"),
            ("::ffff:1.2.3.4", "::ffff:1.2.3.4"),
            ("::1.2.3.4", "::1.2.3.4"),
            ("::2", "::2"),
            ("fe80::", "fe80::"),
        ] {
            assert_eq!(inet_ntop6(p(input)), want, "{input}");
        }
    }

    #[test]
    fn pton6_rejects_garbage() {
        for bad in [
            ":",
            ":::",
            "1::2::3",
            "12345::",
            "1:2:3:4:5:6:7:8:9",
            "::g",
            "1:",
            ":1",
        ] {
            assert!(inet_pton6(bad.as_bytes()).is_none(), "{bad}");
        }
        assert!(inet_pton6(b"1:2:3:4:5:6:7:8").is_some());
        assert!(inet_pton6(b"::1.2.3.4").is_some());
        assert!(inet_pton6(b"1:2:3:4:5:6:1.2.3.4").is_some());
    }

    #[test]
    fn pton4_is_strict() {
        assert_eq!(inet_pton4(b"1.2.3.4"), Some([1, 2, 3, 4]));
        for bad in [
            "1.2.3",
            "1.2.3.4.5",
            "01.2.3.4",
            "256.1.1.1",
            "1..2.3",
            "a.b.c.d",
            ".1.2.3",
            "1.2.3.4 ",
        ] {
            assert!(inet_pton4(bad.as_bytes()).is_none(), "{bad}");
        }
    }

    #[test]
    fn ip2long_long2ip() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("ip2long", vec![Value::str("192.0.34.166")])
                .unwrap()
                .as_int(),
            Some(3221234342)
        );
        assert!(matches!(
            t.call("ip2long", vec![Value::str("192.0.34.166.1")])
                .unwrap(),
            Value::Bool(false)
        ));
        let r = t.call("ip2long", vec![Value::str("")]);
        let (c, m) = thrown(r);
        assert_eq!(
            (c.as_str(), m.as_str()),
            ("ValueError", "ip2long(): Argument #1 ($ip) cannot be empty")
        );
        let r = t.call("ip2long", vec![Value::bytes(b"1.2\0.3.4")]);
        let (c, m) = thrown(r);
        assert_eq!(c, "ValueError");
        assert_eq!(
            m,
            "ip2long(): Argument #1 ($ip) must not contain any null bytes"
        );

        assert_eq!(
            s(&t.call("long2ip", vec![Value::Int(3221234342)]).unwrap()),
            "192.0.34.166"
        );
        assert_eq!(
            s(&t.call("long2ip", vec![Value::Int(-1)]).unwrap()),
            "255.255.255.255"
        );
        assert_eq!(
            s(&t.call("long2ip", vec![Value::Int(0)]).unwrap()),
            "0.0.0.0"
        );
        // only the low 32 bits count
        assert_eq!(
            s(&t.call("long2ip", vec![Value::Int(0x1_0000_0001)]).unwrap()),
            "0.0.0.1"
        );
    }

    #[test]
    fn inet_functions() {
        let mut t = TestVm::new();
        let v4 = t.call("inet_pton", vec![Value::str("127.0.0.1")]).unwrap();
        assert_eq!(v4.as_str().unwrap().as_bytes(), [127, 0, 0, 1]);
        assert_eq!(s(&t.call("inet_ntop", vec![v4]).unwrap()), "127.0.0.1");
        let v6 = t.call("inet_pton", vec![Value::str("::1")]).unwrap();
        assert_eq!(v6.as_str().unwrap().len(), 16);
        assert_eq!(s(&t.call("inet_ntop", vec![v6]).unwrap()), "::1");
        assert!(matches!(
            t.call("inet_pton", vec![Value::str("nope")]).unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("inet_pton", vec![Value::str("1.2.3")]).unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("inet_ntop", vec![Value::str("abc")]).unwrap(),
            Value::Bool(false)
        ));
    }

    #[test]
    fn protocol_database() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("getprotobyname", vec![Value::str("tcp")])
                .unwrap()
                .as_int(),
            Some(6)
        );
        assert_eq!(
            s(&t.call("getprotobynumber", vec![Value::Int(17)]).unwrap()),
            "udp"
        );
        assert!(matches!(
            t.call("getprotobyname", vec![Value::str("no-such-proto")])
                .unwrap(),
            Value::Bool(false)
        ));
    }

    #[test]
    fn service_database() {
        // Skipped silently when the host has no services database.
        let mut t = TestVm::new();
        if let Value::Int(p) = t
            .call("getservbyname", vec![Value::str("http"), Value::str("tcp")])
            .unwrap()
        {
            assert_eq!(p, 80);
            assert_eq!(
                s(
                    &t.call("getservbyport", vec![Value::Int(80), Value::str("tcp")])
                        .unwrap()
                ),
                "http"
            );
        }
        assert!(matches!(
            t.call(
                "getservbyname",
                vec![Value::str("no-such-service"), Value::str("tcp")]
            )
            .unwrap(),
            Value::Bool(false)
        ));
    }
}
