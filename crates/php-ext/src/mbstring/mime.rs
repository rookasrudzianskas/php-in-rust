//! MIME header encoding and decoding (RFC 2047) from `mbstring.c`:
//! `mb_mime_header_encode`, `mb_mime_header_decode` and their functions.

use php_runtime::{Args, PResult, PhpStr, Value, Vm};

use super::globals::{get_encoding, globals};
use super::mbfl::convert::ConvertBuf;
use super::mbfl::encoding::{Encoding, ILLEGAL_MODE_CHAR, MIN_WCHAR_BUFSIZE, NoEncoding, enc};
use super::mbfl::language::no2language;
use super::tables::unicode_prop::MIME_CHAR_NEEDS_QENCODE;

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn needs_qencode(c: u8) -> bool {
    c > 0x7F || c == b'=' || MIME_CHAR_NEEDS_QENCODE[c as usize] != 0
}

/// `transfer_encoded_size`.
fn transfer_encoded_size(tmp: &ConvertBuf, base64: bool) -> usize {
    if base64 {
        tmp.len().div_ceil(3) * 4
    } else {
        tmp.out.iter().map(|&c| if needs_qencode(c) { 3 } else { 1 }).sum()
    }
}

/// `transfer_encode_mime_bytes`: moves `tmp`'s bytes to `out` as Base64 or
/// Q-encoding.
fn transfer_encode_mime_bytes(tmp: &mut ConvertBuf, out: &mut ConvertBuf, base64: bool) {
    let p = &tmp.out;
    if base64 {
        let mut chunks = p.chunks_exact(3);
        for c in &mut chunks {
            let bits = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32;
            out.add4(
                BASE64_TABLE[((bits >> 18) & 0x3F) as usize],
                BASE64_TABLE[((bits >> 12) & 0x3F) as usize],
                BASE64_TABLE[((bits >> 6) & 0x3F) as usize],
                BASE64_TABLE[(bits & 0x3F) as usize],
            );
        }
        match chunks.remainder() {
            [a] => {
                let bits = *a as u32;
                out.add4(
                    BASE64_TABLE[((bits >> 2) & 0x3F) as usize],
                    BASE64_TABLE[((bits & 0x3) << 4) as usize],
                    b'=',
                    b'=',
                );
            }
            [a, b] => {
                let bits = ((*a as u32) << 8) | *b as u32;
                out.add4(
                    BASE64_TABLE[((bits >> 10) & 0x3F) as usize],
                    BASE64_TABLE[((bits >> 4) & 0x3F) as usize],
                    BASE64_TABLE[((bits & 0xF) << 2) as usize],
                    b'=',
                );
            }
            _ => {}
        }
    } else {
        for &c in p {
            if needs_qencode(c) {
                out.add3(b'=', HEX[(c >> 4) as usize], HEX[(c & 0xF) as usize]);
            } else {
                out.add(c);
            }
        }
    }
    tmp.reset(0);
}

const HEADER_ENC_WCHAR_BUFSIZE: usize = 90;

