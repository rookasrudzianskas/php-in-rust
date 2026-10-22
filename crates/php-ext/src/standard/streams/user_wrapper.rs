//! main/streams/userspace.c — userspace stream wrappers
//! (`stream_wrapper_register` & co.) and the stream / directory ops that
//! call methods of the registered PHP class.
//!
//! Every wrapper operation instantiates the class like
//! `user_stream_create_object` (sets the `context` property, runs the
//! constructor) and calls the matching method through
//! [`call_method_if_exists`], which tells an undefined method
//! (`zend_call_method_if_exists` returning FAILURE) apart from one that threw.
//! A thrown exception unwinds immediately as `Err`; php-src keeps going with
//! an `UNDEF` return value instead, but every further engine call is a no-op
//! while an exception is pending, so the observable behaviour is the same.

use std::any::Any;
use std::os::fd::RawFd;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, class_flags, fn_flags, member_flags};
use php_runtime::value::numeric::c_str;
use php_runtime::{
    Args, CallArgs, ClassRef, Method, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, PhpStr,
    Registry, ResRef, Value, Vm,
};

use super::stream::{
    self, CastAs, OptParam, StatBuf, StreamKind, StreamOps, StreamState, flags, handle_of, open,
    opt, stream_warn, with_stream,
};
use super::sys;
use super::wrappers::{self, MetaValue, Wrapper, WrapperRef};

/// `user_stream_wops.label` / `php_stream_userspace_ops.label`.
const LABEL: &str = "user-space";

const USERSTREAM_OPEN: &str = "stream_open";
const USERSTREAM_CLOSE: &str = "stream_close";
const USERSTREAM_READ: &str = "stream_read";
const USERSTREAM_WRITE: &str = "stream_write";
const USERSTREAM_FLUSH: &str = "stream_flush";
const USERSTREAM_SEEK: &str = "stream_seek";
const USERSTREAM_TELL: &str = "stream_tell";
const USERSTREAM_EOF: &str = "stream_eof";
const USERSTREAM_STAT: &str = "stream_stat";
const USERSTREAM_STATURL: &str = "url_stat";
const USERSTREAM_UNLINK: &str = "unlink";
const USERSTREAM_RENAME: &str = "rename";
const USERSTREAM_MKDIR: &str = "mkdir";
const USERSTREAM_RMDIR: &str = "rmdir";
const USERSTREAM_DIR_OPEN: &str = "dir_opendir";
const USERSTREAM_DIR_READ: &str = "dir_readdir";
const USERSTREAM_DIR_REWIND: &str = "dir_rewinddir";
const USERSTREAM_DIR_CLOSE: &str = "dir_closedir";
const USERSTREAM_LOCK: &str = "stream_lock";
const USERSTREAM_CAST: &str = "stream_cast";
const USERSTREAM_SET_OPTION: &str = "stream_set_option";
const USERSTREAM_TRUNCATE: &str = "stream_truncate";
const USERSTREAM_METADATA: &str = "stream_metadata";

/// `PHP_STREAM_IS_URL`.
const PHP_STREAM_IS_URL: i64 = 1;

/// ext/standard/flock_compat.h: the values userland sees in `stream_lock()`.
const PHP_LOCK_SH: i64 = 1;
const PHP_LOCK_EX: i64 = 2;
const PHP_LOCK_UN: i64 = 3;
const PHP_LOCK_NB: i64 = 4;

/// `PHP_STREAM_AS_STDIO` / `PHP_STREAM_AS_FD_FOR_SELECT` (the only cast
/// kinds userland knows: `STREAM_CAST_AS_STREAM`, `STREAM_CAST_FOR_SELECT`).
const PHP_STREAM_AS_STDIO: i64 = 0;
const PHP_STREAM_AS_FD_FOR_SELECT: i64 = 3;

/// `BUFSIZ` of the C library, passed when a buffer option has no size.
#[cfg(target_os = "linux")]
const BUFSIZ: i64 = 8192;
#[cfg(not(target_os = "linux"))]
const BUFSIZ: i64 = 1024;

/// `sizeof(php_stream_dirent.d_name) - 1` (`NAME_MAX`).
const NAME_MAX: usize = 255;

/// Request globals of this file (`FG(user_stream_current_filename)`).
#[derive(Default)]
struct UserStreamGlobals {
    current_filename: Option<Vec<u8>>,
}

/// Registers `stream_wrapper_register` & co. (main/streams/userspace.c).
pub fn register(r: &mut Registry) {
    r.functions(&[
        &STREAM_WRAPPER_REGISTER,
        &STREAM_REGISTER_WRAPPER,
        &STREAM_WRAPPER_UNREGISTER,
        &STREAM_WRAPPER_RESTORE,
    ]);
}

/// A wrapper implemented by a PHP class (`struct php_user_stream_wrapper`).
pub struct UserWrapper {
    pub protocol: PhpStr,
    /// Name of the class (`uwrap->ce->name`).
    pub class: PhpStr,
    pub flags: i64,
    pub ce: ClassRef,
    /// The `"stream factory"` resource (`uwrap->resource`).
    pub resource: ResRef,
}

// ---------------------------------------------------------------------------
// Calling user methods
// ---------------------------------------------------------------------------

/// How `zend_is_callable_ex()` resolved a method name on an object.
enum Callee {
    /// A method accessible from the executing scope.
    Method,
    /// Through `__call` (missing or inaccessible method).
    Magic,
}

