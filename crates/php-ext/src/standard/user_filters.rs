//! ext/standard/user_filters.c — userspace stream filters (`php_user_filter`,
//! `stream_filter_register`), the bucket API (`StreamBucket`,
//! `stream_bucket_*`) and `stream_get_filters`.
//!
//! A userspace filter is an object of the registered class. Each `filter()`
//! call receives the input and output brigades as `"userfilter.bucket
//! brigade"` resources wrapping a shared [`Brigade`]; the buckets move back
//! into the stream's chain when the call returns and the resources are
//! closed. `StreamBucket` objects carry a `"userfilter.bucket"` resource
//! holding the [`BucketRef`].

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use php_runtime::class::{ClassEntry, Function, PropertyInfo, class_flags, member_flags};
use php_runtime::types::{ClassType, TypeDecl, mask};
use php_runtime::{
    Args, CallArgs, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, PhpStr, Registry,
    ResRef, Value, Vm,
};

use super::streams;
use super::streams::filter::{
    Brigade, BucketRef, FILTER_READ, Filter, FilterCtx, FilterOps, FilterRegistry, FilterStatus,
    PSFS_FLAG_FLUSH_CLOSE, PSFS_FLAG_FLUSH_INC, PSFS_FLAG_NORMAL, Seekable, bucket_make_writeable,
    new_bucket, registry,
};

/// `PHP_STREAM_BRIGADE_RES_NAME`.
pub const BRIGADE_RES_NAME: &str = "userfilter.bucket brigade";
/// `PHP_STREAM_BUCKET_RES_NAME`.
pub const BUCKET_RES_NAME: &str = "userfilter.bucket";

pub const PSFS_ERR_FATAL: i64 = 0;
pub const PSFS_FEED_ME: i64 = 1;
pub const PSFS_PASS_ON: i64 = 2;

/// Payload of a `"userfilter.bucket brigade"` resource.
struct BrigadeRes(Rc<RefCell<Brigade>>);

/// Payload of a `"userfilter.bucket"` resource.
struct BucketRes(BucketRef);

/// The brigades handed to `filter()` calls in progress (innermost last).
/// A bucket lives in at most one of them; attaching it elsewhere unlinks it
/// first, as `bucket->brigade` does in php-src.
#[derive(Default)]
struct ActiveBrigades(Vec<Weak<RefCell<Brigade>>>);

/// Wraps a brigade in a new resource (`zend_register_resource`).
pub fn brigade_resource(brigade: Rc<RefCell<Brigade>>) -> ResRef {
    ResRef::new(BRIGADE_RES_NAME, Box::new(BrigadeRes(brigade)))
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `zend_fetch_resource` failure: "fname(): supplied resource is not a
/// valid %s resource".
fn invalid_resource(vm: &mut Vm, what: &str) -> Flow {
    let f = vm.current_function_display_name();
    vm.throw_type_error(format!(
        "{f}(): supplied resource is not a valid {what} resource"
    ))
}

/// `Z_PARAM_RESOURCE` + `zend_fetch_resource(..., "userfilter.bucket
/// brigade")` for argument `arg_num` (1-based).
fn fetch_brigade(vm: &mut Vm, v: &Value, arg_num: usize) -> PResult<Rc<RefCell<Brigade>>> {
    let res = match v.unref() {
        Value::Resource(r) => r,
        other => {
            return Err(vm.argument_type_error(
                arg_num,
                &format!("must be of type resource, {} given", other.value_name()),
            ));
        }
    };
    let found = if res.is_closed() || res.type_name() != BRIGADE_RES_NAME {
        None
    } else {
        let data = res.resource().data.borrow();
        data.as_ref()
            .and_then(|d| d.downcast_ref::<BrigadeRes>())
            .map(|b| b.0.clone())
    };
    found.ok_or_else(|| invalid_resource(vm, BRIGADE_RES_NAME))
}

/// The bucket behind a `StreamBucket::$bucket` value
/// (`zend_fetch_resource_ex(..., "userfilter.bucket")`).
fn fetch_bucket(vm: &mut Vm, v: &Value) -> PResult<BucketRef> {
    let Value::Resource(res) = v.unref() else {
        let f = vm.current_function_display_name();
        return Err(vm.throw_type_error(format!(
            "{f}(): supplied argument is not a valid {BUCKET_RES_NAME} resource"
        )));
    };
    let found = if res.is_closed() || res.type_name() != BUCKET_RES_NAME {
        None
    } else {
        let data = res.resource().data.borrow();
        data.as_ref()
            .and_then(|d| d.downcast_ref::<BucketRes>())
            .map(|b| b.0.clone())
    };
    found.ok_or_else(|| invalid_resource(vm, BUCKET_RES_NAME))
}

/// A `StreamBucket` for `bucket` (`object_init_ex` + the property updates
/// of `stream_bucket_make_writeable` / `stream_bucket_new`).
fn bucket_object(vm: &mut Vm, bucket: BucketRef) -> PResult<Value> {
    let data = bucket.borrow().buf.clone();
    let len = data.len() as i64;
    let res = ResRef::new(BUCKET_RES_NAME, Box::new(BucketRes(bucket)));
    let ce = vm
        .find_class(b"StreamBucket")
        .expect("StreamBucket is registered");
    let obj = vm.instantiate(&ce)?;
    obj.set_prop(b"bucket", Value::Resource(res));
    obj.set_prop(b"data", Value::String(PhpStr::from_vec(data)));
    obj.set_prop(b"datalen", Value::Int(len));
    obj.set_prop(b"dataLength", Value::Int(len));
    Ok(Value::Object(obj))
}

// ---------------------------------------------------------------------------
// Bucket functions
// ---------------------------------------------------------------------------

/// `stream_bucket_make_writeable`: unlinks the head bucket of the brigade
/// and returns it as a `StreamBucket`, or null when the brigade is empty.
fn stream_bucket_make_writeable(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let brigade = fetch_brigade(vm, args.get(0), 1)?;
    let head = brigade.borrow_mut().pop_front();
    match head {
        Some(b) => bucket_object(vm, bucket_make_writeable(b)),
        None => Ok(Value::Null),
    }
}

/// `php_stream_bucket_attach`: syncs `$bucket->data` into the bucket, then
/// moves it to the brigade's tail (`append`) or head.
fn bucket_attach(vm: &mut Vm, args: &mut Args, append: bool) -> PResult<Value> {
    let brigade = fetch_brigade(vm, args.get(0), 1)?;
    let obj = args.object(1).clone();
    let pzbucket = match obj.get_prop(b"bucket") {
        Some(v) => v,
        None => {
            vm.engine_warning("Undefined property: StreamBucket::$bucket")?;
            Value::Null
        }
    };
    let bucket = fetch_bucket(vm, &pzbucket)?;
    let Some(data) = obj.get_prop(b"data") else {
        return Err(vm.throw_error(
            "Error",
            "Typed property StreamBucket::$data must not be accessed before initialization",
        ));
    };
    let data = vm.to_php_string(&data.unref())?;
    {
        let mut bk = bucket.borrow_mut();
        if bk.buf.as_slice() != data.as_bytes() {
            bk.buf = data.to_vec();
        }
    }
    // If the bucket is already on a brigade it is unlinked first.
    let active: Vec<_> = vm
        .ext_data::<ActiveBrigades>()
        .0
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for b in active {
        b.borrow_mut().unlink(&bucket);
    }
    let mut br = brigade.borrow_mut();
    br.unlink(&bucket);
    if append {
        br.append(bucket);
    } else {
        br.prepend(bucket);
    }
    Ok(Value::Null)
}

fn stream_bucket_prepend(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bucket_attach(vm, args, false)
}

fn stream_bucket_append(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    bucket_attach(vm, args, true)
}

/// `stream_bucket_new`: a new bucket holding a copy of `$buffer`.
fn stream_bucket_new(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let stream = streams::stream_param(vm, args, 0)?;
    let buffer = args.str(1).to_vec();
    let persistent = match streams::handle_of(&stream) {
        Some(h) => match h.try_borrow() {
            Ok(s) => s.st.is_persistent,
            Err(_) => stream.type_name() == "persistent stream",
        },
        None => false,
    };
    let b = new_bucket(buffer);
    b.borrow_mut().is_persistent = persistent;
    bucket_object(vm, b)
}

/// `stream_get_filters`: the registered filter names, global factories
/// first, then the ones added by `stream_filter_register`.
fn stream_get_filters(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut out = PhpArray::new();
    for (name, _) in &registry(vm).factories {
        let _ = out.append(Value::String(name.clone()));
    }
    Ok(Value::from(out))
}

/// `stream_filter_register`.
fn stream_filter_register(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filtername = args.str(0).clone();
    let class = args.str(1).clone();
    // Z_PARAM_CLASS
    let Some(ce) = vm.lookup_class(&class)? else {
        return Err(
            vm.argument_type_error(2, &format!("must be a valid class name, \"{class}\" given"))
        );
    };
    if filtername.is_empty() {
        return Err(vm.argument_value_error(1, "must be a non-empty string"));
    }
    // ZEND_ACC_UNINSTANTIABLE
    if ce.is_interface() || ce.is_trait() || ce.is_abstract() || ce.is_enum() {
        return Err(vm.argument_value_error(2, "must be a concrete class"));
    }
    let reg = registry(vm);
    if !reg.register(filtername.as_bytes(), user_filter_factory_create) {
        return Ok(Value::Bool(false));
    }
    reg.user_filters.push((filtername, ce.name.clone()));
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// The userspace filter
// ---------------------------------------------------------------------------

/// Whether a declared type accepts `v` as is (property type check without
/// coercion).
fn type_accepts(ty: &TypeDecl, v: &Value) -> bool {
    let m = ty.mask;
    match v {
        Value::Undef | Value::Null => m & mask::NULL != 0,
        Value::Bool(true) => m & mask::TRUE != 0,
        Value::Bool(false) => m & mask::FALSE != 0,
        Value::Int(_) => m & (mask::INT | mask::FLOAT) != 0,
        Value::Float(_) => m & mask::FLOAT != 0,
        Value::String(_) => m & mask::STRING != 0,
        Value::Array(_) => m & mask::ARRAY != 0,
        Value::Resource(_) => m & mask::RESOURCE != 0,
        Value::Object(o) => {
            m & mask::OBJECT != 0
                || ty.classes.iter().any(|c| match c {
                    ClassType::Name(n) => o.class().instance_of_name(n),
                    ClassType::Intersection(ns) => ns.iter().all(|n| o.class().instance_of_name(n)),
                })
        }
        Value::Ref(r) => type_accepts(ty, &r.get()),
    }
}

/// `zend_update_property` as far as filter objects need it: visibility
/// (`class_scope` is php-src's `EG(fake_scope)` set to the object's class),
/// the declared type, and dynamic property creation.
fn update_property(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &[u8],
    value: Value,
    class_scope: bool,
) -> PResult<()> {
    let class = obj.class().clone();
    if let Some(info) = class.find_property(name).filter(|p| !p.is_static()) {
        let vis = info.visibility();
        if !class_scope && vis != member_flags::PUBLIC {
            let v = if vis & member_flags::PRIVATE != 0 {
                "private"
            } else {
                "protected"
            };
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot access {v} property {}::${}",
                    class.name,
                    lossy(name)
                ),
            ));
        }
        if let Some(ty) = &info.ty
            && !type_accepts(ty, &value)
        {
            return Err(vm.throw_type_error(format!(
                "Cannot assign {} to property {}::${} of type {}",
                value.value_name(),
                info.declaring_class,
                lossy(name),
                ty.to_type_string()
            )));
        }
        obj.set_prop(name, value);
        return Ok(());
    }
    let exists = obj
        .obj()
        .dyn_props
        .borrow()
        .as_ref()
        .is_some_and(|d| d.get_str(name).is_some());
    if !exists {
        if class.has_flag(class_flags::READONLY) || class.is_enum() {
            return Err(vm.throw_error(
                "Error",
                format!(
                    "Cannot create dynamic property {}::${}",
                    class.name,
                    lossy(name)
                ),
            ));
        }
        if !class.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
            vm.engine_deprecated(format!(
                "Creation of dynamic property {}::${} is deprecated",
                class.name,
                lossy(name)
            ))?;
        }
    }
    obj.set_prop(name, value);
    Ok(())
}

