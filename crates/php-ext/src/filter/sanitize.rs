//! ext/filter/sanitizing_filters.c — the `FILTER_SANITIZE_*` filters and
//! `FILTER_UNSAFE_RAW`.

use php_runtime::{PResult, PhpStr, Value, Vm};

use super::{
    FILTER_FLAG_ALLOW_FRACTION, FILTER_FLAG_ALLOW_SCIENTIFIC, FILTER_FLAG_ALLOW_THOUSAND,
    FILTER_FLAG_EMPTY_STRING_NULL, FILTER_FLAG_ENCODE_AMP, FILTER_FLAG_ENCODE_HIGH,
    FILTER_FLAG_ENCODE_LOW, FILTER_FLAG_NO_ENCODE_QUOTES, FILTER_FLAG_STRIP_BACKTICK,
    FILTER_FLAG_STRIP_HIGH, FILTER_FLAG_STRIP_LOW, Outcome, value_str,
};
use crate::standard::html::{ENT_NOQUOTES, ENT_QUOTES, escape_html_entities};
use crate::standard::string::php_addslashes;

/// A set of bytes (`filter_map` / the `chars` tables of the encoders).
type ByteSet = [bool; 256];

fn byte_set(chars: &[u8]) -> ByteSet {
    let mut set = [false; 256];
    for &c in chars {
        set[usize::from(c)] = true;
    }
    set
}

const LOWALPHA: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const HIALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGIT: &[u8] = b"0123456789";

/// `php_filter_encode_html()`: replaces the bytes in `chars` by decimal
/// numeric entities (`&#60;`).
fn encode_html(s: &[u8], chars: &ByteSet) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        if chars[usize::from(c)] {
            out.extend_from_slice(format!("&#{c};").as_bytes());
        } else {
            out.push(c);
        }
    }
    out
}

/// `php_filter_encode_url()`: percent-encodes every byte not in `chars`.
fn encode_url(s: &[u8], chars: &[u8]) -> Vec<u8> {
    const HEXCHARS: &[u8; 16] = b"0123456789ABCDEF";
    let keep = byte_set(chars);
    let mut out = Vec::with_capacity(s.len() * 3);
    for &c in s {
        if keep[usize::from(c)] {
            out.push(c);
        } else {
            out.extend_from_slice(&[
                b'%',
                HEXCHARS[usize::from(c >> 4)],
                HEXCHARS[usize::from(c & 15)],
            ]);
        }
    }
    out
}

/// `php_filter_strip()`: drops the bytes selected by the `STRIP_*` flags.
fn php_filter_strip(s: &PhpStr, flags: i64) -> PhpStr {
    if flags & (FILTER_FLAG_STRIP_LOW | FILTER_FLAG_STRIP_HIGH | FILTER_FLAG_STRIP_BACKTICK) == 0 {
        return s.clone();
    }
    let kept: Vec<u8> = s
        .iter()
        .copied()
        .filter(|&c| {
            !((c >= 127 && flags & FILTER_FLAG_STRIP_HIGH != 0)
                || (c < 32 && flags & FILTER_FLAG_STRIP_LOW != 0)
                || (c == b'`' && flags & FILTER_FLAG_STRIP_BACKTICK != 0))
        })
        .collect();
    PhpStr::from_vec(kept)
}

/// `filter_map_apply()`: keeps only the bytes in `map`.
fn filter_map_apply(value: &mut Value, map: &ByteSet) {
    let s = value_str(value);
    let kept: Vec<u8> = s.iter().copied().filter(|&c| map[usize::from(c)]).collect();
    *value = Value::String(PhpStr::from_vec(kept));
}

/// The `ENCODE_*` part shared by `php_filter_string()` and
/// `php_filter_unsafe_raw()`.
fn encode_flags(enc: &mut ByteSet, flags: i64) {
    if flags & FILTER_FLAG_ENCODE_AMP != 0 {
        enc[usize::from(b'&')] = true;
    }
    if flags & FILTER_FLAG_ENCODE_LOW != 0 {
        enc[..32].fill(true);
    }
    if flags & FILTER_FLAG_ENCODE_HIGH != 0 {
        enc[127..].fill(true);
    }
}

