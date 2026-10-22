//! The string functions of `mbstring.c`: lengths, searching, substrings,
//! widths, case conversion, trimming, padding, validation, `mb_ord`/`mb_chr`.

use php_runtime::{Args, PResult, PhpArray, PhpStr, Value, Vm};

use super::globals::{get_encoding, globals, illegal_settings};
use super::mbfl::convert::{ConvertBuf, WCHAR_BUFSIZE, mb_fast_convert};
use super::mbfl::encoding::{
    BAD_INPUT, Encoding, ILLEGAL_MODE_BADUTF8, MIN_WCHAR_BUFSIZE, NoEncoding, enc,
};
use super::mbfl::strcut::mbfl_strcut;
use super::php_unicode::{CASE_MODE_MAX, CaseMode, convert_case};
use super::tables::eaw_table::{FIRST_DOUBLEWIDTH_CODEPOINT, MBFL_EAW_TABLE};

/// `MBFL_SUBSTR_UNTIL_END`.
pub const SUBSTR_UNTIL_END: usize = usize::MAX;

fn utf8() -> &'static Encoding {
    &super::filters::utf8::MBFL_ENCODING_UTF8
}

/// `mb_fast_check_utf8`: strict UTF-8 validity (no overlongs, surrogates
/// or codepoints above U+10FFFF) — the same rules as `std::str::from_utf8`.
pub fn is_valid_utf8(s: &[u8]) -> bool {
    std::str::from_utf8(s).is_ok()
}

/// `php_mb_check_encoding`.
pub fn check_encoding(input: &[u8], encoding: &'static Encoding) -> bool {
    if let Some(check) = encoding.check {
        return check(input);
    }
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp = input;
    // A wrongly encoded string usually shows in its first bytes, so check
    // just a few codepoints first.
    let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf[..8], &mut state);
    if wchar_buf[..n].contains(&BAD_INPUT) {
        return false;
    }
    while !inp.is_empty() {
        let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        if wchar_buf[..n].contains(&BAD_INPUT) {
            return false;
        }
    }
    true
}

/// `mb_check_str_encoding`.
pub fn check_str_encoding(s: &[u8], encoding: &'static Encoding) -> bool {
    if encoding.is(NoEncoding::Utf8) {
        is_valid_utf8(s)
    } else {
        check_encoding(s, encoding)
    }
}

/// `mb_fast_strlen_utf8`: byte length minus continuation bytes.
pub fn fast_strlen_utf8(s: &[u8]) -> usize {
    s.len() - s.iter().filter(|&&c| (c as i8) < -64).count()
}

