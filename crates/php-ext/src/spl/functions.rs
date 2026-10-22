//! ext/spl/php_spl.c — `class_implements()`, `class_parents()`,
//! `class_uses()`, `spl_classes()`, `spl_object_hash()`, `spl_object_id()`
//! and the autoloader stack (`spl_autoload_*`, which php-src keeps in
//! `Zend/zend_autoload.c`).
//!
//! # Interpreter contract
//! * **Autoloading**: the interpreter's `Host::autoload(vm, name)` should
//!   return [`autoload_class`]`(vm, name)`; it performs
//!   `zend_lookup_class_ex()`'s autoload part (class-name validation, the
//!   `EG(in_autoload)` recursion guard, leading-`\` stripping) and runs the
//!   registered loaders. [`spl_perform_autoload`] runs the loaders without
//!   those checks (`zend_perform_class_autoload`, what `spl_autoload_call()`
//!   does).
//! * **`spl_autoload()` includes**: install an [`IncludeHook`] with
//!   [`set_include_hook`]. It must `require` the file the way
//!   `spl_autoload()` does (`php_stream_open_for_zend_ex` with
//!   `USE_PATH | STREAM_OPEN_FOR_INCLUDE`, skipped when the path is already
//!   in `EG(included_files)`), and return whether the file was compiled
//!   and executed.
//! * **`class_uses()`**: install a [`ClassTraitsHook`] with
//!   [`set_class_traits_hook`] returning the names of the traits a user
//!   class uses (`ce->trait_names`); internal classes use none.

use php_runtime::class::{ClassRef, FunctionKind, fn_flags};
use php_runtime::{
    Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};

use super::engine::lc;

/// `SPL_DEFAULT_FILE_EXTENSIONS`.
const DEFAULT_FILE_EXTENSIONS: &str = ".inc,.php";

/// Requires a file for `spl_autoload()`; `Ok(true)` when it was compiled and
/// executed (see the module documentation).
pub type IncludeHook = fn(&mut Vm, &PhpStr) -> PResult<bool>;

/// Names of the traits a class uses, in declaration order (`ce->trait_names`).
pub type ClassTraitsHook = fn(&Vm, &ClassRef) -> Vec<PhpStr>;

/// Identity of a registered loader (`zend_fcc_equals`).
#[derive(Clone)]
enum LoaderId {
    /// A plain function, by lowercase name.
    Function(PhpStr),
    /// A method: `fcc->object` (for non-static calls), `fcc->calling_scope`
    /// (lowercase) and the method (lowercase).
    Method {
        object: Option<ObjRef>,
        scope: PhpStr,
        method: PhpStr,
    },
    /// A closure or invokable object (`fcc->closure`).
    Object(ObjRef),
}

