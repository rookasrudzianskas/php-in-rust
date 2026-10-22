//! `SplFileObject` and `SplTempFileObject` (spl_directory.c).
//!
//! Line reading follows `spl_filesystem_file_read_ex` /
//! `spl_filesystem_file_read_line`: `current_line` holds the line read
//! ahead (or by `current()`), `current_line_num` is what `key()` reports,
//! and it only advances when a line is read while another one is held, on
//! `next()`, on `fgets()` and on newlines read by `fgetc()`.

use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpStr, ResRef, Value, Vm};

use super::{
    DROP_NEW_LINE, FILE_OBJECT_MASK, FsObject, FsType, READ_AHEAD, READ_CSV, SKIP_EMPTY,
    deprecated_method, is_slash, not_initialized, path_arg, resource_or_null, with_error_handling,
    with_fs,
};
use crate::spl::engine::{self, this};
use crate::standard::file::flock_common;
use crate::standard::filestat::{FS_IS_DIR, php_stat};
use crate::standard::scanf::php_sscanf_internal;
use crate::standard::streams::csv::{self, CSV_NO_ESCAPE, CsvStream};
use crate::standard::streams::stream::{flags, open};
use crate::standard::streams::{self, context, handle_of, with_stream, wrappers};

/// `SEEK_SET`.
const SEEK_SET: i32 = 0;

/// Stream access for the shared CSV code.
struct ResCsv(ResRef);

impl CsvStream for ResCsv {
    fn get_line(&mut self, vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
        streams::read_line(vm, &self.0, maxlen)
    }

    fn write(&mut self, vm: &mut Vm, data: &[u8]) -> PResult<Option<usize>> {
        streams::write(vm, &self.0, data)
    }
}

/// `spl_filesystem_file_open`: opens `file_name` with `open_mode` (set by
/// the caller) and initialises the file state. Warnings are reported; the
/// callers turn them into `RuntimeException`s.
pub(super) fn file_open(vm: &mut Vm, obj: &ObjRef, use_include_path: bool) -> PResult<()> {
    let (file_name, mode, zcontext) = with_fs(obj, |st| {
        st.ty = FsType::File;
        (
            st.file_name.clone().unwrap_or_default(),
            st.open_mode.clone().unwrap_or_default(),
            st.zcontext.clone(),
        )
    });
    let reset = |obj: &ObjRef| {
        with_fs(obj, |st| {
            st.open_mode = None;
            st.file_name = None;
        })
    };
    // A warning of php_stat() (open_basedir) is the pending exception that
    // the open's own warnings do not overwrite.
    let is_dir = match php_stat(vm, &file_name, FS_IS_DIR) {
        Ok(v) => v.identical(&Value::Bool(true)),
        Err(e) => {
            reset(obj);
            return Err(e);
        }
    };
    if is_dir {
        reset(obj);
        return Err(vm.throw_error(
            "LogicException",
            "Cannot use SplFileObject with directories",
        ));
    }
    let zcontext = zcontext.map_or(Value::Null, Value::Resource);
    let opened = context::from_zval(vm, &zcontext, false).and_then(|ctx| {
        let options = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
        wrappers::open_wrapper(vm, &file_name, &mode, options, None, ctx.as_ref())
    });
    let res = match opened {
        Ok(Some(res)) if !file_name.is_empty() => res,
        Ok(_) => {
            reset(obj);
            let mut msg = b"Cannot open file '".to_vec();
            msg.extend_from_slice(&file_name);
            msg.push(b'\'');
            return Err(vm.throw_error("RuntimeException", PhpStr::from_vec(msg)));
        }
        Err(e) => {
            reset(obj);
            return Err(e);
        }
    };
    // prevent closing the stream outside of SplFileObject
    let orig_path = handle_of(&res).and_then(|h| {
        let mut s = h.try_borrow_mut().ok()?;
        s.st.flags |= flags::NO_FCLOSE;
        s.orig_path.clone()
    });
    with_fs(obj, |st| {
        let len = file_name.len();
        st.file_name = Some(if len > 1 && is_slash(file_name[len - 1]) {
            PhpStr::from_bytes(&file_name[..len - 1])
        } else {
            file_name.clone()
        });
        st.orig_path = Some(orig_path.unwrap_or_else(|| file_name.clone()));
        st.stream = Some(res);
        st.delimiter = b',';
        st.enclosure = b'"';
        st.escape = i32::from(b'\\');
        st.is_escape_default = true;
    });
    Ok(())
}

