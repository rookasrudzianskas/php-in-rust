//! main/streams + ext/standard stream wrappers (plain files, php://, data:, glob:).
//!
//! # Stream API for other modules
//!
//! A PHP stream is a resource of type `"stream"` whose payload is a shared
//! [`StreamHandle`] (`Rc<RefCell<Stream>>`). Other modules use:
//!
//! * [`stream_arg`] — `PHP_Z_PARAM_STREAM`: turns an argument into the
//!   stream's [`ResRef`] or throws php-src's TypeError.
//! * [`open`] — `php_stream_open_wrapper_ex` (any wrapper, "Failed to open
//!   stream" warnings), [`open_memory`] / [`open_temp`] for in-memory streams.
//! * [`read`], [`read_line`], [`read_record`], [`read_to_end`], [`write`],
//!   [`write_resource`], [`seek`], [`tell`], [`eof`], [`flush`], [`stat`],
//!   [`close`] — the `php_stream_*` operations on a resource.
//! * [`read_all`] — `file_get_contents($path)`.
//! * [`with_stream`] — direct access to the [`Stream`] for anything else.
//!
//! Wrappers implement [`wrappers::Wrapper`], stream implementations
//! [`stream::StreamOps`], filters [`filter::FilterOps`].

// Nested `if let` mirrors the C control flow (same choice as php-runtime).
#![allow(clippy::collapsible_if)]

pub mod context;
pub mod csv;
pub mod filter;
pub mod glob;
pub mod glob_wrapper;
pub mod ini;
pub mod memory;
pub mod php_wrapper;
pub mod plain;
pub mod socket;
pub mod std_filters;
pub mod stream;
pub mod sys;
pub mod user_wrapper;
pub mod wrappers;

use php_runtime::{Args, PResult, PhpStr, Registry, ResRef, Value, Vm};

pub use stream::{StatBuf, Stream, StreamHandle, handle_of, process_deferred_closes, with_stream};

/// `PHP_Z_PARAM_STREAM`: argument `arg_num` (1-based) must be an open stream.
pub fn stream_arg(vm: &mut Vm, v: &Value, arg_num: usize) -> PResult<ResRef> {
    match &*v.deref() {
        Value::Resource(r) if handle_of(r).is_some() => Ok(r.clone()),
        Value::Resource(_) => {
            Err(vm.argument_type_error(arg_num, "must be an open stream resource"))
        }
        other => Err(vm.argument_type_error(
            arg_num,
            &format!("must be of type resource, {} given", other.value_name()),
        )),
    }
}

/// Like [`stream_arg`] for an `Args` slot (0-based index).
pub fn stream_param(vm: &mut Vm, args: &Args, i: usize) -> PResult<ResRef> {
    let v = args.get(i).clone();
    process_deferred_closes(vm)?;
    stream_arg(vm, &v, i + 1)
}

/// `Z_PARAM_PATH`: a string argument without NUL bytes (0-based index).
pub fn path_param(vm: &mut Vm, args: &Args, i: usize) -> PResult<PhpStr> {
    let s = args.str(i).clone();
    if s.as_bytes().contains(&0) {
        return Err(vm.argument_value_error(i + 1, "must not contain any null bytes"));
    }
    Ok(s)
}

/// `Z_PARAM_RESOURCE_OR_NULL` + `php_stream_context_from_zval` (0-based index).
pub fn context_param(
    vm: &mut Vm,
    args: &Args,
    i: usize,
    no_default: bool,
) -> PResult<Option<ResRef>> {
    let v = args.get(i).unref();
    match &v {
        Value::Null | Value::Undef | Value::Resource(_) => context::from_zval(vm, &v, no_default),
        other => Err(vm.argument_type_error(
            i + 1,
            &format!(
                "must be of type resource or null, {} given",
                other.value_name()
            ),
        )),
    }
}

/// `php_stream_open_wrapper_ex(path, mode, REPORT_ERRORS | USE_PATH?)`.
pub fn open(
    vm: &mut Vm,
    path: &[u8],
    mode: &[u8],
    use_include_path: bool,
    context: Option<&ResRef>,
) -> PResult<Option<ResRef>> {
    let opts = stream::open::REPORT_ERRORS
        | if use_include_path {
            stream::open::USE_PATH
        } else {
            0
        };
    wrappers::open_wrapper(vm, path, mode, opts, None, context)
}