/// `get_scope(EG(current_execute_data))`: the class scope of the innermost
/// userland frame.
fn executing_scope(vm: &Vm) -> Option<ClassRef> {
    let frame = vm.frames.iter().rev().find(|f| !f.is_internal)?;
    let scope = frame.function.as_ref()?.scope.as_ref()?;
    vm.find_class(scope)
}

/// The visibility part of `zend_is_callable_check_func`.
fn method_accessible(vm: &Vm, m: &Method) -> bool {
    if !m.has_flag(member_flags::PRIVATE) && !m.has_flag(member_flags::PROTECTED) {
        return true;
    }
    let (Some(scope), Some(declaring)) = (executing_scope(vm), m.scope.as_ref()) else {
        return false;
    };
    if scope.name.eq_ignore_ascii_case(declaring) {
        return true;
    }
    if m.has_flag(member_flags::PRIVATE) {
        return false;
    }
    // zend_check_protected()
    vm.find_class(declaring)
        .is_some_and(|root| scope.instance_of(&root) || root.instance_of(&scope))
}

/// `zend_is_callable_ex(method_name, object, IS_CALLABLE_SUPPRESS_DEPRECATIONS)`.
fn resolve_method(vm: &Vm, obj: &ObjRef, name: &str) -> Option<Callee> {
    let ce = obj.class();
    if let Some(m) = ce.find_method(name.as_bytes()) {
        if method_accessible(vm, m) {
            return Some(Callee::Method);
        }
    }
    ce.magic.call.is_some().then_some(Callee::Magic)
}

/// `zend_call_method_if_exists`: `Ok(None)` when the method is not callable
/// (FAILURE), `Err` when it threw.
pub fn call_method_if_exists(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &str,
    args: Vec<Value>,
) -> PResult<Option<Value>> {
    let ret = match resolve_method(vm, obj, name) {
        None => return Ok(None),
        Some(Callee::Method) => vm.call_method(obj, name.as_bytes(), args)?,
        Some(Callee::Magic) => {
            // the __call trampoline receives the name and the packed arguments
            let mut packed = PhpArray::new();
            for a in args {
                let _ = packed.append(a);
            }
            vm.call_method(obj, b"__call", vec![Value::from(name), Value::from(packed)])?
        }
    };
    Ok(Some(ret.into_unref()))
}

/// `add_property_resource` / `add_property_null` for `$context`
/// (`zend_std_write_property` from internal code).
fn write_context_property(vm: &mut Vm, obj: &ObjRef, value: Value) -> PResult<()> {
    let ce = obj.class().clone();
    let name = PhpStr::from("context");
    if let Some(h) = ce.handlers.write_property {
        if h(vm, obj, &name, value.clone())? {
            return Ok(());
        }
    }
    if let Some(info) = ce.find_property(b"context").filter(|p| !p.is_static()) {
        if let Some(ty) = &info.ty {
            if !ty.matches_exact(&value) {
                return Err(vm.throw_type_error(format!(
                    "Cannot assign {} to property {}::$context of type {}",
                    value.value_name(),
                    info.declaring_class,
                    ty.to_type_string()
                )));
            }
        }
        obj.set_prop(b"context", value);
        return Ok(());
    }
    if ce.magic.set.is_some() {
        vm.call_method(obj, b"__set", vec![Value::String(name), value])?;
        return Ok(());
    }
    if ce.has_flag(class_flags::READONLY) {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot create dynamic property {}::$context", ce.name),
        ));
    }
    if !ce.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
        vm.engine_deprecated(format!(
            "Creation of dynamic property {}::$context is deprecated",
            ce.name
        ))?;
    }
    obj.set_prop(b"context", value);
    Ok(())
}

/// `user_stream_create_object`: an instance with `$context` set and the
/// constructor run.
fn create_object(vm: &mut Vm, ce: &ClassRef, ctx: Option<&ResRef>) -> PResult<ObjRef> {
    let obj = vm.instantiate(ce)?;
    let context = ctx.map_or(Value::Null, |c| Value::Resource(c.clone()));
    write_context_property(vm, &obj, context)?;
    if let Some(ctor) = ce.magic.constructor.clone() {
        vm.call_function(
            &ctor,
            CallArgs {
                this: Some(obj.clone()),
                scope: Some(ce.clone()),
                ..CallArgs::default()
            },
        )?;
    }
    Ok(obj)
}

/// `statbuf_from_array`: only the named keys are read (`zval_get_long`).
fn statbuf_from_array(a: &PhpArray) -> StatBuf {
    let get = |k: &str| {
        a.get_str(k.as_bytes())
            .map_or(0, php_runtime::value::convert::to_int_silent)
    };
    StatBuf {
        dev: get("dev"),
        ino: get("ino"),
        mode: get("mode"),
        nlink: get("nlink"),
        uid: get("uid"),
        gid: get("gid"),
        rdev: get("rdev"),
        size: get("size"),
        atime: get("atime"),
        mtime: get("mtime"),
        ctime: get("ctime"),
        blksize: get("blksize"),
        blocks: get("blocks"),
    }
}

/// Runs `f` with `PHP_STREAM_FLAG_NO_FCLOSE` set, restoring the original bit
/// afterwards (user code must not `fclose()` a stream inside its own ops).
fn with_no_fclose<R>(st: &mut StreamState, f: impl FnOnce(&mut StreamState) -> R) -> R {
    let orig_no_fclose = st.flags & flags::NO_FCLOSE;
    st.flags |= flags::NO_FCLOSE;
    let r = f(st);
    st.flags &= !flags::NO_FCLOSE;
    st.flags |= orig_no_fclose;
    r
}

