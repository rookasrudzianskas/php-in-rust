//! ext/standard/html.c: `htmlspecialchars()`, `htmlentities()`, their decoders and
//! `get_html_translation_table()`.
//!
//! The entity and charset tables are generated from php-src's html_tables.h into
//! `html/tables.rs` (see `html/gen_tables.py`).

mod charset;
mod entity;
mod tables;

use std::borrow::Cow;

use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

pub use charset::{Charset, map_from_unicode, next_char, next_utf8_char, push_utf8};
use entity::{EntRow, MsTable, NamedEntity, resolve_named_entity};

// Flags (html.h).
pub const ENT_HTML_QUOTE_NONE: i64 = 0;
pub const ENT_HTML_QUOTE_SINGLE: i64 = 1;
pub const ENT_HTML_QUOTE_DOUBLE: i64 = 2;
pub const ENT_HTML_IGNORE_ERRORS: i64 = 4;
pub const ENT_HTML_SUBSTITUTE_ERRORS: i64 = 8;
pub const ENT_HTML_DOC_TYPE_MASK: i64 = 16 | 32;
pub const ENT_HTML_DOC_HTML401: i64 = 0;
pub const ENT_HTML_DOC_XML1: i64 = 16;
pub const ENT_HTML_DOC_XHTML: i64 = 32;
pub const ENT_HTML_DOC_HTML5: i64 = 16 | 32;
pub const ENT_HTML_SUBSTITUTE_DISALLOWED_CHARS: i64 = 128;

pub const PHP_HTML_SPECIALCHARS: i64 = 0;
pub const PHP_HTML_ENTITIES: i64 = 1;

pub const ENT_COMPAT: i64 = ENT_HTML_QUOTE_DOUBLE;
pub const ENT_QUOTES: i64 = ENT_HTML_QUOTE_DOUBLE | ENT_HTML_QUOTE_SINGLE;
pub const ENT_NOQUOTES: i64 = ENT_HTML_QUOTE_NONE;
pub const ENT_IGNORE: i64 = ENT_HTML_IGNORE_ERRORS;
pub const ENT_SUBSTITUTE: i64 = ENT_HTML_SUBSTITUTE_ERRORS;
pub const ENT_HTML401: i64 = 0;
pub const ENT_XML1: i64 = 16;
pub const ENT_XHTML: i64 = 32;
pub const ENT_HTML5: i64 = 16 | 32;
pub const ENT_DISALLOWED: i64 = 128;

pub fn register(r: &mut Registry) {
    for (name, value) in [
        ("HTML_SPECIALCHARS", PHP_HTML_SPECIALCHARS),
        ("HTML_ENTITIES", PHP_HTML_ENTITIES),
        ("ENT_COMPAT", ENT_COMPAT),
        ("ENT_QUOTES", ENT_QUOTES),
        ("ENT_NOQUOTES", ENT_NOQUOTES),
        ("ENT_IGNORE", ENT_IGNORE),
        ("ENT_SUBSTITUTE", ENT_SUBSTITUTE),
        ("ENT_DISALLOWED", ENT_DISALLOWED),
        ("ENT_HTML401", ENT_HTML401),
        ("ENT_XML1", ENT_XML1),
        ("ENT_XHTML", ENT_XHTML),
        ("ENT_HTML5", ENT_HTML5),
    ] {
        r.constant(name, value);
    }
    r.functions(&[
        &HTMLSPECIALCHARS,
        &HTMLSPECIALCHARS_DECODE,
        &HTML_ENTITY_DECODE,
        &HTMLENTITIES,
        &GET_HTML_TRANSLATION_TABLE,
    ]);
}

// ----- charset selection -----------------------------------------------------------------

/// The part of a C string argument before its first NUL byte.
fn c_str(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |n| &s[..n])
}

/// `get_default_charset`: the `internal_encoding` ini setting, else `default_charset`.
fn default_charset(vm: &Vm) -> Option<PhpStr> {
    ["internal_encoding", "default_charset"]
        .into_iter()
        .filter_map(|name| vm.ini_get(name))
        .find(|v| !c_str(v).is_empty())
}