/// `CHECK_SPL_FILE_OBJECT_IS_INITIALIZED`: the open stream.
fn stream_of(vm: &mut Vm, obj: &ObjRef) -> PResult<ResRef> {
    with_fs(obj, |st| st.stream.clone()).ok_or_else(|| not_initialized(vm))
}

/// `spl_filesystem_file_free_line`.
fn free_line(st: &mut FsObject) {
    st.current_line = None;
    st.current_zval = None;
}

/// `spl_filesystem_file_cannot_read`.
fn cannot_read(vm: &mut Vm, obj: &ObjRef) -> php_runtime::Flow {
    let name = with_fs(obj, |st| st.file_name.clone().unwrap_or_default());
    let mut msg = b"Cannot read from file ".to_vec();
    msg.extend_from_slice(&name);
    vm.throw_error("RuntimeException", PhpStr::from_vec(msg))
}

fn has_flag(obj: &ObjRef, flag: i64) -> bool {
    with_fs(obj, |st| st.flags & flag != 0)
}

/// `spl_filesystem_file_read_ex`: reads the next line into `current_line`
/// (`Ok(false)` at EOF when `silent`, else "Cannot read from file").
fn read_ex(vm: &mut Vm, obj: &ObjRef, silent: bool, line_add: i64, csv: bool) -> PResult<bool> {
    let (res, max_len, file_flags) = with_fs(obj, |st| {
        free_line(st);
        (st.stream.clone(), st.max_line_len, st.flags)
    });
    let Some(res) = res else {
        return Err(not_initialized(vm));
    };
    if streams::eof(vm, &res)? {
        return if silent {
            Ok(false)
        } else {
            Err(cannot_read(vm, obj))
        };
    }
    let maxlen = (max_len > 0).then(|| max_len.saturating_add(1));
    let Some(mut buf) = streams::read_line(vm, &res, maxlen)? else {
        return if silent {
            Ok(false)
        } else {
            Err(cannot_read(vm, obj))
        };
    };
    if !csv && file_flags & DROP_NEW_LINE != 0 && buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    with_fs(obj, |st| {
        st.current_line = Some(PhpStr::from_vec(buf));
        st.current_line_num += line_add;
    });
    Ok(true)
}

/// `spl_filesystem_file_read`: the line number advances when a line was
/// held.
fn read(vm: &mut Vm, obj: &ObjRef, silent: bool, csv: bool) -> PResult<bool> {
    let line_add = with_fs(obj, |st| i64::from(st.current_line.is_some()));
    read_ex(vm, obj, silent, line_add, csv)
}

/// `is_line_empty`.
fn is_line_empty(st: &FsObject) -> bool {
    let line = st.current_line.as_deref().unwrap_or_default();
    line.is_empty()
        || (st.flags & READ_CSV != 0
            && st.flags & DROP_NEW_LINE != 0
            && (line == b"\n" || line == b"\r\n"))
}

