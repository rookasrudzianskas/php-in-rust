//! Unit tests ported from php-src `ext/random/tests/**.phpt` (and the
//! shuffle/array_rand/str_shuffle behaviour that depends on the RNG).
//!
//! Seeded sequences must match php-src bit for bit. Userland engines (PHP
//! classes implementing `Random\Engine`) cannot run yet, so the tests use
//! native classes that are *not* flagged internal: the Randomizer drives them
//! through `generate()` like it will drive PHP engines later.

use std::collections::VecDeque;
use std::rc::Rc;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::value::float_fmt;
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpRef, Value, Vm,
};

use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($path:literal) => {
        include_str!(concat!("../../../../tests/phpt/ext/random/tests/", $path))
    };
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn f(d: f64) -> Value {
    Value::Float(d)
}

fn s(x: &str) -> Value {
    Value::str(x)
}

fn bytes(b: &[u8]) -> Value {
    Value::bytes(b)
}

fn dump(v: &Value) -> String {
    match v {
        Value::Int(n) => format!("int({n})"),
        Value::Float(d) => format!("float({})", float_fmt::repr(*d, -1)),
        Value::String(x) => format!("string({}) \"{}\"", x.len(), x.to_string_lossy()),
        Value::Bool(b) => format!("bool({b})"),
        Value::Null => "NULL".to_string(),
        other => format!("{other:?}"),
    }
}

fn as_int(v: &Value) -> i64 {
    v.as_int()
        .unwrap_or_else(|| panic!("expected int, got {v:?}"))
}

fn as_float(v: &Value) -> f64 {
    v.as_float()
        .unwrap_or_else(|| panic!("expected float, got {v:?}"))
}

fn as_bytes(v: &Value) -> Vec<u8> {
    v.as_str()
        .unwrap_or_else(|| panic!("expected string, got {v:?}"))
        .to_vec()
}

fn hex(v: &Value) -> String {
    as_bytes(v).iter().map(|b| format!("{b:02x}")).collect()
}

fn call(t: &mut TestVm, name: &str, args: Vec<Value>) -> Value {
    match t.call(name, args) {
        Ok(v) => v,
        Err(e) => panic!("{name}() threw {e:?}"),
    }
}

fn err(r: PResult<Value>) -> String {
    let (class, msg) = thrown(r);
    format!("{class}: {msg}")
}

fn deprecated(msg: &str) -> String {
    format!("\nDeprecated: {msg} in /t.php on line 3\n")
}

const MT_RAND_PHP_DEPRECATED: &str = "The MT_RAND_PHP variant of Mt19937 is deprecated";

fn class(t: &TestVm, name: &str) -> ClassRef {
    t.vm.find_class(name)
        .unwrap_or_else(|| panic!("class {name} registered"))
}

/// `new $class(...$args)`
fn new_obj(t: &mut TestVm, name: &str, args: Vec<Value>) -> ObjRef {
    try_new_obj(t, name, args).unwrap_or_else(|e| panic!("new {name}() threw {e:?}"))
}

fn try_new_obj(t: &mut TestVm, name: &str, args: Vec<Value>) -> PResult<ObjRef> {
    let ce = class(t, name);
    let obj = t.vm.instantiate(&ce)?;
    if ce.find_method(b"__construct").is_some() {
        t.vm.call_method(&obj, b"__construct", args)?;
    }
    Ok(obj)
}

fn method(t: &mut TestVm, obj: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    match t.vm.call_method(obj, name.as_bytes(), args) {
        Ok(v) => v,
        Err(e) => panic!("{}::{name}() threw {e:?}", obj.class().name),
    }
}

fn try_method(t: &mut TestVm, obj: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    t.vm.call_method(obj, name.as_bytes(), args)
}

fn obj(o: &ObjRef) -> Value {
    Value::Object(o.clone())
}

const MT: &str = "Random\\Engine\\Mt19937";
const PCG: &str = "Random\\Engine\\PcgOneseq128XslRr64";
const XOSHIRO: &str = "Random\\Engine\\Xoshiro256StarStar";
const SECURE: &str = "Random\\Engine\\Secure";
const RANDOMIZER: &str = "Random\\Randomizer";

fn randomizer(t: &mut TestVm, engine: &ObjRef) -> ObjRef {
    new_obj(t, RANDOMIZER, vec![obj(engine)])
}

fn clone_engine(t: &mut TestVm, o: &ObjRef) -> ObjRef {
    let handler = o.class().handlers.clone_obj.expect("clone handler");
    handler(&mut t.vm, o).expect("clone")
}

fn array(v: &Value) -> Rc<PhpArray> {
    v.as_array().expect("array").clone()
}

fn list(values: Vec<Value>) -> Value {
    Value::array(PhpArray::from_values(values))
}

fn range(start: i64, end: i64) -> Value {
    list((start..=end).map(i).collect())
}

/// The Random\* engines used by the "basic functionality" tests.
fn all_engines(t: &mut TestVm) -> Vec<(ObjRef, bool)> {
    let a = new_obj(t, MT, vec![Value::Null, i(0)]);
    let b = new_obj(t, MT, vec![Value::Null, i(1)]);
    t.output();
    let c = new_obj(t, PCG, vec![]);
    let d = new_obj(t, XOSHIRO, vec![]);
    let e = new_obj(t, SECURE, vec![]);
    let g = new_user_sha_like(t);
    vec![
        (a, false),
        (b, false),
        (c, false),
        (d, false),
        (e, false),
        (g, true),
    ]
}

// ---------------------------------------------------------------------------
// Userland-style engines (non-internal classes with native methods)
// ---------------------------------------------------------------------------

/// State of the scripted engines: the values `generate()` hands out in order.
struct Scripted {
    queue: VecDeque<Vec<u8>>,
}

struct Counter {
    count: u64,
    wide: bool,
}

struct Wrapper {
    engine: ObjRef,
    count: u32,
}

/// A repeating 20-byte "SHA-like" generator (stands in for TestShaEngine).
struct Shaish {
    state: u64,
}

fn this(args: &Args) -> ObjRef {
    args.this().cloned().expect("$this")
}

fn scripted_generate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let next = this
        .native_mut::<Scripted>()
        .and_then(|mut s| s.queue.pop_front());
    match next {
        Some(b) => Ok(Value::from(b)),
        None => Err(vm.throw_error("Exception", "Unhandled")),
    }
}

/// `public function &generate(): string` returning a reference to the field.
fn reference_generate(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let field = this.get_prop(b"field").unwrap_or(Value::Null);
    let r = match field {
        Value::Ref(r) => r,
        v => {
            let r = PhpRef::new(v);
            this.set_prop(b"field", Value::Ref(r.clone()));
            r
        }
    };
    Ok(Value::Ref(r))
}

fn counter_generate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mut st = this.native_mut::<Counter>().expect("counter state");
    if st.wide && st.count > 2147483647 {
        drop(st);
        return Err(vm.throw_error("Exception", "Overflow"));
    }
    let mut out = (st.count as u32).to_le_bytes().to_vec();
    if st.wide {
        out.extend_from_slice(&[0, 0, 0, 0]);
    }
    st.count += 1;
    Ok(Value::from(out))
}

fn wrapper_generate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let engine = {
        let mut st = this.native_mut::<Wrapper>().expect("wrapper state");
        st.count += 1;
        st.engine.clone()
    };
    vm.call_method(&engine, b"generate", Vec::new())
}

fn shaish_generate(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mut st = this.native_mut::<Shaish>().expect("sha state");
    // splitmix64 step
    st.state = st.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = st.state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    Ok(Value::from(z.to_le_bytes().to_vec()))
}

static SCRIPTED_GENERATE: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: scripted_generate,
};
static REFERENCE_GENERATE: NativeFunction = NativeFunction {
    sig: "public function &generate(): string",
    handler: reference_generate,
};
static COUNTER_GENERATE: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: counter_generate,
};
static WRAPPER_GENERATE: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: wrapper_generate,
};
static SHAISH_GENERATE: NativeFunction = NativeFunction {
    sig: "public function generate(): string",
    handler: shaish_generate,
};

fn register_user_engine(
    t: &mut TestVm,
    name: &str,
    generate: &'static NativeFunction,
    create: fn(&mut Vm, &ClassRef) -> PResult<ObjRef>,
) {
    let engine = class(t, "Random\\Engine");
    let mut ce = ClassEntry::new(name, class_flags::FINAL);
    ce.interfaces.push(engine);
    ce.add_method(Function::native_method(
        generate,
        name,
        member_flags::PUBLIC,
    ));
    ce.handlers.create_object = Some(create);
    t.vm.register_class(Rc::new(ce));
}

fn user_engine(t: &mut TestVm, name: &str, native: impl FnOnce(&ObjRef)) -> ObjRef {
    let ce = match t.vm.find_class(name) {
        Some(c) => c,
        None => {
            match name {
                "TestScripted" => register_user_engine(t, name, &SCRIPTED_GENERATE, |_, ce| {
                    Ok(ObjRef::new(ce.clone()))
                }),
                "TestReference" => register_user_engine(t, name, &REFERENCE_GENERATE, |_, ce| {
                    Ok(ObjRef::new(ce.clone()))
                }),
                "TestCounter" => register_user_engine(t, name, &COUNTER_GENERATE, |_, ce| {
                    Ok(ObjRef::new(ce.clone()))
                }),
                "TestWrapperEngine" => register_user_engine(t, name, &WRAPPER_GENERATE, |_, ce| {
                    Ok(ObjRef::new(ce.clone()))
                }),
                "TestShaEngine" => register_user_engine(t, name, &SHAISH_GENERATE, |_, ce| {
                    Ok(ObjRef::new(ce.clone()))
                }),
                other => panic!("unknown test engine {other}"),
            }
            class(t, name)
        }
    };
    let o = t.vm.instantiate(&ce).expect("instantiate");
    native(&o);
    o
}

