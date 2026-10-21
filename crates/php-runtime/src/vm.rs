//! The VM context (`executor_globals` + `core_globals` + `output_globals`).
//!
//! [`Vm`] is passed (`&mut`) to every operation that can emit diagnostics,
//! run user code or touch global state. It owns the output stack, the error
//! state, ini directives, constants, the function and class tables and the
//! call-frame stack used for backtraces and `fname(): ` error prefixes.
//!
//! The interpreter plugs in through the [`Host`] trait (calling userland
//! functions and callables, autoloading, destructors, constant expressions).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use crate::class::{CastKind, ClassRef, Function, FunctionKind};
use crate::constants::{ConstantTable, const_flags};
use crate::errors::{DisplayErrors, ErrorState};
use crate::ini::{self, IniRegistry, modifiable};
use crate::native::{CallArgs, ModuleRegister, Registry};
use crate::object::{self, ObjRef};
use crate::output::{OutputSink, OutputState, StdoutSink};
use crate::util::{NameMap, lower};
use crate::value::{PhpArray, PhpStr, Value};

/// Non-local control flow (`?` does the unwinding).
#[derive(Debug, Clone)]
pub enum Flow {
    /// A thrown `Throwable`.
    Exception(ObjRef),
    /// `break N`.
    Break(u32),
    /// `continue N`.
    Continue(u32),
    /// `return $v`.
    Return(Value),
    /// `exit(code)`; the message (if any) was already printed.
    Exit(i32),
    /// A fatal error was displayed; abort the request.
    Fatal,
}

pub type PResult<T> = Result<T, Flow>;

/// A call frame (for backtraces, `fname(): ` prefixes and argument names).
#[derive(Clone, Debug)]
pub struct Frame {
    /// The function being executed (`None` for include/eval pseudo frames).
    pub function: Option<Rc<Function>>,
    /// Name shown in traces (`strlen`, `{closure}`, `include`, ...).
    pub name: PhpStr,
    /// Class shown in traces (the declaring class of methods).
    pub class: Option<PhpStr>,
    /// `"->"` for instance calls, `"::"` for static calls.
    pub call_type: Option<&'static str>,
    pub this: Option<ObjRef>,
    /// Location of the call; `None` when called from internal code
    /// (`[internal function]` in traces).
    pub call_site: Option<(PhpStr, u32)>,
    /// Arguments as passed (named ones carry their name).
    pub args: Vec<(Option<PhpStr>, Value)>,
    pub is_internal: bool,
}

impl Frame {
    /// A frame for a userland function call made from the current location.
    pub fn user(vm: &Vm, function: Rc<Function>, this: Option<ObjRef>, args: Vec<Value>) -> Frame {
        let call_type = match (&this, &function.scope) {
            (Some(_), _) => Some("->"),
            (None, Some(_)) => Some("::"),
            _ => None,
        };
        Frame {
            name: function.name.clone(),
            class: function.scope.clone(),
            call_type,
            this,
            call_site: if vm.in_native_frame() {
                None
            } else {
                Some((vm.current_file.clone(), vm.current_line))
            },
            args: args.into_iter().map(|v| (None, v)).collect(),
            is_internal: false,
            function: Some(function),
        }
    }
}

/// Interpreter hooks. Every method has a default that handles native
/// functions only, so the runtime is usable (and testable) on its own.
pub trait Host {
    /// Invokes a PHP callable (`zend_call_function`): `"func"`,
    /// `"Class::method"`, `[$obj, "method"]`, `["Class", "method"]`, closures
    /// and invokable objects.
    fn call_callable(&self, vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        default_call_callable(vm, callable, args)
    }

    /// Runs a userland function body.
    fn call_user_function(
        &self,
        vm: &mut Vm,
        func: &Rc<Function>,
        call: CallArgs,
    ) -> PResult<Value> {
        let _ = call;
        Err(vm.throw_error(
            "Error",
            format!(
                "Cannot call userland function {}() without an interpreter",
                func.display_name()
            ),
        ))
    }

    /// Calls a method on an object (`zend_call_method`).
    fn call_method(
        &self,
        vm: &mut Vm,
        obj: &ObjRef,
        name: &[u8],
        args: Vec<Value>,
    ) -> PResult<Value> {
        default_call_method(vm, obj, name, args)
    }