/// `spl_filesystem_file_read_csv`: the parsed row (`None` at EOF).
fn read_csv(
    vm: &mut Vm,
    obj: &ObjRef,
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    silent: bool,
) -> PResult<Option<Value>> {
    loop {
        if !read(vm, obj, silent, true)? {
            return Ok(None);
        }
        if !with_fs(obj, |st| is_line_empty(st) && st.flags & SKIP_EMPTY != 0) {
            break;
        }
    }
    // php_fgetcsv() may read more lines (an enclosure spanning lines); the
    // current line keeps the first one.
    let (buf, res) = with_fs(obj, |st| {
        st.current_zval = None;
        (
            st.current_line.clone().unwrap_or_default(),
            st.stream.clone(),
        )
    });
    let Some(res) = res else {
        return Err(not_initialized(vm));
    };
    let mut stream = ResCsv(res);
    let values = csv::parse(
        vm,
        Some(&mut stream),
        delimiter,
        enclosure,
        escape,
        buf.to_vec(),
    )?
    .unwrap_or_else(csv::bc_empty_line);
    let row = Value::array(values);
    with_fs(obj, |st| st.current_zval = Some(row.clone()));
    Ok(Some(row))
}

/// `spl_filesystem_file_read_line_ex`: CSV, an overridden
/// `getCurrentLine()`, or a plain line.
fn read_line_ex(vm: &mut Vm, obj: &ObjRef, silent: bool) -> PResult<bool> {
    let (file_flags, delimiter, enclosure, escape) =
        with_fs(obj, |st| (st.flags, st.delimiter, st.enclosure, st.escape));
    if file_flags & READ_CSV != 0 {
        return Ok(read_csv(vm, obj, delimiter, enclosure, escape, silent)?.is_some());
    }
    if !engine::is_overridden(obj.class(), b"getcurrentline", "SplFileObject") {
        return read(vm, obj, silent, false);
    }
    let res = with_fs(obj, |st| {
        free_line(st);
        st.stream.clone()
    });
    let Some(res) = res else {
        return Err(not_initialized(vm));
    };
    if streams::eof(vm, &res)? {
        return if silent {
            Ok(false)
        } else {
            Err(cannot_read(vm, obj))
        };
    }
    let line = match vm
        .call_method(obj, b"getCurrentLine", Vec::new())?
        .into_unref()
    {
        Value::String(s) => s,
        other => {
            return Err(vm.throw_type_error(format!(
                "{}::getCurrentLine(): Return value must be of type string, {} returned",
                obj.class().name,
                other.value_name()
            )));
        }
    };
    with_fs(obj, |st| {
        if st.current_line.is_some() || st.current_zval.is_some() {
            st.current_line_num += 1;
        }
        free_line(st);
        st.current_line = Some(line);
    });
    Ok(true)
}

/// `spl_filesystem_file_read_line`: skips empty lines with SKIP_EMPTY.
fn read_line(vm: &mut Vm, obj: &ObjRef, silent: bool) -> PResult<bool> {
    let mut ret = read_line_ex(vm, obj, silent)?;
    while ret && with_fs(obj, |st| st.flags & SKIP_EMPTY != 0 && is_line_empty(st)) {
        with_fs(obj, free_line);
        ret = read_line_ex(vm, obj, silent)?;
    }
    Ok(ret)
}