fn new_scripted(t: &mut TestVm, values: &[&[u8]]) -> ObjRef {
    user_engine(t, "TestScripted", |o| {
        o.set_native(Scripted {
            queue: values.iter().map(|v| v.to_vec()).collect(),
        })
    })
}

fn new_counting(t: &mut TestVm, wide: bool) -> ObjRef {
    user_engine(t, "TestCounter", |o| {
        o.set_native(Counter { count: 0, wide })
    })
}

fn new_wrapper(t: &mut TestVm, engine: &ObjRef) -> ObjRef {
    let engine = engine.clone();
    user_engine(t, "TestWrapperEngine", |o| {
        o.set_native(Wrapper { engine, count: 0 })
    })
}

fn wrapper_count(o: &ObjRef) -> u32 {
    o.native::<Wrapper>().expect("wrapper").count
}

fn new_user_sha_like(t: &mut TestVm) -> ObjRef {
    user_engine(t, "TestShaEngine", |o| {
        o.set_native(Shaish { state: 0x1234 })
    })
}

// ---------------------------------------------------------------------------
// 01_functions
// ---------------------------------------------------------------------------

#[test]
fn mt_rand_value_php_mode() {
    // mt_rand_value.phpt
    let mut t = TestVm::new();
    call(&mut t, "mt_srand", vec![i(12345678), i(1)]);
    assert_eq!(t.output(), deprecated(MT_RAND_PHP_DEPRECATED));
    let expected = [
        1614640687, 1711027313, 857485497, 688176834, 1386682158, 412773096, 813703253, 898651287,
        2087374214, 1382556330, 1640700129, 1863374167, 1324097651, 1923803667, 676334965,
        853386222,
    ];
    for e in expected {
        assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), e);
    }
    let mut x = 0;
    for _ in 0..1024 {
        x ^= as_int(&call(&mut t, "mt_rand", vec![]));
    }
    assert_eq!(x, 1571178311);
}

#[test]
fn mt_rand_value_default_mode() {
    let mut t = TestVm::new();
    call(&mut t, "mt_srand", vec![i(12345678)]);
    let expected = [
        527860569, 1711027313, 1280820687, 688176834, 770499160, 412773096, 813703253, 898651287,
        52508912, 757323740, 511765911, 274407457, 833082629, 1923803667, 1461450755, 1301698200,
    ];
    for e in expected {
        assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), e);
    }
    let mut x = 0;
    for _ in 0..1024 {
        x ^= as_int(&call(&mut t, "mt_rand", vec![]));
    }
    assert_eq!(x, 1612214270);
    // srand() is an alias, rand() without arguments shares the generator
    call(&mut t, "srand", vec![i(12345678)]);
    assert_eq!(as_int(&call(&mut t, "rand", vec![])), 527860569);
}

#[test]
fn mt_srand_unknown_modes() {
    // mt_srand_unknown_mode.phpt
    let mut t = TestVm::new();
    call(&mut t, "mt_srand", vec![i(1), i(0)]);
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), 895547922);
    call(&mut t, "mt_srand", vec![i(1), i(1)]);
    assert_eq!(t.output(), deprecated(MT_RAND_PHP_DEPRECATED));
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), 1244335972);
    call(&mut t, "mt_srand", vec![i(1), i(2)]);
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), 895547922);
    call(&mut t, "mt_srand", vec![i(1), i(256)]);
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![])), 895547922);
    assert_eq!(t.output(), "");
}

#[test]
fn mt_rand_php_mode_range() {
    // bug75514.phpt
    let mut t = TestVm::new();
    call(&mut t, "mt_srand", vec![i(0), i(1)]);
    t.output();
    assert_eq!(
        as_int(&call(&mut t, "mt_rand", vec![i(0), i(999999999)])),
        448865905
    );
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![i(0), i(999)])), 592);
}

#[test]
fn mt_rand_modulo_bias() {
    // bug75170.phpt
    let mut t = TestVm::new();
    let max: i64 = 0x66666666;
    call(&mut t, "mt_srand", vec![i(1234567890)]);
    let mut halves = [0u32; 2];
    for _ in 0..10000 {
        let r = as_int(&call(&mut t, "mt_rand", vec![i(0), i(max - 1)]));
        halves[((r >> 1) & 1) as usize] += 1;
    }
    assert_eq!(
        format!(
            "{:.1}% vs. {:.1}%",
            halves[0] as f64 / 100.0,
            halves[1] as f64 / 100.0
        ),
        "49.5% vs. 50.5%"
    );
    call(&mut t, "mt_srand", vec![i(1234567890)]);
    let mut halves = [0u32; 2];
    for _ in 0..10000 {
        let r = as_int(&call(&mut t, "mt_rand", vec![i(0), i(max - 1)])) as f64;
        halves[(r / (max as f64 / 2.0)) as usize] += 1;
    }
    assert_eq!(
        format!(
            "{:.1}% vs. {:.1}%",
            halves[0] as f64 / 100.0,
            halves[1] as f64 / 100.0
        ),
        "50.5% vs. 49.5%"
    );
}

#[test]
fn mt_rand_and_rand_ranges() {
    let mut t = TestVm::new();
    assert_eq!(as_int(&call(&mut t, "mt_getrandmax", vec![])), 2147483647);
    assert_eq!(as_int(&call(&mut t, "getrandmax", vec![])), 2147483647);
    for name in ["mt_rand", "rand"] {
        for _ in 0..100 {
            let r = as_int(&call(&mut t, name, vec![]));
            assert!((0..=2147483647).contains(&r));
        }
        for (min, max) in [
            (10, 100),
            (100, 1000),
            (10500, 1050000),
            (16, 65536),
            (256, 448),
        ] {
            for _ in 0..100 {
                let r = as_int(&call(&mut t, name, vec![i(min), i(max)]));
                assert!((min..=max).contains(&r), "{name}({min}, {max}) = {r}");
            }
        }
    }
    // rand() accepts min > max (rand_inverted_order.phpt)
    for _ in 0..100 {
        let r = as_int(&call(&mut t, "rand", vec![i(20), i(10)]));
        assert!((10..=20).contains(&r));
    }
    // mt_rand() rejects it (bug46587.phpt)
    assert_eq!(
        err(t.call("mt_rand", vec![i(8), i(3)])),
        "ValueError: mt_rand(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)"
    );
    assert_eq!(
        err(t.call("mt_rand", vec![i(8)])),
        "ArgumentCountError: mt_rand() expects exactly 2 arguments, 1 given"
    );
    assert_eq!(
        err(t.call("rand", vec![i(8)])),
        "ArgumentCountError: rand() expects exactly 2 arguments, 1 given"
    );
    // extremes
    call(&mut t, "mt_rand", vec![i(i64::MIN), i(i64::MAX)]);
    assert_eq!(as_int(&call(&mut t, "mt_rand", vec![i(5), i(5)])), 5);
}

#[test]
fn rand_legacy_scaling_in_php_mode() {
    // rand() in MT_RAND_PHP mode scales with the biased legacy formula
    // `(max - min + 1.0) * (r / (PHP_MT_RAND_MAX + 1.0))`.
    let mut t = TestVm::new();
    call(&mut t, "mt_srand", vec![i(1), i(1)]);
    let reference = new_obj(&mut t, MT, vec![i(1), i(1)]);
    t.output();
    for _ in 0..50 {
        let r = u32::from_le_bytes(
            as_bytes(&method(&mut t, &reference, "generate", vec![]))
                .try_into()
                .unwrap(),
        ) >> 1;
        let expected = ((99.0 - 0.0 + 1.0) * (r as f64 / 2147483648.0)) as i64;
        assert_eq!(as_int(&call(&mut t, "rand", vec![i(0), i(99)])), expected);
    }
    // the swapped range is accepted, and (max - min) > PHP_INT_MAX works
    for _ in 0..50 {
        let r = as_int(&call(&mut t, "rand", vec![i(10), i(5)]));
        assert!((5..=10).contains(&r));
        call(&mut t, "rand", vec![i(i64::MIN), i(i64::MAX)]);
    }
}

#[test]
fn lcg_value_is_deprecated_and_in_range() {
    // lcg_value_basic.phpt / lcg_value_deprecation.phpt
    let mut t = TestVm::new();
    let first = as_float(&call(&mut t, "lcg_value", vec![]));
    assert_eq!(
        t.output(),
        deprecated(
            "Function lcg_value() is deprecated since 8.4, use \\Random\\Randomizer::getFloat() instead"
        )
    );
    assert!(first > 0.0 && first < 1.0);
    for _ in 0..1000 {
        let v = as_float(&call(&mut t, "lcg_value", vec![]));
        assert!(v > 0.0 && v < 1.0);
    }
}

#[test]
fn combined_lcg_is_deterministic_from_seed() {
    let mut a = [12345, 6789];
    let mut b = [12345, 6789];
    for _ in 0..100 {
        assert_eq!(
            super::algo::combined_lcg_step(&mut a),
            super::algo::combined_lcg_step(&mut b)
        );
    }
    // z = state0 - state1 (+ 2147483562 when < 1), scaled by 4.656613e-10
    let mut st = [1, 1];
    let v = super::algo::combined_lcg_step(&mut st);
    assert_eq!(st, [40014, 40692]);
    assert_eq!(v, 2147482884.0 * 4.656613e-10);
}