fn class_name(obj: &ObjRef) -> String {
    obj.class().name.to_string()
}

/// Allocates the stream for a successfully opened user object and makes the
/// object its `wrapperdata`.
fn alloc_user_stream(vm: &mut Vm, ops: Box<dyn StreamOps>, mode: &[u8], obj: ObjRef) -> ResRef {
    let res = stream::alloc(vm, ops, mode);
    if let Some(h) = handle_of(&res) {
        let mut s = h.borrow_mut();
        let id = res.id();
        if let Some(us) = s.ops_as::<UserStream>() {
            us.res_id = id;
        }
        // set wrapper data to be a reference to our object
        s.wrapperdata = Some(Value::Object(obj));
    }
    res
}

/// Name of a cast target in "Cannot represent a stream of type ..." (cast.c).
fn cast_name(as_: CastAs) -> &'static str {
    match as_ {
        CastAs::Stdio => "STDIO FILE*",
        CastAs::Fd => "File Descriptor",
        CastAs::SocketFd => "Socket Descriptor",
        CastAs::FdForSelect => "select()able descriptor",
    }
}

/// `php_stream_cast(intstream, castas, retptr, 1)` on the stream returned by
/// `stream_cast` (the generic part of main/streams/cast.c that applies).
fn cast_inner_stream(
    vm: &mut Vm,
    res: &ResRef,
    as_: CastAs,
    report_errors: bool,
) -> PResult<Option<RawFd>> {
    let r = with_stream(vm, res, |vm, s| {
        if s.is_filtered() && as_ != CastAs::FdForSelect {
            stream_warn(vm, &s.st, "Cannot cast a filtered stream on this system")?;
            return Ok(None);
        }
        if let Some(fd) = s.cast(vm, as_, report_errors)? {
            return Ok(Some(fd));
        }
        let label = s.ops.label();
        stream_warn(
            vm,
            &s.st,
            &format!(
                "Cannot represent a stream of type {label} as a {}",
                cast_name(as_)
            ),
        )?;
        Ok(None)
    })?;
    Ok(r.flatten())
}

// ---------------------------------------------------------------------------
// php_stream_userspace_ops
// ---------------------------------------------------------------------------

/// `php_userstream_data_t` behind `php_stream_userspace_ops`.
struct UserStream {
    /// `us->object`; released by `close`.
    object: Option<ObjRef>,
    /// The stream's own resource id ("must not return itself").
    res_id: i64,
}

impl UserStream {
    fn new(object: ObjRef) -> UserStream {
        UserStream {
            object: Some(object),
            res_id: 0,
        }
    }

    /// `user_stream_set_check_liveliness`.
    fn check_liveliness(vm: &mut Vm, st: &mut StreamState, obj: &ObjRef) -> PResult<i32> {
        let Some(ret) = call_method_if_exists(vm, obj, USERSTREAM_EOF, Vec::new())? else {
            stream_warn(
                vm,
                st,
                &format!(
                    "{}::{USERSTREAM_EOF} is not implemented! Assuming EOF",
                    class_name(obj)
                ),
            )?;
            return Ok(opt::RETURN_ERR);
        };
        match ret {
            Value::Bool(true) => Ok(opt::RETURN_ERR),
            Value::Bool(false) => Ok(opt::RETURN_OK),
            other => {
                stream_warn(
                    vm,
                    st,
                    &format!(
                        "{}::{USERSTREAM_EOF} value must be of type bool, {} given",
                        class_name(obj),
                        other.value_name()
                    ),
                )?;
                Ok(opt::RETURN_ERR)
            }
        }
    }

    /// `user_stream_set_locking`: maps the system `LOCK_*` operation to PHP's
    /// `LOCK_*` constants.
    fn set_locking(vm: &mut Vm, st: &mut StreamState, obj: &ObjRef, value: i32) -> PResult<i32> {
        let mut lock = 0;
        if value & sys::LOCK_NB != 0 {
            lock |= PHP_LOCK_NB;
        }
        match value & !sys::LOCK_NB {
            sys::LOCK_SH => lock |= PHP_LOCK_SH,
            sys::LOCK_EX => lock |= PHP_LOCK_EX,
            sys::LOCK_UN => lock |= PHP_LOCK_UN,
            _ => {}
        }
        let Some(ret) = call_method_if_exists(vm, obj, USERSTREAM_LOCK, vec![Value::Int(lock)])?
        else {
            if value == 0 {
                // lock support test (TODO: more check)
                return Ok(opt::RETURN_OK);
            }
            stream_warn(
                vm,
                st,
                &format!("{}::{USERSTREAM_LOCK} is not implemented!", class_name(obj)),
            )?;
            return Ok(opt::RETURN_ERR);
        };
        match ret {
            // This is somewhat confusing and relies on magic numbers.
            Value::Bool(b) => Ok(i32::from(!b)),
            _ => Ok(opt::RETURN_NOTIMPL),
        }
    }