/// `determine_charset`: the charset named by `hint` or, when it is absent or empty, by the
/// `internal_encoding`/`default_charset` ini settings; UTF-8 by default. An unsupported name
/// warns (unless `quiet`) and selects UTF-8.
pub fn determine_charset(vm: &mut Vm, hint: Option<&[u8]>, quiet: bool) -> PResult<Charset> {
    let ini;
    let name = match hint.map(c_str).filter(|h| !h.is_empty()) {
        Some(h) => h,
        None => match default_charset(vm) {
            Some(v) => {
                ini = v;
                c_str(&ini)
            }
            None => return Ok(Charset::Utf8),
        },
    };
    if let Some(cs) = Charset::from_name(name) {
        return Ok(cs);
    }
    if !quiet {
        let mut msg = b"Charset \"".to_vec();
        msg.extend_from_slice(name);
        msg.extend_from_slice(b"\" is not supported, assuming UTF-8");
        vm.warning(PhpStr::from_vec(msg))?;
    }
    Ok(Charset::Utf8)
}

// ----- validity rules --------------------------------------------------------------------

/// `unicode_cp_is_allowed`: whether code point `cp` may appear literally in a document of
/// type `doctype` (one of the `ENT_HTML_DOC_*` values).
///
/// ```text
/// XML 1.0          HTML 4.01          HTML 5
/// 0x09..0x0A       0x09..0x0A         0x09..0x0A
/// 0x0D             0x0D               0x0C..0x0D
/// 0x0020..0xD7FF   0x20..0x7E         0x20..0x7E
///                  0x00A0..0xD7FF     0x00A0..0xD7FF
/// 0xE000..0xFFFD   0xE000..0x10FFFF   0xE000..0xFDCF
/// 0x010000..0x10FFFF                  0xFDF0..0x10FFFF (except (cp & 0xFFFF) >= 0xFFFE)
/// ```
///
/// XHTML follows the XML 1.0 rules.
pub fn unicode_cp_is_allowed(cp: u32, doctype: i64) -> bool {
    match doctype {
        ENT_HTML_DOC_HTML401 => {
            (0x20..=0x7E).contains(&cp)
                || cp == 0x0A
                || cp == 0x09
                || cp == 0x0D
                || (0xA0..=0xD7FF).contains(&cp)
                || (0xE000..=0x10FFFF).contains(&cp)
        }
        ENT_HTML_DOC_HTML5 => {
            (0x20..=0x7E).contains(&cp)
                || ((0x09..=0x0D).contains(&cp) && cp != 0x0B)
                || (0xA0..=0xD7FF).contains(&cp)
                || ((0xE000..=0x10FFFF).contains(&cp)
                    && (cp & 0xFFFF) < 0xFFFE
                    && !(0xFDD0..=0xFDEF).contains(&cp))
        }
        ENT_HTML_DOC_XHTML | ENT_HTML_DOC_XML1 => {
            (0x20..=0xD7FF).contains(&cp)
                || cp == 0x0A
                || cp == 0x09
                || cp == 0x0D
                || ((0xE000..=0x10FFFF).contains(&cp) && cp != 0xFFFE && cp != 0xFFFF)
        }
        _ => true,
    }
}

/// `numeric_entity_is_allowed`: whether a numeric entity may reference `cp` in a document of
/// type `doctype` (less restrictive than [`unicode_cp_is_allowed`] for HTML).
pub fn numeric_entity_is_allowed(cp: u32, doctype: i64) -> bool {
    match doctype {
        // all non-SGML characters (those marked with UNUSED in DESCSET) should be
        // representable with numeric entities
        ENT_HTML_DOC_HTML401 => cp <= 0x10FFFF,
        // any code point but U+0000, U+000D, noncharacters and control characters other than
        // space characters (surrogates are allowed)
        ENT_HTML_DOC_HTML5 => {
            (0x20..=0x7E).contains(&cp)
                || ((0x09..=0x0C).contains(&cp) && cp != 0x0B)
                || ((0xA0..=0x10FFFF).contains(&cp)
                    && (cp & 0xFFFF) < 0xFFFE
                    && !(0xFDD0..=0xFDEF).contains(&cp))
        }
        // XML requires character references to match the production for Char
        ENT_HTML_DOC_XHTML | ENT_HTML_DOC_XML1 => unicode_cp_is_allowed(cp, doctype),
        _ => true,
    }
}

