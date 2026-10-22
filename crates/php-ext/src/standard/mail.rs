//! ext/standard/mail.c

use std::io::Write;
use std::process::{Command, Stdio};

use php_runtime::ini::{IniDef, modifiable};
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use crate::standard::basic::{self, path_arg};
use crate::standard::syslog;

const DEFAULT_SENDMAIL_PATH: &str = "/usr/sbin/sendmail -t -i";

/// `isspace()` in the C locale.
fn c_isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `iscntrl()` in the C locale.
fn c_iscntrl(c: u8) -> bool {
    c < 0x20 || c == 0x7f
}

/// `php_mail_header_value_error_type`
enum HeaderValueError {
    None,
    LfOnly,
    CrOnly,
    Crlf,
    Null,
}

fn check_field_value(v: &[u8]) -> HeaderValueError {
    let mut len = 0;
    while len < v.len() {
        let at = |i: usize| v.get(i).copied().unwrap_or(0);
        if v[len] == b'\r' {
            if at(len + 1) != b'\n' {
                return HeaderValueError::CrOnly;
            }
            if v.len() - len >= 3 && (at(len + 2) == b' ' || at(len + 2) == b'\t') {
                len += 3;
                continue;
            }
            return HeaderValueError::Crlf;
        }
        // LF alone is tolerated for folding.
        if v[len] == b'\n' {
            if v.len() - len >= 2 && (at(len + 1) == b' ' || at(len + 1) == b'\t') {
                len += 2;
                continue;
            }
            return HeaderValueError::LfOnly;
        }
        if v[len] == 0 {
            return HeaderValueError::Null;
        }
        len += 1;
    }
    HeaderValueError::None
}

fn check_field_name(key: &[u8]) -> bool {
    !key.iter().any(|&c| c < 33 || c > 126 || c == b':')
}

fn key_text(key: &[u8]) -> String {
    String::from_utf8_lossy(key).into_owned()
}

fn build_headers_elem(vm: &mut Vm, out: &mut Vec<u8>, key: &[u8], val: &Value) -> PResult<()> {
    match val {
        Value::String(s) => {
            if !check_field_name(key) {
                return Err(vm.throw_value_error(format!(
                    "Header name \"{}\" contains invalid characters",
                    key_text(key)
                )));
            }
            let msg = match check_field_value(s) {
                HeaderValueError::None => None,
                HeaderValueError::LfOnly => {
                    Some("contains LF character that is not allowed in the header")
                }
                HeaderValueError::CrOnly => {
                    Some("contains CR character that is not allowed in the header")
                }
                HeaderValueError::Crlf => Some(
                    "contains CRLF characters that are used as a line separator and are not allowed in the header",
                ),
                HeaderValueError::Null => {
                    Some("contains NULL character that is not allowed in the header")
                }
            };
            if let Some(m) = msg {
                return Err(vm.throw_value_error(format!("Header \"{}\" {m}", key_text(key))));
            }
            out.extend_from_slice(key);
            out.extend_from_slice(b": ");
            out.extend_from_slice(s);
            out.extend_from_slice(b"\r\n");
            Ok(())
        }
        Value::Array(a) => build_headers_elems(vm, out, key, a),
        other => Err(vm.throw_type_error(format!(
            "Header \"{}\" must be of type array|string, {} given",
            key_text(key),
            other.value_name()
        ))),
    }
}

fn build_headers_elems(vm: &mut Vm, out: &mut Vec<u8>, key: &[u8], val: &PhpArray) -> PResult<()> {
    for (k, v) in val.iter() {
        if let php_runtime::ArrayKey::Str(tmp_key) = &k {
            return Err(vm.throw_type_error(format!(
                "Header \"{}\" must only contain numeric keys, \"{}\" found",
                key_text(key),
                tmp_key.to_string_lossy()
            )));
        }
        let v = v.unref();
        if !matches!(v, Value::String(_)) {
            return Err(vm.throw_type_error(format!(
                "Header \"{}\" must only contain values of type string, {} found",
                key_text(key),
                v.value_name()
            )));
        }
        build_headers_elem(vm, out, key, &v)?;
    }
    Ok(())
}

