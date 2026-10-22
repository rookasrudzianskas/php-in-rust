//! ext/standard/file.c

#![allow(clippy::collapsible_if)]

use std::process::{Command, Stdio};

use php_runtime::class::CastKind;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, Registry, ResRef, Value, Vm};

use super::streams::stream::{self, StreamKind, flags, open, opt};
use super::streams::wrappers::{self, StatResult};
use super::streams::{
    self as st, context_param, csv, handle_of, path_param, plain, stream_param, sys, with_stream,
};

pub const PHP_LOCK_SH: i64 = 1;
pub const PHP_LOCK_EX: i64 = 2;
pub const PHP_LOCK_UN: i64 = 3;
pub const PHP_LOCK_NB: i64 = 4;

pub const FILE_USE_INCLUDE_PATH: i64 = 1;
pub const FILE_IGNORE_NEW_LINES: i64 = 2;
pub const FILE_SKIP_EMPTY_LINES: i64 = 4;
pub const FILE_APPEND: i64 = 8;
pub const FILE_NO_DEFAULT_CONTEXT: i64 = 16;

fn value_or_false(v: Option<Value>) -> Value {
    v.unwrap_or(Value::Bool(false))
}

/// `php_flock_common`.
pub fn flock_common(
    vm: &mut Vm,
    res: &ResRef,
    operation: i64,
    arg_num: usize,
    wouldblock: Option<php_runtime::PhpRef>,
) -> PResult<Value> {
    let act = operation & PHP_LOCK_UN;
    if !(1..=3).contains(&act) {
        return Err(vm.argument_value_error(arg_num, "must be one of LOCK_SH, LOCK_EX, or LOCK_UN"));
    }
    if let Some(w) = &wouldblock {
        w.set(Value::Int(0));
    }
    let flock_values = [sys::LOCK_SH, sys::LOCK_EX, sys::LOCK_UN];
    let act = flock_values[act as usize - 1]
        | if operation & PHP_LOCK_NB != 0 {
            sys::LOCK_NB
        } else {
            0
        };
    let r = with_stream(vm, res, |vm, s| s.lock(vm, act))?.unwrap_or(-1);
    if r != 0 {
        if operation != 0 && wrappers::globals(vm).last_errno == sys::EWOULDBLOCK {
            if let Some(w) = &wouldblock {
                w.set(Value::Int(1));
            }
        }
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

fn flock(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let op = args.int(1);
    let wb = args.is_passed(2).then(|| args.by_ref(2));
    flock_common(vm, &res, op, 2, wb)
}

// ---------------------------------------------------------------------------
// get_meta_tags
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tok {
    Eof,
    OpenTag,
    CloseTag,
    Slash,
    Equal,
    Space,
    Id,
    Str,
    Other,
}

struct MetaData {
    res: ResRef,
    ulc: bool,
    lc: i32,
    token: Vec<u8>,
    in_meta: bool,
}

const META_DEF_BUFSIZE: usize = 8192;
const PHP_META_UNSAFE: &[u8] = b".\\+*?[^]$() ";
const PHP_META_HTML401_CHARS: &[u8] = b"-_.:";

fn md_eof(vm: &mut Vm, md: &MetaData) -> PResult<bool> {
    st::eof(vm, &md.res)
}

fn md_getc(vm: &mut Vm, md: &MetaData) -> PResult<i32> {
    Ok(with_stream(vm, &md.res, |vm, s| s.getc(vm))?
        .flatten()
        .map_or(-1, i32::from))
}

/// `php_next_meta_token`.
fn next_meta_token(vm: &mut Vm, md: &mut MetaData) -> PResult<Tok> {
    let mut ch: i32 = 0;
    loop {
        if !md.ulc {
            if md_eof(vm, md)? {
                break;
            }
            ch = md_getc(vm, md)?;
            if ch == 0 {
                break;
            }
        }
        if md_eof(vm, md)? {
            break;
        }
        if md.ulc {
            ch = md.lc;
            md.ulc = false;
        }
        match ch {
            0x3c => return Ok(Tok::OpenTag),
            0x3e => return Ok(Tok::CloseTag),
            0x3d => return Ok(Tok::Equal),
            0x2f => return Ok(Tok::Slash),
            0x27 | 0x22 => {
                let compliment = ch;
                let mut buff = Vec::new();
                loop {
                    if md_eof(vm, md)? {
                        break;
                    }
                    ch = md_getc(vm, md)?;
                    if ch == 0 || ch == compliment || ch == 0x3c || ch == 0x3e {
                        break;
                    }
                    buff.push(ch as u8);
                    if buff.len() == META_DEF_BUFSIZE {
                        break;
                    }
                }
                if ch == 0x3c || ch == 0x3e {
                    md.ulc = true;
                    md.lc = ch;
                }
                md.token = if md.in_meta { buff } else { Vec::new() };
                return Ok(Tok::Str);
            }
            0x0a | 0x0d | 0x09 => {}
            0x20 => return Ok(Tok::Space),
            _ => {
                if (0..256).contains(&ch) && (ch as u8).is_ascii_alphanumeric() {
                    let mut buff = vec![ch as u8];
                    loop {
                        if md_eof(vm, md)? {
                            break;
                        }
                        ch = md_getc(vm, md)?;
                        let c = ch as u8;
                        if ch == 0
                            || !((0..256).contains(&ch)
                                && (c.is_ascii_alphanumeric()
                                    || PHP_META_HTML401_CHARS.contains(&c)))
                        {
                            break;
                        }
                        buff.push(c);
                        if buff.len() == META_DEF_BUFSIZE {
                            break;
                        }
                    }
                    let c = ch as u8;
                    if !((0..256).contains(&ch) && c.is_ascii_alphabetic()) && ch != 0x2d {
                        md.ulc = true;
                        md.lc = ch;
                    }
                    md.token = buff;
                    return Ok(Tok::Id);
                }
                return Ok(Tok::Other);
            }
        }
    }
    Ok(Tok::Eof)
}

fn meta_unsafe(s: &[u8]) -> Vec<u8> {
    // the C string stops at the first NUL
    let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    s[..end]
        .iter()
        .map(|&c| {
            if PHP_META_UNSAFE.contains(&c) {
                b'_'
            } else {
                c
            }
        })
        .collect()
}

fn cstr_eq_ci(a: &[u8], b: &str) -> bool {
    let end = a.iter().position(|&c| c == 0).unwrap_or(a.len());
    a[..end].eq_ignore_ascii_case(b.as_bytes())
}

fn get_meta_tags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let use_include_path = args.bool(1);
    let opts = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
    let Some(res) = wrappers::open_wrapper(vm, &filename, b"rb", opts, None, None)? else {
        return Ok(Value::Bool(false));
    };
    let mut md = MetaData {
        res: res.clone(),
        ulc: false,
        lc: 0,
        token: Vec::new(),
        in_meta: false,
    };
    let mut out = PhpArray::new();
    let (mut in_tag, mut done) = (false, false);
    let (mut looking_for_val, mut have_name, mut have_content) = (false, false, false);
    let (mut saw_name, mut saw_content) = (false, false);
    let mut name: Option<Vec<u8>> = None;
    let mut value: Option<Vec<u8>> = None;
    let mut tok_last = Tok::Eof;
    while !done {
        let tok = next_meta_token(vm, &mut md)?;
        if tok == Tok::Eof {
            break;
        }
        match tok {
            Tok::Id => {
                if tok_last == Tok::OpenTag {
                    md.in_meta = cstr_eq_ci(&md.token, "meta");
                } else if tok_last == Tok::Slash && in_tag {
                    if cstr_eq_ci(&md.token, "head") {
                        done = true;
                    }
                } else if tok_last == Tok::Equal && looking_for_val {
                    if saw_name {
                        name = Some(meta_unsafe(&md.token));
                        have_name = true;
                    } else if saw_content {
                        value = Some(md.token.clone());
                        have_content = true;
                    }
                    looking_for_val = false;
                } else if md.in_meta {
                    if cstr_eq_ci(&md.token, "name") {
                        saw_name = true;
                        saw_content = false;
                        looking_for_val = true;
                    } else if cstr_eq_ci(&md.token, "content") {
                        saw_name = false;
                        saw_content = true;
                        looking_for_val = true;
                    }
                }
            }
            Tok::Str if tok_last == Tok::Equal && looking_for_val => {
                if saw_name {
                    name = Some(meta_unsafe(&md.token));
                    have_name = true;
                } else if saw_content {
                    value = Some(md.token.clone());
                    have_content = true;
                }
                looking_for_val = false;
            }
            Tok::OpenTag => {
                if looking_for_val {
                    looking_for_val = false;
                    have_name = false;
                    saw_name = false;
                    have_content = false;
                    saw_content = false;
                }
                in_tag = true;
            }
            Tok::CloseTag => {
                if have_name {
                    let n = name.take().unwrap_or_default().to_ascii_lowercase();
                    let v = if have_content {
                        let v = value.take().unwrap_or_default();
                        let end = v.iter().position(|&c| c == 0).unwrap_or(v.len());
                        v[..end].to_vec()
                    } else {
                        Vec::new()
                    };
                    out.insert(php_runtime::ArrayKey::from_bytes(&n), Value::from(v));
                }
                name = None;
                value = None;
                in_tag = false;
                looking_for_val = false;
                have_name = false;
                saw_name = false;
                have_content = false;
                saw_content = false;
                md.in_meta = false;
            }
            _ => {}
        }
        tok_last = tok;
        md.token.clear();
    }
    st::close(vm, &res)?;
    Ok(Value::from(out))
}

// ---------------------------------------------------------------------------
// file_get_contents / file_put_contents / file
// ---------------------------------------------------------------------------

fn file_get_contents(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let use_include_path = args.bool(1);
    let offset = args.int(3);
    let maxlen = match args.int_opt(4) {
        None => stream::COPY_ALL,
        Some(n) if n < 0 => {
            return Err(vm.argument_value_error(5, "must be greater than or equal to 0"));
        }
        Some(n) => n as usize,
    };
    let ctx = context_param(vm, args, 2, false)?;
    let opts = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
    let Some(res) = wrappers::open_wrapper(vm, &filename, b"rb", opts, None, ctx.as_ref())? else {
        return Ok(Value::Bool(false));
    };
    let r = with_stream(vm, &res, |vm, s| {
        if s.kind() == StreamKind::Stdio {
            s.set_option(
                vm,
                opt::READ_BUFFER,
                opt::BUFFER_NONE,
                &mut stream::OptParam::None,
            )?;
        }
        if offset != 0 && s.seek(vm, offset, if offset > 0 { 0 } else { 2 })? < 0 {
            return Ok(None);
        }
        Ok(Some(s.copy_to_mem(vm, maxlen)?.unwrap_or_default()))
    })?
    .flatten();
    st::close(vm, &res)?;
    match r {
        Some(data) => Ok(Value::from(data)),
        None => {
            vm.warning(format!("Failed to seek to position {offset} in the stream"))?;
            Ok(Value::Bool(false))
        }
    }
}

fn file_put_contents(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let data = args.get(1).unref();
    let flags = args.int(2);
    let srcstream = match &data {
        Value::Resource(r) => {
            if handle_of(r).is_none() {
                return Err(vm.throw_type_error(
                    "file_put_contents(): supplied resource is not a valid stream resource",
                ));
            }
            Some(r.clone())
        }
        _ => None,
    };
    let ctx = context_param(vm, args, 3, flags & FILE_NO_DEFAULT_CONTEXT != 0)?;
    let mut mode = b"wb".to_vec();
    if flags & FILE_APPEND != 0 {
        mode[0] = b'a';
    } else if flags & PHP_LOCK_EX != 0 {
        let f = filename.as_bytes();
        if f.windows(3).any(|w| w == b"://")
            && !(f.len() >= 7 && f[..7].eq_ignore_ascii_case(b"file://"))
        {
            vm.warning("Exclusive locks may only be set for regular files")?;
            return Ok(Value::Bool(false));
        }
        mode[0] = b'c';
    }
    let opts = open::REPORT_ERRORS
        | if flags & FILE_USE_INCLUDE_PATH != 0 {
            open::USE_PATH
        } else {
            0
        };
    let Some(res) = wrappers::open_wrapper(vm, &filename, &mode, opts, None, ctx.as_ref())? else {
        return Ok(Value::Bool(false));
    };
    if flags & PHP_LOCK_EX != 0 {
        let ok = with_stream(vm, &res, |vm, s| {
            Ok(s.supports_lock(vm)? && s.lock(vm, sys::LOCK_EX)? == 0)
        })?
        .unwrap_or(false);
        if !ok {
            st::close(vm, &res)?;
            vm.warning("Exclusive locks are not supported for this stream")?;
            return Ok(Value::Bool(false));
        }
    }
    if mode[0] == b'c' {
        with_stream(vm, &res, |vm, s| s.truncate_set_size(vm, 0))?;
    }
    let mut numbytes: i64 = 0;
    match &data {
        Value::Resource(_) => {
            let src = handle_of(srcstream.as_ref().expect("resource")).expect("checked");
            let dst = handle_of(&res).expect("open");
            let r = if std::rc::Rc::ptr_eq(&src, &dst) {
                let mut s = src.borrow_mut();
                stream::copy_to_stream(vm, &mut s, None, stream::COPY_ALL)?
            } else {
                let mut s = src.borrow_mut();
                let mut d = dst.borrow_mut();
                d.res = Some(res.clone());
                let r = stream::copy_to_stream(vm, &mut s, Some(&mut d), stream::COPY_ALL)?;
                d.res = None;
                r
            };
            numbytes = if r.0 { r.1 as i64 } else { -1 };
        }
        Value::Null
        | Value::Undef
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::String(_) => {
            let s = vm.to_php_string(&data)?;
            if !s.is_empty() {
                let n = st::write(vm, &res, &s)?.map_or(-1, |n| n as i64);
                numbytes = n;
                if n != -1 && n != s.len() as i64 {
                    vm.warning(format!(
                        "Only {n} of {} bytes written, possibly out of free disk space",
                        s.len()
                    ))?;
                    numbytes = -1;
                }
            }
        }
        Value::Array(a) => {
            for v in a.values() {
                let s = vm.to_php_string(&v.unref())?;
                if !s.is_empty() {
                    numbytes += s.len() as i64;
                    let w = st::write(vm, &res, &s)?;
                    if w != Some(s.len()) {
                        vm.warning(format!("Failed to write {} bytes to {}", s.len(), filename))?;
                        numbytes = -1;
                        break;
                    }
                }
            }
        }
        Value::Object(o) => match vm.object_cast(o, CastKind::String)? {
            Some(Value::String(s)) => {
                let n = st::write(vm, &res, &s)?.map_or(-1, |n| n as i64);
                numbytes = n;
                if n != -1 && n != s.len() as i64 {
                    vm.warning(format!(
                        "Only {n} of {} bytes written, possibly out of free disk space",
                        s.len()
                    ))?;
                    numbytes = -1;
                }
            }
            _ => numbytes = -1,
        },
        Value::Ref(_) => numbytes = -1,
    }
    let closed = st::close(vm, &res)?;
    if numbytes < 0 || !closed {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Int(numbytes))
}

fn file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let flags = args.int(1);
    if flags
        & !(FILE_USE_INCLUDE_PATH
            | FILE_IGNORE_NEW_LINES
            | FILE_SKIP_EMPTY_LINES
            | FILE_NO_DEFAULT_CONTEXT)
        != 0
    {
        return Err(vm.argument_value_error(2, "must be a valid flag value"));
    }
    let include_new_line = flags & FILE_IGNORE_NEW_LINES == 0;
    let skip_blank_lines = flags & FILE_SKIP_EMPTY_LINES != 0;
    let ctx = context_param(vm, args, 2, flags & FILE_NO_DEFAULT_CONTEXT != 0)?;
    let opts = open::REPORT_ERRORS
        | if flags & FILE_USE_INCLUDE_PATH != 0 {
            open::USE_PATH
        } else {
            0
        };
    let Some(res) = wrappers::open_wrapper(vm, &filename, b"rb", opts, None, ctx.as_ref())? else {
        return Ok(Value::Bool(false));
    };
    let mut out = PhpArray::new();
    let r = with_stream(vm, &res, |vm, s| {
        let Some(buf) = s.copy_to_mem(vm, stream::COPY_ALL)? else {
            return Ok(None);
        };
        let first_eol = s.locate_eol(Some(&buf));
        let mac = s.st.flags & flags::EOL_MAC != 0;
        Ok(Some((buf, first_eol, mac)))
    })?
    .flatten();
    if let Some((buf, first_eol, mac)) = r {
        let eol = if mac { b'\r' } else { b'\n' };
        let e = buf.len();
        let mut s = 0usize;
        let push = |out: &mut PhpArray, b: &[u8]| {
            let _ = out.append(Value::bytes(b));
        };
        match first_eol {
            None => push(&mut out, &buf[..]),
            Some(mut p) => {
                if include_new_line {
                    loop {
                        p += 1;
                        push(&mut out, &buf[s..p]);
                        s = p;
                        match buf[p..].iter().position(|&c| c == eol) {
                            Some(i) => p += i,
                            None => break,
                        }
                    }
                } else {
                    loop {
                        let windows_eol =
                            usize::from(p != 0 && eol == b'\n' && buf[p - 1] == b'\r');
                        if skip_blank_lines && p - s - windows_eol == 0 {
                            s = p + 1;
                            p += 1;
                        } else {
                            push(&mut out, &buf[s..p - windows_eol]);
                            s = p + 1;
                            p += 1;
                        }
                        if p > e {
                            break;
                        }
                        match buf[p..].iter().position(|&c| c == eol) {
                            Some(i) => p += i,
                            None => break,
                        }
                    }
                }
                // leftovers of files without a final newline
                if s < e {
                    push(&mut out, &buf[s..e]);
                }
            }
        }
    }
    st::close(vm, &res)?;
    Ok(Value::from(out))
}

