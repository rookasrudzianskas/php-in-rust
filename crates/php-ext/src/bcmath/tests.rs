//! Unit tests for ext/bcmath, ported from `ext/bcmath/tests/*.phpt`.
//!
//! The table driven phpt files (`run_bcmath_tests_function.inc`, `bcround_test_helper.inc`)
//! are replayed from their own sources: the PHP array literals are extracted
//! from the `--FILE--` section and the produced text is compared with the
//! `--EXPECT--` section.

use std::collections::HashMap;

use php_runtime::{ObjRef, PResult, PhpStr, Value, Vm};

use crate::testing::{TestVm, thrown};

macro_rules! phpt {
    ($name:literal) => {
        include_str!(concat!("../../../../tests/phpt/ext/bcmath/tests/", $name))
    };
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(x: &str) -> Value {
    Value::str(x)
}

fn i(n: i64) -> Value {
    Value::Int(n)
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.to_string_lossy().into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn int(v: &Value) -> i64 {
    v.as_int()
        .unwrap_or_else(|| panic!("expected an int, got {v:?}"))
}

fn call(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    text(
        &t.call(name, args)
            .unwrap_or_else(|e| panic!("{name}() failed: {e:?}")),
    )
}

/// `Class: message` of a thrown exception.
fn err(r: PResult<Value>) -> String {
    let (class, message) = thrown(r);
    format!("{class}: {message}")
}

fn call_err(t: &mut TestVm, name: &str, args: Vec<Value>) -> String {
    let r = t.call(name, args);
    err(r)
}

/// The text of the `--EXPECT--` (or `--EXPECTF--`) section, trailing whitespace trimmed.
fn expect(src: &str) -> &str {
    let start = ["--EXPECT--\n", "--EXPECTF--\n"]
        .iter()
        .find_map(|m| src.find(m).map(|i| i + m.len()))
        .expect("phpt has an expectation section");
    src[start..].trim_end()
}

/// The `--FILE--` section.
fn file_section(src: &str) -> &str {
    let start = src.find("--FILE--\n").expect("--FILE--") + "--FILE--\n".len();
    let end = src[start..]
        .find("\n--EXPECT")
        .map_or(src.len(), |i| start + i);
    &src[start..end]
}

/// Splits `s` at `sep` outside quotes and brackets.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut quote) = (0i32, None::<char>);
    for c in s.chars() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' | '[' | '{' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' | ']' | '}' => {
                    depth -= 1;
                    cur.push(c);
                }
                c if c == sep && depth == 0 => out.push(std::mem::take(&mut cur)),
                c => cur.push(c),
            },
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// All quoted string literals of `s` in order.
fn quoted(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let lit: String = chars.by_ref().take_while(|&x| x != c).collect();
            out.push(lit);
        }
    }
    out
}

fn eval_list(expr: &str, vars: &HashMap<String, Vec<String>>) -> Vec<String> {
    let e = expr.trim();
    if let Some(inner) = e
        .strip_prefix("array_merge(")
        .and_then(|x| x.strip_suffix(')'))
    {
        split_top(inner, ',')
            .iter()
            .flat_map(|a| eval_list(a, vars))
            .collect()
    } else if let Some(name) = e.strip_prefix('$') {
        vars[name].clone()
    } else if e.starts_with('[') {
        quoted(e)
    } else {
        panic!("unsupported expression {e}")
    }
}

/// Replays a phpt that calls `run_bcmath_tests($a, $b, "op", fn(...))`.
fn run_bcmath_phpt(src: &str) {
    let mut vars: HashMap<String, Vec<String>> = HashMap::new();
    let mut run = None;
    for stmt in split_top(file_section(src), ';') {
        let stmt = stmt.trim();
        if let Some(rest) = stmt.strip_prefix('$') {
            let (name, rhs) = rest.split_once('=').expect("assignment");
            vars.insert(name.trim().to_string(), eval_list(rhs, &vars));
        } else if let Some(args) = stmt
            .strip_prefix("run_bcmath_tests(")
            .and_then(|x| x.strip_suffix(')'))
        {
            let parts = split_top(args, ',');
            let sym = quoted(&parts[2])[0].clone();
            let func = parts[3].trim().trim_end_matches("(...)").to_string();
            run = Some((
                eval_list(&parts[0], &vars),
                eval_list(&parts[1], &vars),
                sym,
                func,
            ));
        }
    }
    let (firsts, seconds, symbol, func) = run.expect("run_bcmath_tests call");
    let mut t = TestVm::new();
    let mut out = String::new();
    for scale in [0, 10] {
        for first in &firsts {
            out += &format!("Number \"{first}\" (scale {scale})\n");
            for second in &seconds {
                let ret = match t.call(&func, vec![s(first), s(second), i(scale)]) {
                    Ok(v) => text(&v),
                    Err(php_runtime::Flow::Exception(e)) => Vm::exception_message(&e),
                    Err(other) => panic!("unexpected {other:?}"),
                };
                out += &format!("{first} {symbol} {second:<30} = {ret}\n");
            }
            out += "\n";
        }
    }
    assert_eq!(out.trim_end(), expect(src));
}

fn pad_left(s: &str, width: usize) -> String {
    format!("{s:>width$}")
}

// ---------------------------------------------------------------------------
// run_bcmath_tests based phpt files
// ---------------------------------------------------------------------------

#[test]
fn bcadd_tables() {
    run_bcmath_phpt(phpt!("bcadd.phpt"));
    run_bcmath_phpt(phpt!("bcadd_zero.phpt"));
    run_bcmath_phpt(phpt!("bcadd_large_numbers.phpt"));
}

#[test]
fn bcsub_tables() {
    run_bcmath_phpt(phpt!("bcsub.phpt"));
    run_bcmath_phpt(phpt!("bcsub_zero.phpt"));
    run_bcmath_phpt(phpt!("bcsub_large_numbers.phpt"));
}

#[test]
fn bcmul_tables() {
    run_bcmath_phpt(phpt!("bcmul.phpt"));
    run_bcmath_phpt(phpt!("bcmul_zero.phpt"));
    run_bcmath_phpt(phpt!("bcmul_large_numbers.phpt"));
}

#[test]
fn bcdiv_tables() {
    run_bcmath_phpt(phpt!("bcdiv.phpt"));
    run_bcmath_phpt(phpt!("bcdiv_zero.phpt"));
    run_bcmath_phpt(phpt!("bcdiv_large_numbers.phpt"));
}

#[test]
fn bcmod_tables() {
    run_bcmath_phpt(phpt!("bcmod.phpt"));
    run_bcmath_phpt(phpt!("bcmod_zero.phpt"));
    run_bcmath_phpt(phpt!("bcmod_large_numbers.phpt"));
}

#[test]
fn bcpow_tables() {
    run_bcmath_phpt(phpt!("bcpow.phpt"));
    run_bcmath_phpt(phpt!("bcpow_zero.phpt"));
    run_bcmath_phpt(phpt!("bcpow_large_numbers.phpt"));
}

// ---------------------------------------------------------------------------
// bcdivmod
// ---------------------------------------------------------------------------

const DIVIDENDS: [&str; 8] = ["15", "-15", "1", "-9", "14.14", "-16.60", "0.15", "-0.01"];
const DIVISORS_EXTRA: [&str; 6] = [
    "15151324141414.412312232141241",
    "-132132245132134.1515123765412",
    "141241241241241248267654747412",
    "-149143276547656984948124912",
    "0.1322135476547459213732911312",
    "-0.123912932193769965476541321",
];

#[test]
fn bcdivmod_matches_bcdiv_and_bcmod() {
    // bcdivmod.phpt
    let mut t = TestVm::new();
    let divisors: Vec<&str> = DIVIDENDS
        .iter()
        .chain(DIVISORS_EXTRA.iter())
        .copied()
        .collect();
    for scale in [0, 10] {
        for first in DIVIDENDS {
            for second in &divisors {
                let pair = t
                    .call("bcdivmod", vec![s(first), s(second), i(scale)])
                    .expect("bcdivmod");
                let pair = pair.as_array().expect("array").clone();
                let quot = text(pair.get_int(0).expect("quot"));
                let rem = text(pair.get_int(1).expect("rem"));
                let div = call(&mut t, "bcdiv", vec![s(first), s(second), i(0)]);
                let md = call(&mut t, "bcmod", vec![s(first), s(second), i(scale)]);
                let cmp = |t: &mut TestVm, a: &str, b: &str| {
                    int(&t.call("bccomp", vec![s(a), s(b)]).expect("bccomp"))
                };
                assert_eq!(cmp(&mut t, &quot, &div), 0, "{first} {second}");
                assert_eq!(cmp(&mut t, &rem, &md), 0, "{first} {second}");
            }
        }
    }
}

#[test]
fn bcdivmod_by_zero() {
    // bcdivmod_by_zero.phpt
    let mut t = TestVm::new();
    for first in DIVIDENDS.iter().chain(DIVISORS_EXTRA.iter()) {
        for zero in ["0", "0.00"] {
            assert_eq!(
                call_err(&mut t, "bcdivmod", vec![s(first), s(zero)]),
                "DivisionByZeroError: Division by zero"
            );
        }
    }
}

#[test]
fn bcdivmod_values() {
    let mut t = TestVm::new();
    let r = t
        .call("bcdivmod", vec![s("-9"), s("15"), i(2)])
        .expect("bcdivmod");
    let a = r.as_array().unwrap();
    assert_eq!(text(a.get_int(0).unwrap()), "0");
    assert_eq!(text(a.get_int(1).unwrap()), "-9.00");
    let r = t.call("bcdivmod", vec![s("10"), s("3")]).expect("bcdivmod");
    let a = r.as_array().unwrap();
    assert_eq!(text(a.get_int(0).unwrap()), "3");
    assert_eq!(text(a.get_int(1).unwrap()), "1");
}

// ---------------------------------------------------------------------------
// Error tests
// ---------------------------------------------------------------------------

#[test]
fn not_well_formed_arguments() {
    let mut t = TestVm::new();
    for (func, names) in [
        ("bcadd", ["num1", "num2"]),
        ("bcsub", ["num1", "num2"]),
        ("bcmul", ["num1", "num2"]),
        ("bcdiv", ["num1", "num2"]),
        ("bcmod", ["num1", "num2"]),
        ("bccomp", ["num1", "num2"]),
    ] {
        assert_eq!(
            call_err(&mut t, func, vec![s("a"), s("1")]),
            format!(
                "ValueError: {func}(): Argument #1 (${}) is not well-formed",
                names[0]
            )
        );
        assert_eq!(
            call_err(&mut t, func, vec![s("1"), s("a")]),
            format!(
                "ValueError: {func}(): Argument #2 (${}) is not well-formed",
                names[1]
            )
        );
    }
    // bug80545
    assert_eq!(
        call_err(&mut t, "bcadd", vec![s("a"), s("a")]),
        "ValueError: bcadd(): Argument #1 ($num1) is not well-formed"
    );
    // bcpow, bcpowmod, bcsqrt, bcceil, bcfloor, bcround
    assert_eq!(
        call_err(&mut t, "bcpow", vec![s("a"), s("1")]),
        "ValueError: bcpow(): Argument #1 ($num) is not well-formed"
    );
    assert_eq!(
        call_err(&mut t, "bcpow", vec![s("1"), s("a")]),
        "ValueError: bcpow(): Argument #2 ($exponent) is not well-formed"
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("a"), s("1"), s("1")]),
        "ValueError: bcpowmod(): Argument #1 ($num) is not well-formed"
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("1"), s("a"), s("1")]),
        "ValueError: bcpowmod(): Argument #2 ($exponent) is not well-formed"
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("1"), s("1"), s("a")]),
        "ValueError: bcpowmod(): Argument #3 ($modulus) is not well-formed"
    );
    assert_eq!(
        call_err(&mut t, "bcsqrt", vec![s("a")]),
        "ValueError: bcsqrt(): Argument #1 ($num) is not well-formed"
    );
    for func in ["bcceil", "bcfloor", "bcround"] {
        for bad in ["hoge", "0.00.1"] {
            assert_eq!(
                call_err(&mut t, func, vec![s(bad)]),
                format!("ValueError: {func}(): Argument #1 ($num) is not well-formed")
            );
        }
    }
}

