//! Operators (`zend_operators.c`), expectations from php-src `.phpt` files
//! (`Zend/tests/numeric_strings/*`, `add_*.phpt`, `int_overflow_*.phpt`, ...).

use super::*;
use crate::value::PhpArray;
use crate::value::ops::{increment_string, is_identical};

fn int(v: Value) -> i64 {
    match v {
        Value::Int(i) => i,
        other => panic!("expected int, got {other:?}"),
    }
}

fn float(v: Value) -> f64 {
    match v {
        Value::Float(d) => d,
        other => panic!("expected float, got {other:?}"),
    }
}

fn arr(v: &[i64]) -> Value {
    Value::array(PhpArray::from_values(
        v.iter().map(|i| Value::Int(*i)).collect(),
    ))
}

#[test]
fn addition_and_overflow() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(int(vm.add(&1.into(), &1.into()).unwrap()), 2);
    assert_eq!(
        float(vm.add(&i64::MAX.into(), &1.into()).unwrap()),
        9223372036854775808.0
    );
    assert_eq!(
        float(vm.sub(&i64::MIN.into(), &1.into()).unwrap()),
        -9223372036854775809.0
    );
    assert_eq!(
        float(vm.mul(&i64::MAX.into(), &2.into()).unwrap()),
        18446744073709551614.0
    );
    assert_eq!(int(vm.add(&s("1"), &s("1")).unwrap()), 2);
    assert_eq!(float(vm.add(&s("1.5"), &1.into()).unwrap()), 2.5);
    assert_eq!(int(vm.add(&Value::Null, &1.into()).unwrap()), 1);
    assert_eq!(int(vm.add(&true.into(), &1.into()).unwrap()), 2);
    assert_eq!(float(vm.add(&1.into(), &0.5.into()).unwrap()), 1.5);
    assert_eq!(int(vm.add(&s(" 1"), &s("1 ")).unwrap()), 2);
    assert_eq!(t.output(), "");
}

