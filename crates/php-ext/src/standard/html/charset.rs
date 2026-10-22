//! Charsets of html.c: name lookup, character decoding (`get_next_char`) and the mappings
//! between single-byte charsets and Unicode.

use super::tables;

/// `enum entity_charset` (the order matters for the classification methods).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Charset {
    Utf8,
    Iso8859_1,
    Cp1252,
    Iso8859_15,
    Cp1251,
    Iso8859_5,
    Cp866,
    MacRoman,
    Koi8R,
    Big5,
    Gb2312,
    Big5Hkscs,
    Sjis,
    EucJp,
}

impl Charset {
    /// `CHARSET_UNICODE_COMPAT`: code points of the charset are Unicode code points.
    pub fn unicode_compat(self) -> bool {
        self as u8 <= Charset::Iso8859_1 as u8
    }

    /// `CHARSET_SINGLE_BYTE`
    pub fn single_byte(self) -> bool {
        self != Charset::Utf8 && (self as u8) < Charset::Big5 as u8
    }

    /// `CHARSET_PARTIAL_SUPPORT`: multi-byte charsets other than UTF-8 (basic entities only).
    pub fn partial_support(self) -> bool {
        self as u8 >= Charset::Big5 as u8
    }

    /// Resolves a non-empty charset name (`charset_is_utf8` + `lookup_charset`): exactly
    /// "utf-8" in any case, or a name or alias of `charset_map`, case-insensitively.
    /// `None` when the charset is not supported.
    pub fn from_name(name: &[u8]) -> Option<Charset> {
        if name.len() == 5 && name[..3].eq_ignore_ascii_case(b"utf") && &name[3..] == b"-8" {
            return Some(Charset::Utf8);
        }
        if name.len() > tables::LONGEST_CODESET_LENGTH {
            return None;
        }
        tables::CHARSET_MAP
            .iter()
            .find(|(codeset, _)| codeset.as_bytes().eq_ignore_ascii_case(name))
            .map(|&(_, cs)| cs)
    }

    /// The table mapping the bytes of a single-byte charset to Unicode (`enc_to_uni_index`).
    pub fn to_unicode_table(self) -> Option<&'static [u16; 256]> {
        tables::ENC_TO_UNI[self as usize]
    }
}

/// UTF-8 case of [`next_char`] (`php_next_utf8_char`): decodes the character at `*cursor`
/// and moves the cursor after it. On an invalid sequence, returns `None` and moves the cursor
/// past the bytes that belong to it, following strategy 2 of UTR #36 section 3.6.1 (never
/// swallowing a byte that may start a valid sequence).
pub fn next_utf8_char(s: &[u8], cursor: &mut usize) -> Option<u32> {
    let pos = *cursor;
    let fail = |cursor: &mut usize, advance: usize| {
        *cursor = pos + advance;
        None
    };
    let Some(&c) = s.get(pos) else {
        return fail(cursor, 1);
    };
    let avail = s.len() - pos;
    let trail = |i: usize| (0x80..=0xBF).contains(&s[pos + i]);
    let lead = |i: usize| s[pos + i] < 0x80 || (0xC2..=0xF4).contains(&s[pos + i]);
    let c = u32::from(c);
    let (cp, len) = if c < 0x80 {
        (c, 1)
    } else if c < 0xC2 {
        return fail(cursor, 1);
    } else if c < 0xE0 {
        if avail < 2 {
            return fail(cursor, 1);
        }
        if !trail(1) {
            return fail(cursor, if lead(1) { 1 } else { 2 });
        }
        let cp = ((c & 0x1F) << 6) | u32::from(s[pos + 1] & 0x3F);
        if cp < 0x80 {
            // non-shortest form
            return fail(cursor, 2);
        }
        (cp, 2)
    } else if c < 0xF0 {
        if avail < 3 || !trail(1) || !trail(2) {
            return if avail < 2 || lead(1) {
                fail(cursor, 1)
            } else if avail < 3 || lead(2) {
                fail(cursor, 2)
            } else {
                fail(cursor, 3)
            };
        }
        let cp =
            ((c & 0x0F) << 12) | (u32::from(s[pos + 1] & 0x3F) << 6) | u32::from(s[pos + 2] & 0x3F);
        if cp < 0x800 || (0xD800..=0xDFFF).contains(&cp) {
            // non-shortest form or surrogate
            return fail(cursor, 3);
        }
        (cp, 3)
    } else if c < 0xF5 {
        if avail < 4 || !trail(1) || !trail(2) || !trail(3) {
            return if avail < 2 || lead(1) {
                fail(cursor, 1)
            } else if avail < 3 || lead(2) {
                fail(cursor, 2)
            } else if avail < 4 || lead(3) {
                fail(cursor, 3)
            } else {
                fail(cursor, 4)
            };
        }
        let cp = ((c & 0x07) << 18)
            | (u32::from(s[pos + 1] & 0x3F) << 12)
            | (u32::from(s[pos + 2] & 0x3F) << 6)
            | u32::from(s[pos + 3] & 0x3F);
        if !(0x10000..=0x10FFFF).contains(&cp) {
            // non-shortest form or outside the Unicode range
            return fail(cursor, 4);
        }
        (cp, 4)
    } else {
        return fail(cursor, 1);
    };
    *cursor = pos + len;
    Some(cp)
}