// ---------------------------------------------------------------------------
// tempnam / tmpfile / fopen / fclose / popen / pclose
// ---------------------------------------------------------------------------

/// `php_basename(s, len, NULL, 0)` for ASCII-compatible locales.
fn basename(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && s[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"";
    }
    let start = s[..end]
        .iter()
        .rposition(|&c| c == b'/')
        .map_or(0, |i| i + 1);
    &s[start..end]
}

fn tempnam(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dir = path_param(vm, args, 0)?;
    let prefix = path_param(vm, args, 1)?;
    let mut p = basename(&prefix).to_vec();
    if p.len() >= 64 {
        p.truncate(63);
    }
    match plain::open_temporary_fd(
        vm,
        Some(&dir),
        &p,
        plain::TMP_FILE_OPEN_BASEDIR_CHECK_ON_FALLBACK
            | plain::TMP_FILE_OPEN_BASEDIR_CHECK_ON_EXPLICIT_DIR,
    )? {
        Some((_f, path)) => Ok(Value::String(path)),
        None => Ok(Value::Bool(false)),
    }
}

fn tmpfile(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    st::process_deferred_closes(vm)?;
    Ok(value_or_false(
        plain::fopen_temporary_file(vm, None, b"php")?.map(Value::Resource),
    ))
}