#[test]
fn random_bytes_and_random_int() {
    let mut t = TestVm::new();
    for len in [1, 2, 7, 8, 9, 64, 1000] {
        let r = call(&mut t, "random_bytes", vec![i(len)]);
        assert_eq!(as_bytes(&r).len(), len as usize);
    }
    assert_eq!(
        err(t.call("random_bytes", vec![i(0)])),
        "ValueError: random_bytes(): Argument #1 ($length) must be greater than 0"
    );
    assert_eq!(
        err(t.call("random_bytes", vec![])),
        "ArgumentCountError: random_bytes() expects exactly 1 argument, 0 given"
    );
    assert_eq!(
        err(t.call("random_int", vec![])),
        "ArgumentCountError: random_int() expects exactly 2 arguments, 0 given"
    );
    assert_eq!(
        err(t.call("random_int", vec![i(42)])),
        "ArgumentCountError: random_int() expects exactly 2 arguments, 1 given"
    );
    assert_eq!(
        err(t.call("random_int", vec![i(42), i(0)])),
        "ValueError: random_int(): Argument #1 ($min) must be less than or equal to argument #2 ($max)"
    );
    assert_eq!(as_int(&call(&mut t, "random_int", vec![i(7), i(7)])), 7);
    for _ in 0..200 {
        let r = as_int(&call(&mut t, "random_int", vec![i(-3), i(3)]));
        assert!((-3..=3).contains(&r));
        let r = as_int(&call(&mut t, "random_int", vec![i(0), i(255)]));
        assert!((0..=255).contains(&r));
    }
    call(&mut t, "random_int", vec![i(i64::MIN), i(i64::MAX)]);
    call(&mut t, "random_int", vec![i(i64::MIN), i(0)]);
    // a range that is not a power of two exercises the rejection loop
    let mut seen = [false; 3];
    for _ in 0..200 {
        seen[as_int(&call(&mut t, "random_int", vec![i(0), i(2)])) as usize] = true;
    }
    assert!(seen.iter().all(|s| *s));
    // reflection.phpt
    let rb = t.vm.find_function("random_bytes").expect("fn");
    assert_eq!(
        (rb.signature().params.len(), rb.signature().required),
        (1, 1)
    );
    let ri = t.vm.find_function("random_int").expect("fn");
    assert_eq!(
        (ri.signature().params.len(), ri.signature().required),
        (2, 2)
    );
}

#[test]
fn constants() {
    let t = TestVm::new();
    assert_eq!(dump(&t.vm.constant("MT_RAND_MT19937").unwrap()), "int(0)");
    assert_eq!(dump(&t.vm.constant("MT_RAND_PHP").unwrap()), "int(1)");
}

// ---------------------------------------------------------------------------
// 02_engine
// ---------------------------------------------------------------------------

#[test]
fn engine_reference_values() {
    // mt19937_value / pcgoneseq128xslrr64_value / xoshiro256starstar_value
    let mut t = TestVm::new();
    let e = new_obj(&mut t, MT, vec![i(1234)]);
    for _ in 0..10_000 {
        method(&mut t, &e, "generate", vec![]);
    }
    assert_eq!(hex(&method(&mut t, &e, "generate", vec![])), "60fe95d9");

    let e = new_obj(&mut t, PCG, vec![i(1234)]);
    for _ in 0..10_000 {
        method(&mut t, &e, "generate", vec![]);
    }
    method(&mut t, &e, "jump", vec![i(1234567)]);
    assert_eq!(
        hex(&method(&mut t, &e, "generate", vec![])),
        "b88e2a0f23720a1d"
    );

    let e = new_obj(&mut t, XOSHIRO, vec![i(1234)]);
    for _ in 0..10_000 {
        method(&mut t, &e, "generate", vec![]);
    }
    method(&mut t, &e, "jump", vec![]);
    method(&mut t, &e, "jumpLong", vec![]);
    assert_eq!(
        hex(&method(&mut t, &e, "generate", vec![])),
        "1f197e9ca7969123"
    );
}

#[test]
fn pcg_seeding() {
    // pcgoneseq128xslrr64_seed.phpt
    let mut t = TestVm::new();
    new_obj(&mut t, PCG, vec![i(random_i64())]);
    let seed = call(&mut t, "random_bytes", vec![i(16)]);
    new_obj(&mut t, PCG, vec![seed]);
    assert_eq!(
        err(try_new_obj(&mut t, PCG, vec![s("foobar")]).map(Value::Object)),
        "ValueError: Random\\Engine\\PcgOneseq128XslRr64::__construct(): Argument #1 ($seed) must be a 16 byte (128 bit) string"
    );
    let seed: Vec<u8> = [1u8, 2, 3, 4, 5, 6, 7, 8].repeat(2);
    let e = new_obj(&mut t, PCG, vec![bytes(&seed)]);
    let info = array(&method(&mut t, &e, "__debugInfo", vec![]));
    let states = array(info.get_str(b"__states").expect("__states"));
    assert_eq!(
        dump(states.get_int(0).unwrap()),
        "string(16) \"7afbdfd5830d8250\""
    );
    assert_eq!(
        dump(states.get_int(1).unwrap()),
        "string(16) \"dfc50b6959b3bafc\""
    );
    for _ in 0..1000 {
        method(&mut t, &e, "generate", vec![]);
    }
    assert_eq!(
        hex(&method(&mut t, &e, "generate", vec![])),
        "c42016cd9005ef2e"
    );
    // an integer seed becomes the low 64 bits of the 128 bit seed
    let a = new_obj(&mut t, PCG, vec![i(-1)]);
    let mut seed = vec![0u8; 8];
    seed.extend_from_slice(&[0xff; 8]);
    let b = new_obj(&mut t, PCG, vec![bytes(&seed)]);
    for _ in 0..5 {
        assert_eq!(
            as_bytes(&method(&mut t, &a, "generate", vec![])),
            as_bytes(&method(&mut t, &b, "generate", vec![]))
        );
    }
}

fn random_i64() -> i64 {
    let mut b = [0u8; 8];
    super::algo::random_bytes(&mut b).expect("urandom");
    i64::from_le_bytes(b)
}

#[test]
fn xoshiro_seeding() {
    // xoshiro256starstar_seed.phpt
    let mut t = TestVm::new();
    new_obj(&mut t, XOSHIRO, vec![i(random_i64())]);
    let seed = call(&mut t, "random_bytes", vec![i(32)]);
    new_obj(&mut t, XOSHIRO, vec![seed]);
    assert_eq!(
        err(try_new_obj(&mut t, XOSHIRO, vec![s("foobar")]).map(Value::Object)),
        "ValueError: Random\\Engine\\Xoshiro256StarStar::__construct(): Argument #1 ($seed) must be a 32 byte (256 bit) string"
    );
    assert_eq!(
        err(try_new_obj(&mut t, XOSHIRO, vec![bytes(&[0; 32])]).map(Value::Object)),
        "ValueError: Random\\Engine\\Xoshiro256StarStar::__construct(): Argument #1 ($seed) must not consist entirely of NUL bytes"
    );
    assert_eq!(
        err(try_new_obj(&mut t, XOSHIRO, vec![Value::empty_array()]).map(Value::Object)),
        "TypeError: Random\\Engine\\Xoshiro256StarStar::__construct(): Argument #1 ($seed) must be of type string|int|null, array given"
    );
    let seed: Vec<u8> = [1u8, 2, 3, 4, 5, 6, 7, 8].repeat(4);
    let e = new_obj(&mut t, XOSHIRO, vec![bytes(&seed)]);
    let info = array(&method(&mut t, &e, "__debugInfo", vec![]));
    assert_eq!(info.len(), 1);
    let states = array(info.get_str(b"__states").expect("__states"));
    for k in 0..4 {
        assert_eq!(
            dump(states.get_int(k).unwrap()),
            "string(16) \"0102030405060708\""
        );
    }
    for _ in 0..1000 {
        method(&mut t, &e, "generate", vec![]);
    }
    assert_eq!(
        hex(&method(&mut t, &e, "generate", vec![])),
        "90a025df9300cfd1"
    );
}

#[test]
fn mt19937_errors_and_modes() {
    // mt19937_error.phpt
    let mut t = TestVm::new();
    assert_eq!(
        err(try_new_obj(&mut t, MT, vec![i(1234), i(2)]).map(Value::Object)),
        "ValueError: Random\\Engine\\Mt19937::__construct(): Argument #2 ($mode) must be either MT_RAND_MT19937 or MT_RAND_PHP"
    );
    let e = new_obj(&mut t, MT, vec![i(1234), i(1)]);
    assert_eq!(t.output(), deprecated(MT_RAND_PHP_DEPRECATED));
    // Mt19937 in MT_RAND_PHP mode matches mt_rand(), including the seed 1 value
    let reference = new_obj(&mut t, MT, vec![i(1), i(1)]);
    t.output();
    assert_eq!(
        u32::from_le_bytes(
            as_bytes(&method(&mut t, &reference, "generate", vec![]))
                .try_into()
                .unwrap()
        ) >> 1,
        1244335972
    );
    let _ = e;
}