/// `get_next_char`: decodes the character of charset `cs` at `*cursor` and moves the cursor
/// after it. Multi-byte characters of the East Asian charsets are returned as their code
/// units (`lead << 8 | trail`, or three bytes for EUC-JP's JIS X 0212), not as Unicode.
/// On an invalid sequence, returns `None` and moves the cursor past the invalid bytes.
pub fn next_char(cs: Charset, s: &[u8], cursor: &mut usize) -> Option<u32> {
    let pos = *cursor;
    let fail = |cursor: &mut usize, advance: usize| {
        *cursor = pos + advance;
        None
    };
    let Some(&c) = s.get(pos) else {
        return fail(cursor, 1);
    };
    let next = s.get(pos + 1).copied();
    let two = |next: u8| (u32::from(c) << 8) | u32::from(next);
    let (cp, len) = match cs {
        Charset::Utf8 => return next_utf8_char(s, cursor),
        Charset::Big5 | Charset::Big5Hkscs if (0x81..=0xFE).contains(&c) => {
            // reference http://demo.icu-project.org/icu-bin/convexp?conv=big5
            let Some(next) = next else {
                return fail(cursor, 1);
            };
            if (0x40..=0x7E).contains(&next) || (0xA1..=0xFE).contains(&next) {
                (two(next), 2)
            } else if cs == Charset::Big5 || (next != 0x80 && next != 0xFF) {
                return fail(cursor, 1);
            } else {
                return fail(cursor, 2);
            }
        }
        Charset::Gb2312 => {
            // EUC-CN
            let gb2312_lead = |c: u8| c != 0x8E && c != 0x8F && c != 0xA0 && c != 0xFF;
            if (0xA1..=0xFE).contains(&c) {
                let Some(next) = next else {
                    return fail(cursor, 1);
                };
                if (0xA1..=0xFE).contains(&next) {
                    (two(next), 2)
                } else if gb2312_lead(next) {
                    return fail(cursor, 1);
                } else {
                    return fail(cursor, 2);
                }
            } else if gb2312_lead(c) {
                (u32::from(c), 1)
            } else {
                return fail(cursor, 1);
            }
        }
        Charset::Sjis => {
            if (0x81..=0x9F).contains(&c) || (0xE0..=0xFC).contains(&c) {
                let Some(next) = next else {
                    return fail(cursor, 1);
                };
                if next >= 0x40 && next != 0x7F && next < 0xFD {
                    (two(next), 2)
                } else if next != 0x80 && next != 0xA0 && next < 0xFD {
                    return fail(cursor, 1);
                } else {
                    return fail(cursor, 2);
                }
            } else if c < 0x80 || (0xA1..=0xDF).contains(&c) {
                (u32::from(c), 1)
            } else {
                return fail(cursor, 1);
            }
        }
        Charset::EucJp => {
            let bad_trail_advance = |next: u8| if next != 0xA0 && next != 0xFF { 1 } else { 2 };
            if (0xA1..=0xFE).contains(&c) {
                let Some(next) = next else {
                    return fail(cursor, 1);
                };
                if (0xA1..=0xFE).contains(&next) {
                    // JIS X 0208 kanji
                    (two(next), 2)
                } else {
                    return fail(cursor, bad_trail_advance(next));
                }
            } else if c == 0x8E {
                let Some(next) = next else {
                    return fail(cursor, 1);
                };
                if (0xA1..=0xDF).contains(&next) {
                    // JIS X 0201 kana
                    (two(next), 2)
                } else {
                    return fail(cursor, bad_trail_advance(next));
                }
            } else if c == 0x8F {
                let avail = s.len() - pos;
                let ok = |i: usize| (0xA1..=0xFE).contains(&s[pos + i]);
                let a0_or_ff = |i: usize| s[pos + i] == 0xA0 || s[pos + i] == 0xFF;
                if avail < 3 || !ok(1) || !ok(2) {
                    return if avail < 2 || !a0_or_ff(1) {
                        fail(cursor, 1)
                    } else if avail < 3 || !a0_or_ff(2) {
                        fail(cursor, 2)
                    } else {
                        fail(cursor, 3)
                    };
                }
                // JIS X 0212 hojo-kanji
                let cp =
                    (u32::from(c) << 16) | (u32::from(s[pos + 1]) << 8) | u32::from(s[pos + 2]);
                (cp, 3)
            } else if c != 0xA0 && c != 0xFF {
                (u32::from(c), 1)
            } else {
                return fail(cursor, 1);
            }
        }
        // single-byte charsets, and single bytes of BIG5
        _ => (u32::from(c), 1),
    };
    *cursor = pos + len;
    Some(cp)
}

