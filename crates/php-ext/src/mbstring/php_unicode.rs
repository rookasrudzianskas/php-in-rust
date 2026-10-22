//! `ext/mbstring/php_unicode.c`: Unicode properties and case conversion
//! (full and simple mappings, title case, case folding, the final-sigma
//! rule) over the tables of `unicode_data.h`.

use super::mbfl::convert::ConvertBuf;
use super::mbfl::encoding::{Encoding, NoEncoding};
use super::tables::unicode_data::*;

// Property numbers (`UC_*` in php_unicode.h).
pub const UC_MN: usize = 0;
pub const UC_MC: usize = 1;
pub const UC_ME: usize = 2;
pub const UC_ND: usize = 3;
pub const UC_NL: usize = 4;
pub const UC_NO: usize = 5;
pub const UC_ZS: usize = 6;
pub const UC_ZL: usize = 7;
pub const UC_ZP: usize = 8;
pub const UC_OS: usize = 9;
pub const UC_CO: usize = 10;
pub const UC_CN: usize = 11;
pub const UC_LU: usize = 12;
pub const UC_LL: usize = 13;
pub const UC_LT: usize = 14;
pub const UC_LM: usize = 15;
pub const UC_LO: usize = 16;
pub const UC_SM: usize = 17;
pub const UC_SC: usize = 18;
pub const UC_SK: usize = 19;
pub const UC_SO: usize = 20;
pub const UC_C: usize = 33;
pub const UC_P: usize = 34;
pub const UC_CASED: usize = 35;
pub const UC_CASE_IGNORABLE: usize = 36;

/// `php_case_mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMode {
    Upper = 0,
    Lower,
    Title,
    Fold,
    UpperSimple,
    LowerSimple,
    TitleSimple,
    FoldSimple,
}

/// `PHP_UNICODE_CASE_MODE_MAX`.
pub const CASE_MODE_MAX: i64 = 8;

impl CaseMode {
    pub fn from_i64(v: i64) -> Option<CaseMode> {
        Some(match v {
            0 => CaseMode::Upper,
            1 => CaseMode::Lower,
            2 => CaseMode::Title,
            3 => CaseMode::Fold,
            4 => CaseMode::UpperSimple,
            5 => CaseMode::LowerSimple,
            6 => CaseMode::TitleSimple,
            7 => CaseMode::FoldSimple,
            _ => return None,
        })
    }
}

/// `prop_lookup`: binary search in the range list of property `n`.
fn prop_lookup(code: u32, n: usize) -> bool {
    let mut l = _UCPROP_OFFSETS[n] as i64;
    let mut r = _UCPROP_OFFSETS[n + 1] as i64 - 1;
    while l <= r {
        // Make sure the mid point is at the beginning of a range pair.
        let mut m = (l + r) >> 1;
        m -= m & 1;
        if code > _UCPROP_RANGES[m as usize + 1] {
            l = m + 2;
        } else if code < _UCPROP_RANGES[m as usize] {
            r = m - 2;
        } else {
            return true;
        }
    }
    false
}

/// `php_unicode_is_prop1`.
pub fn is_prop1(code: u32, prop: usize) -> bool {
    prop_lookup(code, prop)
}

/// `php_unicode_is_prop`: any of `props`.
pub fn is_prop(code: u32, props: &[usize]) -> bool {
    props.iter().any(|&p| prop_lookup(code, p))
}

/// `php_unicode_is_cased`.
pub fn is_cased(code: u32) -> bool {
    prop_lookup(code, UC_CASED)
}

/// `php_unicode_is_case_ignorable`.
pub fn is_case_ignorable(code: u32) -> bool {
    prop_lookup(code, UC_CASE_IGNORABLE)
}

#[inline]
fn mph_hash(d: u32, x: u32) -> u32 {
    let x = x ^ d;
    ((x >> 16) ^ x).wrapping_mul(0x45d9f3b)
}

const CODE_NOT_FOUND: u32 = u32::MAX;

/// `mph_lookup`: minimal perfect hash lookup in a case mapping table.
fn mph_lookup(
    code: u32,
    g_table: &[i16],
    g_table_size: u32,
    table: &[u32],
    table_size: u32,
) -> u32 {
    let g = g_table[(mph_hash(0, code) % g_table_size) as usize];
    let idx = if g <= 0 {
        (-(g as i32)) as u32
    } else {
        mph_hash(g as u32, code) % table_size
    };
    if table[2 * idx as usize] == code {
        table[2 * idx as usize + 1]
    } else {
        CODE_NOT_FOUND
    }
}