/// `mb_get_strlen`.
pub fn get_strlen(s: &[u8], encoding: &'static Encoding) -> usize {
    let char_len = encoding.char_len();
    if char_len != 0 {
        return s.len() / char_len;
    }
    if encoding.is_utf8_family() && is_valid_utf8(s) {
        return fast_strlen_utf8(s);
    }
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp = s;
    let mut len = 0;
    while !inp.is_empty() {
        len += (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
    }
    len
}

/// `mb_get_substr_slow`.
pub fn get_substr_slow(
    input: &[u8],
    mut from: usize,
    mut len: usize,
    encoding: &'static Encoding,
    illegal: (u32, u32),
) -> Vec<u8> {
    let initial = if from > input.len() {
        0
    } else {
        len.min(input.len() - from)
    };
    let mut buf = ConvertBuf::new(initial, illegal.0, illegal.1);
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp = input;
    while !inp.is_empty() && len != 0 {
        let out_len = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        if from >= out_len {
            from -= out_len;
        } else {
            let needed = (out_len - from).min(len);
            (encoding.from_wchar)(
                &wchar_buf[from..from + needed],
                &mut buf,
                inp.is_empty() || out_len >= len,
            );
            from = 0;
            len -= needed;
        }
    }
    buf.into_bytes()
}

/// `mb_get_substr`: `from`/`len` count codepoints.
pub fn get_substr(
    input: &[u8],
    from: usize,
    len: usize,
    encoding: &'static Encoding,
    illegal: (u32, u32),
) -> Vec<u8> {
    // Other than MacJapanese, no encoding decodes one byte to several
    // codepoints, so skipping at least as many codepoints as there are bytes
    // leaves nothing.
    if len == 0 || (from >= input.len() && !encoding.is(NoEncoding::SjisMac)) {
        return Vec::new();
    }
    let flag = encoding.char_len();
    if flag != 0 {
        let from = from.wrapping_mul(flag);
        let mut len = len.wrapping_mul(flag);
        if from >= input.len() {
            return Vec::new();
        }
        let rest = &input[from..];
        if len > rest.len() {
            len = rest.len();
        }
        return rest[..len].to_vec();
    }
    get_substr_slow(input, from, len, encoding, illegal)
}

/// `offset_to_pointer_utf8`.
fn offset_to_pointer_utf8(s: &[u8], start: usize, offset: isize) -> Option<usize> {
    if offset < 0 {
        let mut pos = s.len();
        let mut offset = offset;
        while offset < 0 {
            if pos <= start {
                return None;
            }
            pos -= 1;
            let c = s[pos];
            if c < 0x80 || (c & 0xC0) != 0x80 {
                offset += 1;
            }
        }
        Some(pos)
    } else {
        let tbl = &super::filters::mblen::MBLEN_TABLE_UTF8;
        let mut pos = start;
        let mut offset = offset;
        while offset > 0 {
            offset -= 1;
            if pos >= s.len() {
                return None;
            }
            pos += tbl[s[pos] as usize] as usize;
        }
        Some(pos.min(s.len()))
    }
}

/// `zend_memnstr`: first occurrence of `needle` in `h[from..end]`.
pub fn memnstr(h: &[u8], from: usize, needle: &[u8], end: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    if end < from || end - from < needle.len() {
        return None;
    }
    h[from..end]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

/// `zend_memnrstr`: last occurrence of `needle` in `h[from..end]`.
pub fn memnrstr(h: &[u8], from: usize, needle: &[u8], end: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(end);
    }
    if end < from || end - from < needle.len() {
        return None;
    }
    h[from..end]
        .windows(needle.len())
        .rposition(|w| w == needle)
        .map(|i| from + i)
}

/// Errors of `mb_find_strpos` (`MBFL_ERROR_*`).
#[derive(Debug, PartialEq, Eq)]
pub enum StrposError {
    NotFound,
    Offset,
}

/// `mb_find_strpos`.
pub fn find_strpos(
    haystack: &[u8],
    needle: &[u8],
    encoding: &'static Encoding,
    offset: i64,
    reverse: bool,
) -> Result<usize, StrposError> {
    let (h_conv, n_conv);
    let (h, n): (&[u8], &[u8]) = if !encoding.is_utf8_family() {
        h_conv = mb_fast_convert(haystack, encoding, utf8(), 0, ILLEGAL_MODE_BADUTF8).0;
        n_conv = mb_fast_convert(needle, encoding, utf8(), 0, ILLEGAL_MODE_BADUTF8).0;
        (&h_conv, &n_conv)
    } else {
        (haystack, needle)
    };

    let offset = offset.clamp(isize::MIN as i64, isize::MAX as i64) as isize;
    let Some(offset_pointer) = offset_to_pointer_utf8(h, 0, offset) else {
        return Err(StrposError::Offset);
    };
    if h.len() < n.len() {
        return Err(StrposError::NotFound);
    }

    let found = if !reverse {
        memnstr(h, offset_pointer, n, h.len())
    } else if offset >= 0 {
        memnrstr(h, offset_pointer, n, h.len())
    } else {
        let needle_len = fast_strlen_utf8(n) as isize;
        let end = offset_to_pointer_utf8(h, offset_pointer, needle_len).unwrap_or(h.len());
        memnrstr(h, 0, n, end)
    };
    match found {
        Some(pos) => Ok(fast_strlen_utf8(&h[..pos])),
        None => Err(StrposError::NotFound),
    }
}

/// `php_mb_stripos`: simple case folding (offsets stay stable).
pub fn stripos(
    reverse: bool,
    haystack: &[u8],
    needle: &[u8],
    offset: i64,
    encoding: &'static Encoding,
) -> Result<usize, StrposError> {
    let h = convert_case(CaseMode::FoldSimple, haystack, encoding, utf8(), ILLEGAL_MODE_BADUTF8, 0);
    let n = convert_case(CaseMode::FoldSimple, needle, encoding, utf8(), ILLEGAL_MODE_BADUTF8, 0);
    find_strpos(&h.out, &n.out, utf8(), offset, reverse)
}

/// `handle_strpos_error`.
fn handle_strpos_error(vm: &mut Vm, e: StrposError) -> PResult<Value> {
    if e == StrposError::Offset {
        return Err(vm.argument_value_error(3, "must be contained in argument #1 ($haystack)"));
    }
    Ok(Value::Bool(false))
}

fn str_value(v: Vec<u8>) -> Value {
    Value::String(PhpStr::from_vec(v))
}

// ----- functions ------------------------------------------------------------

pub fn mb_str_split(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let mut split_len = args.int(1);
    if split_len <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    } else if split_len > (u32::MAX / 4) as i64 {
        return Err(vm.argument_value_error(2, "is too large"));
    }
    if split_len as usize > s.len() {
        split_len = s.len() as i64;
    }
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let mut out = PhpArray::new();
    if s.is_empty() {
        return Ok(Value::array(out));
    }
    let split_len = split_len as usize;
    let char_len = encoding.char_len();
    if char_len != 0 {
        let chunk_len = char_len * split_len;
        for chunk in s.chunks(chunk_len) {
            let _ = out.append(Value::from(chunk));
        }
    } else if let Some(mbtab) = encoding.mblen_table {
        let mut p = 0usize;
        while p < s.len() {
            let chunk = p;
            let mut char_count = 0;
            while char_count < split_len && p < s.len() {
                p += mbtab[s[p] as usize] as usize;
                char_count += 1;
            }
            let p_end = p.min(s.len());
            let _ = out.append(Value::from(&s[chunk..p_end]));
            p = p_end;
        }
    } else {
        let illegal = illegal_settings(vm)?;
        let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
        let mut state = 0u32;
        let mut inp: &[u8] = &s;
        let mut char_count = 0usize;
        let mut buf = ConvertBuf::new(split_len, illegal.0, illegal.1);
        while !inp.is_empty() {
            let out_len = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
            let mut i = 0usize;
            // Finish a chunk left incomplete by the previous iteration.
            if char_count != 0 {
                if out_len >= split_len - char_count {
                    (encoding.from_wchar)(&wchar_buf[..split_len - char_count], &mut buf, true);
                    i += split_len - char_count;
                    char_count = 0;
                    let done = std::mem::take(&mut buf);
                    let _ = out.append(str_value(done.into_bytes()));
                } else {
                    (encoding.from_wchar)(&wchar_buf[..out_len], &mut buf, inp.is_empty());
                    char_count += out_len;
                    continue;
                }
            }
            while i < out_len {
                buf = ConvertBuf::new(split_len, illegal.0, illegal.1);
                if out_len - i >= split_len {
                    (encoding.from_wchar)(&wchar_buf[i..i + split_len], &mut buf, true);
                    i += split_len;
                    let done = std::mem::take(&mut buf);
                    let _ = out.append(str_value(done.into_bytes()));
                } else {
                    (encoding.from_wchar)(&wchar_buf[i..out_len], &mut buf, inp.is_empty());
                    char_count = out_len - i;
                    break;
                }
            }
        }
        if char_count != 0 {
            let _ = out.append(str_value(buf.into_bytes()));
        }
    }
    Ok(Value::array(out))
}