/// `mb_mime_header_encode`.
pub fn mime_header_encode(
    input: &[u8],
    incode: &'static Encoding,
    outcode: &'static Encoding,
    base64: bool,
    linefeed: &[u8],
    indent: i64,
) -> Vec<u8> {
    let mime_name = outcode.mime_name.unwrap_or("").as_bytes();
    if input.is_empty() {
        return Vec::new();
    }
    let mut indent = if !(0..74).contains(&indent) { 0 } else { indent as usize };
    let mut linefeed = &linefeed[..linefeed.len().min(8)];
    // Legacy behaviour: the line feed ends at an embedded NUL.
    if let Some(i) = linefeed.iter().position(|&c| c == 0) {
        linefeed = &linefeed[..i];
    }

    let mut state = 0u32;
    // Big enough that a full buffer always holds enough for a whole line.
    let mut wchar_buf = [0u32; HEADER_ENC_WCHAR_BUFSIZE];
    let mut inp = input;

    // ASCII without spaces (other than leading ones) passes through.
    let mut checking_leading_spaces = true;
    let mut passthrough = true;
    'check: while !inp.is_empty() {
        let n = (incode.to_wchar)(&mut inp, &mut wchar_buf, &mut state);
        for &w in &wchar_buf[..n] {
            if checking_leading_spaces {
                if w == b' ' as u32 {
                    continue;
                }
                checking_leading_spaces = false;
            }
            if !(0x21..=0x7E).contains(&w) || w == b'=' as u32 || w == b'?' as u32 || w == b'_' as u32 {
                passthrough = false;
                break 'check;
            }
        }
    }
    if passthrough {
        return input.to_vec();
    }
    // Start again (php-src keeps the decoder state from the first pass).
    inp = input;

    let mut buf = ConvertBuf::new(inp.len(), b'?' as u32, ILLEGAL_MODE_CHAR);
    let mut offset = 0usize;
    let mut line_start = 0usize;
    let push_linefeed = |buf: &mut ConvertBuf| {
        buf.append(linefeed);
        buf.add(b' ');
    };

    // Encode a prefix as plain ASCII; once Base64/QPrint is needed, it is
    // used to the end of the string.
    let mut p;
    let mut e;
    'ascii: loop {
        if inp.is_empty() {
            // Only reachable with an empty `in` after a full ASCII pass.
            return buf.into_bytes();
        }
        let n = (incode.to_wchar)(&mut inp, &mut wchar_buf[offset..], &mut state);
        p = 0usize;
        e = offset + n;
        // Output is split into space-delimited 'words'; a non-ASCII
        // character in a word makes the whole word encoded.
        let mut word_start = p;
        while p < e && wchar_buf[p] == b' ' as u32 && (p - word_start) <= 74 {
            p += 1;
        }
        let mut need_mime = false;
        while p < e {
            let w = wchar_buf[p];
            p += 1;
            if w < 0x20 || w > 0x7E || w == b'?' as u32 || w == b'=' as u32 || w == b'_' as u32
                || (w == b' ' as u32 && (p - word_start) > 74)
            {
                need_mime = true;
                break;
            } else if w == b' ' as u32 {
                // Insert a line break here if the line got too long.
                if buf.len() - line_start + (p - word_start) + indent > 75 {
                    push_linefeed(&mut buf);
                    indent = 0;
                    line_start = buf.len();
                } else if buf.len() > 0 {
                    buf.add(b' ');
                }
                // Output one space-delimited word as plain ASCII.
                while word_start < p - 1 {
                    buf.add((wchar_buf[word_start] & 0xFF) as u8);
                    word_start += 1;
                }
                word_start += 1;
                while p < e && wchar_buf[p] == b' ' as u32 {
                    p += 1;
                }
            }
        }
        if !need_mime {
            if !inp.is_empty() {
                // Reprocess an incomplete trailing word with more input,
                // unless it fills the buffer (which could loop forever): then
                // fall through to MIME encoding.
                if word_start >= MIN_WCHAR_BUFSIZE {
                    offset = e - word_start;
                    wchar_buf.copy_within(word_start..e, 0);
                    continue 'ascii;
                }
            } else {
                // End of input in ASCII mode: trailing chars not followed by
                // a space.
                if word_start < e && buf.len() > 0 {
                    if buf.len() - line_start + (p - word_start) + indent > 74 {
                        push_linefeed(&mut buf);
                    } else {
                        buf.add(b' ');
                    }
                }
                while word_start < e {
                    buf.add((wchar_buf[word_start] & 0xFF) as u8);
                    word_start += 1;
                }
                return buf.into_bytes();
            }
        }
        // feed_and_mime_encode: add a line feed now if the encoded word
        // cannot start on this line.
        if buf.len() - line_start + indent + mime_name.len() > 55 {
            push_linefeed(&mut buf);
            indent = 0;
            line_start = buf.len();
        } else if buf.len() > 0 {
            buf.add(b' ');
        }
        p = word_start;
        break 'ascii;
    }

    // mime_encoding_needed: convert line by line to the output encoding,
    // then transfer-encode.
    let mut tmpbuf = ConvertBuf::new(inp.len(), b'?' as u32, ILLEGAL_MODE_CHAR);
    offset = e - p;
    let mut need_refill = true;
    if HEADER_ENC_WCHAR_BUFSIZE - offset < MIN_WCHAR_BUFSIZE {
        need_refill = false;
    } else {
        wchar_buf.copy_within(p..e, 0);
    }

    loop {
        if need_refill {
            let n = (incode.to_wchar)(&mut inp, &mut wchar_buf[offset..], &mut state);
            p = 0;
            e = offset + n;
        }
        need_refill = true;

        // start_new_line
        buf.add2(b'=', b'?');
        buf.append(mime_name);
        buf.add3(b'?', if base64 { b'B' } else { b'Q' }, b'?');

        // 'Binary search' for the most wchars which fit on this line.
        let mut n = 12usize;
        let space_available = 73usize
            .wrapping_sub(indent)
            .wrapping_sub(buf.len() - line_start);

        loop {
            let tmppos = tmpbuf.len();
            let tmpstate = tmpbuf.state;
            n = n.min(e - p);
            (outcode.from_wchar)(&wchar_buf[p..p + n], &mut tmpbuf, false);
            // Some encodings emit ending bytes before a line break.
            let tmppos2 = tmpbuf.len();
            let tmpstate2 = tmpbuf.state;
            (outcode.from_wchar)(&[], &mut tmpbuf, true);

            if transfer_encoded_size(&tmpbuf, base64) <= space_available || (n == 1 && tmppos == 0) {
                p += n;
                if p == e {
                    transfer_encode_mime_bytes(&mut tmpbuf, &mut buf, base64);
                    buf.add2(b'?', b'=');
                    return buf.into_bytes();
                }
                tmpbuf.reset(tmppos2);
                tmpbuf.state = tmpstate2;
            } else {
                tmpbuf.reset(tmppos);
                tmpbuf.state = tmpstate;
                if n == 1 {
                    // Exactly the number of chars which fit; finish the line.
                    (outcode.from_wchar)(&[], &mut tmpbuf, true);
                    transfer_encode_mime_bytes(&mut tmpbuf, &mut buf, base64);
                    tmpbuf.state = 0;
                    buf.add2(b'?', b'=');
                    indent = 0; // only affects the first line
                    if !inp.is_empty() || p < e {
                        push_linefeed(&mut buf);
                        line_start = buf.len();
                        offset = e - p;
                        if !inp.is_empty() && HEADER_ENC_WCHAR_BUFSIZE - offset >= MIN_WCHAR_BUFSIZE {
                            wchar_buf.copy_within(p..e, 0);
                            break; // refill
                        }
                        need_refill = false;
                        break; // start_new_line without refilling
                    }
                    return buf.into_bytes();
                }
                n = (n >> 1).max(1);
            }
        }
    }
}