/// PHP's `serialize()` of the `__serialize()` data of an engine object.
fn serialized(t: &mut TestVm, engine: &ObjRef) -> String {
    let data = array(&method(t, engine, "__serialize", vec![]));
    fn ser(v: &Value, out: &mut String) {
        match v {
            Value::Int(n) => out.push_str(&format!("i:{n};")),
            Value::String(x) => {
                out.push_str(&format!("s:{}:\"{}\";", x.len(), x.to_string_lossy()))
            }
            Value::Array(a) => {
                out.push_str(&format!("a:{}:{{", a.len()));
                for (k, v) in a.iter() {
                    match k {
                        ArrayKey::Int(n) => out.push_str(&format!("i:{n};")),
                        ArrayKey::Str(s) => out.push_str(&format!("s:{}:\"{}\";", s.len(), s)),
                    }
                    ser(v, out);
                }
                out.push('}');
            }
            other => panic!("cannot serialize {other:?}"),
        }
    }
    let name = engine.class().name.to_string();
    let mut out = format!("O:{}:\"{name}\":{}:{{", name.len(), data.len());
    for (k, v) in data.iter() {
        out.push_str(&format!("i:{};", k.as_int().unwrap()));
        ser(v, &mut out);
    }
    out.push('}');
    out
}

fn expected_serialization(phpt: &str) -> String {
    let start = phpt.find("--EXPECT--\n").expect("EXPECT") + "--EXPECT--\n".len();
    phpt[start..].trim_end().to_string()
}

#[test]
fn engine_serialization_is_stable() {
    // mt19937_serialize / pcgoneseq128xslrr64_serialize / xoshiro256starstar_serialize
    let mut t = TestVm::new();
    let e = new_obj(&mut t, MT, vec![i(1234)]);
    assert_eq!(
        serialized(&mut t, &e),
        expected_serialization(phpt!("02_engine/mt19937_serialize.phpt"))
    );
    let e = new_obj(&mut t, PCG, vec![i(1234)]);
    assert_eq!(
        serialized(&mut t, &e),
        expected_serialization(phpt!("02_engine/pcgoneseq128xslrr64_serialize.phpt"))
    );
    let e = new_obj(&mut t, XOSHIRO, vec![i(1234)]);
    assert_eq!(
        serialized(&mut t, &e),
        expected_serialization(phpt!("02_engine/xoshiro256starstar_serialize.phpt"))
    );
}

#[test]
fn engine_serialization_roundtrip() {
    // all_serialize_native.phpt
    let mut t = TestVm::new();
    for name in [MT, PCG, XOSHIRO] {
        let engine = new_obj(&mut t, name, vec![i(1234)]);
        for _ in 0..10_000 {
            method(&mut t, &engine, "generate", vec![]);
        }
        let data = method(&mut t, &engine, "__serialize", vec![]);
        let engine2 = {
            let ce = class(&t, name);
            t.vm.instantiate(&ce).unwrap()
        };
        method(&mut t, &engine2, "__unserialize", vec![data]);
        for k in 0..10_000 {
            let a = method(&mut t, &engine, "generate", vec![]);
            let b = method(&mut t, &engine2, "generate", vec![]);
            assert_eq!(as_bytes(&a), as_bytes(&b), "{name} differs at {k}");
        }
    }
}

#[test]
fn engine_unserialize_errors() {
    // all_serialize_error.phpt and xoshiro256starstar_unserialize_zero_state.phpt
    let mut t = TestVm::new();
    let fresh = |t: &mut TestVm, name: &str| {
        let ce = class(t, name);
        t.vm.instantiate(&ce).unwrap()
    };
    let invalid = |name: &str| format!("Exception: Invalid serialization data for {name} object");

    // all-zero xoshiro state
    let zero = "0000000000000000";
    let data = list(vec![
        Value::empty_array(),
        list(vec![s(zero), s(zero), s(zero), s(zero)]),
    ]);
    let e = fresh(&mut t, XOSHIRO);
    assert_eq!(
        err(try_method(&mut t, &e, "__unserialize", vec![data])),
        invalid(XOSHIRO)
    );

    // malformed hex in Mt19937 (seven digits)
    let e = new_obj(&mut t, MT, vec![i(1234)]);
    let good = array(&method(&mut t, &e, "__serialize", vec![]));
    let mut states = (*array(good.get_int(1).unwrap())).clone();
    states.insert(ArrayKey::Int(0), s("5aa6b98"));
    let data = list(vec![Value::empty_array(), Value::array(states)]);
    let e2 = fresh(&mut t, MT);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );

    // non-hex characters
    let mut states = (*array(good.get_int(1).unwrap())).clone();
    states.insert(ArrayKey::Int(1), s("8660cc1g"));
    let data = list(vec![Value::empty_array(), Value::array(states)]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );

    // wrong element counts
    let data = list(vec![Value::empty_array()]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );
    let mut states = (*array(good.get_int(1).unwrap())).clone();
    let _ = states.pop();
    let data = list(vec![Value::empty_array(), Value::array(states)]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );

    // count out of range / invalid mode
    let mut states = (*array(good.get_int(1).unwrap())).clone();
    states.insert(ArrayKey::Int(624), i(625));
    let data = list(vec![Value::empty_array(), Value::array(states)]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );
    let mut states = (*array(good.get_int(1).unwrap())).clone();
    states.insert(ArrayKey::Int(625), i(2));
    let data = list(vec![Value::empty_array(), Value::array(states)]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );

    // members must be an array without dynamic properties
    let data = list(vec![s("x"), good.get_int(1).unwrap().clone()]);
    assert_eq!(
        err(try_method(&mut t, &e2, "__unserialize", vec![data])),
        invalid(MT)
    );
    let mut members = PhpArray::new();
    members.insert_str("foo", Value::Null);
    let data = list(vec![
        Value::array(members),
        good.get_int(1).unwrap().clone(),
    ]);
    let r = try_method(&mut t, &e2, "__unserialize", vec![data]);
    assert_eq!(err(r), invalid(MT));

    // PCG: wrong lengths
    let p = fresh(&mut t, PCG);
    let data = list(vec![
        Value::empty_array(),
        list(vec![s("0123456789abcdef")]),
    ]);
    assert_eq!(
        err(try_method(&mut t, &p, "__unserialize", vec![data])),
        invalid(PCG)
    );
    let data = list(vec![
        Value::empty_array(),
        list(vec![s("0123"), s("0123456789abcdef")]),
    ]);
    assert_eq!(
        err(try_method(&mut t, &p, "__unserialize", vec![data])),
        invalid(PCG)
    );
}

#[test]
fn secure_engine_properties() {
    let mut t = TestVm::new();
    let ce = class(&t, SECURE);
    assert!(ce.has_flag(class_flags::NOT_SERIALIZABLE));
    assert!(ce.has_flag(class_flags::FINAL));
    assert!(ce.instance_of_name(b"Random\\CryptoSafeEngine"));
    assert!(ce.instance_of_name(b"Random\\Engine"));
    let e = new_obj(&mut t, SECURE, vec![]);
    assert_eq!(as_bytes(&method(&mut t, &e, "generate", vec![])).len(), 8);
    // clone_obj = NULL
    let handler = ce.handlers.clone_obj.expect("handler");
    assert_eq!(
        err(handler(&mut t.vm, &e).map(Value::Object)),
        "Error: Trying to clone an uncloneable object of class Random\\Engine\\Secure"
    );
}

#[test]
fn engine_clone_copies_state() {
    let mut t = TestVm::new();
    for name in [MT, PCG, XOSHIRO] {
        let a = new_obj(&mut t, name, vec![i(42)]);
        for _ in 0..10 {
            method(&mut t, &a, "generate", vec![]);
        }
        let b = clone_engine(&mut t, &a);
        for _ in 0..100 {
            assert_eq!(
                as_bytes(&method(&mut t, &a, "generate", vec![])),
                as_bytes(&method(&mut t, &b, "generate", vec![]))
            );
        }
    }
}

#[test]
fn user_engines_match_native_sequences() {
    // user_compatibility.phpt / compatibility_user.phpt
    let mut t = TestVm::new();
    for name in [MT, PCG, XOSHIRO] {
        let native = new_obj(&mut t, name, vec![i(1234)]);
        let reference = clone_engine(&mut t, &native);
        let wrapped = new_wrapper(&mut t, &reference);
        for k in 0..10_000 {
            assert_eq!(
                as_bytes(&method(&mut t, &native, "generate", vec![])),
                as_bytes(&method(&mut t, &wrapped, "generate", vec![])),
                "{name} at {k}"
            );
        }
        let native_r = {
            let c = clone_engine(&mut t, &native);
            randomizer(&mut t, &c)
        };
        let user_r = {
            let c = clone_engine(&mut t, &native);
            let w = new_wrapper(&mut t, &c);
            randomizer(&mut t, &w)
        };
        for k in 0..3000 {
            let a = method(&mut t, &native_r, "getInt", vec![i(0), i(k)]);
            let b = method(&mut t, &user_r, "getInt", vec![i(0), i(k)]);
            assert_eq!(as_int(&a), as_int(&b), "{name} getInt at {k}");
        }
    }
}

#[test]
fn user_engine_returning_a_reference() {
    // user_reference_return.phpt
    let mut t = TestVm::new();
    let e = user_engine(&mut t, "TestReference", |o| {
        o.set_prop(b"field", s("abcdef"))
    });
    let r = randomizer(&mut t, &e);
    assert_eq!(
        dump(&method(&mut t, &r, "getBytes", vec![i(64)])),
        "string(64) \"abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd\""
    );
}

