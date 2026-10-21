//! The `Throwable` hierarchy (port of `Zend/zend_exceptions.c`).
//!
//! Exceptions get `file`, `line` and `trace` when created
//! (`zend_default_exception_new`); the accessor methods, `__toString` and
//! `getTraceAsString` are native.

use crate::class::{ClassEntry, ClassRef, Function, PropertyInfo, class_flags, member_flags};
use crate::errors::{level, trace_to_string};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::types::TypeDecl;
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, PResult, Vm};

/// `zend_default_exception_new`: allocates the object and records
/// file/line/trace of the code currently executing.
pub fn exception_create_object(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    let ignore_args = vm.ini_bool("zend.exception_ignore_args");
    let trace = vm.backtrace(0, ignore_args, 0);
    obj.set_prop(b"trace", Value::array(trace));
    let (file, line) = if vm.executing {
        (vm.current_file.clone(), vm.current_line as i64)
    } else {
        (PhpStr::from("[no active file]"), 0)
    };
    obj.set_prop(b"file", Value::String(file));
    obj.set_prop(b"line", Value::Int(line));
    Ok(obj)
}

fn this(args: &Args) -> ObjRef {
    args.this()
        .cloned()
        .expect("exception method called without $this")
}

fn prop(o: &ObjRef, name: &[u8]) -> Value {
    o.get_prop(name).unwrap_or(Value::Null)
}

fn exception_construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    if args.is_passed(0) {
        o.set_prop(b"message", args.get(0).clone());
    }
    let code = args.int(1);
    if code != 0 {
        o.set_prop(b"code", Value::Int(code));
    }
    if let Some(prev) = args.object_opt(2) {
        o.set_prop(b"previous", Value::Object(prev.clone()));
    }
    Ok(Value::Null)
}

fn error_exception_construct(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    if args.is_passed(0) {
        o.set_prop(b"message", args.get(0).clone());
    }
    let code = args.int(1);
    if code != 0 {
        o.set_prop(b"code", Value::Int(code));
    }
    if let Some(prev) = args.object_opt(5) {
        o.set_prop(b"previous", Value::Object(prev.clone()));
    }
    o.set_prop(b"severity", Value::Int(args.int(2)));
    match (args.str_opt(3), args.int_opt(4)) {
        (Some(file), line) => {
            o.set_prop(b"file", Value::String(file.clone()));
            o.set_prop(b"line", Value::Int(line.unwrap_or(0)));
        }
        (None, Some(line)) => o.set_prop(b"line", Value::Int(line)),
        (None, None) => {}
    }
    Ok(Value::Null)
}

fn exception_wakeup(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

fn exception_clone(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Exception", "Cannot clone object using __clone()"))
}

fn get_message(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"message"))
}

fn get_code(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"code"))
}

fn get_file(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"file"))
}

fn get_line(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"line"))
}

fn get_trace(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"trace"))
}

fn get_previous(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"previous"))
}

fn get_severity(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(prop(&this(args), b"severity"))
}

/// `Exception::getTraceAsString`.
pub fn trace_as_string(vm: &Vm, o: &ObjRef) -> PhpStr {
    match prop(o, b"trace") {
        Value::Array(t) => {
            let max_len = vm.ini_int("zend.exception_string_param_max_len").max(0) as usize;
            trace_to_string(&t, true, max_len, vm.precision)
        }
        _ => PhpStr::from("#0 {main}"),
    }
}

fn get_trace_as_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(trace_as_string(vm, &this(args))))
}

/// `Exception::__toString` (walks the `previous` chain).
pub fn exception_to_string(vm: &mut Vm, ex: &ObjRef) -> PResult<PhpStr> {
    let mut out: Vec<u8> = Vec::new();
    let mut cur = Some(ex.clone());
    let mut seen: Vec<u32> = Vec::new();
    while let Some(e) = cur {
        if !e.class().instance_of_name(b"Throwable") || seen.contains(&e.id()) {
            break;
        }
        seen.push(e.id());
        let msg_v = prop(&e, b"message");
        let mut message = vm.to_php_string(&msg_v)?;
        let file_v = prop(&e, b"file");
        let file = vm.to_php_string(&file_v)?;
        let line_v = prop(&e, b"line");
        let line = vm.to_int(&line_v)?;
        let trace = trace_as_string(vm, &e);
        let lc = e.class().lc_name.as_bytes();
        if (lc == b"typeerror" || lc == b"argumentcounterror")
            && message.windows(12).any(|w| w == b", called in ")
        {
            message = message.concat(b" and defined");
        }
        let trace = if trace.is_empty() {
            PhpStr::from("#0 {main}\n")
        } else {
            trace
        };
        let mut s: Vec<u8> = Vec::new();
        s.extend_from_slice(&e.class().name);
        if !message.is_empty() {
            s.extend_from_slice(b": ");
            s.extend_from_slice(&message);
        }
        s.extend_from_slice(b" in ");
        s.extend_from_slice(&file);
        s.extend_from_slice(format!(":{line}\nStack trace:\n").as_bytes());
        s.extend_from_slice(&trace);
        if !out.is_empty() {
            s.extend_from_slice(b"\n\nNext ");
            s.extend_from_slice(&out);
        }
        out = s;
        cur = match prop(&e, b"previous") {
            Value::Object(p) => Some(p),
            _ => None,
        };
    }
    let s = PhpStr::from_vec(out);
    ex.set_prop(b"string", Value::String(s.clone()));
    Ok(s)
}

