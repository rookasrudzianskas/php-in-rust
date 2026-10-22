//! Unit tests ported from php-src `ext/standard/tests/math/*.phpt`.
//!
//! Tables that are too large to transcribe are compared against the
//! `--EXPECT--` section of the `.phpt` file itself (`phpt_expect!`).

use php_runtime::value::float_fmt;
use php_runtime::{PResult, Value};

use crate::testing::{TestVm, thrown};

/// The `--EXPECT--`/`--EXPECTF--` section of a phpt file below
/// `tests/phpt/ext/standard/tests/math/`, with trailing whitespace trimmed.
macro_rules! phpt_expect {
    ($name:literal) => {{
        let src = include_str!(concat!(
            "../../../../../tests/phpt/ext/standard/tests/math/",
            $name
        ));
        let start = src
            .find("--EXPECT--\n")
            .map(|i| i + "--EXPECT--\n".len())
            .or_else(|| src.find("--EXPECTF--\n").map(|i| i + "--EXPECTF--\n".len()))
            .expect("phpt has an expectation section");
        src[start..].trim_end().to_string()
    }};
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn f(d: f64) -> Value {
    Value::Float(d)
}

fn s(x: &str) -> Value {
    Value::str(x)
}

/// `var_dump()` of a scalar.
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

/// String interpolation of a scalar (`precision=14`).
fn interp(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Float(d) => float_fmt::double_to_string(*d, 14),
        Value::String(x) => x.to_string_lossy().into_owned(),
        Value::Bool(true) => "1".to_string(),
        Value::Bool(false) => String::new(),
        other => format!("{other:?}"),
    }
}

fn call(t: &mut TestVm, name: &str, args: Vec<Value>) -> Value {
    match t.call(name, args) {
        Ok(v) => v,
        Err(e) => panic!("{name}() threw {e:?}"),
    }
}

fn err(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let (class, msg) = thrown(t.call(name, args));
    format!("{class}: {msg}")
}

fn float_of(v: &Value) -> f64 {
    v.as_float()
        .unwrap_or_else(|| panic!("expected float, got {v:?}"))
}

/// `Deprecated: ... in /t.php on line 3` as displayed by the TestVm.
fn deprecated(msg: &str) -> String {
    format!("\nDeprecated: {msg} in /t.php on line 3\n")
}

fn notice(msg: &str) -> String {
    format!("\nNotice: {msg} in /t.php on line 3\n")
}

const INVALID_CHARS: &str =
    "Invalid characters passed for attempted conversion, these have been ignored";
const PRECISION_LOST: &str =
    "Input number is larger than PHP_INT_MAX, precision has been lost in conversion";

// ---------------------------------------------------------------------------
// abs, ceil, floor
// ---------------------------------------------------------------------------

#[test]
fn abs_values() {
    let mut t = TestVm::new();
    assert!(matches!(call(&mut t, "abs", vec![i(-1)]), Value::Int(1)));
    assert!(matches!(call(&mut t, "abs", vec![f(-1.5)]), Value::Float(x) if x == 1.5));
    assert!(matches!(call(&mut t, "abs", vec![s("-1")]), Value::Int(1)));
    assert!(matches!(call(&mut t, "abs", vec![s("-1.5")]), Value::Float(x) if x == 1.5));
    // abs(PHP_INT_MIN) does not fit an int
    assert_eq!(
        dump(&call(&mut t, "abs", vec![i(i64::MIN)])),
        "float(9.223372036854776E+18)"
    );
    assert_eq!(
        dump(&call(&mut t, "abs", vec![i(i64::MIN + 1)])),
        "int(9223372036854775807)"
    );
    assert_eq!(
        dump(&call(&mut t, "abs", vec![i(i64::MAX)])),
        "int(9223372036854775807)"
    );
    assert_eq!(
        dump(&call(&mut t, "abs", vec![i(-2147483648)])),
        "int(2147483648)"
    );
    // float overflow of PHP_INT_MIN - 1
    assert_eq!(
        dump(&call(&mut t, "abs", vec![f(i64::MIN as f64 - 1.0)])),
        "float(9.223372036854776E+18)"
    );
}

#[test]
fn ceil_floor() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "ceil", vec![f(-1.5)])), "float(-1)");
    assert_eq!(dump(&call(&mut t, "ceil", vec![f(1.5)])), "float(2)");
    assert_eq!(dump(&call(&mut t, "floor", vec![f(-1.5)])), "float(-2)");
    assert_eq!(dump(&call(&mut t, "floor", vec![f(1.5)])), "float(1)");
    // integers are returned as floats
    assert_eq!(dump(&call(&mut t, "ceil", vec![i(5)])), "float(5)");
    assert_eq!(
        dump(&call(&mut t, "floor", vec![i(i64::MAX)])),
        "float(9.223372036854776E+18)"
    );
    assert_eq!(dump(&call(&mut t, "ceil", vec![s("4.2")])), "float(5)");
    // negative zero survives ceil(-0.5)
    let r = float_of(&call(&mut t, "ceil", vec![f(-0.5)]));
    assert!(r == 0.0 && r.is_sign_negative());
    assert_eq!(
        err(&mut t, "ceil", vec![s("abc")]),
        "TypeError: ceil(): Argument #1 ($num) must be of type int|float, string given"
    );
}

// ---------------------------------------------------------------------------
// round
// ---------------------------------------------------------------------------

fn case(t: &mut TestVm, class: &str, name: &str) -> Value {
    t.vm.class_constant(class, name)
        .expect("lookup")
        .unwrap_or_else(|| panic!("{class}::{name} exists"))
}

