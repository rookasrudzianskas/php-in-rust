//! Character tables (PCRE2's default "C" locale tables), newline and
//! horizontal/vertical space helpers, and UTF-8 utilities.

pub(crate) const CTYPE_SPACE: u8 = 0x01;
pub(crate) const CTYPE_LETTER: u8 = 0x02;
pub(crate) const CTYPE_LCLETTER: u8 = 0x04;
pub(crate) const CTYPE_DIGIT: u8 = 0x08;
pub(crate) const CTYPE_WORD: u8 = 0x10;

/// Offsets of the individual class bitmaps inside the 320-byte cbits table.
pub(crate) const CBIT_SPACE: usize = 0;
pub(crate) const CBIT_XDIGIT: usize = 32;
pub(crate) const CBIT_DIGIT: usize = 64;
pub(crate) const CBIT_UPPER: usize = 96;
pub(crate) const CBIT_LOWER: usize = 128;
pub(crate) const CBIT_WORD: usize = 160;
pub(crate) const CBIT_GRAPH: usize = 192;
pub(crate) const CBIT_PRINT: usize = 224;
pub(crate) const CBIT_PUNCT: usize = 256;
pub(crate) const CBIT_CNTRL: usize = 288;

/// The character tables used by the compiler and matcher.
pub(crate) struct Tables {
    /// Lower-case table.
    pub lcc: [u8; 256],
    /// Case-flip table.
    pub fcc: [u8; 256],
    /// Class bitmaps.
    pub cbits: [u8; 320],
    /// Character type bits.
    pub ctypes: [u8; 256],
}

const fn build_tables() -> Tables {
    let mut lcc = [0u8; 256];
    let mut fcc = [0u8; 256];
    let mut cbits = [0u8; 320];
    let mut ctypes = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let c = i as u8;
        let is_upper = c.is_ascii_uppercase();
        let is_lower = c.is_ascii_lowercase();
        let is_digit = c.is_ascii_digit();
        let is_alpha = is_upper || is_lower;
        // C-locale isspace: \t \n \v \f \r and space.
        let is_space = matches!(c, 0x09..=0x0d | 0x20);
        let is_xdigit = c.is_ascii_hexdigit();
        let is_graph = c > 0x20 && c < 0x7f;
        let is_print = c >= 0x20 && c < 0x7f;
        let is_punct = is_graph && !is_alpha && !is_digit;
        let is_cntrl = c < 0x20 || c == 0x7f;
        let is_word = is_alpha || is_digit || c == b'_';

        lcc[i] = if is_upper { c + 32 } else { c };
        fcc[i] = if is_upper {
            c + 32
        } else if is_lower {
            c - 32
        } else {
            c
        };

        let byte = i / 8;
        let bit = 1u8 << (i % 8);
        if is_space {
            cbits[CBIT_SPACE + byte] |= bit;
        }
        if is_xdigit {
            cbits[CBIT_XDIGIT + byte] |= bit;
        }
        if is_digit {
            cbits[CBIT_DIGIT + byte] |= bit;
        }
        if is_upper {
            cbits[CBIT_UPPER + byte] |= bit;
        }
        if is_lower {
            cbits[CBIT_LOWER + byte] |= bit;
        }
        if is_word {
            cbits[CBIT_WORD + byte] |= bit;
        }
        if is_graph {
            cbits[CBIT_GRAPH + byte] |= bit;
        }
        if is_print {
            cbits[CBIT_PRINT + byte] |= bit;
        }
        if is_punct {
            cbits[CBIT_PUNCT + byte] |= bit;
        }
        if is_cntrl {
            cbits[CBIT_CNTRL + byte] |= bit;
        }

        let mut t = 0u8;
        if is_space {
            t |= CTYPE_SPACE;
        }
        if is_alpha {
            t |= CTYPE_LETTER;
        }
        if is_lower {
            t |= CTYPE_LCLETTER;
        }
        if is_digit {
            t |= CTYPE_DIGIT;
        }
        if is_word {
            t |= CTYPE_WORD;
        }
        ctypes[i] = t;
        i += 1;
    }
    Tables {
        lcc,
        fcc,
        cbits,
        ctypes,
    }
}