/// `php_strip_tags_ex(buf, len, NULL, 0, /* allow_tag_spaces */ true)`:
/// strips HTML and PHP tags and comments (and NUL bytes). Ported here because
/// ext/standard keeps its version private.
fn strip_tags(input: &[u8]) -> Vec<u8> {
    let prev = |p: usize, n: usize| if p >= n { input[p - n] } else { 0 };
    let prev_ci = |p: usize, n: usize| prev(p, n).to_ascii_lowercase();

    let mut out = Vec::with_capacity(input.len());
    let mut state = 0u8;
    let mut lc = 0u8;
    let mut br: i64 = 0;
    let mut depth: i64 = 0;
    let mut in_q = 0u8;
    let mut is_xml = false;

    let toggle = |in_q: &mut u8, c: u8| *in_q = if *in_q != 0 { 0 } else { c };

    for (p, &c) in input.iter().enumerate() {
        match state {
            // Text.
            0 => match c {
                0 => {}
                b'<' => {
                    if in_q == 0 {
                        lc = b'<';
                        state = 1;
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        out.push(c);
                    }
                }
                _ => out.push(c),
            },
            // Inside an HTML tag.
            1 => match c {
                b'<' => {
                    if in_q == 0 {
                        depth += 1;
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        lc = b'>';
                        if !(is_xml && prev(p, 1) == b'-') {
                            in_q = 0;
                            state = 0;
                            is_xml = false;
                        }
                    }
                }
                b'"' | b'\'' => {
                    if p != 0 && (in_q == 0 || c == in_q) {
                        toggle(&mut in_q, c);
                    }
                }
                b'!' if prev(p, 1) == b'<' => {
                    state = 3;
                    lc = c;
                }
                b'?' if prev(p, 1) == b'<' => {
                    br = 0;
                    state = 2;
                }
                _ => {}
            },
            // Inside `<? ... ?>`.
            2 => match c {
                b'(' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b'(';
                        br += 1;
                    }
                }
                b')' => {
                    if lc != b'"' && lc != b'\'' {
                        lc = b')';
                        br -= 1;
                    }
                }
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 && br == 0 && lc != b'"' && prev(p, 1) == b'?' {
                        in_q = 0;
                        state = 0;
                    }
                }
                b'"' | b'\'' => {
                    if p >= 1 && prev(p, 1) != b'\\' {
                        if lc == c {
                            lc = 0;
                        } else if lc != b'\\' {
                            lc = c;
                        }
                        if in_q == 0 || c == in_q {
                            toggle(&mut in_q, c);
                        }
                    }
                }
                // `<?xml` is not PHP: back to HTML.
                b'l' | b'L'
                    if p > 4
                        && prev_ci(p, 1) == b'm'
                        && prev_ci(p, 2) == b'x'
                        && prev(p, 3) == b'?'
                        && prev(p, 4) == b'<' =>
                {
                    state = 1;
                    is_xml = true;
                }
                _ => {}
            },
            // Inside `<! ... >`.
            3 => match c {
                b'>' => {
                    if depth != 0 {
                        depth -= 1;
                    } else if in_q == 0 {
                        state = 0;
                    }
                }
                b'"' | b'\'' => {
                    if p != 0 && prev(p, 1) != b'\\' && (in_q == 0 || c == in_q) {
                        toggle(&mut in_q, c);
                    }
                }
                b'-' if prev(p, 1) == b'-' && prev(p, 2) == b'!' => state = 4,
                // `<!DOCTYPE` is handled like a normal tag.
                b'E' | b'e'
                    if p > 6
                        && prev_ci(p, 1) == b'p'
                        && prev_ci(p, 2) == b'y'
                        && prev_ci(p, 3) == b't'
                        && prev_ci(p, 4) == b'c'
                        && prev_ci(p, 5) == b'o'
                        && prev_ci(p, 6) == b'd' =>
                {
                    state = 1;
                }
                _ => {}
            },
            // Inside `<!-- ... -->`.
            _ => {
                if c == b'>' && in_q == 0 && prev(p, 1) == b'-' && prev(p, 2) == b'-' {
                    in_q = 0;
                    state = 0;
                }
            }
        }
    }
    out
}

/// `php_filter_string()`: `FILTER_SANITIZE_STRING` / `FILTER_SANITIZE_STRIPPED`.
pub(super) fn php_filter_string(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    // strip high/strip low (see flags)
    let s = php_filter_strip(&value_str(value), flags);

    let mut enc = [false; 256];
    if flags & FILTER_FLAG_NO_ENCODE_QUOTES == 0 {
        enc[usize::from(b'\'')] = true;
        enc[usize::from(b'"')] = true;
    }
    encode_flags(&mut enc, flags);
    let encoded = encode_html(&s, &enc);

    // strip tags, implicitly also removes \0 chars
    let stripped = strip_tags(&encoded);
    *value = if !stripped.is_empty() {
        Value::String(PhpStr::from_vec(stripped))
    } else if flags & FILTER_FLAG_EMPTY_STRING_NULL != 0 {
        Value::Null
    } else {
        Value::empty_string()
    };
    Ok(Outcome::Success)
}

/// `php_filter_encoded()`: `FILTER_SANITIZE_ENCODED` (urlencode).
pub(super) fn php_filter_encoded(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    const DEFAULT_URL_ENCODE: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-._";
    let s = php_filter_strip(&value_str(value), flags);
    *value = Value::String(PhpStr::from_vec(encode_url(&s, DEFAULT_URL_ENCODE)));
    Ok(Outcome::Success)
}