#[test]
fn non_numeric_operands() {
    let mut t = TestVm::new();
    assert_eq!(int(t.vm.add(&s("2 Lorem"), &s("3 ipsum")).unwrap()), 5);
    assert_eq!(
        t.output(),
        "\nWarning: A non-numeric value encountered in /t.php on line 3\n\nWarning: A non-numeric value encountered in /t.php on line 3\n"
    );
    let cases: Vec<(&str, Value, Value, &str)> = vec![
        (
            "+",
            s("dolor"),
            s("sit"),
            "Unsupported operand types: string + string",
        ),
        (
            "-",
            s("adipiscing"),
            s("elit,"),
            "Unsupported operand types: string - string",
        ),
        (
            "*",
            s("eiusmod"),
            s("tempor"),
            "Unsupported operand types: string * string",
        ),
        (
            "/",
            s("labore"),
            s("et"),
            "Unsupported operand types: string / string",
        ),
        (
            "**",
            s("aliqua."),
            s("Ut"),
            "Unsupported operand types: string ** string",
        ),
        (
            "%",
            s("minim"),
            s("veniam,"),
            "Unsupported operand types: string % string",
        ),
        (
            "<<",
            s("quis"),
            s("nostrud"),
            "Unsupported operand types: string << string",
        ),
        (
            ">>",
            s("laboris"),
            s("nisi"),
            "Unsupported operand types: string >> string",
        ),
        (
            "|",
            s("ex"),
            73.into(),
            "Unsupported operand types: string | int",
        ),
        (
            "|",
            79.into(),
            s("ea"),
            "Unsupported operand types: int | string",
        ),
        (
            "&",
            s("Duis"),
            103.into(),
            "Unsupported operand types: string & int",
        ),
        (
            "^",
            139.into(),
            s("reprehenderit"),
            "Unsupported operand types: int ^ string",
        ),
        (
            "+",
            Value::empty_array(),
            1.into(),
            "Unsupported operand types: array + int",
        ),
        (
            "-",
            Value::empty_array(),
            Value::empty_array(),
            "Unsupported operand types: array - array",
        ),
        (
            "*",
            1.into(),
            Value::empty_array(),
            "Unsupported operand types: int * array",
        ),
    ];
    for (op, a, b, msg) in cases {
        let r = match op {
            "+" => t.vm.add(&a, &b),
            "-" => t.vm.sub(&a, &b),
            "*" => t.vm.mul(&a, &b),
            "/" => t.vm.div(&a, &b),
            "**" => t.vm.pow(&a, &b),
            "%" => t.vm.modulo(&a, &b),
            "<<" => t.vm.shift_left(&a, &b),
            ">>" => t.vm.shift_right(&a, &b),
            "|" => t.vm.bitwise_or(&a, &b),
            "&" => t.vm.bitwise_and(&a, &b),
            "^" => t.vm.bitwise_xor(&a, &b),
            _ => unreachable!(),
        };
        assert_eq!(thrown(r), ("TypeError".into(), msg.into()), "{op}");
    }
    assert_eq!(t.output(), "");

    assert_eq!(int(t.vm.bitwise_or(&s("59 ut"), &61.into()).unwrap()), 63);
    assert_eq!(
        int(t.vm.bitwise_and(&s("83 commodo"), &89.into()).unwrap()),
        81
    );
    assert_eq!(
        int(t.vm.bitwise_xor(&127.into(), &s("131 dolor")).unwrap()),
        252
    );
    assert_eq!(int(t.vm.modulo(&s("31 enim"), &s("37 ad")).unwrap()), 31);
    assert_eq!(int(t.vm.unary_plus(&s("149 in")).unwrap()), 149);
    assert_eq!(int(t.vm.negate(&s("151 velit")).unwrap()), -151);
    let _ = t.output();
    assert_eq!(
        thrown(t.vm.unary_plus(&s("voluptate"))).1,
        "Unsupported operand types: string * int"
    );
    assert_eq!(
        thrown(t.vm.negate(&s("esse"))).1,
        "Unsupported operand types: string * int"
    );
    assert_eq!(
        float(t.vm.div(&s("17 incididunt"), &s("19 ut")).unwrap()),
        0.8947368421052632
    );
    assert_eq!(
        float(t.vm.pow(&s("23 dolore"), &s("29 magna")).unwrap()),
        3.0910586430935376E+39
    );
}

#[test]
fn division_and_modulo() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(int(vm.div(&4.into(), &2.into()).unwrap()), 2);
    assert_eq!(float(vm.div(&1.into(), &2.into()).unwrap()), 0.5);
    assert_eq!(
        float(vm.div(&i64::MIN.into(), &(-1).into()).unwrap()),
        9223372036854775808.0
    );
    assert_eq!(int(vm.div(&s("6"), &s("3")).unwrap()), 2);
    assert_eq!(
        thrown(vm.div(&1.into(), &0.into())),
        ("DivisionByZeroError".into(), "Division by zero".into())
    );
    assert_eq!(
        thrown(vm.div(&1.0.into(), &0.0.into())).1,
        "Division by zero"
    );
    assert_eq!(thrown(vm.div(&s("1"), &s("0"))).1, "Division by zero");
    assert_eq!(int(vm.modulo(&5.into(), &3.into()).unwrap()), 2);
    assert_eq!(int(vm.modulo(&(-5).into(), &3.into()).unwrap()), -2);
    assert_eq!(int(vm.modulo(&5.into(), &(-3).into()).unwrap()), 2);
    assert_eq!(int(vm.modulo(&i64::MIN.into(), &(-1).into()).unwrap()), 0);
    assert_eq!(
        thrown(vm.modulo(&5.into(), &0.into())),
        ("DivisionByZeroError".into(), "Modulo by zero".into())
    );
    assert_eq!(int(t.vm.modulo(&5.5.into(), &2.into()).unwrap()), 1);
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float 5.5 to int loses precision in /t.php on line 3\n"
    );
    assert_eq!(int(t.vm.modulo(&s("5.5"), &2.into()).unwrap()), 1);
    assert_eq!(
        t.output(),
        "\nDeprecated: Implicit conversion from float-string \"5.5\" to int loses precision in /t.php on line 3\n"
    );
    assert_eq!(
        int(t.vm.modulo(&1e20.into(), &7.into()).unwrap()),
        7766279631452241920i64 % 7
    );
    assert!(
        t.output()
            .contains("The float 1.0E+20 is not representable as an int, cast occurred")
    );
    assert_eq!(t.vm.intdiv(7, 2).unwrap(), 3);
    assert_eq!(t.vm.intdiv(-7, 2).unwrap(), -3);
}