#[test]
fn round_modes_table() {
    // round_modes.phpt
    let mut t = TestVm::new();
    let mut modes: Vec<(&str, Value)> = vec![
        ("PHP_ROUND_HALF_UP", i(1)),
        ("PHP_ROUND_HALF_DOWN", i(2)),
        ("PHP_ROUND_HALF_EVEN", i(3)),
        ("PHP_ROUND_HALF_ODD", i(4)),
    ];
    for name in [
        "PositiveInfinity",
        "NegativeInfinity",
        "TowardsZero",
        "AwayFromZero",
    ] {
        let c = case(&mut t, "RoundingMode", name);
        modes.push((name, c));
    }
    let numbers = [
        f(2.5),
        f(-2.5),
        f(3.5),
        f(-3.5),
        i(7),
        i(-7),
        f(15.51),
        f(-15.51),
        i(5),
        i(-5),
        f(0.61),
        f(0.69),
        f(0.65),
        f(-0.65),
        f(1.9999),
        f(-1.9999),
        f(0.0001),
        f(-0.0001),
    ];
    let mut out = String::new();
    for (key, mode) in &modes {
        out += &format!("mode: {key}\n");
        for precision in [-1i64, 0, 1, 2, 10] {
            out += &format!("\tprecision: {precision}\n");
            for number in &numbers {
                let result = call(
                    &mut t,
                    "round",
                    vec![number.clone(), i(precision), mode.clone()],
                );
                out += &format!("\t\t{:>7} => {}\n", interp(number), interp(&result));
            }
            out += "\n";
        }
    }
    assert_eq!(out.trim_end(), phpt_expect!("round_modes.phpt"));
}

#[test]
fn round_basic_with_coercions() {
    // round_basic.phpt: precision is coerced by ZPP (with deprecations)
    let mut t = TestVm::new();
    let values = [
        i(123456789),
        f(123.456789),
        f(-4.5679123),
        f(1.23E4),
        f(-4.567E3),
        i(0x234567),
        i(0o67777777),
        s("1.234567"),
        s("2.3456789e8"),
    ];
    let precisions = [
        i(2),
        i(8),
        i(3),
        i(4),
        f(3.6),
        s("2"),
        s("04"),
        s("3.6"),
        s("2.1e1"),
        Value::Bool(true),
        Value::Bool(false),
    ];
    let mut out = String::from("*** Testing round() : basic functionality ***\n");
    for v in &values {
        out += &format!("round: {}\n", interp(v));
        for p in &precisions {
            let res = call(&mut t, "round", vec![v.clone(), p.clone()]);
            out += &t.output();
            out += &format!("...with precision {}-> {}\n", interp(p), dump(&res));
        }
    }
    let expected =
        phpt_expect!("round_basic.phpt").replace(" in %s on line %d", " in /t.php on line 3");
    assert_eq!(out.trim_end(), expected);
}

#[test]
fn round_rounding_mode_enum() {
    let mut t = TestVm::new();
    let list =
        t.vm.call(&Value::str("RoundingMode::cases"), vec![])
            .expect("cases()");
    let list = list.as_array().expect("array").clone();
    assert_eq!(list.len(), 8);
    let mut out = String::new();
    for mode in list.values() {
        let name =
            crate::random::enums::case_name(mode.as_object().expect("object")).expect("name");
        for number in [1.0, -1.0, 1.2, -1.2, 1.7, -1.7, 1.5, -1.5, 2.5, -2.5] {
            let r = float_of(&call(&mut t, "round", vec![f(number), i(0), mode.clone()]));
            out += &format!("{name:<20}: {} -> {}\n", fmt_g17(number), fmt_g17(r));
        }
    }
    assert_eq!(out.trim_end(), phpt_expect!("round_RoundingMode.phpt"));
}

/// `%+.17g`
fn fmt_g17(d: f64) -> String {
    let body = float_fmt::gcvt(d.abs(), 17, b'e');
    let sign = if d.is_sign_negative() { '-' } else { '+' };
    format!("{sign}{body}")
}

fn round_g17(t: &mut TestVm, number: f64, mode: i64) -> String {
    let r = float_of(&call(t, "round", vec![f(number), i(0), i(mode)]));
    format!("{} -> {}", fmt_g17(number), fmt_g17(r))
}

#[test]
#[allow(clippy::excessive_precision)]
fn round_gh12143() {
    let mut t = TestVm::new();
    for (number, expected) in [
        (0.49999999999999994, "+0.49999999999999994 -> +0"),
        (-0.49999999999999994, "-0.49999999999999994 -> -0"),
        (0.50000000000000011, "+0.50000000000000011 -> +1"),
        (-0.50000000000000011, "-0.50000000000000011 -> -1"),
    ] {
        for mode in 1..=4 {
            assert_eq!(round_g17(&mut t, number, mode), expected, "mode {mode}");
        }
    }
}

#[test]
fn round_prerounding_and_edges() {
    let mut t = TestVm::new();
    // round_prerounding.phpt
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(0.285), i(2), i(1)])),
        "float(0.29)"
    );
    // round_bug71201.phpt
    assert_eq!(
        interp(&call(&mut t, "round", vec![f(1.0), i(-2147483648)])),
        "0"
    );
    // gh22410: integral doubles are preserved
    for v in [1.0, -1.0, 9003241321073828.0, -9003241321073828.0] {
        assert_eq!(float_of(&call(&mut t, "round", vec![f(v)])), v);
    }
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(9003241321073828.0)])),
        "float(9003241321073828)"
    );
    // round_large_exp.phpt
    for (v, p) in [
        (2e-22, 22),
        (1e-22, 22),
        (2e-23, 23),
        (1e-23, 23),
        (2e-24, 24),
        (1e-24, 24),
        (2e22, -22),
        (1e22, -22),
        (2e23, -23),
        (1e23, -23),
        (2e24, -24),
        (1e24, -24),
    ] {
        assert_eq!(
            float_of(&call(&mut t, "round", vec![f(v), i(p), i(1)])),
            v,
            "{v} {p}"
        );
    }
}