// ---------------------------------------------------------------------------
// 03_randomizer
// ---------------------------------------------------------------------------

#[test]
fn randomizer_getint_basic() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        for k in 0..2000 {
            let v = as_int(&method(&mut t, &r, "getInt", vec![i(-k), i(k)]));
            assert!((-k..=k).contains(&v), "{} at {k}", engine.class().name);
        }
        for _ in 0..500 {
            method(&mut t, &r, "getInt", vec![i(i64::MIN), i(i64::MAX)]);
        }
    }
}

#[test]
fn randomizer_getint_expansion() {
    // getInt_expansion_32.phpt / getInt_expansion_64.phpt: single-byte engines
    let mut t = TestVm::new();
    let bytes_8: Vec<Vec<u8>> = (1u8..=8).map(|b| vec![b]).collect();
    let refs: Vec<&[u8]> = bytes_8.iter().map(Vec::as_slice).collect();
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    let v = as_int(&method(&mut t, &r, "getInt", vec![i(0), i(0x00FF_FFFF)]));
    assert_eq!(format!("{:08x}", (v as u32).swap_bytes()), "01020300");
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    let v = as_int(&method(
        &mut t,
        &r,
        "getInt",
        vec![i(0), i(0x00FF_FFFF_FFFF_FFFF)],
    ));
    assert_eq!(
        format!("{:016x}", (v as u64).swap_bytes()),
        "0102030405060700"
    );
}

#[test]
fn randomizer_getint_gh9415() {
    let mut t = TestVm::new();
    for (seed, expected) in [(1234, -1324913873), (4321, -1843387587)] {
        let e = new_obj(&mut t, MT, vec![i(seed)]);
        let r = randomizer(&mut t, &e);
        assert_eq!(
            as_int(&method(
                &mut t,
                &r,
                "getInt",
                vec![i(-2147483648), i(2147483647)]
            )),
            expected
        );
    }
}

#[test]
fn randomizer_getint_errors() {
    let mut t = TestVm::new();
    let e = new_obj(&mut t, XOSHIRO, vec![i(1)]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(&mut t, &r, "getInt", vec![i(5), i(4)])),
        "ValueError: Random\\Randomizer::getInt(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)"
    );
}

#[test]
fn randomizer_matches_mt_rand() {
    // compatibility_mt_rand.phpt
    let mut t = TestVm::new();
    for (mode, mt_mode) in [(1, "MT_RAND_PHP"), (0, "MT_RAND_MT19937")] {
        let e = new_obj(&mut t, MT, vec![i(1234), i(mode)]);
        let r = randomizer(&mut t, &e);
        call(&mut t, "mt_srand", vec![i(1234), i(mode)]);
        t.output();
        for k in 0..10_000 {
            assert_eq!(
                as_int(&method(&mut t, &r, "nextInt", vec![])),
                as_int(&call(&mut t, "mt_rand", vec![])),
                "{mt_mode} nextInt at {k}"
            );
        }
        for k in 0..10_000 {
            assert_eq!(
                as_int(&method(&mut t, &r, "getInt", vec![i(0), i(k)])),
                as_int(&call(&mut t, "mt_rand", vec![i(0), i(k)])),
                "{mt_mode} getInt at {k}"
            );
        }
    }
}

#[test]
fn randomizer_matches_array_rand() {
    // compatibility_array_rand.phpt
    let mut t = TestVm::new();
    let mut map = PhpArray::new();
    for (k, v) in [("foo", 0), ("bar", 1), ("baz", 2)] {
        map.insert_str(k, i(v));
    }
    let map = Value::array(map);
    let list_arr = range(1, 10);
    call(&mut t, "mt_srand", vec![i(1234)]);
    let map_one = call(&mut t, "array_rand", vec![map.clone(), i(1)]);
    let map_two = call(&mut t, "array_rand", vec![map.clone(), i(2)]);
    let list_one = call(&mut t, "array_rand", vec![list_arr.clone(), i(1)]);
    let list_two = call(&mut t, "array_rand", vec![list_arr.clone(), i(2)]);
    let e = new_obj(&mut t, MT, vec![i(1234)]);
    let r = randomizer(&mut t, &e);
    let picked =
        |t: &mut TestVm, a: &Value, n: i64| method(t, &r, "pickArrayKeys", vec![a.clone(), i(n)]);
    let m1 = picked(&mut t, &map, 1);
    assert_eq!(dump(array(&m1).get_int(0).unwrap()), dump(&map_one));
    let m2 = picked(&mut t, &map, 2);
    assert_eq!(
        format!("{:?}", array(&m2)),
        format!("{:?}", array(&map_two))
    );
    let l1 = picked(&mut t, &list_arr, 1);
    assert_eq!(dump(array(&l1).get_int(0).unwrap()), dump(&list_one));
    let l2 = picked(&mut t, &list_arr, 2);
    assert_eq!(
        format!("{:?}", array(&l2)),
        format!("{:?}", array(&list_two))
    );
}

#[test]
fn array_rand_ignores_mt_rand_php() {
    // array_rand_mt_rand_php.phpt
    let mut t = TestVm::new();
    let array = list(vec![s("foo"), s("bar"), s("baz")]);
    call(&mut t, "mt_srand", vec![i(1), i(1)]);
    t.output();
    let keys: Vec<i64> = (0..3)
        .map(|_| as_int(&call(&mut t, "array_rand", vec![array.clone()])))
        .collect();
    assert_eq!(keys, [0, 1, 0]);
}

#[test]
fn array_rand_errors_and_shapes() {
    // array_rand.phpt
    let mut t = TestVm::new();
    let empty = Value::empty_array();
    assert_eq!(
        err(t.call("array_rand", vec![empty.clone()])),
        "ValueError: array_rand(): Argument #1 ($array) must not be empty"
    );
    assert_eq!(
        err(t.call("array_rand", vec![empty, i(0)])),
        "ValueError: array_rand(): Argument #1 ($array) must not be empty"
    );
    let three = list(vec![i(1), i(2), i(3)]);
    for n in [0, -1, 10] {
        assert_eq!(
            err(t.call("array_rand", vec![three.clone(), i(n)])),
            "ValueError: array_rand(): Argument #2 ($num) must be between 1 and the number of elements in argument #1 ($array)"
        );
    }
    let r = call(&mut t, "array_rand", vec![three.clone(), i(3)]);
    let keys: Vec<i64> = array(&r).values().map(as_int).collect();
    assert_eq!(keys, [0, 1, 2]);
    let r = call(&mut t, "array_rand", vec![three.clone(), i(2)]);
    let keys: Vec<i64> = array(&r).values().map(as_int).collect();
    assert_eq!(keys.len(), 2);
    assert!(keys[0] < keys[1]);
    // string keys come back as strings
    let mut assoc = PhpArray::new();
    for k in ["a", "b", "c", "d"] {
        assoc.insert_str(k, i(1));
    }
    let k = call(&mut t, "array_rand", vec![Value::array(assoc.clone())]);
    assert!(matches!(k, Value::String(_)));
    let r = call(&mut t, "array_rand", vec![Value::array(assoc), i(3)]);
    assert_eq!(array(&r).len(), 3);
}

#[test]
fn randomizer_getbytes() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        for len in 1..200 {
            let v = method(&mut t, &r, "getBytes", vec![i(len)]);
            assert_eq!(as_bytes(&v).len(), len as usize);
        }
    }
}

#[test]
fn randomizer_getbytes_expansion() {
    // getBytes_expansion.phpt
    let mut t = TestVm::new();
    let e = new_scripted(
        &mut t,
        &[
            b"H",
            b"e",
            b"ll",
            b"o",
            b"abcdefghijklmnopqrstuvwxyz",
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            b"success",
        ],
    );
    let r = randomizer(&mut t, &e);
    assert_eq!(
        dump(&method(&mut t, &r, "getBytes", vec![i(5)])),
        "string(5) \"Hello\""
    );
    assert_eq!(
        dump(&method(&mut t, &r, "getBytes", vec![i(11)])),
        "string(11) \"abcdefghABC\""
    );
    assert_eq!(
        dump(&method(&mut t, &r, "getBytes", vec![i(7)])),
        "string(7) \"success\""
    );
    assert_eq!(
        err(try_method(&mut t, &r, "getBytes", vec![i(0)])),
        "ValueError: Random\\Randomizer::getBytes(): Argument #1 ($length) must be greater than 0"
    );
}

#[test]
fn randomizer_get_bytes_from_string() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        assert_eq!(
            dump(&method(
                &mut t,
                &r,
                "getBytesFromString",
                vec![s("a"), i(10)]
            )),
            "string(10) \"aaaaaaaaaa\""
        );
        assert_eq!(
            dump(&method(
                &mut t,
                &r,
                "getBytesFromString",
                vec![s(&"a".repeat(256)), i(5)]
            )),
            "string(5) \"aaaaa\""
        );
        for k in 1..60 {
            let out = method(
                &mut t,
                &r,
                "getBytesFromString",
                vec![s(&"ab".repeat(k)), i(500)],
            );
            let out = as_bytes(&out);
            assert!(
                out.contains(&b'a') && out.contains(&b'b'),
                "{} at {k}",
                engine.class().name
            );
        }
    }
    let e = new_obj(&mut t, SECURE, vec![]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "getBytesFromString",
            vec![s(""), i(2)]
        )),
        "ValueError: Random\\Randomizer::getBytesFromString(): Argument #1 ($string) must not be empty"
    );
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "getBytesFromString",
            vec![s("abc"), i(0)]
        )),
        "ValueError: Random\\Randomizer::getBytesFromString(): Argument #2 ($length) must be greater than 0"
    );
}