pub fn mb_strlen(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    Ok(Value::Int(get_strlen(args.str(0), encoding) as i64))
}

fn strpos_common(vm: &mut Vm, args: &mut Args, reverse: bool, case_insensitive: bool) -> PResult<Value> {
    let offset = args.int(2);
    let encoding = get_encoding(vm, args.str_opt(3), 4)?;
    let (h, n) = (args.str(0), args.str(1));
    let r = if case_insensitive {
        stripos(reverse, h, n, offset, encoding)
    } else {
        find_strpos(h, n, encoding, offset, reverse)
    };
    match r {
        Ok(n) => Ok(Value::Int(n as i64)),
        Err(e) => handle_strpos_error(vm, e),
    }
}

pub fn mb_strpos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strpos_common(vm, args, false, false)
}

pub fn mb_strrpos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strpos_common(vm, args, true, false)
}

pub fn mb_stripos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strpos_common(vm, args, false, true)
}

pub fn mb_strripos(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strpos_common(vm, args, true, true)
}

/// `php_mb_strstr_variants`.
fn strstr_variants(vm: &mut Vm, args: &mut Args, reverse: bool, case_insensitive: bool) -> PResult<Value> {
    let part = args.bool(2);
    let encoding = get_encoding(vm, args.str_opt(3), 4)?;
    let (h, n) = (args.str(0).clone(), args.str(1).clone());
    let r = if case_insensitive {
        stripos(reverse, &h, &n, 0, encoding)
    } else {
        find_strpos(&h, &n, encoding, 0, reverse)
    };
    match r {
        Ok(n) => {
            let illegal = illegal_settings(vm)?;
            let out = if part {
                get_substr(&h, 0, n, encoding, illegal)
            } else {
                get_substr(&h, n, SUBSTR_UNTIL_END, encoding, illegal)
            };
            Ok(str_value(out))
        }
        Err(_) => Ok(Value::Bool(false)),
    }
}