/// PCRE2's built-in default tables (generated for the "C" locale).
pub(crate) static DEFAULT_TABLES: Tables = build_tables();

#[inline]
pub(crate) fn ctype(c: u32) -> u8 {
    if c < 256 {
        DEFAULT_TABLES.ctypes[c as usize]
    } else {
        0
    }
}

#[inline]
pub(crate) fn lcc(c: u32) -> u32 {
    if c < 256 {
        DEFAULT_TABLES.lcc[c as usize] as u32
    } else {
        c
    }
}

#[inline]
pub(crate) fn fcc(c: u32) -> u32 {
    if c < 256 {
        DEFAULT_TABLES.fcc[c as usize] as u32
    } else {
        c
    }
}

pub(crate) const CHAR_NEL: u32 = 0x85;

/// Horizontal space characters (`\h`).
pub(crate) const HSPACE_LIST: [u32; 19] = [
    0x09, 0x20, 0xa0, 0x1680, 0x180e, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004, 0x2005, 0x2006,
    0x2007, 0x2008, 0x2009, 0x200A, 0x202f, 0x205f, 0x3000,
];

/// Vertical space characters (`\v`).
pub(crate) const VSPACE_LIST: [u32; 7] = [0x0a, 0x0b, 0x0c, 0x0d, 0x85, 0x2028, 0x2029];

#[inline]
pub(crate) fn is_hspace(c: u32) -> bool {
    matches!(
        c,
        0x09 | 0x20 | 0xa0 | 0x1680 | 0x180e | 0x2000..=0x200a | 0x202f | 0x205f | 0x3000
    )
}

#[inline]
pub(crate) fn is_vspace(c: u32) -> bool {
    matches!(c, 0x0a..=0x0d | 0x85 | 0x2028 | 0x2029)
}

// ---------------------------------------------------------------------------
// UTF-8 helpers
// ---------------------------------------------------------------------------

/// Decode the character starting at `pos` (which must be a valid UTF-8
/// sequence start inside `s`). Returns (code point, byte length).
/// Malformed input is decoded leniently (never panics).
#[inline]
pub(crate) fn decode_utf8(s: &[u8], pos: usize) -> (u32, usize) {
    let c = s[pos] as u32;
    if c < 0xc0 {
        return (c, 1);
    }
    let (n, init) = if c < 0xe0 {
        (1, c & 0x1f)
    } else if c < 0xf0 {
        (2, c & 0x0f)
    } else if c < 0xf8 {
        (3, c & 0x07)
    } else if c < 0xfc {
        (4, c & 0x03)
    } else {
        (5, c & 0x01)
    };
    let mut v = init;
    let mut len = 1;
    while len <= n && pos + len < s.len() {
        v = (v << 6) | (s[pos + len] as u32 & 0x3f);
        len += 1;
    }
    (v, len)
}