fn fopen(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let mode = args.str(1).clone();
    let use_include_path = args.bool(2);
    let ctx = context_param(vm, args, 3, false)?;
    let opts = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
    Ok(value_or_false(
        wrappers::open_wrapper(vm, &filename, &mode, opts, None, ctx.as_ref())?
            .map(Value::Resource),
    ))
}

fn fclose(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let no_fclose = handle_of(&res).is_some_and(|h| h.borrow().st.flags & flags::NO_FCLOSE != 0);
    if no_fclose {
        vm.warning("cannot close the provided stream, as it must not be manually closed")?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(st::close(vm, &res)?))
}

fn popen(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let command = path_param(vm, args, 0)?;
    let mode = args.str(1).clone();
    let mut posix_mode = mode.to_vec();
    if let Some(i) = posix_mode.iter().position(|&c| c == b'b') {
        posix_mode.remove(i);
    }
    let valid = match posix_mode.as_slice() {
        [b'r'] | [b'w'] => true,
        [a, b] => matches!((a, b), (b'r', b'b') | (b'w', b'b')),
        _ => false,
    };
    if !valid {
        return Err(vm.argument_value_error(2, "must be one of \"r\", \"rb\", \"w\", or \"wb\""));
    }
    vm.sapi_flush();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(std::ffi::OsStr::from_bytes_compat(&command));
    let reading = posix_mode[0] == b'r';
    if reading {
        cmd.stdout(Stdio::piped());
    } else {
        cmd.stdin(Stdio::piped());
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let n = sys::errno_of(&e);
            vm.warning(sys::strerror(n))?;
            return Ok(Value::Bool(false));
        }
    };
    let fd: std::os::fd::OwnedFd = if reading {
        child.stdout.take().expect("piped").into()
    } else {
        child.stdin.take().expect("piped").into()
    };
    Ok(Value::Resource(plain::fopen_from_pipe(
        vm, fd, child, &mode,
    )))
}