// ----- entity parsing --------------------------------------------------------------------

/// Byte `i` of `s`, or NUL past its end (the C code relies on the string terminator).
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `process_numeric_entity`: parses the numeric entity whose digits (or `x`) start at
/// `s[pos]`, the byte after `#`. Returns the code point and the index of the closing `;`.
fn process_numeric_entity(s: &[u8], pos: usize) -> Option<(u32, usize)> {
    let hex = matches!(at(s, pos), b'x' | b'X');
    let mut i = pos + usize::from(hex);
    let first = at(s, i);
    if !(if hex {
        first.is_ascii_hexdigit()
    } else {
        first.is_ascii_digit()
    }) {
        return None;
    }
    // strtol() in base 16 skips a "0x" prefix
    if hex
        && first == b'0'
        && matches!(at(s, i + 1), b'x' | b'X')
        && at(s, i + 2).is_ascii_hexdigit()
    {
        i += 2;
    }
    let base: u64 = if hex { 16 } else { 10 };
    let mut code: u64 = 0;
    while let Some(d) = char::from(at(s, i)).to_digit(base as u32) {
        // strtol() saturates, which is out of range as well
        code = code.saturating_mul(base).saturating_add(u64::from(d));
        i += 1;
    }
    if at(s, i) != b';' || code > 0x10FFFF {
        return None;
    }
    Some((code as u32, i))
}

/// `process_named_entity_html`: length of the alphanumeric entity name starting at `s[pos]`
/// when it is non-empty and followed by `;`.
fn process_named_entity_html(s: &[u8], pos: usize) -> Option<usize> {
    let len = s[pos.min(s.len())..]
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric())
        .count();
    (len > 0 && at(s, pos + len) == b';').then_some(len)
}

/// Whether `name` is a named entity of `map`, counting `apos` in XHTML (which uses the
/// HTML 4.01 map, lacking it).
fn resolve_entity(map: &[NamedEntity], name: &[u8], doctype: i64) -> Option<(u32, u32)> {
    resolve_named_entity(map, name).or_else(|| {
        (doctype == ENT_HTML_DOC_XHTML && name == b"apos").then_some((u32::from(b'\''), 0))
    })
}

/// `unescape_inverse_map`: entity -> code point map used to decode (or, with `all`, to
/// recognize the entities left alone when not double encoding).
fn unescape_inverse_map(all: bool, flags: i64) -> &'static [NamedEntity] {
    let doctype = flags & ENT_HTML_DOC_TYPE_MASK;
    if all {
        match doctype {
            // XHTML is special-cased for &apos;
            ENT_HTML_DOC_HTML401 | ENT_HTML_DOC_XHTML => &tables::HTML4_NAMES,
            ENT_HTML_DOC_HTML5 => &tables::HTML5_NAMES,
            _ => &tables::BASIC_APOS_NAMES,
        }
    } else if doctype == ENT_HTML_DOC_HTML401 {
        &tables::BASIC_NOAPOS_NAMES
    } else {
        &tables::BASIC_APOS_NAMES
    }
}

/// Whether `flags` keep the quote `code` from being encoded or decoded.
fn quote_excluded(code: u32, flags: i64) -> bool {
    (code == u32::from(b'\'') && flags & ENT_HTML_QUOTE_SINGLE == 0)
        || (code == u32::from(b'"') && flags & ENT_HTML_QUOTE_DOUBLE == 0)
}

// ----- decoding --------------------------------------------------------------------------

