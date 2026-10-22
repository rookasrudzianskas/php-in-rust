//! ext/filter/logical_filters.c — the `FILTER_VALIDATE_*` filters.
//!
//! php-src walks NUL-terminated buffers and sometimes looks one byte past
//! the (trimmed) end; [`at`] reproduces those reads.

use php_regex::{MatchOptions, Regex};
use php_runtime::value::numeric::{MAX_LENGTH_OF_LONG, Numeric, is_numeric_string};
use php_runtime::{PResult, Value, Vm};

use super::{
    FILTER_FLAG_ALLOW_HEX, FILTER_FLAG_ALLOW_OCTAL, FILTER_FLAG_ALLOW_THOUSAND,
    FILTER_FLAG_EMAIL_UNICODE, FILTER_FLAG_GLOBAL_RANGE, FILTER_FLAG_HOSTNAME, FILTER_FLAG_IPV4,
    FILTER_FLAG_IPV6, FILTER_FLAG_NO_PRIV_RANGE, FILTER_FLAG_NO_RES_RANGE,
    FILTER_FLAG_PATH_REQUIRED, FILTER_FLAG_QUERY_REQUIRED, Outcome, compiled_regex, double_option,
    long_option, sanitize, string_option, validation_failed, value_str,
};
use crate::standard::url::php_url_parse;

/// The byte at `i`, or NUL past the end (the `zend_string` terminator).
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `PHP_FILTER_TRIM_DEFAULT_EX`: the `(start, len)` of `s` without leading
/// and trailing `" \t\r\v\n"`.
fn trim_default(s: &[u8]) -> (usize, usize) {
    let is_ws = |c: u8| matches!(c, b' ' | b'\t' | b'\r' | 0x0b | b'\n');
    let mut p = 0;
    let mut len = s.len();
    while len > 0 && is_ws(s[p]) {
        p += 1;
        len -= 1;
    }
    while len > 0 && is_ws(s[p + len - 1]) {
        len -= 1;
    }
    (p, len)
}

/// The `get_active_function_name()`-prefixed ValueError of the options checks.
fn option_error(vm: &mut Vm, msg: &str) -> php_runtime::Flow {
    let fname = vm.current_function_name();
    vm.throw_value_error(format!("{fname}(): {msg}"))
}

// ---------------------------------------------------------------------------
// Integers
// ---------------------------------------------------------------------------

/// `php_filter_parse_int()` on `s[start..end]`.
fn php_filter_parse_int(s: &[u8], start: usize, end: usize) -> Option<i64> {
    let mut i = start;
    let mut is_negative = false;
    match at(s, i) {
        b'-' => {
            is_negative = true;
            i += 1;
        }
        b'+' => i += 1,
        _ => {}
    }

    // Special cases: +0 and -0
    if at(s, i) == b'0' && i + 1 == end {
        return Some(0);
    }

    // must start with 1..9
    let c = at(s, i);
    if i >= end || !(b'1'..=b'9').contains(&c) {
        return None;
    }
    let sign = if is_negative { -1 } else { 1 };
    let mut ctx_value = sign * i64::from(c - b'0');
    i += 1;

    if end - i > MAX_LENGTH_OF_LONG - 1 {
        // overflow
        return None;
    }

    while i < end {
        let c = s[i];
        if !c.is_ascii_digit() {
            return None;
        }
        let digit = i64::from(c - b'0');
        i += 1;
        if !is_negative && ctx_value <= (i64::MAX - digit) / 10 {
            ctx_value = ctx_value * 10 + digit;
        } else if is_negative && ctx_value >= (i64::MIN + digit) / 10 {
            ctx_value = ctx_value * 10 - digit;
        } else {
            return None;
        }
    }
    Some(ctx_value)
}

/// `php_filter_parse_octal()` / `php_filter_parse_hex()`: an unsigned
/// number in `radix` that must fit in 64 bits (returned as `zend_long`).
fn parse_unsigned(digits: &[u8], radix: u32) -> Option<i64> {
    let mut ctx_value: u64 = 0;
    for &c in digits {
        let n = u64::from(char::from(c).to_digit(radix)?);
        ctx_value = ctx_value.checked_mul(u64::from(radix))?.checked_add(n)?;
    }
    Some(ctx_value as i64)
}

