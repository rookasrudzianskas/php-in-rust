//! `mb_send_mail` (`mbstring.c`): MIME-encodes the subject and body in the
//! language's mail charset and hands the message to `php_mail()`.

use std::collections::HashMap;

use php_runtime::{Args, PResult, Value, Vm};

use super::conversion::guess_encoding;
use super::globals::{globals, strict_detection};
use super::mbfl::convert::mb_fast_convert;
use super::mbfl::encoding::{
    Encoding, ILLEGAL_MODE_CHAR, NoEncoding, enc, name2encoding_cstr, preferred_mime_name,
};
use super::mbfl::language::no2language;
use super::mime::mime_header_encode;

/// `_php_mbstr_parse_mail_headers`: header names (lowercased) to values.
fn parse_mail_headers(s: &[u8]) -> HashMap<Vec<u8>, Vec<u8>> {
    let mut ht = HashMap::new();
    let mut state = 0;
    let mut crlf_state: i32 = -1;
    let mut token: Option<usize> = None;
    let mut token_pos = 0usize;
    let mut fld_name: Option<Vec<u8>> = None;
    let mut fld_val: Option<Vec<u8>> = None;

    let mut store = |name: &mut Option<Vec<u8>>, val: &mut Option<Vec<u8>>| {
        if let Some(n) = name.take() {
            if let Some(v) = val.take() {
                ht.insert(n.to_ascii_lowercase(), v);
            }
        }
        *val = None;
    };

    let mut i = 0usize;
    while i < s.len() {
        match s[i] {
            b':' => {
                if crlf_state == 1 {
                    token_pos += 1;
                }
                if state == 0 || state == 1 {
                    if let Some(t) = token {
                        if token_pos > 0 {
                            fld_name = Some(s[t..t + token_pos].to_vec());
                        }
                    }
                    state = 2;
                } else {
                    token_pos += 1;
                }
                crlf_state = 0;
            }
            b'\n' => {
                if crlf_state == -1 {
                    break;
                }
                crlf_state = -1;
            }
            b'\r' => {
                if crlf_state == 1 {
                    token_pos += 1;
                } else {
                    crlf_state = 1;
                }
            }
            b' ' | b'\t' => {
                if crlf_state == -1 {
                    // A continuation of the previous line, or a skipped one.
                    state = if state == 3 { 4 } else { 5 };
                } else {
                    if crlf_state == 1 {
                        token_pos += 1;
                    }
                    if state == 1 || state == 3 {
                        token_pos += 1;
                    }
                }
                crlf_state = 0;
            }
            _ => {
                let mut fallthrough = false;
                match state {
                    0 => {
                        token = Some(i);
                        token_pos = 0;
                        state = 1;
                    }
                    2 => {
                        if crlf_state != -1 {
                            token = Some(i);
                            token_pos = 0;
                            state = 3;
                        } else {
                            fallthrough = true;
                        }
                    }
                    3 => fallthrough = true,
                    4 => {
                        token_pos += 1;
                        state = 3;
                    }
                    _ => {}
                }
                if fallthrough && crlf_state == -1 {
                    if let Some(t) = token {
                        if token_pos > 0 {
                            fld_val = Some(s[t..t + token_pos].to_vec());
                        }
                    }
                    store(&mut fld_name, &mut fld_val);
                    token = Some(i);
                    token_pos = 0;
                    state = 1;
                }
                if crlf_state == 1 {
                    token_pos += 1;
                }
                token_pos += 1;
                crlf_state = 0;
            }
        }
        i += 1;
    }
    let mut empty_value = false;
    if state == 2 {
        empty_value = true;
        state = 3;
    }
    if state == 3 {
        if empty_value {
            // `token = ""`: a zero-length value is not stored.
        } else if let Some(t) = token {
            if token_pos > 0 {
                fld_val = Some(s[t..t + token_pos].to_vec());
            }
        }
        store(&mut fld_name, &mut fld_val);
    }
    ht
}

fn path_arg(vm: &mut Vm, s: &[u8], arg: usize) -> PResult<()> {
    if s.contains(&0) {
        return Err(vm.argument_value_error(arg, "must not contain any null bytes"));
    }
    Ok(())
}