#[test]
fn powers() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(int(vm.pow(&2.into(), &3.into()).unwrap()), 8);
    assert_eq!(int(vm.pow(&2.into(), &0.into()).unwrap()), 1);
    assert_eq!(int(vm.pow(&0.into(), &5.into()).unwrap()), 0);
    assert_eq!(int(vm.pow(&(-2).into(), &3.into()).unwrap()), -8);
    assert_eq!(int(vm.pow(&(-2).into(), &63.into()).unwrap()), i64::MIN);
    assert_eq!(
        float(vm.pow(&2.into(), &63.into()).unwrap()),
        9223372036854775808.0
    );
    assert_eq!(float(vm.pow(&2.into(), &(-1).into()).unwrap()), 0.5);
    assert_eq!(float(vm.pow(&10.into(), &20.into()).unwrap()), 1e20);
    assert_eq!(
        float(vm.pow(&2.0.into(), &0.5.into()).unwrap()),
        2f64.sqrt()
    );
    assert_eq!(int(vm.pow(&s("2"), &s("10")).unwrap()), 1024);
    assert_eq!(
        float(t.vm.pow(&0.into(), &(-1).into()).unwrap()),
        f64::INFINITY
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Power of base 0 and negative exponent is deprecated in /t.php on line 3\n"
    );
}

#[test]
fn shifts_and_bitwise() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    assert_eq!(int(vm.shift_left(&1.into(), &3.into()).unwrap()), 8);
    assert_eq!(int(vm.shift_left(&1.into(), &63.into()).unwrap()), i64::MIN);
    assert_eq!(int(vm.shift_left(&1.into(), &64.into()).unwrap()), 0);
    assert_eq!(int(vm.shift_right(&(-8).into(), &1.into()).unwrap()), -4);
    assert_eq!(int(vm.shift_right(&(-1).into(), &64.into()).unwrap()), -1);
    assert_eq!(int(vm.shift_right(&1.into(), &100.into()).unwrap()), 0);
    assert_eq!(
        thrown(vm.shift_left(&1.into(), &(-1).into())),
        (
            "ArithmeticError".into(),
            "Bit shift by negative number".into()
        )
    );
    assert_eq!(
        thrown(vm.shift_right(&1.into(), &(-1).into())).1,
        "Bit shift by negative number"
    );

    assert_eq!(
        vm.bitwise_or(&s("ab"), &s("  ")).unwrap().as_str().unwrap(),
        "ab"
    );
    assert_eq!(
        vm.bitwise_or(&s("a"), &s("bcd"))
            .unwrap()
            .as_str()
            .unwrap()
            .as_bytes(),
        &[b'a' | b'b', b'c', b'd']
    );
    assert_eq!(
        vm.bitwise_and(&s("abc"), &s("a"))
            .unwrap()
            .as_str()
            .unwrap(),
        "a"
    );
    assert_eq!(
        vm.bitwise_xor(&s("12"), &s("3"))
            .unwrap()
            .as_str()
            .unwrap()
            .as_bytes(),
        &[b'1' ^ b'3']
    );
    assert_eq!(
        vm.bitwise_not(&s("A"))
            .unwrap()
            .as_str()
            .unwrap()
            .as_bytes(),
        &[!b'A']
    );
    assert_eq!(int(vm.bitwise_not(&5.into()).unwrap()), -6);
    assert_eq!(int(vm.bitwise_or(&5.0.into(), &2.into()).unwrap()), 7);
    assert_eq!(
        thrown(vm.bitwise_not(&Value::empty_array())).1,
        "Cannot perform bitwise not on array"
    );
    assert_eq!(
        thrown(vm.bitwise_not(&Value::Null)).1,
        "Cannot perform bitwise not on null"
    );
    assert_eq!(
        thrown(vm.bitwise_not(&true.into())).1,
        "Cannot perform bitwise not on true"
    );
    assert_eq!(int(t.vm.bitwise_not(&1.5.into()).unwrap()), -2);
    assert!(
        t.output()
            .contains("Implicit conversion from float 1.5 to int loses precision")
    );
    assert!(matches!(
        t.vm.bool_xor(&1.into(), &0.into()).unwrap(),
        Value::Bool(true)
    ));
    assert!(matches!(t.vm.bool_not(&s("0")).unwrap(), Value::Bool(true)));
}