fn php_filter_parse_octal(digits: &[u8]) -> Option<i64> {
    parse_unsigned(digits, 8)
}

fn php_filter_parse_hex(digits: &[u8]) -> Option<i64> {
    parse_unsigned(digits, 16)
}

/// `php_filter_int()`: `FILTER_VALIDATE_INT`.
pub(super) fn php_filter_int(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let min_range = long_option(vm, options, "min_range")?;
    let max_range = long_option(vm, options, "max_range")?;

    let s = value_str(value);
    if s.is_empty() {
        return Ok(validation_failed(value, flags));
    }
    let allow_octal = flags & FILTER_FLAG_ALLOW_OCTAL != 0;
    let allow_hex = flags & FILTER_FLAG_ALLOW_HEX != 0;

    let (mut p, mut len) = trim_default(&s);
    if len == 0 {
        return Ok(validation_failed(value, flags));
    }

    let mut ctx_value = Some(0);
    if s[p] == b'0' {
        p += 1;
        len -= 1;
        if allow_hex && matches!(at(&s, p), b'x' | b'X') {
            p += 1;
            len -= 1;
            if len == 0 {
                return Ok(validation_failed(value, flags));
            }
            ctx_value = php_filter_parse_hex(&s[p..p + len]);
        } else if allow_octal {
            // Support explicit octal prefix notation
            if matches!(at(&s, p), b'o' | b'O') {
                p += 1;
                len -= 1;
                if len == 0 {
                    return Ok(validation_failed(value, flags));
                }
            }
            ctx_value = php_filter_parse_octal(&s[p..p + len]);
        } else if len != 0 {
            ctx_value = None;
        }
    } else {
        ctx_value = php_filter_parse_int(&s, p, p + len);
    }

    match ctx_value {
        Some(n)
            if !min_range.is_some_and(|min| n < min) && !max_range.is_some_and(|max| n > max) =>
        {
            *value = Value::Int(n);
            Ok(Outcome::Success)
        }
        _ => Ok(validation_failed(value, flags)),
    }
}

// ---------------------------------------------------------------------------
// Booleans and floats
// ---------------------------------------------------------------------------

/// `php_filter_boolean()`: `FILTER_VALIDATE_BOOL`. True for "1", "true",
/// "on" and "yes"; false for "0", "false", "off", "no" and ""; a failure
/// otherwise.
pub(super) fn php_filter_boolean(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    let (p, len) = trim_default(&s);
    let t = &s[p..p + len];
    let is = |word: &[u8]| t.eq_ignore_ascii_case(word);

    let ret = match len {
        0 => Some(false),
        1 if t == b"1" => Some(true),
        1 if t == b"0" => Some(false),
        2 if is(b"on") => Some(true),
        2 if is(b"no") => Some(false),
        3 if is(b"yes") => Some(true),
        3 if is(b"off") => Some(false),
        4 if is(b"true") => Some(true),
        5 if is(b"false") => Some(false),
        _ => None,
    };
    match ret {
        Some(b) => {
            *value = Value::Bool(b);
            Ok(Outcome::Success)
        }
        None => Ok(validation_failed(value, flags)),
    }
}

/// `strchr(set, c)` on a C string: also true for `c == '\0'`.
fn c_strchr(set: &[u8], c: u8) -> bool {
    c == 0 || php_runtime::value::numeric::c_str(set).contains(&c)
}