fn lookup_upper(code: u32) -> u32 {
    mph_lookup(
        code,
        &_UCCASE_UPPER_G,
        _UCCASE_UPPER_G_SIZE,
        &_UCCASE_UPPER_TABLE,
        _UCCASE_UPPER_TABLE_SIZE,
    )
}
fn lookup_lower(code: u32) -> u32 {
    mph_lookup(
        code,
        &_UCCASE_LOWER_G,
        _UCCASE_LOWER_G_SIZE,
        &_UCCASE_LOWER_TABLE,
        _UCCASE_LOWER_TABLE_SIZE,
    )
}
fn lookup_title(code: u32) -> u32 {
    mph_lookup(
        code,
        &_UCCASE_TITLE_G,
        _UCCASE_TITLE_G_SIZE,
        &_UCCASE_TITLE_TABLE,
        _UCCASE_TITLE_TABLE_SIZE,
    )
}
fn lookup_fold(code: u32) -> u32 {
    mph_lookup(
        code,
        &_UCCASE_FOLD_G,
        _UCCASE_FOLD_G_SIZE,
        &_UCCASE_FOLD_TABLE,
        _UCCASE_FOLD_TABLE_SIZE,
    )
}

#[inline]
fn is_turkish(enc: &Encoding) -> bool {
    enc.is(NoEncoding::Iso8859_9)
}

/// `php_unicode_toupper_raw`: values > 0xFFFFFF index the special casing
/// table.
fn toupper_raw(code: u32, enc: &Encoding) -> u32 {
    // After ASCII, the first codepoint with an uppercase version is U+00B5.
    if code < 0xB5 {
        if (0x61..=0x7A).contains(&code) {
            if is_turkish(enc) && code == 0x69 {
                return 0x130;
            }
            return code - 0x20;
        }
        code
    } else {
        match lookup_upper(code) {
            CODE_NOT_FOUND => code,
            c => c,
        }
    }
}

/// `php_unicode_tolower_raw`.
fn tolower_raw(code: u32, enc: &Encoding) -> u32 {
    // After ASCII, the first codepoint with a lowercase version is U+00C0.
    if code < 0xC0 {
        if (0x41..=0x5A).contains(&code) {
            if is_turkish(enc) && code == 0x49 {
                return 0x131;
            }
            return code + 0x20;
        }
        code
    } else {
        match lookup_lower(code) {
            CODE_NOT_FOUND => code,
            c => {
                if is_turkish(enc) && code == 0x130 {
                    return 0x69;
                }
                c
            }
        }
    }
}

/// `php_unicode_totitle_raw`.
fn totitle_raw(code: u32, enc: &Encoding) -> u32 {
    match lookup_title(code) {
        CODE_NOT_FOUND => toupper_raw(code, enc),
        c => c,
    }
}

/// `php_unicode_tofold_raw`.
fn tofold_raw(code: u32, enc: &Encoding) -> u32 {
    if code < 0xB5 {
        if (0x41..=0x5A).contains(&code) {
            if is_turkish(enc) && code == 0x49 {
                return 0x131;
            }
            return code + 0x20;
        }
        code
    } else {
        match lookup_fold(code) {
            CODE_NOT_FOUND => code,
            c => {
                if is_turkish(enc) && code == 0x130 {
                    return 0x69;
                }
                c
            }
        }
    }
}

#[inline]
fn simple(code: u32) -> u32 {
    if code > 0xFFFFFF {
        _UCCASE_EXTRA_TABLE[(code & 0xFFFFFF) as usize]
    } else {
        code
    }
}

/// `emit_special_casing_sequence`.
fn emit_special(w: u32, out: &mut [u32], p: &mut usize) {
    let len = w >> 24;
    let base = (w & 0xFFFFFF) as usize;
    for i in 1..=len as usize {
        out[*p] = _UCCASE_EXTRA_TABLE[base + i];
        *p += 1;
    }
}

/// `scan_ahead_for_cased_letter`: decodes the rest of the input.
fn scan_ahead_for_cased_letter(mut input: &[u8], mut state: u32, encoding: &Encoding) -> bool {
    let mut wchar_buf = [0u32; 64];
    while !input.is_empty() {
        let out_len = (encoding.to_wchar)(&mut input, &mut wchar_buf, &mut state);
        for &w in &wchar_buf[..out_len] {
            if is_cased(w) {
                return true;
            }
            if !is_case_ignorable(w) {
                return false;
            }
        }
    }
    false
}

/// `scan_back_for_cased_letter` over leftovers of the previous batch.
fn scan_back_for_cased_letter(slice: &[u32]) -> bool {
    for &w in slice.iter().rev() {
        if is_cased(w) {
            return true;
        }
        if !is_case_ignorable(w) {
            return false;
        }
    }
    false
}

/// The final-sigma rule (Unicode SpecialCasing): U+03A3 lowercases to
/// U+03C2 when preceded by a cased letter and not followed by one (ignoring
/// case-ignorable codepoints). Like php-src, scanning back only looks at
/// the current batch and the not yet overwritten part of the previous one.
#[allow(clippy::too_many_arguments)]
fn is_final_sigma(
    wchar_buf: &[u32],
    i: usize,
    converted_buf: &[u32],
    p: usize,
    converted_end: Option<usize>,
    input: &[u8],
    state: u32,
    enc: &Encoding,
) -> bool {
    let mut j = i as isize - 1;
    while j >= 0 && is_case_ignorable(wchar_buf[j as usize]) {
        j -= 1;
    }
    let preceded = if j >= 0 {
        is_cased(wchar_buf[j as usize])
    } else {
        match converted_end {
            Some(end) if end > p => scan_back_for_cased_letter(&converted_buf[p..end]),
            _ => false,
        }
    };
    if !preceded {
        return false;
    }
    let mut j = i + 1;
    while j < wchar_buf.len() && is_case_ignorable(wchar_buf[j]) {
        j += 1;
    }
    if j >= wchar_buf.len() {
        !scan_ahead_for_cased_letter(input, state, enc)
    } else {
        !is_cased(wchar_buf[j])
    }
}