/// `spl_filesystem_file_rewind`.
fn rewind(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let res = stream_of(vm, obj)?;
    if streams::seek(vm, &res, 0, SEEK_SET)? == -1 {
        let name = with_fs(obj, |st| st.file_name.clone().unwrap_or_default());
        let mut msg = b"Cannot rewind file ".to_vec();
        msg.extend_from_slice(&name);
        return Err(vm.throw_error("RuntimeException", PhpStr::from_vec(msg)));
    }
    with_fs(obj, |st| {
        free_line(st);
        st.current_line_num = 0;
    });
    if has_flag(obj, READ_AHEAD) {
        read_line(vm, obj, true)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

/// `SplFileObject::__construct()`.
fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let file_name = path_arg(vm, args, 0)?;
    let open_mode = args.str(1).clone();
    let use_include_path = args.bool(2);
    let context = resource_or_null(vm, args, 3)?;
    let obj = this(args);
    // Prevent reinitialization of Object
    if with_fs(&obj, |st| st.stream.is_some()) {
        return Err(vm.throw_error("Error", "Cannot call constructor twice"));
    }
    with_fs(&obj, |st| {
        st.open_mode = Some(open_mode);
        st.file_name = Some(file_name);
        st.zcontext = context;
    });
    // spl_filesystem_file_open() can generate E_WARNINGs which we want to
    // promote to exceptions
    with_error_handling(vm, "RuntimeException", |vm| {
        file_open(vm, &obj, use_include_path)
    })?;
    with_fs(&obj, |st| {
        let orig = st.orig_path.clone().unwrap_or_default();
        let mut len = orig.len();
        if len > 1 && is_slash(orig[len - 1]) {
            len -= 1;
        }
        while len > 1 && !is_slash(orig[len - 1]) {
            len -= 1;
        }
        len = len.saturating_sub(1);
        st.path = Some(PhpStr::from_bytes(&orig[..len]));
    });
    Ok(Value::Null)
}

/// `SplTempFileObject::__construct()`: `php://memory` for a negative
/// `$maxMemory`, else `php://temp` (with `/maxmemory:N` when passed).
fn temp_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let max_memory = args.int(0);
    let obj = this(args);
    // Prevent reinitialization of Object
    if with_fs(&obj, |st| st.stream.is_some()) {
        return Err(vm.throw_error("Error", "Cannot call constructor twice"));
    }
    let file_name = if max_memory < 0 {
        "php://memory".to_string()
    } else if !args.is_empty() {
        format!("php://temp/maxmemory:{max_memory}")
    } else {
        "php://temp".to_string()
    };
    with_fs(&obj, |st| {
        st.file_name = Some(PhpStr::from(file_name));
        st.open_mode = Some(PhpStr::from("wb"));
    });
    // spl_filesystem_file_open() can generate E_WARNINGs which we want to
    // promote to exceptions
    with_error_handling(vm, "RuntimeException", |vm| file_open(vm, &obj, false))?;
    with_fs(&obj, |st| st.path = Some(PhpStr::new()));
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// Iteration and line access
// ---------------------------------------------------------------------------

fn rewind_m(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    rewind(vm, &this(args))?;
    Ok(Value::Null)
}

fn eof_m(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_of(vm, &this(args))?;
    Ok(Value::Bool(streams::eof(vm, &res)?))
}

/// `SplFileObject::valid()`: with READ_AHEAD whether a line is held, else
/// `!eof()`.
fn valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (read_ahead, held, res) = with_fs(&obj, |st| {
        (
            st.flags & READ_AHEAD != 0,
            st.current_line.is_some() || st.current_zval.is_some(),
            st.stream.clone(),
        )
    });
    if read_ahead {
        return Ok(Value::Bool(held));
    }
    match res {
        Some(res) => Ok(Value::Bool(!streams::eof(vm, &res)?)),
        None => Ok(Value::Bool(false)),
    }
}

/// `SplFileObject::fgets()` (also `getCurrentLine()`).
fn fgets(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let res = stream_of(vm, &obj)?;
    let held = with_fs(&obj, |st| {
        let line = st.current_line.take()?;
        free_line(st);
        st.current_line_num += 1;
        Some(line)
    });
    if let Some(line) = held {
        return Ok(Value::String(line));
    }
    if !read_ex(vm, &obj, true, 1, false)? {
        if streams::eof(vm, &res)? {
            return Ok(Value::empty_string());
        }
        return Err(cannot_read(vm, &obj));
    }
    let line = with_fs(&obj, |st| {
        let line = st.current_line.take().unwrap_or_default();
        free_line(st);
        line
    });
    Ok(Value::String(line))
}

/// `SplFileObject::current()`: the held line (read if needed), or the CSV
/// row with READ_CSV.
fn current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    stream_of(vm, &obj)?;
    if with_fs(&obj, |st| {
        st.current_line.is_none() && st.current_zval.is_none()
    }) {
        read_line(vm, &obj, true)?;
    }
    Ok(with_fs(&obj, |st| {
        match (&st.current_line, &st.current_zval) {
            (Some(line), zval) if st.flags & READ_CSV == 0 || zval.is_none() => {
                Value::String(line.clone())
            }
            (_, Some(row)) => row.clone(),
            _ => Value::Bool(false),
        }
    }))
}