trait OsStrCompat {
    fn from_bytes_compat(b: &[u8]) -> &std::ffi::OsStr;
}

impl OsStrCompat for std::ffi::OsStr {
    fn from_bytes_compat(b: &[u8]) -> &std::ffi::OsStr {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b)
    }
}

fn pclose(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let r = with_stream(vm, &res, |vm, s| s.free(vm, stream::PHP_STREAM_FREE_CLOSE))?;
    res.close();
    Ok(Value::Int(r.unwrap_or(-1) as i64))
}

// ---------------------------------------------------------------------------
// Reading and writing
// ---------------------------------------------------------------------------

fn feof(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    Ok(Value::Bool(st::eof(vm, &res)?))
}

fn fgets(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let line = match args.int_opt(1) {
        None => st::read_line(vm, &res, None)?,
        Some(len) => {
            if len <= 0 {
                return Err(vm.argument_value_error(2, "must be greater than 0"));
            }
            st::read_line(vm, &res, Some(len as usize))?
        }
    };
    Ok(value_or_false(line.map(Value::from)))
}

fn fgetc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let c = with_stream(vm, &res, |vm, s| s.getc(vm))?.flatten();
    Ok(value_or_false(c.map(|c| Value::bytes(&[c]))))
}

fn fscanf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    let res = match &v {
        Value::Resource(r) if handle_of(r).is_some() => r.clone(),
        Value::Resource(_) => {
            return Err(vm.throw_type_error(
                "fscanf(): supplied resource is not a valid File-Handle resource",
            ));
        }
        other => {
            return Err(vm.argument_type_error(
                1,
                &format!("must be of type resource, {} given", other.value_name()),
            ));
        }
    };
    let format = args.str(1).clone();
    let Some(line) = st::read_line(vm, &res, None)? else {
        return Ok(Value::Bool(false));
    };
    let mut call = vec![Value::from(line), Value::String(format)];
    call.extend(args.rest(2).iter().cloned());
    vm.call_native("sscanf", call)
}