/// `php_filter_special_chars()`: `FILTER_SANITIZE_SPECIAL_CHARS`.
pub(super) fn php_filter_special_chars(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = php_filter_strip(&value_str(value), flags);

    // encodes ' " < > & \0 to numerical entities
    let mut enc = byte_set(b"'\"<>&\0");
    // if strip low is not set, then we encode them as &#xx;
    enc[..32].fill(true);
    if flags & FILTER_FLAG_ENCODE_HIGH != 0 {
        enc[127..].fill(true);
    }
    *value = Value::String(PhpStr::from_vec(encode_html(&s, &enc)));
    Ok(Outcome::Success)
}

/// `php_filter_full_special_chars()`: `FILTER_SANITIZE_FULL_SPECIAL_CHARS`
/// (`htmlentities()` without double encoding).
pub(super) fn php_filter_full_special_chars(
    vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let quotes = if flags & FILTER_FLAG_NO_ENCODE_QUOTES == 0 {
        ENT_QUOTES
    } else {
        ENT_NOQUOTES
    };
    let s = value_str(value);
    let escaped = escape_html_entities(vm, &s, true, quotes, None, false, false)?.into_owned();
    *value = Value::String(PhpStr::from_vec(escaped));
    Ok(Outcome::Success)
}

/// `php_filter_unsafe_raw()`: `FILTER_UNSAFE_RAW` / `FILTER_DEFAULT`.
pub(super) fn php_filter_unsafe_raw(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    // Only if no flags are set (optimization)
    if flags != 0 && !s.is_empty() {
        let s = php_filter_strip(&s, flags);
        let mut enc = [false; 256];
        encode_flags(&mut enc, flags);
        *value = Value::String(PhpStr::from_vec(encode_html(&s, &enc)));
    } else if flags & FILTER_FLAG_EMPTY_STRING_NULL != 0 && s.is_empty() {
        *value = Value::Null;
    }
    Ok(Outcome::Success)
}

/// `php_filter_email()`: `FILTER_SANITIZE_EMAIL` keeps the characters of
/// RFC 822 section 6.
pub(super) fn php_filter_email(
    _vm: &mut Vm,
    value: &mut Value,
    _flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let allowed = [LOWALPHA, HIALPHA, DIGIT, b"!#$%&'*+-=?^_`{|}~@.[]"].concat();
    filter_map_apply(value, &byte_set(&allowed));
    Ok(Outcome::Success)
}

/// `php_filter_url()`: `FILTER_SANITIZE_URL` keeps the characters of
/// RFC 1738 section 5.
pub(super) fn php_filter_url(
    _vm: &mut Vm,
    value: &mut Value,
    _flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    filter_map_apply(value, &url_chars());
    Ok(Outcome::Success)
}

/// The bytes `php_filter_url()` keeps (`LOWALPHA HIALPHA DIGIT SAFE EXTRA
/// NATIONAL PUNCTUATION RESERVED`).
fn url_chars() -> ByteSet {
    const SAFE: &[u8] = b"$-_.+";
    const EXTRA: &[u8] = b"!*'(),";
    const NATIONAL: &[u8] = b"{}|\\^~[]`";
    const PUNCTUATION: &[u8] = b"<>#%\"";
    const RESERVED: &[u8] = b";/?:@&=";
    byte_set(
        &[
            LOWALPHA,
            HIALPHA,
            DIGIT,
            SAFE,
            EXTRA,
            NATIONAL,
            PUNCTUATION,
            RESERVED,
        ]
        .concat(),
    )
}

/// `php_filter_number_int()`: `FILTER_SANITIZE_NUMBER_INT` keeps
/// `[0-9+-]`.
pub(super) fn php_filter_number_int(
    _vm: &mut Vm,
    value: &mut Value,
    _flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    filter_map_apply(value, &byte_set(b"+-0123456789"));
    Ok(Outcome::Success)
}

/// `php_filter_number_float()`: `FILTER_SANITIZE_NUMBER_FLOAT` keeps
/// `[0-9+-]` plus `.`, `,` and `eE` as the flags allow.
pub(super) fn php_filter_number_float(
    _vm: &mut Vm,
    value: &mut Value,
    flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let mut allowed = b"+-0123456789".to_vec();
    if flags & FILTER_FLAG_ALLOW_FRACTION != 0 {
        allowed.push(b'.');
    }
    if flags & FILTER_FLAG_ALLOW_THOUSAND != 0 {
        allowed.push(b',');
    }
    if flags & FILTER_FLAG_ALLOW_SCIENTIFIC != 0 {
        allowed.extend_from_slice(b"eE");
    }
    filter_map_apply(value, &byte_set(&allowed));
    Ok(Outcome::Success)
}

/// `php_filter_add_slashes()`: `FILTER_SANITIZE_ADD_SLASHES` (`addslashes()`).
pub(super) fn php_filter_add_slashes(
    _vm: &mut Vm,
    value: &mut Value,
    _flags: i64,
    _options: Option<&Value>,
) -> PResult<Outcome> {
    let s = value_str(value);
    *value = Value::String(PhpStr::from_vec(php_addslashes(&s)));
    Ok(Outcome::Success)
}