fn to_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let o = this(args);
    Ok(Value::String(exception_to_string(vm, &o)?))
}

static CONSTRUCT: NativeFunction = NativeFunction {
    sig: "__construct(string $message = \"\", int $code = 0, ?Throwable $previous = null)",
    handler: exception_construct,
};
static ERROR_EXCEPTION_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "__construct(string $message = \"\", int $code = 0, int $severity = E_ERROR, ?string $filename = null, ?int $line = null, ?Throwable $previous = null)",
    handler: error_exception_construct,
};
static WAKEUP: NativeFunction = NativeFunction {
    sig: "__wakeup(): void",
    handler: exception_wakeup,
};
static CLONE: NativeFunction = NativeFunction {
    sig: "__clone(): void",
    handler: exception_clone,
};
static GET_MESSAGE: NativeFunction = NativeFunction {
    sig: "getMessage(): string",
    handler: get_message,
};
static GET_CODE: NativeFunction = NativeFunction {
    sig: "getCode()",
    handler: get_code,
};
static GET_FILE: NativeFunction = NativeFunction {
    sig: "getFile(): string",
    handler: get_file,
};
static GET_LINE: NativeFunction = NativeFunction {
    sig: "getLine(): int",
    handler: get_line,
};
static GET_TRACE: NativeFunction = NativeFunction {
    sig: "getTrace(): array",
    handler: get_trace,
};
static GET_PREVIOUS: NativeFunction = NativeFunction {
    sig: "getPrevious(): ?Throwable",
    handler: get_previous,
};
static GET_TRACE_AS_STRING: NativeFunction = NativeFunction {
    sig: "getTraceAsString(): string",
    handler: get_trace_as_string,
};
static TO_STRING: NativeFunction = NativeFunction {
    sig: "__toString(): string",
    handler: to_string,
};
static GET_SEVERITY: NativeFunction = NativeFunction {
    sig: "getSeverity(): int",
    handler: get_severity,
};