#[test]
fn randomizer_get_bytes_from_string_fast_path() {
    // getBytesFromString_fast_path.phpt
    let mut t = TestVm::new();
    let all: Vec<u8> = (0..=255u8).collect();
    let xoshiro = new_obj(&mut t, XOSHIRO, vec![]);
    let mut n = 1usize;
    while n <= 256 {
        let wrapper = new_wrapper(&mut t, &xoshiro);
        let r = randomizer(&mut t, &wrapper);
        let result = method(
            &mut t,
            &r,
            "getBytesFromString",
            vec![bytes(&all[..n]), i(20000)],
        );
        // 64-bit engine, power-of-two source: exactly 20000 / 8 engine calls
        assert_eq!(wrapper_count(&wrapper), 2500, "{n}");
        let mut seen = [false; 256];
        for b in as_bytes(&result) {
            seen[b as usize] = true;
        }
        assert_eq!(seen.iter().filter(|s| **s).count(), n);
        n *= 2;
    }
    let mut n = 1usize;
    while n < 256 {
        let wrapper = new_wrapper(&mut t, &xoshiro);
        let r = randomizer(&mut t, &wrapper);
        let result = method(
            &mut t,
            &r,
            "getBytesFromString",
            vec![bytes(&all[..n + 1]), i(20000)],
        );
        let mut seen = [false; 256];
        for b in as_bytes(&result) {
            seen[b as usize] = true;
        }
        assert_eq!(seen.iter().filter(|s| **s).count(), n + 1);
        n *= 2;
    }
    // slow path: one call per byte
    let wrapper = new_wrapper(&mut t, &xoshiro);
    let r = randomizer(&mut t, &wrapper);
    let double: Vec<u8> = all.iter().chain(all.iter()).copied().collect();
    let result = method(
        &mut t,
        &r,
        "getBytesFromString",
        vec![bytes(&double), i(20000)],
    );
    assert_eq!(wrapper_count(&wrapper), 20000);
    let mut seen = [false; 256];
    for b in as_bytes(&result) {
        seen[b as usize] = true;
    }
    assert_eq!(seen.iter().filter(|s| **s).count(), 256);
}

#[test]
fn randomizer_next_int_and_float() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        for _ in 0..2000 {
            let v = as_int(&method(&mut t, &r, "nextInt", vec![]));
            assert!(v >= 0);
            let d = as_float(&method(&mut t, &r, "nextFloat", vec![]));
            assert!((0.0..1.0).contains(&d));
        }
    }
    // an engine delivering more than a native int is rejected on 32 bit only;
    // here a 9-byte value is truncated by the user engine to 8 bytes
    let e = new_scripted(&mut t, &[b"abcdefghi"]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        as_int(&method(&mut t, &r, "nextInt", vec![])),
        0x6867_6665_6463_6261u64 as i64 >> 1
    );
}

fn boundary(t: &mut TestVm, name: &str) -> Value {
    t.vm.class_constant("Random\\IntervalBoundary", name)
        .unwrap()
        .expect("case")
}

#[test]
fn randomizer_getfloat_ranges() {
    let mut t = TestVm::new();
    let closed = boundary(&mut t, "ClosedClosed");
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        let mut x = 0.0;
        while x < 2000.0 {
            let v = as_float(&method(
                &mut t,
                &r,
                "getFloat",
                vec![f(-x), f(x), closed.clone()],
            ));
            assert!((-x..=x).contains(&v));
            x += 1.2345;
        }
    }
}

#[test]
fn randomizer_getfloat_errors() {
    // getFloat_error.phpt
    let mut t = TestVm::new();
    let e = new_obj(&mut t, SECURE, vec![]);
    let r = randomizer(&mut t, &e);
    let prefix = "ValueError: Random\\Randomizer::getFloat(): Argument";
    for name in ["ClosedClosed", "ClosedOpen", "OpenClosed", "OpenOpen"] {
        let b = boundary(&mut t, name);
        let cc = name == "ClosedClosed";
        let call = |t: &mut TestVm, min: f64, max: f64| {
            try_method(t, &r, "getFloat", vec![f(min), f(max), b.clone()])
        };
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                err(call(&mut t, v, 0.0)),
                format!("{prefix} #1 ($min) must be finite")
            );
            assert_eq!(
                err(call(&mut t, 0.0, v)),
                format!("{prefix} #2 ($max) must be finite")
            );
        }
        let order = if cc {
            format!("{prefix} #2 ($max) must be greater than or equal to argument #1 ($min)")
        } else {
            format!("{prefix} #2 ($max) must be greater than argument #1 ($min)")
        };
        assert_eq!(err(call(&mut t, 0.0, -0.1)), order);
        if cc {
            assert_eq!(dump(&call(&mut t, 0.0, 0.0).unwrap()), "float(0)");
            // both values round to the same float
            assert_eq!(
                dump(&call(&mut t, 100_000_000_000_000_000.0, 100_000_000_000_000_000.1).unwrap()),
                "float(1.0E+17)"
            );
            let v = as_float(&call(&mut t, 1.0, 1.0 + f64::EPSILON).unwrap());
            assert!(v == 1.0 || v == 1.0 + f64::EPSILON);
        } else {
            assert_eq!(err(call(&mut t, 0.0, 0.0)), order);
            assert_eq!(
                err(call(
                    &mut t,
                    100_000_000_000_000_000.0,
                    100_000_000_000_000_000.1
                )),
                order
            );
            let v = call(&mut t, 1.0, 1.0 + f64::EPSILON);
            match name {
                "ClosedOpen" => assert_eq!(dump(&v.unwrap()), "float(1)"),
                "OpenClosed" => assert_eq!(dump(&v.unwrap()), "float(1.0000000000000002)"),
                _ => assert_eq!(
                    err(v),
                    "ValueError: The given interval is empty, there are no floats between argument #1 ($min) and argument #2 ($max)"
                ),
            }
        }
    }
}

/// `printf("%.17g", $v)`
fn g17(v: f64) -> String {
    let body = float_fmt::gcvt(v.abs(), 17, b'e');
    if v.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

#[test]
fn randomizer_getfloat_extreme_range() {
    // getFloat_extreme_range.phpt
    let mut t = TestVm::new();
    let e = new_scripted(
        &mut t,
        &[
            &[0x64, 0xe8, 0x58, 0x79, 0x3e, 0xf6, 0x38, 0x00],
            &[0x65, 0xe8, 0x58, 0x79, 0x3e, 0xf6, 0x38, 0x00],
            &[0; 8],
        ],
    );
    let r = randomizer(&mut t, &e);
    let (min, max) = (-1.6e308, 1.6e308);
    let got: Vec<String> = (0..3)
        .map(|_| {
            g17(as_float(&method(
                &mut t,
                &r,
                "getFloat",
                vec![f(min), f(max)],
            )))
        })
        .collect();
    assert_eq!(
        got,
        [
            "-1.5999999999999998e+308",
            "-1.6e+308",
            "1.5999999999999998e+308"
        ]
    );
}

#[test]
fn randomizer_getfloat_opposite_signs() {
    // getFloat_opposite_signs.phpt
    let mut t = TestVm::new();
    let values: Vec<[u8; 8]> = (0x63..=0x67u8)
        .map(|b| [b, 0xe8, 0x58, 0x79, 0x3e, 0xf6, 0x38, 0x00])
        .collect();
    let refs: Vec<&[u8]> = values.iter().map(|v| v.as_slice()).collect();
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    let expected = [
        "-0.78005908680576086",
        "-0.78005908680576097",
        "-0.78005908680576108",
        "-0.78005908680576119",
        "-0.78005908680576130",
    ];
    for want in expected {
        let v = as_float(&method(&mut t, &r, "getFloat", vec![i(-1), i(1)]));
        assert_eq!(format!("{v:.17}"), want);
    }
}

#[test]
fn randomizer_getfloat_equal_steps() {
    // getFloat_equal_steps.phpt (first run)
    let mut t = TestVm::new();
    let e = new_counting(&mut t, true);
    let r = randomizer(&mut t, &e);
    let closed = boundary(&mut t, "ClosedClosed");
    let mut out = String::new();
    let mut prev: Option<f64> = None;
    for _ in 0..150 {
        let v = as_float(&method(
            &mut t,
            &r,
            "getFloat",
            vec![f(1.99999999999999), f(2.00000000000001), closed.clone()],
        ));
        out += &format!("{v:.17}");
        if let Some(p) = prev {
            out += &format!(" ({})", fmt_signed_g17(v - p));
            if p < v {
                out += "\nRepeat";
            }
        }
        out += "\n";
        prev = Some(v);
    }
    let phpt = phpt!("03_randomizer/methods/getFloat_equal_steps.phpt");
    let start = phpt.find("--EXPECT--\n").unwrap() + "--EXPECT--\n".len();
    let first_run = &phpt[start..];
    let end = first_run.find("==================").unwrap();
    let expected = &first_run[..end];
    let header_end = expected.find('\n').unwrap() + 1;
    assert_eq!(out.trim_end(), expected[header_end..].trim_end());
}

/// `%+.17g`
fn fmt_signed_g17(v: f64) -> String {
    let body = float_fmt::gcvt(v.abs(), 17, b'e');
    format!("{}{body}", if v.is_sign_negative() { '-' } else { '+' })
}

#[test]
fn randomizer_shuffle_array() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        let mut n = 1;
        while n < 800 {
            let input = range(1, n);
            let result = method(&mut t, &r, "shuffleArray", vec![input.clone()]);
            let mut values: Vec<i64> = array(&result).values().map(as_int).collect();
            values.sort_unstable();
            assert_eq!(
                values,
                (1..=n).collect::<Vec<_>>(),
                "{} at {n}",
                engine.class().name
            );
            n += if n < 50 { 1 } else { 677 };
        }
    }
    // string keys are dropped, values keep their order class
    let e = new_obj(&mut t, XOSHIRO, vec![i(5)]);
    let r = randomizer(&mut t, &e);
    let mut assoc = PhpArray::new();
    assoc.insert_str("a", i(1));
    assoc.insert_str("b", i(2));
    let out = method(&mut t, &r, "shuffleArray", vec![Value::array(assoc)]);
    let keys: Vec<ArrayKey> = array(&out).keys().collect();
    assert_eq!(keys, [ArrayKey::Int(0), ArrayKey::Int(1)]);
    // empty arrays pass through
    let out = method(&mut t, &r, "shuffleArray", vec![Value::empty_array()]);
    assert_eq!(array(&out).len(), 0);
}