#[test]
fn round_invalid_mode() {
    let mut t = TestVm::new();
    assert_eq!(
        err(&mut t, "round", vec![f(1.5), i(0), i(1234)]),
        "ValueError: round(): Argument #3 ($mode) must be a valid rounding mode (RoundingMode::*)"
    );
    let r =
        t.vm.call_native_named("round", vec![f(1.5)], vec![("mode", i(1234))]);
    assert_eq!(
        thrown(r).1,
        "round(): Argument #3 ($mode) must be a valid rounding mode (RoundingMode::*)"
    );
}

#[test]
fn round_integers_and_places() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "round", vec![i(5)])), "float(5)");
    assert_eq!(
        dump(&call(&mut t, "round", vec![i(1241757), i(-3)])),
        "float(1242000)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(5.055), i(2)])),
        "float(5.06)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(5.045), i(2)])),
        "float(5.05)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(1.95583), i(2)])),
        "float(1.96)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(1241757.0), i(-3)])),
        "float(1242000)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(f64::INFINITY)])),
        "float(INF)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(f64::NAN)])),
        "float(NAN)"
    );
    assert_eq!(dump(&call(&mut t, "round", vec![f(-0.0)])), "float(-0)");
    // half-way modes
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(2.5), i(0), i(3)])),
        "float(2)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(3.5), i(0), i(3)])),
        "float(4)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(2.5), i(0), i(4)])),
        "float(3)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(2.5), i(0), i(2)])),
        "float(2)"
    );
    // ceiling / floor / toward zero / away from zero (modes 5..8)
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(1.2), i(0), i(5)])),
        "float(2)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(-1.2), i(0), i(5)])),
        "float(-1)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(1.2), i(0), i(6)])),
        "float(1)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(-1.2), i(0), i(6)])),
        "float(-2)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(-1.7), i(0), i(7)])),
        "float(-1)"
    );
    assert_eq!(
        dump(&call(&mut t, "round", vec![f(-1.2), i(0), i(8)])),
        "float(-2)"
    );
}

// ---------------------------------------------------------------------------
// trigonometry and friends
// ---------------------------------------------------------------------------

#[test]
fn float_functions() {
    let mut t = TestVm::new();
    let check = |t: &mut TestVm, name: &str, arg: f64, expected: f64| {
        let r = float_of(&call(t, name, vec![f(arg)]));
        assert!(
            (r - expected).abs() < 1e-10 || (r.is_nan() && expected.is_nan()),
            "{name}({arg}) = {r}, expected {expected}"
        );
    };
    check(&mut t, "sin", std::f64::consts::FRAC_PI_2, 1.0);
    check(&mut t, "cos", 0.0, 1.0);
    check(&mut t, "tan", std::f64::consts::FRAC_PI_4, 1.0);
    check(&mut t, "asin", 1.0, std::f64::consts::FRAC_PI_2);
    check(&mut t, "acos", 2.0, f64::NAN);
    check(&mut t, "atan", 1.0, std::f64::consts::FRAC_PI_4);
    check(&mut t, "exp", 1.0, std::f64::consts::E);
    check(&mut t, "expm1", 0.0, 0.0);
    check(&mut t, "log1p", 0.0, 0.0);
    check(&mut t, "log10", 1000.0, 3.0);
    check(&mut t, "sqrt", 16.0, 4.0);
    check(&mut t, "sqrt", -1.0, f64::NAN);
    check(&mut t, "sinh", 0.0, 0.0);
    check(&mut t, "cosh", 0.0, 1.0);
    check(&mut t, "tanh", 0.0, 0.0);
    check(&mut t, "deg2rad", 180.0, std::f64::consts::PI);
    check(&mut t, "rad2deg", std::f64::consts::PI, 180.0);
    assert_eq!(
        dump(&call(&mut t, "pi", vec![])),
        "float(3.141592653589793)"
    );
    assert_eq!(
        dump(&call(&mut t, "atan2", vec![f(1.0), f(1.0)])),
        "float(0.7853981633974483)"
    );
    assert_eq!(
        dump(&call(&mut t, "hypot", vec![f(3.0), f(4.0)])),
        "float(5)"
    );
    // int arguments are accepted
    assert_eq!(dump(&call(&mut t, "sqrt", vec![i(9)])), "float(3)");
}

#[test]
fn inverse_hyperbolic_values() {
    // asinh_variation / acosh_variation / atanh_variation expectations
    let mut t = TestVm::new();
    assert_eq!(
        dump(&call(&mut t, "acosh", vec![i(23)])),
        "float(3.8281684713331012)"
    );
    assert_eq!(
        dump(&call(&mut t, "acosh", vec![f(23.45)])),
        "float(3.8475627390640357)"
    );
    assert_eq!(
        dump(&call(&mut t, "acosh", vec![i(1000)])),
        "float(7.600902209541989)"
    );
    assert_eq!(dump(&call(&mut t, "acosh", vec![i(1)])), "float(0)");
    assert_eq!(dump(&call(&mut t, "acosh", vec![i(0)])), "float(NAN)");
    assert_eq!(dump(&call(&mut t, "acosh", vec![i(-23)])), "float(NAN)");
}

#[test]
fn finite_checks() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "is_finite", vec![f(1.0)])), "bool(true)");
    assert_eq!(
        dump(&call(&mut t, "is_finite", vec![f(f64::INFINITY)])),
        "bool(false)"
    );
    assert_eq!(
        dump(&call(&mut t, "is_finite", vec![f(f64::NAN)])),
        "bool(false)"
    );
    assert_eq!(
        dump(&call(&mut t, "is_infinite", vec![f(f64::NEG_INFINITY)])),
        "bool(true)"
    );
    assert_eq!(
        dump(&call(&mut t, "is_infinite", vec![i(5)])),
        "bool(false)"
    );
    assert_eq!(
        dump(&call(&mut t, "is_nan", vec![f(f64::NAN)])),
        "bool(true)"
    );
    assert_eq!(dump(&call(&mut t, "is_nan", vec![f(0.0)])), "bool(false)");
}