/// `unimap_bsearch`: the byte a BMP code point maps to in a `unimap_*` table.
fn unimap_lookup(table: &[(u16, u8)], code: u32) -> Option<u32> {
    let code = u16::try_from(code).ok()?;
    table
        .binary_search_by_key(&code, |&(cp, _)| cp)
        .ok()
        .map(|i| u32::from(table[i].1))
}

/// `map_from_unicode`: the code of Unicode code point `code` in charset `cs`, if it has one.
/// Only the ASCII range is mapped for the multi-byte charsets other than UTF-8.
pub fn map_from_unicode(code: u32, cs: Charset) -> Option<u32> {
    let table_over_7f = |table: &[(u16, u8)]| {
        if code <= 0x7F {
            Some(code)
        } else {
            unimap_lookup(table, code)
        }
    };
    match cs {
        // identity mapping of code points to unicode
        Charset::Iso8859_1 => (code <= 0xFF).then_some(code),
        Charset::Iso8859_5 => match code {
            // soft hyphen
            0..=0xA0 | 0xAD => Some(code),
            // numero sign
            0x2116 => Some(0xF0),
            // section sign
            0xA7 => Some(0xFD),
            0x040D | 0x0450 | 0x045D => None,
            0x0401..=0x045F => Some(code - 0x360),
            _ => None,
        },
        Charset::Iso8859_15 => {
            if code < 0xA4 || (code > 0xBE && code <= 0xFF) {
                Some(code)
            } else {
                unimap_lookup(&tables::UNIMAP_ISO885915, code)
            }
        }
        Charset::Cp1252 => {
            if code <= 0x7F || (0xA0..=0xFF).contains(&code) {
                Some(code)
            } else {
                unimap_lookup(&tables::UNIMAP_WIN1252, code)
            }
        }
        Charset::MacRoman => {
            if code == 0x7F {
                None
            } else {
                table_over_7f(&tables::UNIMAP_MACROMAN)
            }
        }
        Charset::Cp1251 => table_over_7f(&tables::UNIMAP_WIN1251),
        Charset::Koi8R => table_over_7f(&tables::UNIMAP_KOI8R),
        Charset::Cp866 => table_over_7f(&tables::UNIMAP_CP866),
        // 0x5C is interpreted as the Yen sign in the Japanese charsets. This is not universal.
        // See <http://www.w3.org/Submission/japanese-xml/#ambiguity_of_yen>
        Charset::Sjis | Charset::EucJp => {
            ((0x20..=0x7D).contains(&code) && code != 0x5C).then_some(code)
        }
        Charset::Big5 | Charset::Big5Hkscs | Charset::Gb2312 => {
            (0x20..=0x7D).contains(&code).then_some(code)
        }
        Charset::Utf8 => None,
    }
}

/// `php_utf32_utf8`: appends the UTF-8 encoding of `k` (at most 0x10FFFF, not checked).
pub fn push_utf8(buf: &mut Vec<u8>, k: u32) {
    if k < 0x80 {
        buf.push(k as u8);
    } else if k < 0x800 {
        buf.extend_from_slice(&[0xC0 | (k >> 6) as u8, 0x80 | (k & 0x3F) as u8]);
    } else if k < 0x10000 {
        buf.extend_from_slice(&[
            0xE0 | (k >> 12) as u8,
            0x80 | ((k >> 6) & 0x3F) as u8,
            0x80 | (k & 0x3F) as u8,
        ]);
    } else {
        buf.extend_from_slice(&[
            0xF0 | (k >> 18) as u8,
            0x80 | ((k >> 12) & 0x3F) as u8,
            0x80 | ((k >> 6) & 0x3F) as u8,
            0x80 | (k & 0x3F) as u8,
        ]);
    }
}

/// `write_octet_sequence`: appends character `code` of charset `cs`. Outside UTF-8 only
/// single bytes are written (the low byte of `code`, as the C code does).
pub fn push_char(buf: &mut Vec<u8>, cs: Charset, code: u32) {
    if cs == Charset::Utf8 {
        push_utf8(buf, code);
    } else {
        buf.push(code as u8);
    }
}