fn fwrite(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let input = args.str(1).clone();
    let num_bytes = match args.int_opt(2) {
        None => input.len(),
        Some(n) if n <= 0 => 0,
        Some(n) => (n as usize).min(input.len()),
    };
    if num_bytes == 0 {
        return Ok(Value::Int(0));
    }
    let r = with_stream(vm, &res, |vm, s| s.write(vm, &input[..num_bytes]))?.unwrap_or(-1);
    if r < 0 {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Int(r as i64))
}

fn fflush(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    Ok(Value::Bool(st::flush(vm, &res)?))
}

fn rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    Ok(Value::Bool(st::seek(vm, &res, 0, 0)? != -1))
}

fn ftell(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    match st::tell(&res) {
        Some(p) if p != -1 => Ok(Value::Int(p)),
        _ => Ok(Value::Bool(false)),
    }
}

fn fseek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let offset = args.int(1);
    let whence = args.int(2);
    Ok(Value::Int(st::seek(vm, &res, offset, whence as i32)? as i64))
}

fn fread(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let len = args.int(1);
    if len <= 0 {
        return Err(vm.argument_value_error(2, "must be greater than 0"));
    }
    Ok(value_or_false(
        st::read(vm, &res, len as usize)?.map(Value::from),
    ))
}

fn fpassthru(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let n = with_stream(vm, &res, |vm, s| s.passthru(vm))?.unwrap_or(0);
    Ok(Value::Int(n as i64))
}

fn readfile(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let use_include_path = args.bool(1);
    let ctx = context_param(vm, args, 2, false)?;
    let opts = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
    let Some(res) = wrappers::open_wrapper(vm, &filename, b"rb", opts, None, ctx.as_ref())? else {
        return Ok(Value::Bool(false));
    };
    let n = with_stream(vm, &res, |vm, s| s.passthru(vm))?.unwrap_or(0);
    st::close(vm, &res)?;
    Ok(Value::Int(n as i64))
}

fn ftruncate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let size = args.int(1);
    if size < 0 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
    }
    let r = with_stream(vm, &res, |vm, s| {
        if !s.truncate_supported(vm)? {
            return Ok(None);
        }
        Ok(Some(s.truncate_set_size(vm, size)? == 0))
    })?
    .flatten();
    match r {
        Some(b) => Ok(Value::Bool(b)),
        None => {
            vm.warning("Can't truncate this stream!")?;
            Ok(Value::Bool(false))
        }
    }
}

fn do_sync(vm: &mut Vm, args: &mut Args, data_only: bool) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    let r = with_stream(vm, &res, |vm, s| {
        if !s.sync_supported(vm)? {
            return Ok(None);
        }
        Ok(Some(s.sync(vm, data_only)? == 0))
    })?
    .flatten();
    match r {
        Some(b) => Ok(Value::Bool(b)),
        None => {
            vm.warning("Can't fsync this stream!")?;
            Ok(Value::Bool(false))
        }
    }
}

fn fsync(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_sync(vm, args, false)
}

fn fdatasync(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_sync(vm, args, true)
}

/// `php_fstat`.
fn fstat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    Ok(value_or_false(
        st::stat(vm, &res)?.map(|sb| Value::from(sb.to_php_array())),
    ))
}

// ---------------------------------------------------------------------------
// Filesystem operations
// ---------------------------------------------------------------------------

fn mkdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dir = path_param(vm, args, 0)?;
    let mode = args.int(1);
    let recursive = args.bool(2);
    let ctx = context_param(vm, args, 3, false)?;
    let opts = open::REPORT_ERRORS
        | if recursive {
            wrappers::MKDIR_RECURSIVE
        } else {
            0
        };
    Ok(Value::Bool(wrappers::mkdir(
        vm,
        &dir,
        mode,
        opts,
        ctx.as_ref(),
    )?))
}

fn rmdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dir = path_param(vm, args, 0)?;
    let ctx = context_param(vm, args, 1, false)?;
    Ok(Value::Bool(wrappers::rmdir(
        vm,
        &dir,
        open::REPORT_ERRORS,
        ctx.as_ref(),
    )?))
}