#[test]
fn log_function() {
    // log.phpt / log_error.phpt
    let mut t = TestVm::new();
    assert_eq!(
        dump(&call(&mut t, "log", vec![f(std::f64::consts::E)])),
        "float(1)"
    );
    assert_eq!(dump(&call(&mut t, "log", vec![i(8), i(2)])), "float(3)");
    assert_eq!(dump(&call(&mut t, "log", vec![i(100), i(10)])), "float(2)");
    assert_eq!(dump(&call(&mut t, "log", vec![i(5), i(1)])), "float(NAN)");
    assert_eq!(dump(&call(&mut t, "log", vec![i(0)])), "float(-INF)");
    assert_eq!(dump(&call(&mut t, "log", vec![i(-1)])), "float(NAN)");
    let r = float_of(&call(&mut t, "log", vec![i(9), i(3)]));
    assert!((r - 2.0).abs() < 1e-12);
    assert_eq!(
        err(&mut t, "log", vec![i(10), i(0)]),
        "ValueError: log(): Argument #2 ($base) must be greater than 0"
    );
    assert_eq!(
        err(&mut t, "log", vec![i(10), i(-5)]),
        "ValueError: log(): Argument #2 ($base) must be greater than 0"
    );
    // named argument counts as two arguments
    let r =
        t.vm.call_native_named("log", vec![i(8)], vec![("base", i(2))])
            .expect("log");
    assert_eq!(dump(&r), "float(3)");
}

#[test]
fn integer_helpers() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "intdiv", vec![i(7), i(2)])), "int(3)");
    assert_eq!(dump(&call(&mut t, "intdiv", vec![i(-7), i(2)])), "int(-3)");
    assert_eq!(
        err(&mut t, "intdiv", vec![i(1), i(0)]),
        "DivisionByZeroError: Division by zero"
    );
    assert_eq!(
        err(&mut t, "intdiv", vec![i(i64::MIN), i(-1)]),
        "ArithmeticError: Division of PHP_INT_MIN by -1 is not an integer"
    );
    assert_eq!(
        dump(&call(&mut t, "fdiv", vec![f(1.0), f(0.0)])),
        "float(INF)"
    );
    assert_eq!(
        dump(&call(&mut t, "fdiv", vec![f(-1.0), f(0.0)])),
        "float(-INF)"
    );
    assert_eq!(
        dump(&call(&mut t, "fdiv", vec![f(0.0), f(0.0)])),
        "float(NAN)"
    );
    assert_eq!(
        dump(&call(&mut t, "fmod", vec![f(5.5), f(2.0)])),
        "float(1.5)"
    );
    assert_eq!(
        dump(&call(&mut t, "fmod", vec![f(-5.5), f(2.0)])),
        "float(-1.5)"
    );
    assert_eq!(
        dump(&call(&mut t, "fmod", vec![f(5.0), f(0.0)])),
        "float(NAN)"
    );
    assert_eq!(
        dump(&call(&mut t, "fpow", vec![i(2), i(10)])),
        "float(1024)"
    );
    assert_eq!(dump(&call(&mut t, "fpow", vec![i(0), i(-1)])), "float(INF)");
    assert_eq!(dump(&call(&mut t, "pow", vec![i(2), i(10)])), "int(1024)");
    assert_eq!(dump(&call(&mut t, "pow", vec![i(2), i(-1)])), "float(0.5)");
    assert_eq!(
        dump(&call(&mut t, "pow", vec![i(2), i(64)])),
        "float(1.8446744073709552E+19)"
    );
    assert_eq!(dump(&call(&mut t, "pow", vec![s("3"), i(2)])), "int(9)");
}

// ---------------------------------------------------------------------------
// clamp
// ---------------------------------------------------------------------------

#[test]
fn clamp_values() {
    let mut t = TestVm::new();
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(2), i(1), i(3)])),
        "int(2)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(0), i(1), i(3)])),
        "int(1)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(6), i(1), i(3)])),
        "int(3)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(2), f(1.3), f(3.4)])),
        "int(2)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![f(2.5), i(1), i(3)])),
        "float(2.5)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(0), f(1.3), f(3.4)])),
        "float(1.3)"
    );
    assert_eq!(
        dump(&call(
            &mut t,
            "clamp",
            vec![
                f(std::f64::consts::PI),
                f(f64::NEG_INFINITY),
                f(f64::INFINITY)
            ]
        )),
        "float(3.141592653589793)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![f(f64::NAN), i(4), i(6)])),
        "float(NAN)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![s("a"), s("c"), s("g")])),
        "string(1) \"c\""
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![s("d"), s("c"), s("g")])),
        "string(1) \"d\""
    );
    assert_eq!(
        dump(&call(
            &mut t,
            "clamp",
            vec![s("2025-08-01"), s("2025-08-15"), s("2025-09-15")]
        )),
        "string(10) \"2025-08-15\""
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![Value::Null, i(-1), i(1)])),
        "int(-1)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![Value::Null, i(1), i(3)])),
        "int(1)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![Value::Null, i(-3), i(-1)])),
        "int(-3)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(-9999), Value::Null, i(10)])),
        "int(-9999)"
    );
    assert_eq!(
        dump(&call(&mut t, "clamp", vec![i(12), Value::Null, i(10)])),
        "int(10)"
    );
}

#[test]
fn clamp_errors() {
    let mut t = TestVm::new();
    assert_eq!(
        err(&mut t, "clamp", vec![i(4), f(f64::NAN), i(6)]),
        "ValueError: clamp(): Argument #2 ($min) must not be NAN"
    );
    assert_eq!(
        err(&mut t, "clamp", vec![i(7), i(6), f(f64::NAN)]),
        "ValueError: clamp(): Argument #3 ($max) must not be NAN"
    );
    for args in [
        vec![i(1), i(3), i(2)],
        vec![i(-9999), i(5), Value::Null],
        vec![i(12), i(-5), Value::Null],
    ] {
        assert_eq!(
            err(&mut t, "clamp", args),
            "ValueError: clamp(): Argument #2 ($min) must be smaller than or equal to argument #3 ($max)"
        );
    }
}

