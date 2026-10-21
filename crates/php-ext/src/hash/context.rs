//! The final class `HashContext` and the incremental hashing API:
//! `hash_init`, `hash_update`, `hash_update_stream`, `hash_update_file`,
//! `hash_final` and `hash_copy`, plus the `__serialize` / `__unserialize` /
//! `__debugInfo` methods (`ext/hash/hash.c`).

use std::cell::RefMut;

use php_hash::{HashAlgorithm, HashContext as Context, HashError, StateError, StateItem};
use php_runtime::class::{ClassEntry, ClassRef, Function, class_flags, member_flags};
use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::{HASH_HMAC, algo_arg, check_path, digest_value, init_error, init_options};
use crate::standard::streams::read_all;

/// The native state of a `HashContext` object (`php_hashcontext_object`).
///
/// `ctx` is `None` once the context has been finalized (or while an object
/// created by `unserialize()` is waiting for `__unserialize()`); `algo` stays
/// so that errors can still name the algorithm.
#[derive(Default)]
struct ContextState {
    algo: Option<&'static HashAlgorithm>,
    ctx: Option<Context>,
    /// The raw `$flags` given to `hash_init()`.
    options: i64,
}

/// `php_hashcontext_create`: objects always carry native state.
fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(ContextState::default());
    Ok(obj)
}

fn state_mut(obj: &ObjRef) -> RefMut<'_, ContextState> {
    if obj.native::<ContextState>().is_none() {
        obj.set_native(ContextState::default());
    }
    obj.native_mut::<ContextState>()
        .expect("native state was just installed")
}

/// `PHP_HASHCONTEXT_VERIFY`: the context of `obj`, or the TypeError PHP throws
/// for a finalized one.
fn live_ctx<'a>(vm: &mut Vm, obj: &'a ObjRef) -> PResult<RefMut<'a, Context>> {
    let ctx = RefMut::filter_map(state_mut(obj), |s| s.ctx.as_mut()).ok();
    match ctx {
        Some(ctx) => Ok(ctx),
        None => Err(vm.argument_type_error(1, "must be a valid, non-finalized HashContext")),
    }
}

/// Wraps `ctx` in a new `HashContext` object.
fn new_context(
    vm: &mut Vm,
    algo: &'static HashAlgorithm,
    ctx: Context,
    options: i64,
) -> PResult<Value> {
    let class = vm
        .find_class(b"HashContext")
        .expect("HashContext is registered with the hash module");
    let obj = vm.instantiate(&class)?;
    *state_mut(&obj) = ContextState {
        algo: Some(algo),
        ctx: Some(ctx),
        options,
    };
    Ok(Value::Object(obj))
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("HashContext method called without $this")
}

// ---------------------------------------------------------------------------
// hash_init() and friends
// ---------------------------------------------------------------------------

fn hash_init(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let algo = algo_arg(vm, args.str(0))?;
    let flags = args.int(1);
    let key = args.str(2).clone();
    let hmac = flags & HASH_HMAC != 0;

    if hmac {
        if !algo.is_crypto {
            return Err(vm.argument_value_error(
                1,
                "must be a cryptographic hashing algorithm if HMAC is requested",
            ));
        }
        if key.is_empty() {
            // A zero length key is no key at all.
            return Err(vm.argument_value_error(3, "must not be empty when HMAC is requested"));
        }
    }

    let options = init_options(vm, algo, Some(&**args.array(3)))?;
    let ctx = if hmac {
        Context::new_hmac(algo.name, &key, &options)
    } else {
        Context::new(algo.name, &options)
    }
    .map_err(|e| init_error(vm, e))?;
    new_context(vm, algo, ctx, flags)
}

fn hash_update(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    live_ctx(vm, &obj)?.update(args.str(1));
    Ok(Value::Bool(true))
}

/// Reads up to `len` bytes from a stream resource.
///
/// TODO(streams): wire this to the stream layer's `php_stream_read` (and the
/// `PHP_Z_PARAM_STREAM` validation of "supplied resource is not a valid stream
/// resource") once `crate::standard::streams` exposes a read-from-resource
/// function. An empty result means end of stream or a read error.
fn read_stream_chunk(vm: &mut Vm, stream: &Value, len: usize) -> PResult<Vec<u8>> {
    let _ = (vm, stream, len);
    todo!("hash_update_stream(): stream reads are implemented by the streams work package")
}

fn hash_update_stream(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    let stream = args.get(1).clone();
    let mut length = args.int(2);

    if !matches!(stream, Value::Resource(_)) {
        let msg = format!("must be of type resource, {} given", stream.value_name());
        return Err(vm.argument_type_error(2, &msg));
    }
    drop(live_ctx(vm, &obj)?);

    let mut did_read: i64 = 0;
    while length != 0 {
        let to_read = if length > 0 { length.min(1024) } else { 1024 };
        let chunk = read_stream_chunk(vm, &stream, to_read as usize)?;
        if chunk.is_empty() {
            break;
        }
        live_ctx(vm, &obj)?.update(&chunk);
        length -= chunk.len() as i64;
        did_read += chunk.len() as i64;
    }
    Ok(Value::Int(did_read))
}