#[test]
fn randomizer_shuffle_bytes() {
    let mut t = TestVm::new();
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        let mut n = 1;
        while n < 800 {
            let mut input = as_bytes(&call(&mut t, "random_bytes", vec![i(n)]));
            input.sort_unstable();
            let result = method(&mut t, &r, "shuffleBytes", vec![bytes(&input)]);
            let mut out = as_bytes(&result);
            out.sort_unstable();
            assert_eq!(out, input);
            n += if n < 50 { 1 } else { 677 };
        }
    }
    let e = new_obj(&mut t, XOSHIRO, vec![i(5)]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        as_bytes(&method(&mut t, &r, "shuffleBytes", vec![s("x")])),
        b"x"
    );
    assert_eq!(
        as_bytes(&method(&mut t, &r, "shuffleBytes", vec![s("")])),
        b""
    );
}

#[test]
fn randomizer_pick_array_keys() {
    let mut t = TestVm::new();
    let mut list_arr = Vec::new();
    let mut strings = PhpArray::new();
    let mut mixed = PhpArray::new();
    for k in 0..500i64 {
        let key = format!("key{k:03}");
        list_arr.push(i(k));
        strings.insert_str(&key, i(k));
        mixed.insert_str(&key, i(k));
        mixed.insert(ArrayKey::Int(k), s(&key));
    }
    let arrays = [list(list_arr), Value::array(strings), Value::array(mixed)];
    for (engine, _) in all_engines(&mut t) {
        let r = randomizer(&mut t, &engine);
        for n in 1..60 {
            for input in &arrays {
                let result = method(&mut t, &r, "pickArrayKeys", vec![input.clone(), i(n)]);
                let picked: Vec<ArrayKey> = array(&result)
                    .values()
                    .map(|v| match v {
                        Value::Int(k) => ArrayKey::Int(*k),
                        Value::String(k) => ArrayKey::from_php_str(k.clone()),
                        o => panic!("unexpected key {o:?}"),
                    })
                    .collect();
                assert_eq!(picked.len(), n as usize);
                let mut unique = picked.clone();
                unique.sort();
                unique.dedup();
                assert_eq!(unique.len(), picked.len(), "duplicates");
                for k in &picked {
                    assert!(array(input).contains_key(k));
                }
            }
        }
    }
    // pickArrayKeys_error.phpt
    let e = new_obj(&mut t, SECURE, vec![]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "pickArrayKeys",
            vec![s("foo"), i(2)]
        )),
        "TypeError: Random\\Randomizer::pickArrayKeys(): Argument #1 ($array) must be of type array, string given"
    );
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "pickArrayKeys",
            vec![Value::empty_array(), i(0)]
        )),
        "ValueError: Random\\Randomizer::pickArrayKeys(): Argument #1 ($array) must not be empty"
    );
    for n in [0, -1, 10] {
        assert_eq!(
            err(try_method(
                &mut t,
                &r,
                "pickArrayKeys",
                vec![range(1, 3), i(n)]
            )),
            "ValueError: Random\\Randomizer::pickArrayKeys(): Argument #2 ($num) must be between 1 and the number of elements in argument #1 ($array)"
        );
    }
}

#[test]
fn randomizer_construct_twice_and_readonly() {
    // construct_twice.phpt / readonly.phpt
    let mut t = TestVm::new();
    let r = new_obj(&mut t, RANDOMIZER, vec![]);
    let msg = "Error: Cannot modify readonly property Random\\Randomizer::$engine";
    assert_eq!(err(try_method(&mut t, &r, "__construct", vec![])), msg);
    let x = new_obj(&mut t, XOSHIRO, vec![]);
    let p = new_obj(&mut t, PCG, vec![]);
    let r = randomizer(&mut t, &x);
    assert_eq!(
        err(try_method(&mut t, &r, "__construct", vec![obj(&p)])),
        msg
    );
    let user = new_user_sha_like(&mut t);
    assert_eq!(
        err(try_method(&mut t, &r, "__construct", vec![obj(&user)])),
        msg
    );
    match r.get_prop(b"engine") {
        Some(Value::Object(e)) => assert_eq!(e.class().name.to_string(), XOSHIRO),
        other => panic!("engine property: {other:?}"),
    }
    // the default engine is Random\Engine\Secure
    let r = new_obj(&mut t, RANDOMIZER, vec![]);
    match r.get_prop(b"engine") {
        Some(Value::Object(e)) => assert_eq!(e.class().name.to_string(), SECURE),
        other => panic!("engine property: {other:?}"),
    }
    // the property is declared public readonly
    let ce = class(&t, RANDOMIZER);
    let info = ce.find_property(b"engine").expect("engine property");
    assert!(info.is_readonly());
    assert_eq!(info.ty.as_ref().unwrap().to_type_string(), "Random\\Engine");
    // a non-engine argument is rejected
    assert_eq!(
        err(try_new_obj(&mut t, RANDOMIZER, vec![s("x")]).map(Value::Object)),
        "TypeError: Random\\Randomizer::__construct(): Argument #1 ($engine) must be of type ?Random\\Engine, string given"
    );
}

#[test]
fn randomizer_state_is_shared_with_the_engine() {
    // two Randomizers (or the engine itself) advance the same generator
    let mut t = TestVm::new();
    let e = new_obj(&mut t, PCG, vec![i(7)]);
    let reference = clone_engine(&mut t, &e);
    let r1 = randomizer(&mut t, &e);
    let r2 = randomizer(&mut t, &e);
    let a = as_int(&method(&mut t, &r1, "nextInt", vec![]));
    let b = as_int(&method(&mut t, &r2, "nextInt", vec![]));
    let c = method(&mut t, &e, "generate", vec![]);
    let want: Vec<Vec<u8>> = (0..3)
        .map(|_| as_bytes(&method(&mut t, &reference, "generate", vec![])))
        .collect();
    let to_int = |b: &[u8]| (u64::from_le_bytes(b.try_into().unwrap()) >> 1) as i64;
    assert_eq!(a, to_int(&want[0]));
    assert_eq!(b, to_int(&want[1]));
    assert_eq!(as_bytes(&c), want[2]);
}

#[test]
fn randomizer_serialization() {
    // serialize.phpt
    let mut t = TestVm::new();
    let sha = new_user_sha_like(&mut t);
    let engines = [
        new_obj(&mut t, MT, vec![i(1234), i(0)]),
        new_obj(&mut t, MT, vec![i(1234), i(1)]),
        new_obj(&mut t, PCG, vec![i(1234)]),
        new_obj(&mut t, XOSHIRO, vec![i(1234)]),
        sha,
    ];
    t.output();
    for engine in engines {
        let r = randomizer(&mut t, &engine);
        for k in 0..2000 {
            method(&mut t, &r, "getInt", vec![i(0), i(k)]);
        }
        // __serialize() holds the property table; engines serialize themselves
        let data = array(&method(&mut t, &r, "__serialize", vec![]));
        assert_eq!(data.len(), 1);
        let props = array(data.get_int(0).unwrap());
        assert_eq!(props.len(), 1);
        let engine_copy = match props.get_str(b"engine") {
            Some(Value::Object(e)) => e.clone(),
            other => panic!("engine: {other:?}"),
        };
        assert!(ObjRef::ptr_eq(&engine_copy, &engine));
        // unserialize into a Randomizer around a copy of the engine
        let copy = if engine.class().is_internal() {
            clone_engine(&mut t, &engine)
        } else {
            engine.clone()
        };
        let mut members = PhpArray::new();
        members.insert_str("engine", obj(&copy));
        let r2 = {
            let ce = class(&t, RANDOMIZER);
            t.vm.instantiate(&ce).unwrap()
        };
        method(
            &mut t,
            &r2,
            "__unserialize",
            vec![list(vec![Value::array(members)])],
        );
        if engine.class().is_internal() {
            for k in 0..2000 {
                let a = method(&mut t, &r, "getInt", vec![i(0), i(k)]);
                let b = method(&mut t, &r2, "getInt", vec![i(0), i(k)]);
                assert_eq!(as_int(&a), as_int(&b), "{} at {k}", engine.class().name);
            }
        }
    }
}