    /// `zend_is_callable_ex`: returns the callable name or the error text used
    /// in "must be a valid callback, ..." messages.
    fn check_callable(&self, vm: &mut Vm, callable: &Value) -> Result<PhpStr, String> {
        default_check_callable(vm, callable)
    }

    /// Runs `__destruct` (and internal destructors) for `obj`.
    fn run_destructor(&self, vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
        if let Some(dtor) = obj.class().handlers.dtor_obj {
            dtor(vm, obj)?;
        }
        if obj.class().magic.destructor.is_some() {
            self.call_method(vm, obj, b"__destruct", Vec::new())?;
        }
        Ok(())
    }

    /// Autoloads a class (`spl_autoload_call`); `name` has no leading `\`.
    fn autoload(&self, vm: &mut Vm, name: &[u8]) -> PResult<Option<ClassRef>> {
        let _ = (vm, name);
        Ok(None)
    }

    /// Evaluates an unevaluated class constant expression.
    fn eval_class_constant(
        &self,
        vm: &mut Vm,
        class: &ClassRef,
        expr: &Rc<dyn Any>,
    ) -> PResult<Value> {
        let _ = expr;
        Err(vm.throw_error(
            "Error",
            format!("Cannot evaluate constant of class {}", class.name),
        ))
    }
}

/// The host used when no interpreter is attached.
pub struct DefaultHost;

impl Host for DefaultHost {}

/// Default callable dispatch for native functions and native methods.
pub fn default_call_callable(vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
    match &*callable.deref() {
        Value::String(s) => {
            if let Some(pos) = s.windows(2).position(|w| w == b"::") {
                let (cls, m) = (&s[..pos], &s[pos + 2..]);
                let Some(ce) = vm.lookup_class(cls)? else {
                    return Err(vm.throw_error(
                        "Error",
                        format!("Class \"{}\" not found", String::from_utf8_lossy(cls)),
                    ));
                };
                return call_static_method(vm, &ce, m, args);
            }
            match vm.find_function(s) {
                Some(f) => vm.call_function(
                    &f,
                    CallArgs {
                        dynamic: true,
                        ..CallArgs::new(args)
                    },
                ),
                None => Err(vm.throw_error("Error", format!("Call to undefined function {s}()"))),
            }
        }
        Value::Array(a) if a.len() == 2 => {
            let (Some(target), Some(Value::String(m))) = (
                a.get_int(0).map(Value::unref),
                a.get_int(1).map(Value::unref),
            ) else {
                return Err(
                    vm.throw_error("Error", "Array callback must have exactly two elements")
                );
            };
            match target {
                Value::Object(o) => {
                    let host = vm.host.clone();
                    host.call_method(vm, &o, &m, args)
                }
                Value::String(cls) => {
                    let Some(ce) = vm.lookup_class(&cls)? else {
                        return Err(vm.throw_error("Error", format!("Class \"{cls}\" not found")));
                    };
                    call_static_method(vm, &ce, &m, args)
                }
                _ => Err(vm.throw_error("Error", "Array callback must have exactly two elements")),
            }
        }
        Value::Object(o) => {
            let host = vm.host.clone();
            host.call_method(vm, o, b"__invoke", args)
        }
        _ => Err(vm.throw_error("Error", "Value not callable")),
    }
}

fn call_static_method(vm: &mut Vm, ce: &ClassRef, name: &[u8], args: Vec<Value>) -> PResult<Value> {
    match ce.find_method(name).cloned() {
        Some(m) => vm.call_function(
            &m,
            CallArgs {
                positional: args,
                scope: Some(ce.clone()),
                dynamic: true,
                ..CallArgs::default()
            },
        ),
        None => Err(vm.throw_error(
            "Error",
            format!(
                "Call to undefined method {}::{}()",
                ce.name,
                String::from_utf8_lossy(name)
            ),
        )),
    }
}

/// Default method dispatch: looks the method up in the object's class.
pub fn default_call_method(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &[u8],
    args: Vec<Value>,
) -> PResult<Value> {
    let class = obj.class().clone();
    match class.find_method(name).cloned() {
        Some(m) => vm.call_function(
            &m,
            CallArgs {
                positional: args,
                this: Some(obj.clone()),
                scope: Some(class),
                dynamic: true,
                ..CallArgs::default()
            },
        ),
        None => Err(vm.throw_error(
            "Error",
            format!(
                "Call to undefined method {}::{}()",
                class.name,
                String::from_utf8_lossy(name)
            ),
        )),
    }
}