#[test]
fn division_and_modulo_by_zero() {
    let mut t = TestVm::new();
    for zero in ["0", "0.00", "-0.00"] {
        assert_eq!(
            call_err(&mut t, "bcdiv", vec![s("10.99"), s(zero)]),
            "DivisionByZeroError: Division by zero"
        );
    }
    for zero in ["0", "0.000", "-0.0"] {
        assert_eq!(
            call_err(&mut t, "bcmod", vec![s("10"), s(zero)]),
            "DivisionByZeroError: Modulo by zero"
        );
    }
}

#[test]
fn sqrt_of_negative() {
    let mut t = TestVm::new();
    assert_eq!(
        call_err(&mut t, "bcsqrt", vec![s("-9")]),
        "ValueError: bcsqrt(): Argument #1 ($num) must be greater than or equal to 0"
    );
}

#[test]
fn powmod_errors() {
    let mut t = TestVm::new();
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("1"), s("-1"), s("2")]),
        "ValueError: bcpowmod(): Argument #2 ($exponent) must be greater than or equal to 0"
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("1"), s("1"), s("0")]),
        "DivisionByZeroError: Modulo by zero"
    );
    // bug72093
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![i(1), i(0), i(128), i(-200)]),
        "ValueError: bcpowmod(): Argument #4 ($scale) must be between 0 and 2147483647"
    );
    assert_eq!(
        call_err(
            &mut t,
            "bcpowmod",
            vec![i(1), Value::Float(1.2), i(1), i(1)]
        ),
        "ValueError: bcpowmod(): Argument #2 ($exponent) cannot have a fractional part"
    );
    // bug75178
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("4.1"), s("4"), s("3"), i(3)]),
        "ValueError: bcpowmod(): Argument #1 ($num) cannot have a fractional part"
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("4"), s("4"), s("3.1"), i(3)]),
        "ValueError: bcpowmod(): Argument #3 ($modulus) cannot have a fractional part"
    );
    // bug78878
    assert_eq!(
        call_err(
            &mut t,
            "bcpowmod",
            vec![i(2), i(65535), Value::Float(-4e-4)]
        ),
        "ValueError: bcpowmod(): Argument #3 ($modulus) cannot have a fractional part"
    );
}

#[test]
fn powmod_values() {
    let mut t = TestVm::new();
    // bcpowmod.phpt
    assert_eq!(call(&mut t, "bcpowmod", vec![s("5"), s("2"), s("7")]), "4");
    assert_eq!(
        call(&mut t, "bcpowmod", vec![s("-2"), s("5"), s("7")]),
        "-4"
    );
    assert_eq!(
        call(
            &mut t,
            "bcpowmod",
            vec![s("10"), s("2147483648"), s("2047")]
        ),
        "790"
    );
    assert_eq!(
        call(&mut t, "bcpowmod", vec![s("10"), s("0"), s("2047")]),
        "1"
    );
    // bug44995
    assert_eq!(
        call(&mut t, "bcpowmod", vec![s("4"), s("4"), s("3"), i(1)]),
        "1.0"
    );
    assert_eq!(
        call(
            &mut t,
            "bcpowmod",
            vec![s("3234"), s("32345"), s("22345"), i(1)]
        ),
        "17334.0"
    );
    // bug54598 / bcpowmod_with_mod_1
    assert_eq!(call(&mut t, "bcpowmod", vec![i(5), i(0), i(1)]), "0");
    assert_eq!(
        call(&mut t, "bcpowmod", vec![i(5), i(0), i(1), i(3)]),
        "0.000"
    );
    assert_eq!(call(&mut t, "bcpowmod", vec![i(5), i(0), i(-1)]), "0");
    assert_eq!(
        call(&mut t, "bcpowmod", vec![i(5), i(0), i(-1), i(3)]),
        "0.000"
    );
}

#[test]
fn pow_errors() {
    let mut t = TestVm::new();
    // bcpow_error1
    for exp in ["1.1", "0.1"] {
        assert_eq!(
            call_err(&mut t, "bcpow", vec![s("1"), s(exp), i(2)]),
            "ValueError: bcpow(): Argument #2 ($exponent) cannot have a fractional part"
        );
    }
    // bcpow_error2
    for exp in ["9223372036854775808", "-9223372036854775808"] {
        assert_eq!(
            call_err(&mut t, "bcpow", vec![s("0"), s(exp), i(2)]),
            "ValueError: bcpow(): Argument #2 ($exponent) is too large"
        );
    }
    // bcpow_div_by_zero
    for base in ["0", "-0"] {
        for exp in ["-15", "-1", "-9"] {
            assert_eq!(
                call_err(&mut t, "bcpow", vec![s(base), s(exp)]),
                "DivisionByZeroError: Negative power of zero"
            );
        }
    }
    // GH-16978
    assert_eq!(
        call(&mut t, "bcpow", vec![s("10"), s("-112"), i(10)]),
        "0.0000000000"
    );
    assert_eq!(
        call(
            &mut t,
            "bcdiv",
            vec![s("1"), s(&format!("1{}", "0".repeat(100))), i(1)]
        ),
        "0.0"
    );
}

#[test]
fn pow_overflow_is_reported() {
    let mut t = TestVm::new();
    // The digit count of 10^LONG_MAX overflows size_t.
    assert_eq!(
        call_err(
            &mut t,
            "bcpow",
            vec![s("10000000000"), s("9223372036854775807")]
        ),
        "ValueError: bcpow(): Argument #2 ($exponent) exponent is too large, the number of digits overflowed"
    );
    // A huge but representable result exceeds the memory limit.
    let r = t.call("bcpow", vec![s("2"), s("1128321638")]);
    assert!(
        matches!(&r, Err(php_runtime::Flow::Fatal)) || r.is_err(),
        "huge power must fail"
    );
}