#[test]
fn concatenation() {
    let mut t = TestVm::new();
    assert_eq!(
        t.vm.concat(&1.into(), &2.into()).unwrap().as_str().unwrap(),
        "12"
    );
    assert_eq!(
        t.vm.concat(&1.5.into(), &s("")).unwrap().as_str().unwrap(),
        "1.5"
    );
    assert_eq!(
        t.vm.concat(&Value::Null, &s("x"))
            .unwrap()
            .as_str()
            .unwrap(),
        "x"
    );
    assert_eq!(
        t.vm.concat(&true.into(), &false.into())
            .unwrap()
            .as_str()
            .unwrap(),
        "1"
    );
    assert_eq!(
        t.vm.concat(&(-0.0).into(), &s("|"))
            .unwrap()
            .as_str()
            .unwrap(),
        "-0|"
    );
    assert_eq!(t.output(), "");
    assert_eq!(
        t.vm.concat(&Value::empty_array(), &s("!"))
            .unwrap()
            .as_str()
            .unwrap(),
        "Array!"
    );
    assert_eq!(
        t.output(),
        "\nWarning: Array to string conversion in /t.php on line 3\n"
    );
}

#[test]
fn loose_equality_php8() {
    let mut t = TestVm::new();
    let cases: Vec<(Value, Value, bool)> = vec![
        (0.into(), s("a"), false),
        (s("1"), s("01"), true),
        (s("10"), s("1e1"), true),
        (100.into(), s("1e2"), true),
        (s("abc"), 0.into(), false),
        (Value::Null, false.into(), true),
        (Value::empty_array(), false.into(), true),
        (s("0"), false.into(), true),
        (s(""), Value::Null, true),
        (s("a"), s("a "), false),
        (s("1"), s(" 1"), true),
        (s("1"), s("1 "), true),
        (s("abc"), s("ABC"), false),
        (1.5.into(), s("1.5"), true),
        (1.0.into(), s("1"), true),
        ((0.1 + 0.2).into(), s("0.3"), false),
        (s("1e1000"), s("1e1000"), true),
        (s("9223372036854775808"), s("9223372036854775809"), false),
        (s("9223372036854775807"), s("9223372036854775808"), false),
        (Value::Null, 0.into(), true),
        (Value::Null, s("0"), false),
        (true.into(), s("a"), true),
        (true.into(), Value::empty_array(), false),
        (f64::NAN.into(), f64::NAN.into(), false),
        (arr(&[1, 2]), arr(&[1, 2]), true),
        (s("1.0"), s("1"), true),
        (s("abc"), s("abc"), true),
        (s(""), 0.into(), false),
        (s(" "), 0.into(), false),
        (42.into(), s("42abc"), false),
        (s("INF"), f64::INFINITY.into(), true),
    ];
    for (a, b, want) in cases {
        assert_eq!(t.vm.is_equal(&a, &b).unwrap(), want, "{a:?} == {b:?}");
        assert_eq!(t.vm.is_equal(&b, &a).unwrap(), want, "{b:?} == {a:?}");
    }
    assert_eq!(t.output(), "");
}