/// Outcome of decoding the entity at an `&`.
enum Decoded {
    /// Not an entity: the `&` is copied.
    Invalid,
    /// A complete entity that may not be decoded: it is copied up to its `;` (exclusive).
    Kept(usize),
    /// Code point(s) and index of the `;`.
    Entity(u32, u32, usize),
}

/// Decodes the entity at `s[amp]` (`traverse_for_entities`, at least 4 bytes remain).
fn decode_entity(s: &[u8], amp: usize, all: bool, flags: i64, inv_map: &[NamedEntity]) -> Decoded {
    let doctype = flags & ENT_HTML_DOC_TYPE_MASK;
    let (code, code2, semi) = if s[amp + 1] == b'#' {
        let Some((code, semi)) = process_numeric_entity(s, amp + 2) else {
            return Decoded::Invalid;
        };
        if !all && (code > 63 || tables::BASIC_APOS[code as usize].is_none()) {
            // htmlspecialchars_decode() only decodes the numeric entities of &, <, >, " and '
            return Decoded::Invalid;
        }
        // HTML 5 has a character that cannot be used in a numeric entity but is allowed
        // literally (U+000D)
        if !unicode_cp_is_allowed(code, doctype) || (doctype == ENT_HTML_DOC_HTML5 && code == 0x0D)
        {
            return Decoded::Invalid;
        }
        (code, 0, semi)
    } else {
        let start = amp + 1;
        let window = &s[start..s.len().min(start + tables::LONGEST_ENTITY_LENGTH + 1)];
        let Some(len) = window.iter().position(|&b| b == b';') else {
            return Decoded::Invalid;
        };
        if len == 0 {
            return Decoded::Invalid;
        }
        let Some((code, code2)) = resolve_entity(inv_map, &s[start..start + len], doctype) else {
            return Decoded::Invalid;
        };
        (code, code2, start + len)
    };
    if quote_excluded(code, flags) {
        return Decoded::Kept(semi);
    }
    Decoded::Entity(code, code2, semi)
}

/// `traverse_for_entities`
fn traverse_for_entities(
    s: &[u8],
    all: bool,
    flags: i64,
    inv_map: &[NamedEntity],
    charset: Charset,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut cur = 0;
    while cur < s.len() {
        let Some(off) = s[cur..].iter().position(|&b| b == b'&') else {
            out.extend_from_slice(&s[cur..]);
            break;
        };
        out.extend_from_slice(&s[cur..cur + off]);
        cur += off;
        // fewer than 4 bytes cannot hold an entity
        if s.len() - cur < 4 {
            out.extend_from_slice(&s[cur..]);
            break;
        }
        match decode_entity(s, cur, all, flags, inv_map) {
            Decoded::Invalid => {
                out.push(b'&');
                cur += 1;
            }
            Decoded::Kept(semi) => {
                out.extend_from_slice(&s[cur..semi]);
                cur = semi;
            }
            Decoded::Entity(code, code2, semi) => {
                // UTF-8 needs no mapping (ISO-8859-1 neither, but the call ensures code <= 0xFF)
                let code = if charset == Charset::Utf8 {
                    code
                } else {
                    match map_from_unicode(code, charset) {
                        Some(c) if code2 == 0 => c,
                        _ => {
                            out.extend_from_slice(&s[cur..semi]);
                            cur = semi;
                            continue;
                        }
                    }
                };
                charset::push_char(&mut out, charset, code);
                if code2 != 0 {
                    charset::push_char(&mut out, charset, code2);
                }
                cur = semi + 1;
            }
        }
    }
    out
}

/// Decodes HTML entities in a string of charset `charset` (the core of
/// `php_unescape_html_entities`). With `all`, every named entity of the document type of
/// `flags` and every allowed numeric entity is decoded (`html_entity_decode()`); otherwise only
/// those of `&`, `<`, `>` and the quotes selected by `flags` (`htmlspecialchars_decode()`,
/// where the charset does not matter).
pub fn unescape_html(s: &[u8], all: bool, flags: i64, charset: Charset) -> Cow<'_, [u8]> {
    if !s.contains(&b'&') {
        return Cow::Borrowed(s);
    }
    let charset = if all { charset } else { Charset::Iso8859_1 };
    let inv_map = unescape_inverse_map(all, flags);
    Cow::Owned(traverse_for_entities(s, all, flags, inv_map, charset))
}