pub fn mb_strstr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strstr_variants(vm, args, false, false)
}

pub fn mb_strrchr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strstr_variants(vm, args, true, false)
}

pub fn mb_stristr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strstr_variants(vm, args, false, true)
}

pub fn mb_strrichr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    strstr_variants(vm, args, true, true)
}

pub fn mb_substr_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (h, n) = (args.str(0).clone(), args.str(1).clone());
    if n.is_empty() {
        return Err(vm.argument_value_error(2, "must not be empty"));
    }
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let (h8, n8) = if encoding.is_utf8_family() {
        let conv = |s: &PhpStr| {
            if is_valid_utf8(s) {
                s.to_vec()
            } else {
                mb_fast_convert(s, encoding, utf8(), 0, ILLEGAL_MODE_BADUTF8).0
            }
        };
        (conv(&h), conv(&n))
    } else {
        let h8 = mb_fast_convert(&h, encoding, utf8(), 0, ILLEGAL_MODE_BADUTF8).0;
        let n8 = mb_fast_convert(&n, encoding, utf8(), 0, ILLEGAL_MODE_BADUTF8).0;
        // A non-empty string may decode to no codepoints (e.g. only escapes).
        if n8.is_empty() {
            return Err(vm.argument_value_error(2, "must not be empty"));
        }
        (h8, n8)
    };
    let mut result = 0i64;
    if h8.len() >= n8.len() {
        let mut p = 0usize;
        while let Some(pos) = memnstr(&h8, p, &n8, h8.len()) {
            p = pos + n8.len();
            result += 1;
        }
    }
    Ok(Value::Int(result))
}

pub fn mb_substr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let from = args.int(1);
    let len = args.int_opt(2);
    let range = format!("must be between {} and {}", i64::MIN + 1, i64::MAX);
    if from == i64::MIN {
        return Err(vm.argument_value_error(2, &range));
    }
    if len == Some(i64::MIN) {
        return Err(vm.argument_value_error(3, &range));
    }
    let encoding = get_encoding(vm, args.str_opt(3), 4)?;

    let mut mblen = 0usize;
    if from < 0 || len.is_some_and(|l| l < 0) {
        mblen = get_strlen(&s, encoding);
    }
    // A negative start counts from the end of the string.
    let real_from = if from >= 0 {
        from as usize
    } else if ((-from) as usize) < mblen {
        (mblen as i64 + from) as usize
    } else {
        0
    };
    // A negative length stops that many characters from the end.
    let real_len = match len {
        None => SUBSTR_UNTIL_END,
        Some(l) if l >= 0 => l as usize,
        Some(l) => {
            if real_from < mblen && ((-l) as usize) < mblen - real_from {
                ((mblen - real_from) as i64 + l) as usize
            } else {
                0
            }
        }
    };
    let illegal = illegal_settings(vm)?;
    Ok(str_value(get_substr(&s, real_from, real_len, encoding, illegal)))
}

pub fn mb_strcut(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let mut from = args.int(1);
    let len_opt = args.int_opt(2);
    let encoding = get_encoding(vm, args.str_opt(3), 4)?;
    let slen = s.len() as i64;
    let mut len = len_opt.unwrap_or(slen);

    // A negative start counts from the end of the string.
    if from < 0 {
        from += slen;
        if from < 0 {
            from = 0;
        }
    }
    // A negative length stops that many bytes from the end.
    if len < 0 {
        len = slen.wrapping_sub(from).wrapping_add(len);
        if len < 0 {
            len = 0;
        }
    }
    if from > slen || len == 0 {
        return Ok(Value::empty_string());
    }
    let (from, len) = (from as usize, len as usize);

    if let Some(cut) = encoding.cut {
        return Ok(str_value(cut(&s, from, len)));
    }

    let char_len = encoding.char_len();
    if char_len != 0 {
        // Round down to a multiple of the character width.
        let from = from & !(char_len - 1);
        let len = len.min(s.len() - from);
        return Ok(Value::from(&s[from..from + (len & !(char_len - 1))]));
    }

    if let Some(mbtab) = encoding.mblen_table {
        let mut p = 0usize;
        let mut m = 0usize;
        // Search the start position.
        while p < from {
            m = mbtab[s[p] as usize] as usize;
            p += m;
        }
        if p > from {
            p -= m;
        }
        let start = p;
        let end = if len >= s.len() - start {
            s.len()
        } else {
            let q = p + len;
            while p < q {
                m = mbtab[s[p] as usize] as usize;
                p += m;
            }
            if p > q {
                p -= m;
            }
            p
        };
        return Ok(Value::from(&s[start..end]));
    }

    match mbfl_strcut(&s, encoding, from, len) {
        Some(out) => Ok(str_value(out)),
        None => Ok(Value::empty_string()),
    }
}

