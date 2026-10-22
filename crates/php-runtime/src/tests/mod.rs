//! Integration-style unit tests. Expectations are derived from php-src
//! (`zend_operators.c`, `zend_hash.c`, `zend_API.c`, `main/main.c`,
//! `main/output.c`) and `.phpt` files.

mod arrays;
mod convert;
mod enums;
mod errors;
mod objects;
mod ops;
mod output;
mod zpp;

use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use crate::native::{Args, NativeFunction, Registry};
use crate::output::CaptureSink;
use crate::value::{PhpStr, Value};
use crate::vm::{Flow, Host, PResult, Vm};

/// A `Write` sink shared with the test (for the error log).
#[derive(Clone, Default)]
pub struct SharedBuf(pub Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl SharedBuf {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.borrow()).into_owned()
    }
}

/// A VM "executing" `/t.php` line 3, capturing output and the error log.
pub struct TestVm {
    pub vm: Vm,
    pub out: CaptureSink,
    pub log: SharedBuf,
}

impl TestVm {
    pub fn new() -> TestVm {
        let out = CaptureSink::new();
        let log = SharedBuf::default();
        let mut vm = Vm::with_sink(Box::new(out.clone()));
        vm.stderr = Box::new(log.clone());
        vm.executing = true;
        vm.current_file = PhpStr::from("/t.php");
        vm.current_line = 3;
        vm.register_module("test", register_test_functions);
        vm.register_module("standard", crate::output::register_functions);
        TestVm { vm, out, log }
    }

    /// Output written so far (drained).
    pub fn output(&self) -> String {
        String::from_utf8_lossy(&self.out.take()).into_owned()
    }

    pub fn call(&mut self, name: &str, args: Vec<Value>) -> PResult<Value> {
        self.vm.call_native(name, args)
    }
}

/// Message and class of a thrown exception.
pub fn thrown(r: PResult<Value>) -> (String, String) {
    match r {
        Err(Flow::Exception(e)) => (e.class().name.to_string(), Vm::exception_message(&e)),
        Err(other) => panic!("expected exception, got {other:?}"),
        Ok(v) => panic!("expected exception, got value {v:?}"),
    }
}

pub fn s(v: &str) -> Value {
    Value::str(v)
}

// ----- test-only natives -----------------------------------------------------

fn echo_back(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(args.get(0).clone())
}

fn all_args(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let mut a = crate::value::PhpArray::new();
    for (i, v) in args.values().iter().enumerate() {
        let _ = a.append(if args.is_passed(i) {
            v.clone()
        } else {
            Value::str("<default>")
        });
    }
    Ok(Value::array(a))
}

fn strlen(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(args.str(0).len() as i64))
}

fn str_pad(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0).clone();
    let length = args.int(1);
    let pad = args.str(2).clone();
    let pad_type = args.int(3);
    if length < 0 || (length as usize) <= input.len() {
        return Ok(Value::String(input));
    }
    if pad.is_empty() {
        return Err(vm.argument_value_error(3, "must be a non-empty string"));
    }
    if !(0..=2).contains(&pad_type) {
        return Err(
            vm.argument_value_error(4, "must be STR_PAD_LEFT, STR_PAD_RIGHT, or STR_PAD_BOTH")
        );
    }
    let num_pad = length as usize - input.len();
    let (left, right) = match pad_type {
        0 => (num_pad, 0),
        1 => (0, num_pad),
        _ => (num_pad / 2, num_pad - num_pad / 2),
    };
    let mut out = Vec::new();
    for i in 0..left {
        out.push(pad[i % pad.len()]);
    }
    out.extend_from_slice(&input);
    for i in 0..right {
        out.push(pad[i % pad.len()]);
    }
    Ok(Value::from(out))
}

fn intdiv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(vm.intdiv(args.int(0), args.int(1))?))
}

fn str_repeat(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let times = args.int(1);
    if times < 0 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
    }
    Ok(Value::from(args.str(0).repeat(times as usize)))
}

fn t_warn(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let msg = args.str(0).clone();
    vm.warning(msg)?;
    Ok(Value::Null)
}

fn sort_like(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let r = args.by_ref(0);
    let mut v = r.get();
    if let Value::Array(a) = &mut v {
        let a = Rc::make_mut(a);
        a.sort_by(true, |x, y| {
            Ok::<_, ()>(
                crate::value::convert::to_int_silent(x.1)
                    .cmp(&crate::value::convert::to_int_silent(y.1)),
            )
        })
        .ok();
    }
    r.set(v);
    Ok(Value::Bool(true))
}

fn t_callback(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cb = args.get(0).clone();
    let rest = args.rest(1).to_vec();
    vm.call(&cb, rest)
}

macro_rules! nf {
    ($name:ident, $sig:expr, $h:expr) => {
        pub static $name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $h,
        };
    };
}