impl PartialEq for LoaderId {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (LoaderId::Function(a), LoaderId::Function(b)) => a == b,
            (
                LoaderId::Method {
                    object: oa,
                    scope: sa,
                    method: ma,
                },
                LoaderId::Method {
                    object: ob,
                    scope: sb,
                    method: mb,
                },
            ) => {
                let same_obj = match (oa, ob) {
                    (Some(a), Some(b)) => ObjRef::ptr_eq(a, b),
                    (None, None) => true,
                    _ => false,
                };
                same_obj && sa == sb && ma == mb
            }
            (LoaderId::Object(a), LoaderId::Object(b)) => ObjRef::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// A registered autoloader (`zend_fcall_info_cache`).
#[derive(Clone)]
struct Loader {
    /// What is invoked.
    callable: Value,
    /// What `spl_autoload_functions()` reports (`zend_get_callable_zval_from_fcc`).
    display: Value,
    id: LoaderId,
}

/// Per-VM SPL state of `php_spl.c` / `zend_autoload.c`.
#[derive(Default)]
pub struct SplGlobals {
    /// `zend_class_autoload_functions`; `None` entries are deleted buckets,
    /// so a running `spl_perform_autoload` keeps its position when loaders
    /// unregister themselves.
    loaders: Vec<Option<Loader>>,
    /// `spl_autoload_extensions`.
    extensions: Option<PhpStr>,
    /// `EG(in_autoload)`: lowercase names being autoloaded.
    in_autoload: Vec<PhpStr>,
    include_hook: Option<IncludeHook>,
    traits_hook: Option<ClassTraitsHook>,
}

fn globals(vm: &mut Vm) -> &mut SplGlobals {
    vm.ext_data::<SplGlobals>()
}

/// Installs the interpreter's `require` for `spl_autoload()`.
pub fn set_include_hook(vm: &mut Vm, hook: IncludeHook) {
    globals(vm).include_hook = Some(hook);
}

/// Installs the interpreter's trait lookup for `class_uses()`.
pub fn set_class_traits_hook(vm: &mut Vm, hook: ClassTraitsHook) {
    globals(vm).traits_hook = Some(hook);
}

// ---------------------------------------------------------------------------
// class_implements / class_parents / class_uses
// ---------------------------------------------------------------------------

/// `spl_find_ce_by_name()`.
fn find_ce_by_name(vm: &mut Vm, name: &PhpStr, autoload: bool) -> PResult<Option<ClassRef>> {
    let ce = if autoload {
        vm.lookup_class(name.as_bytes())?
    } else {
        vm.find_class(name.as_bytes())
    };
    if ce.is_none() {
        vm.warning(format!(
            "Class {name} does not exist{}",
            if autoload {
                " and could not be loaded"
            } else {
                ""
            }
        ))?;
    }
    Ok(ce)
}

/// Common argument handling: an object or a class name (`false` when the
/// class cannot be found).
fn class_arg(vm: &mut Vm, args: &Args) -> PResult<Option<ClassRef>> {
    let autoload = args.bool(1);
    match args.get(0).unref() {
        Value::Object(o) => Ok(Some(o.class().clone())),
        Value::String(s) => find_ce_by_name(vm, &s, autoload),
        other => {
            let msg = format!(
                "must be of type object|string, {} given",
                other.value_name()
            );
            Err(vm.argument_type_error(1, &msg))
        }
    }
}

/// `spl_add_class_name()`.
fn add_class_name(list: &mut PhpArray, name: &PhpStr) {
    let key = ArrayKey::Str(name.clone());
    if !list.contains_key(&key) {
        list.insert(key, Value::String(name.clone()));
    }
}

fn class_parents(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(ce) = class_arg(vm, args)? else {
        return Ok(Value::Bool(false));
    };
    let mut list = PhpArray::new();
    let mut parent = ce.parent.clone();
    while let Some(p) = parent {
        add_class_name(&mut list, &p.name);
        parent = p.parent.clone();
    }
    Ok(Value::array(list))
}

fn class_implements(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(ce) = class_arg(vm, args)? else {
        return Ok(Value::Bool(false));
    };
    let mut list = PhpArray::new();
    for iface in &ce.interfaces {
        add_class_name(&mut list, &iface.name);
    }
    Ok(Value::array(list))
}

fn class_uses(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let Some(ce) = class_arg(vm, args)? else {
        return Ok(Value::Bool(false));
    };
    let mut list = PhpArray::new();
    if let Some(hook) = globals(vm).traits_hook {
        for name in hook(vm, &ce) {
            add_class_name(&mut list, &name);
        }
    }
    Ok(Value::array(list))
}

// ---------------------------------------------------------------------------
// spl_classes / spl_object_hash / spl_object_id
// ---------------------------------------------------------------------------

/// `SPL_LIST_CLASSES`.
pub const SPL_CLASSES: &[&str] = &[
    "AppendIterator",
    "ArrayIterator",
    "ArrayObject",
    "BadFunctionCallException",
    "BadMethodCallException",
    "CachingIterator",
    "CallbackFilterIterator",
    "DirectoryIterator",
    "DomainException",
    "EmptyIterator",
    "FilesystemIterator",
    "FilterIterator",
    "GlobIterator",
    "InfiniteIterator",
    "InvalidArgumentException",
    "IteratorIterator",
    "LengthException",
    "LimitIterator",
    "LogicException",
    "MultipleIterator",
    "NoRewindIterator",
    "OuterIterator",
    "OutOfBoundsException",
    "OutOfRangeException",
    "OverflowException",
    "ParentIterator",
    "RangeException",
    "RecursiveArrayIterator",
    "RecursiveCachingIterator",
    "RecursiveCallbackFilterIterator",
    "RecursiveDirectoryIterator",
    "RecursiveFilterIterator",
    "RecursiveIterator",
    "RecursiveIteratorIterator",
    "RecursiveRegexIterator",
    "RecursiveTreeIterator",
    "RegexIterator",
    "RuntimeException",
    "SeekableIterator",
    "SplDoublyLinkedList",
    "SplFileInfo",
    "SplFileObject",
    "SplFixedArray",
    "SplHeap",
    "SplMinHeap",
    "SplMaxHeap",
    "SplObjectStorage",
    "SplObserver",
    "SplPriorityQueue",
    "SplQueue",
    "SplStack",
    "SplSubject",
    "SplTempFileObject",
    "UnderflowException",
    "UnexpectedValueException",
];

fn spl_classes(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.engine_deprecated(
        "Function spl_classes() is deprecated since 8.6, use ReflectionExtension::getClassNames() instead",
    )?;
    let mut list = PhpArray::new();
    for name in SPL_CLASSES {
        if let Some(ce) = vm.find_class(name.as_bytes()) {
            add_class_name(&mut list, &ce.name);
        }
    }
    Ok(Value::array(list))
}

/// `php_spl_object_hash()`: the handle as 16 hex digits followed by 16 zeros.
pub fn object_hash(obj: &ObjRef) -> PhpStr {
    PhpStr::from(format!("{:016x}0000000000000000", obj.id()))
}

fn spl_object_hash(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.engine_deprecated(
        "Function spl_object_hash() is deprecated since 8.6, consider using spl_object_id() instead",
    )?;
    Ok(Value::String(object_hash(args.object(0))))
}

fn spl_object_id(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(args.object(0).id() as i64))
}

