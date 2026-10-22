//! ext/standard/url.c — `parse_url()`, `urlencode()`, `urldecode()`,
//! `rawurlencode()` and `rawurldecode()`, plus the URL helpers other modules
//! reuse (`php_url_parse`, `php_url_encode`, `php_raw_url_decode`, ...).
//!
//! `get_headers()` also lives in url.c but needs the stream layer, so it is
//! not ported here yet.

use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `PHP_URL_*` component identifiers for `parse_url()` (url.h).
pub const PHP_URL_SCHEME: i64 = 0;
pub const PHP_URL_HOST: i64 = 1;
pub const PHP_URL_PORT: i64 = 2;
pub const PHP_URL_USER: i64 = 3;
pub const PHP_URL_PASS: i64 = 4;
pub const PHP_URL_PATH: i64 = 5;
pub const PHP_URL_QUERY: i64 = 6;
pub const PHP_URL_FRAGMENT: i64 = 7;

/// `http_build_query()` encodings (url.h), registered here next to `PHP_URL_*`
/// as in php-src's `url.h`.
pub const PHP_QUERY_RFC1738: i64 = 1;
pub const PHP_QUERY_RFC3986: i64 = 2;

/// `php_url`: the components found by [`php_url_parse`]. Every component has
/// its control characters replaced by `_`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PhpUrl {
    pub scheme: Option<Vec<u8>>,
    pub user: Option<Vec<u8>>,
    pub pass: Option<Vec<u8>>,
    pub host: Option<Vec<u8>>,
    /// The port; only meaningful when `has_port` is set (it may be 0).
    pub port: u16,
    /// The `has_port` out-parameter of `php_url_parse_ex2`.
    pub has_port: bool,
    pub path: Option<Vec<u8>>,
    pub query: Option<Vec<u8>>,
    pub fragment: Option<Vec<u8>>,
}

/// A component with `php_replace_controlchars()` applied (C-locale `iscntrl`).
fn component(bytes: &[u8]) -> Option<Vec<u8>> {
    Some(
        bytes
            .iter()
            .map(|&c| if c < 0x20 || c == 0x7f { b'_' } else { c })
            .collect(),
    )
}

/// `binary_strcspn(s, e, chars)`: the first position in `s..e` holding one of
/// `chars`, or `e`.
fn strcspn(str: &[u8], s: usize, e: usize, chars: &[u8]) -> usize {
    str[s..e]
        .iter()
        .position(|c| chars.contains(c))
        .map_or(e, |p| s + p)
}

fn memchr(str: &[u8], s: usize, e: usize, c: u8) -> Option<usize> {
    str[s..e].iter().position(|&b| b == c).map(|p| s + p)
}

fn memrchr(str: &[u8], s: usize, e: usize, c: u8) -> Option<usize> {
    str[s..e].iter().rposition(|&b| b == c).map(|p| s + p)
}

/// Whether `str[s..]` starts with `//` (a relative-scheme URL).
fn is_relative_scheme(str: &[u8], s: usize) -> bool {
    s + 1 < str.len() && str[s] == b'/' && str[s + 1] == b'/'
}