/// `character_width`: 2 for East Asian fullwidth/wide characters.
pub fn character_width(c: u32) -> usize {
    if c < FIRST_DOUBLEWIDTH_CODEPOINT {
        return 1;
    }
    let (mut lo, mut hi) = (0usize, MBFL_EAW_TABLE.len());
    while lo < hi {
        let probe = (lo + hi) / 2;
        let [begin, end] = MBFL_EAW_TABLE[probe];
        if c < begin {
            hi = probe;
        } else if c > end {
            lo = probe + 1;
        } else {
            return 2;
        }
    }
    1
}

/// `mb_get_strwidth`: error markers count as width 1.
pub fn get_strwidth(s: &[u8], encoding: &'static Encoding) -> usize {
    let mut width = 0;
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp = s;
    while !inp.is_empty() {
        let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        width += wchar_buf[..n].iter().map(|&w| character_width(w)).sum::<usize>();
    }
    width
}

pub fn mb_strwidth(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    Ok(Value::Int(get_strwidth(args.str(0), encoding) as i64))
}

/// `mb_trim_string` (`mb_strimwidth`).
fn trim_string(
    input: &[u8],
    marker: &[u8],
    encoding: &'static Encoding,
    from: usize,
    width: usize,
    illegal: (u32, u32),
) -> Vec<u8> {
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut inp = input;
    let mut state = 0u32;
    let mut remaining_width = width;
    let mut to_skip = from;
    let mut first_call = true;
    let mut input_err = false;
    let mut width = width;
    // `Some(out_len)` when the first batch can be reused for the output.
    let mut truncate: Option<Option<usize>> = None;

    'scan: while !inp.is_empty() {
        let out_len = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        if out_len <= to_skip {
            to_skip -= out_len;
        } else {
            for &w in &wchar_buf[to_skip..out_len] {
                let cw = character_width(w);
                input_err |= w == BAD_INPUT;
                if remaining_width < cw {
                    let marker_width = get_strwidth(marker, encoding);
                    // The trim marker is wider than the requested width.
                    if width <= marker_width {
                        return marker.to_vec();
                    }
                    width -= marker_width;
                    truncate = Some(if first_call { Some(out_len) } else { None });
                    break 'scan;
                }
                remaining_width -= cw;
            }
            to_skip = 0;
        }
        first_call = false;
    }

    let Some(reuse) = truncate else {
        // The string fits; erroneous bytes must still become error markers.
        if !input_err {
            if from == 0 {
                return input.to_vec();
            }
            return get_substr(input, from, SUBSTR_UNTIL_END, encoding, illegal);
        }
        return get_substr_slow(input, from, SUBSTR_UNTIL_END, encoding, illegal);
    };

    let mut buf = ConvertBuf::new(width, illegal.0, illegal.1);
    let mut from = from;
    let mut out_len = match reuse {
        Some(n) => n,
        None => {
            inp = input;
            state = 0;
            (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state)
        }
    };
    loop {
        if out_len <= from {
            from -= out_len;
        } else {
            for i in from..out_len {
                let cw = character_width(wchar_buf[i]);
                if width < cw {
                    (encoding.from_wchar)(&wchar_buf[from..i], &mut buf, true);
                    buf.append(marker);
                    return buf.into_bytes();
                }
                width -= cw;
            }
            (encoding.from_wchar)(&wchar_buf[from..out_len], &mut buf, false);
            from = 0;
        }
        if inp.is_empty() {
            // Unreachable in php-src (the width limit is hit first).
            buf.append(marker);
            return buf.into_bytes();
        }
        out_len = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
    }
}