/// `php_filter_float()`: `FILTER_VALIDATE_FLOAT`. The value is copied into
/// a plain number (sign, digits, `.`, exponent) while checking the decimal
/// and thousand separators, then parsed with `is_numeric_string()`.
pub(super) fn php_filter_float(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    let (start, len) = trim_default(&s);
    if len == 0 {
        return Ok(validation_failed(value, flags));
    }
    let end = start + len;

    let mut dec_sep = b'.';
    if let Some(decimal) = string_option(options, "decimal") {
        if decimal.len() != 1 {
            return Err(option_error(
                vm,
                "\"decimal\" option must be one character long",
            ));
        }
        dec_sep = decimal[0];
    }

    let tsd_sep = match string_option(options, "thousand") {
        Some(thousand) => {
            if thousand.is_empty() {
                return Err(option_error(vm, "\"thousand\" option must not be empty"));
            }
            thousand.to_vec()
        }
        None => b"',.".to_vec(),
    };

    let min_range = double_option(vm, options, "min_range")?;
    let max_range = double_option(vm, options, "max_range")?;

    let mut num = Vec::with_capacity(len + 1);
    let mut i = start;
    if i < end && matches!(s[i], b'+' | b'-') {
        num.push(s[i]);
        i += 1;
    }
    let mut first = true;
    let well_formed = loop {
        let mut n = 0;
        while i < end && s[i].is_ascii_digit() {
            n += 1;
            num.push(s[i]);
            i += 1;
        }
        if i == end || at(&s, i) == dec_sep || matches!(at(&s, i), b'e' | b'E') {
            if !first && n != 3 {
                break false;
            }
            if at(&s, i) == dec_sep {
                num.push(b'.');
                i += 1;
                while i < end && s[i].is_ascii_digit() {
                    num.push(s[i]);
                    i += 1;
                }
            }
            if matches!(at(&s, i), b'e' | b'E') {
                num.push(at(&s, i));
                i += 1;
                if i < end && matches!(s[i], b'+' | b'-') {
                    num.push(s[i]);
                    i += 1;
                }
                while i < end && s[i].is_ascii_digit() {
                    num.push(s[i]);
                    i += 1;
                }
            }
            break true;
        }
        if flags & FILTER_FLAG_ALLOW_THOUSAND != 0 && c_strchr(&tsd_sep, at(&s, i)) {
            if if first { !(1..=3).contains(&n) } else { n != 3 } {
                break false;
            }
            first = false;
            i += 1;
        } else {
            break false;
        }
    };
    if !well_formed || i != end {
        return Ok(validation_failed(value, flags));
    }

    let out_of_range =
        |d: f64| min_range.is_some_and(|min| d < min) || max_range.is_some_and(|max| d > max);
    let result = match is_numeric_string(&num, false) {
        Some(Numeric::Int(l)) => Some(l as f64).filter(|&d| !out_of_range(d)),
        Some(Numeric::Float(d)) => {
            // An underflow to zero (any non-zero digit) or an overflow.
            let underflow =
                d == 0.0 && num.len() > 1 && num.iter().any(|c| (b'1'..=b'9').contains(c));
            Some(d).filter(|&d| !underflow && d.is_finite() && !out_of_range(d))
        }
        None => None,
    };
    match result {
        Some(d) => {
            *value = Value::Float(d);
            Ok(Outcome::Success)
        }
        None => Ok(validation_failed(value, flags)),
    }
}

// ---------------------------------------------------------------------------
// Regular expressions and e-mail addresses
// ---------------------------------------------------------------------------

/// `php_pcre_mctx()`: the match limits from the pcre ini settings.
fn match_options(vm: &Vm) -> MatchOptions {
    let mut o = MatchOptions::default();
    if vm.ini_get("pcre.backtrack_limit").is_some() {
        o.match_limit = vm.ini_int("pcre.backtrack_limit") as u32;
    }
    if vm.ini_get("pcre.recursion_limit").is_some() {
        o.depth_limit = vm.ini_int("pcre.recursion_limit") as u32;
    }
    o
}

/// `pcre2_match(re, subject, len, 0, 0, ...) >= 0`: matching errors (bad
/// UTF-8, limits) count as no match.
fn regex_matches(vm: &Vm, re: &Regex, subject: &[u8]) -> bool {
    matches!(re.match_at(subject, 0, match_options(vm)), Ok(Some(_)))
}

/// `php_filter_validate_regexp()`: `FILTER_VALIDATE_REGEXP`.
pub(super) fn php_filter_validate_regexp(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let Some(regexp) = string_option(options, "regexp") else {
        return Err(option_error(vm, "\"regexp\" option is missing"));
    };
    let Some(re) = compiled_regex(vm, &regexp)? else {
        return Ok(validation_failed(value, flags));
    };
    if !regex_matches(vm, &re, &value_str(value)) {
        return Ok(validation_failed(value, flags));
    }
    Ok(Outcome::Success)
}