fn umask(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mask = args.int_opt(0);
    let oldumask = sys::sys_umask(0o077);
    let g = wrappers::globals(vm);
    if g.saved_umask.is_none() {
        g.saved_umask = Some(oldumask);
    }
    match mask {
        None => sys::sys_umask(oldumask),
        Some(m) => sys::sys_umask(m as u32),
    };
    Ok(Value::Int(oldumask as i64))
}

fn rename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let old_name = path_param(vm, args, 0)?;
    let new_name = path_param(vm, args, 1)?;
    let ctx = context_param(vm, args, 2, false)?;
    let Some(w) = wrappers::locate_url_wrapper(vm, &old_name, 0)? else {
        vm.warning("Unable to locate stream wrapper")?;
        return Ok(Value::Bool(false));
    };
    if !w.wrapper.has_rename() {
        vm.warning(format!(
            "{} wrapper does not support renaming",
            w.wrapper.label()
        ))?;
        return Ok(Value::Bool(false));
    }
    let other = wrappers::locate_url_wrapper(vm, &new_name, 0)?;
    if !other.is_some_and(|o| std::rc::Rc::ptr_eq(&o.wrapper, &w.wrapper)) {
        vm.warning("Cannot rename a file across wrapper types")?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(w.wrapper.rename(
        vm,
        &old_name,
        &new_name,
        open::REPORT_ERRORS,
        ctx.as_ref(),
    )?))
}

fn unlink(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let ctx = context_param(vm, args, 1, false)?;
    let Some(w) = wrappers::locate_url_wrapper(vm, &filename, 0)? else {
        vm.warning("Unable to locate stream wrapper")?;
        return Ok(Value::Bool(false));
    };
    if !w.wrapper.has_unlink() {
        vm.warning(format!("{} does not allow unlinking", w.wrapper.label()))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(w.wrapper.unlink(
        vm,
        &filename,
        open::REPORT_ERRORS,
        ctx.as_ref(),
    )?))
}

/// `php_copy_file_ctx`.
pub fn copy_file(
    vm: &mut Vm,
    src: &[u8],
    dest: &[u8],
    src_flags: i32,
    ctx: Option<&ResRef>,
) -> PResult<bool> {
    let src_stat_flags = if src_flags & open::STREAM_DISABLE_OPEN_BASEDIR != 0 {
        4
    } else {
        0
    };
    'check: {
        let src_s = match wrappers::stat_path(vm, src, src_stat_flags, ctx)? {
            StatResult::Unsupported => break 'check,
            StatResult::Failed => return Ok(false),
            StatResult::Ok(sb) => sb,
        };
        if src_s.is_dir() {
            vm.warning("The first argument to copy() function cannot be a directory")?;
            return Ok(false);
        }
        let dest_s = match wrappers::stat_path(vm, dest, wrappers::URL_STAT_QUIET, ctx)? {
            StatResult::Unsupported | StatResult::Failed => break 'check,
            StatResult::Ok(sb) => sb,
        };
        if dest_s.is_dir() {
            vm.warning("The second argument to copy() function cannot be a directory")?;
            return Ok(false);
        }
        if src_s.ino == 0 || dest_s.ino == 0 {
            let Some(sp) = plain::expand_filepath(src) else {
                return Ok(false);
            };
            let Some(dp) = plain::expand_filepath(dest) else {
                break 'check;
            };
            if sp == dp {
                return Ok(false);
            }
        } else if src_s.ino == dest_s.ino && src_s.dev == dest_s.dev {
            return Ok(false);
        }
    }
    let Some(srcstream) =
        wrappers::open_wrapper(vm, src, b"rb", src_flags | open::REPORT_ERRORS, None, ctx)?
    else {
        return Ok(false);
    };
    let deststream = wrappers::open_wrapper(vm, dest, b"wb", open::REPORT_ERRORS, None, ctx)?;
    let mut ret = false;
    if let Some(d) = &deststream {
        let sh = handle_of(&srcstream).expect("open");
        let dh = handle_of(d).expect("open");
        let mut s = sh.borrow_mut();
        let mut dd = dh.borrow_mut();
        ret = stream::copy_to_stream(vm, &mut s, Some(&mut dd), stream::COPY_ALL)?.0;
    }
    st::close(vm, &srcstream)?;
    if let Some(d) = &deststream {
        if !st::close(vm, d)? {
            ret = false;
        }
    }
    Ok(ret)
}

fn copy(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let source = path_param(vm, args, 0)?;
    let target = path_param(vm, args, 1)?;
    let ctx = context_param(vm, args, 2, false)?;
    if let Some(l) = wrappers::locate_url_wrapper(vm, &source, 0)? {
        if l.wrapper.label() == "plainfile" && plain::check_open_basedir(vm, &source, true)? {
            return Ok(Value::Bool(false));
        }
    }
    Ok(Value::Bool(copy_file(
        vm,
        &source,
        &target,
        0,
        ctx.as_ref(),
    )?))
}

fn realpath(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    match plain::real_path(&filename) {
        Some(p) => {
            if plain::check_open_basedir(vm, &p, true)? {
                return Ok(Value::Bool(false));
            }
            Ok(Value::String(p))
        }
        None => Ok(Value::Bool(false)),
    }
}

const MAXPATHLEN: usize = if cfg!(target_os = "linux") {
    4096
} else {
    1024
};