pub fn mb_encode_mimeheader(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let charset;
    let mut base64 = true;
    if let Some(name) = args.str_opt(1).cloned() {
        let c = get_encoding(vm, Some(&name), 2)?;
        if c.mime_name.is_none_or(str::is_empty) || c.is(NoEncoding::Qprint) {
            return Err(vm.argument_value_error(
                2,
                &format!(
                    "\"{}\" cannot be used for MIME header encoding",
                    super::globals::cstr_lossy(&name)
                ),
            ));
        }
        charset = c;
    } else {
        let lang = no2language(globals(vm)?.language);
        charset = enc(lang.mail_charset);
        let t = enc(lang.mail_header_encoding).name.as_bytes()[0];
        if t == b'Q' || t == b'q' {
            base64 = false;
        }
    }
    if let Some(t) = args.str_opt(2) {
        if let Some(&t) = t.first() {
            if t == b'Q' || t == b'q' {
                base64 = false;
            }
        }
    }
    let linefeed = args.str(3).clone();
    let indent = args.int(4);
    let incode = globals(vm)?.current_internal_encoding;
    Ok(Value::String(PhpStr::from_vec(mime_header_encode(
        &s, incode, charset, base64, &linefeed, indent,
    ))))
}

fn decode_base64(c: u8) -> i8 {
    match c {
        b'A'..=b'Z' => (c - b'A') as i8,
        b'a'..=b'z' => (c - b'a' + 26) as i8,
        b'0'..=b'9' => (c - b'0' + 52) as i8,
        b'+' => 62,
        b'/' => 63,
        _ => -1,
    }
}