/// The `FILTER_FLAG_EMAIL_UNICODE` pattern of `php_filter_validate_email()`
/// (based on Michael Rushton's regex, restricted to routeable addresses).
const EMAIL_REGEXP_UNICODE: &str = r#"/^(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){255,})(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){65,}@)(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E\pL\pN]+)|(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F\pL\pN]|(?:\x5C[\x00-\x7F]))*\x22))(?:\.(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E\pL\pN]+)|(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F\pL\pN]|(?:\x5C[\x00-\x7F]))*\x22)))*@(?:(?:(?!.*[^.]{64,})(?:(?:(?:xn--)?[a-z0-9]+(?:-+[a-z0-9]+)*\.){1,126}){1,}(?:(?:[a-z][a-z0-9]*)|(?:(?:xn--)[a-z0-9]+))(?:-+[a-z0-9]+)*)|(?:\[(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){7})|(?:(?!(?:.*[a-f0-9][:\]]){7,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?::(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?)))|(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){5}:)|(?:(?!(?:.*[a-f0-9]:){5,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3})?::(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3}:)?)))?(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))(?:\.(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))){3}))\]))$/iDu"#;

/// The ASCII pattern of `php_filter_validate_email()`.
const EMAIL_REGEXP: &str = r#"/^(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){255,})(?!(?:(?:\x22?\x5C[\x00-\x7E]\x22?)|(?:\x22?[^\x5C\x22]\x22?)){65,}@)(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E]+)|(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F]|(?:\x5C[\x00-\x7F]))*\x22))(?:\.(?:(?:[\x21\x23-\x27\x2A\x2B\x2D\x2F-\x39\x3D\x3F\x5E-\x7E]+)|(?:\x22(?:[\x01-\x08\x0B\x0C\x0E-\x1F\x21\x23-\x5B\x5D-\x7F]|(?:\x5C[\x00-\x7F]))*\x22)))*@(?:(?:(?!.*[^.]{64,})(?:(?:(?:xn--)?[a-z0-9]+(?:-+[a-z0-9]+)*\.){1,126}){1,}(?:(?:[a-z][a-z0-9]*)|(?:(?:xn--)[a-z0-9]+))(?:-+[a-z0-9]+)*)|(?:\[(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){7})|(?:(?!(?:.*[a-f0-9][:\]]){7,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?::(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,5})?)))|(?:(?:IPv6:(?:(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){5}:)|(?:(?!(?:.*[a-f0-9]:){5,})(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3})?::(?:[a-f0-9]{1,4}(?::[a-f0-9]{1,4}){0,3}:)?)))?(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))(?:\.(?:(?:25[0-5])|(?:2[0-4][0-9])|(?:1[0-9]{2})|(?:[1-9]?[0-9]))){3}))\]))$/iD"#;

/// `php_filter_validate_email()`: `FILTER_VALIDATE_EMAIL`.
pub(super) fn php_filter_validate_email(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    // The maximum length of an e-mail address is 320 octets, per RFC 2821.
    if s.len() > 320 {
        return Ok(validation_failed(value, flags));
    }
    let pattern = if flags & FILTER_FLAG_EMAIL_UNICODE != 0 {
        EMAIL_REGEXP_UNICODE
    } else {
        EMAIL_REGEXP
    };
    let Some(re) = compiled_regex(vm, pattern.as_bytes())? else {
        return Ok(validation_failed(value, flags));
    };
    if !regex_matches(vm, &re, &s) {
        return Ok(validation_failed(value, flags));
    }
    Ok(Outcome::Success)
}

// ---------------------------------------------------------------------------
// Domains and URLs
// ---------------------------------------------------------------------------