/// `php_unescape_html_entities`: [`unescape_html`] with the charset named by `charset_hint`
/// (see [`determine_charset`]), which is only looked at with `all` and when `s` has an `&`.
pub fn unescape_html_entities<'a>(
    vm: &mut Vm,
    s: &'a [u8],
    all: bool,
    flags: i64,
    charset_hint: Option<&[u8]>,
) -> PResult<Cow<'a, [u8]>> {
    if !s.contains(&b'&') {
        return Ok(Cow::Borrowed(s));
    }
    let charset = if all {
        determine_charset(vm, charset_hint, false)?
    } else {
        Charset::Iso8859_1
    };
    Ok(unescape_html(s, all, flags, charset))
}

// ----- encoding --------------------------------------------------------------------------

/// `find_entity_for_char`: the entity of code point `k`, looking at the next character for
/// the entities of two code points (and consuming it when it completes one).
fn find_entity_for_char(
    k: u32,
    charset: Charset,
    table: &MsTable,
    s: &[u8],
    cursor: &mut usize,
) -> Option<&'static str> {
    match *table.row(k)? {
        EntRow::Empty => None,
        EntRow::One(e) => Some(e),
        EntRow::Multi(m) => {
            let before = *cursor;
            let second = if *cursor < s.len() {
                next_char(charset, s, cursor)
                    .and_then(|next| m.rest.iter().find(|(cp, _)| *cp == next))
            } else {
                None
            };
            match second {
                Some(&(_, e)) => Some(e),
                None => {
                    *cursor = before;
                    m.default
                }
            }
        }
    }
}

/// `html_verbatim_prefix_len`: length of the longest prefix of `s` copied unchanged when
/// only the basic entities are encoded and disallowed characters are not substituted.
fn verbatim_prefix_len(s: &[u8], flags: i64, charset: Charset) -> usize {
    let stops = |b: u8| match b {
        b'&' | b'<' | b'>' => true,
        b'"' => flags & ENT_HTML_QUOTE_DOUBLE != 0,
        b'\'' => flags & ENT_HTML_QUOTE_SINGLE != 0,
        0x80.. => !charset.single_byte(),
        _ => false,
    };
    let mut pos = 0;
    while pos < s.len() {
        let b = s[pos];
        if !stops(b) {
            pos += 1;
            continue;
        }
        if b >= 0x80 && charset == Charset::Utf8 {
            let mut next = pos;
            if next_utf8_char(s, &mut next).is_some() {
                pos = next;
                continue;
            }
        }
        break;
    }
    pos
}