/// `php_mail_build_headers()`: `None` when no header was produced.
pub(crate) fn build_headers(vm: &mut Vm, headers: &PhpArray) -> PResult<Option<Vec<u8>>> {
    const SINGLE: &[&str] = &[
        "orig-date",
        "from",
        "sender",
        "reply-to",
        "cc",
        "bcc",
        "message-id",
        "references",
        "in-reply-to",
    ];
    let mut s: Vec<u8> = Vec::new();
    for (k, val) in headers.iter() {
        let key = match &k {
            php_runtime::ArrayKey::Int(i) => {
                return Err(
                    vm.throw_type_error(format!("Header name cannot be numeric, {i} given"))
                );
            }
            php_runtime::ArrayKey::Str(s) => s.clone(),
        };
        let val = val.unref();
        let lc = key.to_ascii_lowercase();
        if lc == "to" {
            return Err(
                vm.throw_value_error("The additional headers cannot contain the \"To\" header")
            );
        } else if lc == "subject" {
            return Err(vm.throw_value_error(
                "The additional headers cannot contain the \"Subject\" header",
            ));
        } else if let Some(target) = SINGLE.iter().find(|t| lc == **t) {
            match &val {
                Value::String(_) => build_headers_elem(vm, &mut s, &key, &val)?,
                // PHP_MAIL_BUILD_HEADER_CHECK: these headers may not be arrays
                Value::Array(_) => {
                    return Err(vm.throw_type_error(format!(
                        "Header \"{target}\" must be of type string, array given"
                    )));
                }
                other => {
                    return Err(vm.throw_type_error(format!(
                        "Header \"{}\" must be of type array|string, {} given",
                        key_text(&key),
                        other.value_name()
                    )));
                }
            }
        } else {
            build_headers_elem(vm, &mut s, &key, &val)?;
        }
    }
    if s.is_empty() {
        return Ok(None);
    }
    // Remove the last \r\n
    s.truncate(s.len() - 2);
    Ok(Some(s))
}

