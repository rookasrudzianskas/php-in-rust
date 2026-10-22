//! ext/standard/type.c — type inspection and conversion functions.
//!
//! `is_callable()` (also declared in type.c) needs `zend_is_callable_ex`
//! and lives with the interpreter; `array_is_list()` is declared with the
//! array.c functions in the stub and is ported there.

use php_runtime::value::numeric::is_numeric_str;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `gettype()` (`zend_zval_get_legacy_type`).
fn gettype(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::str(args.get(0).legacy_type_name()))
}

/// `get_debug_type()`.
fn get_debug_type(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match &*args.get(0).deref() {
        Value::Undef | Value::Null => Value::str("null"),
        Value::Bool(_) => Value::str("bool"),
        Value::Int(_) => Value::str("int"),
        Value::Float(_) => Value::str("float"),
        Value::String(_) => Value::str("string"),
        Value::Array(_) => Value::str("array"),
        // Anonymous class names are cut at their NUL byte (`strlen(name)`).
        Value::Object(o) => {
            let name = o.class().name.as_bytes();
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            Value::bytes(&name[..end])
        }
        Value::Resource(r) => {
            if r.is_closed() {
                Value::str("resource (closed)")
            } else {
                Value::from(format!("resource ({})", r.type_name()))
            }
        }
        Value::Ref(_) => unreachable!("references never nest"),
    })
}

/// `settype()`: converts the referenced variable in place
/// (`convert_to_long`, `convert_to_double`, ...).
fn settype(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let var = args.by_ref(0);
    let ty = args.str(1).to_ascii_lowercase();
    let cur = var.get();
    let new = match ty.as_bytes() {
        b"integer" | b"int" => Value::Int(vm.to_int(&cur)?),
        b"float" | b"double" => Value::Float(vm.to_float(&cur)?),
        b"string" => match vm.to_php_string(&cur) {
            Ok(s) => Value::String(s),
            Err(e) => {
                // `_convert_to_string` leaves an empty string behind.
                var.set(Value::empty_string());
                return Err(e);
            }
        },
        b"array" => Value::array(vm.cast_to_array(&cur)?),
        b"object" => Value::Object(vm.cast_to_object(&cur)?),
        b"bool" | b"boolean" => Value::Bool(vm.to_bool(&cur)?),
        b"null" => {
            vm.cast_to_null(&cur)?;
            Value::Null
        }
        b"resource" => return Err(vm.throw_value_error("Cannot convert to resource type")),
        _ => return Err(vm.argument_value_error(2, "must be a valid type")),
    };
    var.set(new);
    Ok(Value::Bool(true))
}

/// C `isspace` in the "C" locale.
fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `ZEND_STRTOL(s, NULL, base)`: C `strtol` (bases outside 2..=36 other than
/// 0 are rejected with a result of 0, like glibc's `EINVAL`).
fn c_strtol(s: &[u8], base: i64) -> i64 {
    if base != 0 && !(2..=36).contains(&base) {
        return 0;
    }
    php_runtime::ini::strtol(s, base as u32).0
}

/// `intval()`.
fn intval(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let base = args.int(1);
    let num = args.get(0).unref();
    let s = match &num {
        Value::String(s) if base != 10 => s.clone(),
        _ => return Ok(Value::Int(vm.to_int(&num)?)),
    };
    let bytes = s.as_bytes();
    if base == 0 || base == 2 {
        let skip = bytes.iter().take_while(|&&c| is_c_space(c)).count();
        let strval = &bytes[skip..];
        // A length of 3+ covers "0b#" and "-0b" (which results in 0).
        if strval.len() > 2 {
            let offset = usize::from(strval[0] == b'-' || strval[0] == b'+');
            if strval[offset] == b'0' && matches!(strval.get(offset + 1), Some(b'b' | b'B')) {
                if strval[0] != b'-' {
                    return Ok(Value::Int(c_strtol(&strval[2 + offset..], 2)));
                }
                let mut tmp = Vec::with_capacity(strval.len() - 1);
                tmp.push(b'-');
                tmp.extend_from_slice(&strval[offset + 2..]);
                return Ok(Value::Int(c_strtol(&tmp, 2)));
            }
        }
    }
    Ok(Value::Int(c_strtol(bytes, base)))
}

/// `floatval()` / `doubleval()` (`zval_get_double`).
fn floatval(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    Ok(Value::Float(vm.to_float(&v)?))
}

/// `boolval()` (`zend_is_true`).
fn boolval(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    Ok(Value::Bool(vm.to_bool(&v)?))
}

/// `strval()` (`zval_get_string`).
fn strval(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    Ok(Value::String(vm.to_php_string(&v)?))
}

fn is_null(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::Null | Value::Undef
    )))
}

