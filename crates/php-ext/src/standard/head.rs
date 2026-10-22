//! ext/standard/head.c, with the SAPI header machinery of `main/SAPI.c`
//! (`sapi_header_op`, `sapi_send_headers`) as the CLI SAPI behaves.
//!
//! The CLI's header handler (`sapi_cli_header_handler`) discards every
//! header line, so `headers_list()` is always empty; what `header()` and
//! friends do observe is validation (warnings), the response code and the
//! "headers already sent" state.

use php_runtime::value::convert::{to_bool_silent, to_int_silent};
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `SG(sapi_headers)` plus the output layer's "start of output" location.
#[derive(Default)]
pub struct HeaderState {
    /// `SG(sapi_headers).http_response_code` (0 = unset).
    response_code: i64,
    /// `SG(sapi_headers).http_status_line`.
    status_line: Option<PhpStr>,
    /// `SG(send_header_fcc)` (`header_register_callback()`).
    callback: Option<Value>,
    /// `SG(headers_sent)` as set by [`send_headers`].
    headers_sent: bool,
    /// `php_output_get_start_filename()` / `_lineno()`.
    output_start: Option<(PhpStr, u32)>,
}

/// Records where output started (`OG(output_start_filename)`); the output
/// layer calls this when the first byte reaches the SAPI. Without it
/// [`headers_sent`]-related messages report the location of the first call
/// that noticed the output.
pub fn set_output_start(vm: &mut Vm, file: PhpStr, line: u32) {
    let st = vm.ext_data::<HeaderState>();
    if st.output_start.is_none() {
        st.output_start = Some((file, line));
    }
}

/// `SG(headers_sent)`: output has reached the SAPI (or `sapi_send_headers`
/// ran), and where it started.
fn headers_sent_state(vm: &mut Vm) -> Option<(PhpStr, u32)> {
    let explicit = vm.ext_data::<HeaderState>().headers_sent;
    if !explicit && !vm.output.sent {
        return None;
    }
    let here = if vm.executing {
        (vm.current_file.clone(), vm.current_line)
    } else {
        (PhpStr::new(), 0)
    };
    let st = vm.ext_data::<HeaderState>();
    Some(st.output_start.get_or_insert(here).clone())
}

/// `sapi_send_headers()`: runs the registered callback once and marks the
/// headers as sent. The output layer calls it before the first write.
pub fn send_headers(vm: &mut Vm) -> PResult<()> {
    if vm.ext_data::<HeaderState>().headers_sent {
        return Ok(());
    }
    let cb = vm.ext_data::<HeaderState>().callback.take();
    if let Some(cb) = cb {
        // prevent triggering the callback multiple times
        vm.call(&cb, Vec::new())?;
    }
    vm.ext_data::<HeaderState>().headers_sent = true;
    Ok(())
}

/// `sapi_header_op()`'s SAPI_HEADER_ADD / REPLACE / DELETE / DELETE_ALL.
enum Op {
    Add,
    Replace,
    Delete,
    DeleteAll,
}

fn isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `sapi_extract_response_code()`
fn extract_response_code(line: &[u8]) -> i64 {
    for (i, &c) in line.iter().enumerate() {
        if c == b' ' && line.get(i + 1).is_some_and(|&n| n != b' ') {
            return php_runtime::ini::atoi(&line[i + 1..]);
        }
    }
    200
}

/// `sapi_update_response_code()`
fn update_response_code(vm: &mut Vm, code: i64) {
    let st = vm.ext_data::<HeaderState>();
    // if the status code did not change, the status line stays
    if st.response_code == code {
        return;
    }
    st.status_line = None;
    st.response_code = code;
}