/// `zend_call_method_if_exists` without arguments: `None` when the method
/// is not callable from outside the object.
fn call_method_if_exists(vm: &mut Vm, obj: &ObjRef, name: &[u8]) -> PResult<Option<Value>> {
    let class = obj.class().clone();
    let callable = match class.find_method(name) {
        Some(m) => m.visibility() == member_flags::PUBLIC,
        None => class.magic.call.is_some(),
    };
    if !callable {
        return Ok(None);
    }
    vm.call_method(obj, name, Vec::new()).map(Some)
}

/// `userfilter_ops` (label `user-filter`): the filter object of the
/// registered class.
struct UserFilter {
    obj: ObjRef,
}

/// A `filter()` return value as a filter status. php-src passes any other
/// number through, which the stream layer then treats like `PSFS_FEED_ME`.
fn status_from_long(n: i64) -> FilterStatus {
    match n {
        PSFS_ERR_FATAL => FilterStatus::ErrFatal,
        PSFS_PASS_ON => FilterStatus::PassOn,
        _ => FilterStatus::FeedMe,
    }
}

impl UserFilter {
    /// `userfilter_assign_stream`: gives the object a hook back to the
    /// stream through an existing `$stream` property. `Ok(true)` when the
    /// property was set (and must be reset to null afterwards).
    fn assign_stream(&self, vm: &mut Vm, ctx: &FilterCtx<'_>) -> PResult<bool> {
        if self.obj.get_prop(b"stream").is_none() {
            return Ok(false);
        }
        let stream = match ctx.stream_res {
            Some(r) if !r.is_closed() => Value::Resource(r.clone()),
            _ => Value::Null,
        };
        update_property(vm, &self.obj, b"stream", stream, true)?;
        Ok(true)
    }

    /// `call_user_function(NULL, obj, "filter", ...)`.
    fn call_filter(&self, vm: &mut Vm, args: Vec<Value>) -> PResult<Value> {
        let mut cb = PhpArray::new();
        let _ = cb.append(Value::Object(self.obj.clone()));
        let _ = cb.append("filter");
        let callable = Value::from(cb);
        let host = vm.host.clone();
        if let Err(msg) = host.check_callable(vm, &callable) {
            return Err(vm.throw_error(
                "Error",
                format!("Invalid callback {}::filter, {msg}", self.obj.class().name),
            ));
        }
        vm.call(&callable, args)
    }

    /// `userfilter_seek`; `ctx` is `None` when the stream is not known.
    fn seek_impl(
        &mut self,
        vm: &mut Vm,
        ctx: Option<&FilterCtx<'_>>,
        chain: i64,
        offset: i64,
        whence: i32,
    ) -> PResult<bool> {
        let class = self.obj.class().clone();
        let Some(method) = class.find_method(b"seek").cloned() else {
            // Method doesn't exist - consider this a successful seek for BC
            return Ok(true);
        };
        let stream_set = match ctx {
            Some(ctx) => self.assign_stream(vm, ctx)?,
            None => false,
        };
        let r = vm.call_function(
            &method,
            CallArgs {
                positional: vec![
                    Value::Int(offset),
                    Value::Int(whence as i64),
                    Value::Int(chain),
                ],
                this: Some(self.obj.clone()),
                scope: Some(class),
                ..CallArgs::default()
            },
        );
        if stream_set {
            self.obj.set_prop(b"stream", Value::Null);
        }
        let v = r?;
        if v.is_undef() {
            return Ok(false);
        }
        vm.to_bool(&v.unref())
    }
}

impl FilterOps for UserFilter {
    fn label(&self) -> PhpStr {
        PhpStr::from("user-filter")
    }