fn hash_update_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    check_path(vm, args.str(1), 2)?;
    // `r!`: a stream context resource or null.
    if !matches!(args.get(2), Value::Null | Value::Undef | Value::Resource(_)) {
        let msg = format!(
            "must be of type resource or null, {} given",
            args.get(2).value_name()
        );
        return Err(vm.argument_type_error(3, &msg));
    }
    drop(live_ctx(vm, &obj)?);

    // TODO(streams): honour the `$stream_context` once the stream layer takes one.
    match read_all(vm, args.str(1))? {
        Some(contents) => {
            live_ctx(vm, &obj)?.update(&contents);
            Ok(Value::Bool(true))
        }
        None => Ok(Value::Bool(false)),
    }
}

fn hash_final(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    let raw = args.bool(1);
    drop(live_ctx(vm, &obj)?);
    let ctx = state_mut(&obj)
        .ctx
        .take()
        .expect("liveness was just verified");
    Ok(digest_value(ctx.finalize(), raw))
}

fn hash_copy(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = args.object(0).clone();
    drop(live_ctx(vm, &obj)?);
    Ok(Value::Object(clone_context(vm, &obj)?))
}

/// `php_hashcontext_clone`: duplicates the context (HMAC key included) and the
/// object's members.
fn clone_context(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    let (algo, ctx, options) = {
        let st = state_mut(obj);
        (st.algo, st.ctx.clone(), st.options)
    };
    let Some(ctx) = ctx else {
        return Err(vm.throw_error("ValueError", "Cannot clone a finalized HashContext"));
    };
    let class = obj.class().clone();
    let copy = vm.instantiate(&class)?;
    *state_mut(&copy) = ContextState {
        algo,
        ctx: Some(ctx),
        options,
    };
    *copy.obj().props.borrow_mut() = obj.obj().props.borrow().clone();
    *copy.obj().dyn_props.borrow_mut() = obj.obj().dyn_props.borrow().clone();
    Ok(copy)
}

// ---------------------------------------------------------------------------
// HashContext methods
// ---------------------------------------------------------------------------

fn construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // Normally unreachable: the constructor is private and final.
    Err(vm.throw_error("Error", "Illegal call to private/final constructor"))
}

/// Serialization format: `[algo, options, state, magic, members]`.
fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (algo, options, state) = {
        let st = state_mut(&obj);
        (
            st.algo,
            st.options,
            st.ctx.as_ref().map(Context::serialize_state),
        )
    };
    let name = algo.map_or("", |a| a.serialized_name);

    if options & HASH_HMAC != 0 {
        return Err(vm.throw_error(
            "Exception",
            "HashContext with HASH_HMAC option cannot be serialized",
        ));
    }
    let Some(Ok((magic, items))) = state else {
        return Err(vm.throw_error(
            "Exception",
            format!("HashContext for algorithm \"{name}\" cannot be serialized"),
        ));
    };

    let state = PhpArray::from_values(
        items
            .into_iter()
            .map(|item| match item {
                StateItem::Int(n) => Value::Int(n),
                StateItem::Bytes(b) => Value::String(PhpStr::from_vec(b)),
            })
            .collect(),
    );
    let members = obj.obj().dyn_props.borrow().clone().unwrap_or_default();
    Ok(Value::array(PhpArray::from_values(vec![
        Value::str(name),
        Value::Int(options),
        Value::array(state),
        Value::Int(magic),
        Value::array(members),
    ])))
}