/// `php_escape_shell_cmd()` (POSIX flavour, every byte a single character).
fn escape_shell_cmd(cmd: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(cmd.len() * 2);
    let mut p: Option<usize> = None;
    for x in 0..cmd.len() {
        let c = cmd[x];
        match c {
            b'"' | b'\'' => {
                if p.is_none() {
                    p = cmd[x + 1..].iter().position(|&b| b == c).map(|i| x + 1 + i);
                    if p.is_none() {
                        out.push(b'\\');
                    }
                } else if p.is_some_and(|i| cmd[i] == c) {
                    p = None;
                } else {
                    out.push(b'\\');
                }
                out.push(c);
            }
            b'#' | b'&' | b';' | b'`' | b'|' | b'*' | b'?' | b'~' | b'<' | b'>' | b'^' | b'('
            | b')' | b'[' | b']' | b'{' | b'}' | b'$' | b'\\' | b'\x0a' | b'\xff' => {
                out.push(b'\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// `SKIP_LONG_HEADER_SEP` + control-character replacement for `to`/`subject`.
fn sanitize_header_text(src: &[u8]) -> Vec<u8> {
    if src.is_empty() {
        return Vec::new();
    }
    let mut r = src.to_vec();
    // The C code works on a NUL-terminated copy: trailing whitespace is cut.
    while r.last().is_some_and(|&c| c_isspace(c)) {
        r.pop();
    }
    let mut i = 0;
    while i < r.len() {
        if r[i] == 0 {
            // `for (i = 0; to_r[i]; i++)` stops at an embedded NUL
            r.truncate(i);
            break;
        }
        if c_iscntrl(r[i]) {
            // RFC 822 3.1.1: CRLF followed by SP/TAB folds a long header.
            let at = |j: usize| r.get(j).copied().unwrap_or(0);
            if r[i] == b'\r' && at(i + 1) == b'\n' && (at(i + 2) == b' ' || at(i + 2) == b'\t') {
                i += 2;
                while at(i + 1) == b' ' || at(i + 1) == b'\t' {
                    i += 1;
                }
                i += 1;
                continue;
            }
            r[i] = b' ';
        }
        i += 1;
    }
    r
}

/// `php_mail_detect_multiple_crlf()`
fn detect_multiple_crlf(hdr: &[u8]) -> bool {
    if hdr.is_empty() {
        return false;
    }
    let at = |i: usize| hdr.get(i).copied().unwrap_or(0);
    // No newlines at the beginning (RFC 2822 2.2).
    if hdr[0] < 33 || hdr[0] > 126 || hdr[0] == b':' {
        return true;
    }
    let mut i = 0;
    while at(i) != 0 {
        if hdr[i] == b'\r' {
            let n1 = at(i + 1);
            if n1 == 0 || n1 == b'\r' || (n1 == b'\n' && matches!(at(i + 2), 0 | b'\n' | b'\r')) {
                return true;
            }
            i += 2;
        } else if hdr[i] == b'\n' {
            if matches!(at(i + 1), 0 | b'\r' | b'\n') {
                return true;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

/// `php_mail()`: pipes the message to `sendmail_path`.
pub(crate) fn php_mail(
    vm: &mut Vm,
    to: &[u8],
    subject: &[u8],
    message: &[u8],
    headers: Option<&[u8]>,
    extra_cmd: Option<&[u8]>,
) -> PResult<bool> {
    let sendmail_path = vm.ini_get("sendmail_path").unwrap_or_default();
    let mail_log = vm.ini_get("mail.log").filter(|l| !l.is_empty());
    let mut hdr: Option<Vec<u8>> = headers.map(|h| h.to_vec());

    if let Some(mail_log) = mail_log {
        let mut line =
            format!("mail() on [{}:{}]: To: ", vm.current_file, vm.current_line).into_bytes();
        line.extend_from_slice(to);
        line.extend_from_slice(b" -- Headers: ");
        if let Some(h) = &hdr {
            line.extend_from_slice(h);
        }
        line.extend_from_slice(b" -- Subject: ");
        line.extend_from_slice(subject);
        if hdr.is_some() {
            // a log line is always one line long
            for b in line.iter_mut() {
                if *b == b'\r' || *b == b'\n' {
                    *b = b' ';
                }
            }
        }
        if mail_log == "syslog" {
            syslog::php_syslog(vm, 5 /* LOG_NOTICE */, &line);
        } else {
            let tz = basic::default_timezone(vm);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let stamp = php_timelib::builtin_tzinfo(&tz)
                .map(|i| php_timelib::format_timestamp(b"d-M-Y H:i:s e", now, Some(&i)))
                .unwrap_or_default();
            let mut entry = b"[".to_vec();
            entry.extend_from_slice(&stamp);
            entry.extend_from_slice(b"] ");
            entry.extend_from_slice(&line);
            entry.push(b'\n');
            let path = mail_log.to_string_lossy().into_owned();
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = f.write_all(&entry);
                }
                Err(e) => {
                    basic::warning_docref1(
                        vm,
                        path.as_bytes(),
                        &format!("Failed to open stream: {}", basic::os_error_text(&e)),
                    )?;
                }
            }
        }
    }

    let cr_lf_mode = vm.ini_get("mail.cr_lf_mode").unwrap_or_default();
    let line_sep: &[u8] = if !cr_lf_mode.is_empty() && cr_lf_mode != "crlf" {
        // "lf", "mixed" and (on POSIX) "os" all use a bare LF
        b"\n"
    } else if vm.ini_bool("mail.mixed_lf_and_crlf") {
        b"\n"
    } else {
        b"\r\n"
    };

    if vm.ini_bool("mail.add_x_header") {
        let file = vm.current_file.clone();
        let base = file
            .rsplit(|&c| c == b'/')
            .next()
            .unwrap_or(&file[..])
            .to_vec();
        let uid = crate::standard::pageinfo::getuid(vm);
        let mut ahdr = format!("X-PHP-Originating-Script: {uid}:").into_bytes();
        ahdr.extend_from_slice(&base);
        if let Some(h) = headers.filter(|h| !h.is_empty()) {
            ahdr.extend_from_slice(line_sep);
            ahdr.extend_from_slice(h);
        }
        hdr = Some(ahdr);
    }

    if let Some(h) = &hdr {
        if detect_multiple_crlf(h) {
            vm.warning("Multiple or malformed newlines found in additional_header")?;
            return Ok(false);
        }
    }

    if sendmail_path.is_empty() {
        return Ok(false);
    }
    let cmd: Vec<u8> = match extra_cmd {
        Some(extra) => {
            let mut c = sendmail_path.to_vec();
            c.push(b' ');
            c.extend_from_slice(extra);
            c
        }
        None => sendmail_path.to_vec(),
    };

    let spawned = Command::new("/bin/sh")
        .arg("-c")
        .arg(bytes_to_os(&cmd))
        .stdin(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(_) => {
            vm.warning(format!(
                "Could not execute mail delivery program '{sendmail_path}'"
            ))?;
            return Ok(false);
        }
    };
    {
        let mut body = Vec::new();
        body.extend_from_slice(b"To: ");
        body.extend_from_slice(to);
        body.extend_from_slice(line_sep);
        body.extend_from_slice(b"Subject: ");
        body.extend_from_slice(subject);
        body.extend_from_slice(line_sep);
        if let Some(h) = &hdr {
            body.extend_from_slice(h);
            body.extend_from_slice(line_sep);
        }
        body.extend_from_slice(line_sep);
        if cr_lf_mode == "lf" {
            // CRLF in the message becomes LF
            let mut i = 0;
            while i < message.len() {
                if i + 1 < message.len() && message[i] == b'\r' && message[i + 1] == b'\n' {
                    body.push(b'\n');
                    i += 2;
                } else {
                    body.push(message[i]);
                    i += 1;
                }
            }
        } else {
            // the C code prints the message with "%s": it stops at a NUL byte
            let end = message
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(message.len());
            body.extend_from_slice(&message[..end]);
        }
        body.extend_from_slice(line_sep);
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&body);
        }
    }
    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => {
            vm.warning(format!(
                "Sendmail pclose failed {} ({})",
                e.raw_os_error().unwrap_or(0),
                basic::os_error_text(&e)
            ))?;
            return Ok(false);
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            vm.warning(format!(
                "Sendmail killed by signal {sig} ({})",
                signal_name(sig)
            ))?;
            return Ok(false);
        }
    }
    let Some(code) = status.code() else {
        vm.warning("Sendmail did not exit")?;
        return Ok(false);
    };
    // EX_OK and EX_TEMPFAIL
    if code != 0 && code != 75 {
        vm.warning(format!("Sendmail exited with non-zero exit code {code}"))?;
        return Ok(false);
    }
    Ok(true)
}

fn bytes_to_os(b: &[u8]) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(b.to_vec())
    }
    #[cfg(not(unix))]
    {
        std::ffi::OsString::from(String::from_utf8_lossy(b).into_owned())
    }
}

#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "Hangup",
        2 => "Interrupt",
        3 => "Quit",
        6 => "Aborted",
        9 => "Killed",
        11 => "Segmentation fault",
        13 => "Broken pipe",
        14 => "Alarm clock",
        15 => "Terminated",
        _ => "Unknown signal",
    }
}

