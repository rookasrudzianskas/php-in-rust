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

#[cfg(test)]
mod tests {
    //! Expectations from ext/standard/tests/file/userstreams*.phpt,
    //! userwrapper.phpt, userdirstream.phpt and streams/bug76943.phpt, driven
    //! through internal classes whose methods are native.

    use super::*;
    use crate::standard::streams::{self as streams, wrappers::StatResult};
    use crate::testing::{TestVm, thrown};
    use php_runtime::Function;
    use php_runtime::class::PropertyInfo;
    use php_runtime::types::TypeDecl;
    use php_runtime::vm::Frame;

    // ----- helpers ---------------------------------------------------------

    fn this(args: &Args) -> ObjRef {
        args.this().cloned().expect("method called without $this")
    }

    fn sprop(o: &ObjRef, name: &str) -> Vec<u8> {
        match o.get_prop(name.as_bytes()) {
            Some(Value::String(s)) => s.to_vec(),
            _ => Vec::new(),
        }
    }

    fn iprop(o: &ObjRef, name: &str) -> i64 {
        o.get_prop(name.as_bytes())
            .map_or(0, |v| php_runtime::value::convert::to_int_silent(&v))
    }

    fn has(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    fn show(v: &Value) -> String {
        match v {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::String(s) => s.to_string(),
            Value::Array(a) => {
                let parts: Vec<String> = a.iter().map(|(_, v)| show(v)).collect();
                format!("[{}]", parts.join(","))
            }
            other => other.type_name().into_owned(),
        }
    }

    fn echo(vm: &mut Vm, line: String) -> PResult<()> {
        vm.echo(line.as_bytes())?;
        vm.echo(b"\n")
    }

    /// Runs `f` inside a pseudo frame of the built-in function `name`, so that
    /// warnings read "name(): ...".
    fn in_vm_fn<R>(vm: &mut Vm, name: &str, f: impl FnOnce(&mut Vm) -> R) -> R {
        let site = (vm.current_file.clone(), vm.current_line);
        vm.push_frame(Frame {
            function: None,
            name: PhpStr::from(name),
            class: None,
            call_type: None,
            this: None,
            call_site: Some(site),
            args: Vec::new(),
            is_internal: true,
        });
        let r = f(vm);
        vm.pop_frame();
        r
    }

    fn in_fn<R>(t: &mut TestVm, name: &str, f: impl FnOnce(&mut Vm) -> R) -> R {
        in_vm_fn(&mut t.vm, name, f)
    }

    /// `fopen($path, 'r')` called from inside a user method.
    fn nested_fopen(vm: &mut Vm, path: &[u8]) -> PResult<bool> {
        let r = in_vm_fn(vm, "fopen", |vm| streams::open(vm, path, b"r", false, None))?;
        Ok(r.is_some())
    }

    fn warning(func: &str, msg: &str) -> String {
        format!("\nWarning: {func}(): {msg} in /t.php on line 3\n")
    }

    fn notice(func: &str, msg: &str) -> String {
        format!("\nNotice: {func}(): {msg} in /t.php on line 3\n")
    }

    fn register(t: &mut TestVm, protocol: &str, class: &str) -> Value {
        t.call(
            "stream_wrapper_register",
            vec![Value::from(protocol), Value::from(class)],
        )
        .expect("stream_wrapper_register")
    }

    fn fopen(t: &mut TestVm, path: &str, mode: &str) -> Option<ResRef> {
        in_fn(t, "fopen", |vm| {
            streams::open(vm, path.as_bytes(), mode.as_bytes(), false, None)
        })
        .expect("fopen")
    }

    fn object_of(res: &ResRef) -> ObjRef {
        let h = handle_of(res).expect("stream");
        let s = h.borrow();
        match &s.wrapperdata {
            Some(Value::Object(o)) => o.clone(),
            other => panic!("wrapper_data is {other:?}"),
        }
    }

    // ----- the test classes --------------------------------------------------

    fn t_construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        this(args).set_prop(b"constructed", Value::Bool(true));
        Ok(Value::Null)
    }