    /// `user_stream_set_truncation`.
    fn set_truncation(
        vm: &mut Vm,
        st: &mut StreamState,
        obj: &ObjRef,
        value: i32,
        param: &OptParam<'_>,
    ) -> PResult<i32> {
        if value == opt::TRUNCATE_SUPPORTED {
            return Ok(if resolve_method(vm, obj, USERSTREAM_TRUNCATE).is_some() {
                opt::RETURN_OK
            } else {
                opt::RETURN_ERR
            });
        }
        let new_size = match param {
            OptParam::Size(n) => *n,
            _ => return Ok(opt::RETURN_ERR),
        };
        if new_size < 0 {
            // bad new size
            return Ok(opt::RETURN_ERR);
        }
        let Some(ret) =
            call_method_if_exists(vm, obj, USERSTREAM_TRUNCATE, vec![Value::Int(new_size)])?
        else {
            stream_warn(
                vm,
                st,
                &format!(
                    "{}::{USERSTREAM_TRUNCATE} is not implemented!",
                    class_name(obj)
                ),
            )?;
            return Ok(opt::RETURN_ERR);
        };
        match ret {
            Value::Bool(true) => Ok(opt::RETURN_OK),
            Value::Bool(false) => Ok(opt::RETURN_ERR),
            other => {
                stream_warn(
                    vm,
                    st,
                    &format!(
                        "{}::{USERSTREAM_TRUNCATE} value must be of type bool, {} given",
                        class_name(obj),
                        other.value_name()
                    ),
                )?;
                Ok(opt::RETURN_ERR)
            }
        }
    }

    /// `user_stream_set_option`: blocking, read timeout and buffer options.
    fn set_user_option(
        vm: &mut Vm,
        st: &mut StreamState,
        obj: &ObjRef,
        option: i32,
        value: i32,
        param: &OptParam<'_>,
    ) -> PResult<i32> {
        let mut arg1 = Value::Int(value.into());
        let mut arg2 = Value::Null;
        if option == opt::READ_TIMEOUT {
            if let OptParam::Timeout(sec, usec) = param {
                arg1 = Value::Int(*sec);
                arg2 = Value::Int(*usec);
            }
        } else if option == opt::READ_BUFFER || option == opt::WRITE_BUFFER {
            arg2 = Value::Int(match param {
                OptParam::Size(n) => *n,
                _ => BUFSIZ,
            });
        }
        let args = vec![Value::Int(option.into()), arg1, arg2];
        let Some(ret) = call_method_if_exists(vm, obj, USERSTREAM_SET_OPTION, args)? else {
            stream_warn(
                vm,
                st,
                &format!(
                    "{}::{USERSTREAM_SET_OPTION} is not implemented!",
                    class_name(obj)
                ),
            )?;
            return Ok(opt::RETURN_ERR);
        };
        Ok(if vm.to_bool(&ret)? {
            opt::RETURN_OK
        } else {
            opt::RETURN_ERR
        })
    }
}