pub fn mb_strimwidth(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let mut from = args.int(1);
    let mut width = args.int(2);
    let marker = args.str(3).clone();
    let encoding = get_encoding(vm, args.str_opt(4), 5)?;

    if from != 0 {
        let str_len = get_strlen(&s, encoding) as i64;
        if from < 0 {
            from += str_len;
        }
        if from < 0 || from > str_len {
            return Err(vm.argument_value_error(2, "is out of range"));
        }
    }
    let illegal = illegal_settings(vm)?;
    if width < 0 {
        vm.deprecated("passing a negative integer to argument #3 ($width) is deprecated")?;
        width += get_strwidth(&s, encoding) as i64;
        if from > 0 {
            let trimmed = get_substr(&s, 0, from as usize, encoding, illegal);
            width -= get_strwidth(&trimmed, encoding) as i64;
        }
        if width < 0 {
            return Err(vm.argument_value_error(3, "is out of range"));
        }
    }
    Ok(str_value(trim_string(
        &s,
        &marker,
        encoding,
        from as usize,
        width as usize,
        illegal,
    )))
}

/// `mbstring_convert_case` with the current substitution settings.
pub fn mbstring_convert_case(vm: &mut Vm, mode: CaseMode, s: &[u8], encoding: &'static Encoding) -> PResult<Vec<u8>> {
    let (substchar, mode_illegal) = illegal_settings(vm)?;
    Ok(convert_case(mode, s, encoding, encoding, mode_illegal, substchar).into_bytes())
}

pub fn mb_convert_case(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mode = args.int(1);
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    if !(0..CASE_MODE_MAX).contains(&mode) {
        return Err(vm.argument_value_error(2, "must be one of the MB_CASE_* constants"));
    }
    let mode = CaseMode::from_i64(mode).expect("checked range");
    let s = args.str(0).clone();
    Ok(str_value(mbstring_convert_case(vm, mode, &s, encoding)?))
}

pub fn mb_strtoupper(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    let s = args.str(0).clone();
    Ok(str_value(mbstring_convert_case(vm, CaseMode::Upper, &s, encoding)?))
}

pub fn mb_strtolower(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    let s = args.str(0).clone();
    Ok(str_value(mbstring_convert_case(vm, CaseMode::Lower, &s, encoding)?))
}

/// `php_mb_ulcfirst`.
fn ulcfirst(vm: &mut Vm, args: &mut Args, mode: CaseMode) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    let s = args.str(0).clone();
    let illegal = illegal_settings(vm)?;
    let first = get_substr(&s, 0, 1, encoding, illegal);
    let head = mbstring_convert_case(vm, mode, &first, encoding)?;
    if first == head {
        return Ok(Value::String(s));
    }
    let mut out = head;
    out.extend(get_substr(&s, 1, SUBSTR_UNTIL_END, encoding, illegal));
    Ok(str_value(out))
}

pub fn mb_ucfirst(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    ulcfirst(vm, args, CaseMode::Title)
}

pub fn mb_lcfirst(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    ulcfirst(vm, args, CaseMode::Lower)
}

const MB_LTRIM: u8 = 1;
const MB_RTRIM: u8 = 2;
const MB_BOTH_TRIM: u8 = 3;

/// `trim_each_wchar`.
fn trim_each_wchar(
    s: &[u8],
    what: &[u32],
    mut mode: u8,
    encoding: &'static Encoding,
    illegal: (u32, u32),
) -> Option<Vec<u8>> {
    let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
    let mut inp = s;
    let mut state = 0u32;
    let (mut left, mut right, mut total_len) = (0usize, 0usize, 0usize);
    while !inp.is_empty() {
        let out_len = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        total_len += out_len;
        for w in &wchar_buf[..out_len] {
            if what.contains(w) {
                if mode & MB_LTRIM != 0 {
                    left += 1;
                }
                if mode & MB_RTRIM != 0 {
                    right += 1;
                }
            } else {
                mode &= !MB_LTRIM;
                if mode & MB_RTRIM != 0 {
                    right = 0;
                }
            }
        }
    }
    if left == 0 && right == 0 {
        return None;
    }
    // Everything trimmed: the length wraps around (size_t), selecting nothing.
    Some(get_substr(s, left, total_len.wrapping_sub(right + left), encoding, illegal))
}

const TRIM_DEFAULT_CHARS: [u32; 27] = [
    0x20, 0x0C, 0x0A, 0x0D, 0x09, 0x0B, 0x00, 0xA0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004,
    0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0x85,
    0x180E,
];