fn fnmatch(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pattern = path_param(vm, args, 0)?;
    let filename = path_param(vm, args, 1)?;
    let flags = args.int(2);
    if filename.len() >= MAXPATHLEN {
        vm.warning(format!(
            "Filename exceeds the maximum allowed length of {MAXPATHLEN} characters"
        ))?;
        return Ok(Value::Bool(false));
    }
    if pattern.len() >= MAXPATHLEN {
        vm.warning(format!(
            "Pattern exceeds the maximum allowed length of {MAXPATHLEN} characters"
        ))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(sys::sys_fnmatch(
        &pattern,
        &filename,
        flags as i32,
    )))
}

/// Stream access for the shared CSV code.
struct ResCsv(ResRef);

impl csv::CsvStream for ResCsv {
    fn get_line(&mut self, vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
        st::read_line(vm, &self.0, maxlen)
    }

    fn write(&mut self, vm: &mut Vm, data: &[u8]) -> PResult<Option<usize>> {
        st::write(vm, &self.0, data)
    }
}

fn fgetcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    csv::fgetcsv_impl(vm, args, &mut ResCsv(res))
}

fn fputcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_param(vm, args, 0)?;
    csv::fputcsv_impl(vm, args, &mut ResCsv(res))
}

fn sys_get_temp_dir(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::String(plain::temporary_directory(vm)))
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
        const ALL: &[&NativeFunction] = &[$(&$name),*];
    };
}

natives! {
    FLOCK: "flock($stream, int $operation, &$would_block = null): bool" => flock;
    GET_META_TAGS: "get_meta_tags(string $filename, bool $use_include_path = false): array|false" => get_meta_tags;
    PCLOSE: "pclose($handle): int" => pclose;
    POPEN: "popen(string $command, string $mode)" => popen;
    READFILE: "readfile(string $filename, bool $use_include_path = false, $context = null): int|false" => readfile;
    REWIND: "rewind($stream): bool" => rewind;
    RMDIR: "rmdir(string $directory, $context = null): bool" => rmdir;
    UMASK: "umask(?int $mask = null): int" => umask;
    FCLOSE: "fclose($stream): bool" => fclose;
    FEOF: "feof($stream): bool" => feof;
    FGETC: "fgetc($stream): string|false" => fgetc;
    FGETS: "fgets($stream, ?int $length = null): string|false" => fgets;
    FREAD: "fread($stream, int $length): string|false" => fread;
    FOPEN: "fopen(string $filename, string $mode, bool $use_include_path = false, $context = null)" => fopen;
    FSCANF: "fscanf($stream, string $format, mixed &...$vars): array|int|false|null" => fscanf;
    FPASSTHRU: "fpassthru($stream): int" => fpassthru;
    FTRUNCATE: "ftruncate($stream, int $size): bool" => ftruncate;
    FSTAT: "fstat($stream): array|false" => fstat;
    FSEEK: "fseek($stream, int $offset, int $whence = SEEK_SET): int" => fseek;
    FTELL: "ftell($stream): int|false" => ftell;
    FFLUSH: "fflush($stream): bool" => fflush;
    FSYNC: "fsync($stream): bool" => fsync;
    FDATASYNC: "fdatasync($stream): bool" => fdatasync;
    FWRITE: "fwrite($stream, string $data, ?int $length = null): int|false" => fwrite;
    FPUTS: "fputs($stream, string $data, ?int $length = null): int|false" => fwrite;
    MKDIR: "mkdir(string $directory, int $permissions = 0777, bool $recursive = false, $context = null): bool" => mkdir;
    RENAME: "rename(string $from, string $to, $context = null): bool" => rename;
    COPY: "copy(string $from, string $to, $context = null): bool" => copy;
    TEMPNAM: "tempnam(string $directory, string $prefix): string|false" => tempnam;
    TMPFILE: "tmpfile()" => tmpfile;
    FILE: "file(string $filename, int $flags = 0, $context = null): array|false" => file;
    FILE_GET_CONTENTS: "file_get_contents(string $filename, bool $use_include_path = false, $context = null, int $offset = 0, ?int $length = null): string|false" => file_get_contents;
    UNLINK: "unlink(string $filename, $context = null): bool" => unlink;
    FILE_PUT_CONTENTS: "file_put_contents(string $filename, mixed $data, int $flags = 0, $context = null): int|false" => file_put_contents;
    FPUTCSV: "fputcsv($stream, array $fields, string $separator = \",\", string $enclosure = \"\\\"\", string $escape = \"\\\\\", string $eol = \"\\n\"): int|false" => fputcsv;
    FGETCSV: "fgetcsv($stream, ?int $length = null, string $separator = \",\", string $enclosure = \"\\\"\", string $escape = \"\\\\\"): array|false" => fgetcsv;
    REALPATH: "realpath(string $path): string|false" => realpath;
    FNMATCH: "fnmatch(string $pattern, string $filename, int $flags = 0): bool" => fnmatch;
    SYS_GET_TEMP_DIR: "sys_get_temp_dir(): string" => sys_get_temp_dir;
}