/// `ZEND_STRTOL(port_buf, &end, 10)` on a port candidate (at most 5 bytes):
/// leading whitespace and a sign are accepted, trailing garbage is ignored,
/// and `None` means no digits were converted (`end == port_buf`).
fn strtol_port(buf: &[u8]) -> Option<i64> {
    let mut i = buf
        .iter()
        .position(|&c| !matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .unwrap_or(buf.len());
    let negative = match buf.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let digits = buf[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let value = buf[i..i + digits]
        .iter()
        .fold(0i64, |acc, &d| acc * 10 + i64::from(d - b'0'));
    Some(if negative { -value } else { value })
}

/// Where the `php_url_parse_ex2` state machine continues.
enum Next {
    /// `parse_port`, with `e` at the ':' that may introduce a port.
    ParsePort(usize),
    ParseHost,
    JustPath,
}

/// `php_url_parse_ex2()`: splits a URL into its components, or `None` for a
/// seriously malformed URL. Like the C code this is not a validating parser:
/// it accepts relative URLs, bare `host:port` pairs and scheme-only strings.
pub fn php_url_parse(str: &[u8]) -> Option<PhpUrl> {
    let ue = str.len();
    let mut ret = PhpUrl::default();
    let mut s = 0;

    // Parse the scheme.
    let next = match memchr(str, 0, ue, b':') {
        Some(e) if e != s => 'scheme: {
            // scheme = 1*[ lowalpha | digit | "+" | "-" | "." ]
            let valid = str[s..e]
                .iter()
                .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'.' | b'-'));
            if !valid {
                if e + 1 < ue && e < strcspn(str, s, ue, b"?#") {
                    break 'scheme Next::ParsePort(e);
                } else if is_relative_scheme(str, s) {
                    s += 2;
                    break 'scheme Next::ParseHost;
                } else {
                    break 'scheme Next::JustPath;
                }
            }

            if e + 1 == ue {
                // Only the scheme is available.
                ret.scheme = component(&str[s..e]);
                return Some(ret);
            }

            if str[e + 1] != b'/' {
                // Schemes like mailto: and zlib: may not have any '/' after
                // them; a run of digits is a port instead (a.com:80).
                let mut p = e + 1;
                while p < ue && str[p].is_ascii_digit() {
                    p += 1;
                }
                if (p == ue || str[p] == b'/') && p - e < 7 {
                    break 'scheme Next::ParsePort(e);
                }
                ret.scheme = component(&str[s..e]);
                s = e + 1;
                Next::JustPath
            } else {
                let is_file = str[s..e].eq_ignore_ascii_case(b"file");
                ret.scheme = component(&str[s..e]);
                if e + 2 < ue && str[e + 2] == b'/' {
                    s = e + 3;
                    if is_file && e + 3 < ue && str[e + 3] == b'/' {
                        // Windows drive letters: file:///c:/somedir/file.txt
                        if e + 5 < ue && str[e + 5] == b':' {
                            s = e + 4;
                        }
                        break 'scheme Next::JustPath;
                    }
                    Next::ParseHost
                } else {
                    s = e + 1;
                    Next::JustPath
                }
            }
        }
        // No scheme; starts with a colon: look for a port.
        Some(e) => Next::ParsePort(e),
        None if is_relative_scheme(str, s) => {
            s += 2;
            Next::ParseHost
        }
        None => Next::JustPath,
    };

    let next = match next {
        Next::ParsePort(e) => {
            let p = e + 1;
            let mut pp = p;
            while pp < ue && pp - p < 6 && str[pp].is_ascii_digit() {
                pp += 1;
            }
            if pp - p > 0 && pp - p < 6 && (pp == ue || str[pp] == b'/') {
                match strtol_port(&str[p..pp]) {
                    Some(port @ 0..=65535) => {
                        ret.has_port = true;
                        ret.port = port as u16;
                        if is_relative_scheme(str, s) {
                            s += 2;
                        }
                    }
                    _ => return None,
                }
                Next::ParseHost
            } else if p == pp && pp == ue {
                return None;
            } else if is_relative_scheme(str, s) {
                s += 2;
                Next::ParseHost
            } else {
                Next::JustPath
            }
        }
        other => other,
    };

    if let Next::ParseHost = next {
        let e = strcspn(str, s, ue, b"/?#");

        // Login and password.
        if let Some(at) = memrchr(str, s, e, b'@') {
            if let Some(colon) = memchr(str, s, at, b':') {
                ret.user = component(&str[s..colon]);
                ret.pass = component(&str[colon + 1..at]);
            } else {
                ret.user = component(&str[s..at]);
            }
            s = at + 1;
        }

        // Port; an IPv6 literal ("[::1]") short-circuits the scan.
        let colon = if s < ue && str[s] == b'[' && str[e - 1] == b']' {
            None
        } else {
            memrchr(str, s, e, b':')
        };
        let host_end = match colon {
            Some(colon) => {
                if ret.port == 0 {
                    let digits = &str[colon + 1..e];
                    if digits.len() > 5 {
                        // A port cannot be longer than 5 characters.
                        return None;
                    } else if !digits.is_empty() {
                        match strtol_port(digits) {
                            Some(port @ 0..=65535) => {
                                ret.has_port = true;
                                ret.port = port as u16;
                            }
                            _ => return None,
                        }
                    }
                }
                colon
            }
            None => e,
        };

        // Without a valid host the string is not a URL.
        if host_end <= s {
            return None;
        }
        ret.host = component(&str[s..host_end]);

        if e == ue {
            return Some(ret);
        }
        s = e;
    }

    // just_path: fragment, query and path.
    let mut e = ue;
    if let Some(hash) = memchr(str, s, e, b'#') {
        ret.fragment = component(&str[hash + 1..e]);
        e = hash;
    }
    if let Some(question) = memchr(str, s, e, b'?') {
        ret.query = component(&str[question + 1..e]);
        e = question;
    }
    if s < e || s == ue {
        ret.path = component(&str[s..e]);
    }

    Some(ret)
}