fn key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // Do not read the next line to support correct counting with fgetc()
    Ok(Value::Int(with_fs(&this(args), |st| st.current_line_num)))
}

/// `SplFileObject::next()`.
fn next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    stream_of(vm, &obj)?;
    if with_fs(&obj, |st| {
        st.current_line.is_none() && st.current_zval.is_none()
    }) && !read_line(vm, &obj, true)?
    {
        return Ok(Value::Null);
    }
    with_fs(&obj, free_line);
    if has_flag(&obj, READ_AHEAD) {
        read_line(vm, &obj, true)?;
    }
    with_fs(&obj, |st| st.current_line_num += 1);
    Ok(Value::Null)
}

fn set_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = args.int(0);
    with_fs(&this(args), |st| st.flags = flags);
    Ok(Value::Null)
}

fn get_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(
        with_fs(&this(args), |st| st.flags) & FILE_OBJECT_MASK,
    ))
}

fn set_max_line_len(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let max_len = args.int(0);
    if max_len < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    let max_len = usize::try_from(max_len).unwrap_or(usize::MAX);
    with_fs(&this(args), |st| st.max_line_len = max_len);
    Ok(Value::Null)
}

fn get_max_line_len(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let n = with_fs(&this(args), |st| st.max_line_len);
    Ok(Value::Int(i64::try_from(n).unwrap_or(i64::MAX)))
}

fn has_children(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(false))
}

fn get_children(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

/// `SplFileObject::seek()`: rewinds and reads `$line` lines.
fn seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let line_pos = args.int(0);
    let obj = this(args);
    stream_of(vm, &obj)?;
    if line_pos < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    rewind(vm, &obj)?;
    for _ in 0..line_pos {
        if !read_line(vm, &obj, true)? {
            break;
        }
    }
    if line_pos > 0 && !has_flag(&obj, READ_AHEAD) {
        with_fs(&obj, |st| {
            st.current_line_num += 1;
            free_line(st);
        });
    }
    Ok(Value::Null)
}

/// `SplFileObject::__toString()`: the held line (read if needed).
fn to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    stream_of(vm, &obj)?;
    if with_fs(&obj, |st| st.current_line.is_none()) && !read_line(vm, &obj, true)? {
        return Ok(Value::empty_string());
    }
    Ok(Value::String(with_fs(&obj, |st| {
        st.current_line.clone().unwrap_or_default()
    })))
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// A `$separator` / `$enclosure` argument at index `i`: its character,
/// `default` when not passed.
fn char_arg(vm: &mut Vm, args: &Args, i: usize, default: u8) -> PResult<u8> {
    if !args.is_passed(i) {
        return Ok(default);
    }
    match args.str(i).as_bytes() {
        &[c] => Ok(c),
        _ => Err(vm.argument_value_error(i + 1, "must be a single character")),
    }
}

/// `spl_csv_enclosure_param_handling`: the `$escape` argument at index `i`,
/// or the object's escape character (deprecated while it is the default).
fn escape_arg(vm: &mut Vm, args: &Args, obj: &ObjRef, i: usize) -> PResult<i32> {
    if args.is_passed(i) {
        let escape = args.str(i).clone();
        let e = csv::handle_escape_argument(vm, Some(escape.as_bytes()), i + 1)?;
        return Ok(e.unwrap_or(CSV_NO_ESCAPE));
    }
    let (escape, is_default) = with_fs(obj, |st| (st.escape, st.is_escape_default));
    if is_default {
        vm.deprecated(
            "the $escape parameter must be provided, as its default value will change, \
             either explicitly or via SplFileObject::setCsvControl()",
        )?;
    }
    Ok(escape)
}