/// Encode a code point as UTF-8 (lenient: values up to 0x7fffffff).
pub(crate) fn encode_utf8(c: u32, out: &mut Vec<u8>) {
    if c < 0x80 {
        out.push(c as u8);
    } else if c < 0x800 {
        out.push(0xc0 | (c >> 6) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else if c < 0x10000 {
        out.push(0xe0 | (c >> 12) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else if c < 0x200000 {
        out.push(0xf0 | (c >> 18) as u8);
        out.push(0x80 | ((c >> 12) & 0x3f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else if c < 0x4000000 {
        out.push(0xf8 | (c >> 24) as u8);
        out.push(0x80 | ((c >> 18) & 0x3f) as u8);
        out.push(0x80 | ((c >> 12) & 0x3f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    } else {
        out.push(0xfc | (c >> 30) as u8);
        out.push(0x80 | ((c >> 24) & 0x3f) as u8);
        out.push(0x80 | ((c >> 18) & 0x3f) as u8);
        out.push(0x80 | ((c >> 12) & 0x3f) as u8);
        out.push(0x80 | ((c >> 6) & 0x3f) as u8);
        out.push(0x80 | (c & 0x3f) as u8);
    }
}

/// Number of bytes the UTF-8 encoding of `c` takes.
#[inline]
pub(crate) fn utf8_len(c: u32) -> usize {
    if c < 0x80 {
        1
    } else if c < 0x800 {
        2
    } else if c < 0x10000 {
        3
    } else if c < 0x200000 {
        4
    } else if c < 0x4000000 {
        5
    } else {
        6
    }
}

/// First byte of the UTF-8 encoding of `c`.
#[inline]
pub(crate) fn utf8_first_byte(c: u32) -> u8 {
    let mut v = Vec::with_capacity(6);
    encode_utf8(c, &mut v);
    v[0]
}

/// PCRE2's `valid_utf()` for 8-bit code units. Returns `Err((code, offset))`
/// with the negative `PCRE2_ERROR_UTF8_ERRn` code and the offset of the start
/// of the bad character.
pub(crate) fn valid_utf8(s: &[u8]) -> Result<(), (i32, usize)> {
    let mut p = 0usize;
    let mut length = s.len();
    while length > 0 {
        let c = s[p] as u32;
        length -= 1;
        if c < 128 {
            p += 1;
            continue;
        }
        if c < 0xc0 {
            return Err((-22, p));
        }
        if c >= 0xfe {
            return Err((-23, p));
        }
        let ab: usize = match c & 0x3f {
            0x00..=0x1f => 1,
            0x20..=0x2f => 2,
            0x30..=0x37 => 3,
            0x38..=0x3b => 4,
            _ => 5,
        };
        if length < ab {
            return Err((-(2 + (ab - length) as i32), p));
        }
        length -= ab;
        let start = p;
        p += 1;
        let d = s[p] as u32;
        if d & 0xc0 != 0x80 {
            return Err((-8, start));
        }
        match ab {
            1 => {
                if c & 0x3e == 0 {
                    return Err((-17, start));
                }
            }
            2 => {
                p += 1;
                if s[p] & 0xc0 != 0x80 {
                    return Err((-9, start));
                }
                if c == 0xe0 && d & 0x20 == 0 {
                    return Err((-18, start));
                }
                if c == 0xed && d >= 0xa0 {
                    return Err((-16, start));
                }
            }
            3 => {
                p += 1;
                if s[p] & 0xc0 != 0x80 {
                    return Err((-9, start));
                }
                p += 1;
                if s[p] & 0xc0 != 0x80 {
                    return Err((-10, start));
                }
                if c == 0xf0 && d & 0x30 == 0 {
                    return Err((-19, start));
                }
                if c > 0xf4 || (c == 0xf4 && d > 0x8f) {
                    return Err((-15, start));
                }
            }
            4 => {
                for (k, code) in [(1usize, -9), (2, -10), (3, -11)] {
                    if s[start + 1 + k] & 0xc0 != 0x80 {
                        return Err((code, start));
                    }
                }
                p = start + 4;
                if c == 0xf8 && d & 0x38 == 0 {
                    return Err((-20, start));
                }
            }
            _ => {
                for (k, code) in [(1usize, -9), (2, -10), (3, -11), (4, -12)] {
                    if s[start + 1 + k] & 0xc0 != 0x80 {
                        return Err((code, start));
                    }
                }
                p = start + 5;
                if c == 0xfc && d & 0x3c == 0 {
                    return Err((-21, start));
                }
            }
        }
        if ab > 3 {
            return Err((if ab == 4 { -13 } else { -14 }, start));
        }
        p += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables() {
        assert_eq!(lcc(b'A' as u32), b'a' as u32);
        assert_eq!(fcc(b'z' as u32), b'Z' as u32);
        assert_eq!(fcc(0xc0), 0xc0);
        assert!(ctype(b'_' as u32) & CTYPE_WORD != 0);
        assert!(ctype(0x0b) & CTYPE_SPACE != 0);
        assert!(ctype(0xe9) == 0);
    }

    #[test]
    fn utf8_validation() {
        assert_eq!(valid_utf8("héllo€😀".as_bytes()), Ok(()));
        assert_eq!(valid_utf8(b"a\x80"), Err((-22, 1)));
        assert_eq!(valid_utf8(b"a\xff"), Err((-23, 1)));
        assert_eq!(valid_utf8(b"\xc3"), Err((-3, 0)));
        assert_eq!(valid_utf8(b"\xe2\x82"), Err((-3, 0)));
        assert_eq!(valid_utf8(b"\xf0\x9f"), Err((-4, 0)));
        assert_eq!(valid_utf8(b"\xc3\x28"), Err((-8, 0)));
        assert_eq!(valid_utf8(b"\xc0\x80"), Err((-17, 0)));
        assert_eq!(valid_utf8(b"\xed\xa0\x80"), Err((-16, 0)));
        assert_eq!(valid_utf8(b"\xf4\x90\x80\x80"), Err((-15, 0)));
        assert_eq!(valid_utf8(b"\xe0\x80\x80"), Err((-18, 0)));
        assert_eq!(valid_utf8(b"\xf8\x88\x80\x80\x80"), Err((-13, 0)));
    }

    #[test]
    fn utf8_roundtrip() {
        for c in [0x41u32, 0xe9, 0x20ac, 0x1f600, 0x10ffff] {
            let mut v = Vec::new();
            encode_utf8(c, &mut v);
            assert_eq!(v.len(), utf8_len(c));
            assert_eq!(decode_utf8(&v, 0), (c, v.len()));
        }
    }
}

// ---------------------------------------------------------------------------
// Fast byte search (word-at-a-time), standing in for C's memchr().
// ---------------------------------------------------------------------------

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

#[inline]
fn has_zero(v: u64) -> bool {
    v.wrapping_sub(LO) & !v & HI != 0
}

/// Position of the first `n` in `hay`.
pub(crate) fn memchr(n: u8, hay: &[u8]) -> Option<usize> {
    let rep = LO.wrapping_mul(n as u64);
    let mut i = 0;
    let len = hay.len();
    while i + 8 <= len {
        let w = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap());
        if has_zero(w ^ rep) {
            break;
        }
        i += 8;
    }
    hay[i..].iter().position(|&b| b == n).map(|p| p + i)
}

/// Position of the first `a` or `b` in `hay`.
pub(crate) fn memchr2(a: u8, b: u8, hay: &[u8]) -> Option<usize> {
    let ra = LO.wrapping_mul(a as u64);
    let rb = LO.wrapping_mul(b as u64);
    let mut i = 0;
    let len = hay.len();
    while i + 8 <= len {
        let w = u64::from_le_bytes(hay[i..i + 8].try_into().unwrap());
        if has_zero(w ^ ra) || has_zero(w ^ rb) {
            break;
        }
        i += 8;
    }
    hay[i..]
        .iter()
        .position(|&c| c == a || c == b)
        .map(|p| p + i)
}

#[cfg(test)]
mod memchr_tests {
    use super::*;

    #[test]
    fn search() {
        let h = b"hello world, this is a longer haystack with zebras";
        for n in *b"hzqsa" {
            assert_eq!(memchr(n, h), h.iter().position(|&c| c == n));
        }
        assert_eq!(memchr2(b'z', b'w', h), Some(6));
        assert_eq!(memchr2(b'Q', b'q', h), None);
        assert_eq!(memchr(0x80, &[0u8, 1, 0x80, 0xff]), Some(2));
    }
}