// ---------------------------------------------------------------------------
// base conversion
// ---------------------------------------------------------------------------

#[test]
fn bindec_values_64bit() {
    // bindec_basic_64bit.phpt
    let mut t = TestVm::new();
    let cases: Vec<(Value, &str, bool)> = vec![
        (i(111000111), "int(455)", false),
        (i(0o11100000), "int(0)", true),
        (f(1111111111111111111111111111111.0), "int(32766)", true),
        (s("111000111"), "int(455)", false),
        (s("011100000"), "int(224)", false),
        (
            s("1111111111111111111111111111111"),
            "int(2147483647)",
            false,
        ),
        (
            s("10000000000000000000000000000000"),
            "int(2147483648)",
            false,
        ),
        (f(10000000000000000000000000000000.0), "int(5)", true),
        (s("100002001"), "int(129)", true),
        (s("abcdefg"), "int(0)", true),
        (i(311015), "int(13)", true),
        (f(31101.3), "int(13)", true),
        (f(31.1013e5), "int(26)", true),
        (i(0x111ABC), "int(6)", true),
        (i(0o11237), "int(0)", true),
        (Value::Bool(true), "int(1)", false),
        (Value::Bool(false), "int(0)", false),
    ];
    for (arg, expected, deprecation) in cases {
        let r = call(&mut t, "bindec", vec![arg.clone()]);
        assert_eq!(dump(&r), expected, "bindec({arg:?})");
        let out = t.output();
        if deprecation {
            assert_eq!(out, deprecated(INVALID_CHARS), "bindec({arg:?})");
        } else {
            assert_eq!(out, "", "bindec({arg:?})");
        }
    }
    // empty strings have no digits, but no invalid characters either... except
    // that nothing is invalid in an empty string
    assert_eq!(dump(&call(&mut t, "bindec", vec![s("")])), "int(0)");
    assert_eq!(t.output(), "");
    assert_eq!(
        err(&mut t, "bindec", vec![Value::empty_array()]),
        "TypeError: bindec(): Argument #1 ($binary_string) must be of type string, array given"
    );
}

#[test]
fn hexdec_overflow() {
    // hexdec.phpt
    let mut t = TestVm::new();
    assert_eq!(
        dump(&call(&mut t, "hexdec", vec![s("012345")])),
        "int(74565)"
    );
    assert_eq!(
        dump(&call(&mut t, "hexdec", vec![s("12345")])),
        "int(74565)"
    );
    assert_eq!(t.output(), "");
    for input in ["q12345", "12345+?!", "12345q"] {
        assert_eq!(dump(&call(&mut t, "hexdec", vec![s(input)])), "int(74565)");
        assert_eq!(t.output(), deprecated(INVALID_CHARS));
    }
    assert_eq!(
        dump(&call(&mut t, "hexdec", vec![s("12345678901234567")])),
        "float(2.0988295476557332E+19)"
    );
    assert_eq!(t.output(), notice(PRECISION_LOST));
    assert_eq!(
        dump(&call(&mut t, "hexdec", vec![s("17fffffffffffffff")])),
        "float(2.7670116110564327E+19)"
    );
    assert_eq!(t.output(), notice(PRECISION_LOST));
}

#[test]
fn octdec_and_prefixes() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "octdec", vec![s("777")])), "int(511)");
    assert_eq!(dump(&call(&mut t, "octdec", vec![s("0o7")])), "int(7)");
    assert_eq!(dump(&call(&mut t, "hexdec", vec![s("0xFf")])), "int(255)");
    assert_eq!(dump(&call(&mut t, "bindec", vec![s("0b1010")])), "int(10)");
    assert_eq!(
        dump(&call(&mut t, "hexdec", vec![s(" 0xFF\t\n")])),
        "int(255)"
    );
    assert_eq!(t.output(), "");
    // a prefix for another base is invalid input ("0" and "7" remain)
    assert_eq!(dump(&call(&mut t, "octdec", vec![s("0x7")])), "int(7)");
    assert_eq!(t.output(), deprecated(INVALID_CHARS));
    assert_eq!(dump(&call(&mut t, "bindec", vec![s("0x7")])), "int(0)");
    assert_eq!(t.output(), deprecated(INVALID_CHARS));
}

#[test]
fn base_convert_improvements() {
    // base_convert_improvements.phpt
    let mut t = TestVm::new();
    for (num, from, to, expected) in [
        ("FF", 16, 10, "255"),
        ("20", 8, 10, "16"),
        ("0xFf", 16, 10, "255"),
        ("0FF", 16, 10, "255"),
        (" 0xFF\t\n", 16, 10, "255"),
        ("\r\nFF\t\n", 16, 10, "255"),
        ("0o7", 8, 10, "7"),
        ("0b1010", 2, 10, "10"),
    ] {
        let r = call(&mut t, "base_convert", vec![s(num), i(from), i(to)]);
        assert_eq!(interp(&r), expected, "base_convert({num:?})");
        assert_eq!(t.output(), "");
    }
    // invalid characters are ignored with a deprecation
    for (num, from, expected) in [
        ("fg", 16, "15"),
        ("f 0xff ", 16, "61695"),
        ("1xff ", 16, "511"),
        ("\0", 16, "0"),
        ("0o7", 9, "7"),
        ("0 0", 9, "0"),
    ] {
        let r = call(&mut t, "base_convert", vec![s(num), i(from), i(10)]);
        assert_eq!(interp(&r), expected, "base_convert({num:?})");
        assert_eq!(t.output(), deprecated(INVALID_CHARS));
    }
}