/// `SplFileObject::fgetcsv()` (`#[\Deprecated(since: '8.6')]`).
fn fgetcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "SplFileObject", "fgetcsv", "8.6")?;
    let obj = this(args);
    stream_of(vm, &obj)?;
    let (d, e) = with_fs(&obj, |st| (st.delimiter, st.enclosure));
    let delimiter = char_arg(vm, args, 0, d)?;
    let enclosure = char_arg(vm, args, 1, e)?;
    let escape = escape_arg(vm, args, &obj, 2)?;
    Ok(read_csv(vm, &obj, delimiter, enclosure, escape, true)?.unwrap_or(Value::Bool(false)))
}

/// `SplFileObject::fputcsv()` (`#[\Deprecated(since: '8.6')]`).
fn fputcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "SplFileObject", "fputcsv", "8.6")?;
    let obj = this(args);
    let fields = args.array(0).clone();
    let res = stream_of(vm, &obj)?;
    let (d, e) = with_fs(&obj, |st| (st.delimiter, st.enclosure));
    let delimiter = char_arg(vm, args, 1, d)?;
    let enclosure = char_arg(vm, args, 2, e)?;
    let escape = escape_arg(vm, args, &obj, 3)?;
    let eol = args.is_passed(4).then(|| args.str(4).clone());
    let written = csv::put_line(
        vm,
        &mut ResCsv(res),
        &fields,
        delimiter,
        enclosure,
        escape,
        eol.as_ref().map(PhpStr::as_bytes),
    )?;
    Ok(match written {
        Some(n) => Value::Int(i64::try_from(n).unwrap_or(i64::MAX)),
        None => Value::Bool(false),
    })
}

/// `SplFileObject::setCsvControl()` (`#[\Deprecated(since: '8.6')]`).
fn set_csv_control(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "SplFileObject", "setCsvControl", "8.6")?;
    let delimiter = char_arg(vm, args, 0, b',')?;
    let enclosure = char_arg(vm, args, 1, b'"')?;
    let escape_str = args.is_passed(2).then(|| args.str(2).clone());
    let escape = csv::handle_escape_argument(vm, escape_str.as_ref().map(PhpStr::as_bytes), 3)?
        .unwrap_or(CSV_NO_ESCAPE);
    with_fs(&this(args), |st| {
        if escape_str.is_some() {
            st.is_escape_default = false;
        }
        st.delimiter = delimiter;
        st.enclosure = enclosure;
        st.escape = escape;
    });
    Ok(Value::Null)
}

/// A one-character C string (`""` for NUL).
fn c_char(c: u8) -> Value {
    if c == 0 {
        Value::empty_string()
    } else {
        Value::bytes(&[c])
    }
}

/// `SplFileObject::getCsvControl()` (`#[\Deprecated(since: '8.6')]`).
fn get_csv_control(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "SplFileObject", "getCsvControl", "8.6")?;
    let (d, e, esc) = with_fs(&this(args), |st| (st.delimiter, st.enclosure, st.escape));
    let escape = if esc == CSV_NO_ESCAPE {
        Value::empty_string()
    } else {
        c_char(esc as u8)
    };
    Ok(Value::array(php_runtime::PhpArray::from_values(vec![
        c_char(d),
        c_char(e),
        escape,
    ])))
}

// ---------------------------------------------------------------------------
// The f*() methods
// ---------------------------------------------------------------------------

fn flock(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let operation = args.int(0);
    let wouldblock = args.is_passed(1).then(|| args.by_ref(1));
    let res = stream_of(vm, &this(args))?;
    flock_common(vm, &res, operation, 1, wouldblock)
}

fn fflush(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_of(vm, &this(args))?;
    Ok(Value::Bool(streams::flush(vm, &res)?))
}

fn ftell(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_of(vm, &this(args))?;
    Ok(match streams::tell(&res) {
        Some(p) if p != -1 => Value::Int(p),
        _ => Value::Bool(false),
    })
}