#[test]
fn ordering_and_spaceship() {
    let mut t = TestVm::new();
    let vm = &mut t.vm;
    let c = |vm: &mut Vm, a: Value, b: Value| vm.compare(&a, &b).unwrap();
    assert_eq!(c(vm, 1.into(), 2.into()), -1);
    assert_eq!(c(vm, s("abc"), s("abd")), -1);
    assert_eq!(c(vm, s("b"), s("abc")), 1);
    assert_eq!(c(vm, s("10"), s("9")), 1);
    assert_eq!(c(vm, s("10"), s("9a")), -1);
    assert_eq!(c(vm, 1.into(), s("abc")), -1);
    assert_eq!(c(vm, s("abc"), 1.into()), 1);
    assert_eq!(c(vm, Value::Null, (-1).into()), -1);
    assert!(vm.is_smaller(&Value::Null, &(-1).into()).unwrap());
    assert_eq!(c(vm, f64::NAN.into(), 1.into()), 1);
    assert_eq!(c(vm, 1.into(), f64::NAN.into()), 1);
    assert!(!vm.is_smaller(&f64::NAN.into(), &1.into()).unwrap());
    assert!(!vm.is_smaller(&1.into(), &f64::NAN.into()).unwrap());
    assert_eq!(c(vm, arr(&[1, 2, 3]), arr(&[1, 2, 4])), -1);
    assert_eq!(c(vm, arr(&[1, 2]), arr(&[1, 2, 3])), -1);
    assert_eq!(c(vm, arr(&[5]), arr(&[1, 2])), -1);
    let a = Value::array(PhpArray::from_pairs([("a", 1)]));
    let b = Value::array(PhpArray::from_pairs([("b", 1)]));
    assert_eq!(c(vm, a.clone(), b.clone()), 1);
    assert_eq!(c(vm, b.clone(), a.clone()), 1);
    assert!(!vm.is_equal(&a, &b).unwrap());
    assert_eq!(c(vm, arr(&[1]), 1.into()), 1);
    assert_eq!(c(vm, 1.into(), arr(&[1])), -1);
    assert_eq!(c(vm, true.into(), false.into()), 1);
    assert_eq!(c(vm, Value::Null, s("a")), -1);
    assert_eq!(c(vm, s(""), Value::Null), 0);
    assert!(matches!(
        vm.spaceship(&2.into(), &1.into()).unwrap(),
        Value::Int(1)
    ));
    let o1 = vm
        .cast_to_object(&Value::array(PhpArray::from_pairs([("a", 1)])))
        .unwrap();
    let o2 = vm
        .cast_to_object(&Value::array(PhpArray::from_pairs([("a", 2)])))
        .unwrap();
    assert_eq!(
        c(vm, Value::Object(o1.clone()), Value::Object(o2.clone())),
        -1
    );
    assert_eq!(
        c(vm, Value::Object(o1.clone()), Value::Object(o1.clone())),
        0
    );
    assert_eq!(c(vm, Value::Object(o1.clone()), Value::Null), 1);
    assert_eq!(c(vm, Value::Object(o1.clone()), true.into()), 0);
    let ex = vm.create_exception("Exception", "x", 0);
    assert_eq!(c(vm, Value::Object(o1), Value::Object(ex)), 1);
}