    fn t_open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        let path = args.str(0).clone();
        echo(vm, format!("open {path} {} {}", args.str(1), args.int(2)))?;
        if has(&path, b"fail") {
            return Ok(Value::Bool(false));
        }
        o.set_prop(b"data", Value::from("hello world"));
        o.set_prop(b"pos", Value::Int(0));
        if has(&path, b"opened") {
            args.by_ref(3).set(Value::from("/real/opened"));
        }
        Ok(Value::Bool(true))
    }

    fn t_read(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        let count = args.int(0);
        echo(vm, format!("read {count}"))?;
        let data = sprop(&o, "data");
        let pos = (iprop(&o, "pos") as usize).min(data.len());
        let want = (count + iprop(&o, "extra")).max(0) as usize;
        let end = (pos + want).min(data.len());
        o.set_prop(b"pos", Value::Int(end as i64));
        Ok(Value::from(&data[pos..end]))
    }

    fn t_write(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        let chunk = args.str(0).clone();
        echo(vm, format!("write {}", chunk.len()))?;
        let mut data = sprop(&o, "data");
        data.extend_from_slice(&chunk);
        o.set_prop(b"data", Value::from(data));
        Ok(Value::Int(chunk.len() as i64 + iprop(&o, "extra")))
    }

    fn t_eof(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        Ok(Value::Bool(
            iprop(&o, "pos") >= sprop(&o, "data").len() as i64,
        ))
    }

    fn t_tell(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        Ok(Value::Int(iprop(&this(args), "pos")))
    }

    fn t_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        let (offset, whence) = (args.int(0), args.int(1));
        echo(vm, format!("seek {offset} {whence}"))?;
        let len = sprop(&o, "data").len() as i64;
        let pos = match whence {
            0 => offset,
            1 => iprop(&o, "pos") + offset,
            _ => len + offset,
        };
        if pos < 0 {
            return Ok(Value::Bool(false));
        }
        o.set_prop(b"pos", Value::Int(pos));
        Ok(Value::Bool(true))
    }

    fn t_flush(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        echo(vm, "flush".into())?;
        Ok(Value::Bool(true))
    }

    fn t_close(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        echo(vm, "close".into())?;
        Ok(Value::Null)
    }

    fn t_stat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, "stat".into())?;
        let mut a = PhpArray::new();
        let _ = a.append(7i64);
        a.insert_str("size", sprop(&this(args), "data").len() as i64);
        a.insert_str("mode", 0o100644i64);
        Ok(Value::from(a))
    }

    fn t_lock(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("lock {}", args.int(0)))?;
        Ok(Value::Bool(true))
    }

    fn t_truncate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("truncate {}", args.int(0)))?;
        Ok(Value::Bool(true))
    }

    fn t_set_option(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let line = format!(
            "set_option {} {} {}",
            args.int(0),
            args.int(1),
            show(args.get(2))
        );
        echo(vm, line)?;
        Ok(Value::Bool(args.int(0) != i64::from(opt::BLOCKING)))
    }

    fn t_cast(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("cast {}", args.int(0)))?;
        Ok(this(args).get_prop(b"cast").unwrap_or(Value::Null))
    }

    fn t_metadata(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let line = format!(
            "metadata {} {} {}",
            args.str(0),
            args.int(1),
            show(args.get(2))
        );
        echo(vm, line)?;
        Ok(Value::Bool(false))
    }

    fn t_url_stat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let path = args.str(0).clone();
        echo(vm, format!("url_stat {path} {}", args.int(1)))?;
        if has(&path, b"missing") {
            return Ok(Value::Bool(false));
        }
        let mut a = PhpArray::new();
        let _ = a.append(99i64);
        a.insert_str("size", 42i64);
        a.insert_str("mode", 0o040755i64);
        a.insert_str("mtime", "123abc");
        Ok(Value::from(a))
    }

    fn t_unlink(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("unlink {}", args.str(0)))?;
        Ok(Value::Bool(true))
    }

    fn t_rename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("rename {} {}", args.str(0), args.str(1)))?;
        Ok(Value::Bool(true))
    }

    fn t_mkdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let line = format!("mkdir {} {:o} {}", args.str(0), args.int(1), args.int(2));
        echo(vm, line)?;
        Ok(Value::Bool(true))
    }

    fn t_rmdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, format!("rmdir {} {}", args.str(0), args.int(1)))?;
        // only a real `true` counts
        Ok(Value::Int(1))
    }

    fn t_opendir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let path = args.str(0).clone();
        echo(vm, format!("opendir {path} {}", args.int(1)))?;
        let o = this(args);
        let mut entries = PhpArray::new();
        let _ = entries.append(".");
        let _ = entries.append("file");
        let _ = entries.append(Value::from(vec![b'x'; 300]));
        let _ = entries.append(Value::from(&b"nul\0cut"[..]));
        let _ = entries.append(Value::Null);
        o.set_prop(b"entries", Value::from(entries));
        o.set_prop(b"dirpos", Value::Int(0));
        Ok(Value::Bool(!has(&path, b"fail")))
    }

    fn t_readdir(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let o = this(args);
        let pos = iprop(&o, "dirpos");
        let next = match o.get_prop(b"entries") {
            Some(Value::Array(a)) => a.get_int(pos).cloned(),
            _ => None,
        };
        o.set_prop(b"dirpos", Value::Int(pos + 1));
        Ok(next.unwrap_or(Value::Bool(false)))
    }

    fn t_rewinddir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        echo(vm, "rewinddir".into())?;
        this(args).set_prop(b"dirpos", Value::Int(0));
        Ok(Value::Bool(true))
    }

    fn t_closedir(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        echo(vm, "closedir".into())?;
        Ok(Value::Bool(true))
    }

    fn t_minimal_open(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Ok(Value::Bool(true))
    }

    fn t_bad_truncate(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Ok(Value::from("kkk"))
    }

    fn t_bad_eof(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Ok(Value::Int(1))
    }

    fn t_throwing_read(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        echo(vm, "stream_read".into())?;
        Err(vm.throw_error("Exception", "Message"))
    }

    fn t_magic_call(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let name = args.str(0).clone();
        echo(vm, format!("__call {name} {}", args.array(1).len()))?;
        Ok(match name.as_bytes() {
            b"stream_open" | b"stream_eof" => Value::Bool(true),
            b"stream_read" => Value::from("abc"),
            _ => Value::Null,
        })
    }

    /// `stream_open` re-opening its own path (the infinite recursion guard).
    fn t_recursive_open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let path = args.str(0).clone();
        Ok(Value::Bool(nested_fopen(vm, &path)?))
    }

    /// A local wrapper whose `stream_open` opens a URL wrapper
    /// (include_userstream_002.phpt).
    fn t_include_open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let path = args.str(0).clone();
        if path.starts_with(b"inc://") {
            return Ok(Value::Bool(nested_fopen(vm, b"url://hello")?));
        }
        Ok(Value::Bool(true))
    }

    macro_rules! natives {
        ($($id:ident = $sig:literal => $f:ident;)*) => {
            $(static $id: NativeFunction = NativeFunction { sig: $sig, handler: $f };)*
        };
    }

    natives! {
        CONSTRUCT = "__construct()" => t_construct;
        OPEN = "stream_open(string $path, string $mode, int $options, &$opened_path): bool" => t_open;
        READ = "stream_read(int $count): string|false" => t_read;
        WRITE = "stream_write(string $data): int" => t_write;
        EOF = "stream_eof(): bool" => t_eof;
        TELL = "stream_tell(): int" => t_tell;
        SEEK = "stream_seek(int $offset, int $whence): bool" => t_seek;
        FLUSH = "stream_flush(): bool" => t_flush;
        CLOSE = "stream_close(): void" => t_close;
        STAT = "stream_stat(): array|false" => t_stat;
        LOCK = "stream_lock(int $operation): bool" => t_lock;
        TRUNCATE = "stream_truncate(int $new_size): bool" => t_truncate;
        SET_OPTION = "stream_set_option(int $option, int $arg1, ?int $arg2): bool" => t_set_option;
        CAST = "stream_cast(int $cast_as)" => t_cast;
        METADATA = "stream_metadata(string $path, int $option, mixed $value): bool" => t_metadata;
        URL_STAT = "url_stat(string $path, int $flags): array|false" => t_url_stat;
        UNLINK = "unlink(string $path): bool" => t_unlink;
        RENAME = "rename(string $path_from, string $path_to): bool" => t_rename;
        MKDIR = "mkdir(string $path, int $mode, int $options): bool" => t_mkdir;
        RMDIR = "rmdir(string $path, int $options)" => t_rmdir;
        DIR_OPEN = "dir_opendir(string $path, int $options): bool" => t_opendir;
        DIR_READ = "dir_readdir()" => t_readdir;
        DIR_REWIND = "dir_rewinddir(): bool" => t_rewinddir;
        DIR_CLOSE = "dir_closedir(): bool" => t_closedir;
        MINIMAL_OPEN = "stream_open(string $path, string $mode, int $options, &$opened_path): bool" => t_minimal_open;
        BAD_TRUNCATE = "stream_truncate(int $new_size)" => t_bad_truncate;
        BAD_EOF = "stream_eof()" => t_bad_eof;
        THROWING_READ = "stream_read(int $count): string" => t_throwing_read;
        MAGIC_CALL = "__call(string $name, array $arguments): mixed" => t_magic_call;
        RECURSIVE_OPEN = "stream_open(string $path, string $mode, int $options, &$opened_path): bool" => t_recursive_open;
        INCLUDE_OPEN = "stream_open(string $path, string $mode, int $options, &$opened_path): bool" => t_include_open;
    }

    const FULL: &[&NativeFunction] = &[
        &CONSTRUCT,
        &OPEN,
        &READ,
        &WRITE,
        &EOF,
        &TELL,
        &SEEK,
        &FLUSH,
        &CLOSE,
        &STAT,
        &LOCK,
        &TRUNCATE,
        &SET_OPTION,
        &CAST,
        &METADATA,
        &URL_STAT,
        &UNLINK,
        &RENAME,
        &MKDIR,
        &RMDIR,
        &DIR_OPEN,
        &DIR_READ,
        &DIR_REWIND,
        &DIR_CLOSE,
    ];

    /// Builds and registers an internal class `name` with `methods`; `context`
    /// declares `public $context` (typed when non-empty).
    fn class(
        t: &mut TestVm,
        name: &str,
        cflags: u32,
        context: Option<&str>,
        methods: &[(&'static NativeFunction, u32)],
    ) {
        let mut ce = ClassEntry::new(name, cflags);
        if let Some(ty) = context {
            let mut p = PropertyInfo::new("context", name, member_flags::PUBLIC);
            if !ty.is_empty() {
                p.ty = Some(TypeDecl::parse(ty).expect("type"));
                p.default = Value::Undef;
            }
            ce.declare_property(p);
        }
        for (nf, mflags) in methods {
            ce.add_method(Function::native_method(nf, name, *mflags));
        }
        Registry::new(&mut t.vm, "user_wrapper_test").class(ce);
    }

    fn public(fs: &[&'static NativeFunction]) -> Vec<(&'static NativeFunction, u32)> {
        fs.iter().map(|f| (*f, member_flags::PUBLIC)).collect()
    }

    fn setup() -> TestVm {
        let mut t = TestVm::new();
        class(&mut t, "TestWrapper", 0, Some(""), &public(FULL));
        class(
            &mut t,
            "MinimalWrapper",
            0,
            Some(""),
            &public(&[&MINIMAL_OPEN]),
        );
        t
    }

    // ----- registration ------------------------------------------------------

    #[test]
    fn register_unregister_and_restore() {
        let mut t = setup();
        assert!(matches!(
            register(&mut t, "test", "TestWrapper"),
            Value::Bool(true)
        ));
        let names = wrappers::wrapper_names(&mut t.vm);
        assert!(names.iter().any(|n| n.as_bytes() == b"test"));
        // the alias registers too
        let r = t.call(
            "stream_register_wrapper",
            vec![
                Value::from("test1"),
                Value::from("testwrapper"),
                Value::Int(PHP_STREAM_IS_URL),
            ],
        );
        assert!(matches!(r, Ok(Value::Bool(true))));
        let located = wrappers::locate_url_wrapper(&mut t.vm, b"test1://x", 0)
            .unwrap()
            .unwrap();
        assert!(located.wrapper.is_url());
        assert!(wrappers::is_user_wrapper(&located.wrapper));
        assert_eq!(t.output(), "");

        // unregistering a user wrapper releases its "stream factory" resource
        let factory = located
            .wrapper
            .as_any()
            .downcast_ref::<UserWrapper>()
            .unwrap()
            .resource
            .clone();
        assert_eq!(factory.type_name(), "stream factory");
        drop(located);
        let r = t.call("stream_wrapper_unregister", vec![Value::from("test1")]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        assert!(factory.is_closed());

        let r = t.call("stream_wrapper_unregister", vec![Value::from("nope")]);
        assert!(matches!(r, Ok(Value::Bool(false))));
        assert_eq!(
            t.output(),
            warning(
                "stream_wrapper_unregister",
                "Unable to unregister protocol nope://"
            )
        );

        // bug76943.phpt
        let r = t.call("stream_wrapper_restore", vec![Value::from("foo")]);
        assert!(matches!(r, Ok(Value::Bool(false))));
        assert_eq!(
            t.output(),
            warning(
                "stream_wrapper_restore",
                "foo:// never existed, nothing to restore"
            )
        );
        let r = t.call("stream_wrapper_restore", vec![Value::from("php")]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        assert_eq!(
            t.output(),
            notice(
                "stream_wrapper_restore",
                "php:// was never changed, nothing to restore"
            )
        );

        // replace a built-in wrapper, then restore it
        let r = t.call("stream_wrapper_unregister", vec![Value::from("data")]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        assert!(matches!(
            register(&mut t, "data", "TestWrapper"),
            Value::Bool(true)
        ));
        let res = fopen(&mut t, "data://text/plain,abc", "r").unwrap();
        assert_eq!(t.output(), "open data://text/plain,abc r 0\n");
        streams::close(&mut t.vm, &res).unwrap();
        assert_eq!(t.output(), "close\n");
        let r = t.call("stream_wrapper_restore", vec![Value::from("data")]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        let res = fopen(&mut t, "data://text/plain,abc", "r").unwrap();
        let data = streams::read(&mut t.vm, &res, 10).unwrap().unwrap();
        assert_eq!(data, b"abc");
        assert_eq!(t.output(), "");
        // merely unregistered, then restored
        t.call("stream_wrapper_unregister", vec![Value::from("data")])
            .unwrap();
        let r = t.call("stream_wrapper_restore", vec![Value::from("data")]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn registration_errors() {
        let mut t = setup();
        assert!(matches!(
            register(&mut t, "file", "TestWrapper"),
            Value::Bool(false)
        ));
        assert_eq!(
            t.output(),
            warning(
                "stream_wrapper_register",
                "Protocol file:// is already defined."
            )
        );
        assert!(matches!(
            register(&mut t, "test", "TestWrapper"),
            Value::Bool(true)
        ));
        assert!(matches!(
            register(&mut t, "test", "MinimalWrapper"),
            Value::Bool(false)
        ));
        assert_eq!(
            t.output(),
            warning(
                "stream_wrapper_register",
                "Protocol test:// is already defined."
            )
        );
        assert!(matches!(
            register(&mut t, "te st", "TestWrapper"),
            Value::Bool(false)
        ));
        assert_eq!(
            t.output(),
            warning(
                "stream_wrapper_register",
                "Invalid protocol scheme specified. Unable to register wrapper class TestWrapper to te st://"
            )
        );

        // stream_wrapper_register_error.phpt
        let r = t.call(
            "stream_wrapper_register",
            vec![Value::from("foo"), Value::from("class_not_exist")],
        );
        assert_eq!(
            thrown(r),
            (
                "TypeError".into(),
                "stream_wrapper_register(): Argument #2 ($class) must be a valid class name, \"class_not_exist\" given".into()
            )
        );
        class(&mut t, "AbstractWrapper", class_flags::ABSTRACT, None, &[]);
        class(
            &mut t,
            "WrapperInterface",
            class_flags::INTERFACE,
            None,
            &[],
        );
        for c in ["AbstractWrapper", "WrapperInterface"] {
            let r = t.call(
                "stream_wrapper_register",
                vec![Value::from("foo"), Value::from(c)],
            );
            assert_eq!(
                thrown(r),
                (
                    "ValueError".into(),
                    "stream_wrapper_register(): Argument #2 ($class) must be a concrete class"
                        .into()
                )
            );
        }
        assert_eq!(t.output(), "");
    }

    // ----- stream ops --------------------------------------------------------

    #[test]
    fn read_write_seek_and_close() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://hello", "r+").unwrap();
        assert_eq!(res.type_name(), "stream");
        assert_eq!(t.output(), "open test://hello r+ 0\n");

        let obj = object_of(&res);
        assert_eq!(obj.class().name.as_bytes(), b"TestWrapper");
        assert!(matches!(
            obj.get_prop(b"constructed"),
            Some(Value::Bool(true))
        ));
        assert!(matches!(obj.get_prop(b"context"), Some(Value::Null)));

        let vm = &mut t.vm;
        assert_eq!(streams::read(vm, &res, 5).unwrap().unwrap(), b"hello");
        assert_eq!(streams::tell(&res), Some(5));
        // the second read drains the buffer, then asks the object again
        assert_eq!(streams::read(vm, &res, 100).unwrap().unwrap(), b" world");
        assert!(streams::eof(vm, &res).unwrap());
        assert_eq!(t.output(), "read 8192\nread 8192\n");

        let vm = &mut t.vm;
        assert_eq!(streams::seek(vm, &res, 6, 0).unwrap(), 0);
        assert_eq!(streams::tell(&res), Some(6));
        assert!(!streams::eof(vm, &res).unwrap());
        assert_eq!(streams::read(vm, &res, 3).unwrap().unwrap(), b"wor");
        assert_eq!(streams::write(vm, &res, b"!!").unwrap(), Some(2));
        assert_eq!(t.output(), "seek 6 0\nread 8192\nseek 9 0\nwrite 2\n");
        assert!(streams::flush(&mut t.vm, &res).unwrap());
        // nothing written since the flush: closing does not flush again
        assert!(streams::close(&mut t.vm, &res).unwrap());
        assert_eq!(t.output(), "flush\nclose\n");
        assert!(res.is_closed());
    }

    #[test]
    fn opened_path_and_options() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let mut opened = None;
        let res = in_fn(&mut t, "fopen", |vm| {
            wrappers::open_wrapper(
                vm,
                b"test://opened",
                b"rb",
                open::REPORT_ERRORS | open::STREAM_MUST_SEEK,
                Some(&mut opened),
                None,
            )
        })
        .unwrap();
        assert!(res.is_some());
        assert_eq!(opened.unwrap().as_bytes(), b"/real/opened");
        assert_eq!(t.output(), "open test://opened rb 16\n");

        // append mode asks the object where it is
        let res = fopen(&mut t, "test://x", "a").unwrap();
        assert_eq!(t.output(), "open test://x a 0\nseek 0 1\n");
        assert_eq!(streams::tell(&res), Some(0));
    }

    #[test]
    fn open_failures() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        class(&mut t, "EmptyWrapper", 0, Some(""), &[]);
        register(&mut t, "empty", "EmptyWrapper");
        assert!(fopen(&mut t, "test://fail", "r").is_none());
        assert_eq!(
            t.output(),
            format!(
                "open test://fail r 0\n{}",
                warning(
                    "fopen",
                    "Failed to open stream: \"TestWrapper::stream_open\" call failed"
                )
            )
        );
        assert!(fopen(&mut t, "empty://x", "r").is_none());
        assert_eq!(
            t.output(),
            warning(
                "fopen",
                "Failed to open stream: \"EmptyWrapper::stream_open\" is not implemented"
            )
        );
        // with REPORT_ERRORS the opener reports right away
        let r = in_fn(&mut t, "fopen", |vm| {
            let mut op = None;
            let w = wrappers::locate_url_wrapper(vm, b"empty://x", 0)?.unwrap();
            w.wrapper
                .open(vm, b"empty://x", b"r", open::REPORT_ERRORS, &mut op, None)
        })
        .unwrap();
        assert!(r.is_none());
        assert_eq!(
            t.output(),
            warning("fopen", "\"EmptyWrapper::stream_open\" is not implemented")
        );
    }

    #[test]
    fn infinite_recursion_is_prevented() {
        let mut t = TestVm::new();
        let methods = public(&[&RECURSIVE_OPEN]);
        class(&mut t, "RecursiveWrapper", 0, Some(""), &methods);
        register(&mut t, "rec", "RecursiveWrapper");
        assert!(fopen(&mut t, "rec://loop", "r").is_none());
        assert_eq!(
            t.output(),
            format!(
                "{}{}",
                warning(
                    "fopen",
                    "Failed to open stream: infinite recursion prevented"
                ),
                warning(
                    "fopen",
                    "Failed to open stream: \"RecursiveWrapper::stream_open\" call failed"
                )
            )
        );
        let g = t.vm.ext_data::<UserStreamGlobals>();
        assert!(g.current_filename.is_none());
    }

    #[test]
    fn local_wrapper_in_include_obeys_allow_url_include() {
        // include_userstream_002.phpt
        let mut t = TestVm::new();
        let methods = public(&[&INCLUDE_OPEN]);
        class(&mut t, "IncludeWrapper", 0, Some(""), &methods);
        let r = t.call(
            "stream_wrapper_register",
            vec![
                Value::from("url"),
                Value::from("IncludeWrapper"),
                Value::Int(PHP_STREAM_IS_URL),
            ],
        );
        assert!(matches!(r, Ok(Value::Bool(true))));
        register(&mut t, "inc", "IncludeWrapper");
        let opts = open::REPORT_ERRORS | open::STREAM_OPEN_FOR_INCLUDE;
        let r = in_fn(&mut t, "include", |vm| {
            wrappers::open_wrapper(vm, b"inc://hello", b"rb", opts, None, None)
        })
        .unwrap();
        assert!(r.is_none());
        assert_eq!(
            t.output(),
            format!(
                "{}{}{}",
                warning(
                    "fopen",
                    "url:// wrapper is disabled in the server configuration by allow_url_include=0"
                ),
                warning(
                    "fopen",
                    "Failed to open stream: no suitable wrapper could be found"
                ),
                warning(
                    "include",
                    "Failed to open stream: \"IncludeWrapper::stream_open\" call failed"
                )
            )
        );
        assert!(!wrappers::globals(&mut t.vm).in_user_include);
        // outside of include the nested URL open is fine
        assert!(fopen(&mut t, "inc://hello", "r").is_some());
        assert_eq!(t.output(), "");
    }

    #[test]
    fn oversized_returns_are_clamped() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://big", "r+").unwrap();
        let obj = object_of(&res);
        obj.set_prop(b"data", Value::from(vec![b'a'; 9000]));
        obj.set_prop(b"extra", Value::Int(10));
        t.output();
        let data = in_fn(&mut t, "fread", |vm| streams::read(vm, &res, 8192));
        assert_eq!(data.unwrap().unwrap().len(), 8192);
        assert_eq!(
            t.output(),
            format!(
                "read 8192\n{}",
                warning(
                    "fread",
                    "TestWrapper::stream_read - read 10 bytes more data than requested (8202 read, 8192 max) - excess data will be lost"
                )
            )
        );
        let n = in_fn(&mut t, "fwrite", |vm| streams::write(vm, &res, b"xyz"));
        assert_eq!(n.unwrap(), Some(3));
        assert_eq!(
            t.output(),
            format!(
                "write 3\n{}",
                warning(
                    "fwrite",
                    "TestWrapper::stream_write wrote 10 bytes more data than requested (13 written, 3 max)"
                )
            )
        );
    }

    #[test]
    fn write_chunks_follow_the_chunk_size() {
        // userstreams_006.phpt
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://foo", "r").unwrap();
        t.output();
        let old = with_stream(&mut t.vm, &res, |vm, s| {
            let mut size = OptParam::Size(50);
            let w = s.set_option(vm, opt::WRITE_BUFFER, opt::BUFFER_FULL, &mut size)?;
            assert_eq!(w, opt::RETURN_OK);
            s.set_option(vm, opt::SET_CHUNK_SIZE, 42, &mut OptParam::None)
        })
        .unwrap()
        .unwrap();
        assert_eq!(old, 8192);
        let n = streams::write(&mut t.vm, &res, &[b'0'; 70]).unwrap();
        assert_eq!(n, Some(70));
        assert_eq!(t.output(), "set_option 3 2 50\nwrite 42\nwrite 28\n");
    }

    #[test]
    fn set_option_mapping() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://foo", "r").unwrap();
        t.output();
        let rets = with_stream(&mut t.vm, &res, |vm, s| {
            Ok([
                s.set_option(vm, opt::BLOCKING, 0, &mut OptParam::None)?,
                s.set_option(vm, opt::READ_TIMEOUT, 0, &mut OptParam::Timeout(5, 10))?,
                s.set_option(vm, opt::READ_BUFFER, opt::BUFFER_NONE, &mut OptParam::None)?,
                s.set_option(vm, opt::META_DATA_API, 0, &mut OptParam::None)?,
            ])
        })
        .unwrap()
        .unwrap();
        // the user's `false` for BLOCKING is an error; READ_BUFFER is not
        // applied by the core because the wrapper handled it
        assert_eq!(
            rets,
            [
                opt::RETURN_ERR,
                opt::RETURN_OK,
                opt::RETURN_OK,
                opt::RETURN_NOTIMPL
            ]
        );
        assert_eq!(
            t.output(),
            format!("set_option 1 0 null\nset_option 4 5 10\nset_option 2 0 {BUFSIZ}\n")
        );
        let sflags = handle_of(&res).unwrap().borrow().st.flags;
        assert_eq!(sflags & flags::NO_BUFFER, 0);
    }

    #[test]
    fn lock_operations_are_mapped() {
        // userstreams_004.phpt
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://foo", "r").unwrap();
        t.output();
        let rets = with_stream(&mut t.vm, &res, |vm, s| {
            Ok([
                i32::from(s.supports_lock(vm)?),
                s.lock(vm, sys::LOCK_SH)?,
                s.lock(vm, sys::LOCK_EX | sys::LOCK_NB)?,
                s.lock(vm, sys::LOCK_UN)?,
            ])
        })
        .unwrap()
        .unwrap();
        assert_eq!(rets, [1, 0, 0, 0]);
        assert_eq!(t.output(), "lock 0\nlock 1\nlock 6\nlock 3\n");
    }

    #[test]
    fn truncate() {
        // userstreams_005.phpt
        let mut t = setup();
        let bad = [
            (&MINIMAL_OPEN, member_flags::PUBLIC),
            (&BAD_TRUNCATE, member_flags::PUBLIC),
        ];
        class(&mut t, "BadWrapper", 0, Some(""), &bad);
        register(&mut t, "test", "TestWrapper");
        register(&mut t, "test2", "MinimalWrapper");
        register(&mut t, "test3", "BadWrapper");
        let fd = fopen(&mut t, "test://foo", "r").unwrap();
        let fd2 = fopen(&mut t, "test2://foo", "r").unwrap();
        let fd3 = fopen(&mut t, "test3://foo", "r").unwrap();
        t.output();
        let supported = |t: &mut TestVm, r: &ResRef| {
            with_stream(&mut t.vm, r, |vm, s| s.truncate_supported(vm))
                .unwrap()
                .unwrap()
        };
        assert!(!supported(&mut t, &fd2));
        assert!(supported(&mut t, &fd));
        let set = |t: &mut TestVm, r: &ResRef, n: i64| {
            in_fn(t, "ftruncate", |vm| {
                with_stream(vm, r, |vm, s| s.truncate_set_size(vm, n))
            })
            .unwrap()
            .unwrap()
        };
        assert_eq!(set(&mut t, &fd, 0), opt::RETURN_OK);
        assert_eq!(set(&mut t, &fd, 10), opt::RETURN_OK);
        assert_eq!(set(&mut t, &fd, -1), opt::RETURN_ERR);
        assert_eq!(t.output(), "truncate 0\ntruncate 10\n");
        assert_eq!(set(&mut t, &fd3, 0), opt::RETURN_ERR);
        assert_eq!(
            t.output(),
            warning(
                "ftruncate",
                "BadWrapper::stream_truncate value must be of type bool, string given"
            )
        );
        assert_eq!(set(&mut t, &fd2, 0), opt::RETURN_ERR);
        assert_eq!(
            t.output(),
            warning(
                "ftruncate",
                "MinimalWrapper::stream_truncate is not implemented!"
            )
        );
    }

    #[test]
    fn cast_returns_the_inner_stream() {
        // userstreams_002.phpt
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        register(&mut t, "test2", "MinimalWrapper");
        let fd = fopen(&mut t, "test://foo", "r").unwrap();
        let fd2 = fopen(&mut t, "test2://foo", "r").unwrap();
        t.output();
        let obj = object_of(&fd);
        let cast = |t: &mut TestVm, r: &ResRef| {
            in_fn(t, "stream_select", |vm| {
                with_stream(vm, r, |vm, s| s.cast(vm, CastAs::FdForSelect, true))
            })
            .unwrap()
            .unwrap()
        };

        let stdin = t.vm.constant("STDIN").unwrap();
        let stdin_fd = with_stream(&mut t.vm, stdin.as_resource().unwrap(), |_, s| {
            Ok(s.cast_fd(CastAs::FdForSelect))
        });
        let stdin_fd = stdin_fd.unwrap().unwrap();
        assert!(stdin_fd.is_some());
        obj.set_prop(b"cast", stdin);
        assert_eq!(cast(&mut t, &fd), stdin_fd);
        assert_eq!(t.output(), "cast 3\n");

        assert_eq!(cast(&mut t, &fd2), None);
        assert_eq!(
            t.output(),
            warning(
                "stream_select",
                "MinimalWrapper::stream_cast is not implemented!"
            )
        );
        // probing does not report
        let probe = with_stream(&mut t.vm, &fd2, |vm, s| {
            s.cast(vm, CastAs::FdForSelect, false)
        });
        assert_eq!(probe.unwrap().unwrap(), None);
        assert_eq!(t.output(), "");

        obj.set_prop(b"cast", Value::Bool(false));
        assert_eq!(cast(&mut t, &fd), None);
        assert_eq!(t.output(), "cast 3\n");

        obj.set_prop(b"cast", Value::from("foo"));
        assert_eq!(cast(&mut t, &fd), None);
        assert_eq!(
            t.output(),
            format!(
                "cast 3\n{}",
                warning(
                    "stream_select",
                    "TestWrapper::stream_cast must return a stream resource"
                )
            )
        );

        obj.set_prop(b"cast", Value::Resource(fd.clone()));
        assert_eq!(cast(&mut t, &fd), None);
        assert_eq!(
            t.output(),
            format!(
                "cast 3\n{}",
                warning(
                    "stream_select",
                    "TestWrapper::stream_cast must not return itself"
                )
            )
        );

        obj.set_prop(b"cast", Value::Resource(fd2.clone()));
        assert_eq!(cast(&mut t, &fd), None);
        assert_eq!(
            t.output(),
            format!(
                "cast 3\n{}{}",
                warning(
                    "stream_select",
                    "MinimalWrapper::stream_cast is not implemented!"
                ),
                warning(
                    "stream_select",
                    "Cannot represent a stream of type user-space as a select()able descriptor"
                )
            )
        );
        // other casts are offered as STREAM_CAST_AS_STREAM
        obj.set_prop(b"cast", Value::Null);
        let r = with_stream(&mut t.vm, &fd, |vm, s| s.cast(vm, CastAs::Fd, true));
        assert_eq!(r.unwrap().unwrap(), None);
        assert_eq!(t.output(), "cast 0\n");
    }

    #[test]
    fn stat_and_url_stat() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://foo", "r").unwrap();
        t.output();
        let sb = streams::stat(&mut t.vm, &res).unwrap().unwrap();
        assert_eq!((sb.size, sb.mode, sb.dev), (11, 0o100644, 0));
        assert_eq!(t.output(), "stat\n");

        let quiet = wrappers::URL_STAT_QUIET;
        let r = wrappers::stat_path(&mut t.vm, b"test://dir", quiet, None);
        let StatResult::Ok(sb) = r.unwrap() else {
            panic!("url_stat failed")
        };
        assert!(sb.is_dir());
        assert_eq!((sb.size, sb.mtime, sb.dev), (42, 123, 0));
        assert!(matches!(
            wrappers::stat_path(&mut t.vm, b"test://missing", 0, None).unwrap(),
            StatResult::Failed
        ));
        assert_eq!(
            t.output(),
            "url_stat test://dir 2\nurl_stat test://missing 0\n"
        );
    }

    #[test]
    fn wrapper_ops_and_metadata() {
        // userstreams_007.phpt
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let w = wrappers::locate_url_wrapper(&mut t.vm, b"test://x", 0)
            .unwrap()
            .unwrap()
            .wrapper;
        let vm = &mut t.vm;
        assert!(
            w.unlink(vm, b"test://a", open::REPORT_ERRORS, None)
                .unwrap()
        );
        assert!(w.rename(vm, b"test://a", b"test://b", 0, None).unwrap());
        let recursive = wrappers::MKDIR_RECURSIVE;
        assert!(w.mkdir(vm, b"test://d", 0o755, recursive, None).unwrap());
        // rmdir returns 1, not true
        assert!(!w.rmdir(vm, b"test://d", 0, None).unwrap());
        for v in [
            MetaValue::Touch(None),
            MetaValue::Touch(Some((1, 2))),
            MetaValue::OwnerName(PhpStr::from("test")),
            MetaValue::Owner(42),
            MetaValue::GroupName(PhpStr::from("test")),
            MetaValue::Group(42),
            MetaValue::Access(0o755),
        ] {
            assert!(!w.metadata(vm, b"test://testdir/touch", &v, None).unwrap());
        }
        assert_eq!(
            t.output(),
            "unlink test://a\n\
             rename test://a test://b\n\
             mkdir test://d 755 1\n\
             rmdir test://d 0\n\
             metadata test://testdir/touch 1 []\n\
             metadata test://testdir/touch 1 [1,2]\n\
             metadata test://testdir/touch 2 test\n\
             metadata test://testdir/touch 3 42\n\
             metadata test://testdir/touch 4 test\n\
             metadata test://testdir/touch 5 42\n\
             metadata test://testdir/touch 6 493\n"
        );
    }

    #[test]
    fn missing_methods_warn() {
        let mut t = setup();
        register(&mut t, "test2", "MinimalWrapper");
        let res = fopen(&mut t, "test2://foo", "r+").unwrap();
        assert_eq!(t.output(), "");
        // php_userstreamop_read fails silently
        assert_eq!(streams::read(&mut t.vm, &res, 10).unwrap(), None);
        assert_eq!(t.output(), "");
        let vm = &mut t.vm;
        assert_eq!(streams::write(vm, &res, b"abc").unwrap(), None);
        assert!(streams::eof(vm, &res).unwrap());
        assert_eq!(streams::stat(vm, &res).unwrap(), None);
        let lock = with_stream(vm, &res, |vm, s| {
            Ok((s.supports_lock(vm)?, s.lock(vm, sys::LOCK_EX)?))
        });
        assert_eq!(lock.unwrap().unwrap(), (true, opt::RETURN_ERR));
        let blocking = with_stream(vm, &res, |vm, s| {
            s.set_option(vm, opt::BLOCKING, 1, &mut OptParam::None)
        });
        assert_eq!(blocking.unwrap().unwrap(), opt::RETURN_ERR);
        assert_eq!(
            t.output(),
            [
                warning("main", "MinimalWrapper::stream_write is not implemented!"),
                warning(
                    "main",
                    "MinimalWrapper::stream_eof is not implemented! Assuming EOF"
                ),
                warning("main", "MinimalWrapper::stream_stat is not implemented!"),
                warning("main", "MinimalWrapper::stream_lock is not implemented!"),
                warning(
                    "main",
                    "MinimalWrapper::stream_set_option is not implemented!"
                ),
            ]
            .concat()
        );
        // no stream_seek: seeking gets disabled
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), -1);
        let sflags = handle_of(&res).unwrap().borrow().st.flags;
        assert_ne!(sflags & flags::NO_SEEK, 0);
        assert_eq!(
            t.output(),
            warning("main", "Stream does not support seeking")
        );
        // stream_close is optional, but the failed write left the stream
        // dirty and the final flush (no stream_flush) fails fclose()
        assert!(!streams::close(&mut t.vm, &res).unwrap());
        assert!(res.is_closed());

        let w = wrappers::locate_url_wrapper(&mut t.vm, b"test2://x", 0)
            .unwrap()
            .unwrap()
            .wrapper;
        let vm = &mut t.vm;
        assert!(w.url_stat(vm, b"test2://x", 0, None).unwrap().is_none());
        assert!(!w.unlink(vm, b"test2://x", 0, None).unwrap());
        assert!(!w.rename(vm, b"test2://x", b"test2://y", 0, None).unwrap());
        assert!(!w.mkdir(vm, b"test2://x", 0o777, 0, None).unwrap());
        assert!(!w.rmdir(vm, b"test2://x", 0, None).unwrap());
        let access = MetaValue::Access(0);
        assert!(!w.metadata(vm, b"test2://x", &access, None).unwrap());
        assert_eq!(
            t.output(),
            [
                warning("main", "MinimalWrapper::url_stat is not implemented!"),
                warning("main", "MinimalWrapper::unlink is not implemented!"),
                warning("main", "MinimalWrapper::rename is not implemented!"),
                warning("main", "MinimalWrapper::mkdir is not implemented!"),
                warning("main", "MinimalWrapper::rmdir is not implemented!"),
                warning(
                    "main",
                    "MinimalWrapper::stream_metadata is not implemented!"
                ),
            ]
            .concat()
        );
        let d = in_fn(&mut t, "opendir", |vm| {
            wrappers::opendir(vm, b"test2://dir", open::REPORT_ERRORS, None)
        });
        assert!(d.unwrap().is_none());
        assert_eq!(
            t.output(),
            warning(
                "opendir",
                "Failed to open directory: \"MinimalWrapper::dir_opendir\" is not implemented"
            )
        );
    }

    #[test]
    fn eof_must_return_bool() {
        let mut t = TestVm::new();
        let methods = [
            (&MINIMAL_OPEN, member_flags::PUBLIC),
            (&BAD_EOF, member_flags::PUBLIC),
        ];
        class(&mut t, "BadEof", 0, Some(""), &methods);
        register(&mut t, "test", "BadEof");
        let res = fopen(&mut t, "test://x", "r").unwrap();
        assert!(in_fn(&mut t, "feof", |vm| streams::eof(vm, &res)).unwrap());
        assert_eq!(
            t.output(),
            warning(
                "feof",
                "BadEof::stream_eof value must be of type bool, int given"
            )
        );
    }

    #[test]
    fn directory_streams() {
        // userdirstream.phpt
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let path = b"test://example.com/path/to/test.php";
        let d = in_fn(&mut t, "opendir", |vm| {
            wrappers::opendir(vm, path, open::REPORT_ERRORS, None)
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            t.output(),
            "opendir test://example.com/path/to/test.php 0\n"
        );
        let h = handle_of(&d).unwrap();
        assert_eq!(h.borrow().ops.label(), "user-space-dir");
        drop(h);
        let mut names = Vec::new();
        while let Some(n) = with_stream(&mut t.vm, &d, |vm, s| s.readdir(vm))
            .unwrap()
            .unwrap()
        {
            names.push(n);
        }
        assert_eq!(
            names,
            vec![
                b".".to_vec(),
                b"file".to_vec(),
                vec![b'x'; 255],
                b"nul".to_vec(),
                Vec::new()
            ]
        );
        let rewound = with_stream(&mut t.vm, &d, |vm, s| s.rewind(vm));
        assert_eq!(rewound.unwrap(), Some(0));
        let first = with_stream(&mut t.vm, &d, |vm, s| s.readdir(vm)).unwrap();
        assert_eq!(first, Some(Some(b".".to_vec())));
        // directory streams are not writable
        assert_eq!(streams::write(&mut t.vm, &d, b"x").unwrap(), None);
        assert!(streams::close(&mut t.vm, &d).unwrap());
        assert_eq!(
            t.output(),
            format!(
                "rewinddir\n{}closedir\n",
                notice("main", "Stream is not writable")
            )
        );

        let failed = in_fn(&mut t, "opendir", |vm| {
            wrappers::opendir(vm, b"test://fail", open::REPORT_ERRORS, None)
        });
        assert!(failed.unwrap().is_none());
        assert_eq!(
            t.output(),
            format!(
                "opendir test://fail 0\n{}",
                warning(
                    "opendir",
                    "Failed to open directory: \"TestWrapper::dir_opendir\" call failed"
                )
            )
        );
    }

    #[test]
    fn implicit_close_is_deferred() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let res = fopen(&mut t, "test://foo", "r+").unwrap();
        streams::write(&mut t.vm, &res, b"x").unwrap();
        assert_eq!(t.output(), "open test://foo r+ 0\nwrite 1\n");
        drop(res);
        assert_eq!(t.output(), "");
        streams::process_deferred_closes(&mut t.vm).unwrap();
        assert_eq!(t.output(), "flush\nclose\n");
    }

    #[test]
    fn calls_through_magic_call() {
        // Zend/tests/exceptions/exception_stream_wrapper.phpt (without the throw)
        let mut t = TestVm::new();
        class(&mut t, "Loader", 0, Some(""), &public(&[&MAGIC_CALL]));
        register(&mut t, "abc", "Loader");
        let res = fopen(&mut t, "abc://", "r").unwrap();
        let data = streams::read(&mut t.vm, &res, 10).unwrap().unwrap();
        assert_eq!(data, b"abc");
        assert!(streams::stat(&mut t.vm, &res).unwrap().is_none());
        assert_eq!(
            t.output(),
            "__call stream_open 4\n__call stream_read 1\n__call stream_eof 0\n__call stream_stat 0\n"
        );
        // __call makes every method callable
        let supported = with_stream(&mut t.vm, &res, |vm, s| s.truncate_supported(vm));
        assert!(supported.unwrap().unwrap());
    }

    #[test]
    fn exceptions_propagate() {
        let mut t = TestVm::new();
        let methods = [
            (&MINIMAL_OPEN, member_flags::PUBLIC),
            (&THROWING_READ, member_flags::PUBLIC),
            (&MAGIC_CALL, member_flags::PUBLIC),
        ];
        class(&mut t, "Thrower", 0, Some(""), &methods);
        register(&mut t, "abc", "Thrower");
        let res = fopen(&mut t, "abc://", "r").unwrap();
        let r = streams::read(&mut t.vm, &res, 10);
        let Err(php_runtime::Flow::Exception(e)) = r else {
            panic!("expected an exception")
        };
        assert_eq!(e.class().name.as_bytes(), b"Exception");
        assert_eq!(Vm::exception_message(&e), "Message");
        // stream_eof is not asked after a failed read
        assert_eq!(t.output(), "stream_read\n");
    }

    #[test]
    fn private_methods_are_not_callable() {
        let mut t = TestVm::new();
        let methods = [(&OPEN, member_flags::PRIVATE)];
        class(&mut t, "PrivateWrapper", 0, Some(""), &methods);
        register(&mut t, "priv", "PrivateWrapper");
        assert!(fopen(&mut t, "priv://x", "r").is_none());
        assert_eq!(
            t.output(),
            warning(
                "fopen",
                "Failed to open stream: \"PrivateWrapper::stream_open\" is not implemented"
            )
        );
    }

    #[test]
    fn context_property() {
        let mut t = setup();
        register(&mut t, "test", "TestWrapper");
        let ctx = super::super::context::alloc();
        let res = in_fn(&mut t, "fopen", |vm| {
            streams::open(vm, b"test://ctx", b"r", false, Some(&ctx))
        })
        .unwrap()
        .unwrap();
        match object_of(&res).get_prop(b"context") {
            Some(Value::Resource(r)) => assert!(ResRef::ptr_eq(&r, &ctx)),
            other => panic!("context is {other:?}"),
        }
        assert_eq!(t.output(), "open test://ctx r 0\n");

        // no declared property: a deprecated dynamic one
        class(&mut t, "NoContext", 0, None, &public(&[&MINIMAL_OPEN]));
        register(&mut t, "noctx", "NoContext");
        assert!(fopen(&mut t, "noctx://x", "r").is_some());
        assert_eq!(
            t.output(),
            "\nDeprecated: Creation of dynamic property NoContext::$context is deprecated in /t.php on line 3\n"
        );

        // user_streams_context_001.phpt
        class(&mut t, "Typed", 0, Some("Foo"), &public(&[&MINIMAL_OPEN]));
        register(&mut t, "typed", "Typed");
        let r = in_fn(&mut t, "fopen", |vm| {
            streams::open(vm, b"typed://x", b"r", false, Some(&ctx))
        });
        let Err(php_runtime::Flow::Exception(e)) = r else {
            panic!("expected a TypeError")
        };
        assert_eq!(e.class().name.as_bytes(), b"TypeError");
        assert_eq!(
            Vm::exception_message(&e),
            "Cannot assign resource to property Typed::$context of type Foo"
        );
        assert_eq!(t.output(), "");
        // the recursion guard was reset on the way out
        let g = t.vm.ext_data::<UserStreamGlobals>();
        assert!(g.current_filename.is_none());
    }
}