fn fseek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let offset = args.int(0);
    let whence = args.int(1);
    let obj = this(args);
    let res = stream_of(vm, &obj)?;
    with_fs(&obj, free_line);
    let whence = i32::try_from(whence).unwrap_or(-1);
    Ok(Value::Int(i64::from(streams::seek(
        vm, &res, offset, whence,
    )?)))
}

/// `SplFileObject::fgetc()`: a newline advances the line number.
fn fgetc(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let res = stream_of(vm, &obj)?;
    with_fs(&obj, free_line);
    let c = with_stream(vm, &res, |vm, s| s.getc(vm))?.flatten();
    let Some(c) = c else {
        return Ok(Value::Bool(false));
    };
    if c == b'\n' {
        with_fs(&obj, |st| st.current_line_num += 1);
    }
    Ok(Value::bytes(&[c]))
}

fn fpassthru(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_of(vm, &this(args))?;
    let n = with_stream(vm, &res, |vm, s| s.passthru(vm))?.unwrap_or(0);
    Ok(Value::Int(n as i64))
}

/// `SplFileObject::fscanf()`: scans the next line.
fn fscanf(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let format = args.str(0).clone();
    let vars = args.rest(1).to_vec();
    let obj = this(args);
    stream_of(vm, &obj)?;
    // Get next line
    read(vm, &obj, false, false)?;
    let line = with_fs(&obj, |st| st.current_line.clone().unwrap_or_default());
    php_sscanf_internal(vm, &line, &format, &vars)
}

fn fwrite(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = args.str(0).clone();
    let length = args.int_opt(1);
    let res = stream_of(vm, &this(args))?;
    let len = match length {
        None => data.len(),
        Some(n) if n >= 0 => usize::try_from(n).unwrap_or(usize::MAX).min(data.len()),
        // Negative length given, nothing to write
        Some(_) => 0,
    };
    if len == 0 {
        return Ok(Value::Int(0));
    }
    Ok(match streams::write(vm, &res, &data[..len])? {
        Some(n) => Value::Int(i64::try_from(n).unwrap_or(i64::MAX)),
        None => Value::Bool(false),
    })
}

fn fread(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let length = args.int(0);
    let res = stream_of(vm, &this(args))?;
    if length <= 0 {
        return Err(vm.argument_value_error(1, "must be greater than 0"));
    }
    let len = usize::try_from(length).unwrap_or(usize::MAX);
    Ok(match streams::read(vm, &res, len)? {
        Some(s) => Value::from(s),
        None => Value::Bool(false),
    })
}

/// `php_fstat`.
fn fstat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = stream_of(vm, &this(args))?;
    Ok(match streams::stat(vm, &res)? {
        Some(sb) => Value::from(sb.to_php_array()),
        None => Value::Bool(false),
    })
}

fn ftruncate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let size = args.int(0);
    let obj = this(args);
    let res = stream_of(vm, &obj)?;
    if size < 0 {
        return Err(vm.argument_value_error(1, "must be greater than or equal to 0"));
    }
    let supported = with_stream(vm, &res, |vm, s| s.truncate_supported(vm))?.unwrap_or(false);
    if !supported {
        let name = with_fs(&obj, |st| st.file_name.clone().unwrap_or_default());
        let mut msg = b"Can't truncate file ".to_vec();
        msg.extend_from_slice(&name);
        return Err(vm.throw_error("LogicException", PhpStr::from_vec(msg)));
    }
    let r = with_stream(vm, &res, |vm, s| s.truncate_set_size(vm, size))?.unwrap_or(-1);
    Ok(Value::Bool(r == 0))
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