#[test]
fn identity() {
    assert!(!is_identical(&1.into(), &1.0.into()));
    assert!(is_identical(&s("1"), &s("1")));
    assert!(is_identical(&arr(&[1, 2]), &arr(&[1, 2])));
    let a = Value::array(PhpArray::from_pairs([("a", 1), ("b", 2)]));
    let b = Value::array(PhpArray::from_pairs([("b", 2), ("a", 1)]));
    assert!(!is_identical(&a, &b));
    let mut t = TestVm::new();
    assert!(t.vm.is_equal(&a, &b).unwrap());
    assert!(!is_identical(&f64::NAN.into(), &f64::NAN.into()));
    assert!(is_identical(&0.0.into(), &(-0.0).into()));
    assert!(is_identical(&Value::Null, &Value::Undef));
    let r = Value::Ref(crate::value::PhpRef::new(Value::Int(1)));
    assert!(is_identical(&r, &1.into()));
    assert!(!is_identical(
        &arr(&[1]),
        &Value::array(PhpArray::from_values(vec![s("1")]))
    ));
}

#[test]
fn string_increment_table() {
    let cases = [
        ("a", "b"),
        ("z", "aa"),
        ("Az", "Ba"),
        ("zz", "aaa"),
        ("Zz", "AAa"),
        ("a9", "b0"),
        ("Zz9", "AAa0"),
        ("9z", "10a"),
        ("a!", "a!"),
        ("!z", "!a"),
        ("", "1"),
    ];
    for (from, to) in cases {
        assert_eq!(increment_string(from.as_bytes()), to, "{from}");
    }
}

#[test]
fn increment_decrement() {
    let mut t = TestVm::new();
    assert_eq!(int(t.vm.increment(&1.into()).unwrap()), 2);
    assert_eq!(
        float(t.vm.increment(&i64::MAX.into()).unwrap()),
        9223372036854775808.0
    );
    assert_eq!(
        float(t.vm.decrement(&i64::MIN.into()).unwrap()),
        -9223372036854775809.0
    );
    assert_eq!(int(t.vm.increment(&Value::Null).unwrap()), 1);
    assert_eq!(int(t.vm.increment(&s("9")).unwrap()), 10);
    assert_eq!(float(t.vm.increment(&s("9.5")).unwrap()), 10.5);
    assert_eq!(
        float(t.vm.increment(&s("9223372036854775807")).unwrap()),
        9223372036854775808.0
    );
    assert_eq!(int(t.vm.increment(&s(" 5 ")).unwrap()), 6);
    assert_eq!(int(t.vm.decrement(&s("5")).unwrap()), 4);
    assert_eq!(float(t.vm.increment(&1.5.into()).unwrap()), 2.5);
    assert_eq!(t.output(), "");

    assert_eq!(t.vm.increment(&s("a")).unwrap().as_str().unwrap(), "b");
    assert_eq!(
        t.output(),
        "\nDeprecated: Increment on non-numeric string is deprecated, use str_increment() instead in /t.php on line 3\n"
    );
    assert_eq!(t.vm.increment(&s("")).unwrap().as_str().unwrap(), "1");
    assert!(
        t.output()
            .contains("Increment on non-numeric string is deprecated")
    );
    assert!(matches!(t.vm.decrement(&Value::Null).unwrap(), Value::Null));
    assert_eq!(
        t.output(),
        "\nWarning: Decrement on type null has no effect, this will change in the next major version of PHP in /t.php on line 3\n"
    );
    assert!(matches!(
        t.vm.increment(&true.into()).unwrap(),
        Value::Bool(true)
    ));
    assert_eq!(
        t.output(),
        "\nWarning: Increment on type bool has no effect, this will change in the next major version of PHP in /t.php on line 3\n"
    );
    assert!(matches!(
        t.vm.decrement(&false.into()).unwrap(),
        Value::Bool(false)
    ));
    assert!(t.output().contains("Decrement on type bool has no effect"));
    assert_eq!(int(t.vm.decrement(&s("")).unwrap()), -1);
    assert_eq!(
        t.output(),
        "\nDeprecated: Decrement on empty string is deprecated as non-numeric in /t.php on line 3\n"
    );
    assert_eq!(t.vm.decrement(&s("abc")).unwrap().as_str().unwrap(), "abc");
    assert_eq!(
        t.output(),
        "\nDeprecated: Decrement on non-numeric string has no effect and is deprecated in /t.php on line 3\n"
    );
    assert_eq!(
        thrown(t.vm.increment(&Value::empty_array())),
        ("TypeError".into(), "Cannot increment array".into())
    );
    assert_eq!(
        thrown(t.vm.decrement(&Value::empty_array())).1,
        "Cannot decrement array"
    );
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    assert_eq!(
        thrown(t.vm.increment(&Value::Object(o))).1,
        "Cannot increment stdClass"
    );
}