#[test]
fn randomizer_unserialize_errors() {
    // gh_9186_unserialize.phpt, gh_9708_unserialize.phpt, gh_19765_unserialize.phpt
    let mut t = TestVm::new();
    let msg = "Exception: Invalid serialization data for Random\\Randomizer object";
    let fresh = |t: &mut TestVm| {
        let ce = class(t, RANDOMIZER);
        t.vm.instantiate(&ce).unwrap()
    };
    let engine = new_obj(&mut t, XOSHIRO, vec![i(1)]);

    // GH-9186: dynamic properties
    let mut members = PhpArray::new();
    members.insert_str("foo", Value::Null);
    members.insert_str("engine", obj(&engine));
    let r = fresh(&mut t);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "__unserialize",
            vec![list(vec![Value::array(members)])]
        )),
        msg
    );
    // GH-9708: typed property check
    let mut members = PhpArray::new();
    members.insert_str("engine", Value::Null);
    let r = fresh(&mut t);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "__unserialize",
            vec![list(vec![Value::array(members)])]
        )),
        msg
    );
    // GH-19765: readonly property
    let mt = new_obj(&mut t, MT, vec![]);
    let r = randomizer(&mut t, &mt);
    let pcg = new_obj(&mut t, PCG, vec![]);
    let mut members = PhpArray::new();
    members.insert_str("engine", obj(&pcg));
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "__unserialize",
            vec![list(vec![Value::array(members)])]
        )),
        msg
    );
    match r.get_prop(b"engine") {
        Some(Value::Object(e)) => assert_eq!(e.class().name.to_string(), MT),
        other => panic!("{other:?}"),
    }
    // wrong shapes
    let r = fresh(&mut t);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "__unserialize",
            vec![Value::empty_array()]
        )),
        msg
    );
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "__unserialize",
            vec![list(vec![s("x")])]
        )),
        msg
    );
    // the original error is chained as `previous`
    let mut members = PhpArray::new();
    members.insert_str("engine", Value::Null);
    match try_method(
        &mut t,
        &r,
        "__unserialize",
        vec![list(vec![Value::array(members)])],
    ) {
        Err(php_runtime::Flow::Exception(ex)) => {
            let prev = ex.get_prop(b"previous").expect("previous");
            let prev = prev.as_object().expect("object").clone();
            assert_eq!(
                Vm::exception_message(&prev),
                "Cannot assign null to property Random\\Randomizer::$engine of type Random\\Engine"
            );
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// engines that misbehave
// ---------------------------------------------------------------------------

#[test]
fn engine_unsafe_empty_string_and_throws() {
    // engine_unsafe_empty_string.phpt / engine_unsafe_throws.phpt
    let mut t = TestVm::new();
    let e = new_scripted(&mut t, &[b""]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(&mut t, &r, "getInt", vec![i(1), i(10)])),
        "Random\\BrokenRandomEngineError: A random engine must return a non-empty string"
    );
    let e = new_scripted(&mut t, &[]);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(&mut t, &r, "nextInt", vec![])),
        "Exception: Unhandled"
    );
    assert_eq!(
        err(try_method(&mut t, &r, "getBytes", vec![i(10)])),
        "Exception: Unhandled"
    );
    assert_eq!(
        err(try_method(&mut t, &r, "shuffleBytes", vec![s("abc")])),
        "Exception: Unhandled"
    );
}

#[test]
fn engine_unsafe_biased_gives_up() {
    // engine_unsafe_biased.phpt: an engine that always returns the maximum value
    // can never produce an acceptable number.
    let mut t = TestVm::new();
    let values: Vec<Vec<u8>> = (0..200).map(|_| vec![0xff; 8]).collect();
    let refs: Vec<&[u8]> = values.iter().map(Vec::as_slice).collect();
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    let msg = "Random\\BrokenRandomEngineError: Failed to generate an acceptable random number in 50 attempts";
    assert_eq!(err(try_method(&mut t, &r, "getInt", vec![i(1), i(3)])), msg);
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "getInt",
            vec![i(0), i(i64::MAX - 2)]
        )),
        msg
    );
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "pickArrayKeys",
            vec![range(1, 100), i(1)]
        )),
        msg
    );
    // getBytesFromString: 50 consecutive rejected offsets
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    assert_eq!(
        err(try_method(
            &mut t,
            &r,
            "getBytesFromString",
            vec![s("abc"), i(10)]
        )),
        msg
    );
}

#[test]
fn engine_unsafe_nul_bytes() {
    // engine_unsafe_nul.phpt: a zero engine is accepted
    let mut t = TestVm::new();
    let values: Vec<Vec<u8>> = (0..20).map(|_| vec![0; 8]).collect();
    let refs: Vec<&[u8]> = values.iter().map(Vec::as_slice).collect();
    let e = new_scripted(&mut t, &refs);
    let r = randomizer(&mut t, &e);
    assert_eq!(as_int(&method(&mut t, &r, "getInt", vec![i(5), i(10)])), 5);
    assert_eq!(as_int(&method(&mut t, &r, "nextInt", vec![])), 0);
    assert_eq!(as_float(&method(&mut t, &r, "nextFloat", vec![])), 0.0);
}

// ---------------------------------------------------------------------------
// shuffle() / str_shuffle()
// ---------------------------------------------------------------------------

#[test]
fn str_shuffle_is_a_permutation() {
    let mut t = TestVm::new();
    let r = call(&mut t, "str_shuffle", vec![s("123")]);
    let mut out = as_bytes(&r);
    out.sort_unstable();
    assert_eq!(out, b"123");
    assert_eq!(as_bytes(&call(&mut t, "str_shuffle", vec![s("")])), b"");
    assert_eq!(as_bytes(&call(&mut t, "str_shuffle", vec![s("a")])), b"a");
    // seeded sequences are reproducible and match Randomizer::shuffleBytes
    call(&mut t, "mt_srand", vec![i(1234)]);
    let a = call(&mut t, "str_shuffle", vec![s("abcdefghijklmnopqrstuvwxyz")]);
    call(&mut t, "mt_srand", vec![i(1234)]);
    let b = call(&mut t, "str_shuffle", vec![s("abcdefghijklmnopqrstuvwxyz")]);
    assert_eq!(as_bytes(&a), as_bytes(&b));
    let e = new_obj(&mut t, MT, vec![i(1234)]);
    let r = randomizer(&mut t, &e);
    let c = method(
        &mut t,
        &r,
        "shuffleBytes",
        vec![s("abcdefghijklmnopqrstuvwxyz")],
    );
    assert_eq!(as_bytes(&a), as_bytes(&c));
}

#[test]
fn shuffle_function() {
    let mut t = TestVm::new();
    let input = range(1, 20);
    let r = PhpRef::new(input.clone());
    let ret = call(&mut t, "shuffle", vec![Value::Ref(r.clone())]);
    assert!(matches!(ret, Value::Bool(true)));
    let shuffled = array(&r.get());
    let mut values: Vec<i64> = shuffled.values().map(as_int).collect();
    assert_ne!(
        values,
        (1..=20).collect::<Vec<_>>(),
        "20 elements staying in order is not plausible"
    );
    values.sort_unstable();
    assert_eq!(values, (1..=20).collect::<Vec<_>>());
    assert!(shuffled.is_list());
    // seeded: same as Randomizer::shuffleArray on a fresh Mt19937 with that seed
    call(&mut t, "mt_srand", vec![i(99)]);
    let r = PhpRef::new(input.clone());
    call(&mut t, "shuffle", vec![Value::Ref(r.clone())]);
    let e = new_obj(&mut t, MT, vec![i(99)]);
    let rz = randomizer(&mut t, &e);
    let expected = method(&mut t, &rz, "shuffleArray", vec![input]);
    let got: Vec<i64> = array(&r.get()).values().map(as_int).collect();
    let want: Vec<i64> = array(&expected).values().map(as_int).collect();
    assert_eq!(got, want);
    // keys are renumbered, an empty array stays empty
    let mut assoc = PhpArray::new();
    assoc.insert_str("x", i(1));
    let r = PhpRef::new(Value::array(assoc));
    call(&mut t, "shuffle", vec![Value::Ref(r.clone())]);
    assert_eq!(
        array(&r.get()).keys().collect::<Vec<_>>(),
        [ArrayKey::Int(0)]
    );
    let r = PhpRef::new(Value::empty_array());
    call(&mut t, "shuffle", vec![Value::Ref(r.clone())]);
    assert_eq!(array(&r.get()).len(), 0);
}

#[test]
fn enum_and_class_shapes() {
    let t = TestVm::new();
    let ce = class(&t, "Random\\IntervalBoundary");
    assert!(ce.is_enum());
    let names: Vec<String> = ce.constants.values().map(|c| c.name.to_string()).collect();
    assert_eq!(
        names,
        ["ClosedOpen", "ClosedClosed", "OpenClosed", "OpenOpen"]
    );
    assert!(class(&t, "Random\\BrokenRandomEngineError").instance_of_name(b"Random\\RandomError"));
    assert!(class(&t, "Random\\RandomError").instance_of_name(b"Error"));
    assert!(class(&t, "Random\\RandomException").instance_of_name(b"Exception"));
    assert!(class(&t, "Random\\CryptoSafeEngine").is_interface());
    assert!(class(&t, "Random\\Engine").is_interface());
    for name in [MT, PCG, XOSHIRO, SECURE, RANDOMIZER] {
        assert!(class(&t, name).has_flag(class_flags::FINAL), "{name}");
        assert!(class(&t, name).is_internal(), "{name}");
    }
    // the deprecated constant and the global module registry
    assert!(t.vm.modules.contains(&"random"));
}