natives! {
    CONSTRUCT: r#"public function __construct(string $filename, string $mode = "r", bool $useIncludePath = false, $context = null) {}"# => construct;
    REWIND: "public function rewind(): void {}" => rewind_m;
    EOF: "public function eof(): bool {}" => eof_m;
    VALID: "public function valid(): bool {}" => valid;
    FGETS: "public function fgets(): string {}" => fgets;
    FREAD: "public function fread(int $length): string|false {}" => fread;
    // #[\Deprecated(since: '8.6')]
    FGETCSV: r#"public function fgetcsv(string $separator = ",", string $enclosure = "\"", string $escape = "\\"): array|false {}"# => fgetcsv;
    // #[\Deprecated(since: '8.6')]
    FPUTCSV: r#"public function fputcsv(array $fields, string $separator = ",", string $enclosure = "\"", string $escape = "\\", string $eol = "\n"): int|false {}"# => fputcsv;
    // #[\Deprecated(since: '8.6')]
    SET_CSV_CONTROL: r#"public function setCsvControl(string $separator = ",", string $enclosure = "\"", string $escape = "\\"): void {}"# => set_csv_control;
    // #[\Deprecated(since: '8.6')]
    GET_CSV_CONTROL: "public function getCsvControl(): array {}" => get_csv_control;
    FLOCK: "public function flock(int $operation, &$wouldBlock = null): bool {}" => flock;
    FFLUSH: "public function fflush(): bool {}" => fflush;
    FTELL: "public function ftell(): int|false {}" => ftell;
    FSEEK: "public function fseek(int $offset, int $whence = SEEK_SET): int {}" => fseek;
    FGETC: "public function fgetc(): string|false {}" => fgetc;
    FPASSTHRU: "public function fpassthru(): int {}" => fpassthru;
    FSCANF: "public function fscanf(string $format, mixed &...$vars): array|int|null {}" => fscanf;
    FWRITE: "public function fwrite(string $data, ?int $length = null): int|false {}" => fwrite;
    FSTAT: "public function fstat(): array {}" => fstat;
    FTRUNCATE: "public function ftruncate(int $size): bool {}" => ftruncate;
    CURRENT: "public function current(): string|array|false {}" => current;
    KEY: "public function key(): int {}" => key;
    NEXT: "public function next(): void {}" => next;
    SET_FLAGS: "public function setFlags(int $flags): void {}" => set_flags;
    GET_FLAGS: "public function getFlags(): int {}" => get_flags;
    SET_MAX_LINE_LEN: "public function setMaxLineLen(int $maxLength): void {}" => set_max_line_len;
    GET_MAX_LINE_LEN: "public function getMaxLineLen(): int {}" => get_max_line_len;
    HAS_CHILDREN: "public function hasChildren(): false {}" => has_children;
    GET_CHILDREN: "public function getChildren(): null {}" => get_children;
    SEEK: "public function seek(int $line): void {}" => seek;
    // @alias SplFileObject::fgets
    GET_CURRENT_LINE: "public function getCurrentLine(): string {}" => fgets;
    TO_STRING: "public function __toString(): string {}" => to_string;

    TEMP_CONSTRUCT: "public function __construct(int $maxMemory = 2 * 1024 * 1024) {}" => temp_construct;
}

pub(super) static METHODS: &[&NativeFunction] = &[
    &CONSTRUCT,
    &REWIND,
    &EOF,
    &VALID,
    &FGETS,
    &FREAD,
    &FLOCK,
    &FFLUSH,
    &FTELL,
    &FSEEK,
    &FGETC,
    &FPASSTHRU,
    &FSCANF,
    &FWRITE,
    &FSTAT,
    &FTRUNCATE,
    &CURRENT,
    &KEY,
    &NEXT,
    &SET_FLAGS,
    &GET_FLAGS,
    &SET_MAX_LINE_LEN,
    &GET_MAX_LINE_LEN,
    &HAS_CHILDREN,
    &GET_CHILDREN,
    &SEEK,
    &GET_CURRENT_LINE,
    &TO_STRING,
];

pub(super) static DEPRECATED_METHODS: &[&NativeFunction] =
    &[&FGETCSV, &FPUTCSV, &SET_CSV_CONTROL, &GET_CSV_CONTROL];

pub(super) static TEMP_METHODS: &[&NativeFunction] = &[&TEMP_CONSTRUCT];
