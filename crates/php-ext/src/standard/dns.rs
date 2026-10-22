//! ext/standard/dns.c
//!
//! The record functions run on the pure-Rust stub resolver in [`resolver`]
//! (`/etc/resolv.conf` name servers, search list and `ndots`); the
//! `gethostby*` functions use the system resolver.

// Let-chains are not available at the workspace MSRV, so nested `if let`s stay.
#![allow(clippy::collapsible_if)]

pub mod resolver;

use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::basic::{netfuncs, path_arg, sys};
use resolver::{HErrno, Resolver, dn_expand, dn_skipname};

pub const DNS_A: i64 = 0x0000_0001;
pub const DNS_NS: i64 = 0x0000_0002;
pub const DNS_CNAME: i64 = 0x0000_0010;
pub const DNS_SOA: i64 = 0x0000_0020;
pub const DNS_PTR: i64 = 0x0000_0800;
pub const DNS_HINFO: i64 = 0x0000_1000;
pub const DNS_CAA: i64 = 0x0000_2000;
pub const DNS_MX: i64 = 0x0000_4000;
pub const DNS_TXT: i64 = 0x0000_8000;
pub const DNS_A6: i64 = 0x0100_0000;
pub const DNS_SRV: i64 = 0x0200_0000;
pub const DNS_NAPTR: i64 = 0x0400_0000;
pub const DNS_AAAA: i64 = 0x0800_0000;
pub const DNS_ANY: i64 = 0x1000_0000;
pub const DNS_ALL: i64 = DNS_A
    | DNS_NS
    | DNS_CNAME
    | DNS_SOA
    | DNS_PTR
    | DNS_HINFO
    | DNS_CAA
    | DNS_MX
    | DNS_TXT
    | DNS_A6
    | DNS_SRV
    | DNS_NAPTR
    | DNS_AAAA;
/// `PHP_DNS_NUM_TYPES`
const NUM_TYPES: i64 = 13;

const T_NS: u32 = 2;
const T_CNAME: u32 = 5;
const T_SOA: u32 = 6;
const T_PTR: u32 = 12;
const T_HINFO: u32 = 13;
const T_MX: u32 = 15;
const T_TXT: u32 = 16;
const T_AAAA: u32 = 28;
const T_SRV: u32 = 33;
const T_NAPTR: u32 = 35;
const T_A6: u32 = 38;
const T_CAA: u32 = 257;
const T_ANY: u32 = 255;
const T_A: u32 = 1;

/// `MAXFQDNLEN`
const MAXFQDNLEN: usize = 255;

/// The stub resolver configuration, read from the system on first use.
#[derive(Default)]
struct DnsState {
    resolver: Option<Resolver>,
}

/// Replaces the resolver configuration (tests and embedders).
pub fn set_resolver(vm: &mut Vm, resolver: Resolver) {
    vm.ext_data::<DnsState>().resolver = Some(resolver);
}

fn resolver(vm: &mut Vm) -> Resolver {
    vm.ext_data::<DnsState>()
        .resolver
        .get_or_insert_with(Resolver::system)
        .clone()
}