fn opt_string(c: Option<Vec<u8>>) -> Value {
    c.map_or(Value::Null, |b| Value::String(PhpStr::from_vec(b)))
}

fn add_component(result: &mut PhpArray, name: &str, c: Option<Vec<u8>>) {
    if let Some(b) = c {
        result.insert_str(name, PhpStr::from_vec(b));
    }
}

/// `PHP_FUNCTION(parse_url)`.
fn parse_url(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = args.int(1);
    let Some(url) = php_url_parse(args.str(0)) else {
        return Ok(Value::Bool(false));
    };

    // Negative identifiers other than the default -1 also return the array.
    if key > -1 {
        return Ok(match key {
            PHP_URL_SCHEME => opt_string(url.scheme),
            PHP_URL_HOST => opt_string(url.host),
            PHP_URL_PORT if url.has_port => Value::Int(i64::from(url.port)),
            PHP_URL_PORT => Value::Null,
            PHP_URL_USER => opt_string(url.user),
            PHP_URL_PASS => opt_string(url.pass),
            PHP_URL_PATH => opt_string(url.path),
            PHP_URL_QUERY => opt_string(url.query),
            PHP_URL_FRAGMENT => opt_string(url.fragment),
            _ => {
                return Err(vm.argument_value_error(
                    2,
                    &format!("must be a valid URL component identifier, {key} given"),
                ));
            }
        });
    }

    let mut result = PhpArray::new();
    add_component(&mut result, "scheme", url.scheme);
    add_component(&mut result, "host", url.host);
    if url.has_port {
        result.insert_str("port", i64::from(url.port));
    }
    add_component(&mut result, "user", url.user);
    add_component(&mut result, "pass", url.pass);
    add_component(&mut result, "path", url.path);
    add_component(&mut result, "query", url.query);
    add_component(&mut result, "fragment", url.fragment);
    Ok(Value::array(result))
}

const HEXCHARS: &[u8; 16] = b"0123456789ABCDEF";

/// `php_url_encode_to_smart_str()`: appends `s` URL-encoded to `buf`.
/// Alphanumerics and `-_.` stay as they are; `raw` (RFC 3986) also keeps `~`
/// and encodes spaces as `%20`, otherwise (RFC 1738 forms) spaces become `+`.
pub fn php_url_encode_to(buf: &mut Vec<u8>, s: &[u8], raw: bool) {
    buf.reserve(s.len());
    for &c in s {
        if !raw && c == b' ' {
            buf.push(b'+');
        } else if (c < b'0' && c != b'-' && c != b'.')
            || (c < b'A' && c > b'9')
            || (c > b'Z' && c < b'a' && c != b'_')
            || (c > b'z' && (!raw || c != b'~'))
        {
            buf.extend_from_slice(&[
                b'%',
                HEXCHARS[usize::from(c >> 4)],
                HEXCHARS[usize::from(c & 15)],
            ]);
        } else {
            buf.push(c);
        }
    }
}

/// `php_url_encode()`: `urlencode()` of `s`.
pub fn php_url_encode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    php_url_encode_to(&mut out, s, false);
    out
}