#[test]
fn base_convert_overflow_and_errors() {
    let mut t = TestVm::new();
    let r = call(
        &mut t,
        "base_convert",
        vec![s("fffffffffffffffff"), i(16), i(15)],
    );
    assert_eq!(dump(&r), "string(18) \"2ee03c32ad644bd1e1\"");
    assert_eq!(t.output(), notice(PRECISION_LOST));
    // gh22395: no 64-character limit
    let r = call(
        &mut t,
        "base_convert",
        vec![s(&"1".repeat(61)), i(36), i(16)],
    );
    let digits = interp(&r);
    assert_eq!(digits.len(), 78);
    assert_eq!(&digits[..13], "4b61b5e0639ff");
    assert_eq!(t.output(), notice(PRECISION_LOST));
    // errors
    assert_eq!(
        err(&mut t, "base_convert", vec![i(1234), i(1), i(10)]),
        "ValueError: base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)"
    );
    assert_eq!(
        err(&mut t, "base_convert", vec![i(1234), i(10), i(37)]),
        "ValueError: base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)"
    );
    // an infinite intermediate value cannot be converted
    let huge = "f".repeat(300);
    assert_eq!(
        err(&mut t, "base_convert", vec![s(&huge), i(16), i(10)]),
        "ValueError: An infinite value cannot be converted to base 10"
    );
    t.output();
}

#[test]
fn dec_to_base() {
    let mut t = TestVm::new();
    assert_eq!(dump(&call(&mut t, "decbin", vec![i(0)])), "string(1) \"0\"");
    assert_eq!(
        dump(&call(&mut t, "decbin", vec![i(10)])),
        "string(4) \"1010\""
    );
    assert_eq!(
        dump(&call(&mut t, "decbin", vec![i(-1)])),
        format!("string(64) \"{}\"", "1".repeat(64))
    );
    assert_eq!(
        dump(&call(&mut t, "decoct", vec![i(8)])),
        "string(2) \"10\""
    );
    assert_eq!(
        dump(&call(&mut t, "decoct", vec![i(i64::MIN)])),
        "string(22) \"1000000000000000000000\""
    );
    assert_eq!(
        dump(&call(&mut t, "dechex", vec![i(255)])),
        "string(2) \"ff\""
    );
    assert_eq!(
        dump(&call(&mut t, "dechex", vec![i(-1)])),
        "string(16) \"ffffffffffffffff\""
    );
    assert_eq!(
        dump(&call(&mut t, "dechex", vec![i(i64::MAX)])),
        "string(16) \"7fffffffffffffff\""
    );
    // base_convert with a long source goes through the generic converter
    assert_eq!(
        interp(&call(&mut t, "base_convert", vec![s("zz"), i(36), i(2)])),
        "10100001111"
    );
}

// ---------------------------------------------------------------------------
// number_format
// ---------------------------------------------------------------------------

fn nf(t: &mut TestVm, args: Vec<Value>) -> String {
    interp(&call(t, "number_format", args))
}

#[test]
fn number_format_basic() {
    // number_format_basic.phpt
    let mut t = TestVm::new();
    let values = [
        f(1234.5678),
        f(-1234.5678),
        f(1234.6578e4),
        f(-1234.56789e4),
        i(999999),
        i(-999999),
        f(999999.0),
        f(-999999.0),
        i(0x1234CDEF),
        i(0o2777777777),
        s("123456789"),
        s("123.456789"),
        s("12.3456789e1"),
        Value::Bool(true),
        Value::Bool(false),
    ];
    let sections: [(&str, Vec<Value>); 5] = [
        ("default", vec![]),
        ("with two dp", vec![i(2)]),
        ("English format", vec![i(2), s("."), s(" ")]),
        ("French format", vec![i(2), s(","), s(" ")]),
        (
            "multichar format",
            vec![i(2), s(" DECIMALS "), s(" THOUSAND ")],
        ),
    ];
    let mut out = String::new();
    for (idx, (title, extra)) in sections.iter().enumerate() {
        let _ = idx;
        out += &format!("\n number_format tests.....{title}\n");
        for v in &values {
            let mut args = vec![v.clone()];
            args.extend(extra.iter().cloned());
            let r = call(&mut t, "number_format", args);
            out += &dump(&r);
            out += "\n";
        }
    }
    assert_eq!(out.trim(), phpt_expect!("number_format_basic.phpt").trim());
}

#[test]
fn number_format_multichar() {
    let mut t = TestVm::new();
    let values = [
        f(1234.5678),
        f(-1234.5678),
        f(1234.6578e4),
        f(-1234.56789e4),
        i(0x1234CDEF),
        i(0o2777777777),
        s("123456789"),
        s("123.456789"),
        s("12.3456789e1"),
        Value::Bool(true),
        Value::Bool(false),
    ];
    let mut out = String::new();
    for (title, dp, ts) in [
        ("multiple character decimal point", "&#183;", " "),
        ("multiple character thousand separator", ".", "&thinsp;"),
        (
            "multiple character decimal and thousep",
            "&#183;",
            "&thinsp;",
        ),
    ] {
        out += &format!("\n number_format tests.....{title}\n");
        for v in &values {
            let r = call(&mut t, "number_format", vec![v.clone(), i(2), s(dp), s(ts)]);
            out += &dump(&r);
            out += "\n";
        }
    }
    assert_eq!(
        out.trim(),
        phpt_expect!("number_format_multichar.phpt").trim()
    );
}