/// `escape_html_entities_from`: encodes `s`, whose first `start` bytes are copied unchanged.
fn escape_from(
    s: &[u8],
    start: usize,
    all: bool,
    flags: i64,
    charset: Charset,
    double_encode: bool,
) -> Vec<u8> {
    let doctype = flags & ENT_HTML_DOC_TYPE_MASK;
    let ms_table = if doctype == ENT_HTML_DOC_HTML5 {
        &tables::HTML5
    } else {
        &tables::HTML4
    };
    let basic_table = if doctype == ENT_HTML_DOC_HTML401 {
        &tables::BASIC_NOAPOS
    } else {
        &tables::BASIC_APOS
    };
    let to_uni = if all && !charset.unicode_compat() {
        charset.to_unicode_table()
    } else {
        None
    };
    // identifies the valid named entities even if only the basic ones are encoded
    let inv_map = unescape_inverse_map(true, flags);
    let replacement: &[u8] =
        if flags & (ENT_HTML_SUBSTITUTE_ERRORS | ENT_HTML_SUBSTITUTE_DISALLOWED_CHARS) == 0 {
            b""
        } else if charset == Charset::Utf8 {
            "\u{FFFD}".as_bytes()
        } else {
            b"&#xFFFD;"
        };

    let mut out = Vec::with_capacity(if s.len() < 64 { 128 } else { s.len() * 2 });
    out.extend_from_slice(&s[..start]);
    let mut cursor = start;
    while cursor < s.len() {
        let before = cursor;
        let Some(this_char) = next_char(charset, s, &mut cursor) else {
            // invalid multi-byte sequence
            if flags & ENT_HTML_IGNORE_ERRORS != 0 {
                continue;
            } else if flags & ENT_HTML_SUBSTITUTE_ERRORS != 0 {
                out.extend_from_slice(replacement);
                continue;
            }
            return Vec::new();
        };
        let mut sequence = &s[before..cursor];

        if this_char != u32::from(b'&') {
            if quote_excluded(this_char, flags) {
                out.extend_from_slice(sequence);
                continue;
            }
            let mut this_char = this_char;
            let rep = if all {
                if let Some(to_uni) = to_uni {
                    // a single-byte charset (UTF-8 is the only multi-byte charset with `all`)
                    this_char = u32::from(to_uni[this_char as usize]);
                    if this_char == 0xFFFF {
                        // no mapping; pass through
                        out.extend_from_slice(sequence);
                        continue;
                    }
                }
                // the cursor may advance
                find_entity_for_char(this_char, charset, ms_table, s, &mut cursor)
            } else if this_char < 64 {
                basic_table[this_char as usize]
            } else {
                None
            };

            if let Some(rep) = rep {
                out.push(b'&');
                out.extend_from_slice(rep.as_bytes());
                out.push(b';');
                continue;
            }
            // no entity for this char: pass it unchanged if it is valid
            if flags & ENT_HTML_SUBSTITUTE_DISALLOWED_CHARS != 0 {
                let disallowed = if charset.unicode_compat() || to_uni.is_some() {
                    // `this_char` is already a code point (mapped above for `to_uni`)
                    !unicode_cp_is_allowed(this_char, doctype)
                } else {
                    // Not a code point unless, coincidentally, in the 0x20..0x7D range (except
                    // 0x5C in SJIS). Conversion tables frequently map 0x00-0x1F to the C0 code
                    // points, so admit that is the case.
                    this_char <= 0x7D && !unicode_cp_is_allowed(this_char, doctype)
                };
                if disallowed {
                    sequence = replacement;
                }
            }
            out.extend_from_slice(sequence);
        } else if double_encode {
            out.extend_from_slice(b"&amp;");
        } else {
            // keep the entity starting here if it is valid
            let ent_len = if at(s, cursor) == b'#' {
                process_numeric_entity(s, cursor + 1)
                    .filter(|&(cp, _)| {
                        flags & ENT_HTML_SUBSTITUTE_DISALLOWED_CHARS == 0
                            || numeric_entity_is_allowed(cp, doctype)
                    })
                    .map(|(_, semi)| semi - cursor)
            } else {
                process_named_entity_html(s, cursor).filter(|&len| {
                    resolve_entity(inv_map, &s[cursor..cursor + len], doctype).is_some()
                })
            };
            match ent_len {
                Some(len) => {
                    out.push(b'&');
                    out.extend_from_slice(&s[cursor..cursor + len]);
                    out.push(b';');
                    cursor += len + 1;
                }
                None => out.extend_from_slice(b"&amp;"),
            }
        }
    }
    out
}

