//! Expectations from php-src's `ext/standard/tests/strings/sscanf*.phpt` and
//! the sscanf `bug*.phpt` / `gh15552.phpt` tests.

use php_runtime::value::float_fmt;
use php_runtime::{PhpRef, Value};

use crate::testing::{TestVm, thrown};

/// A compact `var_dump`-like rendering.
fn dump(v: &Value) -> String {
    match v {
        Value::Null | Value::Undef => "NULL".into(),
        Value::Bool(b) => format!("bool({b})"),
        Value::Int(i) => format!("int({i})"),
        Value::Float(d) => format!("float({})", float_fmt::format_h(*d, -1)),
        Value::String(s) => format!("\"{}\"", String::from_utf8_lossy(s)),
        Value::Array(a) => {
            let items: Vec<String> = a.iter().map(|(_, v)| dump(v)).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Ref(r) => dump(&r.get()),
        other => format!("{other:?}"),
    }
}

/// `sscanf($string, $format)` (array mode), rendered with [`dump`].
fn scan(string: &[u8], format: &[u8]) -> String {
    let mut t = TestVm::new();
    let v = t
        .call("sscanf", vec![Value::bytes(string), Value::bytes(format)])
        .unwrap();
    assert_eq!(t.output(), "");
    dump(&v)
}

/// `sscanf($string, $format, ...$vars)` with the given initial variable
/// values; returns the result and the variables afterwards.
fn scan_into(string: &str, format: &str, initial: Vec<Value>) -> (String, Vec<String>) {
    let mut t = TestVm::new();
    let refs: Vec<PhpRef> = initial.into_iter().map(PhpRef::new).collect();
    let mut args = vec![Value::str(string), Value::str(format)];
    args.extend(refs.iter().cloned().map(Value::Ref));
    let v = t.call("sscanf", args).unwrap();
    (dump(&v), refs.iter().map(|r| dump(&r.get())).collect())
}

fn scan_vars(string: &str, format: &str, n: usize) -> (String, Vec<String>) {
    scan_into(string, format, vec![Value::Null; n])
}

fn scan_err(string: &[u8], format: &[u8], nvars: usize) -> (String, String) {
    let mut t = TestVm::new();
    let mut args = vec![Value::bytes(string), Value::bytes(format)];
    args.extend((0..nvars).map(|_| Value::Ref(PhpRef::new(Value::Null))));
    thrown(t.call("sscanf", args))
}

fn value_error(msg: &str) -> (String, String) {
    ("ValueError".to_string(), msg.to_string())
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn basic_string_int_float_formats() {
    let s = b"Part: Widget Serial Number: 1234789 Stock: 25";
    assert_eq!(
        scan(s, b"Part: %s Serial Number: %s Stock: %s"),
        r#"["Widget", "1234789", "25"]"#
    );
    assert_eq!(
        scan(s, b"Part: %s Serial Number: %d Stock: %d"),
        r#"["Widget", int(1234789), int(25)]"#
    );
    let (r, vars) = scan_vars(
        "Part: Widget Serial Number: 1234789 Stock: 25",
        "Part: %s Serial Number: %d Stock: %d",
        3,
    );
    assert_eq!(r, "int(3)");
    assert_eq!(vars, strs(&["\"Widget\"", "int(1234789)", "int(25)"]));

    let s = b"Part: Widget Length: 111.53 Width: 22.345 Depth: 12.4";
    let f = b"Part: %s Length: %f Width: %f Depth: %f";
    assert_eq!(
        scan(s, f),
        r#"["Widget", float(111.53), float(22.345), float(12.4)]"#
    );
}

#[test]
fn basic_char_and_exponent_formats() {
    assert_eq!(
        scan(b"X = A + B - C", b"%c = %c + %c - %c"),
        r#"["X", "A", "B", "C"]"#
    );
    let (r, vars) = scan_vars("X = A + B - C", "%c = %c + %c - %c", 4);
    assert_eq!(r, "int(4)");
    assert_eq!(vars, strs(&["\"X\"", "\"A\"", "\"B\"", "\"C\""]));

    let s = b"10.12345 10.12345E3 10.12345e3 -10.12345e4";
    let want = "[float(10.12345), float(10123.45), float(10123.45), float(-101234.5)]";
    assert_eq!(scan(s, b"%e %e %e %e"), want);
    assert_eq!(scan(s, b"%E %E %E %E"), want);
    assert_eq!(scan(s, b"%f %g %lf %Lf"), want);
}

#[test]
fn basic_unsigned_octal_hex_formats() {
    // sscanf_basic6.phpt (64-bit values).
    assert_eq!(
        scan(b"-11 +11 11 -11.234 +11.234 11.234", b"%u %u %u %u %u %u"),
        r#"["18446744073709551605", int(11), int(11), "18446744073709551605", NULL, NULL]"#
    );
    let (r, vars) = scan_vars("-11 +11 11 -11.234 +11.234 11.234", "%u %u %u %u %u %u", 6);
    assert_eq!(r, "int(4)");
    assert_eq!(vars[0], "\"18446744073709551605\"");
    assert_eq!(vars[5], "NULL");
    // sscanf_basic7.phpt
    assert_eq!(
        scan(b"0123 -0123 +0123 0129 -0129 +0129", b"%o %o %o %o %o %o"),
        "[int(83), int(-83), int(83), int(10), NULL, NULL]"
    );
    // sscanf_basic8.phpt
    let want = "[int(297), int(303), int(4667), int(-4667), int(6844), int(1)]";
    assert_eq!(
        scan(b"129 12F 123B -123B 01ABC 1G", b"%x %x %x %x %x %x"),
        want
    );
    assert_eq!(
        scan(b"129 12F 123B -123B 01ABC 1G", b"%X %X %X %X %X %X"),
        want
    );
    let (r, _) = scan_vars("129 12F 123B -123B 01ABC 1G", "%x %x %x %x %x %x", 6);
    assert_eq!(r, "int(6)");
}

#[test]
fn integer_details() {
    // bug47842.phpt
    assert_eq!(scan(b"2147483647", b"%d"), "[int(2147483647)]");
    assert_eq!(scan(b"4294967295", b"%u"), "[int(4294967295)]");
    assert_eq!(
        scan(b"9223372036854775807", b"%d"),
        "[int(9223372036854775807)]"
    );
    assert_eq!(
        scan(b"18446744073709551615", b"%u"),
        r#"["18446744073709551615"]"#
    );
    // strtol saturates, strtoul too.
    assert_eq!(
        scan(b"99999999999999999999", b"%d"),
        "[int(9223372036854775807)]"
    );
    assert_eq!(
        scan(b"-99999999999999999999", b"%d"),
        "[int(-9223372036854775808)]"
    );
    assert_eq!(
        scan(b"99999999999999999999", b"%u"),
        r#"["18446744073709551615"]"#
    );
    // %i detects the base.
    assert_eq!(
        scan(b"0x1A 012 123", b"%i %i %i"),
        "[int(26), int(10), int(123)]"
    );
    assert_eq!(scan(b"-0x1f", b"%i%s"), r#"[int(0), "x1f"]"#);
    assert_eq!(scan(b"0x1f", b"%x"), "[int(31)]");
    // A trailing x after a 0 is given back.
    assert_eq!(scan(b"0xg", b"%x%s"), r#"[int(0), "xg"]"#);
    assert_eq!(scan(b"08", b"%o%d"), "[int(0), int(8)]");
    // Widths, signs and the D / size modifiers.
    assert_eq!(scan(b"12345", b"%2d%3d"), "[int(12), int(345)]");
    assert_eq!(scan(b"+5 -7", b"%D %ld"), "[int(5), int(-7)]");
    assert_eq!(scan(b"--5", b"%d"), "[NULL]");
    assert_eq!(scan(b"  42", b"%d"), "[int(42)]");
}

#[test]
fn float_details() {
    assert_eq!(scan(b"1e5", b"%f"), "[float(100000)]");
    assert_eq!(scan(b"-.5", b"%f"), "[float(-0.5)]");
    // A dangling exponent (with or without sign) is given back.
    assert_eq!(scan(b"1e+x", b"%f%s"), r#"[float(1), "e+x"]"#);
    assert_eq!(scan(b"2ex", b"%f%s"), r#"[float(2), "ex"]"#);
    assert_eq!(scan(b"3e", b"%f%s"), r#"[float(3), "e"]"#);
    assert_eq!(scan(b".e5", b"%f"), "[NULL]");
    assert_eq!(scan(b"1.5.5", b"%f%s"), r#"[float(1.5), ".5"]"#);
    assert_eq!(scan(b"12.345", b"%3f%f"), "[float(12), float(345)]");
}

#[test]
fn strings_chars_and_sets() {
    assert_eq!(scan(b"abcdef", b"%3s%s"), r#"["abc", "def"]"#);
    // %c with a width still stops at whitespace (and does not skip it).
    assert_eq!(scan(b"ab cd", b"%5c"), r#"["ab"]"#);
    assert_eq!(scan(b" a", b"%c"), r#"[""]"#);
    assert_eq!(scan(b"abcd", b"%[a-c]"), r#"["abc"]"#);
    assert_eq!(scan(b"abcd", b"%[c-a]"), r#"["abc"]"#);
    assert_eq!(scan(b"foo,bar", b"%[^,],%s"), r#"["foo", "bar"]"#);
    assert_eq!(scan(b"]a]b", b"%[]a]"), r#"["]a]"]"#);
    assert_eq!(scan(b"a-b", b"%[a-]"), r#"["a-"]"#);
    assert_eq!(scan(b"-ab", b"%[-a]"), r#"["-a"]"#);
    assert_eq!(scan(b"abcabc", b"%2[abc]%s"), r#"["ab", "cabc"]"#);
    // Nothing matched: the scan stops (no underflow).
    assert_eq!(scan(b"xyz", b"%[a-c]"), "[NULL]");
    // bug27295.phpt
    let (r, vars) = scan_vars("foo = bar", "%s = %[^[]]", 2);
    assert_eq!(r, "int(2)");
    assert_eq!(vars, strs(&["\"foo\"", "\"bar\""]));
    // Ranges compare signed chars: 'a'-'\xff' is the range '\xff'..'a'.
    assert_eq!(scan(b"a\x01b", b"%[a-\xff]"), "[\"a\u{1}\"]");
    assert_eq!(scan(b"\xff\xfe", b"%[\xfe-\xff]"), "[\"\u{fffd}\u{fffd}\"]");
}

#[test]
fn suppression_positions_and_literals() {
    assert_eq!(scan(b"12 34", b"%*d %d"), "[int(34)]");
    assert_eq!(scan(b"abc def", b"%*s%n"), "[int(3)]");
    // bug21730.phpt
    let (r, vars) = scan_vars("ABC = DEF", "%s = %s %n", 3);
    assert_eq!(r, "int(3)");
    assert_eq!(vars, strs(&["\"ABC\"", "\"DEF\"", "int(9)"]));
    assert_eq!(
        scan(b"ABC = DEF", b"%s = %s %n"),
        r#"["ABC", "DEF", int(9)]"#
    );
    assert_eq!(scan(b"  abc", b"%s%n"), r#"["abc", int(5)]"#);
    // %n does not need more input.
    assert_eq!(scan(b"", b"%n"), "[int(0)]");
    assert_eq!(scan(b"100%", b"%d%%"), "[int(100)]");
    assert_eq!(scan(b"100x", b"%d%%%n"), "[int(100), NULL]");
    assert_eq!(scan(b"abc", b"x%s"), "[NULL]");
    // The input and the format end at a NUL byte.
    assert_eq!(scan(b"ab\0cd", b"%s"), r#"["ab"]"#);
    assert_eq!(scan(b"ab cd", b"%s\0%s"), r#"["ab"]"#);
}

#[test]
fn underflow_results() {
    assert_eq!(scan(b"", b"%d"), "NULL");
    assert_eq!(scan(b"   ", b"%s"), "NULL");
    assert_eq!(scan(b"", b"x%d"), "NULL");
    assert_eq!(scan(b"abc", b"%d"), "[NULL]");
    assert_eq!(scan(b"12", b"%d %d"), "[int(12), NULL]");
    assert_eq!(scan(b"-", b"%d"), "NULL");
    let (r, vars) = scan_vars("", "%d", 1);
    assert_eq!(r, "int(-1)");
    assert_eq!(vars, strs(&["NULL"]));
    let (r, _) = scan_vars("abc", "%d", 1);
    assert_eq!(r, "int(0)");
    // bug38322.phpt
    let (r, vars) = scan_vars("a ", "%1$s", 1);
    assert_eq!(r, "int(1)");
    assert_eq!(vars, strs(&["\"a\""]));
}

#[test]
fn xpg_positional_specifiers() {
    // bug42107.phpt
    assert_eq!(scan(b"one two", b"%1$s %2$s"), r#"["one", "two"]"#);
    assert_eq!(scan(b"one two", b"%2$s %1$s"), r#"["two", "one"]"#);
    let (_, vars) = scan_vars("one two", "%1$s %2$s", 2);
    assert_eq!(vars, strs(&["\"one\"", "\"two\""]));
    let (_, vars) = scan_vars("one two", "%2$s %1$s", 2);
    assert_eq!(vars, strs(&["\"two\"", "\"one\""]));
    assert_eq!(scan(b"one two", b"%1$d %2$d"), "[NULL, NULL]");
    assert_eq!(scan(b"one two", b"%1$d"), "[NULL]");
    // Without variables, gaps are allowed.
    assert_eq!(scan(b"7", b"%3$d"), "[NULL, NULL, int(7)]");
    assert_eq!(scan(b"7", b"%255$d").matches("NULL").count(), 254);
}

#[test]
fn previous_values_survive_failed_conversions() {
    // bug47322.phpt
    let (_, vars) = scan_vars(":59:58", "%s:%d:%f", 3);
    assert_eq!(vars, strs(&["\":59:58\"", "NULL", "NULL"]));
    let (_, vars) = scan_vars("15:01:58.2", "%d:%f:%f", 3);
    assert_eq!(vars, strs(&["int(15)", "float(1)", "float(58.2)"]));
    let (r, vars) = scan_into(
        "15.1111::foo",
        "%f:%d:%s",
        vec![Value::Int(15), Value::Float(1.0), Value::Float(58.2)],
    );
    assert_eq!(r, "int(1)");
    assert_eq!(vars, strs(&["float(15.1111)", "float(1)", "float(58.2)"]));
}

#[test]
fn format_errors() {
    // sscanf_error.phpt
    assert_eq!(
        scan_err(b"Hello World", b"%s %s", 3),
        value_error("Variable is not assigned by any conversion specifiers")
    );
    assert_eq!(
        scan_err(b"Hello World", b"%s %s", 1),
        value_error("Different numbers of variable names and field specifiers")
    );
    // gh15552.phpt
    assert_eq!(
        scan_err(b"hello", b"%2147483648$s", 0),
        value_error("\"%n$\" argument index out of range")
    );
    assert_eq!(
        scan_err(b"hello", b"%256$s", 0),
        value_error("\"%n$\" argument index out of range")
    );
    assert_eq!(
        scan_err(b"hello", b"%2$s", 1),
        value_error("\"%n$\" argument index out of range")
    );
    assert_eq!(
        scan_err(b"hello", b"%0$s", 0),
        value_error("\"%n$\" argument index out of range")
    );
    assert_eq!(
        scan_err(b"a b", b"%1$s %s", 0),
        value_error("cannot mix \"%\" and \"%n$\" conversion specifiers")
    );
    assert_eq!(
        scan_err(b"a b", b"%s %1$s", 0),
        value_error("cannot mix \"%\" and \"%n$\" conversion specifiers")
    );
    assert_eq!(
        scan_err(b"a b", b"%1$s %1$s", 0),
        value_error("Variable is assigned by multiple \"%n$\" conversion specifiers")
    );
    assert_eq!(
        scan_err(b"a b", b"%2$s", 2),
        value_error("Variable is not assigned by any conversion specifiers")
    );
    assert_eq!(
        scan_err(b"abc", b"%z", 0),
        value_error("Bad scan conversion character \"z\"")
    );
    assert_eq!(
        scan_err(b"abc", b"%G", 0),
        value_error("Bad scan conversion character \"G\"")
    );
    assert_eq!(
        scan_err(b"abc", b"abc %", 0),
        value_error("Bad scan conversion character \"\0\"")
    );
    assert_eq!(
        scan_err(b"abc", b"%*", 0),
        value_error("Bad scan conversion character \"\0\"")
    );
    for f in [&b"%["[..], b"%[abc", b"%[^", b"%[]", b"%[^]"] {
        assert_eq!(
            scan_err(b"abc", f, 0),
            value_error("Unmatched [ in format string"),
            "{f:?}"
        );
    }
    // The variable count is checked before the conversion character.
    assert_eq!(
        scan_err(b"abc", b"%d %z", 1),
        value_error("Different numbers of variable names and field specifiers")
    );
    // Suppressed conversions need no variable (but are counted).
    let (r, vars) = scan_vars("1 2", "%*d %d", 1);
    assert_eq!(r, "int(2)");
    assert_eq!(vars, strs(&["int(2)"]));
}

#[test]
fn signature_checks() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("sscanf", vec![Value::str("a")]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "sscanf() expects at least 2 arguments, 1 given");
}