#[test]
fn operators_through_references() {
    let mut t = TestVm::new();
    let r = Value::Ref(crate::value::PhpRef::new(Value::Int(5)));
    assert_eq!(int(t.vm.add(&r, &r).unwrap()), 10);
    assert!(t.vm.is_equal(&r, &5.into()).unwrap());
    assert_eq!(t.vm.concat(&r, &s("x")).unwrap().as_str().unwrap(), "5x");
    let mut slot = r.clone();
    t.vm.concat_assign(&mut slot, &s("y")).unwrap();
    assert_eq!(r.unref().as_str().unwrap(), "5y");
}

#[test]
fn recursion_is_detected() {
    let mut t = TestVm::new();
    // $a = [null]; $a[0] = &$a;
    let a_ref = crate::value::PhpRef::new(Value::Null);
    let mut arr = PhpArray::new();
    arr.insert(0.into(), Value::Ref(a_ref.clone()));
    a_ref.set(Value::array(arr));
    let a = Value::Ref(a_ref.clone());
    let other = Value::array(PhpArray::from_values(vec![Value::empty_array()]));
    assert_eq!(
        thrown(t.vm.is_identical(&a, &other).map(Value::Bool)),
        (
            "Error".into(),
            "Nesting level too deep - recursive dependency?".into()
        )
    );
    assert_eq!(
        thrown(t.vm.is_equal(&a, &other).map(Value::Bool)).1,
        "Nesting level too deep - recursive dependency?"
    );
    assert!(!is_identical(&a, &other));
    // Break the cycle so the test does not leak.
    a_ref.set(Value::Null);

    // $o->x = $o; $p->x = $p; $o == $p
    let o = t.vm.cast_to_object(&Value::Null).unwrap();
    o.set_prop(b"x", Value::Object(o.clone()));
    let p = t.vm.cast_to_object(&Value::Null).unwrap();
    p.set_prop(b"x", Value::Object(p.clone()));
    assert_eq!(
        thrown(
            t.vm.is_equal(&Value::Object(o.clone()), &Value::Object(p.clone()))
                .map(Value::Bool)
        )
        .1,
        "Nesting level too deep - recursive dependency?"
    );
    o.set_prop(b"x", Value::Null);
    p.set_prop(b"x", Value::Null);
}

#[test]
fn numeric_string_quirks() {
    let mut t = TestVm::new();
    // `ptr = e++` quirk of _is_numeric_string_ex.
    assert_eq!(
        int(t.vm.add(&s("9223372036854775808e+"), &0.into()).unwrap()),
        i64::MIN
    );
    assert!(t.output().contains("A non-numeric value encountered"));
    assert_eq!(
        t.vm.to_int(&s("1999999999999999999e-")).unwrap(),
        2000000000000000000
    );
    let _ = t.vm.modulo(&s("1.5\0x"), &7.into());
    assert!(
        t.output()
            .contains("Implicit conversion from float-string \"1.5\" to int loses precision")
    );
}