nf!(STRLEN, "function strlen(string $string): int {}", strlen);
nf!(
    STR_PAD,
    "function str_pad(string $string, int $length, string $pad_string = \" \", int $pad_type = STR_PAD_RIGHT): string {}",
    str_pad
);
nf!(
    INTDIV,
    "function intdiv(int $num1, int $num2): int {}",
    intdiv
);
nf!(
    STR_REPEAT,
    "function str_repeat(string $string, int $times): string {}",
    str_repeat
);
nf!(T_INT, "t_int(int $i): int", echo_back);
nf!(T_FLOAT, "t_float(float $f): float", echo_back);
nf!(T_BOOL, "t_bool(bool $b): bool", echo_back);
nf!(T_STR, "t_str(string $s): string", echo_back);
nf!(T_NULLABLE, "t_nullable(?int $v): ?int", echo_back);
nf!(T_INT_STR, "t_int_str(int|string $v): int|string", echo_back);
nf!(
    T_INT_FLOAT,
    "t_int_float(int|float $v): int|float",
    echo_back
);
nf!(
    T_ARR_STR,
    "t_arr_str(array|string $v): array|string",
    echo_back
);
nf!(T_ARR, "t_arr(array $a): array", echo_back);
nf!(T_ITER, "t_iter(iterable $it): iterable", echo_back);
nf!(T_OBJ, "t_obj(object $o): object", echo_back);
nf!(
    T_THROWABLE,
    "t_throwable(?Throwable $t): ?Throwable",
    echo_back
);
nf!(T_MIXED, "t_mixed(mixed $m): mixed", echo_back);
nf!(
    T_CB,
    "t_cb(callable $cb, mixed ...$args): mixed",
    t_callback
);
nf!(T_NCB, "t_ncb(?callable $cb): mixed", echo_back);
nf!(T_VAR, "t_var(int $a, string ...$rest): array", all_args);
nf!(
    T_UNKNOWN,
    "t_unknown(array $array, mixed $filter_value = UNKNOWN, bool $strict = false): array",
    all_args
);
nf!(
    T_DEFAULTS,
    "t_defaults(int $a = 1, string $b = \"x\", ?array $c = null, int $d = STR_PAD_LEFT | STR_PAD_BOTH, float $e = -1.5): array",
    all_args
);
nf!(T_WARN, "t_warn(string $message): void", t_warn);
nf!(
    SORT_LIKE,
    "function sort_like(array &$array, int $flags = SORT_REGULAR): true {}",
    sort_like
);

fn register_test_functions(r: &mut Registry<'_>) {
    r.constant("STR_PAD_LEFT", 0i64);
    r.constant("STR_PAD_RIGHT", 1i64);
    r.constant("STR_PAD_BOTH", 2i64);
    r.constant("SORT_REGULAR", 0i64);
    r.functions(&[
        &STRLEN,
        &STR_PAD,
        &INTDIV,
        &STR_REPEAT,
        &T_INT,
        &T_FLOAT,
        &T_BOOL,
        &T_STR,
        &T_NULLABLE,
        &T_INT_STR,
        &T_INT_FLOAT,
        &T_ARR_STR,
        &T_ARR,
        &T_ITER,
        &T_OBJ,
        &T_THROWABLE,
        &T_MIXED,
        &T_CB,
        &T_NCB,
        &T_VAR,
        &T_UNKNOWN,
        &T_DEFAULTS,
        &T_WARN,
        &SORT_LIKE,
    ]);
}

/// A host whose callables are Rust closures registered by name.
#[derive(Default)]
pub struct ClosureHost {
    #[allow(clippy::type_complexity)]
    pub fns: RefCell<Vec<(String, Rc<dyn Fn(&mut Vm, Vec<Value>) -> PResult<Value>>)>>,
}

impl ClosureHost {
    pub fn add(&self, name: &str, f: impl Fn(&mut Vm, Vec<Value>) -> PResult<Value> + 'static) {
        self.fns.borrow_mut().push((name.to_string(), Rc::new(f)));
    }

    #[allow(clippy::type_complexity)]
    fn find(&self, v: &Value) -> Option<Rc<dyn Fn(&mut Vm, Vec<Value>) -> PResult<Value>>> {
        let Value::String(s) = v.unref() else {
            return None;
        };
        self.fns
            .borrow()
            .iter()
            .find(|(n, _)| n.as_bytes() == s.as_bytes())
            .map(|(_, f)| f.clone())
    }
}

impl Host for ClosureHost {
    fn call_callable(&self, vm: &mut Vm, callable: &Value, args: Vec<Value>) -> PResult<Value> {
        match self.find(callable) {
            Some(f) => f(vm, args),
            None => crate::vm::default_call_callable(vm, callable, args),
        }
    }

    fn check_callable(&self, vm: &mut Vm, callable: &Value) -> Result<PhpStr, String> {
        match self.find(callable) {
            Some(_) => Ok(callable.unref().as_str().cloned().unwrap_or_default()),
            None => crate::vm::default_check_callable(vm, callable),
        }
    }
}