/// Default `zend_is_callable_ex` for strings, arrays and invokable objects.
pub fn default_check_callable(vm: &mut Vm, callable: &Value) -> Result<PhpStr, String> {
    match &*callable.deref() {
        Value::String(s) => {
            if let Some(pos) = s.windows(2).position(|w| w == b"::") {
                let (cls, m) = (&s[..pos], &s[pos + 2..]);
                let Some(ce) = vm.find_class(cls) else {
                    return Err(format!(
                        "class \"{}\" not found",
                        String::from_utf8_lossy(cls)
                    ));
                };
                if ce.find_method(m).is_none() {
                    return Err(format!(
                        "class {} does not have a method \"{}\"",
                        ce.name,
                        String::from_utf8_lossy(m)
                    ));
                }
                return Ok(s.clone());
            }
            if vm.find_function(s).is_some() {
                Ok(s.clone())
            } else {
                Err(format!(
                    "function \"{s}\" not found or invalid function name"
                ))
            }
        }
        Value::Array(a) => {
            if a.len() != 2 {
                return Err("array callback must have exactly two members".into());
            }
            let (Some(target), Some(method)) = (
                a.get_int(0).map(Value::unref),
                a.get_int(1).map(Value::unref),
            ) else {
                return Err("array callback must have exactly two members".into());
            };
            let Value::String(m) = method else {
                return Err("second array member is not a valid method".into());
            };
            let ce = match &target {
                Value::Object(o) => o.class().clone(),
                Value::String(c) => match vm.find_class(c) {
                    Some(ce) => ce,
                    None => return Err(format!("class \"{c}\" not found")),
                },
                _ => return Err("first array member is not a valid class name or object".into()),
            };
            if ce.find_method(&m).is_none() {
                return Err(format!("class {} does not have a method \"{m}\"", ce.name));
            }
            Ok(PhpStr::from(format!("{}::{m}", ce.name)))
        }
        Value::Object(o) => {
            if o.class().find_method(b"__invoke").is_some()
                || o.class().handlers.get_closure.is_some()
            {
                Ok(PhpStr::from(format!("{}::__invoke", o.class().name)))
            } else {
                Err("no array or string given".into())
            }
        }
        _ => Err("no array or string given".into()),
    }
}

/// `PHP_VERSION` and friends (one place to change the advertised version).
pub mod version {
    pub const MAJOR: i64 = 8;
    pub const MINOR: i64 = 5;
    pub const RELEASE: i64 = 0;
    pub const EXTRA: &str = "";
    pub const VERSION: &str = "8.5.0";
    pub const VERSION_ID: i64 = MAJOR * 10000 + MINOR * 100 + RELEASE;
    pub const ZEND_VERSION: &str = "4.5.0";
}

/// The VM context.
pub struct Vm {
    pub host: Rc<dyn Host>,
    pub output: OutputState,
    pub errors: ErrorState,
    pub ini: IniRegistry,
    pub constants: ConstantTable,
    /// Function table keyed by lowercase name.
    pub functions: NameMap<Rc<Function>>,
    /// Class table keyed by lowercase name.
    pub classes: NameMap<ClassRef>,
    pub frames: Vec<Frame>,
    /// Location of the code being executed (updated by the interpreter).
    pub current_file: PhpStr,
    pub current_line: u32,
    /// Whether a script is running (affects error locations/origins).
    pub executing: bool,
    /// Mirrors of hot ini settings.
    pub precision: i64,
    pub serialize_precision: i64,
    pub exit_status: i32,
    /// Error-log sink (stderr for the CLI).
    pub stderr: Box<dyn Write>,
    /// Loaded module names (`get_loaded_extensions`).
    pub modules: Vec<&'static str>,
    /// Arrays currently being compared (recursion detection).
    pub(crate) compare_guard: Vec<usize>,
    ext_data: HashMap<TypeId, Box<dyn Any>>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    /// A VM writing to stdout with the core module registered and CLI ini defaults.
    pub fn new() -> Vm {
        Self::with_sink(Box::new(StdoutSink))
    }