#[test]
fn negative_scale_errors() {
    // negative_scale.phpt
    let mut t = TestVm::new();
    let msg = |f: &str, n: usize, name: &str| {
        format!("ValueError: {f}(): Argument #{n} (${name}) must be between 0 and 2147483647")
    };
    assert_eq!(
        call_err(&mut t, "bcadd", vec![s("1"), s("2"), i(-1)]),
        msg("bcadd", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcsub", vec![s("1"), s("2"), i(-1)]),
        msg("bcsub", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcmul", vec![s("1"), s("2"), i(-1)]),
        msg("bcmul", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcdiv", vec![s("1"), s("2"), i(-1)]),
        msg("bcdiv", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcmod", vec![s("1"), s("2"), i(-1)]),
        msg("bcmod", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcpowmod", vec![s("1"), s("2"), s("3"), i(-9)]),
        msg("bcpowmod", 4, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcpow", vec![s("1"), s("2"), i(-1)]),
        msg("bcpow", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcsqrt", vec![s("9"), i(-1)]),
        msg("bcsqrt", 2, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bccomp", vec![s("1"), s("2"), i(-1)]),
        msg("bccomp", 3, "scale")
    );
    assert_eq!(
        call_err(&mut t, "bcscale", vec![i(-1)]),
        msg("bcscale", 1, "scale")
    );
    // bug60377
    assert_eq!(
        call_err(&mut t, "bcscale", vec![i(634314234334311)]),
        msg("bcscale", 1, "scale")
    );
}

// ---------------------------------------------------------------------------
// bcscale and bcmath.scale
// ---------------------------------------------------------------------------

#[test]
fn bcscale_changes_default_scale() {
    // bcscale.phpt
    let mut t = TestVm::new();
    assert_eq!(call(&mut t, "bcadd", vec![s("1"), s("2")]), "3");
    t.call("bcscale", vec![i(2)]).unwrap();
    assert_eq!(call(&mut t, "bcadd", vec![s("1"), s("2")]), "3.00");
    t.call("bcscale", vec![i(10)]).unwrap();
    assert_eq!(call(&mut t, "bcadd", vec![s("1"), s("2")]), "3.0000000000");
    t.call("bcscale", vec![i(0)]).unwrap();
    assert_eq!(call(&mut t, "bcadd", vec![s("1"), s("2")]), "3");
}

#[test]
fn bcscale_return_values() {
    // bcscale_variation003.phpt
    let mut t = TestVm::new();
    let ini = |t: &TestVm| {
        t.vm.ini_get("bcmath.scale")
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    assert_eq!(ini(&t), "0");
    assert_eq!(int(&t.call("bcscale", vec![i(1)]).unwrap()), 0);
    assert_eq!(ini(&t), "1");
    assert_eq!(int(&t.call("bcscale", vec![i(4)]).unwrap()), 1);
    assert_eq!(ini(&t), "4");
    assert_eq!(int(&t.call("bcscale", vec![]).unwrap()), 4);
    assert_eq!(int(&t.call("bcscale", vec![Value::Null]).unwrap()), 4);
    assert_eq!(ini(&t), "4");
}

#[test]
fn bcmath_scale_ini() {
    // scale_ini.phpt
    let mut t = TestVm::new();
    t.vm.ini_set_system("bcmath.scale", "5");
    assert_eq!(call(&mut t, "bcadd", vec![s("2"), s("1")]), "3.00000");
    assert_eq!(call(&mut t, "bcdiv", vec![s("2"), s("1")]), "2.00000");
    assert_eq!(call(&mut t, "bcmul", vec![s("2"), s("1")]), "2.00000");
    assert_eq!(call(&mut t, "bcpow", vec![s("2"), s("1")]), "2.00000");
    assert_eq!(
        call(&mut t, "bcpowmod", vec![s("2"), s("1"), s("3")]),
        "2.00000"
    );
    assert_eq!(call(&mut t, "bcsqrt", vec![s("4")]), "2.00000");
    assert_eq!(call(&mut t, "bcsub", vec![s("2"), s("1")]), "1.00000");
    // ini_set rejects values outside 0..=INT_MAX.
    assert!(t.vm.ini_set("bcmath.scale", "-1").is_none());
    assert!(t.vm.ini_set("bcmath.scale", "2147483648").is_none());
    assert!(t.vm.ini_set("bcmath.scale", "3").is_some());
}

#[test]
fn explicit_scale_results() {
    // scale.phpt
    let mut t = TestVm::new();
    let five = |t: &mut TestVm, f: &str, args: Vec<&str>| {
        let mut a: Vec<Value> = args.into_iter().map(s).collect();
        a.push(i(5));
        call(t, f, a)
    };
    assert_eq!(five(&mut t, "bcadd", vec!["2", "1"]), "3.00000");
    assert_eq!(five(&mut t, "bcdiv", vec!["2", "1"]), "2.00000");
    assert_eq!(five(&mut t, "bcmul", vec!["2", "1"]), "2.00000");
    assert_eq!(five(&mut t, "bcpow", vec!["2", "1"]), "2.00000");
    assert_eq!(five(&mut t, "bcpowmod", vec!["2", "1", "3"]), "2.00000");
    assert_eq!(five(&mut t, "bcsqrt", vec!["4"]), "2.00000");
    assert_eq!(five(&mut t, "bcsub", vec!["2", "1"]), "1.00000");
}

#[test]
fn negative_ini_scale_falls_back_to_zero() {
    // bcscale_variation002.phpt (bcmath.scale=-2)
    let mut t = TestVm::new();
    t.vm.ini_set_system("bcmath.scale", "-2");
    assert_eq!(call(&mut t, "bcadd", vec![s("-4.27"), s("7.3")]), "3");
}

#[test]
fn bcscale_with_default_scale_division() {
    // bcscale_variation001.phpt
    let mut t = TestVm::new();
    assert_eq!(call(&mut t, "bcdiv", vec![s("20.56"), s("4")]), "5");
    assert_eq!(
        call_err(&mut t, "bcscale", vec![i(-4)]),
        "ValueError: bcscale(): Argument #1 ($scale) must be between 0 and 2147483647"
    );
}

// ---------------------------------------------------------------------------
// Argument formatting
// ---------------------------------------------------------------------------

#[test]
fn argument_formatting() {
    // str2num_formatting.phpt
    let mut t = TestVm::new();
    assert_eq!(call(&mut t, "bcadd", vec![s("1"), s("2")]), "3");
    assert_eq!(call(&mut t, "bcadd", vec![s("1.1"), s("2"), i(2)]), "3.10");
    assert_eq!(call(&mut t, "bcadd", vec![s(""), s("2"), i(2)]), "2.00");
    assert_eq!(call(&mut t, "bcadd", vec![s("+0"), s("2")]), "2");
    assert_eq!(call(&mut t, "bcadd", vec![s("-0"), s("2")]), "2");
    for bad in [" 0", "1e1", "1,1", "Hello", "1 1", "1.a"] {
        assert_eq!(
            call_err(&mut t, "bcadd", vec![s(bad), s("2")]),
            "ValueError: bcadd(): Argument #1 ($num1) is not well-formed"
        );
        assert_eq!(
            call_err(&mut t, "bccomp", vec![s(bad), s("2")]),
            "ValueError: bccomp(): Argument #1 ($num1) is not well-formed"
        );
    }
    for (a, scale) in [
        ("1", None),
        ("1.1", Some(2)),
        ("", None),
        ("+0", None),
        ("-0", None),
    ] {
        let mut args = vec![s(a), s("2")];
        if let Some(sc) = scale {
            args.push(i(sc));
        }
        assert_eq!(int(&t.call("bccomp", args).unwrap()), -1);
    }
}

#[test]
fn embedded_nul_is_rejected() {
    // sec_embedded_null_truncation.phpt
    let mut t = TestVm::new();
    for bad in ["100\x005", "1.2\x003", "\x00123"] {
        let v = Value::String(PhpStr::from(bad));
        for (func, rest, name) in [
            ("bcadd", vec![s("0")], "num1"),
            ("bcsub", vec![s("0")], "num1"),
            ("bcmul", vec![s("1")], "num1"),
            ("bcdiv", vec![s("1")], "num1"),
            ("bcmod", vec![s("7")], "num1"),
            ("bcpow", vec![s("1")], "num"),
            ("bccomp", vec![s("0")], "num1"),
            ("bcsqrt", vec![], "num"),
        ] {
            let mut args = vec![v.clone()];
            args.extend(rest);
            assert_eq!(
                call_err(&mut t, func, args),
                format!("ValueError: {func}(): Argument #1 (${name}) is not well-formed")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// bccomp
// ---------------------------------------------------------------------------

#[test]
fn bccomp_cases() {
    let mut t = TestVm::new();
    let cmp = |t: &mut TestVm, a: &str, b: &str, scale: Option<i64>| {
        let mut args = vec![s(a), s(b)];
        if let Some(sc) = scale {
            args.push(i(sc));
        }
        int(&t.call("bccomp", args).unwrap())
    };
    // bccomp.phpt
    assert_eq!(cmp(&mut t, "-1", "5", Some(4)), -1);
    assert_eq!(
        cmp(
            &mut t,
            "1928372132132819737213",
            "8728932001983192837219398127471",
            None
        ),
        -1
    );
    assert_eq!(cmp(&mut t, "1.00000000000000000001", "1", Some(2)), 0);
    assert_eq!(cmp(&mut t, "97321", "2321", None), 1);
    assert_eq!(cmp(&mut t, "0", "14", None), -1);
    assert_eq!(cmp(&mut t, "1", "0", None), 1);
    assert_eq!(cmp(&mut t, "0.000", "0", Some(3)), 0);
    // bccomp_variation001
    assert_eq!(cmp(&mut t, "2.2", "2.2", Some(2)), 0);
    assert_eq!(cmp(&mut t, "2.32", "2.2", Some(2)), 1);
    assert_eq!(cmp(&mut t, "2.29", "2.3", Some(2)), -1);
    assert_eq!(cmp(&mut t, "2.29", "0", Some(2)), 1);
    // bccomp_variation002
    assert_eq!(cmp(&mut t, "-2", "-2", None), 0);
    assert_eq!(cmp(&mut t, "-2", "2", Some(1)), -1);
    assert_eq!(cmp(&mut t, "-2.29", "-2.3", Some(2)), 1);
    assert_eq!(cmp(&mut t, "-2.29", "2.3", Some(2)), -1);
    assert_eq!(cmp(&mut t, "2.29", "-2.3", Some(2)), 1);
    assert_eq!(cmp(&mut t, "-2.29", "-2.3", Some(1)), 1);
    assert_eq!(cmp(&mut t, "-2.29", "0", Some(1)), -1);
    assert_eq!(cmp(&mut t, "0.001", "-0.001", Some(1)), 0);
    // GHSA-x692-q9x7-8c3f
    let n = format!("1.9{}1", "0".repeat(300));
    assert_eq!(cmp(&mut t, &n, "0", Some(300)), 1);
    // bug46781
    assert_eq!(cmp(&mut t, "-0.0", "0", Some(1)), 0);
}

// ---------------------------------------------------------------------------
// bcceil, bcfloor, bcround
// ---------------------------------------------------------------------------

fn floor_ceil_expect(src: &str, func: &str) {
    let mut t = TestVm::new();
    let mut out = String::new();
    for num in quoted(file_section(src).split("foreach").next().unwrap()) {
        out += &format!(
            "{} => {}\n",
            pad_left(&num, 50),
            call(&mut t, func, vec![s(&num)])
        );
    }
    assert_eq!(out.trim_end(), expect(src));
}

#[test]
fn bcceil_and_bcfloor() {
    floor_ceil_expect(phpt!("bcceil.phpt"), "bcceil");
    floor_ceil_expect(phpt!("bcfloor.phpt"), "bcfloor");
}

/// The `[number, precision]` pairs of a `$name = [...]` array in a PHP file.
fn round_cases(src: &str, name: &str) -> Vec<(String, i64)> {
    let start = src.find(&format!("${name} = [")).expect("array") + name.len() + 5;
    let body = &src[start..];
    let end = body.find("];").expect("array end");
    split_top(&body[..end], ',')
        .iter()
        .filter(|c| !c.trim().is_empty())
        .map(|c| {
            let inner = c.trim().trim_start_matches('[').trim_end_matches(']');
            let parts = split_top(inner, ',');
            (
                quoted(&parts[0])[0].clone(),
                parts[1].trim().parse().expect("precision"),
            )
        })
        .collect()
}

fn mode_case(t: &mut TestVm, name: &str) -> Value {
    t.vm.class_constant("RoundingMode", name)
        .expect("lookup")
        .unwrap_or_else(|| panic!("RoundingMode::{name}"))
}

fn run_round_test(t: &mut TestVm, mode: &str) -> String {
    let helper = phpt!("bcround_test_helper.inc");
    let mode = mode_case(t, mode);
    let mut out = String::new();
    for (title, case) in [
        ("non-boundary value", "non_boundary_value_cases"),
        ("minus precision", "minus_precision_cases"),
        ("zero precision", "zero_precision_cases"),
        ("plus precision", "plus_precision_cases"),
    ] {
        out += &format!("========== {title} ==========\n");
        for (num, precision) in round_cases(helper, case) {
            let label = pad_left(&format!("[{num}, {precision}]"), 17);
            let r = call(t, "bcround", vec![s(&num), i(precision), mode.clone()]);
            out += &format!("{label} => {r}\n");
        }
        out += "\n";
    }
    out
}

#[test]
fn bcround_modes() {
    let mut t = TestVm::new();
    for (mode, src) in [
        ("HalfAwayFromZero", phpt!("bcround_half_up.phpt")),
        ("HalfTowardsZero", phpt!("bcround_half_down.phpt")),
        ("HalfEven", phpt!("bcround_half_even.phpt")),
        ("HalfOdd", phpt!("bcround_half_odd.phpt")),
        ("TowardsZero", phpt!("bcround_toward_zero.phpt")),
        ("AwayFromZero", phpt!("bcround_away_from_zero.phpt")),
        ("PositiveInfinity", phpt!("bcround_ceiling.phpt")),
        ("NegativeInfinity", phpt!("bcround_floor.phpt")),
    ] {
        let out = run_round_test(&mut t, mode);
        assert_eq!(out.trim_end(), expect(src), "mode {mode}");
    }
}

#[test]
fn bcround_all_modes_table() {
    // bcround_all.phpt
    let mut t = TestVm::new();
    let cases =
        t.vm.call(&s("RoundingMode::cases"), vec![])
            .expect("cases()");
    let mut out = String::new();
    for mode in cases.as_array().unwrap().values() {
        let name = crate::random::enums::case_name(mode.as_object().unwrap()).unwrap();
        for number in [
            "1.0", "-1.0", "1.2", "-1.2", "1.7", "-1.7", "1.5", "-1.5", "2.5", "-2.5",
        ] {
            let r = call(&mut t, "bcround", vec![s(number), i(0), mode.clone()]);
            out += &format!("{name:<20}: {number} -> {r}\n");
        }
    }
    assert_eq!(out.trim_end(), expect(phpt!("bcround_all.phpt")));
}

#[test]
fn bcround_early_return() {
    // bcround_early_return.phpt
    let cases: [(&str, i64); 16] = [
        ("123", 1),
        ("123.5", 1),
        ("123.5", 2),
        ("123.0000000000000000000001", 22),
        ("123.0000000000000000000001", 23),
        ("-123", 1),
        ("-123.5", 1),
        ("-123.5", 2),
        ("-123.0000000000000000000001", 22),
        ("-123.0000000000000000000001", 23),
        ("0", 0),
        ("0.0", 0),
        ("0.0000", 0),
        ("-0", 0),
        ("-0.0", 0),
        ("-0.0000", 0),
    ];
    let mut t = TestVm::new();
    let render = |t: &mut TestVm, mode: &Value| -> Vec<String> {
        cases
            .iter()
            .map(|(num, p)| {
                let label = pad_left(&format!("[{num}, {p}]"), 33);
                let r = call(t, "bcround", vec![s(num), i(*p), mode.clone()]);
                format!("{label} => {r}\n")
            })
            .collect()
    };
    let base_mode = mode_case(&mut t, "HalfAwayFromZero");
    let base = render(&mut t, &base_mode);
    let mut out: String = base.concat();
    out += "\n";
    let all = t.vm.call(&s("RoundingMode::cases"), vec![]).unwrap();
    for mode in all.as_array().unwrap().values() {
        let name = crate::random::enums::case_name(mode.as_object().unwrap()).unwrap();
        let same = render(&mut t, mode) == base;
        assert!(same, "{name} differs");
        out += &format!("{name:>24}: result is same to HalfAwayFromZero\n");
    }
    assert_eq!(out.trim_end(), expect(phpt!("bcround_early_return.phpt")));
}

#[test]
fn bcround_precision_bounds() {
    let mut t = TestVm::new();
    let mode = mode_case(&mut t, "HalfAwayFromZero");
    let e = call_err(&mut t, "bcround", vec![s("1"), i(i64::MAX), mode]);
    assert_eq!(
        e,
        "ValueError: bcround(): Argument #2 ($precision) must be between -9223372036854775808 and 2147483647"
    );
}

#[test]
fn bcround_absurd_negative_precision_is_a_fatal_error() {
    // Allocating 10^PHP_INT_MIN digits fails in php-src with or without a memory limit.
    for limit in [None, Some("-1")] {
        let mut t = TestVm::new();
        if let Some(l) = limit {
            t.vm.ini_set_system("memory_limit", l);
        }
        let mode = mode_case(&mut t, "AwayFromZero");
        let r = t.call("bcround", vec![s("1"), i(i64::MIN), mode]);
        assert!(matches!(r, Err(php_runtime::Flow::Fatal)), "{r:?}");
    }
}

#[test]
fn bcround_default_arguments() {
    let mut t = TestVm::new();
    assert_eq!(call(&mut t, "bcround", vec![s("1.5")]), "2");
    assert_eq!(call(&mut t, "bcround", vec![s("1.55"), i(1)]), "1.6");
    assert_eq!(call(&mut t, "bcround", vec![s("1234.5678"), i(-2)]), "1200");
}

// ---------------------------------------------------------------------------
// Division details
// ---------------------------------------------------------------------------

#[test]
fn bcdiv_by_one() {
    // bcdiv_by_one.phpt
    let mut t = TestVm::new();
    let dividends = ["100", "-100"];
    let divisors = ["1.010", "1.01", "1.1", "1", "-1.010", "-1.01", "-1.1", "-1"];
    let mut out = String::new();
    for scale in 0..=3i64 {
        out += &format!("scale: {scale}\n");
        for divisor in divisors {
            for dividend in dividends {
                let quot = call(&mut t, "bcdiv", vec![s(dividend), s(divisor), i(scale)]);
                let width = scale as usize + 2 + usize::from(scale > 0);
                out += &format!(
                    "{} / {} = {}\n",
                    pad_left(dividend, 4),
                    pad_left(divisor, 6),
                    pad_left(&quot, width)
                );
            }
        }
        out += "\n";
    }
    assert_eq!(out.trim_end(), expect(phpt!("bcdiv_by_one.phpt")));
}

#[test]
fn bcdiv_by_pow_10() {
    // bcdiv_by_pow_10.phpt
    let mut t = TestVm::new();
    let dividends = [
        "0.012345",
        "0.12345",
        "1.2345",
        "12.345",
        "123.45",
        "-0.012345",
        "-0.12345",
        "-1.2345",
        "-12.345",
        "-123.45",
    ];
    let divisors = [
        "0.01", "0.1", "1", "10", "100", "-0.01", "-0.1", "-1", "-10", "-100",
    ];
    let mut out = String::new();
    for scale in [0i64, 3, 5] {
        out += &format!("scale: {scale}\n");
        for divisor in divisors {
            for dividend in dividends {
                let quot = call(&mut t, "bcdiv", vec![s(dividend), s(divisor), i(scale)]);
                let width = scale as usize + 7 + usize::from(scale > 0);
                out += &format!(
                    "{} / {} = {}\n",
                    pad_left(dividend, 9),
                    pad_left(divisor, 5),
                    pad_left(&quot, width)
                );
            }
        }
        out += "\n";
    }
    assert_eq!(out.trim_end(), expect(phpt!("bcdiv_by_pow_10.phpt")));
}

#[test]
fn division_regressions() {
    let mut t = TestVm::new();
    // GH-17275
    assert_eq!(
        call(&mut t, "bcdiv", vec![s("0.03772321"), s("9650.0"), i(8)]),
        "0.00000390"
    );
    assert_eq!(
        call(&mut t, "bcdiv", vec![s("0.03772321"), s("9650.0"), i(9)]),
        "0.000003909"
    );
    // GH-16265
    assert_eq!(
        call(
            &mut t,
            "bcdiv",
            vec![s("-0.01"), Value::Float(-12.3456789000e10), i(9)]
        ),
        "0.000000000"
    );
    // bug #66364, #78238
    assert_eq!(
        call(&mut t, "bcmul", vec![s("0.3"), s("0.2"), i(4)]),
        "0.0600"
    );
    assert_eq!(call(&mut t, "bcadd", vec![s("0"), s("-0.1")]), "0");
    assert_eq!(
        call(&mut t, "bcmul", vec![s("0.9"), s("-0.1"), i(1)]),
        "0.0"
    );
    assert_eq!(
        call(&mut t, "bcmul", vec![s("0.90"), s("-0.1"), i(1)]),
        "0.0"
    );
    // bug #46781
    assert_eq!(
        call(&mut t, "bcadd", vec![s("-0.0"), s("-0.0"), i(1)]),
        "0.0"
    );
}

#[test]
fn multiplication_overflow_check() {
    // bcmul_check_overflow.phpt (reduced to 2^9 repetitions)
    let mut t = TestVm::new();
    for e in 1..10u32 {
        let repeat = 2usize.pow(e);
        let num = "99999999".repeat(repeat);
        let expected = format!(
            "{}8{}1",
            "9".repeat(repeat * 8 - 1),
            "0".repeat(repeat * 8 - 1)
        );
        assert_eq!(
            call(&mut t, "bcmul", vec![s(&num), s(&num), i(0)]),
            expected
        );
    }
}

// ---------------------------------------------------------------------------
// sqrt
// ---------------------------------------------------------------------------

#[test]
fn bcsqrt_table() {
    // bcsqrt.phpt
    let mut t = TestVm::new();
    let radicants = [
        "0",
        "0.00",
        "-0",
        "-0.00",
        "15151324141414.412312232141241",
        "141241241241241248267654747412",
        "0.1322135476547459213732911312",
        "14.14",
        "0.15",
        "15",
        "1",
    ];
    let mut out = String::new();
    for scale in [0, 10] {
        for radicant in radicants {
            out += &call(&mut t, "bcsqrt", vec![s(radicant), i(scale)]);
            out += "\n";
        }
    }
    assert_eq!(out.trim_end(), expect(phpt!("bcsqrt.phpt")));
}

// ---------------------------------------------------------------------------
// floor / ceil values
// ---------------------------------------------------------------------------

#[test]
fn floor_ceil_negative_zero() {
    let mut t = TestVm::new();
    assert_eq!(call(&mut t, "bcceil", vec![s("-0.01")]), "0");
    assert_eq!(call(&mut t, "bcfloor", vec![s("-0.01")]), "-1");
}

// ---------------------------------------------------------------------------
// Memory limit
// ---------------------------------------------------------------------------

#[test]
fn huge_scale_exhausts_memory() {
    // GH-17398: bcmul('0', '0', 2147483647)
    let mut t = TestVm::new();
    let r = t.call("bcmul", vec![s("0"), s("0"), i(2147483647)]);
    assert!(matches!(r, Err(php_runtime::Flow::Fatal)), "{r:?}");
    assert!(
        t.output()
            .contains("Allowed memory size of 134217728 bytes exhausted")
    );
}

// ---------------------------------------------------------------------------
// BcMath\Number
// ---------------------------------------------------------------------------

fn number(t: &mut TestVm, v: Value) -> PResult<ObjRef> {
    let ce = t.vm.find_class("BcMath\\Number").expect("class");
    let obj = t.vm.instantiate(&ce)?;
    t.vm.call_method(&obj, b"__construct", vec![v])?;
    Ok(obj)
}

fn num(t: &mut TestVm, v: &str) -> ObjRef {
    number(t, s(v)).expect("number")
}

/// `(value, scale)` of a Number.
fn parts(o: &ObjRef) -> (String, i64) {
    (
        text(&o.get_prop(b"value").expect("value")),
        int(&o.get_prop(b"scale").expect("scale")),
    )
}

fn obj_of(v: &Value) -> ObjRef {
    v.as_object().expect("object").clone()
}

fn method(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    t.vm.call_method(o, name.as_bytes(), args)
}

fn method_num(t: &mut TestVm, o: &ObjRef, name: &str, args: Vec<Value>) -> (String, i64) {
    let r = method(t, o, name, args).unwrap_or_else(|e| panic!("{name} failed: {e:?}"));
    parts(&obj_of(&r))
}

/// `(value, scale)` pairs of every `var_dump()` of a Number in `expect`.
fn dumped_numbers(expect: &str) -> Vec<(String, i64)> {
    let lines: Vec<&str> = expect.lines().collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < lines.len() {
        if lines[k].trim() == "[\"value\"]=>" {
            let v = lines[k + 1].trim();
            let value = &v[v.find('"').unwrap() + 1..v.rfind('"').unwrap()];
            let sc = lines[k + 3].trim();
            let scale: i64 = sc
                .strip_prefix("int(")
                .unwrap()
                .trim_end_matches(')')
                .parse()
                .unwrap();
            out.push((value.to_string(), scale));
            k += 4;
        } else {
            k += 1;
        }
    }
    out
}

#[derive(Clone)]
enum Arg {
    Int(i64),
    Str(&'static str),
    Obj(&'static str),
}

fn arg_value(t: &mut TestVm, a: &Arg) -> Value {
    match a {
        Arg::Int(n) => i(*n),
        Arg::Str(x) => s(x),
        Arg::Obj(x) => Value::Object(num(t, x)),
    }
}

fn arg_string(a: &Arg) -> String {
    match a {
        Arg::Int(n) => n.to_string(),
        Arg::Str(x) | Arg::Obj(x) => (*x).to_string(),
    }
}

fn std_second_args() -> Vec<Arg> {
    vec![
        Arg::Int(100),
        Arg::Int(-30),
        Arg::Str("-20"),
        Arg::Str("0.01"),
        Arg::Str("-0.40"),
        Arg::Obj("80.3"),
        Arg::Obj("-50.6"),
    ]
}

#[test]
fn number_construct_and_properties() {
    // construct_64bit.phpt, properties_read.phpt
    let mut t = TestVm::new();
    let cases: [(Value, &str, i64); 17] = [
        (s("0"), "0", 0),
        (s("0.00"), "0.00", 2),
        (s(".1"), "0.1", 1),
        (s("0.1"), "0.1", 1),
        (s("0.20"), "0.20", 2),
        (s("0.00000030"), "0.00000030", 8),
        (s("1234.0"), "1234.0", 1),
        (s("123450"), "123450", 0),
        (s(""), "0", 0),
        (s("-"), "0", 0),
        (s("."), "0", 0),
        (i(0), "0", 0),
        (i(123), "123", 0),
        (i(9223372036854775806), "9223372036854775806", 0),
        (i(i64::MAX), "9223372036854775807", 0),
        (i(-9223372036854775807), "-9223372036854775807", 0),
        (i(i64::MIN), "-9223372036854775808", 0),
    ];
    for (arg, value, scale) in cases {
        let o = number(&mut t, arg).expect("construct");
        assert_eq!(parts(&o), (value.to_string(), scale));
    }
    for (arg, value, scale) in [
        ("12.0004", "12.0004", 4),
        ("0.1230", "0.1230", 4),
        ("1234", "1234", 0),
    ] {
        assert_eq!(parts(&num(&mut t, arg)), (value.to_string(), scale));
    }
}

#[test]
fn number_construct_errors() {
    // construct_error.phpt
    let mut t = TestVm::new();
    let o = num(&mut t, "1");
    let r = method(&mut t, &o, "__construct", vec![i(5)]);
    assert_eq!(
        err(r),
        "Error: Cannot modify readonly property BcMath\\Number::$value"
    );
    let r = number(&mut t, s("a")).map(Value::Object);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::__construct(): Argument #1 ($num) is not well-formed"
    );
}

#[test]
fn number_construct_float_is_truncated_with_deprecation() {
    // construct_float.phpt
    let mut t = TestVm::new();
    let o = number(&mut t, Value::Float(0.1234)).unwrap();
    assert_eq!(parts(&o), ("0".to_string(), 0));
    assert!(
        t.output()
            .contains("Deprecated: Implicit conversion from float 0.1234 to int loses precision")
    );
}

#[test]
fn number_is_final_readonly_stringable() {
    let t = TestVm::new();
    let ce = t.vm.find_class("BcMath\\Number").unwrap();
    assert!(ce.has_flag(php_runtime::class::class_flags::FINAL));
    assert!(ce.has_flag(php_runtime::class::class_flags::READONLY));
    assert!(ce.instance_of_name(b"Stringable"));
    assert!(ce.find_property(b"value").unwrap().is_readonly());
    assert!(ce.find_property(b"scale").unwrap().is_readonly());
}

#[test]
fn number_cast_and_to_string() {
    // cast.phpt
    let mut t = TestVm::new();
    for (v, truthy) in [
        ("0", false),
        ("0.0", false),
        ("2", true),
        ("1234", true),
        ("12.0004", true),
        ("0.1230", true),
        ("-0", false),
        ("-0.0", false),
        ("-2", true),
        ("-12.0004", true),
        ("-0.1230", true),
    ] {
        let o = num(&mut t, v);
        let cast = Value::Object(o.clone());
        assert_eq!(t.vm.to_bool(&cast).unwrap(), truthy, "(bool) {v}");
        let st = method(&mut t, &o, "__toString", vec![]).unwrap();
        assert_eq!(text(&st), parts(&o).0);
        let st =
            t.vm.object_cast(&o, php_runtime::class::CastKind::String)
                .unwrap()
                .unwrap();
        assert_eq!(text(&st), parts(&o).0);
    }
    // (int)/(float) casts are unsupported (the engine warns).
    let o = num(&mut t, "1");
    assert!(
        t.vm.object_cast(&o, php_runtime::class::CastKind::Int)
            .unwrap()
            .is_none()
    );
    assert!(
        t.vm.object_cast(&o, php_runtime::class::CastKind::Float)
            .unwrap()
            .is_none()
    );
}

#[test]
fn number_clone() {
    let mut t = TestVm::new();
    for v in ["0", "0.0", "2", "12.0004", "-0", "-0.1230", "-12345"] {
        let o = num(&mut t, v);
        let clone = (o.class().handlers.clone_obj.unwrap())(&mut t.vm, &o).unwrap();
        assert_eq!(parts(&o), parts(&clone));
        assert!(!ObjRef::ptr_eq(&o, &clone));
        // The clone is a fully working number.
        let sum = method_num(&mut t, &clone, "add", vec![i(1)]);
        assert_eq!(sum.1, parts(&o).1.max(0));
    }
}

#[test]
fn number_serialize_and_unserialize() {
    let mut t = TestVm::new();
    // serialize.phpt: __serialize() carries only the value.
    for v in ["0", "0.0", "2", "1234", "12.0004", "0.1230"] {
        let o = num(&mut t, v);
        let data = method(&mut t, &o, "__serialize", vec![]).unwrap();
        let arr = data.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(text(arr.get_str(b"value").unwrap()), v);

        // unserialize.phpt
        let ce = t.vm.find_class("BcMath\\Number").unwrap();
        let fresh = t.vm.instantiate(&ce).unwrap();
        method(&mut t, &fresh, "__unserialize", vec![data.clone()]).unwrap();
        assert_eq!(parts(&fresh), parts(&o));
    }
    // unserialize_error.phpt
    let o = num(&mut t, "1");
    let mut arr = php_runtime::PhpArray::new();
    arr.insert_str("value", s("5"));
    let r = method(&mut t, &o, "__unserialize", vec![Value::array(arr)]);
    assert_eq!(
        err(r),
        "Error: Cannot modify readonly property BcMath\\Number::$value"
    );

    let bad: [Vec<(&str, Value)>; 4] = [
        vec![("value", s("a"))],
        vec![("value", s(""))],
        vec![],
        vec![("value", i(1))],
    ];
    for entries in bad {
        let ce = t.vm.find_class("BcMath\\Number").unwrap();
        let fresh = t.vm.instantiate(&ce).unwrap();
        let mut arr = php_runtime::PhpArray::new();
        for (k, v) in entries {
            arr.insert_str(k, v);
        }
        let r = method(&mut t, &fresh, "__unserialize", vec![Value::array(arr)]);
        assert_eq!(
            err(r),
            "Exception: Invalid serialization data for BcMath\\Number object"
        );
    }
}

#[test]
fn number_arithmetic_methods_match_expected_dumps() {
    // methods/{add,sub,mul,div,mod}.phpt
    let mut t = TestVm::new();
    for (name, src) in [
        ("add", phpt!("number/methods/add.phpt")),
        ("sub", phpt!("number/methods/sub.phpt")),
        ("mul", phpt!("number/methods/mul.phpt")),
        ("div", phpt!("number/methods/div.phpt")),
        ("mod", phpt!("number/methods/mod.phpt")),
    ] {
        let mut got = Vec::new();
        for first in ["100.012", "-100.012"] {
            let o = num(&mut t, first);
            for a in std_second_args() {
                let v = arg_value(&mut t, &a);
                got.push(method_num(&mut t, &o, name, vec![v]));
            }
        }
        assert_eq!(got, dumped_numbers(expect(src)), "{name}");
    }
}

#[test]
fn number_pow_method() {
    // methods/pow.phpt
    let mut t = TestVm::new();
    let exps = vec![
        Arg::Int(2),
        Arg::Int(-3),
        Arg::Str("-2"),
        Arg::Str("0"),
        Arg::Obj("2"),
        Arg::Obj("-2"),
        Arg::Obj("0"),
    ];
    let mut got = Vec::new();
    for base in ["12.5", "-12.5"] {
        let o = num(&mut t, base);
        for a in &exps {
            let v = arg_value(&mut t, a);
            got.push(method_num(&mut t, &o, "pow", vec![v]));
        }
    }
    assert_eq!(
        got,
        dumped_numbers(expect(phpt!("number/methods/pow.phpt")))
    );
}

/// Calls the function form and the method form and compares the results.
#[test]
fn number_methods_agree_with_functions() {
    let mut t = TestVm::new();
    let scales: Vec<i64> = (0..=10).collect();
    let second = {
        let mut v = std_second_args();
        v.push(Arg::Str("100.012"));
        v.push(Arg::Str("-100.012"));
        v
    };
    for (m, f) in [
        ("add", "bcadd"),
        ("sub", "bcsub"),
        ("mul", "bcmul"),
        ("div", "bcdiv"),
        ("mod", "bcmod"),
    ] {
        for &scale in &scales {
            for first in ["100.012", "-100.012"] {
                let o = num(&mut t, first);
                for a in &second {
                    let v = arg_value(&mut t, a);
                    let func = call(&mut t, f, vec![s(first), s(&arg_string(a)), i(scale)]);
                    let (mv, ms) = method_num(&mut t, &o, m, vec![v, i(scale)]);
                    assert_eq!(ms, scale);
                    let result = num(&mut t, &mv);
                    let cmp = int(&method(&mut t, &result, "compare", vec![s(&func)]).unwrap());
                    assert_eq!(
                        cmp,
                        0,
                        "{first} {m} {} scale {scale}: {mv} vs {func}",
                        arg_string(a)
                    );
                }
            }
        }
    }
    // divmod
    for &scale in &scales {
        for first in ["100.012", "-100.012"] {
            let o = num(&mut t, first);
            for a in &std_second_args() {
                let v = arg_value(&mut t, a);
                let func = t
                    .call("bcdivmod", vec![s(first), s(&arg_string(a)), i(scale)])
                    .unwrap();
                let func = func.as_array().unwrap().clone();
                let r = method(&mut t, &o, "divmod", vec![v, i(scale)]).unwrap();
                let pair = r.as_array().unwrap().clone();
                for (k, name) in [(0, "quot"), (1, "rem")] {
                    let n = obj_of(pair.get_int(k).unwrap());
                    let cmp = int(&method(
                        &mut t,
                        &n,
                        "compare",
                        vec![func.get_int(k).unwrap().clone()],
                    )
                    .unwrap());
                    assert_eq!(
                        cmp,
                        0,
                        "{name} of {first} divmod {} scale {scale}",
                        arg_string(a)
                    );
                }
            }
        }
    }
    // powmod and sqrt
    for scale in [0, 10] {
        for value in ["12", "-12"] {
            let o = num(&mut t, value);
            for e in [
                Arg::Int(2),
                Arg::Str("0"),
                Arg::Str("3"),
                Arg::Obj("2"),
                Arg::Obj("0"),
            ] {
                for m in [Arg::Int(2), Arg::Str("3"), Arg::Obj("2")] {
                    let (ev, mv) = (arg_value(&mut t, &e), arg_value(&mut t, &m));
                    let func = call(
                        &mut t,
                        "bcpowmod",
                        vec![s(value), s(&arg_string(&e)), s(&arg_string(&m)), i(scale)],
                    );
                    let r = method(&mut t, &o, "powmod", vec![ev, mv, i(scale)]).unwrap();
                    let cmp = int(&method(&mut t, &obj_of(&r), "compare", vec![s(&func)]).unwrap());
                    assert_eq!(cmp, 0);
                }
            }
        }
        for radicant in [
            "0",
            "0.00",
            "15151324141414.412312232141241",
            "0.15",
            "14.14",
            "1",
        ] {
            let func = call(&mut t, "bcsqrt", vec![s(radicant), i(scale)]);
            let o = num(&mut t, radicant);
            let r = method(&mut t, &o, "sqrt", vec![i(scale)]).unwrap();
            let cmp = int(&method(&mut t, &obj_of(&r), "compare", vec![s(&func)]).unwrap());
            assert_eq!(cmp, 0, "sqrt {radicant}");
        }
    }
    // round
    let all = t.vm.call(&s("RoundingMode::cases"), vec![]).unwrap();
    for mode in all.as_array().unwrap().values() {
        for number in [
            "0",
            "0.1",
            "-0.1",
            "1.0",
            "-1.0",
            "1.2",
            "-1.2",
            "1.7",
            "-1.7",
            "1.5",
            "-1.5",
            "2.5",
            "-2.5",
            "1.2345678",
            "-1.2345678",
        ] {
            for p in [0, 5, -5] {
                let func = call(&mut t, "bcround", vec![s(number), i(p), mode.clone()]);
                let o = num(&mut t, number);
                let r = method(&mut t, &o, "round", vec![i(p), mode.clone()]).unwrap();
                let cmp = int(&method(&mut t, &obj_of(&r), "compare", vec![s(&func)]).unwrap());
                assert_eq!(cmp, 0, "round {number} {p}");
            }
        }
    }
}

#[test]
fn number_div_scale_extension() {
    // GH-16262 (the Number half): the scale grows up to 10 digits.
    let mut t = TestVm::new();
    let one = num(&mut t, "1");
    assert_eq!(
        method_num(&mut t, &one, "div", vec![s("1000")]),
        ("0.001".into(), 3)
    );
    assert_eq!(
        method_num(&mut t, &one, "div", vec![s("2000")]),
        ("0.0005".into(), 4)
    );
    for (scale, d1000, d2000) in [
        (0, "0", "0"),
        (1, "0.0", "0.0"),
        (2, "0.00", "0.00"),
        (3, "0.001", "0.000"),
        (4, "0.0010", "0.0005"),
        (5, "0.00100", "0.00050"),
    ] {
        assert_eq!(
            method_num(&mut t, &one, "div", vec![s("1000"), i(scale)]),
            (d1000.into(), scale)
        );
        assert_eq!(
            method_num(&mut t, &one, "div", vec![s("2000"), i(scale)]),
            (d2000.into(), scale)
        );
    }
    let third = method_num(&mut t, &one, "div", vec![i(3)]);
    assert_eq!(third, ("0.3333333333".into(), 10));
}

#[test]
fn number_zero_result_sign() {
    // methods/calc_methods_zero_result_sign.phpt
    let mut t = TestVm::new();
    let cases: [(&str, &str, Arg, i64); 8] = [
        ("div", "-0.001", Arg::Str("1"), 0),
        ("div", "0.001", Arg::Str("-1"), 0),
        ("div", "-0.001", Arg::Str("10"), 2),
        ("div", "-0.001", Arg::Str("0.1"), 1),
        ("add", "-0.001", Arg::Str("-0.001"), 0),
        ("sub", "-0.001", Arg::Str("0.001"), 0),
        ("mul", "-0.001", Arg::Str("1"), 0),
        ("pow", "-0.1", Arg::Int(3), 2),
    ];
    for (m, n, a, scale) in cases {
        let o = num(&mut t, n);
        let v = arg_value(&mut t, &a);
        let r = method(&mut t, &o, m, vec![v, i(scale)]).unwrap();
        let r = obj_of(&r);
        let (value, _) = parts(&r);
        assert!(!value.starts_with('-'), "{n} {m}: {value}");
        assert_eq!(int(&method(&mut t, &r, "compare", vec![i(0)]).unwrap()), 0);
        assert!(!t.vm.to_bool(&Value::Object(r)).unwrap());
    }
    let o = num(&mut t, "-0.001");
    let r = method(&mut t, &o, "divmod", vec![s("1"), i(0)]).unwrap();
    let quot = obj_of(r.as_array().unwrap().get_int(0).unwrap());
    assert_eq!(parts(&quot).0, "0");
}

#[test]
fn number_compare_method() {
    // methods/compare.phpt, compare_near_zero.phpt, bc_is_zero_for_scale_clamp.phpt
    let mut t = TestVm::new();
    let v = num(&mut t, "100.0000");
    for (arg, expect) in [
        (s("99.9999"), 1),
        (i(99), 1),
        (s("100"), 0),
        (s("100.0000"), 0),
        (i(100), 0),
        (s("100.00001"), -1),
        (i(101), -1),
    ] {
        assert_eq!(
            int(&method(&mut t, &v, "compare", vec![arg]).unwrap()),
            expect
        );
    }
    // With an explicit scale the comparison truncates.
    let small = num(&mut t, "0.001");
    for (scale, other, expect) in [
        (0, "0.0011", 0),
        (2, "0.0011", 0),
        (3, "0.0011", 0),
        (4, "0.0011", -1),
        (4, "-0.0011", 1),
    ] {
        assert_eq!(
            int(&method(&mut t, &small, "compare", vec![s(other), i(scale)]).unwrap()),
            expect,
            "{other} at {scale}"
        );
    }
    let a = num(&mut t, "1.0");
    let short_zero = method(&mut t, &a, "sub", vec![s("1.0")]).unwrap();
    let long_negative = num(&mut t, &format!("-0.{}1", "0".repeat(64)));
    let r = method(
        &mut t,
        &obj_of(&short_zero),
        "compare",
        vec![Value::Object(long_negative), i(64)],
    )
    .unwrap();
    assert_eq!(int(&r), 0);
}

#[test]
fn number_floor_ceil_round() {
    let mut t = TestVm::new();
    let o = num(&mut t, "-1.5");
    assert_eq!(method_num(&mut t, &o, "floor", vec![]), ("-2".into(), 0));
    assert_eq!(method_num(&mut t, &o, "ceil", vec![]), ("-1".into(), 0));
    assert_eq!(method_num(&mut t, &o, "round", vec![]), ("-2".into(), 0));
    // methods/round_has_trailing_zeros.phpt: value keeps the rounded scale.
    let o = num(&mut t, "1.5");
    assert_eq!(
        method_num(&mut t, &o, "round", vec![i(3)]),
        ("1.500".into(), 3)
    );
    let mode = mode_case(&mut t, "HalfEven");
    assert_eq!(
        method_num(&mut t, &o, "round", vec![i(0), mode]),
        ("2".into(), 0)
    );
    // bcround_precision_bounds
    let r = method(&mut t, &o, "round", vec![i(2147483648)]);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::round(): Argument #1 ($precision) must be between -9223372036854775808 and 2147483647"
    );
}

#[test]
fn number_sqrt_and_powmod_errors() {
    let mut t = TestVm::new();
    let o = num(&mut t, "-4");
    let r = method(&mut t, &o, "sqrt", vec![]);
    assert_eq!(
        err(r),
        "ValueError: Base number must be greater than or equal to 0"
    );
    let four = num(&mut t, "4");
    assert_eq!(method_num(&mut t, &four, "sqrt", vec![]), ("2".into(), 0));
    let two = num(&mut t, "2");
    let (value, scale) = method_num(&mut t, &two, "sqrt", vec![]);
    assert_eq!((value.as_str(), scale), ("1.4142135623", 10));

    let base = num(&mut t, "4.5");
    let r = method(&mut t, &base, "powmod", vec![i(2), i(3)]);
    assert_eq!(
        err(r),
        "ValueError: Base number cannot have a fractional part"
    );
    let base = num(&mut t, "4");
    let r = method(&mut t, &base, "powmod", vec![s("1.5"), i(3)]);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::powmod(): Argument #1 ($exponent) cannot have a fractional part"
    );
    let r = method(&mut t, &base, "powmod", vec![i(-1), i(3)]);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::powmod(): Argument #1 ($exponent) must be greater than or equal to 0"
    );
    let r = method(&mut t, &base, "powmod", vec![i(2), s("3.5")]);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::powmod(): Argument #2 ($modulus) cannot have a fractional part"
    );
    let r = method(&mut t, &base, "powmod", vec![i(2), i(0)]);
    assert_eq!(err(r), "DivisionByZeroError: Modulo by zero");
}

#[test]
fn number_method_argument_errors() {
    // methods/calc_methods_num_arg_error.phpt (the parts the runtime formats itself)
    let mut t = TestVm::new();
    let o = num(&mut t, "100.0000");
    for (m, arg_name) in [
        ("add", "num"),
        ("sub", "num"),
        ("mul", "num"),
        ("div", "num"),
        ("mod", "num"),
        ("pow", "exponent"),
        ("compare", "num"),
    ] {
        let r = method(&mut t, &o, m, vec![s("a")]);
        assert_eq!(
            err(r),
            format!(
                "ValueError: BcMath\\Number::{m}(): Argument #1 (${arg_name}) is not well-formed"
            )
        );
    }
    for m in ["add", "sub", "mul", "div", "mod", "pow", "compare"] {
        let r = method(&mut t, &o, m, vec![i(1), i(-1)]);
        assert_eq!(
            err(r),
            format!(
                "ValueError: BcMath\\Number::{m}(): Argument #2 ($scale) must be between 0 and 2147483647"
            )
        );
    }
    let r = method(&mut t, &o, "div", vec![i(0)]);
    assert_eq!(err(r), "DivisionByZeroError: Division by zero");
    let r = method(&mut t, &o, "mod", vec![i(0)]);
    assert_eq!(err(r), "DivisionByZeroError: Modulo by zero");
    let r = method(&mut t, &o, "divmod", vec![i(0)]);
    assert_eq!(err(r), "DivisionByZeroError: Division by zero");
    let zero = num(&mut t, "0");
    let r = method(&mut t, &zero, "pow", vec![i(-1)]);
    assert_eq!(err(r), "DivisionByZeroError: Negative power of zero");
    let r = method(&mut t, &o, "pow", vec![s("1.5")]);
    assert_eq!(
        err(r),
        "ValueError: BcMath\\Number::pow(): Argument #1 ($exponent) exponent cannot have a fractional part"
    );
}

#[test]
fn number_divmod_powmod_sqrt_dumps() {
    // methods/divmod.phpt
    let mut t = TestVm::new();
    let mut got = Vec::new();
    for first in ["100.012", "-100.012"] {
        let o = num(&mut t, first);
        for a in std_second_args() {
            let v = arg_value(&mut t, &a);
            let r = method(&mut t, &o, "divmod", vec![v]).unwrap();
            let pair = r.as_array().unwrap().clone();
            got.push(parts(&obj_of(pair.get_int(0).unwrap())));
            got.push(parts(&obj_of(pair.get_int(1).unwrap())));
        }
    }
    assert_eq!(
        got,
        dumped_numbers(expect(phpt!("number/methods/divmod.phpt")))
    );

    // methods/powmod.phpt
    let mut got = Vec::new();
    for value in ["12", "-12"] {
        let o = num(&mut t, value);
        for e in [
            Arg::Int(2),
            Arg::Str("0"),
            Arg::Str("3"),
            Arg::Obj("2"),
            Arg::Obj("0"),
        ] {
            for m in [Arg::Int(2), Arg::Str("3"), Arg::Obj("2")] {
                let (ev, mv) = (arg_value(&mut t, &e), arg_value(&mut t, &m));
                got.push(method_num(&mut t, &o, "powmod", vec![ev, mv]));
            }
        }
    }
    assert_eq!(
        got,
        dumped_numbers(expect(phpt!("number/methods/powmod.phpt")))
    );

    // methods/sqrt.phpt
    let mut got = Vec::new();
    for radicant in [
        "15151324141414.412312232141241",
        "141241241241241248267654747412",
        "0.1322135476547459213732911312",
    ] {
        let o = num(&mut t, radicant);
        got.push(method_num(&mut t, &o, "sqrt", vec![]));
    }
    assert_eq!(
        got,
        dumped_numbers(expect(phpt!("number/methods/sqrt.phpt")))
    );
}

#[test]
fn number_pow_and_floor_ceil_agree_with_functions() {
    let mut t = TestVm::new();
    // methods/pow_with_scale.phpt
    for scale in 0..=10 {
        for value in ["12.5", "-12.5"] {
            let o = num(&mut t, value);
            for e in [
                Arg::Int(2),
                Arg::Int(-3),
                Arg::Str("-2"),
                Arg::Str("0"),
                Arg::Obj("2"),
                Arg::Obj("-2"),
                Arg::Obj("0"),
            ] {
                let v = arg_value(&mut t, &e);
                let func = call(
                    &mut t,
                    "bcpow",
                    vec![s(value), s(&arg_string(&e)), i(scale)],
                );
                let r = method(&mut t, &o, "pow", vec![v, i(scale)]).unwrap();
                let cmp = int(&method(&mut t, &obj_of(&r), "compare", vec![s(&func)]).unwrap());
                assert_eq!(cmp, 0, "{value} ** {} at {scale}", arg_string(&e));
            }
        }
    }
    // methods/floor.phpt, ceil.phpt
    for n in [
        "0",
        "0.00",
        "-0",
        "-0.00",
        "0.01",
        "0.000000000000000000000000000000000000000001",
        "-0.01",
        "-0.000000000000000000000000000000000000000001",
        "1",
        "1.0000",
        "1.0001",
        "100000.000000000000000000000000000000000000000001",
        "-1",
        "-1.0000",
        "-1.0001",
        "-100000.000000000000000000000000000000000000000001",
    ] {
        for (m, f) in [("floor", "bcfloor"), ("ceil", "bcceil")] {
            let func = call(&mut t, f, vec![s(n)]);
            let o = num(&mut t, n);
            let (value, scale) = method_num(&mut t, &o, m, vec![]);
            assert_eq!((value, scale), (func, 0), "{m} {n}");
        }
    }
}

#[test]
fn number_division_by_zero_variants() {
    // methods/div_mod_by_zero.phpt
    let mut t = TestVm::new();
    for first in ["100", "-100"] {
        let o = num(&mut t, first);
        let zeros = [
            i(0),
            s("0"),
            s("-0"),
            s("0.000"),
            s("-0.000"),
            Value::Object(num(&mut t, "0")),
            Value::Object(num(&mut t, "-0")),
            Value::Object(num(&mut t, "0.000")),
            Value::Object(num(&mut t, "-0.000")),
        ];
        for z in zeros {
            let r = method(&mut t, &o, "div", vec![z.clone()]);
            assert_eq!(err(r), "DivisionByZeroError: Division by zero");
            let r = method(&mut t, &o, "mod", vec![z.clone()]);
            assert_eq!(err(r), "DivisionByZeroError: Modulo by zero");
            let r = method(&mut t, &o, "divmod", vec![z]);
            assert_eq!(err(r), "DivisionByZeroError: Division by zero");
        }
    }
    // methods/pow_div_by_zero.phpt
    for base in [i(0), s("0")] {
        let o = number(&mut t, base).unwrap();
        for e in [i(-3), s("-2"), Value::Object(num(&mut t, "-2"))] {
            let r = method(&mut t, &o, "pow", vec![e]);
            assert_eq!(err(r), "DivisionByZeroError: Negative power of zero");
        }
    }
}

// ----- operators -----------------------------------------------------------

fn op(t: &mut TestVm, which: &str, a: &Value, b: &Value) -> PResult<Value> {
    match which {
        "+" => t.vm.add(a, b),
        "-" => t.vm.sub(a, b),
        "*" => t.vm.mul(a, b),
        "/" => t.vm.div(a, b),
        "%" => t.vm.modulo(a, b),
        "**" => t.vm.pow(a, b),
        _ => unreachable!(),
    }
}

#[test]
fn number_operators() {
    let mut t = TestVm::new();
    // add_int.phpt and friends: the operator equals the method with automatic scale.
    for (which, m) in [
        ("+", "add"),
        ("-", "sub"),
        ("*", "mul"),
        ("/", "div"),
        ("%", "mod"),
    ] {
        for first in [i(100), s("-20")] {
            let a = number(&mut t, first.clone()).unwrap();
            for second in [i(100), s("-20"), s("0.25")] {
                let b = number(&mut t, second.clone()).unwrap();
                let expect_method = method_num(&mut t, &a, m, vec![Value::Object(b.clone())]);
                // Number op Number
                let r = op(
                    &mut t,
                    which,
                    &Value::Object(a.clone()),
                    &Value::Object(b.clone()),
                )
                .unwrap();
                assert_eq!(parts(&obj_of(&r)), expect_method, "{which}");
                // Number op int/string
                let r = op(&mut t, which, &Value::Object(a.clone()), &second).unwrap();
                assert_eq!(parts(&obj_of(&r)), expect_method, "{which} rhs");
                // int/string op Number
                let lhs = number(&mut t, first.clone()).unwrap();
                let left = method_num(&mut t, &lhs, m, vec![Value::Object(b.clone())]);
                let r = op(&mut t, which, &first, &Value::Object(b.clone())).unwrap();
                assert_eq!(parts(&obj_of(&r)), left, "{which} lhs");
            }
        }
    }
    // pow_int.phpt
    for (base, exp, value, scale) in [
        (100, 2, "10000", 0),
        (100, 0, "1", 0),
        (100, -1, "0.01", 2),
        (3, -2, "0.1111111111", 10),
    ] {
        let a = number(&mut t, i(base)).unwrap();
        let r = op(&mut t, "**", &Value::Object(a), &i(exp)).unwrap();
        assert_eq!(
            parts(&obj_of(&r)),
            (value.to_string(), scale),
            "{base} ** {exp}"
        );
    }
    // increment / decrement behave like `+ 1` and `- 1`.
    let a = num(&mut t, "-0.40");
    let r = op(&mut t, "+", &Value::Object(a.clone()), &i(1)).unwrap();
    assert_eq!(parts(&obj_of(&r)), ("0.60".into(), 2));
    let r = op(&mut t, "-", &Value::Object(a), &i(1)).unwrap();
    assert_eq!(parts(&obj_of(&r)), ("-1.40".into(), 2));
    // compound assignments (compound_assignment.phpt)
    let mut v = Value::Object(number(&mut t, i(100)).unwrap());
    v = op(&mut t, "/", &v, &i(100)).unwrap();
    assert_eq!(parts(&obj_of(&v)).0, "1");
    v = op(&mut t, "/", &v, &s("50")).unwrap();
    assert_eq!(parts(&obj_of(&v)).0, "0.02");
    let thirty = Value::Object(number(&mut t, i(30)).unwrap());
    v = op(&mut t, "/", &v, &thirty).unwrap();
    assert_eq!(parts(&obj_of(&v)).0, "0.000666666666");
}

#[test]
fn number_operator_errors() {
    let mut t = TestVm::new();
    let a = Value::Object(number(&mut t, i(100)).unwrap());
    // calc_non_numeric_string.phpt
    for which in ["+", "-", "*", "/", "%", "**"] {
        let r = op(&mut t, which, &a, &s("a"));
        assert_eq!(
            err(r),
            "ValueError: Right string operand cannot be converted to BcMath\\Number"
        );
        let r = op(&mut t, which, &s("a"), &a);
        if which == "%" {
            // mod_function converts a non-numeric left operand before the object is asked.
            assert_eq!(
                err(r),
                "TypeError: Unsupported operand types: string % BcMath\\Number"
            );
        } else {
            assert_eq!(
                err(r),
                "ValueError: Left string operand cannot be converted to BcMath\\Number"
            );
        }
    }
    // div_by_zero.phpt, mod_by_zero.phpt
    let zero = Value::Object(number(&mut t, i(0)).unwrap());
    for rhs in [i(0), s("0"), zero.clone()] {
        let r = op(&mut t, "/", &a, &rhs);
        assert_eq!(err(r), "DivisionByZeroError: Division by zero");
        let r = op(&mut t, "%", &a, &rhs);
        assert_eq!(err(r), "DivisionByZeroError: Modulo by zero");
    }
    // pow_div_by_zero.phpt
    for rhs in [i(-3), s("-2")] {
        let r = op(&mut t, "**", &zero, &rhs);
        assert_eq!(err(r), "DivisionByZeroError: Negative power of zero");
    }
    // GH-20006
    let r = op(&mut t, "**", &zero, &i(2)).unwrap();
    assert_eq!(parts(&obj_of(&r)), ("0".into(), 0));
    // Unsupported operand types are left to the engine (`FAILURE`).
    let handler = obj_of(&a).class().handlers.do_operation.unwrap();
    let arr = Value::array(php_runtime::PhpArray::new());
    let std_class = {
        let ce = t.vm.find_class("stdClass");
        ce.map(|ce| Value::Object(ObjRef::new(ce)))
    };
    for rhs in [Value::Null, arr].into_iter().chain(std_class) {
        assert!(
            handler(&mut t.vm, php_runtime::class::OpKind::Add, &a, &rhs)
                .unwrap()
                .is_none()
        );
    }
    // Other operators are not handled.
    assert!(
        handler(&mut t.vm, php_runtime::class::OpKind::Concat, &a, &i(1))
            .unwrap()
            .is_none()
    );
    // calc_float.phpt: a float operand is converted with a deprecation.
    let r = op(&mut t, "+", &a, &Value::Float(1.01)).unwrap();
    assert_eq!(parts(&obj_of(&r)), ("101".into(), 0));
    assert!(
        t.output()
            .contains("Implicit conversion from float 1.01 to int loses precision")
    );
}

#[test]
fn number_comparison() {
    // operators/compare.phpt, compare_with_invalid_types.phpt
    let mut t = TestVm::new();
    let v1 = Value::Object(num(&mut t, "100.0000"));
    let cases = [
        (i(99), 1),
        (s("99.9999"), 1),
        (Value::Object(num(&mut t, "99.9999")), 1),
        (i(100), 0),
        (s("100"), 0),
        (s("100.0000"), 0),
        (Value::Object(num(&mut t, "100")), 0),
        (Value::Object(num(&mut t, "100.0000")), 0),
        (i(101), -1),
        (s("100.00001"), -1),
        (Value::Object(num(&mut t, "100.00001")), -1),
    ];
    for (other, expect) in cases {
        assert_eq!(t.vm.compare(&v1, &other).unwrap(), expect);
        assert_eq!(t.vm.compare(&other, &v1).unwrap(), -expect);
        assert_eq!(t.vm.is_equal(&v1, &other).unwrap(), expect == 0);
    }
    // Operands that cannot be compared are never equal and compare as uncomparable (1).
    for other in [s("string"), Value::array(php_runtime::PhpArray::new())] {
        assert_eq!(t.vm.compare(&v1, &other).unwrap(), 1);
        assert!(!t.vm.is_equal(&v1, &other).unwrap());
    }
    assert_eq!(t.vm.compare(&v1, &Value::Null).unwrap(), 1);
    assert_eq!(t.vm.compare(&Value::Null, &v1).unwrap(), -1);
}