/// `php_raw_url_encode()`: `rawurlencode()` of `s`.
pub fn php_raw_url_encode(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    php_url_encode_to(&mut out, s, true);
    out
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// `php_url_decode_ex()` / `php_raw_url_decode_ex()` decoding `buf` in place
/// (the output never outgrows the input); returns the decoded length. `%XY`
/// with two hex digits becomes a byte, malformed escapes are kept verbatim,
/// and `plus` turns `+` into a space.
fn decode_in_place(buf: &mut [u8], plus: bool) -> usize {
    let (mut src, mut dest) = (0, 0);
    while src < buf.len() {
        let c = buf[src];
        buf[dest] = if plus && c == b'+' {
            b' '
        } else if c == b'%' && src + 2 < buf.len() {
            match (hex_value(buf[src + 1]), hex_value(buf[src + 2])) {
                (Some(hi), Some(lo)) => {
                    src += 2;
                    hi * 16 + lo
                }
                _ => c,
            }
        } else {
            c
        };
        src += 1;
        dest += 1;
    }
    dest
}

/// `php_url_decode(str, len)`: `urldecode()` in place, truncating `buf` to the
/// decoded length (which is returned).
pub fn php_url_decode_in_place(buf: &mut Vec<u8>) -> usize {
    let len = decode_in_place(buf, true);
    buf.truncate(len);
    len
}

/// `php_raw_url_decode(str, len)`: `rawurldecode()` in place, truncating `buf`
/// to the decoded length (which is returned).
pub fn php_raw_url_decode_in_place(buf: &mut Vec<u8>) -> usize {
    let len = decode_in_place(buf, false);
    buf.truncate(len);
    len
}

/// `php_url_decode_ex()`: `urldecode()` of `s`.
pub fn php_url_decode(s: &[u8]) -> Vec<u8> {
    let mut out = s.to_vec();
    php_url_decode_in_place(&mut out);
    out
}

/// `php_raw_url_decode_ex()`: `rawurldecode()` of `s`.
pub fn php_raw_url_decode(s: &[u8]) -> Vec<u8> {
    let mut out = s.to_vec();
    php_raw_url_decode_in_place(&mut out);
    out
}

fn bytes_value(b: Vec<u8>) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(b)))
}

/// `PHP_FUNCTION(urlencode)`.
fn urlencode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bytes_value(php_url_encode(args.str(0)))
}

/// `PHP_FUNCTION(urldecode)`.
fn urldecode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bytes_value(php_url_decode(args.str(0)))
}

/// `PHP_FUNCTION(rawurlencode)`.
fn rawurlencode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bytes_value(php_raw_url_encode(args.str(0)))
}

/// `PHP_FUNCTION(rawurldecode)`.
fn rawurldecode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bytes_value(php_raw_url_decode(args.str(0)))
}

pub static PARSE_URL: NativeFunction = NativeFunction {
    sig: "parse_url(string $url, int $component = -1): int|string|array|null|false",
    handler: parse_url,
};

pub static URLENCODE: NativeFunction = NativeFunction {
    sig: "urlencode(string $string): string",
    handler: urlencode,
};

pub static URLDECODE: NativeFunction = NativeFunction {
    sig: "urldecode(string $string): string",
    handler: urldecode,
};

pub static RAWURLENCODE: NativeFunction = NativeFunction {
    sig: "rawurlencode(string $string): string",
    handler: rawurlencode,
};

pub static RAWURLDECODE: NativeFunction = NativeFunction {
    sig: "rawurldecode(string $string): string",
    handler: rawurldecode,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &PARSE_URL,
        &URLENCODE,
        &URLDECODE,
        &RAWURLENCODE,
        &RAWURLDECODE,
    ]);
    r.constant("PHP_URL_SCHEME", PHP_URL_SCHEME);
    r.constant("PHP_URL_HOST", PHP_URL_HOST);
    r.constant("PHP_URL_PORT", PHP_URL_PORT);
    r.constant("PHP_URL_USER", PHP_URL_USER);
    r.constant("PHP_URL_PASS", PHP_URL_PASS);
    r.constant("PHP_URL_PATH", PHP_URL_PATH);
    r.constant("PHP_URL_QUERY", PHP_URL_QUERY);
    r.constant("PHP_URL_FRAGMENT", PHP_URL_FRAGMENT);
    r.constant("PHP_QUERY_RFC1738", PHP_QUERY_RFC1738);
    r.constant("PHP_QUERY_RFC3986", PHP_QUERY_RFC3986);
}

#[cfg(test)]
mod tests;