/// `php_stream_open_for_zend_ex` (include/require): opens `path` with
/// `USE_PATH | REPORT_ERRORS | STREAM_OPEN_FOR_INCLUDE` and returns the
/// whole contents plus the opened (resolved) path. `None` after the
/// "Failed to open stream" warning; the caller adds "Failed opening ... for
/// inclusion".
pub fn open_for_include(vm: &mut Vm, path: &[u8]) -> PResult<Option<(Vec<u8>, PhpStr)>> {
    let opts = stream::open::USE_PATH
        | stream::open::REPORT_ERRORS
        | stream::open::STREAM_OPEN_FOR_INCLUDE;
    let mut opened = None;
    let Some(res) = wrappers::open_wrapper(vm, path, b"rb", opts, Some(&mut opened), None)? else {
        return Ok(None);
    };
    let data = read_to_end(vm, &res)?.unwrap_or_default();
    close(vm, &res)?;
    Ok(Some((
        data,
        opened.unwrap_or_else(|| PhpStr::from_bytes(path)),
    )))
}

/// A `php://memory` stream holding `data` (read/write, positioned at 0).
pub fn open_memory(vm: &mut Vm, data: &[u8]) -> ResRef {
    memory::memory_open(vm, memory::TEMP_STREAM_DEFAULT, data.to_vec())
}

/// A `php://temp` stream holding `data` (positioned at 0).
pub fn open_temp(vm: &mut Vm, data: &[u8]) -> PResult<ResRef> {
    memory::temp_open(vm, memory::TEMP_STREAM_DEFAULT, memory::MAX_MEM, data)
}

/// `php_stream_read_to_str`: up to `len` bytes, `None` on error.
pub fn read(vm: &mut Vm, res: &ResRef, len: usize) -> PResult<Option<Vec<u8>>> {
    Ok(with_stream(vm, res, |vm, s| s.read_to_str(vm, len))?.flatten())
}

/// `php_stream_get_line`: the next line including its EOL; `maxlen` is the
/// C buffer size (at most `maxlen - 1` bytes), `None` for unlimited.
pub fn read_line(vm: &mut Vm, res: &ResRef, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
    Ok(with_stream(vm, res, |vm, s| s.get_line(vm, maxlen))?.flatten())
}

/// `php_stream_get_record` (`stream_get_line` semantics).
pub fn read_record(
    vm: &mut Vm,
    res: &ResRef,
    maxlen: usize,
    delim: &[u8],
) -> PResult<Option<Vec<u8>>> {
    Ok(with_stream(vm, res, |vm, s| s.get_record(vm, maxlen, delim))?.flatten())
}

/// `php_stream_copy_to_mem(stream, PHP_STREAM_COPY_ALL)`: the rest of the
/// stream (`Some(empty)` at EOF).
pub fn read_to_end(vm: &mut Vm, res: &ResRef) -> PResult<Option<Vec<u8>>> {
    with_stream(vm, res, |vm, s| {
        Ok(s.copy_to_mem(vm, stream::COPY_ALL)?.unwrap_or_default())
    })
}

/// `php_stream_write`: bytes written, `None` on failure.
pub fn write(vm: &mut Vm, res: &ResRef, data: &[u8]) -> PResult<Option<usize>> {
    let n = with_stream(vm, res, |vm, s| s.write(vm, data))?;
    Ok(n.and_then(|n| (n >= 0).then_some(n as usize)))
}

/// `php_stream_seek`: 0 on success, -1 on failure.
pub fn seek(vm: &mut Vm, res: &ResRef, offset: i64, whence: i32) -> PResult<i32> {
    Ok(with_stream(vm, res, |vm, s| s.seek(vm, offset, whence))?.unwrap_or(-1))
}

/// `php_stream_tell`.
pub fn tell(res: &ResRef) -> Option<i64> {
    handle_of(res).and_then(|h| h.try_borrow().ok().map(|s| s.tell()))
}

/// `php_stream_eof`.
pub fn eof(vm: &mut Vm, res: &ResRef) -> PResult<bool> {
    Ok(with_stream(vm, res, |vm, s| s.eof(vm))?.unwrap_or(true))
}

/// `php_stream_flush`: true on success.
pub fn flush(vm: &mut Vm, res: &ResRef) -> PResult<bool> {
    Ok(with_stream(vm, res, |vm, s| s.flush(vm))?.is_some_and(|r| r == 0))
}

/// `php_stream_stat`.
pub fn stat(vm: &mut Vm, res: &ResRef) -> PResult<Option<StatBuf>> {
    Ok(with_stream(vm, res, |vm, s| s.stat(vm))?.flatten())
}