fn base_class(
    name: &str,
    throwable: &ClassRef,
    stringable: &ClassRef,
    is_error: bool,
) -> ClassEntry {
    use member_flags::*;
    let mut ce = ClassEntry::new(name, 0);
    ce.interfaces = vec![throwable.clone(), stringable.clone()];
    let mut add = |pname: &str, flags: u32, ty: Option<&str>, default: Value| {
        let mut p = PropertyInfo::new(pname, name, flags);
        p.ty = ty.map(|t| TypeDecl::parse(t).expect("valid type"));
        p.default = default;
        ce.declare_property(p);
    };
    add("message", PROTECTED, None, Value::empty_string());
    add("string", PRIVATE, Some("string"), Value::empty_string());
    add("code", PROTECTED, None, Value::Int(0));
    add("file", PROTECTED, Some("string"), Value::empty_string());
    add(
        "line",
        PROTECTED,
        Some("int"),
        if is_error {
            Value::Undef
        } else {
            Value::Int(0)
        },
    );
    add("trace", PRIVATE, Some("array"), Value::empty_array());
    add("previous", PRIVATE, Some("?Throwable"), Value::Null);
    let methods: [(&'static NativeFunction, u32); 11] = [
        (&CLONE, PRIVATE),
        (&CONSTRUCT, PUBLIC),
        (&WAKEUP, PUBLIC),
        (&GET_MESSAGE, PUBLIC | FINAL),
        (&GET_CODE, PUBLIC | FINAL),
        (&GET_FILE, PUBLIC | FINAL),
        (&GET_LINE, PUBLIC | FINAL),
        (&GET_TRACE, PUBLIC | FINAL),
        (&GET_PREVIOUS, PUBLIC | FINAL),
        (&GET_TRACE_AS_STRING, PUBLIC | FINAL),
        (&TO_STRING, PUBLIC),
    ];
    for (nf, flags) in methods {
        ce.add_method(Function::native_method(nf, name, flags));
    }
    ce.handlers.create_object = Some(exception_create_object);
    ce
}

/// Registers `Throwable`, `Exception`, `ErrorException`, `Error` and the
/// standard `Error` subclasses.
pub fn register(r: &mut Registry<'_>) {
    let stringable = r
        .find_class("Stringable")
        .expect("Stringable registered first");
    let mut throwable = ClassEntry::new("Throwable", class_flags::INTERFACE);
    throwable.interfaces.push(stringable.clone());
    let throwable = r.class(throwable);

    let exception = r.class(base_class("Exception", &throwable, &stringable, false));
    let mut ee = ClassEntry::new_subclass("ErrorException", 0, &exception);
    let mut sev = PropertyInfo::new("severity", "ErrorException", member_flags::PROTECTED);
    sev.ty = Some(TypeDecl::parse("int").expect("int"));
    sev.default = Value::Int(level::E_ERROR);
    ee.declare_property(sev);
    ee.add_method(Function::native_method(
        &ERROR_EXCEPTION_CONSTRUCT,
        "ErrorException",
        member_flags::PUBLIC,
    ));
    ee.add_method(Function::native_method(
        &GET_SEVERITY,
        "ErrorException",
        member_flags::PUBLIC | member_flags::FINAL,
    ));
    r.class(ee);

    let error = r.class(base_class("Error", &throwable, &stringable, true));
    let compile_error = r.class(ClassEntry::new_subclass("CompileError", 0, &error));
    r.class(ClassEntry::new_subclass("ParseError", 0, &compile_error));
    let type_error = r.class(ClassEntry::new_subclass("TypeError", 0, &error));
    r.class(ClassEntry::new_subclass(
        "ArgumentCountError",
        0,
        &type_error,
    ));
    r.class(ClassEntry::new_subclass("ValueError", 0, &error));
    let arith = r.class(ClassEntry::new_subclass("ArithmeticError", 0, &error));
    r.class(ClassEntry::new_subclass("DivisionByZeroError", 0, &arith));
    r.class(ClassEntry::new_subclass("UnhandledMatchError", 0, &error));
    r.class(ClassEntry::new_subclass(
        "RequestParseBodyException",
        0,
        &exception,
    ));
}

impl Vm {
    /// Reports an uncaught exception (`zend_exception_error(ex, E_ERROR)`):
    /// `"PHP Fatal error:  Uncaught Exception: msg in file:line\nStack trace:\n#0 {main}\n  thrown in file on line N"`.
    pub fn report_uncaught_exception(&mut self, ex: &ObjRef) -> PResult<()> {
        let ce = ex.class().clone();
        let lc = ce.lc_name.as_bytes();
        let file_line = |o: &ObjRef| {
            let file = match o.get_prop(b"file") {
                Some(Value::String(s)) => s,
                _ => PhpStr::new(),
            };
            let line = o.get_prop(b"line").and_then(|v| v.as_int()).unwrap_or(0);
            (file, line)
        };
        if lc == b"parseerror" || lc == b"compileerror" {
            let msg = match ex.get_prop(b"message") {
                Some(Value::String(s)) => s,
                _ => PhpStr::new(),
            };
            let (file, line) = file_line(ex);
            let ty = if lc == b"parseerror" {
                level::E_PARSE
            } else {
                level::E_COMPILE_ERROR
            };
            return self.php_error_cb(ty | level::E_DONT_BAIL, file, line as u32, msg, None);
        }
        if !ce.instance_of_name(b"Throwable") {
            return self.zend_error(level::E_ERROR, format!("Uncaught exception {}", ce.name));
        }
        let r = self.call_method(ex, b"__tostring", Vec::new());
        match r {
            Ok(v) => match v.into_unref() {
                Value::String(s) => ex.set_prop(b"string", Value::String(s)),
                _ => {
                    let (file, line) = file_line(ex);
                    self.php_error_cb(
                        level::E_WARNING,
                        file,
                        line as u32,
                        PhpStr::from(format!("{}::__toString() must return a string", ce.name)),
                        None,
                    )?;
                }
            },
            Err(Flow::Exception(inner)) => {
                let (file, line) = file_line(&inner);
                let file = if file.is_empty() {
                    PhpStr::from("Unknown")
                } else {
                    file
                };
                let msg = format!(
                    "Uncaught {} in exception handling during call to {}::__toString()",
                    inner.class().name,
                    ce.name
                );
                self.php_error_cb(level::E_WARNING, file, line as u32, PhpStr::from(msg), None)?;
            }
            Err(f) => return Err(f),
        }
        let s = match ex.get_prop(b"string") {
            Some(Value::String(s)) => s,
            _ => PhpStr::new(),
        };
        let (file, line) = file_line(ex);
        let file = if file.is_empty() {
            PhpStr::from("Unknown")
        } else {
            file
        };
        let mut msg = b"Uncaught ".to_vec();
        msg.extend_from_slice(&s);
        msg.extend_from_slice(b"\n  thrown");
        self.php_error_cb(
            level::E_ERROR | level::E_DONT_BAIL,
            file,
            line as u32,
            PhpStr::from_vec(msg),
            None,
        )
    }

    /// The message of a throwable (for Rust-side assertions and diagnostics).
    pub fn exception_message(ex: &ObjRef) -> String {
        match ex.get_prop(b"message") {
            Some(Value::String(s)) => s.to_string(),
            Some(v) => format!("{v:?}"),
            None => String::new(),
        }
    }
}

/// Convenience: whether `flow` is an exception of class `name` (or a subclass).
pub fn is_exception_of(flow: &Flow, name: &str) -> bool {
    matches!(flow, Flow::Exception(o) if o.class().instance_of_name(name.as_bytes()))
}