/// `is_resource()`: closed resources have no type name and are rejected.
fn is_resource(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(
        matches!(&*args.get(0).deref(), Value::Resource(r) if !r.is_closed()),
    ))
}

fn is_bool(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(&*args.get(0).deref(), Value::Bool(_))))
}

fn is_int(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(&*args.get(0).deref(), Value::Int(_))))
}

fn is_float(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::Float(_)
    )))
}

/// `is_numeric()` (`_zend_is_numeric`): numbers and numeric strings
/// (leading and trailing whitespace allowed, no trailing data).
fn is_numeric(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(match &*args.get(0).deref() {
        Value::Int(_) | Value::Float(_) => true,
        Value::String(s) => is_numeric_str(s).is_some(),
        _ => false,
    }))
}

fn is_string(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::String(_)
    )))
}

fn is_array(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::Array(_)
    )))
}

fn is_object(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::Object(_)
    )))
}

fn is_scalar(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(matches!(
        &*args.get(0).deref(),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::String(_)
    )))
}

/// `is_iterable()` (`zend_is_iterable`): arrays and `Traversable` objects.
fn is_iterable(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(match &*args.get(0).deref() {
        Value::Array(_) => true,
        Value::Object(o) => o.class().instance_of_name(b"Traversable"),
        _ => false,
    }))
}

/// `is_countable()`: arrays, objects with a `count_elements` handler and
/// `Countable` implementations.
fn is_countable(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(match &*args.get(0).deref() {
        Value::Array(_) => true,
        Value::Object(o) => {
            o.class().handlers.count_elements.is_some() || o.class().instance_of_name(b"Countable")
        }
        _ => false,
    }))
}

pub static GETTYPE: NativeFunction = NativeFunction {
    sig: "function gettype(mixed $value): string {}",
    handler: gettype,
};
pub static GET_DEBUG_TYPE: NativeFunction = NativeFunction {
    sig: "function get_debug_type(mixed $value): string {}",
    handler: get_debug_type,
};
pub static SETTYPE: NativeFunction = NativeFunction {
    sig: "function settype(mixed &$var, string $type): bool {}",
    handler: settype,
};
pub static INTVAL: NativeFunction = NativeFunction {
    sig: "function intval(mixed $value, int $base = 10): int {}",
    handler: intval,
};
pub static FLOATVAL: NativeFunction = NativeFunction {
    sig: "function floatval(mixed $value): float {}",
    handler: floatval,
};
pub static DOUBLEVAL: NativeFunction = NativeFunction {
    sig: "function doubleval(mixed $value): float {}",
    handler: floatval,
};
pub static BOOLVAL: NativeFunction = NativeFunction {
    sig: "function boolval(mixed $value): bool {}",
    handler: boolval,
};
pub static STRVAL: NativeFunction = NativeFunction {
    sig: "function strval(mixed $value): string {}",
    handler: strval,
};
pub static IS_NULL: NativeFunction = NativeFunction {
    sig: "function is_null(mixed $value): bool {}",
    handler: is_null,
};
pub static IS_RESOURCE: NativeFunction = NativeFunction {
    sig: "function is_resource(mixed $value): bool {}",
    handler: is_resource,
};
pub static IS_BOOL: NativeFunction = NativeFunction {
    sig: "function is_bool(mixed $value): bool {}",
    handler: is_bool,
};
pub static IS_INT: NativeFunction = NativeFunction {
    sig: "function is_int(mixed $value): bool {}",
    handler: is_int,
};
pub static IS_INTEGER: NativeFunction = NativeFunction {
    sig: "function is_integer(mixed $value): bool {}",
    handler: is_int,
};
pub static IS_LONG: NativeFunction = NativeFunction {
    sig: "function is_long(mixed $value): bool {}",
    handler: is_int,
};
pub static IS_FLOAT: NativeFunction = NativeFunction {
    sig: "function is_float(mixed $value): bool {}",
    handler: is_float,
};
pub static IS_DOUBLE: NativeFunction = NativeFunction {
    sig: "function is_double(mixed $value): bool {}",
    handler: is_float,
};
pub static IS_NUMERIC: NativeFunction = NativeFunction {
    sig: "function is_numeric(mixed $value): bool {}",
    handler: is_numeric,
};
pub static IS_STRING: NativeFunction = NativeFunction {
    sig: "function is_string(mixed $value): bool {}",
    handler: is_string,
};
pub static IS_ARRAY: NativeFunction = NativeFunction {
    sig: "function is_array(mixed $value): bool {}",
    handler: is_array,
};
pub static IS_OBJECT: NativeFunction = NativeFunction {
    sig: "function is_object(mixed $value): bool {}",
    handler: is_object,
};
pub static IS_SCALAR: NativeFunction = NativeFunction {
    sig: "function is_scalar(mixed $value): bool {}",
    handler: is_scalar,
};
pub static IS_ITERABLE: NativeFunction = NativeFunction {
    sig: "function is_iterable(mixed $value): bool {}",
    handler: is_iterable,
};
pub static IS_COUNTABLE: NativeFunction = NativeFunction {
    sig: "function is_countable(mixed $value): bool {}",
    handler: is_countable,
};