fn gethostname(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    match sys::gethostname() {
        Some(h) => Ok(Value::String(PhpStr::from_vec(h))),
        None => {
            let e = std::io::Error::last_os_error();
            vm.warning(format!(
                "Unable to fetch host [{}]: {}",
                e.raw_os_error().unwrap_or(0),
                super::basic::os_error_text(&e)
            ))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `php_gethostbyaddr()`: `None` for an invalid address.
fn php_gethostbyaddr(ip: &[u8]) -> Option<Vec<u8>> {
    // not a valid IP otherwise
    let raw: Vec<u8> = match netfuncs::inet_pton6(ip) {
        Some(b) => b.to_vec(),
        None => netfuncs::inet_pton4(ip)?.to_vec(),
    };
    Some(sys::gethostbyaddr(&raw).unwrap_or_else(|| ip.to_vec()))
}

fn gethostbyaddr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let addr = path_arg(vm, args, 0)?;
    match php_gethostbyaddr(&addr) {
        Some(h) => Ok(Value::String(PhpStr::from_vec(h))),
        None => {
            vm.warning("Address is not a valid IPv4 or IPv6 address")?;
            Ok(Value::Bool(false))
        }
    }
}

fn gethostbyname(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hostname = path_arg(vm, args, 0)?;
    if hostname.len() > MAXFQDNLEN {
        // name too long, protect from CVE-2015-0235
        vm.warning(format!(
            "Host name cannot be longer than {MAXFQDNLEN} characters"
        ))?;
        return Ok(Value::String(hostname));
    }
    // php_gethostbyname(): the first IPv4 address, or the name itself
    let found = resolver::gethostbyname_v4(&hostname.to_string_lossy());
    Ok(match found.and_then(|v| v.first().copied()) {
        Some(ip) => Value::from(ip.to_string()),
        None => Value::String(hostname),
    })
}

fn gethostbynamel(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hostname = path_arg(vm, args, 0)?;
    if hostname.len() > MAXFQDNLEN {
        vm.warning(format!(
            "Host name cannot be longer than {MAXFQDNLEN} characters"
        ))?;
        return Ok(Value::Bool(false));
    }
    let Some(ips) = resolver::gethostbyname_v4(&hostname.to_string_lossy()) else {
        return Ok(Value::Bool(false));
    };
    let mut a = PhpArray::new();
    for ip in ips {
        let _ = a.append(Value::from(ip.to_string()));
    }
    Ok(Value::array(a))
}

fn type_from_name(name: &[u8]) -> Option<u32> {
    let n = name.to_ascii_uppercase();
    Some(match n.as_slice() {
        b"A" => T_A,
        b"NS" => T_NS,
        b"MX" => T_MX,
        b"PTR" => T_PTR,
        b"ANY" => T_ANY,
        b"SOA" => T_SOA,
        b"CAA" => T_CAA,
        b"TXT" => T_TXT,
        b"CNAME" => T_CNAME,
        b"AAAA" => T_AAAA,
        b"SRV" => T_SRV,
        b"NAPTR" => T_NAPTR,
        b"A6" => T_A6,
        _ => return None,
    })
}

/// Sets a by-reference parameter to an empty array (`zend_try_array_init`).
fn init_ref_array(args: &Args, i: usize) {
    args.by_ref(i).set(Value::empty_array());
}

fn push_ref_array(args: &Args, i: usize, item: Value) {
    let r = args.by_ref(i);
    let mut cur = match r.get() {
        Value::Array(a) => (*a).clone(),
        _ => PhpArray::new(),
    };
    let _ = cur.append(item);
    r.set(Value::array(cur));
}

fn dns_check_record(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hostname = path_arg(vm, args, 0)?;
    if hostname.is_empty() {
        return Err(vm.argument_value_error(1, "cannot be empty"));
    }
    let mut qtype = T_MX;
    if args.is_passed(1) {
        match type_from_name(args.str(1)) {
            Some(t) => qtype = t,
            None => return Err(vm.argument_value_error(2, "must be a valid DNS record type")),
        }
    }
    let r = resolver(vm);
    let answer = match r.search(&hostname.to_string_lossy(), qtype as u16) {
        Ok(a) => a,
        Err(_) => return Ok(Value::Bool(false)),
    };
    Ok(Value::Bool(u16::from_be_bytes([answer[6], answer[7]]) != 0))
}

/// Formats address words like dns.c does for `AAAA`/`A6` (not the
/// canonical `inet_ntop` form).
struct V6Text {
    out: String,
    have_break: bool,
    in_break: bool,
}

impl V6Text {
    fn new() -> V6Text {
        V6Text {
            out: String::new(),
            have_break: false,
            in_break: false,
        }
    }

    fn word(&mut self, s: u16) {
        if s != 0 {
            if !self.out.is_empty() {
                self.in_break = false;
                self.out.push(':');
            }
            self.out.push_str(&format!("{s:x}"));
        } else {
            self.zero();
        }
    }

    fn zero(&mut self) {
        if !self.have_break {
            self.have_break = true;
            self.in_break = true;
            self.out.push(':');
        } else if !self.in_break {
            self.out.push_str(":0");
        }
    }

    fn finish(mut self) -> String {
        if self.have_break && self.in_break {
            self.out.push(':');
        }
        self.out
    }
}

fn s(bytes: &[u8]) -> Value {
    Value::bytes(bytes)
}

/// `php_parserr()`: parses one resource record at `cp`. `None` stands for
/// php-src's `NULL` return (stop parsing); otherwise the new offset and the
/// record array, if one was produced.
fn parserr(
    msg: &[u8],
    mut cp: usize,
    type_to_fetch: u32,
    store: bool,
    raw: bool,
) -> Option<(usize, Option<PhpArray>)> {
    let end = msg.len();
    macro_rules! checkcp {
        ($n:expr) => {
            if cp + $n > end {
                return None;
            }
        };
    }
    macro_rules! getshort {
        () => {{
            let v = u16::from_be_bytes([msg[cp], msg[cp + 1]]) as i64;
            cp += 2;
            v
        }};
    }
    macro_rules! getlong {
        () => {{
            let v = u32::from_be_bytes([msg[cp], msg[cp + 1], msg[cp + 2], msg[cp + 3]]) as i64;
            cp += 4;
            v
        }};
    }
    macro_rules! expand {
        () => {{
            let (name, next) = dn_expand(msg, cp)?;
            cp = next;
            name
        }};
    }

    let (name, next) = dn_expand(msg, cp)?;
    cp = next;

    checkcp!(10);
    let ty = getshort!() as u32;
    let _class = getshort!();
    let ttl = getlong!();
    let dlen = getshort!() as usize;
    checkcp!(dlen);
    if dlen == 0 {
        // No data in the response - nothing to do
        return None;
    }
    if type_to_fetch != T_ANY && ty != type_to_fetch {
        return Some((cp + dlen, None));
    }
    if !store {
        return Some((cp + dlen, None));
    }

    let mut a = PhpArray::new();
    a.insert_str("host", s(&name));
    a.insert_str("class", "IN");
    a.insert_str("ttl", ttl);

    if raw {
        a.insert_str("type", ty as i64);
        a.insert_str("data", s(&msg[cp..cp + dlen]));
        return Some((cp + dlen, Some(a)));
    }

    match ty {
        T_A => {
            checkcp!(4);
            a.insert_str("type", "A");
            a.insert_str(
                "ip",
                format!(
                    "{}.{}.{}.{}",
                    msg[cp],
                    msg[cp + 1],
                    msg[cp + 2],
                    msg[cp + 3]
                ),
            );
            cp += dlen;
        }
        T_MX | T_CNAME | T_NS | T_PTR => {
            match ty {
                T_MX => {
                    checkcp!(2);
                    a.insert_str("type", "MX");
                    let pri = getshort!();
                    a.insert_str("pri", pri);
                }
                T_CNAME => {
                    a.insert_str("type", "CNAME");
                }
                T_NS => {
                    a.insert_str("type", "NS");
                }
                _ => {
                    a.insert_str("type", "PTR");
                }
            }
            let target = expand!();
            a.insert_str("target", s(&target));
        }
        T_HINFO => {
            // See RFC 1010 for values
            a.insert_str("type", "HINFO");
            checkcp!(1);
            let n = msg[cp] as usize;
            cp += 1;
            checkcp!(n);
            a.insert_str("cpu", s(&msg[cp..cp + n]));
            cp += n;
            checkcp!(1);
            let n = msg[cp] as usize;
            cp += 1;
            checkcp!(n);
            a.insert_str("os", s(&msg[cp..cp + n]));
            cp += n;
        }
        T_CAA => {
            // See RFC 6844 for values
            a.insert_str("type", "CAA");
            checkcp!(1);
            a.insert_str("flags", msg[cp] as i64);
            cp += 1;
            checkcp!(1);
            let n = msg[cp] as usize;
            cp += 1;
            checkcp!(n);
            a.insert_str("tag", s(&msg[cp..cp + n]));
            cp += n;
            if dlen < n + 2 {
                return None;
            }
            let n = dlen - n - 2;
            checkcp!(n);
            a.insert_str("value", s(&msg[cp..cp + n]));
            cp += n;
        }
        T_TXT => {
            a.insert_str("type", "TXT");
            let mut joined: Vec<u8> = Vec::new();
            let mut entries = PhpArray::new();
            let mut l1 = 0usize;
            while l1 < dlen {
                let mut n = msg[cp + l1] as usize;
                if l1 + n >= dlen {
                    // Invalid chunk length, truncate
                    n = dlen - (l1 + 1);
                }
                if n > 0 {
                    let chunk = &msg[cp + l1 + 1..cp + l1 + 1 + n];
                    joined.extend_from_slice(chunk);
                    let _ = entries.append(s(chunk));
                }
                l1 += n + 1;
            }
            cp += dlen;
            a.insert_str("txt", s(&joined));
            a.insert_str("entries", Value::array(entries));
        }
        T_SOA => {
            a.insert_str("type", "SOA");
            let mname = expand!();
            a.insert_str("mname", s(&mname));
            let rname = expand!();
            a.insert_str("rname", s(&rname));
            checkcp!(5 * 4);
            for key in ["serial", "refresh", "retry", "expire", "minimum-ttl"] {
                let v = getlong!();
                a.insert_str(key, v);
            }
        }
        T_AAAA => {
            checkcp!(16);
            let mut t = V6Text::new();
            for _ in 0..8 {
                let w = getshort!() as u16;
                t.word(w);
            }
            a.insert_str("type", "AAAA");
            a.insert_str("ipv6", t.finish());
        }
        T_A6 => {
            let p = cp;
            a.insert_str("type", "A6");
            checkcp!(1);
            let n = msg[cp] as usize;
            cp += 1;
            a.insert_str("masklen", n as i64);
            let mut t = V6Text::new();
            if n > 15 {
                t.have_break = true;
                t.in_break = true;
                t.out.push(':');
            }
            if n % 16 > 8 {
                // Partial short
                checkcp!(1);
                let b = msg[cp];
                if b != 0 {
                    if !t.out.is_empty() {
                        t.in_break = false;
                        t.out.push(':');
                    }
                    t.out.push_str(&format!("{:x}", b));
                } else {
                    t.zero();
                }
                cp += 1;
            }
            for _ in (n + 8) / 16..8 {
                checkcp!(2);
                let w = getshort!() as u16;
                t.word(w);
            }
            a.insert_str("ipv6", t.finish());
            if cp < p + dlen {
                let chain = expand!();
                a.insert_str("chain", s(&chain));
            }
        }
        T_SRV => {
            checkcp!(3 * 2);
            a.insert_str("type", "SRV");
            let v = getshort!();
            a.insert_str("pri", v);
            let v = getshort!();
            a.insert_str("weight", v);
            let v = getshort!();
            a.insert_str("port", v);
            let target = expand!();
            a.insert_str("target", s(&target));
        }
        T_NAPTR => {
            checkcp!(2 * 2);
            a.insert_str("type", "NAPTR");
            let v = getshort!();
            a.insert_str("order", v);
            let v = getshort!();
            a.insert_str("pref", v);
            for key in ["flags", "services", "regex"] {
                checkcp!(1);
                let n = msg[cp] as usize;
                cp += 1;
                checkcp!(n);
                a.insert_str(key, s(&msg[cp..cp + n]));
                cp += n;
            }
            let repl = expand!();
            a.insert_str("replacement", s(&repl));
        }
        _ => {
            // unsupported record type: skipped
            return Some((cp + dlen, None));
        }
    }
    Some((cp, Some(a)))
}

fn dns_get_record(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hostname = path_arg(vm, args, 0)?;
    let type_param = args.int(1);
    let want_authns = args.is_passed(2);
    let want_addtl = args.is_passed(3);
    let raw = args.bool(4);

    if want_authns {
        init_ref_array(args, 2);
    }
    if want_addtl {
        init_ref_array(args, 3);
    }

    if !raw {
        if (type_param & !DNS_ALL) != 0 && type_param != DNS_ANY {
            return Err(vm.argument_value_error(2, "must be a DNS_* constant"));
        }
    } else if !(1..=0xFFFF).contains(&type_param) {
        return Err(vm.argument_value_error(
            2,
            "must be between 1 and 65535 when argument #5 ($raw) is true",
        ));
    }

    let mut ret = PhpArray::new();
    let resolver = resolver(vm);
    let name = hostname.to_string_lossy().into_owned();

    // The type mask is emulated by querying type by type; ANY is queried
    // last when additional records were requested.
    let mut store_results = true;
    let mut first_query = true;
    let mut ty: i64 = if raw {
        -1
    } else if type_param == DNS_ANY {
        NUM_TYPES + 1
    } else {
        0
    };

    while ty < (if want_addtl { NUM_TYPES + 2 } else { NUM_TYPES }) || first_query {
        first_query = false;
        let current = ty;
        ty += 1;
        let pick = |flag: i64, t: u32| if type_param & flag != 0 { t } else { 0 };
        let type_to_fetch: u32 = match current {
            -1 => {
                // raw: skip over the rest and go directly to additional records
                ty = NUM_TYPES;
                type_param as u32
            }
            0 => pick(DNS_A, T_A),
            1 => pick(DNS_NS, T_NS),
            2 => pick(DNS_CNAME, T_CNAME),
            3 => pick(DNS_SOA, T_SOA),
            4 => pick(DNS_PTR, T_PTR),
            5 => pick(DNS_HINFO, T_HINFO),
            6 => pick(DNS_MX, T_MX),
            7 => pick(DNS_TXT, T_TXT),
            8 => pick(DNS_AAAA, T_AAAA),
            9 => pick(DNS_SRV, T_SRV),
            10 => pick(DNS_NAPTR, T_NAPTR),
            11 => pick(DNS_A6, T_A6),
            12 => pick(DNS_CAA, T_CAA),
            NUM_TYPES => {
                store_results = false;
                continue;
            }
            _ => T_ANY,
        };
        if type_to_fetch == 0 {
            continue;
        }

        let answer = match resolver.search(&name, type_to_fetch as u16) {
            Ok(a) => a,
            Err(HErrno::NoData | HErrno::HostNotFound) => continue,
            Err(HErrno::NoRecovery) => {
                vm.warning("An unexpected server failure occurred.")?;
                return Ok(Value::Bool(false));
            }
            Err(HErrno::TryAgain) => {
                vm.warning("A temporary server error occurred.")?;
                return Ok(Value::Bool(false));
            }
        };

        let end = answer.len();
        let mut cp = 12usize;
        let mut qd = u16::from_be_bytes([answer[4], answer[5]]);
        let mut an = u16::from_be_bytes([answer[6], answer[7]]);
        let mut ns = u16::from_be_bytes([answer[8], answer[9]]);
        let mut ar = u16::from_be_bytes([answer[10], answer[11]]);

        // Skip QD entries, they're only used by dn_expand later on
        while qd > 0 {
            qd -= 1;
            match dn_skipname(&answer, cp) {
                Some(n) => cp = n + 4,
                None => {
                    vm.warning("Unable to parse DNS data received")?;
                    return Ok(Value::Bool(false));
                }
            }
        }

        // YAY! Our real answers!
        let mut ok = true;
        while an > 0 && ok && cp < end {
            an -= 1;
            match parserr(&answer, cp, type_to_fetch, store_results, raw) {
                Some((next, rec)) => {
                    cp = next;
                    if let (Some(rec), true) = (rec, store_results) {
                        let _ = ret.append(Value::array(rec));
                    }
                }
                None => ok = false,
            }
        }

        if want_authns || want_addtl {
            // Authoritative name servers (also parsed to get past them when
            // only the additional records are wanted)
            while ns > 0 && ok && cp < end {
                ns -= 1;
                match parserr(&answer, cp, T_ANY, want_authns, raw) {
                    Some((next, rec)) => {
                        cp = next;
                        if let Some(rec) = rec {
                            push_ref_array(args, 2, Value::array(rec));
                        }
                    }
                    None => ok = false,
                }
            }
        }
        if want_addtl {
            // Additional records associated with the authoritative name servers
            while ar > 0 && ok && cp < end {
                ar -= 1;
                match parserr(&answer, cp, T_ANY, true, raw) {
                    Some((next, rec)) => {
                        cp = next;
                        if let Some(rec) = rec {
                            push_ref_array(args, 3, Value::array(rec));
                        }
                    }
                    None => ok = false,
                }
            }
        }
    }
    Ok(Value::array(ret))
}

fn dns_get_mx(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let hostname = path_arg(vm, args, 0)?;
    init_ref_array(args, 1);
    let want_weights = args.is_passed(2);
    if want_weights {
        init_ref_array(args, 2);
    }

    let r = resolver(vm);
    let Ok(answer) = r.search(&hostname.to_string_lossy(), T_MX as u16) else {
        return Ok(Value::Bool(false));
    };
    let end = answer.len();
    let mut cp = 12usize;
    for _ in 0..u16::from_be_bytes([answer[4], answer[5]]) {
        match dn_skipname(&answer, cp) {
            Some(n) => cp = n + 4,
            None => return Ok(Value::Bool(false)),
        }
    }
    let mut count = u16::from_be_bytes([answer[6], answer[7]]) as i32;
    let mut found = 0;
    loop {
        count -= 1;
        if count < 0 || cp >= end {
            break;
        }
        let Some(n) = dn_skipname(&answer, cp) else {
            return Ok(Value::Bool(false));
        };
        cp = n;
        if cp + 10 > end {
            break;
        }
        let ty = u16::from_be_bytes([answer[cp], answer[cp + 1]]) as u32;
        cp += 2 + 2 + 4; // type, class, ttl
        let rdlen = u16::from_be_bytes([answer[cp], answer[cp + 1]]) as usize;
        cp += 2;
        if ty != T_MX {
            cp += rdlen;
            continue;
        }
        if cp + 2 > end {
            return Ok(Value::Bool(false));
        }
        let weight = u16::from_be_bytes([answer[cp], answer[cp + 1]]) as i64;
        cp += 2;
        let Some((name, next)) = dn_expand(&answer, cp) else {
            return Ok(Value::Bool(false));
        };
        cp = next;
        push_ref_array(args, 1, Value::bytes(&name));
        found += 1;
        if want_weights {
            push_ref_array(args, 2, Value::Int(weight));
        }
    }
    Ok(Value::Bool(found != 0))
}

pub static GETHOSTNAME: NativeFunction = NativeFunction {
    sig: "gethostname(): string|false",
    handler: gethostname,
};
pub static GETHOSTBYADDR: NativeFunction = NativeFunction {
    sig: "gethostbyaddr(string $ip): string|false",
    handler: gethostbyaddr,
};
pub static GETHOSTBYNAME: NativeFunction = NativeFunction {
    sig: "gethostbyname(string $hostname): string",
    handler: gethostbyname,
};
pub static GETHOSTBYNAMEL: NativeFunction = NativeFunction {
    sig: "gethostbynamel(string $hostname): array|false",
    handler: gethostbynamel,
};
pub static DNS_CHECK_RECORD: NativeFunction = NativeFunction {
    sig: "dns_check_record(string $hostname, string $type = \"MX\"): bool",
    handler: dns_check_record,
};
pub static CHECKDNSRR: NativeFunction = NativeFunction {
    sig: "checkdnsrr(string $hostname, string $type = \"MX\"): bool",
    handler: dns_check_record,
};
pub static DNS_GET_RECORD: NativeFunction = NativeFunction {
    sig: "dns_get_record(string $hostname, int $type = DNS_ANY, &$authoritative_name_servers = null, &$additional_records = null, bool $raw = false): array|false",
    handler: dns_get_record,
};
pub static DNS_GET_MX_FN: NativeFunction = NativeFunction {
    sig: "dns_get_mx(string $hostname, &$hosts, &$weights = null): bool",
    handler: dns_get_mx,
};
pub static GETMXRR: NativeFunction = NativeFunction {
    sig: "getmxrr(string $hostname, &$hosts, &$weights = null): bool",
    handler: dns_get_mx,
};

pub fn register(r: &mut Registry) {
    for (name, v) in [
        ("DNS_A", DNS_A),
        ("DNS_NS", DNS_NS),
        ("DNS_CNAME", DNS_CNAME),
        ("DNS_SOA", DNS_SOA),
        ("DNS_PTR", DNS_PTR),
        ("DNS_HINFO", DNS_HINFO),
        ("DNS_CAA", DNS_CAA),
        ("DNS_MX", DNS_MX),
        ("DNS_TXT", DNS_TXT),
        ("DNS_SRV", DNS_SRV),
        ("DNS_NAPTR", DNS_NAPTR),
        ("DNS_AAAA", DNS_AAAA),
        ("DNS_A6", DNS_A6),
        ("DNS_ANY", DNS_ANY),
        ("DNS_ALL", DNS_ALL),
    ] {
        r.constant(name, v);
    }
    r.functions(&[
        &GETHOSTNAME,
        &GETHOSTBYADDR,
        &GETHOSTBYNAME,
        &GETHOSTBYNAMEL,
        &DNS_CHECK_RECORD,
        &CHECKDNSRR,
        &DNS_GET_RECORD,
        &DNS_GET_MX_FN,
        &GETMXRR,
    ]);
}

#[cfg(test)]
mod tests;