/// `php_stream_free(stream, PHP_STREAM_FREE_CLOSE | KEEP_RSRC)` as done by
/// `fclose()`: true when the close succeeded.
pub fn close(vm: &mut Vm, res: &ResRef) -> PResult<bool> {
    let r = with_stream(vm, res, |vm, s| {
        let persistent = if s.st.is_persistent {
            stream::PHP_STREAM_FREE_PERSISTENT
        } else {
            0
        };
        s.free(
            vm,
            stream::PHP_STREAM_FREE_KEEP_RSRC | stream::PHP_STREAM_FREE_CLOSE | persistent,
        )
    })?;
    if r.is_some() {
        res.close();
    }
    Ok(r == Some(0))
}

/// `fwrite()` semantics on a stream resource: returns the number of bytes
/// written, or `None` when the write failed (any warning has been emitted).
/// A value that is not a stream resource raises PHP's TypeError.
pub fn write_resource(vm: &mut Vm, stream: &Value, data: &[u8]) -> PResult<Option<usize>> {
    process_deferred_closes(vm)?;
    let res = stream_arg(vm, stream, 1)?;
    if data.is_empty() {
        return Ok(Some(0));
    }
    write(vm, &res, data)
}

/// `file_get_contents($path)` semantics for any supported wrapper, emitting
/// PHP's "Failed to open stream" warnings. `None` on failure.
pub fn read_all(vm: &mut Vm, path: &[u8]) -> PResult<Option<Vec<u8>>> {
    let ctx = context::default_context(vm);
    let Some(res) = open(vm, path, b"rb", false, Some(&ctx))? else {
        return Ok(None);
    };
    let data = with_stream(vm, &res, |vm, s| {
        if s.kind() == stream::StreamKind::Stdio {
            s.set_option(
                vm,
                stream::opt::READ_BUFFER,
                stream::opt::BUFFER_NONE,
                &mut stream::OptParam::None,
            )?;
        }
        s.copy_to_mem(vm, stream::COPY_ALL)
    })?
    .flatten()
    .unwrap_or_default();
    close(vm, &res)?;
    Ok(Some(data))
}

pub fn register(r: &mut Registry) {
    csv::register(r);
    user_wrapper::register(r);
    ini::register(r);

    // main/streams/userspace.stub.php
    for (n, v) in [
        ("STREAM_USE_PATH", 1i64),
        ("STREAM_IGNORE_URL", 2),
        ("STREAM_REPORT_ERRORS", 8),
        ("STREAM_MUST_SEEK", 16),
        ("STREAM_URL_STAT_LINK", 1),
        ("STREAM_URL_STAT_QUIET", 2),
        ("STREAM_MKDIR_RECURSIVE", 1),
        ("STREAM_IS_URL", 1),
        ("STREAM_OPTION_BLOCKING", 1),
        ("STREAM_OPTION_READ_TIMEOUT", 4),
        ("STREAM_OPTION_READ_BUFFER", 2),
        ("STREAM_OPTION_WRITE_BUFFER", 3),
        ("STREAM_BUFFER_NONE", 0),
        ("STREAM_BUFFER_LINE", 1),
        ("STREAM_BUFFER_FULL", 2),
        ("STREAM_CAST_AS_STREAM", 0),
        ("STREAM_CAST_FOR_SELECT", 3),
        ("STREAM_META_TOUCH", 1),
        ("STREAM_META_OWNER", 3),
        ("STREAM_META_OWNER_NAME", 2),
        ("STREAM_META_GROUP", 5),
        ("STREAM_META_GROUP_NAME", 4),
        ("STREAM_META_ACCESS", 6),
    ] {
        r.constant(n, v);
    }

    // sapi/cli/php_cli.c cli_register_file_handles
    let vm = &mut *r.vm;
    let handles = [
        php_wrapper::open_std(vm, 0, b"rb"),
        php_wrapper::open_std(vm, 1, b"wb"),
        php_wrapper::open_std(vm, 2, b"wb"),
    ];
    for (name, h) in ["STDIN", "STDOUT", "STDERR"].into_iter().zip(handles) {
        if let Some(res) = h {
            if let Some(s) = handle_of(&res) {
                s.borrow_mut().st.flags |= stream::flags::NO_RSCR_DTOR_CLOSE;
            }
            r.constant(name, Value::Resource(res));
        }
    }
}

#[cfg(test)]
mod tests;