/// Encodes a string of charset `charset` (the core of `php_escape_html_entities_ex`): `&`,
/// `<`, `>` and the quotes selected by `flags` become entities and, with `all`, every
/// character that has a named entity in the document type of `flags`. Invalid code unit
/// sequences are dropped (`ENT_IGNORE`), replaced by U+FFFD (`ENT_SUBSTITUTE`) or make the
/// result empty; `ENT_DISALLOWED` replaces the characters the document type does not allow.
/// Without `double_encode`, valid entities are left alone.
///
/// `all` is ignored for XML 1.0 and the multi-byte charsets other than UTF-8.
pub fn escape_html(
    s: &[u8],
    all: bool,
    flags: i64,
    charset: Charset,
    double_encode: bool,
) -> Cow<'_, [u8]> {
    // LIMIT_ALL
    let all =
        all && !charset.partial_support() && flags & ENT_HTML_DOC_TYPE_MASK != ENT_HTML_DOC_XML1;
    let mut start = 0;
    if !all && flags & ENT_HTML_SUBSTITUTE_DISALLOWED_CHARS == 0 {
        start = verbatim_prefix_len(s, flags, charset);
        if start == s.len() {
            return Cow::Borrowed(s);
        }
    }
    Cow::Owned(escape_from(s, start, all, flags, charset, double_encode))
}

/// `php_escape_html_entities_ex`: [`escape_html`] with the charset named by `charset_hint`
/// (see [`determine_charset`]). Unless `quiet`, an unsupported charset warns and `all` with a
/// multi-byte charset other than UTF-8 raises a notice.
pub fn escape_html_entities<'a>(
    vm: &mut Vm,
    s: &'a [u8],
    all: bool,
    flags: i64,
    charset_hint: Option<&[u8]>,
    double_encode: bool,
    quiet: bool,
) -> PResult<Cow<'a, [u8]>> {
    let charset = determine_charset(vm, charset_hint, quiet)?;
    if all && !quiet && charset.partial_support() {
        vm.notice(
            "Only basic entities substitution is supported for multi-byte encodings other than \
             UTF-8; functionality is equivalent to htmlspecialchars",
        )?;
    }
    Ok(escape_html(s, all, flags, charset, double_encode))
}

// ----- PHP functions ---------------------------------------------------------------------

fn string_result(original: &PhpStr, result: Cow<'_, [u8]>) -> Value {
    match result {
        Cow::Borrowed(_) => Value::String(original.clone()),
        Cow::Owned(bytes) => Value::String(PhpStr::from_vec(bytes)),
    }
}

/// `php_html_entities`
fn php_html_entities(vm: &mut Vm, args: &mut Args, all: bool) -> PResult<Value> {
    let s = args.str(0).clone();
    if s.is_empty() {
        return Ok(Value::String(s));
    }
    let flags = args.int(1);
    let hint = args.str_opt(2).cloned();
    let double_encode = args.bool(3);
    let r = escape_html_entities(vm, &s, all, flags, hint.as_deref(), double_encode, false)?;
    Ok(string_result(&s, r))
}

fn htmlspecialchars(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_html_entities(vm, args, false)
}

fn htmlentities(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    php_html_entities(vm, args, true)
}

fn htmlspecialchars_decode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let r = unescape_html_entities(vm, &s, false, args.int(1), None)?;
    Ok(string_result(&s, r))
}

fn html_entity_decode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let hint = args.str_opt(2).cloned();
    let r = unescape_html_entities(vm, &s, true, args.int(1), hint.as_deref())?;
    Ok(string_result(&s, r))
}

pub static HTMLSPECIALCHARS: NativeFunction = NativeFunction {
    sig: "htmlspecialchars(string $string, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401, ?string $encoding = null, bool $double_encode = true): string",
    handler: htmlspecialchars,
};

pub static HTMLSPECIALCHARS_DECODE: NativeFunction = NativeFunction {
    sig: "htmlspecialchars_decode(string $string, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401): string",
    handler: htmlspecialchars_decode,
};

pub static HTML_ENTITY_DECODE: NativeFunction = NativeFunction {
    sig: "html_entity_decode(string $string, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401, ?string $encoding = null): string",
    handler: html_entity_decode,
};

pub static HTMLENTITIES: NativeFunction = NativeFunction {
    sig: "htmlentities(string $string, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401, ?string $encoding = null, bool $double_encode = true): string",
    handler: htmlentities,
};