// ---------------------------------------------------------------------------
// Autoloading
// ---------------------------------------------------------------------------

/// Resolves a (ZPP-validated) callable to its loader identity, like
/// `zend_is_callable_ex()` filling a `zend_fcall_info_cache`.
fn resolve_loader(vm: &mut Vm, callable: &Value) -> Loader {
    let callable = callable.unref();
    let method_loader = |vm: &mut Vm, object: Option<ObjRef>, ce: ClassRef, m: &PhpStr| {
        let found = ce.find_method(m.as_bytes()).cloned();
        let method_name = found.as_ref().map_or_else(|| m.clone(), |f| f.name.clone());
        let is_static = found.as_ref().is_some_and(|f| f.is_static());
        let object = if is_static { None } else { object };
        let target = match &object {
            Some(o) => Value::Object(o.clone()),
            None => Value::String(ce.name.clone()),
        };
        let _ = vm;
        Loader {
            display: Value::array(PhpArray::from_values(vec![
                target,
                Value::String(method_name),
            ])),
            id: LoaderId::Method {
                object,
                scope: ce.lc_name.clone(),
                method: lc(m.as_bytes()),
            },
            callable: callable.clone(),
        }
    };
    match &callable {
        Value::String(s) => {
            let bytes = s.as_bytes();
            if let Some(pos) = bytes.windows(2).position(|w| w == b"::") {
                let (cls, m) = (&bytes[..pos], &bytes[pos + 2..]);
                if let Some(ce) = vm.find_class(cls) {
                    return method_loader(vm, None, ce, &PhpStr::from_bytes(m));
                }
            }
            let name = vm
                .find_function(bytes)
                .map_or_else(|| s.clone(), |f| f.name.clone());
            Loader {
                display: Value::String(name.clone()),
                id: LoaderId::Function(lc(name.as_bytes())),
                callable: callable.clone(),
            }
        }
        Value::Array(a) => {
            let target = a.get_int(0).map(Value::unref).unwrap_or(Value::Null);
            let m = match a.get_int(1).map(Value::unref) {
                Some(Value::String(m)) => m,
                _ => PhpStr::new(),
            };
            match target {
                Value::Object(o) => {
                    let ce = o.class().clone();
                    method_loader(vm, Some(o), ce, &m)
                }
                Value::String(c) => match vm.find_class(c.as_bytes()) {
                    Some(ce) => method_loader(vm, None, ce, &m),
                    None => Loader {
                        display: callable.clone(),
                        id: LoaderId::Method {
                            object: None,
                            scope: lc(c.as_bytes()),
                            method: lc(m.as_bytes()),
                        },
                        callable: callable.clone(),
                    },
                },
                _ => Loader {
                    display: callable.clone(),
                    id: LoaderId::Function(PhpStr::new()),
                    callable: callable.clone(),
                },
            }
        }
        Value::Object(o) => Loader {
            display: callable.clone(),
            id: LoaderId::Object(o.clone()),
            callable: callable.clone(),
        },
        _ => Loader {
            display: callable.clone(),
            id: LoaderId::Function(PhpStr::new()),
            callable: callable.clone(),
        },
    }
}