/// `php_filter_validate_domain_ex()`.
fn php_filter_validate_domain_ex(domain: &[u8], flags: i64) -> bool {
    let hostname = flags & FILTER_FLAG_HOSTNAME != 0;
    let alnum = |c: u8| c.is_ascii_alphanumeric();
    let mut l = domain.len();
    let mut e = l;

    // Ignore trailing dot
    if l > 0 && domain[l - 1] == b'.' {
        e = l - 1;
        l -= 1;
    }

    // The total length cannot exceed 253 characters (final dot not included)
    if l > 253 {
        return false;
    }

    // First char must be alphanumeric
    if at(domain, 0) == b'.' || (hostname && !alnum(at(domain, 0))) {
        return false;
    }

    // `unsigned char` label length counter
    let mut i: u8 = 1;
    for s in 0..e {
        let c = domain[s];
        if c == b'.' {
            // The first and the last character of a label must be alphanumeric
            if at(domain, s + 1) == b'.'
                || (hostname && (!alnum(domain[s - 1]) || !alnum(at(domain, s + 1))))
            {
                return false;
            }
            // Reset label length counter
            i = 1;
        } else {
            if i > 63 || (hostname && (c != b'-' || at(domain, s + 1) == 0) && !alnum(c)) {
                return false;
            }
            i += 1;
        }
    }
    true
}

/// `php_filter_validate_domain()`: `FILTER_VALIDATE_DOMAIN`.
pub(super) fn php_filter_validate_domain(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    if !php_filter_validate_domain_ex(&value_str(value), flags) {
        return Ok(validation_failed(value, flags));
    }
    Ok(Outcome::Success)
}

/// `is_userinfo_valid()`: RFC 3986 userinfo characters or `%XX` escapes.
fn is_userinfo_valid(s: &[u8]) -> bool {
    const VALID: &[u8] = b"-._~!$&'()*+,;=:";
    let mut p = 0;
    while p < s.len() {
        let c = s[p];
        // `strchr(valid, c)` also matches the terminating NUL.
        if c.is_ascii_alphanumeric() || c == 0 || VALID.contains(&c) {
            p += 1;
        } else if c == b'%'
            && (s.len() < 3 || p <= s.len() - 3)
            && at(s, p + 1).is_ascii_digit()
            && at(s, p + 2).is_ascii_hexdigit()
        {
            p += 3;
        } else {
            return false;
        }
    }
    true
}

/// `php_filter_is_valid_ipv6_hostname()`: an IPv6 address in brackets.
fn php_filter_is_valid_ipv6_hostname(host: &[u8]) -> bool {
    host.len() >= 2
        && host[0] == b'['
        && host[host.len() - 1] == b']'
        && php_filter_validate_ipv6(&host[1..host.len() - 1], None)
}

/// `php_filter_validate_url()`: `FILTER_VALIDATE_URL`, with the
/// `parse_url()` parser (`php_uri_get_parser(NULL)`).
pub(super) fn php_filter_validate_url(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let old_len = value_str(value).len();

    sanitize::php_filter_url(vm, value, flags, options)?;

    if !matches!(value, Value::String(s) if s.len() == old_len) {
        return Ok(validation_failed(value, flags));
    }

    // Only the parse_url() parser is available (ext/uri is not ported).
    if let Some(parser_name) = string_option(options, "uri_parser_class")
        && parser_name != "parse_url"
    {
        return Err(option_error(
            vm,
            "\"uri_parser_class\" option has invalid value",
        ));
    }

    // Parse the URI - if it fails, we return NULL
    let Some(url) = php_url_parse(&value_str(value)) else {
        return Ok(validation_failed(value, flags));
    };

    if let Some(scheme) = &url.scheme
        && (scheme.eq_ignore_ascii_case(b"http") || scheme.eq_ignore_ascii_case(b"https"))
    {
        let Some(host) = &url.host else {
            return Ok(validation_failed(value, flags));
        };
        // An IPv6 enclosed by square brackets is a valid hostname; the
        // domain check includes a loose check for an IPv4 address.
        if !php_filter_is_valid_ipv6_hostname(host)
            && !php_filter_validate_domain_ex(host, FILTER_FLAG_HOSTNAME)
        {
            return Ok(validation_failed(value, flags));
        }
    }

    let Some(scheme) = &url.scheme else {
        return Ok(validation_failed(value, flags));
    };
    // some schemes allow the host to be empty
    if (url.host.is_none() && !matches!(scheme.as_slice(), b"mailto" | b"news" | b"file"))
        || (flags & FILTER_FLAG_PATH_REQUIRED != 0 && url.path.is_none())
        || (flags & FILTER_FLAG_QUERY_REQUIRED != 0 && url.query.is_none())
    {
        return Ok(validation_failed(value, flags));
    }

    if url.user.as_deref().is_some_and(|u| !is_userinfo_valid(u))
        || url.pass.as_deref().is_some_and(|p| !is_userinfo_valid(p))
    {
        return Ok(validation_failed(value, flags));
    }
    Ok(Outcome::Success)
}