/// `php_do_mb_trim`.
fn do_trim(vm: &mut Vm, args: &mut Args, mode: u8) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(2), 3)?;
    let s = args.str(0).clone();
    let illegal = illegal_settings(vm)?;
    let what: Vec<u32> = match args.str_opt(1) {
        None => TRIM_DEFAULT_CHARS.to_vec(),
        Some(what) => {
            let mut chars = Vec::new();
            let mut wchar_buf = [0u32; WCHAR_BUFSIZE];
            let mut inp: &[u8] = what;
            let mut state = 0u32;
            let mut first = true;
            while !inp.is_empty() {
                let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
                // php-src uses the (at most 4) codepoints of a short first
                // batch directly and ignores the rest.
                if first && n <= 4 {
                    chars = wchar_buf[..n].to_vec();
                    break;
                }
                first = false;
                chars.extend_from_slice(&wchar_buf[..n]);
            }
            if what.is_empty() {
                return Ok(Value::String(s));
            }
            chars
        }
    };
    Ok(match trim_each_wchar(&s, &what, mode, encoding, illegal) {
        Some(v) => str_value(v),
        None => Value::String(s),
    })
}

pub fn mb_trim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, MB_BOTH_TRIM)
}

pub fn mb_ltrim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, MB_LTRIM)
}

pub fn mb_rtrim(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_trim(vm, args, MB_RTRIM)
}

/// `php_mb_check_encoding_recursive`.
fn check_encoding_recursive(
    vm: &mut Vm,
    arr: &PhpArray,
    encoding: &'static Encoding,
    stack: &mut Vec<*const PhpArray>,
) -> PResult<bool> {
    let ptr = arr as *const PhpArray;
    if stack.contains(&ptr) {
        vm.warning("Cannot not handle circular references")?;
        return Ok(false);
    }
    stack.push(ptr);
    let mut valid = true;
    for (key, v) in arr.iter() {
        if let php_runtime::ArrayKey::Str(k) = &key {
            if !check_str_encoding(k, encoding) {
                valid = false;
                break;
            }
        }
        let v = v.deref();
        let ok = match &*v {
            Value::String(s) => check_str_encoding(s, encoding),
            Value::Array(a) => check_encoding_recursive(vm, a, encoding, stack)?,
            Value::Int(_) | Value::Float(_) | Value::Null | Value::Bool(_) => true,
            _ => false,
        };
        if !ok {
            valid = false;
            break;
        }
    }
    stack.pop();
    Ok(valid)
}

pub fn mb_check_encoding(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    match args.get(0).clone() {
        Value::Array(a) => Ok(Value::Bool(check_encoding_recursive(vm, &a, encoding, &mut Vec::new())?)),
        Value::String(s) => Ok(Value::Bool(check_str_encoding(&s, encoding))),
        _ => {
            vm.deprecated("Calling mb_check_encoding() without argument is deprecated")?;
            Ok(Value::Bool(globals(vm)?.illegalchars == 0))
        }
    }
}

/// `php_mb_convert_encoding_ex`: converts with the current substitution
/// settings and counts errors into `MBSTRG(illegalchars)`.
pub fn convert_encoding_ex(
    vm: &mut Vm,
    input: &[u8],
    to: &'static Encoding,
    from: &'static Encoding,
) -> PResult<Vec<u8>> {
    let (substchar, mode) = illegal_settings(vm)?;
    let (out, errors) = mb_fast_convert(input, from, to, substchar, mode);
    globals(vm)?.illegalchars += errors as i64;
    Ok(out)
}

pub fn mb_scrub(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    let s = args.str(0).clone();
    if encoding.is(NoEncoding::Utf8) && is_valid_utf8(&s) {
        return Ok(Value::String(s));
    }
    Ok(str_value(convert_encoding_ex(vm, &s, encoding, encoding)?))
}

/// `php_mb_is_unsupported_no_encoding` (for `mb_ord`/`mb_chr`).
fn is_unsupported_no_encoding(no: NoEncoding) -> bool {
    use NoEncoding as E;
    (no >= E::Invalid && no <= E::Qprint)
        || (no >= E::Utf7 && no <= E::Utf7imap)
        || (no >= E::Jis && no <= E::Iso2022jpms)
        || (no >= E::Cp50220 && no <= E::Cp50222)
}