/// `php_strtok_r`: the next token delimited by any of `delims`.
fn strtok<'a>(rest: &mut &'a [u8], delims: &[u8]) -> Option<&'a [u8]> {
    let s = *rest;
    let start = s.iter().position(|c| !delims.contains(c))?;
    let len = s[start..]
        .iter()
        .position(|c| delims.contains(c))
        .unwrap_or(s.len() - start);
    let end = start + len;
    *rest = s.get(end + 1..).unwrap_or(&[]);
    Some(&s[start..end])
}

fn rtrim(s: &[u8]) -> &[u8] {
    let end = s
        .iter()
        .rposition(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0))
        .map_or(0, |i| i + 1);
    &s[..end]
}

pub fn mb_send_mail(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let lang = no2language(globals(vm)?.language);
    let mut tran_cs: &'static Encoding = enc(lang.mail_charset);
    let head_enc: &'static Encoding = enc(lang.mail_header_encoding);
    let mut body_enc: &'static Encoding = enc(lang.mail_body_encoding);

    let to = args.str(0).clone();
    path_arg(vm, &to, 1)?;
    let subject = args.str(1).clone();
    path_arg(vm, &subject, 2)?;
    let message = args.str(2).clone();
    path_arg(vm, &message, 3)?;
    let extra_cmd = args.str_opt(4).cloned();
    if let Some(e) = &extra_cmd {
        path_arg(vm, e, 5)?;
    }

    let str_headers: Option<Vec<u8>> = match args.get(3).clone() {
        Value::String(h) => {
            if h.contains(&0) {
                return Err(vm.argument_value_error(4, "must not contain any null bytes"));
            }
            Some(rtrim(&h).to_vec())
        }
        Value::Array(a) => crate::standard::mail::build_headers(vm, &a)?,
        _ => None,
    };

    let ht = str_headers
        .as_deref()
        .map(parse_mail_headers)
        .unwrap_or_default();

    let mut suppress_content_type = false;
    if let Some(ct) = ht.get(b"content-type".as_slice()) {
        // `strchr(value, ';')` on the C string.
        let ct = &ct[..ct.iter().position(|&c| c == 0).unwrap_or(ct.len())];
        if let Some(semi) = ct.iter().position(|&c| c == b';') {
            let mut p = semi + 1;
            while p < ct.len() && (ct[p] == b' ' || ct[p] == b'\t') {
                p += 1;
            }
            if p < ct.len() {
                let mut rest = &ct[p..];
                if let Some(param_name) = strtok(&mut rest, b"= ") {
                    if param_name.eq_ignore_ascii_case(b"charset") {
                        let charset = strtok(&mut rest, b"= \"");
                        let new_cs = match charset {
                            Some(c) => name2encoding_cstr(c),
                            None => Some(tran_cs),
                        };
                        tran_cs = match new_cs {
                            Some(e) => e,
                            None => {
                                vm.warning(format!(
                                    "Unsupported charset \"{}\" - will be regarded as ascii",
                                    String::from_utf8_lossy(charset.unwrap_or_default())
                                ))?;
                                enc(NoEncoding::Ascii)
                            }
                        };
                    }
                }
            }
        }
        suppress_content_type = true;
    }

    let mut suppress_cte = false;
    if let Some(cte) = ht.get(b"content-transfer-encoding".as_slice()) {
        let e = name2encoding_cstr(cte);
        match e.map(|e| e.no) {
            Some(NoEncoding::Base64 | NoEncoding::SevenBit | NoEncoding::EightBit) => {
                body_enc = e.expect("matched");
            }
            _ => {
                vm.warning(format!(
                    "Unsupported transfer encoding \"{}\" - will be regarded as 8bit",
                    super::globals::cstr_lossy(cte)
                ))?;
                body_enc = enc(NoEncoding::EightBit);
            }
        }
        suppress_cte = true;
    }

    // To: drop trailing whitespace, replace control characters by spaces
    // except folding CRLF + whitespace.
    let mut to_r = to.to_vec();
    while to_r.last().is_some_and(|c| c.is_ascii_whitespace() || *c == 0x0b) {
        to_r.pop();
    }
    let end = to_r.iter().position(|&c| c == 0).unwrap_or(to_r.len());
    let mut i = 0;
    while i < end {
        if to_r[i] < 0x20 || to_r[i] == 0x7f {
            if to_r[i] == b'\r'
                && to_r.get(i + 1) == Some(&b'\n')
                && matches!(to_r.get(i + 2), Some(b' ' | b'\t'))
            {
                i += 2;
                while matches!(to_r.get(i + 1), Some(b' ' | b'\t')) {
                    i += 1;
                }
                i += 1;
                continue;
            }
            to_r[i] = b' ';
        }
        i += 1;
    }

    let line_sep: &[u8] = if vm.ini_bool("mail.mixed_lf_and_crlf") {
        b"\n"
    } else {
        b"\r\n"
    };

    // Subject
    let strict = strict_detection(vm);
    let (internal, detect_order) = {
        let g = globals(vm)?;
        (g.current_internal_encoding, g.current_detect_order_list.clone())
    };
    let subj_enc = if internal.is(NoEncoding::Pass) {
        guess_encoding(&subject, &detect_order, strict, false).unwrap_or(internal)
    } else {
        internal
    };
    let subject_enc = mime_header_encode(
        &subject,
        subj_enc,
        tran_cs,
        head_enc.is(NoEncoding::Base64),
        line_sep,
        ("Subject: [PHP-jp nnnnnnnn]".len() + line_sep.len()) as i64,
    );

    // Message body
    let msg_enc = if internal.is(NoEncoding::Pass) {
        guess_encoding(&message, &detect_order, strict, false).unwrap_or(internal)
    } else {
        internal
    };
    let (tmp, _) = mb_fast_convert(&message, msg_enc, tran_cs, b'?' as u32, ILLEGAL_MODE_CHAR);
    let (conv, _) = mb_fast_convert(&tmp, enc(NoEncoding::EightBit), body_enc, b'?' as u32, ILLEGAL_MODE_CHAR);

    // Other headers
    let mut hdr: Vec<u8> = Vec::new();
    let mut empty = true;
    if let Some(h) = str_headers.as_deref().filter(|h| !h.is_empty()) {
        // Strip a trailing CRLF; it is added back if necessary.
        let mut len = h.len();
        if h[len - 1] == b'\n' {
            len -= 1;
        }
        if len > 0 && h[len - 1] == b'\r' {
            len -= 1;
        }
        hdr.extend_from_slice(&h[..len]);
        empty = false;
    }
    if !ht.contains_key(b"mime-version".as_slice()) {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"MIME-Version: 1.0");
        empty = false;
    }
    if !suppress_content_type {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"Content-Type: text/plain");
        if let Some(p) = preferred_mime_name(tran_cs) {
            hdr.extend_from_slice(b"; charset=");
            hdr.extend_from_slice(p.as_bytes());
        }
        empty = false;
    }
    if !suppress_cte {
        if !empty {
            hdr.extend_from_slice(line_sep);
        }
        hdr.extend_from_slice(b"Content-Transfer-Encoding: ");
        hdr.extend_from_slice(preferred_mime_name(body_enc).unwrap_or("7bit").as_bytes());
    }

    let force = vm.ini_get("mail.force_extra_parameters").filter(|f| !f.is_empty());
    let extra = match (force, extra_cmd) {
        (Some(f), _) => Some(crate::standard::exec::php_escape_shell_cmd(vm, &f)?),
        (None, Some(e)) => Some(crate::standard::exec::php_escape_shell_cmd(vm, &e)?),
        (None, None) => None,
    };

    let to_c = &to_r[..to_r.iter().position(|&c| c == 0).unwrap_or(to_r.len())];
    let ok = crate::standard::mail::php_mail(
        vm,
        to_c,
        &subject_enc,
        &conv,
        Some(&hdr),
        extra.as_deref(),
    )?;
    Ok(Value::Bool(ok))
}

#[cfg(test)]
mod tests {
    use super::parse_mail_headers;

    #[test]
    fn parses_headers() {
        let ht = parse_mail_headers(b"Content-Type: text/plain; charset=EUC-JP\r\nX-Foo: bar\r\n baz");
        assert_eq!(ht.get(b"content-type".as_slice()).unwrap(), b"text/plain; charset=EUC-JP");
        assert_eq!(ht.get(b"x-foo".as_slice()).unwrap(), b"bar\r\n baz");
    }
}