// ---------------------------------------------------------------------------
// IP addresses
// ---------------------------------------------------------------------------

/// `_php_filter_validate_ipv4()`: dotted quad without leading zeros; the
/// parts go to `ip[0..4]`.
fn php_filter_validate_ipv4(s: &[u8], ip: &mut [i32; 8]) -> bool {
    let end = s.len();
    let mut i = 0;
    let mut n = 0;
    while i < end {
        if !s[i].is_ascii_digit() {
            return false;
        }
        let leading_zero = s[i] == b'0';
        let mut m = 1;
        let mut num = i32::from(s[i] - b'0');
        i += 1;
        while i < end && s[i].is_ascii_digit() {
            num = num * 10 + i32::from(s[i] - b'0');
            i += 1;
            m += 1;
            if num > 255 || m > 3 {
                return false;
            }
        }
        // don't allow a leading 0; that introduces octal numbers, which we
        // don't support
        if leading_zero && (num != 0 || m > 1) {
            return false;
        }
        ip[n] = num;
        n += 1;
        if n == 4 {
            return i == end;
        }
        if i >= end || s[i] != b'.' {
            return false;
        }
        i += 1;
    }
    false
}

/// `_php_filter_validate_ipv6()`; with `ip`, also the eight 16-bit groups
/// (an embedded IPv4 address always yields `::ffff:a.b.c.d`).
fn php_filter_validate_ipv6(s: &[u8], mut ip: Option<&mut [i32; 8]>) -> bool {
    if !s.contains(&b':') {
        return false;
    }

    let mut str_len = s.len();
    let mut blocks: i32 = 0;
    let mut ip4elm = [0i32; 8];
    let mut has_ipv4 = false;

    // check for bundled IPv4
    if let Some(dot) = s.iter().position(|&c| c == b'.') {
        let mut ipv4 = dot;
        while ipv4 > 0 && s[ipv4 - 1] != b':' {
            ipv4 -= 1;
        }
        if !php_filter_validate_ipv4(&s[ipv4..], &mut ip4elm) {
            return false;
        }
        // length excluding ipv4
        str_len = ipv4;
        if str_len < 2 {
            return false;
        }
        if s[ipv4 - 2] != b':' {
            // don't include : before ipv4 unless it's a ::
            str_len -= 1;
        }
        blocks = 2;
        has_ipv4 = true;
    }

    let end = str_len;
    let mut i = 0;
    let mut compressed_pos: i32 = -1;
    while i < end {
        if s[i] == b':' {
            i += 1;
            if i >= end {
                // cannot end in : without previous :
                return false;
            }
            if s[i] == b':' {
                if compressed_pos >= 0 {
                    return false;
                }
                if let Some(ip) = ip.as_deref_mut()
                    && blocks < 8
                {
                    ip[blocks as usize] = -1;
                }
                // :: means 1 or more 16-bit 0 blocks
                compressed_pos = blocks;
                blocks += 1;
                i += 1;
                if i == end {
                    if blocks > 8 {
                        return false;
                    }
                    break;
                }
            } else if i - 1 == 0 {
                // don't allow leading : without another : following
                return false;
            }
        }
        let mut num: u32 = 0;
        let mut n = 0;
        while i < end {
            let Some(d) = char::from(s[i]).to_digit(16) else {
                break;
            };
            num = num.wrapping_mul(16).wrapping_add(d);
            n += 1;
            i += 1;
        }
        if let Some(ip) = ip.as_deref_mut()
            && blocks < 8
        {
            ip[blocks as usize] = num as i32;
        }
        if !(1..=4).contains(&n) {
            return false;
        }
        blocks += 1;
        if blocks > 8 {
            return false;
        }
    }

    // fixup_ip:
    if let Some(ip) = ip {
        if has_ipv4 {
            ip[..5].fill(0);
            ip[5] = 0xffff;
            ip[6] = 256 * ip4elm[0] + ip4elm[1];
            ip[7] = 256 * ip4elm[2] + ip4elm[3];
        } else if compressed_pos >= 0 && blocks <= 8 {
            let offset = 8 - blocks;
            let mut k = 7;
            while k > compressed_pos + offset {
                ip[k as usize] = ip[(k - offset) as usize];
                k -= 1;
            }
            let mut k = compressed_pos + offset;
            while k >= compressed_pos {
                ip[k as usize] = 0;
                k -= 1;
            }
        }
    }

    (compressed_pos >= 0 && blocks <= 8) || blocks == 8
}