/// `sapi_header_op()`: `Ok(true)` is SUCCESS.
fn sapi_header_op(vm: &mut Vm, op: Op, line: Option<&[u8]>, response_code: i64) -> PResult<bool> {
    if let Some((file, lineno)) = headers_sent_state(vm) {
        let msg = if file.is_empty() {
            "Cannot modify header information - headers already sent".to_string()
        } else {
            format!(
                "Cannot modify header information - headers already sent by (output started at {file}:{lineno})"
            )
        };
        vm.engine_warning(msg)?;
        return Ok(false);
    }

    if matches!(op, Op::DeleteAll) {
        return Ok(true);
    }
    let Some(line) = line.filter(|l| !l.is_empty()) else {
        return Ok(false);
    };

    // cut off trailing spaces, linefeeds and carriage-returns
    let mut end = line.len();
    while end > 0 && isspace(line[end - 1]) {
        end -= 1;
    }
    let line = &line[..end];

    if matches!(op, Op::Delete) {
        if line.contains(&b':') {
            vm.engine_warning("Header to delete may not contain colon.")?;
            return Ok(false);
        }
        return Ok(true);
    }

    // new line/NUL character safety check
    for &c in line {
        // RFC 7230 ch. 3.2.4 deprecates folding support
        if c == b'\n' || c == b'\r' {
            vm.engine_warning(
                "Header may not contain more than a single header, new line detected",
            )?;
            return Ok(false);
        }
        if c == 0 {
            vm.engine_warning("Header may not contain NUL bytes")?;
            return Ok(false);
        }
    }

    // a few headers have special meaning for the SAPI
    if line.len() >= 5 && line[..5].eq_ignore_ascii_case(b"HTTP/") {
        // filter out the response code
        update_response_code(vm, extract_response_code(line));
        vm.ext_data::<HeaderState>().status_line = Some(PhpStr::from(line));
        return Ok(true);
    }
    if let Some(colon) = line.iter().position(|&c| c == b':') {
        let name = &line[..colon];
        if name.eq_ignore_ascii_case(b"Location") {
            let code = vm.ext_data::<HeaderState>().response_code;
            if !(300..=399).contains(&code) && code != 201 {
                // Return a Found Redirect if one is not already specified
                if response_code != 0 {
                    update_response_code(vm, response_code);
                } else {
                    // the CLI has no request method: a plain 302
                    update_response_code(vm, 302);
                }
            }
        } else if name.eq_ignore_ascii_case(b"WWW-Authenticate") {
            // HTTP Authentication
            update_response_code(vm, 401);
        }
    }
    if response_code != 0 {
        update_response_code(vm, response_code);
    }
    // sapi_cli_header_handler() refuses every header line, so `Add` and
    // `Replace` have nothing left to do
    Ok(true)
}

fn header(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let line = args.str(0).clone();
    let replace = args.bool(1);
    let code = args.int(2);
    let op = if replace { Op::Replace } else { Op::Add };
    sapi_header_op(vm, op, Some(&line), code)?;
    Ok(Value::Null)
}

fn header_remove(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    match args.str_opt(0).cloned() {
        None => sapi_header_op(vm, Op::DeleteAll, None, 0)?,
        Some(line) => sapi_header_op(vm, Op::Delete, Some(&line), 0)?,
    };
    Ok(Value::Null)
}

fn header_register_callback(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cb = args.get(0).clone();
    // Don't store the callback if the headers have already been sent.
    if headers_sent_state(vm).is_none() {
        vm.ext_data::<HeaderState>().callback = Some(cb);
    } else {
        vm.ext_data::<HeaderState>().callback = None;
    }
    Ok(Value::Bool(true))
}

const ILLEGAL_COOKIE_CHARACTER: &str =
    "\",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"";

fn has_cookie_separator(s: &[u8]) -> bool {
    s.iter().any(|c| b",; \t\r\n\x0b\x0c".contains(c))
}

/// `php_raw_url_encode()` (RFC 3986).
fn raw_url_encode(s: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(s.len());
    for &c in s {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c);
        } else {
            out.push(b'%');
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 0xf) as usize]);
        }
    }
    out
}

/// The parameters of `php_setcookie()`.
#[derive(Default)]
pub struct Cookie {
    pub name: PhpStr,
    pub value: Option<PhpStr>,
    pub expires: i64,
    pub path: Option<PhpStr>,
    pub domain: Option<PhpStr>,
    pub secure: bool,
    pub httponly: bool,
    pub samesite: Option<PhpStr>,
    pub partitioned: bool,
}