pub fn mb_ord(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    if s.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    if is_unsupported_no_encoding(encoding.no) {
        return Err(vm.throw_value_error(format!(
            "mb_ord() does not support the \"{}\" encoding",
            encoding.name
        )));
    }
    // Some legacy encodings need a minimum codepoint buffer size.
    let mut wchar_buf = [0u32; MIN_WCHAR_BUFSIZE];
    let mut state = 0u32;
    let mut inp: &[u8] = &s;
    let n = (encoding.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
    if n == 0 || wchar_buf[0] == BAD_INPUT {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Int(wchar_buf[0] as i64))
}

pub fn mb_chr(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cp = args.int(0);
    let encoding = get_encoding(vm, args.str_opt(1), 2)?;
    if is_unsupported_no_encoding(encoding.no) {
        return Err(vm.throw_value_error(format!(
            "mb_chr() does not support the \"{}\" encoding",
            encoding.name
        )));
    }
    if !(0..=0x10FFFF).contains(&cp) {
        return Ok(Value::Bool(false));
    }
    let cp = cp as u32;
    if encoding.is_utf8_family() {
        return Ok(match char::from_u32(cp) {
            Some(c) => Value::from(c.to_string()),
            None => Value::Bool(false),
        });
    }
    let bytes = cp.to_be_bytes();
    let orig = std::mem::replace(&mut globals(vm)?.illegalchars, 0);
    let out = convert_encoding_ex(vm, &bytes, encoding, enc(NoEncoding::Ucs4be))?;
    let g = globals(vm)?;
    let failed = g.illegalchars != 0;
    g.illegalchars = orig;
    Ok(if failed { Value::Bool(false) } else { str_value(out) })
}

/// `php_mb_str_pad_fill`.
fn pad_fill(out: &mut Vec<u8>, pad: &[u8], pad_bytes: usize) {
    if pad_bytes == 0 {
        return;
    }
    out.extend(pad.iter().cycle().take(pad_bytes));
}

pub fn mb_str_pad(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0).clone();
    let pad_to_length = args.int(1);
    let pad = args.str(2).clone();
    let pad_type = args.int(3);
    let encoding = get_encoding(vm, args.str_opt(4), 5)?;

    let input_length = get_strlen(&input, encoding);
    if pad_to_length < 0 || (pad_to_length as usize) <= input_length {
        return Ok(Value::String(input));
    }
    if pad.is_empty() {
        return Err(vm.argument_value_error(3, "must not be empty"));
    }
    // STR_PAD_LEFT = 0, STR_PAD_RIGHT = 1, STR_PAD_BOTH = 2
    if !(0..=2).contains(&pad_type) {
        return Err(vm.argument_value_error(4, "must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH"));
    }
    let pad_length = get_strlen(&pad, encoding);
    if pad_length == 0 {
        // Possible with an invalidly encoded padding string.
        return Err(vm.argument_value_error(3, "must not be empty"));
    }
    let num_pad_chars = pad_to_length as usize - input_length;
    let (left_pad, right_pad) = match pad_type {
        1 => (0, num_pad_chars),
        0 => (num_pad_chars, 0),
        _ => (num_pad_chars / 2, num_pad_chars - num_pad_chars / 2),
    };
    let full_left = left_pad / pad_length;
    let full_right = right_pad / pad_length;
    let (Some(left_bytes), Some(right_bytes)) =
        (full_left.checked_mul(pad.len()), full_right.checked_mul(pad.len()))
    else {
        return Err(vm.throw_error("Error", "String size overflow"));
    };
    let illegal = illegal_settings(vm)?;
    let rem_left = get_substr(&pad, 0, left_pad % pad_length, encoding, illegal);
    let rem_right = get_substr(&pad, 0, right_pad % pad_length, encoding, illegal);
    let total = left_bytes
        .checked_add(rem_left.len())
        .and_then(|t| t.checked_add(right_bytes))
        .and_then(|t| t.checked_add(rem_right.len()))
        .and_then(|t| t.checked_add(input.len()));
    let Some(total) = total.filter(|&t| t <= isize::MAX as usize) else {
        return Err(vm.throw_error("Error", "String size overflow"));
    };
    let mut out = Vec::with_capacity(total);
    pad_fill(&mut out, &pad, left_bytes);
    out.extend_from_slice(&rem_left);
    out.extend_from_slice(&input);
    pad_fill(&mut out, &pad, right_bytes);
    out.extend_from_slice(&rem_right);
    Ok(str_value(out))
}