pub fn register(r: &mut Registry) {
    for (n, v) in [
        ("SEEK_SET", 0i64),
        ("SEEK_CUR", 1),
        ("SEEK_END", 2),
        ("LOCK_SH", PHP_LOCK_SH),
        ("LOCK_EX", PHP_LOCK_EX),
        ("LOCK_UN", PHP_LOCK_UN),
        ("LOCK_NB", PHP_LOCK_NB),
        ("STREAM_NOTIFY_CONNECT", 2),
        ("STREAM_NOTIFY_AUTH_REQUIRED", 3),
        ("STREAM_NOTIFY_AUTH_RESULT", 10),
        ("STREAM_NOTIFY_MIME_TYPE_IS", 4),
        ("STREAM_NOTIFY_FILE_SIZE_IS", 5),
        ("STREAM_NOTIFY_REDIRECTED", 6),
        ("STREAM_NOTIFY_PROGRESS", 7),
        ("STREAM_NOTIFY_FAILURE", 9),
        ("STREAM_NOTIFY_COMPLETED", 8),
        ("STREAM_NOTIFY_RESOLVE", 1),
        ("STREAM_NOTIFY_SEVERITY_INFO", 0),
        ("STREAM_NOTIFY_SEVERITY_WARN", 1),
        ("STREAM_NOTIFY_SEVERITY_ERR", 2),
        ("STREAM_FILTER_READ", 1),
        ("STREAM_FILTER_WRITE", 2),
        ("STREAM_FILTER_ALL", 3),
        ("STREAM_CLIENT_PERSISTENT", 1),
        ("STREAM_CLIENT_ASYNC_CONNECT", 2),
        ("STREAM_CLIENT_CONNECT", 4),
        ("STREAM_CRYPTO_METHOD_ANY_CLIENT", 127),
        ("STREAM_CRYPTO_METHOD_SSLv2_CLIENT", 3),
        ("STREAM_CRYPTO_METHOD_SSLv3_CLIENT", 5),
        ("STREAM_CRYPTO_METHOD_SSLv23_CLIENT", 57),
        ("STREAM_CRYPTO_METHOD_TLS_CLIENT", 121),
        ("STREAM_CRYPTO_METHOD_TLSv1_0_CLIENT", 9),
        ("STREAM_CRYPTO_METHOD_TLSv1_1_CLIENT", 17),
        ("STREAM_CRYPTO_METHOD_TLSv1_2_CLIENT", 33),
        ("STREAM_CRYPTO_METHOD_TLSv1_3_CLIENT", 65),
        ("STREAM_CRYPTO_METHOD_ANY_SERVER", 126),
        ("STREAM_CRYPTO_METHOD_SSLv2_SERVER", 2),
        ("STREAM_CRYPTO_METHOD_SSLv3_SERVER", 4),
        ("STREAM_CRYPTO_METHOD_SSLv23_SERVER", 120),
        ("STREAM_CRYPTO_METHOD_TLS_SERVER", 120),
        ("STREAM_CRYPTO_METHOD_TLSv1_0_SERVER", 8),
        ("STREAM_CRYPTO_METHOD_TLSv1_1_SERVER", 16),
        ("STREAM_CRYPTO_METHOD_TLSv1_2_SERVER", 32),
        ("STREAM_CRYPTO_METHOD_TLSv1_3_SERVER", 64),
        ("STREAM_CRYPTO_PROTO_SSLv3", 4),
        ("STREAM_CRYPTO_PROTO_TLSv1_0", 8),
        ("STREAM_CRYPTO_PROTO_TLSv1_1", 16),
        ("STREAM_CRYPTO_PROTO_TLSv1_2", 32),
        ("STREAM_CRYPTO_PROTO_TLSv1_3", 64),
        ("STREAM_CRYPTO_STATUS_NONE", 0),
        ("STREAM_CRYPTO_STATUS_WANT_READ", 1),
        ("STREAM_CRYPTO_STATUS_WANT_WRITE", 2),
        ("STREAM_SHUT_RD", 0),
        ("STREAM_SHUT_WR", 1),
        ("STREAM_SHUT_RDWR", 2),
        ("STREAM_PF_INET", 2),
        (
            "STREAM_PF_INET6",
            if cfg!(target_os = "linux") { 10 } else { 30 },
        ),
        ("STREAM_PF_UNIX", 1),
        ("STREAM_IPPROTO_IP", 0),
        ("STREAM_IPPROTO_TCP", 6),
        ("STREAM_IPPROTO_UDP", 17),
        ("STREAM_IPPROTO_ICMP", 1),
        ("STREAM_IPPROTO_RAW", 255),
        ("STREAM_SOCK_STREAM", 1),
        ("STREAM_SOCK_DGRAM", 2),
        ("STREAM_SOCK_RAW", 3),
        ("STREAM_SOCK_SEQPACKET", 5),
        ("STREAM_SOCK_RDM", 4),
        ("STREAM_PEEK", 2),
        ("STREAM_OOB", 1),
        ("STREAM_SERVER_BIND", 4),
        ("STREAM_SERVER_LISTEN", 8),
        ("FILE_USE_INCLUDE_PATH", FILE_USE_INCLUDE_PATH),
        ("FILE_IGNORE_NEW_LINES", FILE_IGNORE_NEW_LINES),
        ("FILE_SKIP_EMPTY_LINES", FILE_SKIP_EMPTY_LINES),
        ("FILE_APPEND", FILE_APPEND),
        ("FILE_NO_DEFAULT_CONTEXT", FILE_NO_DEFAULT_CONTEXT),
        ("FNM_NOESCAPE", sys::fnm::NOESCAPE),
        ("FNM_PATHNAME", sys::fnm::PATHNAME),
        ("FNM_PERIOD", sys::fnm::PERIOD),
        ("FNM_CASEFOLD", sys::fnm::CASEFOLD),
    ] {
        r.constant(n, v);
    }
    r.deprecated_constant(
        "FILE_TEXT",
        0i64,
        "since 8.1, as the constant has no effect",
    );
    r.deprecated_constant(
        "FILE_BINARY",
        0i64,
        "since 8.1, as the constant has no effect",
    );
    r.functions(ALL);
}

#[cfg(test)]
mod tests;