    /// A VM writing its output to `sink`.
    pub fn with_sink(sink: Box<dyn OutputSink>) -> Vm {
        let mut vm = Vm {
            host: Rc::new(DefaultHost),
            output: OutputState::new(sink),
            errors: ErrorState::default(),
            ini: IniRegistry::new(),
            constants: ConstantTable::new(),
            functions: NameMap::new(),
            classes: NameMap::new(),
            frames: Vec::new(),
            current_file: PhpStr::from("Unknown"),
            current_line: 0,
            executing: false,
            precision: 14,
            serialize_precision: -1,
            exit_status: 0,
            stderr: Box::new(std::io::stderr()),
            modules: Vec::new(),
            compare_guard: Vec::new(),
            ext_data: HashMap::new(),
        };
        vm.register_module("Core", crate::builtins::register_core);
        // sapi/cli HARDCODED_INI and INI_DEFAULT values.
        for (k, v) in [
            ("html_errors", "0"),
            ("implicit_flush", "1"),
            ("output_buffering", "0"),
            ("max_execution_time", "0"),
            ("max_input_time", "-1"),
            ("display_errors", "1"),
        ] {
            vm.ini.set_default(k.as_bytes(), Some(PhpStr::from(v)));
            vm.sync_ini(k.as_bytes());
        }
        vm
    }

    /// Installs the interpreter hooks.
    pub fn set_host(&mut self, host: Rc<dyn Host>) {
        self.host = host;
    }

    /// Runs a module's registration function.
    pub fn register_module(&mut self, name: &'static str, f: ModuleRegister) {
        let mut r = Registry::new(self, name);
        f(&mut r);
    }