fn mail(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let to = path_arg(vm, args, 0)?;
    let subject = path_arg(vm, args, 1)?;
    let message = path_arg(vm, args, 2)?;
    let extra_cmd: Option<PhpStr> = if args.is_passed(4) {
        Some(path_arg(vm, args, 4)?)
    } else {
        None
    };

    let mut headers_str: Option<Vec<u8>> = None;
    match args.get(3) {
        Value::String(h) if args.is_passed(3) => {
            if h.contains(&0) {
                return Err(vm.argument_value_error(4, "must not contain any null bytes"));
            }
            // php_trim(..., mode 2): right trim with the default characters
            let mut v = h.to_vec();
            while v
                .last()
                .is_some_and(|&c| matches!(c, b' ' | b'\n' | b'\r' | b'\t' | 0x0b | 0))
            {
                v.pop();
            }
            headers_str = Some(v);
        }
        Value::Array(a) => {
            let a = a.clone();
            headers_str = build_headers(vm, &a)?;
        }
        _ => {}
    }

    let to_r = sanitize_header_text(&to);
    let subject_r = sanitize_header_text(&subject);

    let force = vm
        .ini_get("mail.force_extra_parameters")
        .unwrap_or_default();
    let extra: Option<Vec<u8>> = if !force.is_empty() {
        Some(escape_shell_cmd(&force))
    } else {
        extra_cmd.map(|e| escape_shell_cmd(&e))
    };

    let ok = php_mail(
        vm,
        &to_r,
        &subject_r,
        &message,
        headers_str.as_deref().filter(|h| !h.is_empty()),
        extra.as_deref(),
    )?;
    Ok(Value::Bool(ok))
}