impl StreamOps for UserStream {
    fn label(&self) -> &'static str {
        LABEL
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Userspace
    }

    /// `php_userstreamop_write`.
    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        let Some(obj) = self.object.clone() else {
            return Ok(-1);
        };
        let count = buf.len();
        let ret = with_no_fclose(st, |_| {
            call_method_if_exists(vm, &obj, USERSTREAM_WRITE, vec![Value::from(buf)])
        })?;
        let Some(ret) = ret else {
            stream_warn(
                vm,
                st,
                &format!(
                    "{}::{USERSTREAM_WRITE} is not implemented!",
                    class_name(&obj)
                ),
            )?;
            return Ok(-1);
        };
        let mut didwrite = match ret {
            Value::Bool(false) => -1,
            other => vm.to_int(&other)?,
        };
        // don't allow strange buffer overruns due to bogus return
        if didwrite > 0 && didwrite as u64 > count as u64 {
            stream_warn(
                vm,
                st,
                &format!(
                    "{}::{USERSTREAM_WRITE} wrote {} bytes more data than requested ({didwrite} written, {count} max)",
                    class_name(&obj),
                    didwrite - count as i64
                ),
            )?;
            didwrite = count as i64;
        }
        Ok(didwrite as isize)
    }

    /// `php_userstreamop_read`.
    fn read(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        let Some(obj) = self.object.clone() else {
            return Ok(-1);
        };
        with_no_fclose(st, |st| {
            let count = buf.len();
            let args = vec![Value::Int(count as i64)];
            // zend_call_method_if_exists() leaves the return value UNDEF when the
            // method is missing, so php-src bails out before its "is not
            // implemented" warning.
            let Some(ret) = call_method_if_exists(vm, &obj, USERSTREAM_READ, args)? else {
                return Ok(-1);
            };
            if matches!(ret, Value::Bool(false)) {
                return Ok(-1);
            }
            let data = vm.to_php_string(&ret)?;
            let mut didread = data.len();
            if didread > 0 {
                if didread > count {
                    stream_warn(
                        vm,
                        st,
                        &format!(
                            "{}::{USERSTREAM_READ} - read {} bytes more data than requested ({didread} read, {count} max) - excess data will be lost",
                            class_name(&obj),
                            didread - count
                        ),
                    )?;
                    didread = count;
                }
                buf[..didread].copy_from_slice(&data[..didread]);
            }

            // since the user stream has no way of setting the eof flag directly,
            // we need to ask it if we hit eof
            let eof = match call_method_if_exists(vm, &obj, USERSTREAM_EOF, Vec::new()) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    stream_warn(
                        vm,
                        st,
                        &format!(
                            "{}::{USERSTREAM_EOF} is not implemented! Assuming EOF",
                            class_name(&obj)
                        ),
                    )?;
                    st.eof = true;
                    return Ok(-1);
                }
                Err(e) => {
                    st.eof = true;
                    return Err(e);
                }
            };
            if vm.to_bool(&eof)? {
                st.eof = true;
            }
            Ok(didread as isize)
        })
    }

    /// `php_userstreamop_close`.
    fn close(&mut self, vm: &mut Vm, _st: &mut StreamState, _close_handle: bool) -> PResult<i32> {
        if let Some(obj) = self.object.take() {
            call_method_if_exists(vm, &obj, USERSTREAM_CLOSE, Vec::new())?;
        }
        Ok(0)
    }

    /// `php_userstreamop_flush`.
    fn flush(&mut self, vm: &mut Vm, _st: &mut StreamState) -> PResult<i32> {
        let Some(obj) = self.object.clone() else {
            return Ok(-1);
        };
        Ok(
            match call_method_if_exists(vm, &obj, USERSTREAM_FLUSH, Vec::new())? {
                Some(v) if vm.to_bool(&v)? => 0,
                _ => -1,
            },
        )
    }

    fn can_seek(&self) -> bool {
        true
    }

    /// `php_userstreamop_seek`: `stream_seek`, then `stream_tell` for the new
    /// position.
    fn seek(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        let Some(obj) = self.object.clone() else {
            return Ok((-1, None));
        };
        with_no_fclose(st, |st| {
            let args = vec![Value::Int(offset), Value::Int(whence.into())];
            let ret = match call_method_if_exists(vm, &obj, USERSTREAM_SEEK, args)? {
                None => {
                    // stream_seek is not implemented, so disable seeks for this stream
                    st.flags |= flags::NO_SEEK;
                    return Ok((-1, None));
                }
                Some(v) => {
                    if vm.to_bool(&v)? {
                        0
                    } else {
                        -1
                    }
                }
            };
            if ret != 0 {
                return Ok((ret, None));
            }
            // now determine where we are
            match call_method_if_exists(vm, &obj, USERSTREAM_TELL, Vec::new())? {
                Some(Value::Int(pos)) => Ok((0, Some(pos))),
                None => {
                    stream_warn(
                        vm,
                        st,
                        &format!(
                            "{}::{USERSTREAM_TELL} is not implemented!",
                            class_name(&obj)
                        ),
                    )?;
                    Ok((-1, None))
                }
                Some(_) => Ok((-1, None)),
            }
        })
    }

    /// `php_userstreamop_cast`.
    fn cast_vm(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        as_: CastAs,
        report_errors: bool,
    ) -> PResult<Option<RawFd>> {
        let Some(obj) = self.object.clone() else {
            return Ok(None);
        };
        let self_id = self.res_id;
        // Userland only knows STREAM_CAST_FOR_SELECT
        let castas = if as_ == CastAs::FdForSelect {
            PHP_STREAM_AS_FD_FOR_SELECT
        } else {
            PHP_STREAM_AS_STDIO
        };
        with_no_fclose(st, |st| {
            let args = vec![Value::Int(castas)];
            let Some(ret) = call_method_if_exists(vm, &obj, USERSTREAM_CAST, args)? else {
                if report_errors {
                    stream_warn(
                        vm,
                        st,
                        &format!(
                            "{}::{USERSTREAM_CAST} is not implemented!",
                            class_name(&obj)
                        ),
                    )?;
                }
                return Ok(None);
            };
            if !vm.to_bool(&ret)? {
                return Ok(None);
            }
            // php-src also throws a TypeError from php_stream_from_zval_no_verify
            // here; it stays pending (and is usually replaced) in C, so only the
            // warning is reported.
            let inner = match &ret {
                Value::Resource(r) if handle_of(r).is_some() => r.clone(),
                _ => {
                    if report_errors {
                        stream_warn(
                            vm,
                            st,
                            &format!(
                                "{}::{USERSTREAM_CAST} must return a stream resource",
                                class_name(&obj)
                            ),
                        )?;
                    }
                    return Ok(None);
                }
            };
            if inner.id() == self_id {
                if report_errors {
                    stream_warn(
                        vm,
                        st,
                        &format!(
                            "{}::{USERSTREAM_CAST} must not return itself",
                            class_name(&obj)
                        ),
                    )?;
                }
                return Ok(None);
            }
            cast_inner_stream(vm, &inner, as_, report_errors)
        })
    }

    /// `php_userstreamop_stat`.
    fn stat(&mut self, vm: &mut Vm, st: &mut StreamState) -> PResult<Option<StatBuf>> {
        let Some(obj) = self.object.clone() else {
            return Ok(None);
        };
        match call_method_if_exists(vm, &obj, USERSTREAM_STAT, Vec::new())? {
            None => {
                stream_warn(
                    vm,
                    st,
                    &format!(
                        "{}::{USERSTREAM_STAT} is not implemented!",
                        class_name(&obj)
                    ),
                )?;
                Ok(None)
            }
            Some(Value::Array(a)) => Ok(Some(statbuf_from_array(&a))),
            Some(_) => Ok(None),
        }
    }

    /// `php_userstreamop_set_option`.
    fn set_option(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        let Some(obj) = self.object.clone() else {
            return Ok(opt::RETURN_NOTIMPL);
        };
        match option {
            opt::CHECK_LIVENESS => Self::check_liveliness(vm, st, &obj),
            opt::LOCKING => Self::set_locking(vm, st, &obj, value),
            opt::TRUNCATE_API => Self::set_truncation(vm, st, &obj, value, param),
            opt::READ_BUFFER | opt::WRITE_BUFFER | opt::READ_TIMEOUT | opt::BLOCKING => {
                Self::set_user_option(vm, st, &obj, option, value, param)
            }
            _ => Ok(opt::RETURN_NOTIMPL),
        }
    }

    fn needs_vm_to_close(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// php_stream_userspace_dir_ops
// ---------------------------------------------------------------------------

/// `php_userstream_data_t` behind `php_stream_userspace_dir_ops`.
struct UserDirStream {
    object: Option<ObjRef>,
}

impl StreamOps for UserDirStream {
    fn label(&self) -> &'static str {
        "user-space-dir"
    }

    fn can_write(&self) -> bool {
        false
    }

    /// Plain reads never match `sizeof(php_stream_dirent)`.
    fn read(&mut self, _vm: &mut Vm, _st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        Ok(-1)
    }

    /// `php_userstreamop_readdir`.
    fn readdir(&mut self, vm: &mut Vm, st: &mut StreamState) -> PResult<Option<Vec<u8>>> {
        let Some(obj) = self.object.clone() else {
            return Ok(None);
        };
        match call_method_if_exists(vm, &obj, USERSTREAM_DIR_READ, Vec::new())? {
            None => {
                stream_warn(
                    vm,
                    st,
                    &format!(
                        "{}::{USERSTREAM_DIR_READ} is not implemented!",
                        class_name(&obj)
                    ),
                )?;
                Ok(None)
            }
            Some(Value::Bool(_)) => Ok(None),
            Some(v) => {
                let name = vm.to_php_string(&v)?;
                // PHP_STRLCPY into d_name; readers take strlen(d_name)
                let name = &name[..name.len().min(NAME_MAX)];
                Ok(Some(c_str(name).to_vec()))
            }
        }
    }

    /// `php_userstreamop_closedir`.
    fn close(&mut self, vm: &mut Vm, _st: &mut StreamState, _close_handle: bool) -> PResult<i32> {
        if let Some(obj) = self.object.take() {
            call_method_if_exists(vm, &obj, USERSTREAM_DIR_CLOSE, Vec::new())?;
        }
        Ok(0)
    }

    fn can_seek(&self) -> bool {
        true
    }

    /// `php_userstreamop_rewinddir`.
    fn seek(
        &mut self,
        vm: &mut Vm,
        _st: &mut StreamState,
        _offset: i64,
        _whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        if let Some(obj) = self.object.clone() {
            call_method_if_exists(vm, &obj, USERSTREAM_DIR_REWIND, Vec::new())?;
        }
        Ok((0, None))
    }

    fn needs_vm_to_close(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// user_stream_wops
// ---------------------------------------------------------------------------

impl UserWrapper {
    /// `FG(user_stream_current_filename)` check: `false` (after logging
    /// "infinite recursion prevented") when `filename` is being opened already.
    fn enter_opener(vm: &mut Vm, filename: &[u8], options: i32) -> PResult<bool> {
        // Try to catch bad usage without preventing flexibility
        let g = vm.ext_data::<UserStreamGlobals>();
        if g.current_filename.as_deref() == Some(filename) {
            wrappers::log_error(vm, LABEL, options, "infinite recursion prevented".into())?;
            return Ok(false);
        }
        g.current_filename = Some(filename.to_vec());
        Ok(true)
    }

    fn leave_opener(vm: &mut Vm) {
        vm.ext_data::<UserStreamGlobals>().current_filename = None;
    }

    /// The body of `user_wrapper_opener` between the recursion guards.
    fn open_object(
        &self,
        vm: &mut Vm,
        filename: &[u8],
        mode: &[u8],
        options: i32,
        opened_path: &mut Option<PhpStr>,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let obj = create_object(vm, &self.ce, ctx)?;
        // call its stream_open method - set up params first
        let opened = PhpRef::new(Value::Null);
        let args = vec![
            Value::from(filename),
            Value::from(mode),
            Value::Int(options.into()),
            Value::Ref(opened.clone()),
        ];
        let Some(ret) = call_method_if_exists(vm, &obj, USERSTREAM_OPEN, args)? else {
            wrappers::log_error(
                vm,
                LABEL,
                options,
                format!("\"{}::{USERSTREAM_OPEN}\" is not implemented", self.ce.name),
            )?;
            return Ok(None);
        };
        if !vm.to_bool(&ret)? {
            wrappers::log_error(
                vm,
                LABEL,
                options,
                format!("\"{}::{USERSTREAM_OPEN}\" call failed", self.ce.name),
            )?;
            return Ok(None);
        }
        // the stream is now open!
        let res = alloc_user_stream(vm, Box::new(UserStream::new(obj.clone())), mode, obj);
        // if the opened path is set, copy it out
        if let Value::String(p) = opened.get() {
            *opened_path = Some(p);
        }
        Ok(Some(res))
    }

    /// The body of `user_wrapper_opendir` between the recursion guards.
    fn opendir_object(
        &self,
        vm: &mut Vm,
        filename: &[u8],
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let obj = create_object(vm, &self.ce, ctx)?;
        let args = vec![Value::from(filename), Value::Int(options.into())];
        let Some(ret) = call_method_if_exists(vm, &obj, USERSTREAM_DIR_OPEN, args)? else {
            wrappers::log_error(
                vm,
                LABEL,
                options,
                format!(
                    "\"{}::{USERSTREAM_DIR_OPEN}\" is not implemented",
                    self.ce.name
                ),
            )?;
            return Ok(None);
        };
        if !vm.to_bool(&ret)? {
            wrappers::log_error(
                vm,
                LABEL,
                options,
                format!("\"{}::{USERSTREAM_DIR_OPEN}\" call failed", self.ce.name),
            )?;
            return Ok(None);
        }
        let ops = Box::new(UserDirStream {
            object: Some(obj.clone()),
        });
        Ok(Some(alloc_user_stream(vm, ops, b"r", obj)))
    }

    /// Shared body of `user_wrapper_unlink`, `_rename`, `_mkdir`, `_rmdir` and
    /// `_metadata`: a fresh object, one call, `true` only for a `true` return.
    fn call_bool_method(
        &self,
        vm: &mut Vm,
        ctx: Option<&ResRef>,
        method: &str,
        args: Vec<Value>,
    ) -> PResult<bool> {
        let obj = create_object(vm, &self.ce, ctx)?;
        let ret = call_method_if_exists(vm, &obj, method, args)?;
        drop(obj);
        match ret {
            None => {
                vm.warning(format!("{}::{method} is not implemented!", self.ce.name))?;
                Ok(false)
            }
            Some(Value::Bool(b)) => Ok(b),
            Some(_) => Ok(false),
        }
    }
}

impl Wrapper for UserWrapper {
    fn label(&self) -> &'static str {
        LABEL
    }

    fn is_url(&self) -> bool {
        self.flags & PHP_STREAM_IS_URL != 0
    }

    /// `user_wrapper_opener`.
    fn open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        mode: &[u8],
        options: i32,
        opened_path: &mut Option<PhpStr>,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        if !Self::enter_opener(vm, path, options)? {
            return Ok(None);
        }
        // if the user stream was registered as local and we are in include
        // context, we add allow_url_include restrictions to allow_url_fopen ones
        let old_in_user_include = wrappers::globals(vm).in_user_include;
        if !self.is_url()
            && options & open::STREAM_OPEN_FOR_INCLUDE != 0
            && !vm.ini_bool("allow_url_include")
        {
            wrappers::globals(vm).in_user_include = true;
        }
        let r = self.open_object(vm, path, mode, options, opened_path, ctx);
        Self::leave_opener(vm);
        wrappers::globals(vm).in_user_include = old_in_user_include;
        r
    }

    fn has_url_stat(&self) -> bool {
        true
    }

    /// `user_wrapper_stat_url`.
    fn url_stat(
        &self,
        vm: &mut Vm,
        url: &[u8],
        flags: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<StatBuf>> {
        let obj = create_object(vm, &self.ce, ctx)?;
        let args = vec![Value::from(url), Value::Int(flags.into())];
        let ret = call_method_if_exists(vm, &obj, USERSTREAM_STATURL, args)?;
        drop(obj);
        match ret {
            None => {
                vm.warning(format!(
                    "{}::{USERSTREAM_STATURL} is not implemented!",
                    self.ce.name
                ))?;
                Ok(None)
            }
            Some(Value::Array(a)) => Ok(Some(statbuf_from_array(&a))),
            Some(_) => Ok(None),
        }
    }

    fn has_dir_opener(&self) -> bool {
        true
    }

    /// `user_wrapper_opendir`.
    fn dir_open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        if !Self::enter_opener(vm, path, options)? {
            return Ok(None);
        }
        let r = self.opendir_object(vm, path, options, ctx);
        Self::leave_opener(vm);
        r
    }

    fn has_unlink(&self) -> bool {
        true
    }

    /// `user_wrapper_unlink`.
    fn unlink(
        &self,
        vm: &mut Vm,
        url: &[u8],
        _options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        self.call_bool_method(vm, ctx, USERSTREAM_UNLINK, vec![Value::from(url)])
    }

    fn has_rename(&self) -> bool {
        true
    }

    /// `user_wrapper_rename`.
    fn rename(
        &self,
        vm: &mut Vm,
        from: &[u8],
        to: &[u8],
        _options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let args = vec![Value::from(from), Value::from(to)];
        self.call_bool_method(vm, ctx, USERSTREAM_RENAME, args)
    }

    fn has_mkdir(&self) -> bool {
        true
    }

    /// `user_wrapper_mkdir`.
    fn mkdir(
        &self,
        vm: &mut Vm,
        url: &[u8],
        mode: i64,
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let args = vec![
            Value::from(url),
            Value::Int(mode),
            Value::Int(options.into()),
        ];
        self.call_bool_method(vm, ctx, USERSTREAM_MKDIR, args)
    }

    fn has_rmdir(&self) -> bool {
        true
    }

    /// `user_wrapper_rmdir`.
    fn rmdir(&self, vm: &mut Vm, url: &[u8], options: i32, ctx: Option<&ResRef>) -> PResult<bool> {
        let args = vec![Value::from(url), Value::Int(options.into())];
        self.call_bool_method(vm, ctx, USERSTREAM_RMDIR, args)
    }

    fn has_metadata(&self) -> bool {
        true
    }

    /// `user_wrapper_metadata`: `stream_metadata($path, $option, $value)`.
    fn metadata(
        &self,
        vm: &mut Vm,
        url: &[u8],
        value: &MetaValue,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let (option, arg) = match value {
            MetaValue::Touch(times) => {
                let mut a = PhpArray::new();
                if let Some((mtime, atime)) = times {
                    let _ = a.append(*mtime);
                    let _ = a.append(*atime);
                }
                (wrappers::META_TOUCH, Value::from(a))
            }
            MetaValue::Group(n) => (wrappers::META_GROUP, Value::Int(*n)),
            MetaValue::Owner(n) => (wrappers::META_OWNER, Value::Int(*n)),
            MetaValue::Access(n) => (wrappers::META_ACCESS, Value::Int(*n)),
            MetaValue::GroupName(s) => (wrappers::META_GROUP_NAME, Value::String(s.clone())),
            MetaValue::OwnerName(s) => (wrappers::META_OWNER_NAME, Value::String(s.clone())),
        };
        let args = vec![Value::from(url), Value::Int(option), arg];
        self.call_bool_method(vm, ctx, USERSTREAM_METADATA, args)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------
// stream_wrapper_register & co.
// ---------------------------------------------------------------------------

/// `ZEND_ACC_UNINSTANTIABLE`.
fn is_uninstantiable(ce: &ClassEntry) -> bool {
    ce.is_interface()
        || ce.is_trait()
        || ce.is_enum()
        || ce.is_abstract()
        || ce.methods.values().any(|m| m.has_flag(fn_flags::ABSTRACT))
}

/// A message assembled from byte strings (protocols are C strings in php-src).
fn bytes_msg(parts: &[&[u8]]) -> PhpStr {
    PhpStr::from_vec(parts.concat())
}

/// `PHP_FUNCTION(stream_wrapper_register)`: registers a custom URL protocol
/// handler class.
fn stream_wrapper_register(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let protocol = args.str(0).clone();
    let class = args.str(1).clone();
    let wflags = args.int(2);
    // Z_PARAM_CLASS
    let Some(ce) = vm.lookup_class(class.as_bytes())? else {
        return Err(
            vm.argument_type_error(2, &format!("must be a valid class name, \"{class}\" given"))
        );
    };
    if is_uninstantiable(&ce) {
        return Err(vm.argument_value_error(2, "must be a concrete class"));
    }
    let resource = ResRef::new("stream factory", Box::new(()));
    let uwrap: WrapperRef = Rc::new(UserWrapper {
        protocol: protocol.clone(),
        class: ce.name.clone(),
        flags: wflags,
        ce: ce.clone(),
        resource: resource.clone(),
    });
    if wrappers::register_volatile(vm, &protocol, uwrap) {
        return Ok(Value::Bool(true));
    }
    // We failed.  But why?
    let proto = c_str(&protocol);
    let exists = wrappers::wrapper_table(vm)
        .iter()
        .any(|(n, _)| n.as_bytes() == protocol.as_bytes());
    if exists {
        vm.warning(bytes_msg(&[
            b"Protocol ",
            proto,
            b":// is already defined.",
        ]))?;
    } else {
        // Hash doesn't exist so it must have been an invalid protocol scheme
        vm.warning(bytes_msg(&[
            b"Invalid protocol scheme specified. Unable to register wrapper class ",
            c_str(&ce.name),
            b" to ",
            proto,
            b"://",
        ]))?;
    }
    resource.close();
    Ok(Value::Bool(false))
}

/// `PHP_FUNCTION(stream_wrapper_unregister)`: unregisters a wrapper for the
/// life of the current request.
fn stream_wrapper_unregister(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let protocol = args.str(0).clone();
    let wrapper = wrappers::wrapper_table(vm)
        .into_iter()
        .find(|(n, _)| n.as_bytes() == protocol.as_bytes())
        .map(|(_, w)| w);
    if !wrappers::unregister_volatile(vm, &protocol) {
        // We failed
        vm.warning(bytes_msg(&[
            b"Unable to unregister protocol ",
            c_str(&protocol),
            b"://",
        ]))?;
        return Ok(Value::Bool(false));
    }
    if let Some(uwrap) = wrapper
        .as_ref()
        .and_then(|w| w.as_any().downcast_ref::<UserWrapper>())
    {
        uwrap.resource.close();
    }
    Ok(Value::Bool(true))
}

/// `PHP_FUNCTION(stream_wrapper_restore)`: restores the original protocol
/// handler, overriding if necessary.
fn stream_wrapper_restore(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let protocol = args.str(0).clone();
    let proto = c_str(&protocol);
    let g = wrappers::globals(vm);
    let find = |t: &[(PhpStr, WrapperRef)]| {
        t.iter()
            .find(|(n, _)| n.as_bytes() == protocol.as_bytes())
            .map(|(_, w)| w.clone())
    };
    let Some(wrapper) = find(&g.global) else {
        vm.warning(bytes_msg(&[
            proto,
            b":// never existed, nothing to restore",
        ]))?;
        return Ok(Value::Bool(false));
    };
    let unchanged = match &g.volatile {
        None => true,
        Some(t) => find(t).is_some_and(|w| Rc::ptr_eq(&w, &wrapper)),
    };
    if unchanged {
        vm.notice(bytes_msg(&[
            proto,
            b":// was never changed, nothing to restore",
        ]))?;
        return Ok(Value::Bool(true));
    }
    // A failure here could be okay given that the protocol might have been merely unregistered
    wrappers::unregister_volatile(vm, &protocol);
    if !wrappers::register_volatile(vm, &protocol, wrapper) {
        vm.warning(bytes_msg(&[
            b"Unable to restore original ",
            proto,
            b":// wrapper",
        ]))?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

static STREAM_WRAPPER_REGISTER: NativeFunction = NativeFunction {
    sig: "stream_wrapper_register(string $protocol, string $class, int $flags = 0): bool",
    handler: stream_wrapper_register,
};

/// `@alias stream_wrapper_register`.
static STREAM_REGISTER_WRAPPER: NativeFunction = NativeFunction {
    sig: "stream_register_wrapper(string $protocol, string $class, int $flags = 0): bool",
    handler: stream_wrapper_register,
};

static STREAM_WRAPPER_UNREGISTER: NativeFunction = NativeFunction {
    sig: "stream_wrapper_unregister(string $protocol): bool",
    handler: stream_wrapper_unregister,
};

static STREAM_WRAPPER_RESTORE: NativeFunction = NativeFunction {
    sig: "stream_wrapper_restore(string $protocol): bool",
    handler: stream_wrapper_restore,
};