/// The RFC 6890 range of an address: `(global, reserved, private)`, or
/// `None` outside the special-purpose blocks.
type RangeFlags = Option<(bool, bool, bool)>;

/// `ipv4_get_status_flags()` (RFC 6890 including errata).
#[allow(clippy::if_same_then_else)]
fn ipv4_get_status_flags(ip: &[i32; 8]) -> RangeFlags {
    const NONE: (bool, bool, bool) = (false, false, false);
    const GLOBAL: (bool, bool, bool) = (true, false, false);
    const RESERVED: (bool, bool, bool) = (false, true, false);
    const PRIVATE: (bool, bool, bool) = (false, false, true);
    Some(if ip[0] == 0 {
        // RFC 0791 - This network
        RESERVED
    } else if ip[0] == 10 {
        // RFC 1918 - Private Use
        PRIVATE
    } else if ip[0] == 100 && ip[1] >= 64 && ip[1] <= 127 {
        // RFC 6598 - Shared Address Space
        NONE
    } else if ip[0] == 127 {
        // RFC 1122 - Loopback
        RESERVED
    } else if ip[0] == 169 && ip[1] == 254 {
        // RFC 3927 - Link Local
        RESERVED
    } else if ip[0] == 172 && ip[1] >= 16 && ip[1] <= 31 {
        // RFC 1918 - Private Use
        PRIVATE
    } else if ip[0] == 192 && ip[1] == 0 && ip[2] == 0 {
        // RFC 6890 - IETF Protocol Assignments (includes RFC 6333 DS-Lite)
        NONE
    } else if ip[0] == 192 && ip[1] == 0 && ip[2] == 2 {
        // RFC 5737 - Documentation
        NONE
    } else if ip[0] == 192 && ip[1] == 88 && ip[2] == 99 {
        // RFC 3068 - 6to4 Relay Anycast
        GLOBAL
    } else if ip[0] == 192 && ip[1] == 168 {
        // RFC 1918 - Private Use
        PRIVATE
    } else if ip[0] == 198 && ip[1] >= 18 && ip[1] <= 19 {
        // RFC 2544 - Benchmarking
        NONE
    } else if ip[0] == 198 && ip[1] == 51 && ip[2] == 100 {
        // RFC 5737 - Documentation
        NONE
    } else if ip[0] == 203 && ip[1] == 0 && ip[2] == 113 {
        // RFC 5737 - Documentation
        NONE
    } else if ip[0] >= 240 && ip[1] <= 255 {
        // RFC 1122 - Reserved (and RFC 0919 limited broadcast)
        RESERVED
    } else {
        return None;
    })
}