#[test]
fn number_format_decimals_table() {
    // number_format_decimals.phpt
    let mut t = TestVm::new();
    let values = [
        f(1.5151),
        f(15.151),
        f(151.51),
        f(1515.1),
        i(15151),
        f(-1.5151),
        f(-15.151),
        f(-151.51),
        f(-1515.1),
        i(-15151),
        i(999),
        i(-999),
        f(999.0),
        f(-999.0),
        i(999999),
        i(-999999),
        f(999999.0),
        f(-999999.0),
        i(2147483647),
        i(-2147483648),
    ];
    let mut out = String::new();
    for v in &values {
        out += "testing ";
        out += &dump(v);
        out += "\n";
        for decimal in [0i64, 1, 2, 3, 4, 5, -1, -2, -3, -4, -5] {
            let r = call(&mut t, "number_format", vec![v.clone(), i(decimal)]);
            out += &format!("... with decimal places of {decimal}: {}\n", dump(&r));
        }
    }
    assert_eq!(out.trim_end(), phpt_expect!("number_format_decimals.phpt"));
}

#[test]
fn number_format_long_table() {
    // number_format_basiclong_64bit.phpt
    let mut t = TestVm::new();
    let (max64, min64, max32, min32) = (i64::MAX, i64::MIN, 2147483647i64, -2147483648i64);
    let numbers = [
        i(max64),
        i(min64),
        i(max32),
        i(min32),
        i(max64 - max32),
        i(min64 - min32),
        i(max32 + 1),
        i(min32 - 1),
        i(max32 * 2),
        i(max32 * 2 + 1),
        i(max32 * 2 - 1),
        i(max64 - 1),
        f(max64 as f64 + 1.0),
        i(min64 + 1),
        f(min64 as f64 - 1.0),
        f(max64 as f64 - 1024.0),
        f(min64 as f64 + 1024.0),
    ];
    let mut out = String::new();
    for number in &numbers {
        out += "--- testing: ";
        out += &dump(number);
        out += "\n";
        for precision in [5i64, 0, -1, -5, -10, -11, -17, -19, -20] {
            let r = call(&mut t, "number_format", vec![number.clone(), i(precision)]);
            out += &format!("... with precision {precision}: {}\n", dump(&r));
        }
    }
    assert_eq!(
        out.trim_end(),
        phpt_expect!("number_format_basiclong_64bit.phpt")
    );
}

#[test]
fn number_format_misc() {
    let mut t = TestVm::new();
    // negative zero
    assert_eq!(nf(&mut t, vec![f(-1.15E-15), i(2)]), "0.00");
    assert_eq!(nf(&mut t, vec![f(-0.01), i(2)]), "-0.01");
    // null separators select the defaults (number_format_with_null_args.phpt)
    assert_eq!(
        nf(&mut t, vec![f(2020.1415), i(2), Value::Null, s("T")]),
        "2T020.14"
    );
    assert_eq!(
        nf(&mut t, vec![f(2020.1415), i(2), s("F"), Value::Null]),
        "2,020F14"
    );
    // empty separators
    assert_eq!(
        nf(&mut t, vec![f(1234567.891), i(2), s(""), s("")]),
        "123456789"
    );
    // special floats
    assert_eq!(nf(&mut t, vec![f(f64::NAN), i(2)]), "nan");
    assert_eq!(nf(&mut t, vec![f(f64::INFINITY)]), "inf");
    // doubles beyond 2^52 use the integer formatter
    assert_eq!(
        nf(&mut t, vec![f(4503599627370496.0)]),
        "4,503,599,627,370,496"
    );
    // large doubles beyond the integer range
    assert_eq!(nf(&mut t, vec![f(1e19)]), "10,000,000,000,000,000,000");
    assert_eq!(nf(&mut t, vec![f(0.5)]), "1");
    assert_eq!(nf(&mut t, vec![f(-0.5)]), "-1");
    assert_eq!(nf(&mut t, vec![f(1.005), i(2)]), "1.01");
    assert_eq!(nf(&mut t, vec![i(1000)]), "1,000");
    assert_eq!(nf(&mut t, vec![i(-1234567), i(2)]), "-1,234,567.00");
}

#[test]
fn number_format_decimals_range() {
    // gh17384.phpt
    let mut t = TestVm::new();
    for number in [f(1.23456), i(1)] {
        for decimals in [9876543210i64, -9876543210] {
            assert_eq!(
                err(&mut t, "number_format", vec![number.clone(), i(decimals)]),
                "ValueError: number_format(): Argument #2 ($decimals) must be between -2147483648 and 2147483647"
            );
        }
    }
    assert_eq!(
        err(&mut t, "number_format", vec![Value::empty_array()]),
        "TypeError: number_format(): Argument #1 ($num) must be of type int|float, array given"
    );
}

// ---------------------------------------------------------------------------
// constants and registration
// ---------------------------------------------------------------------------

#[test]
fn constants() {
    let t = TestVm::new();
    let c = |n: &str| t.vm.constant(n).unwrap_or_else(|| panic!("{n} defined"));
    assert_eq!(dump(&c("M_PI")), "float(3.141592653589793)");
    assert_eq!(dump(&c("M_E")), "float(2.718281828459045)");
    assert_eq!(dump(&c("M_LOG2E")), "float(1.4426950408889634)");
    assert_eq!(dump(&c("M_LOG10E")), "float(0.4342944819032518)");
    assert_eq!(dump(&c("M_LN2")), "float(0.6931471805599453)");
    assert_eq!(dump(&c("M_LN10")), "float(2.302585092994046)");
    assert_eq!(dump(&c("M_PI_2")), "float(1.5707963267948966)");
    assert_eq!(dump(&c("M_PI_4")), "float(0.7853981633974483)");
    assert_eq!(dump(&c("M_1_PI")), "float(0.3183098861837907)");
    assert_eq!(dump(&c("M_2_PI")), "float(0.6366197723675814)");
    assert_eq!(dump(&c("M_SQRTPI")), "float(1.772453850905516)");
    assert_eq!(dump(&c("M_2_SQRTPI")), "float(1.1283791670955126)");
    assert_eq!(dump(&c("M_LNPI")), "float(1.1447298858494002)");
    assert_eq!(dump(&c("M_EULER")), "float(0.5772156649015329)");
    assert_eq!(dump(&c("M_SQRT2")), "float(1.4142135623730951)");
    assert_eq!(dump(&c("M_SQRT1_2")), "float(0.7071067811865476)");
    assert_eq!(dump(&c("M_SQRT3")), "float(1.7320508075688772)");
    assert_eq!(dump(&c("INF")), "float(INF)");
    assert_eq!(dump(&c("NAN")), "float(NAN)");
    assert_eq!(dump(&c("PHP_ROUND_HALF_UP")), "int(1)");
    assert_eq!(dump(&c("PHP_ROUND_HALF_DOWN")), "int(2)");
    assert_eq!(dump(&c("PHP_ROUND_HALF_EVEN")), "int(3)");
    assert_eq!(dump(&c("PHP_ROUND_HALF_ODD")), "int(4)");
}