    /// Per-VM state for extensions, keyed by type (created on first use).
    pub fn ext_data<T: Default + 'static>(&mut self) -> &mut T {
        self.ext_data
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()))
            .downcast_mut::<T>()
            .expect("type-keyed")
    }

    // ----- functions, classes, constants --------------------------------

    /// Case-insensitive function lookup (a leading `\` is ignored).
    pub fn find_function(&self, name: impl AsRef<[u8]>) -> Option<Rc<Function>> {
        let name = name.as_ref();
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        if name.iter().any(u8::is_ascii_uppercase) {
            self.functions.get(&name.to_ascii_lowercase()).cloned()
        } else {
            self.functions.get(name).cloned()
        }
    }

    /// Registers a function; returns `false` if the name is taken.
    pub fn register_function(&mut self, f: Rc<Function>) -> bool {
        let key = lower(&f.name);
        if self.functions.contains_key(&key) {
            return false;
        }
        self.functions.insert(key, f);
        true
    }

    /// Case-insensitive class lookup without autoloading.
    pub fn find_class(&self, name: impl AsRef<[u8]>) -> Option<ClassRef> {
        let name = name.as_ref();
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        if name.iter().any(u8::is_ascii_uppercase) {
            self.classes.get(&name.to_ascii_lowercase()).cloned()
        } else {
            self.classes.get(name).cloned()
        }
    }

    /// Class lookup with autoloading (`zend_lookup_class`).
    pub fn lookup_class(&mut self, name: impl AsRef<[u8]>) -> PResult<Option<ClassRef>> {
        let name = name.as_ref();
        if let Some(c) = self.find_class(name) {
            return Ok(Some(c));
        }
        let host = self.host.clone();
        let name = name.strip_prefix(b"\\").unwrap_or(name);
        host.autoload(self, name)
    }

    /// Registers a class; returns `false` if the name is taken.
    pub fn register_class(&mut self, ce: ClassRef) -> bool {
        if self.classes.contains_key(&ce.lc_name) {
            return false;
        }
        self.classes.insert(ce.lc_name.clone(), ce);
        true
    }

    /// Constant value (no deprecation diagnostics).
    pub fn constant(&self, name: impl AsRef<[u8]>) -> Option<Value> {
        self.constants.get(name.as_ref()).map(|c| c.value.clone())
    }

    /// Constant fetch as done by code (`zend_get_constant_ex`): emits the
    /// deprecation for deprecated constants.
    pub fn fetch_constant(&mut self, name: impl AsRef<[u8]>) -> PResult<Option<Value>> {
        let Some(c) = self.constants.get(name.as_ref()) else {
            return Ok(None);
        };
        let (value, flags, msg, cname) = (c.value.clone(), c.flags, c.deprecation, c.name.clone());
        if flags & const_flags::DEPRECATED != 0 {
            let m = match msg {
                Some(m) => format!("Constant {cname} is deprecated, {m}"),
                None => format!("Constant {cname} is deprecated"),
            };
            self.engine_deprecated(m)?;
        }
        Ok(Some(value))
    }

    /// `define()`: returns `false` if the constant already exists.
    pub fn define_constant(&mut self, name: impl Into<PhpStr>, value: Value) -> bool {
        self.constants.define(crate::constants::Constant {
            name: name.into(),
            value,
            flags: 0,
            module: None,
            deprecation: None,
        })
    }

    /// `Class::NAME` lookup, evaluating lazily through the host if needed.
    pub fn class_constant(
        &mut self,
        class: impl AsRef<[u8]>,
        name: impl AsRef<[u8]>,
    ) -> PResult<Option<Value>> {
        let Some(ce) = self.lookup_class(class)? else {
            return Ok(None);
        };
        let Some(c) = ce.find_constant(name.as_ref()).cloned() else {
            return Ok(None);
        };
        let state = c.value.borrow().clone();
        match state {
            crate::class::ConstValue::Value(v) => Ok(Some(v)),
            crate::class::ConstValue::Evaluating => Err(self.throw_error(
                "Error",
                format!(
                    "Cannot declare self-referencing constant {}::{}",
                    ce.name, c.name
                ),
            )),
            crate::class::ConstValue::Unevaluated(expr) => {
                *c.value.borrow_mut() = crate::class::ConstValue::Evaluating;
                let host = self.host.clone();
                match host.eval_class_constant(self, &ce, &expr) {
                    Ok(v) => {
                        *c.value.borrow_mut() = crate::class::ConstValue::Value(v.clone());
                        Ok(Some(v))
                    }
                    Err(e) => {
                        *c.value.borrow_mut() = crate::class::ConstValue::Unevaluated(expr);
                        Err(e)
                    }
                }
            }
        }
    }

    // ----- ini -----------------------------------------------------------

    /// `ini_get()`: `None` for unknown directives.
    pub fn ini_get(&self, name: &str) -> Option<PhpStr> {
        self.ini.value(name.as_bytes())
    }

    /// `ini_set()` (`PHP_INI_USER`): returns the old value, or `None` on failure.
    pub fn ini_set(&mut self, name: impl AsRef<[u8]>, value: impl Into<PhpStr>) -> Option<PhpStr> {
        let name = name.as_ref();
        let r = self
            .ini
            .alter(name, value.into(), modifiable::USER, false)
            .ok();
        if r.is_some() {
            self.sync_ini(name);
        }
        r
    }

    /// Startup-time setting (`php -d name=value`).
    pub fn ini_set_system(&mut self, name: impl AsRef<[u8]>, value: impl Into<PhpStr>) -> bool {
        let name = name.as_ref();
        let ok = self.ini.set_default(name, Some(value.into()));
        if ok {
            self.sync_ini(name);
        }
        ok
    }

    /// `ini_restore()`.
    pub fn ini_restore(&mut self, name: impl AsRef<[u8]>) {
        let name = name.as_ref();
        if self.ini.restore(name) {
            self.sync_ini(name);
        }
    }

    /// An ini value as a boolean (`INI_BOOL`).
    pub fn ini_bool(&self, name: &str) -> bool {
        self.ini
            .value(name.as_bytes())
            .is_some_and(|v| ini::parse_bool(&v))
    }

    /// An ini value as an integer (`INI_INT`, with K/M/G suffixes).
    pub fn ini_int(&self, name: &str) -> i64 {
        self.ini
            .value(name.as_bytes())
            .map_or(0, |v| ini::atol_quantity(&v))
    }

    /// Re-applies side effects of an ini directive to the typed mirrors.
    pub(crate) fn sync_ini(&mut self, name: &[u8]) {
        let Some(v) = self.ini.get(name).map(|e| e.value.clone()) else {
            return;
        };
        let s = v.clone().unwrap_or_default();
        match name {
            b"precision" => self.precision = ini::atol_quantity(&s),
            b"serialize_precision" => self.serialize_precision = ini::atol_quantity(&s),
            b"error_reporting" => {
                self.errors.reporting = match v {
                    None => crate::errors::level::E_ALL,
                    Some(v) => ini::atoi(&v),
                }
            }
            b"display_errors" => {
                self.errors.display = match ini::display_errors_mode(&s) {
                    0 => DisplayErrors::Off,
                    2 => DisplayErrors::Stderr,
                    _ => DisplayErrors::Stdout,
                }
            }
            b"html_errors" => self.errors.html_errors = ini::parse_bool(&s),
            b"log_errors" => self.errors.log_errors = ini::parse_bool(&s),
            b"ignore_repeated_errors" => self.errors.ignore_repeated = ini::parse_bool(&s),
            b"ignore_repeated_source" => self.errors.ignore_repeated_source = ini::parse_bool(&s),
            b"fatal_error_backtraces" => self.errors.fatal_error_backtraces = ini::parse_bool(&s),
            b"error_prepend_string" => self.errors.prepend = v,
            b"error_append_string" => self.errors.append = v,
            b"implicit_flush" => self.output.implicit_flush = ini::parse_bool(&s),
            _ => {}
        }
    }

    // ----- frames ----------------------------------------------------------

    pub fn push_frame(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    pub fn pop_frame(&mut self) -> Option<Frame> {
        self.frames.pop()
    }

    /// Whether the innermost frame is a built-in function.
    pub fn in_native_frame(&self) -> bool {
        self.frames.last().is_some_and(|f| f.is_internal)
    }

    /// Name of the active function (`get_active_function_name`), `main` at top level.
    pub fn current_function_name(&self) -> String {
        match self.frames.last() {
            Some(f) if !f.name.is_empty() => f.name.to_string(),
            _ => "main".to_string(),
        }
    }

    /// `get_active_function_or_method_name`.
    pub fn current_function_display_name(&self) -> String {
        match self.frames.last().and_then(|f| f.function.as_ref()) {
            Some(func) => func.display_name(),
            None => self.current_function_name(),
        }
    }

    /// `zend_fetch_debug_backtrace`: innermost frame first.
    pub fn backtrace(&self, skip_last: usize, ignore_args: bool, limit: usize) -> PhpArray {
        self.backtrace_ex(skip_last, ignore_args, false, limit)
    }

    /// Like [`Vm::backtrace`] with `DEBUG_BACKTRACE_PROVIDE_OBJECT`.
    pub fn backtrace_ex(
        &self,
        skip_last: usize,
        ignore_args: bool,
        provide_object: bool,
        limit: usize,
    ) -> PhpArray {
        let mut out = PhpArray::new();
        for f in self.frames.iter().rev().skip(skip_last) {
            if limit > 0 && out.len() >= limit {
                break;
            }
            let mut e = PhpArray::new();
            if let Some((file, line)) = &f.call_site {
                e.insert_str("file", file.clone());
                e.insert_str("line", *line as i64);
            }
            e.insert_str("function", f.name.clone());
            if let Some(class) = &f.class {
                e.insert_str("class", class.clone());
                if provide_object {
                    if let Some(this) = &f.this {
                        e.insert_str("object", Value::Object(this.clone()));
                    }
                }
                e.insert_str("type", f.call_type.unwrap_or("::"));
            }
            if !ignore_args {
                let mut args = PhpArray::new();
                for (name, v) in &f.args {
                    match name {
                        Some(n) => {
                            args.insert(crate::value::ArrayKey::Str(n.clone()), v.clone());
                        }
                        None => {
                            let _ = args.append(v.clone());
                        }
                    }
                }
                e.insert_str("args", args);
            }
            let _ = out.append(e);
        }
        out
    }

    // ----- calls -------------------------------------------------------------

    /// Invokes a PHP callable through the host (`call_user_func`).
    pub fn call(&mut self, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        let host = self.host.clone();
        host.call_callable(self, callable, args)
    }

    /// Calls a function or method (native directly, userland via the host).
    pub fn call_function(&mut self, func: &Rc<Function>, call: CallArgs) -> PResult<Value> {
        match &func.kind {
            FunctionKind::Native(_) => self.invoke_native(func, call),
            FunctionKind::User(_) => {
                let host = self.host.clone();
                host.call_user_function(self, func, call)
            }
        }
    }

    /// Calls `$obj->name(...$args)` through the host.
    pub fn call_method(&mut self, obj: &ObjRef, name: &[u8], args: Vec<Value>) -> PResult<Value> {
        let host = self.host.clone();
        host.call_method(self, obj, name, args)
    }

    // ----- objects -------------------------------------------------------------

    /// Creates an instance (`object_init_ex`): the class's `create_object`
    /// handler or a plain object with default properties.
    pub fn instantiate(&mut self, class: &ClassRef) -> PResult<ObjRef> {
        match class.handlers.create_object {
            Some(f) => f(self, class),
            None => Ok(ObjRef::new(class.clone())),
        }
    }

    /// `cast_object` dispatch: the class handler, else the standard handler
    /// (`zend_std_cast_object_tostring`: `__toString` for strings, `true` for bool).
    pub fn object_cast(&mut self, obj: &ObjRef, kind: CastKind) -> PResult<Option<Value>> {
        if let Some(f) = obj.class().handlers.cast_object {
            return f(self, obj, kind);
        }
        match kind {
            CastKind::String => {
                if obj.class().magic.tostring.is_none() {
                    return Ok(None);
                }
                let v = self.call_method(obj, b"__tostring", Vec::new())?;
                match v.into_unref() {
                    Value::String(s) => Ok(Some(Value::String(s))),
                    _ => Err(self.throw_error(
                        "Error",
                        format!(
                            "Method {}::__toString() must return a string value",
                            obj.class().name
                        ),
                    )),
                }
            }
            CastKind::Bool => Ok(Some(Value::Bool(true))),
            _ => Ok(None),
        }
    }

    // ----- exceptions ------------------------------------------------------

    /// Builds (without throwing) an exception of `class` with message and code.
    /// Unknown classes fall back to `Error`.
    pub fn create_exception(
        &mut self,
        class: &str,
        message: impl Into<PhpStr>,
        code: i64,
    ) -> ObjRef {
        let ce = self
            .find_class(class.as_bytes())
            .or_else(|| self.find_class(b"Error"))
            .expect("core exception classes are registered");
        let obj = match self.instantiate(&ce) {
            Ok(o) => o,
            Err(_) => ObjRef::new(ce.clone()),
        };
        obj.set_prop(b"message", Value::String(message.into()));
        if code != 0 {
            obj.set_prop(b"code", Value::Int(code));
        }
        obj
    }

    /// `zend_throw_error(ce, msg)`: returns the `Flow` to propagate
    /// (`return Err(vm.throw_error("ValueError", "..."))`).
    pub fn throw_error(&mut self, class: &str, message: impl Into<PhpStr>) -> Flow {
        Flow::Exception(self.create_exception(class, message, 0))
    }

    /// Alias of [`Vm::throw_error`].
    pub fn throw(&mut self, class: &str, message: impl Into<PhpStr>) -> Flow {
        self.throw_error(class, message)
    }

    pub fn throw_type_error(&mut self, message: impl Into<PhpStr>) -> Flow {
        self.throw_error("TypeError", message)
    }

    pub fn throw_value_error(&mut self, message: impl Into<PhpStr>) -> Flow {
        self.throw_error("ValueError", message)
    }

    pub fn throw_arithmetic_error(&mut self, message: impl Into<PhpStr>) -> Flow {
        self.throw_error("ArithmeticError", message)
    }

    pub fn throw_division_by_zero(&mut self, message: impl Into<PhpStr>) -> Flow {
        self.throw_error("DivisionByZeroError", message)
    }

    // ----- destructors and shutdown -------------------------------------------

    /// Runs destructors of objects whose last reference was dropped.
    pub fn run_pending_destructors(&mut self) -> PResult<()> {
        while let Some(obj) = object::take_pending_destructor() {
            obj.clear_flag(object::obj_flags::DESTRUCTOR_QUEUED);
            if obj.destructor_called() {
                continue;
            }
            obj.mark_destructor_called();
            let host = self.host.clone();
            host.run_destructor(self, &obj)?;
        }
        Ok(())
    }

    /// Calls the destructors of all remaining live objects in handle order
    /// (`zend_objects_store_call_destructors`).
    pub fn call_remaining_destructors(&mut self) -> PResult<()> {
        self.run_pending_destructors()?;
        for obj in object::live_objects() {
            if obj.destructor_called() || !obj.class().has_destructor() {
                continue;
            }
            obj.mark_destructor_called();
            let host = self.host.clone();
            host.run_destructor(self, &obj)?;
            self.run_pending_destructors()?;
        }
        Ok(())
    }

    /// Request shutdown: destructors, then flush all output buffers.
    pub fn shutdown(&mut self) -> PResult<()> {
        let r = self.call_remaining_destructors();
        let o = self.ob_end_all();
        self.output.sink.flush();
        r.and(o)
    }
}