fn qprint_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// `mime_header_decode_encoded_word`: returns the position after the word.
fn decode_encoded_word(
    s: &[u8],
    start: usize,
    outcode: &'static Encoding,
    outbuf: &mut ConvertBuf,
    state: &mut u32,
) -> Option<usize> {
    let mut e = s.len();
    if e - start < 6 {
        return None;
    }
    let mut p = start + 2;
    let charset_start = p;
    let charset_end = charset_start + s[charset_start..e].iter().position(|&c| c == b'?')?;
    let encoding = charset_end + 1;
    p = encoding + 1;
    if p >= e || s[p] != b'?' {
        return None;
    }
    p += 1;
    let charset = &s[charset_start..charset_end];
    let incode = super::mbfl::encoding::name2encoding_cstr(charset)?;

    if let Some(end) = super::strings::memnstr(s, p, b"?=", e) {
        e = end;
    } else if p < e && s[e - 1] == b'?' {
        // An unterminated word ending in '?' (legacy behaviour).
        e -= 1;
    }

    let mut bytes = Vec::with_capacity(e.saturating_sub(p));
    match s[encoding] {
        b'Q' | b'q' => {
            while p < e {
                let c = s[p];
                p += 1;
                if c == b'_' {
                    bytes.push(b' ');
                    continue;
                } else if c == b'=' && e - p >= 2 {
                    let c2 = s[p];
                    let c3 = s[p + 1];
                    p += 2;
                    if let (Some(h), Some(l)) = (qprint_value(c2), qprint_value(c3)) {
                        bytes.push((h << 4) | l);
                        continue;
                    } else if c2 == b'\r' {
                        if c3 != b'\n' {
                            p -= 1;
                        }
                        continue;
                    } else if c2 == b'\n' {
                        p -= 1;
                        continue;
                    }
                }
                bytes.push(c);
            }
        }
        b'B' | b'b' => {
            let (mut bits, mut cache) = (0u32, 0u32);
            while p < e {
                let c = s[p];
                p += 1;
                if matches!(c, b'\r' | b'\n' | b' ' | b'\t' | b'=') {
                    continue;
                }
                let d = decode_base64(c);
                if d == -1 {
                    bytes.push(b'?');
                    continue;
                }
                bits += 6;
                cache = (cache << 6) | (d as u32 & 0x3F);
                if bits == 24 {
                    bytes.push((cache >> 16) as u8);
                    bytes.push((cache >> 8) as u8);
                    bytes.push(cache as u8);
                    bits = 0;
                    cache = 0;
                }
            }
            if bits == 18 {
                bytes.push((cache >> 10) as u8);
                bytes.push((cache >> 2) as u8);
            } else if bits == 12 {
                bytes.push((cache >> 4) as u8);
            }
        }
        _ => return None,
    }

    let mut wchar_buf = [0u32; 128];
    let mut inp: &[u8] = &bytes;
    while !inp.is_empty() {
        let n = (incode.to_wchar)(&mut inp, &mut wchar_buf, state);
        (outcode.from_wchar)(&wchar_buf[..n], outbuf, false);
    }
    Some(e + 2)
}

fn is_ws(c: u8) -> bool {
    matches!(c, b'\n' | b'\r' | b'\t' | b' ')
}

/// `mb_mime_header_decode`.
pub fn mime_header_decode(s: &[u8], outcode: &'static Encoding) -> Vec<u8> {
    let e = s.len();
    let mut p = 0usize;
    let mut state = 0u32;
    let mut space_pending = false;
    let mut buf = ConvertBuf::new(s.len(), b'?' as u32, ILLEGAL_MODE_CHAR);
    let ascii = enc(NoEncoding::Ascii);

    while p < e {
        let c = s[p];
        if c == b'=' && s.get(p + 1) == Some(&b'?') && e - p >= 6 {
            // Does this look like an encoded word? Try to decode it.
            if let Some(i) = s[p + 2..].iter().position(|&c| c == b'?') {
                let incode_end = p + 2 + i;
                if e - incode_end >= 3 {
                    if let Some(next) = decode_encoded_word(s, p, outcode, &mut buf, &mut state) {
                        p = next;
                        // Collapse whitespace; it becomes a space only if no
                        // encoded word follows directly.
                        if p < e && is_ws(s[p]) {
                            while p < e && is_ws(s[p]) {
                                p += 1;
                            }
                            space_pending = true;
                        } else {
                            space_pending = false;
                        }
                        continue;
                    }
                }
            }
        }
        if space_pending {
            (outcode.from_wchar)(&[b' ' as u32], &mut buf, false);
            space_pending = false;
        }
        // A run of plain ASCII.
        if c != b'\n' && c != b'\r' {
            let mut end = p + 1;
            while end < e && s[end] != b'=' && s[end] != b'\n' && s[end] != b'\r' {
                end += 1;
            }
            let mut wchar_buf = [0u32; 128];
            let mut run = &s[p..end];
            while !run.is_empty() {
                let n = (ascii.to_wchar)(&mut run, &mut wchar_buf, &mut state);
                (outcode.from_wchar)(&wchar_buf[..n], &mut buf, false);
            }
            p = end;
        }
        // Collapse a line break and following whitespace into one space
        // (but not at the very end).
        if p < e && (s[p] == b'\n' || s[p] == b'\r') {
            while p < e && is_ws(s[p]) {
                p += 1;
            }
            if p < e {
                (outcode.from_wchar)(&[b' ' as u32], &mut buf, false);
            }
        }
    }
    (outcode.from_wchar)(&[], &mut buf, true);
    buf.into_bytes()
}

pub fn mb_decode_mimeheader(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    let outcode = globals(vm)?.current_internal_encoding;
    Ok(Value::String(PhpStr::from_vec(mime_header_decode(&s, outcode))))
}