/// `php_setcookie()`: validates, builds the `Set-Cookie` line and hands it
/// to the SAPI. `Ok(None)` is FAILURE (a warning was emitted); the line is
/// returned for callers that want to see it.
pub fn php_setcookie(vm: &mut Vm, c: &Cookie, url_encode: bool) -> PResult<Option<Vec<u8>>> {
    let fname = vm.current_function_name();
    if c.name.is_empty() {
        return Err(vm.argument_value_error(1, "cannot be empty"));
    }
    if c.name.iter().any(|ch| b"=,; \t\r\n\x0b\x0c".contains(ch)) {
        return Err(vm.argument_value_error(
            1,
            &format!("cannot contain \"=\", {ILLEGAL_COOKIE_CHARACTER}"),
        ));
    }
    if !url_encode {
        if let Some(v) = &c.value {
            if has_cookie_separator(v) {
                return Err(vm.argument_value_error(
                    2,
                    &format!("cannot contain {ILLEGAL_COOKIE_CHARACTER}"),
                ));
            }
        }
    }
    if c.path.as_ref().is_some_and(|p| has_cookie_separator(p)) {
        return Err(vm.throw_value_error(format!(
            "{fname}(): \"path\" option cannot contain {ILLEGAL_COOKIE_CHARACTER}"
        )));
    }
    if c.domain.as_ref().is_some_and(|d| has_cookie_separator(d)) {
        return Err(vm.throw_value_error(format!(
            "{fname}(): \"domain\" option cannot contain {ILLEGAL_COOKIE_CHARACTER}"
        )));
    }
    if c.expires >= 253_402_300_800 {
        return Err(vm.throw_value_error(format!(
            "{fname}(): \"expires\" option cannot have a year greater than 9999"
        )));
    }
    if c.partitioned && !c.secure {
        return Err(vm.throw_value_error(format!(
            "{fname}(): \"partitioned\" option cannot be used without \"secure\" option"
        )));
    }
    if let Some(s) = c.samesite.as_ref().filter(|s| !s.is_empty()) {
        let ok = ["Strict", "Lax", "None"]
            .iter()
            .any(|v| s.eq_ignore_ascii_case(v.as_bytes()));
        if !ok {
            return Err(vm.throw_value_error(format!(
                "{fname}(): \"samesite\" option must be \"Strict\", \"Lax\", \"None\", or \"\""
            )));
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    match c.value.as_ref().filter(|v| !v.is_empty()) {
        None => {
            // MSIE doesn't delete a cookie set to a null value: expire it in the past
            buf.extend_from_slice(b"Set-Cookie: ");
            buf.extend_from_slice(&c.name);
            buf.extend_from_slice(b"=deleted; expires=Thu, 01 Jan 1970 00:00:01 GMT; Max-Age=0");
        }
        Some(value) => {
            buf.extend_from_slice(b"Set-Cookie: ");
            buf.extend_from_slice(&c.name);
            buf.push(b'=');
            if url_encode {
                buf.extend_from_slice(&raw_url_encode(value));
            } else {
                buf.extend_from_slice(value);
            }
            if c.expires > 0 {
                buf.extend_from_slice(b"; expires=");
                buf.extend_from_slice(&php_timelib::format_timestamp(
                    b"D, d M Y H:i:s \\G\\M\\T",
                    c.expires,
                    None,
                ));
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                let diff = (c.expires - now).max(0);
                buf.extend_from_slice(format!("; Max-Age={diff}").as_bytes());
            }
        }
    }
    if let Some(p) = c.path.as_ref().filter(|p| !p.is_empty()) {
        buf.extend_from_slice(b"; path=");
        buf.extend_from_slice(p);
    }
    if let Some(d) = c.domain.as_ref().filter(|d| !d.is_empty()) {
        buf.extend_from_slice(b"; domain=");
        buf.extend_from_slice(d);
    }
    if c.secure {
        buf.extend_from_slice(b"; secure");
    }
    if c.httponly {
        buf.extend_from_slice(b"; HttpOnly");
    }
    if let Some(s) = c.samesite.as_ref().filter(|s| !s.is_empty()) {
        buf.extend_from_slice(b"; SameSite=");
        buf.extend_from_slice(s);
    }
    if c.partitioned {
        buf.extend_from_slice(b"; Partitioned");
    }

    if sapi_header_op(vm, Op::Add, Some(&buf), 0)? {
        Ok(Some(buf))
    } else {
        Ok(None)
    }
}

/// `php_head_parse_cookie_options_array()`: `Ok(false)` after a thrown error.
fn parse_cookie_options(vm: &mut Vm, options: &PhpArray, c: &mut Cookie) -> PResult<()> {
    let fname = vm.current_function_name();
    for (k, v) in options.iter() {
        let php_runtime::ArrayKey::Str(key) = &k else {
            return Err(
                vm.throw_value_error(format!("{fname}(): option array cannot have numeric keys"))
            );
        };
        let v = v.unref();
        let lk = key.to_ascii_lowercase();
        match lk.as_bytes() {
            b"expires" => c.expires = to_int_silent(&v),
            b"path" => c.path = Some(vm.to_php_string(&v)?),
            b"domain" => c.domain = Some(vm.to_php_string(&v)?),
            b"secure" => c.secure = to_bool_silent(&v),
            b"httponly" => c.httponly = to_bool_silent(&v),
            b"samesite" => c.samesite = Some(vm.to_php_string(&v)?),
            b"partitioned" => c.partitioned = to_bool_silent(&v),
            _ => {
                return Err(vm.throw_value_error(format!("{fname}(): option \"{key}\" is invalid")));
            }
        }
    }
    Ok(())
}

fn setcookie_common(vm: &mut Vm, args: &mut Args, is_raw: bool) -> PResult<Value> {
    let mut c = Cookie {
        name: args.str(0).clone(),
        ..Cookie::default()
    };
    if args.is_passed(1) {
        c.value = Some(args.str(1).clone());
    }
    match args.get(2).clone() {
        Value::Array(options) => {
            if args.len() > 3 {
                let fname = vm.current_function_name();
                return Err(vm.argument_count_error(format!(
                    "{fname}(): Expects exactly 3 arguments when argument #3 ($expires_or_options) is an array"
                )));
            }
            parse_cookie_options(vm, &options, &mut c)?;
        }
        v => {
            c.expires = to_int_silent(&v);
            if args.is_passed(3) {
                c.path = Some(args.str(3).clone());
            }
            if args.is_passed(4) {
                c.domain = Some(args.str(4).clone());
            }
            c.secure = args.bool(5);
            c.httponly = args.bool(6);
        }
    }
    Ok(Value::Bool(php_setcookie(vm, &c, !is_raw)?.is_some()))
}

fn setcookie(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    setcookie_common(vm, args, false)
}

fn setrawcookie(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    setcookie_common(vm, args, true)
}

fn headers_sent(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let state = headers_sent_state(vm);
    let (file, line) = match &state {
        Some((f, l)) => (f.clone(), *l as i64),
        None => (PhpStr::new(), 0),
    };
    if args.is_passed(0) {
        args.by_ref(0).set(Value::String(file));
    }
    if args.is_passed(1) {
        args.by_ref(1).set(Value::Int(line));
    }
    Ok(Value::Bool(state.is_some()))
}

fn headers_list(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // sapi_cli_header_handler() keeps no header lines
    Ok(Value::empty_array())
}

fn http_response_code(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let response_code = args.int(0);
    if response_code != 0 {
        if let Some((file, line)) = headers_sent_state(vm) {
            if file.is_empty() {
                vm.warning("Cannot set response code - headers already sent")?;
            } else {
                vm.warning(format!(
                    "Cannot set response code - headers already sent (output started at {file}:{line})"
                ))?;
            }
            return Ok(Value::Bool(false));
        }
        if vm.ext_data::<HeaderState>().status_line.is_some() {
            vm.warning("Calling http_response_code() after header('HTTP/...') has no effect")?;
        }
        let st = vm.ext_data::<HeaderState>();
        let old = st.response_code;
        st.response_code = response_code;
        return Ok(if old != 0 {
            Value::Int(old)
        } else {
            Value::Bool(true)
        });
    }
    let code = vm.ext_data::<HeaderState>().response_code;
    Ok(if code == 0 {
        Value::Bool(false)
    } else {
        Value::Int(code)
    })
}

pub static HEADER: NativeFunction = NativeFunction {
    sig: "header(string $header, bool $replace = true, int $response_code = 0): void",
    handler: header,
};
pub static HEADER_REMOVE: NativeFunction = NativeFunction {
    sig: "header_remove(?string $name = null): void",
    handler: header_remove,
};
pub static HEADER_REGISTER_CALLBACK: NativeFunction = NativeFunction {
    sig: "header_register_callback(callable $callback): true",
    handler: header_register_callback,
};
pub static SETRAWCOOKIE: NativeFunction = NativeFunction {
    sig: "setrawcookie(string $name, string $value = \"\", array|int $expires_or_options = 0, string $path = \"\", string $domain = \"\", bool $secure = false, bool $httponly = false): bool",
    handler: setrawcookie,
};
pub static SETCOOKIE: NativeFunction = NativeFunction {
    sig: "setcookie(string $name, string $value = \"\", array|int $expires_or_options = 0, string $path = \"\", string $domain = \"\", bool $secure = false, bool $httponly = false): bool",
    handler: setcookie,
};
pub static HTTP_RESPONSE_CODE: NativeFunction = NativeFunction {
    sig: "http_response_code(int $response_code = 0): int|bool",
    handler: http_response_code,
};
pub static HEADERS_SENT: NativeFunction = NativeFunction {
    sig: "headers_sent(&$filename = null, &$line = null): bool",
    handler: headers_sent,
};
pub static HEADERS_LIST: NativeFunction = NativeFunction {
    sig: "headers_list(): array",
    handler: headers_list,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &HEADER,
        &HEADER_REMOVE,
        &HEADER_REGISTER_CALLBACK,
        &SETRAWCOOKIE,
        &SETCOOKIE,
        &HTTP_RESPONSE_CODE,
        &HEADERS_SENT,
        &HEADERS_LIST,
    ]);
}

#[cfg(test)]
mod tests;