/// Aliases marked `#[\Deprecated(since: "8.6")]` in the stub
/// (`Function X() is deprecated since 8.6, use Y() instead`).
pub const DEPRECATED_ALIASES: &[(&str, &str)] = &[
    ("doubleval", "floatval"),
    ("is_integer", "is_int"),
    ("is_long", "is_int"),
    ("is_double", "is_float"),
];

/// The deprecation message `zend_deprecated_function` prints for one of
/// [`DEPRECATED_ALIASES`] (for the interpreter's call path).
pub fn deprecation_message(name: &str) -> Option<PhpStr> {
    DEPRECATED_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(alias, target)| {
            PhpStr::from(format!(
                "Function {alias}() is deprecated since 8.6, use {target}() instead"
            ))
        })
}

pub fn register(r: &mut Registry) {
    r.functions(&[
        &GETTYPE,
        &GET_DEBUG_TYPE,
        &SETTYPE,
        &INTVAL,
        &FLOATVAL,
        &BOOLVAL,
        &STRVAL,
        &IS_NULL,
        &IS_RESOURCE,
        &IS_BOOL,
        &IS_INT,
        &IS_FLOAT,
        &IS_NUMERIC,
        &IS_STRING,
        &IS_ARRAY,
        &IS_OBJECT,
        &IS_SCALAR,
        &IS_ITERABLE,
        &IS_COUNTABLE,
    ]);
    // #[\Deprecated(since: "8.6", message: "use X() instead")]: the
    // runtime emits the deprecation before the call.
    r.deprecated_function(&DOUBLEVAL, "8.6", "use floatval() instead");
    r.deprecated_function(&IS_INTEGER, "8.6", "use is_int() instead");
    r.deprecated_function(&IS_LONG, "8.6", "use is_int() instead");
    r.deprecated_function(&IS_DOUBLE, "8.6", "use is_float() instead");
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use php_runtime::class::{class_flags, fn_flags};
    use php_runtime::value::ResRef;
    use php_runtime::{ClassEntry, ObjRef, PhpArray, PhpRef, Value};

    use crate::testing::{TestVm, thrown};

    fn call(t: &mut TestVm, f: &str, args: Vec<Value>) -> Value {
        t.call(f, args)
            .unwrap_or_else(|e| panic!("{f}() failed: {e:?}"))
    }

    #[track_caller]
    fn same(a: Value, b: Value) {
        assert!(a.identical(&b), "{a:?} !== {b:?}");
    }

    fn int(t: &mut TestVm, v: Value) -> i64 {
        call(t, "intval", vec![v]).as_int().unwrap()
    }

    fn intb(t: &mut TestVm, v: &str, base: i64) -> i64 {
        call(t, "intval", vec![Value::str(v), Value::Int(base)])
            .as_int()
            .unwrap()
    }

    fn b(t: &mut TestVm, f: &str, v: Value) -> bool {
        call(t, f, vec![v]).as_bool().unwrap()
    }

    fn std_obj(t: &TestVm) -> Value {
        let ce = t.vm.find_class(b"stdClass").unwrap();
        Value::Object(ObjRef::new(ce))
    }

    fn resources() -> (Value, Value) {
        let open = ResRef::new("stream", Box::new(()));
        let closed = ResRef::new("stream", Box::new(()));
        closed.close();
        (Value::Resource(open), Value::Resource(closed))
    }

    #[test]
    fn gettype_names() {
        let mut t = TestVm::new();
        let (open, closed) = resources();
        let cases = [
            (Value::Null, "NULL"),
            (Value::Bool(true), "boolean"),
            (Value::Int(1), "integer"),
            (Value::Float(1.5), "double"),
            (Value::str("x"), "string"),
            (Value::empty_array(), "array"),
            (std_obj(&t), "object"),
            (open, "resource"),
            (closed, "resource (closed)"),
        ];
        for (v, want) in cases {
            same(call(&mut t, "gettype", vec![v]), Value::str(want));
        }
    }

    #[test]
    fn get_debug_type_names() {
        // ext/standard/tests/general_functions/get_debug_type_basic.phpt
        let mut t = TestVm::new();
        let (open, closed) = resources();
        let anon = ClassEntry::new("class@anonymous\0/in/t.php:3$0", class_flags::ANON);
        let anon = Value::Object(ObjRef::new(Rc::new(anon)));
        let ext = ClassEntry::new("ToBeExtended@anonymous\0/in/t.php:4$1", class_flags::ANON);
        let ext = Value::Object(ObjRef::new(Rc::new(ext)));
        let cases = [
            (std_obj(&t), "stdClass"),
            (anon, "class@anonymous"),
            (ext, "ToBeExtended@anonymous"),
            (Value::str("foo"), "string"),
            (Value::Bool(false), "bool"),
            (Value::Bool(true), "bool"),
            (Value::Int(1), "int"),
            (Value::Float(1.1), "float"),
            (Value::empty_array(), "array"),
            (Value::Null, "null"),
            (open, "resource (stream)"),
            (closed, "resource (closed)"),
        ];
        for (v, want) in cases {
            same(call(&mut t, "get_debug_type", vec![v]), Value::str(want));
        }
    }

    #[test]
    fn intval_base_10_and_conversions() {
        // ext/standard/tests/general_functions/intval.phpt (64-bit values)
        let mut t = TestVm::new();
        assert_eq!(int(&mut t, Value::str("0")), 0);
        assert_eq!(int(&mut t, Value::str("-1")), -1);
        assert_eq!(int(&mut t, Value::str("0x12ab")), 0);
        assert_eq!(int(&mut t, Value::str("0123")), 123);
        assert_eq!(int(&mut t, Value::str("12abc")), 12);
        assert_eq!(int(&mut t, Value::str(" 12")), 12);
        assert_eq!(int(&mut t, Value::str("1e3")), 1000);
        assert_eq!(int(&mut t, Value::str("9223372036854775808")), i64::MAX);
        assert_eq!(int(&mut t, Value::str("-9223372036854775809")), i64::MIN);
        assert_eq!(int(&mut t, Value::Float(-0.1)), 0);
        assert_eq!(int(&mut t, Value::Float(10.5e5)), 1050000);
        assert_eq!(int(&mut t, Value::empty_array()), 0);
        let one = Value::array(PhpArray::from_values(vec![Value::Null]));
        assert_eq!(int(&mut t, one), 1);
        assert_eq!(int(&mut t, Value::Bool(true)), 1);
        assert_eq!(int(&mut t, Value::Null), 0);
        assert_eq!(int(&mut t, Value::str("\x01")), 0);
        assert_eq!(int(&mut t, Value::str("TRUE")), 0);
        assert_eq!(t.output(), "");
        // Out-of-range floats warn (zend_dval_to_lval) and wrap.
        assert_eq!(int(&mut t, Value::Float(1e20)), 7766279631452241920);
        assert!(
            t.output().contains(
                "Warning: The float 1.0E+20 is not representable as an int, cast occurred"
            )
        );
        // Objects warn and give 1.
        let o = std_obj(&t);
        assert_eq!(int(&mut t, o), 1);
        assert!(
            t.output()
                .contains("Warning: Object of class stdClass could not be converted to int")
        );
    }

    #[test]
    fn intval_other_bases() {
        let mut t = TestVm::new();
        assert_eq!(intb(&mut t, "0x12ab", 16), 4779);
        assert_eq!(intb(&mut t, "0Xfff", 16), 4095);
        assert_eq!(intb(&mut t, "0XFA", 16), 250);
        assert_eq!(intb(&mut t, "0123", 16), 291);
        assert_eq!(intb(&mut t, "0123", 8), 83);
        assert_eq!(intb(&mut t, "-2147483648", 8), -9020);
        assert_eq!(intb(&mut t, "0x1A", 0), 26);
        assert_eq!(intb(&mut t, "012", 0), 10);
        assert_eq!(intb(&mut t, "42", 0), 42);
        assert_eq!(intb(&mut t, "z", 36), 35);
        assert_eq!(intb(&mut t, "42", 1), 0);
        assert_eq!(intb(&mut t, "42", 37), 0);
        assert_eq!(intb(&mut t, "42", -2), 0);
        assert_eq!(intb(&mut t, "ffffffffffffffffffff", 16), i64::MAX);
        assert_eq!(intb(&mut t, "-ffffffffffffffffffff", 16), i64::MIN);
        // Non-strings ignore the base.
        same(
            call(&mut t, "intval", vec![Value::Int(42), Value::Int(8)]),
            Value::Int(42),
        );
    }

    #[test]
    fn intval_binary_prefix() {
        // ext/standard/tests/general_functions/intval_binary_prefix.phpt
        let mut t = TestVm::new();
        let ws = " \t\n\r\x0c\x0b";
        let ones = "0b1111111111111111111111111111111";
        let good: Vec<(String, i64)> = vec![
            (ones.to_string(), 2147483647),
            (format!("+{ones}"), 2147483647),
            (format!("-{ones}"), -2147483647),
            (format!("{ws}{ones}"), 2147483647),
            (format!("{ws}+{ones}"), 2147483647),
            (format!("{ws}-{ones}"), -2147483647),
            ("0b".into(), 0),
            ("0B".into(), 0),
            ("0B1".into(), 1),
            ("0b000".into(), 0),
            ("0b001".into(), 1),
            ("0b00100".into(), 4),
            ("0b1 1".into(), 1),
        ];
        for base in [0, 2] {
            for (s, want) in &good {
                assert_eq!(intb(&mut t, s, base), *want, "{s:?} base {base}");
            }
        }
        for (s, _) in &good {
            assert_eq!(int(&mut t, Value::str(s)), 0, "{s:?}");
        }
        for s in ["b101", "0b00200", "--0b123", "++0b123", "0bb123", "0 b123"] {
            assert_eq!(intb(&mut t, s, 0), 0, "{s:?}");
        }
    }

    #[test]
    fn floatval_boolval_strval() {
        // floatval_basic.phpt, boolval.phpt, strval.phpt
        let mut t = TestVm::new();
        let f = |t: &mut TestVm, v: Value| call(t, "floatval", vec![v]).as_float().unwrap();
        assert_eq!(f(&mut t, Value::str("-2147483648")), -2147483648.0);
        assert_eq!(f(&mut t, Value::str("1e5abc")), 100000.0);
        assert_eq!(f(&mut t, Value::str("-.5e+6")), -500000.0);
        assert_eq!(f(&mut t, Value::str("abc")), 0.0);
        assert_eq!(f(&mut t, Value::Int(3)), 3.0);
        assert_eq!(f(&mut t, Value::Bool(true)), 1.0);
        assert_eq!(f(&mut t, Value::str("0x1A")), 0.0);
        assert_eq!(f(&mut t, Value::str(" 1.5 ")), 1.5);
        same(
            call(&mut t, "doubleval", vec![Value::str("2.5")]),
            Value::Float(2.5),
        );

        assert!(!b(&mut t, "boolval", Value::Int(0)));
        assert!(b(&mut t, "boolval", Value::Int(42)));
        assert!(!b(&mut t, "boolval", Value::Float(0.0)));
        assert!(b(&mut t, "boolval", Value::Float(4.2)));
        assert!(!b(&mut t, "boolval", Value::str("")));
        assert!(b(&mut t, "boolval", Value::str("string")));
        assert!(!b(&mut t, "boolval", Value::str("0")));
        assert!(b(&mut t, "boolval", Value::str("0.0")));
        assert!(!b(&mut t, "boolval", Value::empty_array()));
        let one = Value::array(PhpArray::from_values(vec![Value::Int(1)]));
        assert!(b(&mut t, "boolval", one));
        let o = std_obj(&t);
        assert!(b(&mut t, "boolval", o));
        assert!(!b(&mut t, "boolval", Value::Null));

        let sv = |t: &mut TestVm, v: Value| call(t, "strval", vec![v]);
        same(sv(&mut t, Value::Float(1.5e25)), Value::str("1.5E+25"));
        same(sv(&mut t, Value::Bool(true)), Value::str("1"));
        same(sv(&mut t, Value::Bool(false)), Value::str(""));
        same(sv(&mut t, Value::Null), Value::str(""));
        same(sv(&mut t, Value::Int(-7)), Value::str("-7"));
        same(sv(&mut t, Value::Float(0.1 + 0.2)), Value::str("0.3"));
        assert_eq!(t.output(), "");
        same(sv(&mut t, Value::empty_array()), Value::str("Array"));
        assert!(t.output().contains("Warning: Array to string conversion"));
        let o = std_obj(&t);
        assert_eq!(
            thrown(t.call("strval", vec![o])),
            (
                "Error".to_string(),
                "Object of class stdClass could not be converted to string".to_string()
            )
        );
    }

    #[test]
    fn is_type_functions() {
        let mut t = TestVm::new();
        let o = std_obj(&t);
        let (open, closed) = resources();
        let vals = [
            Value::Null,
            Value::Bool(false),
            Value::Int(0),
            Value::Float(0.0),
            Value::str("1"),
            Value::empty_array(),
            o,
            open,
            closed,
        ];
        let (y, n) = (true, false);
        let table: &[(&str, [bool; 9])] = &[
            ("is_null", [y, n, n, n, n, n, n, n, n]),
            ("is_bool", [n, y, n, n, n, n, n, n, n]),
            ("is_int", [n, n, y, n, n, n, n, n, n]),
            ("is_integer", [n, n, y, n, n, n, n, n, n]),
            ("is_long", [n, n, y, n, n, n, n, n, n]),
            ("is_float", [n, n, n, y, n, n, n, n, n]),
            ("is_double", [n, n, n, y, n, n, n, n, n]),
            ("is_string", [n, n, n, n, y, n, n, n, n]),
            ("is_array", [n, n, n, n, n, y, n, n, n]),
            ("is_object", [n, n, n, n, n, n, y, n, n]),
            ("is_resource", [n, n, n, n, n, n, n, y, n]),
            ("is_scalar", [n, y, y, y, y, n, n, n, n]),
            ("is_numeric", [n, n, y, y, y, n, n, n, n]),
            ("is_iterable", [n, n, n, n, n, y, n, n, n]),
            ("is_countable", [n, n, n, n, n, y, n, n, n]),
        ];
        for (f, want) in table {
            for (v, w) in vals.iter().zip(want) {
                assert_eq!(b(&mut t, f, v.clone()), *w, "{f}({v:?})");
            }
        }
        // References are looked through.
        let r = Value::Ref(PhpRef::new(Value::Int(1)));
        assert!(b(&mut t, "is_int", r));
    }

    #[test]
    fn is_numeric_strings() {
        // ext/standard/tests/general_functions/is_numeric.phpt
        let mut t = TestVm::new();
        let numeric = [
            "1", "-1", "1e2", " 1", "1 ", " 1 ", "+1", "-0", ".5", "5.", "-.5e-3", "\t\n1", "1e5",
            "0123",
        ];
        let not_numeric = [
            "1e", "0x1A", "", " ", "1 1", "abc", "1abc", "--1", ".", "e5", "1e+", "0b11", "- 1",
        ];
        for s in numeric {
            assert!(b(&mut t, "is_numeric", Value::str(s)), "{s:?}");
        }
        for s in not_numeric {
            assert!(!b(&mut t, "is_numeric", Value::str(s)), "{s:?}");
        }
        assert!(!b(&mut t, "is_numeric", Value::Null));
        assert!(!b(&mut t, "is_numeric", Value::Bool(true)));
        assert!(b(&mut t, "is_numeric", Value::Float(f64::NAN)));
    }

    #[test]
    fn is_iterable_and_countable_objects() {
        // is_iterable.phpt, is_countable_with_classes.phpt
        let mut t = TestVm::new();
        let traversable = t.vm.find_class(b"Traversable").unwrap();
        let countable = t.vm.find_class(b"Countable").unwrap();
        let mut it = ClassEntry::new("It", 0);
        it.interfaces.push(traversable);
        let it = Value::Object(ObjRef::new(Rc::new(it)));
        let mut cnt = ClassEntry::new("Cnt", 0);
        cnt.interfaces.push(countable);
        let cnt = Value::Object(ObjRef::new(Rc::new(cnt)));
        assert!(b(&mut t, "is_iterable", it.clone()));
        assert!(!b(&mut t, "is_countable", it));
        assert!(b(&mut t, "is_countable", cnt.clone()));
        assert!(!b(&mut t, "is_iterable", cnt));
    }

    fn settype(t: &mut TestVm, v: Value, ty: &str) -> Value {
        let r = PhpRef::new(v);
        same(
            call(t, "settype", vec![Value::Ref(r.clone()), Value::str(ty)]),
            Value::Bool(true),
        );
        r.get()
    }

    #[test]
    fn settype_conversions() {
        // gettype_settype_basic.phpt and variations (64-bit values)
        let mut t = TestVm::new();
        same(
            settype(&mut t, Value::str("10string"), "int"),
            Value::Int(10),
        );
        same(
            settype(&mut t, Value::str("1e2"), "integer"),
            Value::Int(100),
        );
        same(settype(&mut t, Value::Float(-1.5), "INT"), Value::Int(-1));
        same(settype(&mut t, Value::Null, "int"), Value::Int(0));
        same(
            settype(&mut t, Value::str("-.5e-6"), "float"),
            Value::Float(-5e-7),
        );
        same(
            settype(&mut t, Value::Int(10), "double"),
            Value::Float(10.0),
        );
        same(settype(&mut t, Value::Int(-7), "string"), Value::str("-7"));
        same(
            settype(&mut t, Value::Float(1e5), "string"),
            Value::str("100000"),
        );
        same(settype(&mut t, Value::str("0"), "bool"), Value::Bool(false));
        same(
            settype(&mut t, Value::str("0.0"), "boolean"),
            Value::Bool(true),
        );
        same(settype(&mut t, Value::Int(5), "null"), Value::Null);
        let a = settype(&mut t, Value::Int(5), "array");
        same(
            a.as_array().unwrap().get_int(0).unwrap().clone(),
            Value::Int(5),
        );
        let a = settype(&mut t, Value::Null, "array");
        assert!(a.as_array().unwrap().is_empty());
        let o = settype(&mut t, Value::str("x"), "object");
        let o = o.as_object().unwrap().clone();
        assert_eq!(o.class().name, "stdClass");
        assert!(o.get_prop(b"scalar").unwrap().identical(&Value::str("x")));
        assert_eq!(t.output(), "");
        let arr = PhpArray::from_values(vec![Value::Int(1), Value::Int(2)]);
        same(
            settype(&mut t, Value::array(arr), "string"),
            Value::str("Array"),
        );
        assert!(t.output().contains("Warning: Array to string conversion"));
    }

    #[test]
    fn settype_errors() {
        // gettype_settype_error.phpt
        let mut t = TestVm::new();
        let r = PhpRef::new(Value::Null);
        assert_eq!(
            thrown(t.call(
                "settype",
                vec![Value::Ref(r.clone()), Value::str("unknown")]
            )),
            (
                "ValueError".to_string(),
                "settype(): Argument #2 ($type) must be a valid type".to_string()
            )
        );
        assert_eq!(
            thrown(t.call("settype", vec![Value::Ref(r), Value::str("resource")])),
            (
                "ValueError".to_string(),
                "Cannot convert to resource type".to_string()
            )
        );
        // NaN conversions warn.
        settype(&mut t, Value::Float(f64::NAN), "null");
        assert!(
            t.output()
                .contains("Warning: unexpected NAN value was coerced to null")
        );
        settype(&mut t, Value::Float(f64::NAN), "bool");
        assert!(
            t.output()
                .contains("Warning: unexpected NAN value was coerced to bool")
        );
        // A failed string conversion leaves "" behind.
        let r = PhpRef::new(std_obj(&t));
        assert!(
            t.call("settype", vec![Value::Ref(r.clone()), Value::str("string")])
                .is_err()
        );
        same(r.get(), Value::str(""));
    }

    #[test]
    fn deprecated_aliases_are_flagged() {
        let t = TestVm::new();
        for name in ["doubleval", "is_integer", "is_long", "is_double"] {
            let f = t.vm.find_function(name).unwrap();
            assert!(f.has_flag(fn_flags::DEPRECATED), "{name}");
        }
        let is_int = t.vm.find_function("is_int").unwrap();
        assert!(!is_int.has_flag(fn_flags::DEPRECATED));
        assert_eq!(
            super::deprecation_message("doubleval").unwrap(),
            "Function doubleval() is deprecated since 8.6, use floatval() instead"
        );
    }
}