/// Whether a resolved callable is the internal function `name`.
fn is_internal_function(vm: &Vm, callable: &Value, name: &str) -> bool {
    let Value::String(s) = callable.unref() else {
        return false;
    };
    vm.find_function(s.as_bytes()).is_some_and(|f| {
        matches!(f.kind, FunctionKind::Native(_)) && f.name.eq_ignore_ascii_case(name.as_bytes())
    })
}

/// `zend_perform_class_autoload()`: calls the registered loaders in order
/// until one of them declares `class_name`.
pub fn perform_class_autoload(vm: &mut Vm, class_name: &PhpStr) -> PResult<Option<ClassRef>> {
    let lc_name = lc(class_name.as_bytes());
    let mut pos = 0;
    loop {
        let loader = match globals(vm).loaders.get(pos) {
            None => return Ok(None),
            Some(None) => {
                pos += 1;
                continue;
            }
            Some(Some(l)) => l.clone(),
        };
        vm.call(&loader.callable, vec![Value::String(class_name.clone())])?;
        if let Some(ce) = vm.find_class(lc_name.as_bytes()) {
            return Ok(Some(ce));
        }
        pos += 1;
    }
}

/// Runs the registered autoloaders for `class_name` (`spl_autoload_call()`).
pub fn spl_perform_autoload(vm: &mut Vm, class_name: &PhpStr) -> PResult<()> {
    perform_class_autoload(vm, class_name).map(|_| ())
}

/// `zend_is_valid_class_name()`.
fn is_valid_class_name(name: &[u8]) -> bool {
    name.iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'\\' || c >= 0x80)
}

/// The autoload part of `zend_lookup_class_ex()` for the interpreter's
/// `Host::autoload`: validates the name, guards against recursive
/// autoloading of the same class and strips a leading `\`.
pub fn autoload_class(vm: &mut Vm, name: &[u8]) -> PResult<Option<ClassRef>> {
    if !is_valid_class_name(name) {
        return Ok(None);
    }
    let lc_name = lc(name.strip_prefix(b"\\").unwrap_or(name));
    if globals(vm).in_autoload.contains(&lc_name) {
        return Ok(None);
    }
    globals(vm).in_autoload.push(lc_name.clone());
    let autoload_name = PhpStr::from_bytes(name.strip_prefix(b"\\").unwrap_or(name));
    let r = perform_class_autoload(vm, &autoload_name);
    let g = globals(vm);
    if let Some(i) = g.in_autoload.iter().position(|n| *n == lc_name) {
        g.in_autoload.remove(i);
    }
    r
}