    /// `userfilter_filter`: `$this->filter($in, $out, &$consumed, $closing)`.
    fn filter(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        bytes_consumed: Option<&mut usize>,
        flags: i32,
    ) -> PResult<FilterStatus> {
        let stream_set = match self.assign_stream(vm, ctx) {
            Ok(s) => s,
            Err(e) => {
                // If the property update threw, the filter does not run.
                if !inp.is_empty() {
                    vm.warning("Unprocessed filter buckets remaining on input brigade")?;
                }
                return Err(e);
            }
        };

        let in_rc = Rc::new(RefCell::new(std::mem::take(inp)));
        let out_rc = Rc::new(RefCell::new(std::mem::take(out)));
        let in_res = brigade_resource(in_rc.clone());
        let out_res = brigade_resource(out_rc.clone());
        let consumed = PhpRef::new(match &bytes_consumed {
            Some(c) => Value::Int(**c as i64),
            None => Value::Null,
        });
        let args = vec![
            Value::Resource(in_res.clone()),
            Value::Resource(out_res.clone()),
            Value::Ref(consumed.clone()),
            Value::Bool(flags & PSFS_FLAG_FLUSH_CLOSE != 0),
        ];
        {
            let active = &mut vm.ext_data::<ActiveBrigades>().0;
            active.push(Rc::downgrade(&in_rc));
            active.push(Rc::downgrade(&out_rc));
        }
        let call = self.call_filter(vm, args);
        {
            let active = &mut vm.ext_data::<ActiveBrigades>().0;
            active.truncate(active.len().saturating_sub(2));
            active.retain(|w| w.strong_count() > 0);
        }

        let mut first_err: Option<Flow> = None;
        let mut ret = FilterStatus::ErrFatal;
        match call {
            Ok(v) if !v.is_undef() => match vm.to_int(&v.unref()) {
                Ok(n) => ret = status_from_long(n),
                Err(e) => first_err = Some(e),
            },
            Ok(_) => {}
            Err(e) => first_err = Some(e),
        }
        if let Some(c) = bytes_consumed {
            match vm.to_int(&consumed.get()) {
                // size_t in php-src; negative counts are clamped here.
                Ok(n) => *c = n.max(0) as usize,
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        let unprocessed = !in_rc.borrow().is_empty();
        if unprocessed
            && let Err(e) = vm.warning("Unprocessed filter buckets remaining on input brigade")
        {
            first_err.get_or_insert(e);
        }
        // Filter resources are cleaned up by the stream destructor; keeping
        // a reference to the stream here would keep it alive.
        if stream_set {
            self.obj.set_prop(b"stream", Value::Null);
        }
        *inp = std::mem::take(&mut *in_rc.borrow_mut());
        *out = std::mem::take(&mut *out_rc.borrow_mut());
        in_res.close();
        out_res.close();
        match first_err {
            Some(e) => Err(e),
            None => Ok(ret),
        }
    }

    /// Without the chain and stream (see [`FilterOps::seek_in_chain`]), the
    /// read chain is assumed and `$this->stream` is left alone.
    fn seek(&mut self, vm: &mut Vm, offset: i64, whence: i32) -> PResult<bool> {
        self.seek_impl(vm, None, FILTER_READ, offset, whence)
    }

    fn seek_in_chain(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        chain: i64,
        offset: i64,
        whence: i32,
    ) -> PResult<bool> {
        self.seek_impl(vm, Some(ctx), chain, offset, whence)
    }

    /// `userfilter_dtor`: `$this->onClose()`.
    fn dtor(&mut self, vm: &mut Vm) -> PResult<()> {
        call_method_if_exists(vm, &self.obj, b"onclose")?;
        Ok(())
    }

    fn needs_vm(&self) -> bool {
        true
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The class registered for `name`: exact match, then `a.b.*`, `a.*`
/// (the first wildcard found wins, as in php-src).
fn find_user_filter_class(reg: &FilterRegistry, name: &[u8]) -> Option<PhpStr> {
    let find = |n: &[u8]| {
        reg.user_filters
            .iter()
            .find(|(f, _)| f.as_bytes() == n)
            .map(|(_, c)| c.clone())
    };
    if let Some(c) = find(name) {
        return Some(c);
    }
    let mut wild = name.to_vec();
    while let Some(period) = wild.iter().rposition(|&c| c == b'.') {
        wild.truncate(period);
        let mut pat = wild.clone();
        pat.extend_from_slice(b".*");
        if let Some(c) = find(&pat) {
            return Some(c);
        }
    }
    None
}

/// `user_filter_factory_create`.
fn user_filter_factory_create(
    vm: &mut Vm,
    name: &[u8],
    params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    if persistent {
        vm.warning("Cannot use a user-space filter with a persistent stream")?;
        return Ok(None);
    }
    let Some(class_name) = find_user_filter_class(registry(vm), name) else {
        return Ok(None);
    };
    let Some(ce) = vm.lookup_class(&class_name)? else {
        return Ok(None);
    };
    let obj = match vm.instantiate(&ce) {
        Ok(o) => o,
        Err(e) => {
            // php_stream_filter_create reports the failed creation before
            // the exception propagates.
            vm.warning(format!(
                "Unable to create or locate filter \"{}\"",
                lossy(name)
            ))?;
            return Err(e);
        }
    };

    // filtername, and the parameters if any; like php-src, a failed update
    // does not stop the next one.
    let params = match params.unref() {
        Value::Undef => Value::Null,
        v => v,
    };
    let r1 = update_property(
        vm,
        &obj,
        b"filtername",
        Value::String(PhpStr::from_bytes(name)),
        false,
    );
    let r2 = update_property(vm, &obj, b"params", params, false);
    r1?;
    r2?;

    // invoke the constructor
    if let Some(Value::Bool(false)) = call_method_if_exists(vm, &obj, b"oncreate")? {
        // The filter is freed without calling onClose().
        return Ok(None);
    }
    Ok(Some(Filter::new(
        Box::new(UserFilter { obj }),
        Seekable::Check,
        Seekable::Check,
    )))
}

// ---------------------------------------------------------------------------
// php_user_filter methods
// ---------------------------------------------------------------------------

fn require_resource(vm: &mut Vm, args: &Args, i: usize) -> PResult<()> {
    match args.get(i).unref() {
        Value::Resource(_) => Ok(()),
        other => Err(vm.argument_type_error(
            i + 1,
            &format!("must be of type resource, {} given", other.value_name()),
        )),
    }
}

/// `php_user_filter::filter`: the base class fails every call.
fn user_filter_filter(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    require_resource(vm, args, 0)?;
    require_resource(vm, args, 1)?;
    Ok(Value::Int(PSFS_ERR_FATAL))
}

fn user_filter_seek(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(true))
}

fn user_filter_on_create(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(true))
}

fn user_filter_on_close(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

static FILTER: NativeFunction = NativeFunction {
    sig: "public function filter($in, $out, &$consumed, bool $closing): int {}",
    handler: user_filter_filter,
};
static SEEK: NativeFunction = NativeFunction {
    sig: "public function seek(int $offset, int $whence, int $chain): bool {}",
    handler: user_filter_seek,
};
static ON_CREATE: NativeFunction = NativeFunction {
    sig: "public function onCreate(): bool {}",
    handler: user_filter_on_create,
};
static ON_CLOSE: NativeFunction = NativeFunction {
    sig: "public function onClose(): void {}",
    handler: user_filter_on_close,
};

static STREAM_BUCKET_MAKE_WRITEABLE: NativeFunction = NativeFunction {
    sig: "function stream_bucket_make_writeable($brigade): ?StreamBucket {}",
    handler: stream_bucket_make_writeable,
};
static STREAM_BUCKET_PREPEND: NativeFunction = NativeFunction {
    sig: "function stream_bucket_prepend($brigade, StreamBucket $bucket): void {}",
    handler: stream_bucket_prepend,
};
static STREAM_BUCKET_APPEND: NativeFunction = NativeFunction {
    sig: "function stream_bucket_append($brigade, StreamBucket $bucket): void {}",
    handler: stream_bucket_append,
};
static STREAM_BUCKET_NEW: NativeFunction = NativeFunction {
    sig: "function stream_bucket_new($stream, string $buffer): StreamBucket {}",
    handler: stream_bucket_new,
};
static STREAM_GET_FILTERS: NativeFunction = NativeFunction {
    sig: "function stream_get_filters(): array {}",
    handler: stream_get_filters,
};
static STREAM_FILTER_REGISTER: NativeFunction = NativeFunction {
    sig: "function stream_filter_register(string $filter_name, string $class): bool {}",
    handler: stream_filter_register,
};

fn declare(ce: &mut ClassEntry, class: &str, name: &str, ty: Option<&str>, default: Value) {
    let mut p = PropertyInfo::new(name, class, member_flags::PUBLIC);
    p.ty = ty.map(|t| TypeDecl::parse(t).expect("valid type"));
    p.default = default;
    ce.declare_property(p);
}

/// `PHP_MINIT(user_filters)` plus the user_filters.c functions of
/// basic_functions.stub.php.
pub fn register(r: &mut Registry) {
    for (name, v) in [
        ("PSFS_PASS_ON", PSFS_PASS_ON),
        ("PSFS_FEED_ME", PSFS_FEED_ME),
        ("PSFS_ERR_FATAL", PSFS_ERR_FATAL),
        ("PSFS_FLAG_NORMAL", PSFS_FLAG_NORMAL as i64),
        ("PSFS_FLAG_FLUSH_INC", PSFS_FLAG_FLUSH_INC as i64),
        ("PSFS_FLAG_FLUSH_CLOSE", PSFS_FLAG_FLUSH_CLOSE as i64),
    ] {
        r.constant(name, v);
    }

    let mut ce = ClassEntry::new("php_user_filter", 0);
    declare(
        &mut ce,
        "php_user_filter",
        "filtername",
        Some("string"),
        Value::empty_string(),
    );
    declare(
        &mut ce,
        "php_user_filter",
        "params",
        Some("mixed"),
        Value::empty_string(),
    );
    declare(&mut ce, "php_user_filter", "stream", None, Value::Null);
    for nf in [&FILTER, &SEEK, &ON_CREATE, &ON_CLOSE] {
        ce.add_method(Function::native_method(
            nf,
            "php_user_filter",
            member_flags::PUBLIC,
        ));
    }
    r.class(ce);

    let mut ce = ClassEntry::new("StreamBucket", class_flags::FINAL);
    declare(&mut ce, "StreamBucket", "bucket", None, Value::Null);
    declare(
        &mut ce,
        "StreamBucket",
        "data",
        Some("string"),
        Value::Undef,
    );
    declare(
        &mut ce,
        "StreamBucket",
        "datalen",
        Some("int"),
        Value::Undef,
    );
    declare(
        &mut ce,
        "StreamBucket",
        "dataLength",
        Some("int"),
        Value::Undef,
    );
    r.class(ce);

    r.functions(&[
        &STREAM_BUCKET_MAKE_WRITEABLE,
        &STREAM_BUCKET_PREPEND,
        &STREAM_BUCKET_APPEND,
        &STREAM_BUCKET_NEW,
        &STREAM_GET_FILTERS,
        &STREAM_FILTER_REGISTER,
    ]);
}

#[cfg(test)]
mod tests {
    //! Expectations from ext/standard/tests/filters/*.phpt. Without an
    //! interpreter, the userland filter classes of those tests are stood in
    //! for by classes whose methods are native functions.

    use super::*;
    use crate::standard::streams::{self, filter, with_stream};
    use crate::testing::{TestVm, thrown};
    use php_runtime::ClassRef;

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static STASH: RefCell<Option<Value>> = const { RefCell::new(None) };
    }

    fn log(s: impl Into<String>) {
        LOG.with(|l| l.borrow_mut().push(s.into()));
    }

    fn take_log() -> Vec<String> {
        LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    type Prop<'a> = (&'a str, u32, Option<&'a str>, Value);

    /// `class $name [extends php_user_filter]` with public native methods.
    fn define(
        vm: &mut Vm,
        name: &str,
        extends: bool,
        methods: &[&'static NativeFunction],
        props: &[Prop<'_>],
    ) -> ClassRef {
        let mut ce = if extends {
            let parent = vm.find_class(b"php_user_filter").unwrap();
            ClassEntry::new_subclass(name, 0, &parent)
        } else {
            ClassEntry::new(name, 0)
        };
        for (p, flags, ty, default) in props {
            let mut info = PropertyInfo::new(*p, name, *flags);
            info.ty = ty.map(|t| TypeDecl::parse(t).unwrap());
            info.default = default.clone();
            ce.declare_property(info);
        }
        for nf in methods {
            ce.add_method(Function::native_method(nf, name, member_flags::PUBLIC));
        }
        ce.link_magic_methods();
        let rc = Rc::new(ce);
        assert!(vm.register_class(rc.clone()));
        rc
    }

    const PUBLIC: u32 = member_flags::PUBLIC;

    fn mixed_props() -> Vec<Prop<'static>> {
        vec![
            ("filtername", PUBLIC, None, Value::Null),
            ("params", PUBLIC, None, Value::Null),
        ]
    }

    fn register(t: &mut TestVm, name: &str, class: &str) -> Value {
        t.call("stream_filter_register", vec![name.into(), class.into()])
            .unwrap()
    }

    fn create(t: &mut TestVm, name: &str, params: &Value) -> PResult<Option<Filter>> {
        filter::create_filter(&mut t.vm, name.as_bytes(), params, false)
    }

    fn filter_obj(f: &mut Filter) -> ObjRef {
        f.ops
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<UserFilter>())
            .expect("a user filter")
            .obj
            .clone()
    }

    fn attach(t: &mut TestVm, res: &ResRef, f: Filter, read: bool) -> u64 {
        let id = f.id;
        let ok = with_stream(&mut t.vm, res, |vm, s| s.attach_filter(vm, f, read, true))
            .unwrap()
            .unwrap();
        assert!(ok);
        id
    }

    /// Removes a filter without flushing it. Streams whose filters need the
    /// VM must not be dropped with the filter attached: the stream core then
    /// defers the close to a thread-local that is gone at thread exit.
    fn detach(t: &mut TestVm, res: &ResRef, id: u64) {
        with_stream(&mut t.vm, res, |vm, s| s.remove_filter(vm, id, false)).unwrap();
    }

    fn str_of(v: &Value) -> String {
        match v.unref() {
            Value::String(s) => s.to_string(),
            other => panic!("string expected, got {other:?}"),
        }
    }

    fn strings(v: &Value) -> Vec<String> {
        let Value::Array(a) = v else {
            panic!("array expected, got {v:?}")
        };
        a.values().map(str_of).collect()
    }

    fn data_of(v: &Value) -> Vec<u8> {
        let Value::Object(o) = v else {
            panic!("StreamBucket expected, got {v:?}")
        };
        match o.get_prop(b"data") {
            Some(Value::String(s)) => s.to_vec(),
            other => panic!("data: {other:?}"),
        }
    }

    fn brigade_with(chunks: &[&[u8]]) -> (Rc<RefCell<Brigade>>, Value) {
        let br = Rc::new(RefCell::new(Brigade::new()));
        for c in chunks {
            br.borrow_mut().append(new_bucket(c.to_vec()));
        }
        let res = brigade_resource(br.clone());
        (br, Value::Resource(res))
    }

    fn contents(br: &Rc<RefCell<Brigade>>) -> Vec<Vec<u8>> {
        br.borrow()
            .buckets
            .iter()
            .map(|b| b.borrow().buf.clone())
            .collect()
    }

    // --- native stand-ins for userland methods ---------------------------

    /// `strtoupper_filter::filter` of read.phpt: uppercases every bucket and
    /// appends a marker bucket on the closing call.
    fn upper_filter(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let (inp, out, consumed) = (args.get(0).clone(), args.get(1).clone(), args.by_ref(2));
        let closing = args.bool(3);
        let this = args.this().unwrap().clone();
        let stream = this.get_prop(b"stream").unwrap_or(Value::Null);
        log(format!(
            "filter closing={closing} stream={}",
            stream.type_name()
        ));
        while let Value::Object(b) =
            vm.call_native("stream_bucket_make_writeable", vec![inp.clone()])?
        {
            let mut data = data_of(&Value::Object(b.clone()));
            data.make_ascii_uppercase();
            b.set_prop(b"data", Value::from(data));
            let n = vm.to_int(&consumed.get())? + vm.to_int(&b.get_prop(b"datalen").unwrap())?;
            consumed.set(Value::Int(n));
            vm.call_native("stream_bucket_append", vec![out.clone(), Value::Object(b)])?;
        }
        if closing {
            let b = vm.call_native(
                "stream_bucket_new",
                vec![stream, Value::from("\n===close===\n")],
            )?;
            vm.call_native("stream_bucket_append", vec![out, b])?;
        }
        Ok(Value::Int(PSFS_PASS_ON))
    }

    /// Moves every bucket to the output unchanged.
    fn pass_filter(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let (inp, out, consumed) = (args.get(0).clone(), args.get(1).clone(), args.by_ref(2));
        STASH.with(|s| *s.borrow_mut() = Some(inp.clone()));
        if let Value::Resource(r) = &inp {
            log(format!("in={}", r.type_name()));
        }
        while let Value::Object(b) =
            vm.call_native("stream_bucket_make_writeable", vec![inp.clone()])?
        {
            let n = vm.to_int(&consumed.get())? + vm.to_int(&b.get_prop(b"datalen").unwrap())?;
            consumed.set(Value::Int(n));
            vm.call_native("stream_bucket_append", vec![out.clone(), Value::Object(b)])?;
        }
        Ok(Value::Int(PSFS_PASS_ON))
    }

    fn feed_me_filter(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Ok(Value::Int(PSFS_FEED_ME))
    }

    fn throwing_filter(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Err(vm.throw_error("Exception", "No"))
    }

    fn log_on_create(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let this = args.this().unwrap();
        let name = str_of(&this.get_prop(b"filtername").unwrap());
        let params = this.get_prop(b"params").unwrap();
        log(format!("onCreate {name} {}", params.type_name()));
        Ok(Value::Bool(true))
    }

    fn false_on_create(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        log("onCreate");
        Ok(Value::Bool(false))
    }

    fn log_on_close(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        log("onClose");
        Ok(Value::Null)
    }

    fn throwing_on_close(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
        Err(vm.throw_error("Exception", "No"))
    }

    /// `seek()` of user_filter_seek_02/03: logs the call, only seeks to 0.
    fn log_seek(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        let (offset, whence, chain) = (args.int(0), args.int(1), args.int(2));
        log(format!("seek {chain}:{offset}:{whence}"));
        Ok(Value::Bool(offset == 0))
    }

    static UPPER_FILTER: NativeFunction = NativeFunction {
        sig: "public function filter($in, $out, &$consumed, bool $closing): int {}",
        handler: upper_filter,
    };
    static PASS_FILTER: NativeFunction = NativeFunction {
        sig: "public function filter($in, $out, &$consumed, bool $closing): int {}",
        handler: pass_filter,
    };
    static FEED_ME_FILTER: NativeFunction = NativeFunction {
        sig: "public function filter($in, $out, &$consumed, bool $closing): int {}",
        handler: feed_me_filter,
    };
    static THROWING_FILTER: NativeFunction = NativeFunction {
        sig: "public function filter($in, $out, &$consumed, bool $closing): int {}",
        handler: throwing_filter,
    };
    static LOG_ON_CREATE: NativeFunction = NativeFunction {
        sig: "public function onCreate(): bool {}",
        handler: log_on_create,
    };
    static FALSE_ON_CREATE: NativeFunction = NativeFunction {
        sig: "public function onCreate(): bool {}",
        handler: false_on_create,
    };
    static LOG_ON_CLOSE: NativeFunction = NativeFunction {
        sig: "public function onClose(): void {}",
        handler: log_on_close,
    };
    static THROWING_ON_CLOSE: NativeFunction = NativeFunction {
        sig: "public function onclose(): void {}",
        handler: throwing_on_close,
    };
    static LOG_SEEK: NativeFunction = NativeFunction {
        sig: "public function seek(int $offset, int $whence, int $chain): bool {}",
        handler: log_seek,
    };

    // --- tests --------------------------------------------------------------

    #[test]
    fn constants_and_classes() {
        let mut t = TestVm::new();
        for (name, v) in [
            ("PSFS_PASS_ON", 2),
            ("PSFS_FEED_ME", 1),
            ("PSFS_ERR_FATAL", 0),
            ("PSFS_FLAG_NORMAL", 0),
            ("PSFS_FLAG_FLUSH_INC", 1),
            ("PSFS_FLAG_FLUSH_CLOSE", 2),
        ] {
            assert!(
                matches!(t.vm.constant(name), Some(Value::Int(n)) if n == v),
                "{name}"
            );
        }
        let ce = t.vm.find_class(b"php_user_filter").unwrap();
        let obj = t.vm.instantiate(&ce).unwrap();
        assert!(matches!(obj.get_prop(b"filtername"), Some(Value::String(s)) if s.is_empty()));
        assert!(matches!(obj.get_prop(b"params"), Some(Value::String(s)) if s.is_empty()));
        assert!(matches!(obj.get_prop(b"stream"), Some(Value::Null)));
        for m in ["filter", "seek", "oncreate", "onclose"] {
            assert!(ce.find_method(m.as_bytes()).is_some(), "{m}");
        }
        let sb = t.vm.find_class(b"StreamBucket").unwrap();
        assert!(sb.has_flag(class_flags::FINAL));
        let b = t.vm.instantiate(&sb).unwrap();
        assert!(matches!(b.get_prop(b"bucket"), Some(Value::Null)));
        // Typed properties without default start uninitialised.
        assert!(b.get_prop(b"data").is_none());
        assert!(b.get_prop(b"datalen").is_none());
        assert!(b.get_prop(b"dataLength").is_none());
    }

    #[test]
    fn base_class_methods() {
        let mut t = TestVm::new();
        let ce = t.vm.find_class(b"php_user_filter").unwrap();
        let obj = t.vm.instantiate(&ce).unwrap();
        let (_, inp) = brigade_with(&[]);
        let (_, out) = brigade_with(&[]);
        let consumed = Value::Ref(PhpRef::new(Value::Null));
        let r = t.vm.call_method(
            &obj,
            b"filter",
            vec![inp.clone(), out, consumed.clone(), Value::Bool(false)],
        );
        assert!(matches!(r, Ok(Value::Int(PSFS_ERR_FATAL))));
        let r = t.vm.call_method(
            &obj,
            b"filter",
            vec!["x".into(), inp, consumed, Value::Bool(false)],
        );
        assert_eq!(
            thrown(r),
            (
                "TypeError".into(),
                "php_user_filter::filter(): Argument #1 ($in) must be of type resource, string given"
                    .into()
            )
        );
        let r = t.vm.call_method(&obj, b"onCreate", vec![]);
        assert!(matches!(r, Ok(Value::Bool(true))));
        let r = t.vm.call_method(&obj, b"onClose", vec![]);
        assert!(matches!(r, Ok(Value::Null)));
        let r =
            t.vm.call_method(&obj, b"seek", vec![0.into(), 0.into(), 1.into()]);
        assert!(matches!(r, Ok(Value::Bool(true))));
    }

    #[test]
    fn register_and_list_filters() {
        // stream_filter_register.phpt, gh17037.phpt
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", true, &[&PASS_FILTER], &[]);
        let std = [
            "string.rot13",
            "string.toupper",
            "string.tolower",
            "convert.*",
            "consumed",
            "dechunk",
        ];
        assert_eq!(strings(&t.call("stream_get_filters", vec![]).unwrap()), std);
        assert!(matches!(
            register(&mut t, "myfilter", "foo"),
            Value::Bool(true)
        ));
        assert!(matches!(
            register(&mut t, "myfilter", "foo"),
            Value::Bool(false)
        ));
        assert!(matches!(register(&mut t, "foo", "FOO"), Value::Bool(true)));
        assert!(matches!(
            register(&mut t, "string.toupper", "foo"),
            Value::Bool(false)
        ));
        let mut all: Vec<&str> = std.to_vec();
        all.extend(["myfilter", "foo"]);
        assert_eq!(strings(&t.call("stream_get_filters", vec![]).unwrap()), all);
        // The class name is stored as declared.
        let reg = registry(&mut t.vm);
        assert_eq!(reg.user_filters[1].1.to_string(), "foo");
        assert_eq!(t.output(), "");
    }

    #[test]
    fn register_errors() {
        // 001.phpt, stream_filter_register_non_existing_class.phpt
        let mut t = TestVm::new();
        assert_eq!(
            thrown(t.call("stream_filter_register", vec!["".into(), "stdClass".into()])),
            (
                "ValueError".into(),
                "stream_filter_register(): Argument #1 ($filter_name) must be a non-empty string"
                    .into()
            )
        );
        assert_eq!(
            thrown(t.call(
                "stream_filter_register",
                vec!["test".into(), "Throwable".into()]
            )),
            (
                "ValueError".into(),
                "stream_filter_register(): Argument #2 ($class) must be a concrete class".into()
            )
        );
        assert_eq!(
            thrown(t.call(
                "stream_filter_register",
                vec!["not_existing_filter".into(), "not_existing".into()]
            )),
            (
                "TypeError".into(),
                "stream_filter_register(): Argument #2 ($class) must be a valid class name, \"not_existing\" given"
                    .into()
            )
        );
        // The class is checked before the name.
        assert_eq!(
            thrown(t.call("stream_filter_register", vec!["".into(), "nope".into()])).0,
            "TypeError"
        );
        assert!(
            create(&mut t, "not_existing_filter", &Value::Null)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unable to locate filter \"not_existing_filter\" in /t.php on line 3\n"
        );
    }

    #[test]
    fn read_chain_filter() {
        // read.phpt, userfilters.phpt, bug73586.phpt
        let mut t = TestVm::new();
        define(
            &mut t.vm,
            "strtoupper_filter",
            true,
            &[&UPPER_FILTER, &LOG_ON_CREATE, &LOG_ON_CLOSE],
            &[],
        );
        register(&mut t, "strtoupper", "strtoupper_filter");
        let res = streams::open_memory(&mut t.vm, b"Hello There!");
        let mut f = create(&mut t, "strtoupper", &Value::from("testuserfilter"))
            .unwrap()
            .unwrap();
        assert_eq!(f.label().to_string(), "user-filter");
        assert_eq!(
            (f.read_seekable, f.write_seekable),
            (Seekable::Check, Seekable::Check)
        );
        let obj = filter_obj(&mut f);
        assert_eq!(take_log(), ["onCreate strtoupper string"]);
        assert!(matches!(obj.get_prop(b"params"), Some(Value::String(s)) if s == "testuserfilter"));
        attach(&mut t, &res, f, true);
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"HELLO THERE!\n===close===\n"
        );
        let log = take_log();
        assert_eq!(log.first().unwrap(), "filter closing=false stream=resource");
        assert_eq!(log.last().unwrap(), "filter closing=true stream=resource");
        // The hook back to the stream only exists during the call.
        assert!(matches!(obj.get_prop(b"stream"), Some(Value::Null)));
        assert!(streams::close(&mut t.vm, &res).unwrap());
        assert_eq!(take_log(), ["onClose"]);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn write_chain_filters() {
        // bug21478.phpt: three user filters on the write chain
        let mut t = TestVm::new();
        define(&mut t.vm, "debugfilter", true, &[&UPPER_FILTER], &[]);
        register(&mut t, "myfilter", "debugfilter");
        let res = streams::open_memory(&mut t.vm, b"");
        for _ in 0..3 {
            let f = create(&mut t, "myfilter", &Value::Null).unwrap().unwrap();
            attach(&mut t, &res, f, false);
        }
        assert_eq!(
            streams::write(&mut t.vm, &res, b"This is a test.\n").unwrap(),
            Some(16)
        );
        take_log();
        with_stream(&mut t.vm, &res, |vm, s| s.flush(vm)).unwrap();
        streams::seek(&mut t.vm, &res, 0, 0).unwrap();
        let data = streams::read_to_end(&mut t.vm, &res).unwrap().unwrap();
        assert!(data.starts_with(b"THIS IS A TEST.\n"), "{data:?}");
        assert!(streams::close(&mut t.vm, &res).unwrap());
        assert_eq!(t.output(), "");
    }

    #[test]
    fn on_create_false() {
        // php_user_filter_onCreate_failure.phpt
        let mut t = TestVm::new();
        define(
            &mut t.vm,
            "my_filter",
            true,
            &[&FALSE_ON_CREATE, &LOG_ON_CLOSE],
            &[],
        );
        register(&mut t, "my_filter", "my_filter");
        assert!(create(&mut t, "my_filter", &Value::Null).unwrap().is_none());
        assert_eq!(take_log(), ["onCreate"]);
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unable to create or locate filter \"my_filter\" in /t.php on line 3\n"
        );
    }

    #[test]
    fn persistent_streams_are_refused() {
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", true, &[], &[]);
        register(&mut t, "foo", "foo");
        let f = filter::create_filter(&mut t.vm, b"foo", &Value::Null, true).unwrap();
        assert!(f.is_none());
        assert_eq!(
            t.output(),
            "\nWarning: main(): Cannot use a user-space filter with a persistent stream in /t.php on line 3\n\
             \nWarning: main(): Unable to create or locate filter \"foo\" in /t.php on line 3\n"
        );
    }

    #[test]
    fn wildcard_registration() {
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", true, &[&LOG_ON_CREATE], &[]);
        register(&mut t, "test.*", "foo");
        let f = create(&mut t, "test.foo.bar", &opts_array()).unwrap();
        assert!(f.is_some());
        assert_eq!(take_log(), ["onCreate test.foo.bar array"]);
        assert!(create(&mut t, "other.foo", &Value::Null).unwrap().is_none());
        assert_eq!(take_log(), Vec::<String>::new());
    }

    fn opts_array() -> Value {
        let mut a = PhpArray::new();
        a.insert_str("rotation", 13);
        Value::from(a)
    }

    #[test]
    fn unprocessed_buckets() {
        // stream_filter_register_filter_always_feed.phpt
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", true, &[&FEED_ME_FILTER], &[]);
        register(&mut t, "invalid_filter", "foo");
        let res = streams::open_memory(&mut t.vm, b"");
        let f = create(&mut t, "invalid_filter", &Value::Null)
            .unwrap()
            .unwrap();
        let id = attach(&mut t, &res, f, false);
        assert_eq!(
            streams::write(&mut t.vm, &res, b"Hello\n").unwrap(),
            Some(0)
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unprocessed filter buckets remaining on input brigade in /t.php on line 3\n"
        );
        detach(&mut t, &res, id);
    }

    #[test]
    fn base_filter_fails_and_throwing_on_close() {
        // stream_filter_register_class_throwing_onclose.phpt
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", true, &[&THROWING_ON_CLOSE], &[]);
        register(&mut t, "invalid_filter", "foo");
        let res = streams::open_memory(&mut t.vm, b"");
        let f = create(&mut t, "invalid_filter", &Value::Null)
            .unwrap()
            .unwrap();
        attach(&mut t, &res, f, false);
        assert_eq!(streams::write(&mut t.vm, &res, b"Hello\n").unwrap(), None);
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unprocessed filter buckets remaining on input brigade in /t.php on line 3\n"
        );
        let r = streams::close(&mut t.vm, &res).map(Value::Bool);
        assert_eq!(thrown(r), ("Exception".into(), "No".into()));
    }

    #[test]
    fn class_without_filter_method() {
        // stream_filter_register_class_completely_invalid.phpt
        let mut t = TestVm::new();
        define(&mut t.vm, "foo", false, &[], &[]);
        assert!(matches!(
            register(&mut t, "invalid_filter", "foo"),
            Value::Bool(true)
        ));
        let res = streams::open_memory(&mut t.vm, b"");
        let f = create(&mut t, "invalid_filter", &Value::Null)
            .unwrap()
            .unwrap();
        assert_eq!(
            t.output(),
            "\nDeprecated: Creation of dynamic property foo::$filtername is deprecated in /t.php on line 3\n\
             \nDeprecated: Creation of dynamic property foo::$params is deprecated in /t.php on line 3\n"
        );
        let id = attach(&mut t, &res, f, false);
        let r = streams::write(&mut t.vm, &res, b"Hello\n").map(|_| Value::Null);
        assert_eq!(
            thrown(r),
            (
                "Error".into(),
                "Invalid callback foo::filter, class foo does not have a method \"filter\"".into()
            )
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unprocessed filter buckets remaining on input brigade in /t.php on line 3\n"
        );
        detach(&mut t, &res, id);
    }

    #[test]
    fn property_errors_on_create() {
        // stream_filter_register_class_private_filtername_prop.phpt
        let mut t = TestVm::new();
        define(
            &mut t.vm,
            "foo",
            false,
            &[&PASS_FILTER],
            &[("filtername", member_flags::PRIVATE, None, Value::Null)],
        );
        register(&mut t, "invalid_filter", "foo");
        let r = create(&mut t, "invalid_filter", &Value::Null).map(|_| Value::Null);
        assert_eq!(
            thrown(r),
            (
                "Error".into(),
                "Cannot access private property foo::$filtername".into()
            )
        );
        assert_eq!(
            t.output(),
            "\nDeprecated: Creation of dynamic property foo::$params is deprecated in /t.php on line 3\n"
        );

        // stream_filter_register_class_completely_invalid_filtername_prop_type.phpt
        define(
            &mut t.vm,
            "bar",
            false,
            &[],
            &[("filtername", PUBLIC, Some("array"), Value::Undef)],
        );
        register(&mut t, "bar_filter", "bar");
        let r = create(&mut t, "bar_filter", &Value::Null).map(|_| Value::Null);
        assert_eq!(
            thrown(r),
            (
                "TypeError".into(),
                "Cannot assign string to property bar::$filtername of type array".into()
            )
        );
        assert!(
            t.output()
                .contains("Creation of dynamic property bar::$params")
        );
    }

    fn failing_create_object(vm: &mut Vm, _ce: &ClassRef) -> PResult<ObjRef> {
        Err(vm.throw_error("Error", "Undefined constant \"FOO\""))
    }

    #[test]
    fn object_creation_failure() {
        // object_init_failure.phpt
        let mut t = TestVm::new();
        let parent = t.vm.find_class(b"php_user_filter").unwrap();
        let mut ce = ClassEntry::new_subclass("SampleFilter", 0, &parent);
        ce.handlers.create_object = Some(failing_create_object);
        t.vm.register_class(Rc::new(ce));
        register(&mut t, "sample.filter", "SampleFilter");
        let r = create(&mut t, "sample.filter", &Value::Null).map(|_| Value::Null);
        assert_eq!(
            thrown(r),
            ("Error".into(), "Undefined constant \"FOO\"".into())
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unable to create or locate filter \"sample.filter\" in /t.php on line 3\n"
        );
    }

    #[test]
    fn typed_stream_property() {
        // gh20370.phpt
        let mut t = TestVm::new();
        let mut props = mixed_props();
        props.push(("stream", PUBLIC, Some("int"), Value::Int(1)));
        define(&mut t.vm, "pass_filter", false, &[&PASS_FILTER], &props);
        register(&mut t, "pass", "pass_filter");
        let res = streams::open_memory(&mut t.vm, b"");
        let f = create(&mut t, "pass", &Value::Null).unwrap().unwrap();
        let id = attach(&mut t, &res, f, false);
        let r = streams::write(&mut t.vm, &res, b"data").map(|_| Value::Null);
        assert_eq!(
            thrown(r),
            (
                "TypeError".into(),
                "Cannot assign resource to property pass_filter::$stream of type int".into()
            )
        );
        assert_eq!(
            t.output(),
            "\nWarning: main(): Unprocessed filter buckets remaining on input brigade in /t.php on line 3\n"
        );
        assert_eq!(take_log(), Vec::<String>::new());
        detach(&mut t, &res, id);
    }

    #[test]
    fn stream_property_only_when_declared() {
        // gh20370_no_stream_property.phpt, gh20370_private_stream_property.phpt
        let mut t = TestVm::new();
        define(
            &mut t.vm,
            "nostream",
            false,
            &[&PASS_FILTER],
            &mixed_props(),
        );
        let mut props = mixed_props();
        props.push(("stream", member_flags::PRIVATE, None, Value::Null));
        define(&mut t.vm, "privstream", false, &[&UPPER_FILTER], &props);
        register(&mut t, "nostream", "nostream");
        register(&mut t, "privstream", "privstream");

        let res = streams::open_memory(&mut t.vm, b"");
        let mut f = create(&mut t, "nostream", &Value::Null).unwrap().unwrap();
        let obj = filter_obj(&mut f);
        attach(&mut t, &res, f, false);
        assert_eq!(streams::write(&mut t.vm, &res, b"data").unwrap(), Some(4));
        assert!(obj.get_prop(b"stream").is_none());
        assert!(streams::close(&mut t.vm, &res).unwrap());

        let res = streams::open_memory(&mut t.vm, b"");
        let mut f = create(&mut t, "privstream", &Value::Null).unwrap().unwrap();
        let obj = filter_obj(&mut f);
        attach(&mut t, &res, f, false);
        take_log();
        assert_eq!(streams::write(&mut t.vm, &res, b"data").unwrap(), Some(4));
        assert_eq!(take_log(), ["filter closing=false stream=resource"]);
        assert!(matches!(obj.get_prop(b"stream"), Some(Value::Null)));
        assert!(streams::close(&mut t.vm, &res).unwrap());
        assert_eq!(t.output(), "");
    }

    #[test]
    fn brigade_resources_live_for_one_call() {
        let mut t = TestVm::new();
        define(&mut t.vm, "pass", true, &[&PASS_FILTER], &[]);
        register(&mut t, "pass", "pass");
        let res = streams::open_memory(&mut t.vm, b"abc");
        let f = create(&mut t, "pass", &Value::Null).unwrap().unwrap();
        let id = attach(&mut t, &res, f, true);
        assert_eq!(
            streams::read_to_end(&mut t.vm, &res).unwrap().unwrap(),
            b"abc"
        );
        assert!(
            take_log()
                .iter()
                .all(|l| l == "in=userfilter.bucket brigade")
        );
        let stashed = STASH.with(|s| s.borrow_mut().take()).unwrap();
        assert_eq!(
            thrown(t.call("stream_bucket_make_writeable", vec![stashed])),
            (
                "TypeError".into(),
                "stream_bucket_make_writeable(): supplied resource is not a valid userfilter.bucket brigade resource"
                    .into()
            )
        );
        detach(&mut t, &res, id);
    }

    #[test]
    fn exceptions_from_filter_propagate() {
        let mut t = TestVm::new();
        define(&mut t.vm, "boom", true, &[&THROWING_FILTER], &[]);
        register(&mut t, "boom", "boom");
        let res = streams::open_memory(&mut t.vm, b"abc");
        let f = create(&mut t, "boom", &Value::Null).unwrap().unwrap();
        let id = attach(&mut t, &res, f, true);
        let r = streams::read_to_end(&mut t.vm, &res).map(|_| Value::Null);
        assert_eq!(thrown(r), ("Exception".into(), "No".into()));
        assert!(
            t.output()
                .contains("Unprocessed filter buckets remaining on input brigade")
        );
        detach(&mut t, &res, id);
    }

    #[test]
    fn seek_dispatch() {
        // user_filter_seek_02.phpt, user_filter_seek_03.phpt
        let mut t = TestVm::new();
        define(
            &mut t.vm,
            "CountingFilter",
            true,
            &[&PASS_FILTER, &LOG_SEEK],
            &[],
        );
        register(&mut t, "test.counting", "CountingFilter");

        let mut f = create(&mut t, "test.counting", &Value::Null)
            .unwrap()
            .unwrap();
        let ctx = FilterCtx {
            stream_res: None,
            is_persistent: false,
        };
        assert!(!f.ops.seek_in_chain(&mut t.vm, &ctx, 2, 5, 0).unwrap());
        assert!(f.ops.seek_in_chain(&mut t.vm, &ctx, 2, 0, 0).unwrap());
        assert_eq!(take_log(), ["seek 2:5:0", "seek 2:0:0"]);

        let res = streams::open_memory(&mut t.vm, b"ABC");
        let id = attach(&mut t, &res, f, true);
        assert_eq!(streams::read(&mut t.vm, &res, 2).unwrap().unwrap(), b"AB");
        take_log();
        assert_eq!(streams::seek(&mut t.vm, &res, 0, 0).unwrap(), 0);
        assert_eq!(take_log(), ["seek 1:0:0"]);
        assert_eq!(streams::read(&mut t.vm, &res, 10).unwrap().unwrap(), b"ABC");
        assert_eq!(streams::seek(&mut t.vm, &res, 1, 0).unwrap(), -1);
        assert_eq!(
            t.output(),
            "\nWarning: main(): Stream filter seeking for user-filter failed in /t.php on line 3\n"
        );
        detach(&mut t, &res, id);
        assert_eq!(take_log().last().unwrap(), "seek 1:1:0");

        // Without a seek() method seeking succeeds (BC).
        define(&mut t.vm, "noseek", false, &[&PASS_FILTER], &mixed_props());
        register(&mut t, "noseek", "noseek");
        let mut f = create(&mut t, "noseek", &Value::Null).unwrap().unwrap();
        assert!(f.ops.seek(&mut t.vm, 7, 0).unwrap());
        assert_eq!(take_log(), Vec::<String>::new());
    }

    #[test]
    fn bucket_new() {
        // bug39551.phpt
        let mut t = TestVm::new();
        let s = streams::open_memory(&mut t.vm, b"");
        let b = t
            .call("stream_bucket_new", vec![Value::Resource(s), "".into()])
            .unwrap();
        let Value::Object(o) = &b else { panic!() };
        assert_eq!(o.class().name.to_string(), "StreamBucket");
        assert!(
            matches!(o.get_prop(b"bucket"), Some(Value::Resource(r)) if r.type_name() == "userfilter.bucket")
        );
        assert!(matches!(o.get_prop(b"data"), Some(Value::String(s)) if s.is_empty()));
        assert!(matches!(o.get_prop(b"datalen"), Some(Value::Int(0))));
        assert!(matches!(o.get_prop(b"dataLength"), Some(Value::Int(0))));

        let s = streams::open_memory(&mut t.vm, b"");
        let b = t
            .call("stream_bucket_new", vec![Value::Resource(s), "abc".into()])
            .unwrap();
        assert_eq!(data_of(&b), b"abc");

        // gh15836.phpt: no stream during the closing flush
        assert_eq!(
            thrown(t.call("stream_bucket_new", vec![Value::Null, "x".into()])),
            (
                "TypeError".into(),
                "stream_bucket_new(): Argument #1 ($stream) must be of type resource, null given"
                    .into()
            )
        );
    }

    #[test]
    fn bucket_make_writeable_and_attach() {
        let mut t = TestVm::new();
        let (inb, inp) = brigade_with(&[b"ab", b"cd"]);
        let (outb, out) = brigade_with(&[]);
        let b1 = t
            .call("stream_bucket_make_writeable", vec![inp.clone()])
            .unwrap();
        assert_eq!(data_of(&b1), b"ab");
        let Value::Object(o1) = &b1 else { panic!() };
        assert!(matches!(o1.get_prop(b"datalen"), Some(Value::Int(2))));
        let b2 = t
            .call("stream_bucket_make_writeable", vec![inp.clone()])
            .unwrap();
        assert_eq!(data_of(&b2), b"cd");
        assert!(matches!(
            t.call("stream_bucket_make_writeable", vec![inp.clone()]),
            Ok(Value::Null)
        ));
        assert!(inb.borrow().is_empty());

        // bug35916.phpt: appending the same bucket twice keeps one copy.
        o1.set_prop(b"data", Value::from("AB"));
        t.call("stream_bucket_append", vec![out.clone(), b1.clone()])
            .unwrap();
        t.call("stream_bucket_append", vec![out.clone(), b1.clone()])
            .unwrap();
        assert_eq!(contents(&outb), [b"AB".to_vec()]);
        // Attaching moves a bucket between brigades.
        t.call("stream_bucket_append", vec![out.clone(), b2.clone()])
            .unwrap();
        t.call("stream_bucket_prepend", vec![out.clone(), b2.clone()])
            .unwrap();
        assert_eq!(contents(&outb), [b"cd".to_vec(), b"AB".to_vec()]);
        // bug72941.phpt: data written after make_writeable is synced on attach.
        let Value::Object(o2) = &b2 else { panic!() };
        o2.set_prop(b"data", Value::from("ello, worldd"));
        t.call("stream_bucket_append", vec![out.clone(), b2.clone()])
            .unwrap();
        assert_eq!(contents(&outb), [b"AB".to_vec(), b"ello, worldd".to_vec()]);
    }

    #[test]
    fn bucket_attach_with_active_brigades() {
        // gh17345.phpt: prepend, prepend a clone, prepend again: one bucket.
        let mut t = TestVm::new();
        let (inb, inp) = brigade_with(&[b"x"]);
        let (outb, out) = brigade_with(&[]);
        t.vm.ext_data::<ActiveBrigades>()
            .0
            .extend([Rc::downgrade(&inb), Rc::downgrade(&outb)]);
        let b = t
            .call("stream_bucket_make_writeable", vec![inp.clone()])
            .unwrap();
        let Value::Object(o) = &b else { panic!() };
        let sb = t.vm.find_class(b"StreamBucket").unwrap();
        let clone = t.vm.instantiate(&sb).unwrap();
        for p in [&b"bucket"[..], b"data", b"datalen", b"dataLength"] {
            clone.set_prop(p, o.get_prop(p).unwrap());
        }
        t.call("stream_bucket_prepend", vec![out.clone(), b.clone()])
            .unwrap();
        t.call(
            "stream_bucket_prepend",
            vec![out.clone(), Value::Object(clone)],
        )
        .unwrap();
        t.call("stream_bucket_prepend", vec![out.clone(), b.clone()])
            .unwrap();
        assert_eq!(contents(&outb), [b"x".to_vec()]);
        // A bucket put back on the input is taken off the output.
        t.call("stream_bucket_append", vec![inp.clone(), b.clone()])
            .unwrap();
        assert!(outb.borrow().is_empty());
        assert_eq!(contents(&inb), [b"x".to_vec()]);
    }

    #[test]
    fn bucket_errors() {
        let mut t = TestVm::new();
        let (_, out) = brigade_with(&[]);
        assert_eq!(
            thrown(t.call("stream_bucket_make_writeable", vec!["x".into()])),
            (
                "TypeError".into(),
                "stream_bucket_make_writeable(): Argument #1 ($brigade) must be of type resource, string given"
                    .into()
            )
        );
        let s = streams::open_memory(&mut t.vm, b"");
        assert_eq!(
            thrown(t.call(
                "stream_bucket_make_writeable",
                vec![Value::Resource(s.clone())]
            )),
            (
                "TypeError".into(),
                "stream_bucket_make_writeable(): supplied resource is not a valid userfilter.bucket brigade resource"
                    .into()
            )
        );
        let b = t
            .call("stream_bucket_new", vec![Value::Resource(s), "abc".into()])
            .unwrap();
        let Value::Object(o) = &b else { panic!() };
        assert_eq!(
            thrown(t.call("stream_bucket_append", vec![out.clone(), "x".into()])).0,
            "TypeError"
        );
        // bucket_data_unset.phpt
        let data = o.get_prop(b"data").unwrap();
        o.set_prop(b"data", Value::Undef);
        assert_eq!(
            thrown(t.call("stream_bucket_prepend", vec![out.clone(), b.clone()])),
            (
                "Error".into(),
                "Typed property StreamBucket::$data must not be accessed before initialization"
                    .into()
            )
        );
        o.set_prop(b"data", data);
        let bucket = o.get_prop(b"bucket").unwrap();
        o.set_prop(b"bucket", Value::Undef);
        assert_eq!(
            thrown(t.call("stream_bucket_append", vec![out.clone(), b.clone()])),
            (
                "TypeError".into(),
                "stream_bucket_append(): supplied argument is not a valid userfilter.bucket resource"
                    .into()
            )
        );
        assert_eq!(
            t.output(),
            "\nWarning: Undefined property: StreamBucket::$bucket in /t.php on line 3\n"
        );
        let Value::Resource(r) = &bucket else {
            panic!()
        };
        r.close();
        o.set_prop(b"bucket", bucket.clone());
        assert_eq!(
            thrown(t.call("stream_bucket_append", vec![out, b.clone()])),
            (
                "TypeError".into(),
                "stream_bucket_append(): supplied resource is not a valid userfilter.bucket resource"
                    .into()
            )
        );
    }

    #[test]
    fn make_writeable_copies_shared_buckets() {
        let mut t = TestVm::new();
        let (inb, inp) = brigade_with(&[b"ab"]);
        let held = inb.borrow().buckets[0].clone();
        let b = t.call("stream_bucket_make_writeable", vec![inp]).unwrap();
        let Value::Object(o) = &b else { panic!() };
        let Some(Value::Resource(r)) = o.get_prop(b"bucket") else {
            panic!()
        };
        let data = r.resource().data.borrow();
        let got = &data
            .as_ref()
            .unwrap()
            .downcast_ref::<BucketRes>()
            .unwrap()
            .0;
        assert!(!Rc::ptr_eq(got, &held));
        assert_eq!(got.borrow().buf, b"ab");
    }
}