/// `php_unicode_convert_case`.
pub fn convert_case(
    case_mode: CaseMode,
    src: &[u8],
    src_encoding: &'static Encoding,
    dst_encoding: &'static Encoding,
    illegal_mode: u32,
    illegal_substchar: u32,
) -> ConvertBuf {
    // A codepoint can expand to up to 3 codepoints (SpecialCasing.txt).
    let mut wchar_buf = [0u32; 64];
    let mut converted_buf = [0u32; 192];
    let mut state = 0u32;
    let mut title_mode = false;
    let mut input = src;
    let mut converted_end: Option<usize> = None;

    let mut buf = ConvertBuf::new(src.len() + 1, illegal_substchar, illegal_mode);
    let enc = src_encoding;

    while !input.is_empty() {
        let out_len = (enc.to_wchar)(&mut input, &mut wchar_buf, &mut state);
        let wchars = &wchar_buf[..out_len];
        let mut p = 0usize;

        // Invalid characters (> 0xFFFFFF) pass through untouched.
        match case_mode {
            CaseMode::UpperSimple => {
                for &w in wchars {
                    converted_buf[p] = if w > 0xFFFFFF {
                        w
                    } else {
                        simple(toupper_raw(w, enc))
                    };
                    p += 1;
                }
            }
            CaseMode::LowerSimple => {
                for &w in wchars {
                    converted_buf[p] = if w > 0xFFFFFF {
                        w
                    } else {
                        simple(tolower_raw(w, enc))
                    };
                    p += 1;
                }
            }
            CaseMode::FoldSimple => {
                for &w in wchars {
                    converted_buf[p] = if w > 0xFFFFFF {
                        w
                    } else {
                        simple(tofold_raw(w, enc))
                    };
                    p += 1;
                }
            }
            CaseMode::TitleSimple => {
                for &w in wchars {
                    if w > 0xFFFFFF {
                        converted_buf[p] = w;
                        p += 1;
                        continue;
                    }
                    converted_buf[p] = if title_mode {
                        simple(tolower_raw(w, enc))
                    } else {
                        simple(totitle_raw(w, enc))
                    };
                    p += 1;
                    if !is_case_ignorable(w) {
                        title_mode = is_cased(w);
                    }
                }
            }
            CaseMode::Upper | CaseMode::Fold => {
                for &w in wchars {
                    if w > 0xFFFFFF {
                        converted_buf[p] = w;
                        p += 1;
                        continue;
                    }
                    let w = if case_mode == CaseMode::Upper {
                        toupper_raw(w, enc)
                    } else {
                        tofold_raw(w, enc)
                    };
                    if w > 0xFFFFFF {
                        emit_special(w, &mut converted_buf, &mut p);
                    } else {
                        converted_buf[p] = w;
                        p += 1;
                    }
                }
            }
            CaseMode::Lower => {
                for (i, &w) in wchars.iter().enumerate() {
                    if w > 0xFFFFFF {
                        converted_buf[p] = w;
                        p += 1;
                        continue;
                    }
                    if w == 0x3A3
                        && is_final_sigma(
                            wchars,
                            i,
                            &converted_buf,
                            p,
                            converted_end,
                            input,
                            state,
                            enc,
                        )
                    {
                        converted_buf[p] = 0x3C2;
                        p += 1;
                        continue;
                    }
                    let w = tolower_raw(w, enc);
                    if w > 0xFFFFFF {
                        emit_special(w, &mut converted_buf, &mut p);
                    } else {
                        converted_buf[p] = w;
                        p += 1;
                    }
                }
            }
            CaseMode::Title => {
                for (i, &w) in wchars.iter().enumerate() {
                    if w > 0xFFFFFF {
                        converted_buf[p] = w;
                        p += 1;
                        continue;
                    }
                    if title_mode
                        && w == 0x3A3
                        && is_final_sigma(
                            wchars,
                            i,
                            &converted_buf,
                            p,
                            converted_end,
                            input,
                            state,
                            enc,
                        )
                    {
                        converted_buf[p] = 0x3C2;
                        p += 1;
                    } else {
                        let w2 = if title_mode {
                            tolower_raw(w, enc)
                        } else {
                            totitle_raw(w, enc)
                        };
                        if w2 > 0xFFFFFF {
                            emit_special(w2, &mut converted_buf, &mut p);
                        } else {
                            converted_buf[p] = w2;
                            p += 1;
                        }
                    }
                    if !is_case_ignorable(w) {
                        title_mode = is_cased(w);
                    }
                }
            }
        }

        converted_end = Some(p);
        (dst_encoding.from_wchar)(&converted_buf[..p], &mut buf, input.is_empty());
    }

    buf
}