/// `zend_autoload_register_class_loader()`.
fn register_loader(vm: &mut Vm, loader: Loader, prepend: bool) {
    let g = globals(vm);
    if g.loaders.iter().flatten().any(|l| l.id == loader.id) {
        return;
    }
    if prepend && g.loaders.iter().flatten().next().is_some() {
        // php-src moves the new bucket to the head and rehashes (which
        // drops deleted buckets).
        g.loaders.retain(Option::is_some);
        g.loaders.insert(0, Some(loader));
    } else {
        g.loaders.push(Some(loader));
    }
}

fn spl_autoload_register(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let do_throw = args.bool(1);
    let prepend = args.bool(2);
    if !do_throw {
        vm.notice(
            "Argument #2 ($do_throw) has been ignored, spl_autoload_register() will always throw",
        )?;
    }
    let loader = match args.opt(0).cloned() {
        Some(callable) => {
            if is_internal_function(vm, &callable, "spl_autoload_call") {
                return Err(
                    vm.argument_value_error(1, "must not be the spl_autoload_call() function")
                );
            }
            resolve_loader(vm, &callable)
        }
        None => resolve_loader(vm, &Value::str("spl_autoload")),
    };
    register_loader(vm, loader, prepend);
    Ok(Value::Bool(true))
}

fn spl_autoload_unregister(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let callable = args.get(0).clone();
    if is_internal_function(vm, &callable, "spl_autoload_call") {
        vm.deprecated(
            "Using spl_autoload_call() as a callback for spl_autoload_unregister() is deprecated, \
             to remove all registered autoloaders, call spl_autoload_unregister() for all values \
             returned from spl_autoload_functions()",
        )?;
        // zend_hash_clean(): a running autoload sees an empty table.
        globals(vm).loaders.clear();
        return Ok(Value::Bool(true));
    }
    let loader = resolve_loader(vm, &callable);
    let g = globals(vm);
    let found = g
        .loaders
        .iter_mut()
        .find(|l| l.as_ref().is_some_and(|l| l.id == loader.id));
    Ok(Value::Bool(match found {
        Some(slot) => {
            *slot = None;
            true
        }
        None => false,
    }))
}

fn spl_autoload_functions(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let list: Vec<Value> = globals(vm)
        .loaders
        .iter()
        .flatten()
        .map(|l| l.display.clone())
        .collect();
    Ok(Value::array(PhpArray::from_values(list)))
}

fn spl_autoload_call(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let class = args.str(0).clone();
    perform_class_autoload(vm, &class)?;
    Ok(Value::Null)
}

fn spl_autoload_extensions(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let g = globals(vm);
    if let Some(exts) = args.str_opt(0) {
        g.extensions = Some(exts.clone());
    }
    Ok(Value::String(
        g.extensions
            .clone()
            .unwrap_or_else(|| PhpStr::from(DEFAULT_FILE_EXTENSIONS)),
    ))
}

/// The static `spl_autoload()` helper: requires `lc_name` + `ext` (with `\`
/// turned into `/`) and reports whether that declared the class.
fn autoload_file(vm: &mut Vm, lc_name: &PhpStr, ext: &[u8]) -> PResult<bool> {
    let mut file: Vec<u8> = lc_name.as_bytes().to_vec();
    file.extend_from_slice(ext);
    if std::path::MAIN_SEPARATOR != '\\' {
        for b in &mut file {
            if *b == b'\\' {
                *b = b'/';
            }
        }
    }
    let Some(hook) = globals(vm).include_hook else {
        return Ok(false);
    };
    if !hook(vm, &PhpStr::from_vec(file))? {
        return Ok(false);
    }
    Ok(vm.classes.contains_key(lc_name.as_bytes()))
}