#[test]
fn rounding_mode_enum_shape() {
    let mut t = TestVm::new();
    let ce = t.vm.find_class("RoundingMode").expect("RoundingMode");
    assert!(ce.is_enum());
    assert!(ce.instance_of_name(b"UnitEnum"));
    let names: Vec<String> = ce.constants.values().map(|c| c.name.to_string()).collect();
    assert_eq!(
        names,
        [
            "HalfAwayFromZero",
            "HalfTowardsZero",
            "HalfEven",
            "HalfOdd",
            "TowardsZero",
            "AwayFromZero",
            "NegativeInfinity",
            "PositiveInfinity"
        ]
    );
    // cases are singletons
    let a = case(&mut t, "RoundingMode", "HalfEven");
    let b = case(&mut t, "RoundingMode", "HalfEven");
    assert!(Value::identical(&a, &b));
    // a stray object is rejected by ZPP
    let obj = case(&mut t, "Random\\IntervalBoundary", "OpenOpen");
    let r: PResult<Value> = t.call("round", vec![f(1.5), i(0), obj]);
    assert_eq!(
        thrown(r).1,
        "round(): Argument #3 ($mode) must be of type RoundingMode|int, Random\\IntervalBoundary given"
    );
}

// ---------------------------------------------------------------------------
// regression tests (bug*.phpt)
// ---------------------------------------------------------------------------

#[test]
fn number_format_bug21523_many_decimals() {
    let mut t = TestVm::new();
    let r = call(&mut t, "number_format", vec![i(-2000), i(2768)]);
    let want = format!("-2,000.{}", "0".repeat(2768));
    assert_eq!(interp(&r), want);
    assert_eq!(want.len(), 2775);
    // doubles go through the 318-digit limit of %F and are zero padded
    let r = call(&mut t, "number_format", vec![f(-2000.5), i(400)]);
    let s = interp(&r);
    assert!(s.starts_with("-2,000.5000"));
    assert_eq!(s.len(), "-2,000.".len() + 400);
}

#[test]
fn round_bug24142_and_bug25694() {
    let mut t = TestVm::new();
    for (v, p, want) in [
        (0.005, 2, "float(0.01)"),
        (0.015, 2, "float(0.02)"),
        (0.025, 2, "float(0.03)"),
        (0.035, 2, "float(0.04)"),
        (0.045, 2, "float(0.05)"),
        (0.055, 2, "float(0.06)"),
        (0.065, 2, "float(0.07)"),
        (0.075, 2, "float(0.08)"),
        (0.085, 2, "float(0.09)"),
    ] {
        assert_eq!(
            dump(&call(&mut t, "round", vec![f(v), i(p)])),
            want,
            "round({v}, {p})"
        );
    }
    for (v, want) in [
        (0.045, "0.05"),
        (0.055, "0.06"),
        (5.045, "5.05"),
        (5.055, "5.06"),
        (3.025, "3.03"),
        (4.025, "4.03"),
    ] {
        assert_eq!(interp(&call(&mut t, "round", vec![f(v), i(2)])), want);
        assert_eq!(
            interp(&call(&mut t, "number_format", vec![f(v), i(2)])),
            want
        );
    }
}

#[test]
fn number_format_separators_bug28228_bug62112() {
    let mut t = TestVm::new();
    let n = f(1234.5678);
    assert_eq!(nf(&mut t, vec![n.clone(), i(4), s(""), s("")]), "12345678");
    assert_eq!(
        nf(&mut t, vec![n.clone(), i(4), Value::Null, s(",")]),
        "1,234.5678"
    );
    assert_eq!(
        nf(&mut t, vec![n.clone(), i(4), i(0), s(",")]),
        "1,23405678"
    );
    assert_eq!(nf(&mut t, vec![n, i(4)]), "1,234.5678");
    // binary safe separators
    let r = call(&mut t, "number_format", vec![f(2.5), i(2), s("\0"), s("'")]);
    assert_eq!(dump(&r).as_bytes(), b"string(4) \"2\x0050\"");
}

#[test]
fn infinity_checks_bug74039() {
    let mut t = TestVm::new();
    let mut out = Vec::new();
    for v in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        for name in ["is_finite", "is_infinite", "is_nan"] {
            out.push(dump(&call(&mut t, name, vec![f(v)])));
        }
    }
    let expected = [
        "bool(false)",
        "bool(true)",
        "bool(false)",
        "bool(false)",
        "bool(true)",
        "bool(false)",
        "bool(false)",
        "bool(false)",
        "bool(true)",
    ];
    assert_eq!(out, expected);
    // pow(0, -2) is INF; acos(1.01) is NAN (bug45712 / bug25665)
    assert_eq!(dump(&call(&mut t, "pow", vec![i(0), i(-2)])), "float(INF)");
    assert_eq!(dump(&call(&mut t, "acos", vec![f(1.01)])), "float(NAN)");
    assert_eq!(dump(&call(&mut t, "log", vec![i(0)])), "float(-INF)");
}