/// `ipv6_get_status_flags()` (RFC 6890).
#[allow(clippy::if_same_then_else)]
fn ipv6_get_status_flags(ip: &[i32; 8]) -> RangeFlags {
    const NONE: (bool, bool, bool) = (false, false, false);
    const GLOBAL: (bool, bool, bool) = (true, false, false);
    const RESERVED: (bool, bool, bool) = (false, true, false);
    const PRIVATE: (bool, bool, bool) = (false, false, true);
    let zero_prefix = |n: usize| ip[..n].iter().all(|&g| g == 0);
    Some(if zero_prefix(8) {
        // RFC 4291 - Unspecified Address
        RESERVED
    } else if zero_prefix(7) && ip[7] == 1 {
        // RFC 4291 - Loopback Address
        RESERVED
    } else if ip[0] == 0x0064 && ip[1] == 0xff9b {
        // RFC 6052 - IPv4-IPv6 Translation
        GLOBAL
    } else if zero_prefix(5) && ip[5] == 0xffff {
        // RFC 4291 - IPv4-mapped Address
        RESERVED
    } else if ip[0] == 0x0100 && ip[1] == 0 && ip[2] == 0 && ip[3] == 0 {
        // RFC 6666 - Discard-Only Address Block
        NONE
    } else if ip[0] == 0x2001 && ip[1] <= 0x01ff {
        // RFC 4380 - TEREDO, RFC 2928 - IETF Protocol Assignments
        NONE
    } else if ip[0] == 0x2001 && ip[1] == 0x0db8 {
        // RFC 3849 - Documentation
        NONE
    } else if ip[0] == 0x2002 {
        // RFC 3056 - 6to4
        NONE
    } else if ip[0] >= 0xfc00 && ip[0] <= 0xfdff {
        // RFC 4193 - Unique-Local
        PRIVATE
    } else if ip[0] >= 0xfe80 && ip[0] <= 0xfebf {
        // RFC 4291 - Linked-Scoped Unicast
        RESERVED
    } else {
        return None;
    })
}

/// `php_filter_validate_ip()`: `FILTER_VALIDATE_IP`. The first `:` or `.`
/// decides the format; the range flags reject special-purpose blocks.
pub(super) fn php_filter_validate_ip(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    let ipv6 = if s.contains(&b':') {
        true
    } else if s.contains(&b'.') {
        false
    } else {
        return Ok(validation_failed(value, flags));
    };

    let want_v4 = flags & FILTER_FLAG_IPV4 != 0;
    let want_v6 = flags & FILTER_FLAG_IPV6 != 0;
    if !(want_v4 && want_v6) && ((want_v4 && ipv6) || (want_v6 && !ipv6)) {
        return Ok(validation_failed(value, flags));
    }

    let mut ip = [0i32; 8];
    let status = if ipv6 {
        if !php_filter_validate_ipv6(&s, Some(&mut ip)) {
            return Ok(validation_failed(value, flags));
        }
        ipv6_get_status_flags(&ip)
    } else {
        if !php_filter_validate_ipv4(&s, &mut ip) {
            return Ok(validation_failed(value, flags));
        }
        ipv4_get_status_flags(&ip)
    };
    // no special block
    let Some((global, reserved, private)) = status else {
        return Ok(Outcome::Success);
    };

    if (flags & FILTER_FLAG_GLOBAL_RANGE != 0 && !global)
        || (flags & FILTER_FLAG_NO_PRIV_RANGE != 0 && private)
        || (flags & FILTER_FLAG_NO_RES_RANGE != 0 && reserved)
    {
        return Ok(validation_failed(value, flags));
    }
    Ok(Outcome::Success)
}

/// `php_filter_validate_mac()`: `FILTER_VALIDATE_MAC` (`01-23-45-67-89-ab`,
/// `01:23:45:67:89:ab` or `0123.4567.89ab`).
pub(super) fn php_filter_validate_mac(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let input = value_str(value);
    let exp_separator = string_option(options, "separator");
    if exp_separator.as_ref().is_some_and(|sep| sep.len() != 1) {
        return Err(option_error(
            vm,
            "\"separator\" option must be one character long",
        ));
    }

    let (tokens, length, separator) = match (input.len(), at(&input, 2)) {
        // EUI-64 format: Four hexadecimal digits separated by dots.
        (14, _) => (3, 4, b'.'),
        // IEEE 802 format: Six hexadecimal digits separated by hyphens or colons.
        (17, b'-') => (6, 2, b'-'),
        (17, b':') => (6, 2, b':'),
        _ => return Ok(validation_failed(value, flags)),
    };

    if exp_separator.is_some_and(|sep| sep[0] != separator) {
        return Ok(validation_failed(value, flags));
    }

    for i in 0..tokens {
        let offset = i * (length + 1);
        if i < tokens - 1 && input[offset + length] != separator {
            // The current token did not end with e.g. a "."
            return Ok(validation_failed(value, flags));
        }
        if php_filter_parse_hex(&input[offset..offset + length]).is_none() {
            // The current token is no valid hexadecimal digit
            return Ok(validation_failed(value, flags));
        }
    }
    Ok(Outcome::Success)
}