/// Converts the serialized state array into the items the library expects.
///
/// PHP reads the state by index; values that are neither an integer nor a
/// string become an empty string, which never matches what a field expects and
/// so yields the same "malformed at this offset" failure.
fn state_items(state: &PhpArray) -> Vec<StateItem> {
    let mut items = Vec::new();
    while let Some(value) = state.get_int(items.len() as i64) {
        items.push(match value.unref() {
            Value::Int(n) => StateItem::Int(n),
            Value::String(s) => StateItem::Bytes(s.to_vec()),
            _ => StateItem::Bytes(Vec::new()),
        });
    }
    items
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let data = args.array(0).clone();

    if state_mut(&obj).ctx.is_some() {
        return Err(vm.throw_error(
            "Exception",
            "HashContext::__unserialize called on initialized object",
        ));
    }

    let field = |i: i64| data.get_int(i).map(Value::unref);
    let (Some(Value::String(algo_name)), Some(Value::Int(options)), Some(hash_state)) =
        (field(0), field(1), field(2))
    else {
        return Err(ill_formed(vm));
    };
    let (Some(Value::Int(magic)), Some(Value::Array(members))) = (field(3), field(4)) else {
        return Err(ill_formed(vm));
    };

    if options & HASH_HMAC != 0 {
        return Err(vm.throw_error(
            "Exception",
            "HashContext with HASH_HMAC option cannot be serialized",
        ));
    }
    let Some(algo) = super::fetch_algo(&algo_name) else {
        return Err(vm.throw_error("Exception", "Unknown hash algorithm"));
    };
    let serializable = algo
        .new_hasher(&php_hash::HashOptions::none())
        .is_ok_and(|h| h.serialize_state().is_ok());
    if !serializable {
        return Err(vm.throw_error(
            "Exception",
            format!(
                "Hash algorithm \"{}\" cannot be unserialized",
                algo.serialized_name
            ),
        ));
    }

    {
        let mut st = state_mut(&obj);
        st.algo = Some(algo);
        st.options = options;
    }
    let loaded = match &hash_state {
        Value::Array(state) => Context::from_state(algo.name, magic, &state_items(state)),
        // The spec loader only understands arrays.
        _ => Err(HashError::InvalidState(StateError::Failure)),
    };
    match loaded {
        Ok(ctx) => state_mut(&obj).ctx = Some(ctx),
        Err(err) => {
            let code = match err {
                HashError::InvalidState(e) => e.code(),
                _ => StateError::Failure.code(),
            };
            return Err(vm.throw_error(
                "Exception",
                format!(
                    "Incomplete or ill-formed serialization data (\"{}\" code {code})",
                    algo.serialized_name
                ),
            ));
        }
    }

    // `object_properties_load`
    for (key, value) in members.iter() {
        obj.set_prop(&key.to_php_str(), value.clone());
    }
    Ok(Value::Null)
}

fn ill_formed(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error("Exception", "Incomplete or ill-formed serialization data")
}

fn debug_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let algo = state_mut(&obj).algo;
    let mut info = PhpArray::new();
    if let Some(algo) = algo {
        info.insert_str("algo", Value::str(algo.serialized_name));
    }
    Ok(Value::array(info))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

static HASH_INIT: NativeFunction = NativeFunction {
    sig: "hash_init(string $algo, int $flags = 0, #[\\SensitiveParameter] string $key = \"\", array $options = []): HashContext",
    handler: hash_init,
};
static HASH_UPDATE: NativeFunction = NativeFunction {
    sig: "hash_update(HashContext $context, string $data): true",
    handler: hash_update,
};
static HASH_UPDATE_STREAM: NativeFunction = NativeFunction {
    sig: "hash_update_stream(HashContext $context, $stream, int $length = -1): int",
    handler: hash_update_stream,
};
static HASH_UPDATE_FILE: NativeFunction = NativeFunction {
    sig: "hash_update_file(HashContext $context, string $filename, $stream_context = null): bool",
    handler: hash_update_file,
};
static HASH_FINAL: NativeFunction = NativeFunction {
    sig: "hash_final(HashContext $context, bool $binary = false): string",
    handler: hash_final,
};
static HASH_COPY: NativeFunction = NativeFunction {
    sig: "hash_copy(HashContext $context): HashContext",
    handler: hash_copy,
};

static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "private function __construct() {}",
    handler: construct,
};
static SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __serialize(): array {}",
    handler: serialize,
};
static UNSERIALIZE: NativeFunction = NativeFunction {
    sig: "public function __unserialize(array $data): void {}",
    handler: unserialize,
};
static DEBUG_INFO: NativeFunction = NativeFunction {
    sig: "public function __debugInfo(): array {}",
    handler: debug_info,
};

/// Registers `hash_init()` … `hash_copy()` (in stub order).
pub(super) fn register_functions(r: &mut Registry) {
    r.function(&HASH_INIT);
    r.function(&HASH_UPDATE);
    r.function(&HASH_UPDATE_STREAM);
    r.function(&HASH_UPDATE_FILE);
    r.function(&HASH_FINAL);
    r.function(&HASH_COPY);
}

/// Registers `final class HashContext`.
pub(super) fn register_class(r: &mut Registry) {
    let mut ce = ClassEntry::new("HashContext", class_flags::FINAL);
    for (method, flags) in [
        (&CONSTRUCT, member_flags::PRIVATE),
        (&SERIALIZE, member_flags::PUBLIC),
        (&UNSERIALIZE, member_flags::PUBLIC),
        (&DEBUG_INFO, member_flags::PUBLIC),
    ] {
        ce.add_method(Function::native_method(method, "HashContext", flags));
    }
    ce.handlers.create_object = Some(create_object);
    ce.handlers.clone_obj = Some(clone_context);
    r.class(ce);
}