fn spl_autoload(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let class = args.str(0).clone();
    let exts = match args.str_opt(1) {
        Some(e) => e.clone(),
        None => globals(vm)
            .extensions
            .clone()
            .unwrap_or_else(|| PhpStr::from(DEFAULT_FILE_EXTENSIONS)),
    };
    let lc_name = lc(class
        .as_bytes()
        .strip_prefix(b"\\")
        .unwrap_or(class.as_bytes()));
    // `while (pos && *pos)`: an empty list (or a trailing ",") stops the loop.
    let mut rest: &[u8] = exts.as_bytes();
    while !rest.is_empty() {
        let (ext, next) = match rest.iter().position(|&c| c == b',') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        if autoload_file(vm, &lc_name, ext)? {
            break;
        }
        match next {
            Some(n) => rest = n,
            None => break,
        }
    }
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub static CLASS_IMPLEMENTS: NativeFunction = NativeFunction {
    sig: "function class_implements($object_or_class, bool $autoload = true): array|false {}",
    handler: class_implements,
};
pub static CLASS_PARENTS: NativeFunction = NativeFunction {
    sig: "function class_parents($object_or_class, bool $autoload = true): array|false {}",
    handler: class_parents,
};
pub static CLASS_USES: NativeFunction = NativeFunction {
    sig: "function class_uses($object_or_class, bool $autoload = true): array|false {}",
    handler: class_uses,
};
pub static SPL_AUTOLOAD: NativeFunction = NativeFunction {
    sig: "function spl_autoload(string $class, ?string $file_extensions = null): void {}",
    handler: spl_autoload,
};
pub static SPL_AUTOLOAD_CALL: NativeFunction = NativeFunction {
    sig: "function spl_autoload_call(string $class): void {}",
    handler: spl_autoload_call,
};
pub static SPL_AUTOLOAD_EXTENSIONS: NativeFunction = NativeFunction {
    sig: "function spl_autoload_extensions(?string $file_extensions = null): string {}",
    handler: spl_autoload_extensions,
};
pub static SPL_AUTOLOAD_FUNCTIONS: NativeFunction = NativeFunction {
    sig: "function spl_autoload_functions(): array {}",
    handler: spl_autoload_functions,
};
pub static SPL_AUTOLOAD_REGISTER: NativeFunction = NativeFunction {
    sig: "function spl_autoload_register(?callable $callback = null, bool $throw = true, bool $prepend = false): bool {}",
    handler: spl_autoload_register,
};
pub static SPL_AUTOLOAD_UNREGISTER: NativeFunction = NativeFunction {
    sig: "function spl_autoload_unregister(callable $callback): bool {}",
    handler: spl_autoload_unregister,
};
pub static SPL_CLASSES_FN: NativeFunction = NativeFunction {
    sig: "function spl_classes(): array {}",
    handler: spl_classes,
};
pub static SPL_OBJECT_HASH: NativeFunction = NativeFunction {
    sig: "function spl_object_hash(object $object): string {}",
    handler: spl_object_hash,
};
pub static SPL_OBJECT_ID: NativeFunction = NativeFunction {
    sig: "function spl_object_id(object $object): int {}",
    handler: spl_object_id,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &CLASS_IMPLEMENTS,
        &CLASS_PARENTS,
        &CLASS_USES,
        &SPL_AUTOLOAD,
        &SPL_AUTOLOAD_CALL,
        &SPL_AUTOLOAD_EXTENSIONS,
        &SPL_AUTOLOAD_FUNCTIONS,
        &SPL_AUTOLOAD_REGISTER,
        &SPL_AUTOLOAD_UNREGISTER,
    ]);
    r.function_with_flags(&SPL_CLASSES_FN, fn_flags::DEPRECATED);
    r.function_with_flags(&SPL_OBJECT_HASH, fn_flags::DEPRECATED);
    r.function(&SPL_OBJECT_ID);
}