pub static MAIL: NativeFunction = NativeFunction {
    sig: "mail(string $to, string $subject, string $message, array|string $additional_headers = [], string $additional_params = \"\"): bool",
    handler: mail,
};

pub fn register(r: &mut Registry) {
    // `main/main.c` directives, owned by Core.
    for def in [
        IniDef::new("SMTP", Some("localhost"), modifiable::ALL),
        IniDef::new("smtp_port", Some("25"), modifiable::ALL),
        IniDef::new(
            "mail.add_x_header",
            Some("0"),
            modifiable::SYSTEM | modifiable::PERDIR,
        ),
        IniDef::new(
            "mail.mixed_lf_and_crlf",
            Some("0"),
            modifiable::SYSTEM | modifiable::PERDIR,
        ),
        IniDef::new(
            "mail.cr_lf_mode",
            Some("crlf"),
            modifiable::SYSTEM | modifiable::PERDIR,
        )
        .validated(|v| v.is_empty() || matches!(v, b"crlf" | b"lf" | b"mixed" | b"os")),
        IniDef::new("mail.log", None, modifiable::SYSTEM | modifiable::PERDIR),
        IniDef::new("sendmail_from", None, modifiable::ALL),
        IniDef::new(
            "sendmail_path",
            Some(DEFAULT_SENDMAIL_PATH),
            modifiable::SYSTEM,
        ),
        IniDef::new(
            "mail.force_extra_parameters",
            None,
            modifiable::SYSTEM | modifiable::PERDIR,
        )
        .validated(|v| !v.contains(&0)),
    ] {
        r.vm.ini.register(&def, "Core");
    }
    r.function(&MAIL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("php_in_rust_{name}_{}", std::process::id()))
    }

    fn arr(pairs: &[(&str, Value)]) -> Value {
        let mut a = PhpArray::new();
        for (k, v) in pairs {
            a.insert_str(k, v.clone());
        }
        Value::array(a)
    }

    #[test]
    fn sendmail_pipe_receives_headers_and_body() {
        let mut t = TestVm::new();
        let out = tmp("mail_out");
        let _ = std::fs::remove_file(&out);
        t.vm.ini_set_system("sendmail_path", format!("cat > {}", out.display()));
        let r = t
            .call(
                "mail",
                vec![
                    Value::str("user@example.com"),
                    Value::str("Hello"),
                    Value::str("Body text"),
                    Value::str("From: me@example.com\r\nX-Foo: bar\r\n"),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(true)), "{}", t.output());
        let got = std::fs::read(&out).unwrap();
        assert_eq!(
            got,
            b"To: user@example.com\r\nSubject: Hello\r\nFrom: me@example.com\r\nX-Foo: bar\r\n\r\nBody text\r\n"
        );
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn lf_mode_and_array_headers() {
        let mut t = TestVm::new();
        let out = tmp("mail_lf");
        t.vm.ini_set_system("sendmail_path", format!("cat > {}", out.display()));
        t.vm.ini_set_system("mail.cr_lf_mode", "lf");
        let r = t
            .call(
                "mail",
                vec![
                    Value::str("a@b.c"),
                    Value::str("S"),
                    Value::str("l1\r\nl2"),
                    arr(&[("From", Value::str("x@y.z")), ("X-A", Value::str("1"))]),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(true)));
        assert_eq!(
            std::fs::read(&out).unwrap(),
            b"To: a@b.c\nSubject: S\nFrom: x@y.z\r\nX-A: 1\n\nl1\nl2\n".to_vec()
        );
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn without_sendmail_returns_false() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("sendmail_path", "");
        let r = t
            .call(
                "mail",
                vec![Value::str("a@b.c"), Value::str("S"), Value::str("M")],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn failing_sendmail_warns() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("sendmail_path", "exit 3");
        let r = t
            .call(
                "mail",
                vec![Value::str("a@b.c"), Value::str("S"), Value::str("M")],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: mail(): Sendmail exited with non-zero exit code 3 in /t.php on line 3\n"
        );
    }

    #[test]
    fn header_validation_errors() {
        let mut t = TestVm::new();
        let call = |t: &mut TestVm, h: Value| {
            thrown(t.call(
                "mail",
                vec![Value::str("a@b.c"), Value::str("S"), Value::str("M"), h],
            ))
        };
        let (c, m) = call(&mut t, arr(&[("To", Value::str("x"))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "ValueError",
                "The additional headers cannot contain the \"To\" header"
            )
        );
        let (c, m) = call(&mut t, arr(&[("Subject", Value::str("x"))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "ValueError",
                "The additional headers cannot contain the \"Subject\" header"
            )
        );
        let (_, m) = call(&mut t, arr(&[("X-A", Value::str("a\nb"))]));
        assert_eq!(
            m,
            "Header \"X-A\" contains LF character that is not allowed in the header"
        );
        let (_, m) = call(&mut t, arr(&[("X-A", Value::str("a\rb"))]));
        assert_eq!(
            m,
            "Header \"X-A\" contains CR character that is not allowed in the header"
        );
        let (_, m) = call(&mut t, arr(&[("X-A", Value::str("a\r\nb"))]));
        assert_eq!(
            m,
            "Header \"X-A\" contains CRLF characters that are used as a line separator and are not allowed in the header"
        );
        let (_, m) = call(&mut t, arr(&[("X-A", Value::bytes(b"a\0b"))]));
        assert_eq!(
            m,
            "Header \"X-A\" contains NULL character that is not allowed in the header"
        );
        let (c, m) = call(&mut t, arr(&[("X A", Value::str("v"))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "ValueError",
                "Header name \"X A\" contains invalid characters"
            )
        );
        let (c, m) = call(&mut t, arr(&[("X-A", Value::Int(1))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "TypeError",
                "Header \"X-A\" must be of type array|string, int given"
            )
        );
        let (c, m) = call(&mut t, arr(&[("From", arr(&[("0", Value::str("a"))]))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "TypeError",
                "Header \"from\" must be of type string, array given"
            )
        );
        let (c, m) = call(&mut t, arr(&[("X-A", arr(&[("k", Value::str("a"))]))]));
        assert_eq!(
            (c.as_str(), m.as_str()),
            (
                "TypeError",
                "Header \"X-A\" must only contain numeric keys, \"k\" found"
            )
        );
        let (_, m) = call(
            &mut t,
            arr(&[(
                "X-A",
                Value::array(PhpArray::from_values(vec![Value::Int(1)])),
            )]),
        );
        assert_eq!(
            m,
            "Header \"X-A\" must only contain values of type string, int found"
        );
        // folded values are fine
        assert!(matches!(
            check_field_value(b"a\r\n b"),
            HeaderValueError::None
        ));
        assert!(matches!(
            check_field_value(b"a\n b"),
            HeaderValueError::None
        ));
    }

    #[test]
    fn multiple_crlf_detection() {
        assert!(!detect_multiple_crlf(b"From: a\r\nX: b"));
        assert!(detect_multiple_crlf(b"From: a\r\n\r\nX: b"));
        assert!(detect_multiple_crlf(b"\r\nFrom: a"));
        assert!(detect_multiple_crlf(b"From: a\r\n"));
        assert!(detect_multiple_crlf(b":bad"));
    }

    #[test]
    fn malformed_additional_headers_warn() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("sendmail_path", "cat > /dev/null");
        let r = t
            .call(
                "mail",
                vec![
                    Value::str("a@b.c"),
                    Value::str("S"),
                    Value::str("M"),
                    Value::str("From: a\r\n\r\nX: b"),
                ],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: mail(): Multiple or malformed newlines found in additional_header in /t.php on line 3\n"
        );
    }

    #[test]
    fn to_and_subject_are_sanitised() {
        assert_eq!(sanitize_header_text(b"a@b.c \r\n"), b"a@b.c");
        assert_eq!(sanitize_header_text(b"x\ny"), b"x y");
        assert_eq!(
            sanitize_header_text(b"long\r\n continued"),
            b"long\r\n continued"
        );
        assert_eq!(sanitize_header_text(b""), b"");
    }

    #[test]
    fn escapeshellcmd_like_php() {
        assert_eq!(escape_shell_cmd(b"-f a@b.c"), b"-f a@b.c");
        assert_eq!(escape_shell_cmd(b"a;b|c"), b"a\\;b\\|c");
        assert_eq!(escape_shell_cmd(b"'quoted' x"), b"'quoted' x");
        assert_eq!(escape_shell_cmd(b"it's"), b"it\\'s");
    }

    #[test]
    fn mail_log_to_file() {
        let mut t = TestVm::new();
        let log = tmp("mail_log");
        let _ = std::fs::remove_file(&log);
        t.vm.ini_set_system("mail.log", log.to_str().unwrap());
        t.vm.ini_set_system("sendmail_path", "");
        t.call(
            "mail",
            vec![
                Value::str("a@b.c"),
                Value::str("Subj"),
                Value::str("M"),
                Value::str("X-H: 1\r\nX-I: 2"),
            ],
        )
        .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(
                "] mail() on [/t.php:3]: To: a@b.c -- Headers: X-H: 1  X-I: 2 -- Subject: Subj\n"
            ),
            "{text}"
        );
        let _ = std::fs::remove_file(&log);
    }

    #[test]
    fn null_bytes_are_rejected() {
        let mut t = TestVm::new();
        let (c, m) = thrown(t.call(
            "mail",
            vec![Value::bytes(b"a\0"), Value::str("S"), Value::str("M")],
        ));
        assert_eq!(c, "ValueError");
        assert_eq!(
            m,
            "mail(): Argument #1 ($to) must not contain any null bytes"
        );
        let (_, m) = thrown(t.call(
            "mail",
            vec![
                Value::str("a"),
                Value::str("S"),
                Value::str("M"),
                Value::bytes(b"H\0"),
            ],
        ));
        assert_eq!(
            m,
            "mail(): Argument #4 ($additional_headers) must not contain any null bytes"
        );
    }

    #[test]
    fn cr_lf_mode_directive_is_validated() {
        let mut t = TestVm::new();
        assert!(!t.vm.ini_set_system("no.such", "x"));
        assert_eq!(t.vm.ini_get("mail.cr_lf_mode").unwrap(), "crlf");
        assert_eq!(
            t.vm.ini_get("sendmail_path").unwrap(),
            DEFAULT_SENDMAIL_PATH
        );
    }
}