pub static GET_HTML_TRANSLATION_TABLE: NativeFunction = NativeFunction {
    sig: "get_html_translation_table(int $table = HTML_SPECIALCHARS, int $flags = ENT_QUOTES | ENT_SUBSTITUTE | ENT_HTML401, string $encoding = \"UTF-8\"): array",
    handler: get_html_translation_table,
};

/// `write_s3row_data`: adds the entities of `row` (the row of `orig_cp`, a character of
/// `charset`) to the translation table.
fn write_s3row_data(row: &EntRow, orig_cp: u32, charset: Charset, arr: &mut PhpArray) {
    let mut key = Vec::with_capacity(8);
    charset::push_char(&mut key, charset, orig_cp);
    let mut add = |key: &[u8], entity: &str| {
        let mut value = Vec::with_capacity(entity.len() + 2);
        value.push(b'&');
        value.extend_from_slice(entity.as_bytes());
        value.push(b';');
        arr.insert(
            ArrayKey::from_bytes(key),
            Value::String(PhpStr::from_vec(value)),
        );
    };
    match *row {
        EntRow::Empty => {}
        EntRow::One(e) => add(&key, e),
        EntRow::Multi(m) => {
            if let Some(e) = m.default {
                add(&key, e);
            }
            let first_len = key.len();
            for &(second, e) in m.rest {
                let second = if charset.unicode_compat() {
                    second
                } else {
                    match map_from_unicode(second, charset) {
                        Some(c) => c,
                        // not representable in this charset
                        None => continue,
                    }
                };
                key.truncate(first_len);
                charset::push_char(&mut key, charset, second);
                add(&key, e);
            }
        }
    }
}

/// `get_html_translation_table()`
fn get_html_translation_table(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let all = args.int(0);
    let flags = args.int(1);
    let hint = args.str(2).clone();
    let charset = determine_charset(vm, Some(&hint), false)?;
    let doctype = flags & ENT_HTML_DOC_TYPE_MASK;
    // LIMIT_ALL
    let all = all != 0 && !charset.partial_support() && doctype != ENT_HTML_DOC_XML1;
    let mut arr = PhpArray::new();
    if all {
        let ms_table = if doctype == ENT_HTML_DOC_HTML5 {
            &tables::HTML5
        } else {
            &tables::HTML4
        };
        if let Some(to_uni) = charset
            .to_unicode_table()
            .filter(|_| !charset.unicode_compat())
        {
            // map the code points of the charset to Unicode
            for i in 0..=0xFFu32 {
                // can be done before mapping, they are invariant
                if quote_excluded(i, flags) {
                    continue;
                }
                if let Some(row) = ms_table.row(u32::from(to_uni[i as usize])) {
                    write_s3row_data(row, i, charset, &mut arr);
                }
            }
        } else {
            // no mapping to Unicode required
            let (max_i, max_j) = if charset.single_byte() {
                (1, 4)
            } else {
                (0x1E, 64)
            };
            for i in 0..max_i {
                if ms_table.stage1_is_empty(i) {
                    continue;
                }
                for j in 0..max_j {
                    if ms_table.stage2_is_empty(i, j) {
                        continue;
                    }
                    for k in 0..64 {
                        let code = ((i << 12) | (j << 6) | k) as u32;
                        if quote_excluded(code, flags) {
                            continue;
                        }
                        if let Some(row) = ms_table.row(code) {
                            write_s3row_data(row, code, charset, &mut arr);
                        }
                    }
                }
            }
        }
    } else {
        let table = if doctype == ENT_HTML_DOC_HTML401 {
            &tables::BASIC_NOAPOS
        } else {
            &tables::BASIC_APOS
        };
        let rows = table
            .iter()
            .enumerate()
            .filter_map(|(j, e)| e.map(|e| (j as u32, e)));
        for (j, e) in rows {
            if !quote_excluded(j, flags) {
                // the charset does not matter, ISO-8859-1 is used for efficiency
                write_s3row_data(&EntRow::One(e), j, Charset::Iso8859_1, &mut arr);
            }
        }
    }
    Ok(Value::array(arr))
}
