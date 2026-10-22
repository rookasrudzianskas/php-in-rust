//! Expectations from php-src's `ext/standard/tests/strings/{sprintf,printf,
//! vsprintf,vprintf,fprintf,vfprintf}*.phpt` and the related `bug*.phpt`.

use php_runtime::{PhpArray, PhpRef, PhpStr, Value};

use super::php_conv_fp;
use crate::testing::{TestVm, thrown};

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(d: f64) -> Value {
    Value::Float(d)
}

fn st(s: &str) -> Value {
    Value::str(s)
}

fn list(values: Vec<Value>) -> Value {
    let mut a = PhpArray::new();
    for v in values {
        a.append(v).unwrap();
    }
    Value::array(a)
}

#[track_caller]
fn assert_val(got: &Value, want: &Value) {
    assert!(got.identical(want), "got {got:?}, want {want:?}");
}

fn call(t: &mut TestVm, name: &str, format: &str, values: Vec<Value>) -> Value {
    let mut args = vec![Value::bytes(format.as_bytes())];
    args.extend(values);
    t.call(name, args).unwrap()
}

/// `sprintf($format, ...$values)` as bytes.
fn spb(format: &[u8], values: Vec<Value>) -> Vec<u8> {
    let mut t = TestVm::new();
    let mut args = vec![Value::bytes(format)];
    args.extend(values);
    match t.call("sprintf", args).unwrap() {
        Value::String(s) => s.to_vec(),
        v => panic!("sprintf returned {v:?}"),
    }
}

/// `sprintf($format, ...$values)` as a (lossy) string; no diagnostics allowed.
fn sp(format: &str, values: Vec<Value>) -> String {
    let mut t = TestVm::new();
    let v = call(&mut t, "sprintf", format, values);
    assert_eq!(t.output(), "", "unexpected diagnostics for {format:?}");
    match v {
        Value::String(s) => String::from_utf8_lossy(&s).into_owned(),
        v => panic!("sprintf returned {v:?}"),
    }
}

/// Class and message of the exception thrown by `sprintf`.
fn sp_err(format: &str, values: Vec<Value>) -> (String, String) {
    let mut t = TestVm::new();
    let mut args = vec![Value::bytes(format.as_bytes())];
    args.extend(values);
    thrown(t.call("sprintf", args))
}

fn value_error(msg: &str) -> (String, String) {
    ("ValueError".to_string(), msg.to_string())
}

fn count_error(msg: &str) -> (String, String) {
    ("ArgumentCountError".to_string(), msg.to_string())
}

fn check_all(format: &str, cases: &[(Value, &str)]) {
    for (v, want) in cases {
        assert_eq!(sp(format, vec![v.clone()]), *want, "{format} with {v:?}");
    }
}

#[test]
fn plain_text_and_percent() {
    assert_eq!(sp("", vec![]), "");
    assert_eq!(sp("hello", vec![]), "hello");
    assert_eq!(sp("%%", vec![float(1.23456789e10)]), "%");
    assert_eq!(sp("%%%.2f", vec![float(1.23456789e10)]), "%12345678900.00");
    assert_eq!(sp("100%% sure", vec![]), "100% sure");
    // Extra arguments are ignored.
    assert_eq!(sp("%s", vec![st("a"), st("b"), st("c")]), "a");
    assert_eq!(spb(b"a\0b%s", vec![st("c")]), b"a\0bc");
}

#[test]
fn strings_padding_and_precision() {
    check_all(
        "%5s",
        &[
            (Value::Null, "     "),
            (st("abc"), "  abc"),
            (st("aaa"), "  aaa"),
        ],
    );
    check_all("%-5s", &[(Value::Null, "     "), (st("abc"), "abc  ")]);
    check_all("%05s", &[(Value::Null, "00000"), (st("abc"), "00abc")]);
    check_all("%'#5s", &[(Value::Null, "#####"), (st("abc"), "##abc")]);
    assert_eq!(sp("[%s]", vec![st("monkey")]), "[monkey]");
    assert_eq!(sp("[%10s]", vec![st("monkey")]), "[    monkey]");
    assert_eq!(sp("[%-10s]", vec![st("monkey")]), "[monkey    ]");
    assert_eq!(sp("[%010s]", vec![st("monkey")]), "[0000monkey]");
    assert_eq!(sp("[%'#10s]", vec![st("monkey")]), "[####monkey]");
    assert_eq!(sp("[%10.10s]", vec![st("many monkeys")]), "[many monke]");
    assert_eq!(sp("%.5s", vec![st("James Bond, 007")]), "James");
    assert_eq!(sp("%-3.3s", vec![st("abcdef")]), "abc");
    assert_eq!(sp("%.0s|", vec![st("abc")]), "|");
    // `'0'` padding of a left-aligned string pads on the right.
    assert_eq!(sp("%-05s", vec![st("ab")]), "ab000");
    // Bug #20108.
    let z = sp("%580.58s\n", vec![st("boo")]);
    assert_eq!(z.len(), 581);
    assert_eq!(z, format!("{}boo\n", " ".repeat(577)));
    assert_eq!(
        sp("abc%04d %-20s%c", vec![int(20), st("fisketur"), int(33)]),
        "abc0020 fisketur            !"
    );
}

#[test]
fn string_conversions() {
    check_all(
        "%s",
        &[
            (Value::Null, ""),
            (Value::Bool(false), ""),
            (Value::Bool(true), "1"),
            (int(2), "2"),
            (float(3.5), "3.5"),
            (st("foo"), "foo"),
            (float(0.1 + 0.2), "0.3"),
            (float(1e25), "1.0E+25"),
        ],
    );
    let mut t = TestVm::new();
    let v = call(&mut t, "sprintf", "%s", vec![Value::array(PhpArray::new())]);
    assert_val(&v, &Value::str("Array"));
    assert_eq!(
        t.output(),
        "\nWarning: Array to string conversion in /t.php on line 3\n"
    );
}

#[test]
fn integers() {
    let numbers = [
        int(0),
        int(1),
        int(-1),
        float(2.7),
        float(-2.7),
        int(23333333),
        int(-23333333),
        st("1234"),
    ];
    let table: &[(&str, [&str; 8])] = &[
        (
            "%d",
            ["0", "1", "-1", "2", "-2", "23333333", "-23333333", "1234"],
        ),
        (
            "%+d",
            [
                "+0",
                "+1",
                "-1",
                "+2",
                "-2",
                "+23333333",
                "-23333333",
                "+1234",
            ],
        ),
        (
            "%7.2d",
            [
                "      0",
                "      1",
                "     -1",
                "      2",
                "     -2",
                "23333333",
                "-23333333",
                "   1234",
            ],
        ),
        (
            "%-7.2d",
            [
                "0      ",
                "1      ",
                "-1     ",
                "2      ",
                "-2     ",
                "23333333",
                "-23333333",
                "1234   ",
            ],
        ),
        (
            "%07.2d",
            [
                "0000000",
                "0000001",
                "-000001",
                "0000002",
                "-000002",
                "23333333",
                "-23333333",
                "0001234",
            ],
        ),
        (
            // Integers can't be right-padded with zeros.
            "%-07.2d",
            [
                "0      ",
                "1      ",
                "-1     ",
                "2      ",
                "-2     ",
                "23333333",
                "-23333333",
                "1234   ",
            ],
        ),
        (
            "%'#7.2d",
            [
                "######0",
                "######1",
                "#####-1",
                "######2",
                "#####-2",
                "23333333",
                "-23333333",
                "###1234",
            ],
        ),
        (
            "%u",
            [
                "0",
                "1",
                "18446744073709551615",
                "2",
                "18446744073709551614",
                "23333333",
                "18446744073686218283",
                "1234",
            ],
        ),
        (
            "%b",
            [
                "0",
                "1",
                "1111111111111111111111111111111111111111111111111111111111111111",
                "10",
                "1111111111111111111111111111111111111111111111111111111111111110",
                "1011001000000100111010101",
                "1111111111111111111111111111111111111110100110111111011000101011",
                "10011010010",
            ],
        ),
        (
            "%o",
            [
                "0",
                "1",
                "1777777777777777777777",
                "2",
                "1777777777777777777776",
                "131004725",
                "1777777777777646773053",
                "2322",
            ],
        ),
        (
            "%x",
            [
                "0",
                "1",
                "ffffffffffffffff",
                "2",
                "fffffffffffffffe",
                "16409d5",
                "fffffffffe9bf62b",
                "4d2",
            ],
        ),
        (
            "%e",
            [
                "0.000000e+0",
                "1.000000e+0",
                "-1.000000e+0",
                "2.700000e+0",
                "-2.700000e+0",
                "2.333333e+7",
                "-2.333333e+7",
                "1.234000e+3",
            ],
        ),
    ];
    for (format, expected) in table {
        for (v, want) in numbers.iter().zip(expected.iter()) {
            assert_eq!(sp(format, vec![v.clone()]), *want, "{format} with {v:?}");
        }
    }
}

#[test]
fn integer_signs_and_extremes() {
    // Bug #26973.
    assert_eq!(sp("%+05d", vec![int(200)]), "+0200");
    assert_eq!(sp("%+05d", vec![int(-200)]), "-0200");
    assert_eq!(sp("%+05f", vec![int(200)]), "+200.000000");
    assert_eq!(sp("%+05f", vec![int(-200)]), "-200.000000");
    assert_eq!(sp("%+05u", vec![int(200)]), "00200");
    assert_eq!(sp("%+05u", vec![int(-200)]), "18446744073709551416");
    assert_eq!(sp("%05d", vec![int(-200)]), "-0200");
    assert_eq!(sp("%05u", vec![int(200)]), "00200");
    // Bug #23894.
    assert_eq!(sp("%04d", vec![float(-12.3456)]), "-012");
    assert_eq!(sp("% 13u", vec![float(-12.3456)]), "18446744073709551604");
    // Bug #47842.
    assert_eq!(
        sp("%d", vec![int(i64::MAX)]),
        "9223372036854775807".to_string()
    );
    assert_eq!(sp("%d", vec![int(i64::MIN)]), "-9223372036854775808");
    assert_eq!(sp("%+d", vec![int(i64::MIN)]), "-9223372036854775808");
    assert_eq!(sp("%x", vec![int(i64::MIN)]), "8000000000000000");
    assert_eq!(sp("%X", vec![int(-1)]), "FFFFFFFFFFFFFFFF");
    // The `l` size modifier is ignored.
    assert_eq!(sp("%ld %lu", vec![int(-5), int(5)]), "-5 5");
    // Docs examples (sprintf_f_2.phpt).
    let n = int(43951789);
    let u = int(-43951789);
    assert_eq!(
        sp("%%b = '%b'", vec![n.clone()]),
        "%b = '10100111101010011010101101'"
    );
    assert_eq!(sp("%%c = '%c'", vec![int(65)]), "%c = 'A'");
    assert_eq!(sp("%%e = '%e'", vec![n.clone()]), "%e = '4.395179e+7'");
    assert_eq!(
        sp("%%u = '%u'", vec![u.clone()]),
        "%u = '18446744073665599827'"
    );
    assert_eq!(sp("%%f = '%f'", vec![n.clone()]), "%f = '43951789.000000'");
    assert_eq!(sp("%%o = '%o'", vec![n.clone()]), "%o = '247523255'");
    assert_eq!(sp("%%x = '%x'", vec![n.clone()]), "%x = '29ea6ad'");
    assert_eq!(sp("%%X = '%X'", vec![n.clone()]), "%X = '29EA6AD'");
    assert_eq!(sp("%%+d = '%+d'", vec![n]), "%+d = '+43951789'");
    assert_eq!(sp("%%+d = '%+d'", vec![u]), "%+d = '-43951789'");
    assert_eq!(
        sp("%04d-%02d-%02d", vec![int(2006), int(12), int(18)]),
        "2006-12-18"
    );
    assert_eq!(
        sp(
            "%4o %4x %4X %08x",
            vec![int(128), int(1024), int(49151), int(3457925)]
        ),
        " 200  400 BFFF 0034c385"
    );
    assert_eq!(sp("%d", vec![float(100.9)]), "100");
    assert_eq!(sp("%o", vec![float(100.9)]), "144");
    assert_eq!(sp("%d", vec![st("abcdefghjklmnpqrstuvwxyz")]), "0");
}

#[test]
fn base_conversions_with_padding_and_precision() {
    // append2n pads zeros on either side and truncates to the precision
    // (its maximum width is 0).
    assert_eq!(sp("%-05x", vec![int(255)]), "ff000");
    assert_eq!(sp("%05x", vec![int(255)]), "000ff");
    assert_eq!(sp("%.1x|", vec![int(255)]), "|");
    assert_eq!(sp("%5.1x|", vec![int(255)]), "     |");
    assert_eq!(sp("%+x", vec![int(255)]), "ff");
}

#[test]
fn chars() {
    assert_eq!(spb(b"%c", vec![st("a")]), b"\0");
    assert_eq!(spb(b"%c", vec![int(67)]), b"C");
    assert_eq!(spb(b"%c", vec![int(-67)]), b"\xbd");
    assert_eq!(spb(b"%c", vec![int(99)]), b"c");
    // Width and padding are ignored.
    assert_eq!(spb(b"%5c|% c", vec![int(1234), int(65)]), b"\xd2|A");
}

#[test]
fn floats_fixed() {
    let numbers = [
        int(0),
        int(1),
        int(-1),
        float(0.32),
        float(-0.32),
        float(2.54),
        float(-2.54),
        float(1.2345678e99),
        float(-1.2345678e99),
    ];
    let big = "1234567799999999974395005011934353453808157274826416694779123657996479008398428960266851879135215616";
    let table: &[(&str, [String; 9])] = &[
        (
            "%f",
            [
                "0.000000".into(),
                "1.000000".into(),
                "-1.000000".into(),
                "0.320000".into(),
                "-0.320000".into(),
                "2.540000".into(),
                "-2.540000".into(),
                format!("{big}.000000"),
                format!("-{big}.000000"),
            ],
        ),
        (
            "%+f",
            [
                "+0.000000".into(),
                "+1.000000".into(),
                "-1.000000".into(),
                "+0.320000".into(),
                "-0.320000".into(),
                "+2.540000".into(),
                "-2.540000".into(),
                format!("+{big}.000000"),
                format!("-{big}.000000"),
            ],
        ),
        (
            "%7.2f",
            [
                "   0.00".into(),
                "   1.00".into(),
                "  -1.00".into(),
                "   0.32".into(),
                "  -0.32".into(),
                "   2.54".into(),
                "  -2.54".into(),
                format!("{big}.00"),
                format!("-{big}.00"),
            ],
        ),
        (
            "%-7.2f",
            [
                "0.00   ".into(),
                "1.00   ".into(),
                "-1.00  ".into(),
                "0.32   ".into(),
                "-0.32  ".into(),
                "2.54   ".into(),
                "-2.54  ".into(),
                format!("{big}.00"),
                format!("-{big}.00"),
            ],
        ),
        (
            "%07.2f",
            [
                "0000.00".into(),
                "0001.00".into(),
                "-001.00".into(),
                "0000.32".into(),
                "-000.32".into(),
                "0002.54".into(),
                "-002.54".into(),
                format!("{big}.00"),
                format!("-{big}.00"),
            ],
        ),
        (
            // Floats *are* right-padded with zeros.
            "%-07.2f",
            [
                "0.00000".into(),
                "1.00000".into(),
                "-1.0000".into(),
                "0.32000".into(),
                "-0.3200".into(),
                "2.54000".into(),
                "-2.5400".into(),
                format!("{big}.00"),
                format!("-{big}.00"),
            ],
        ),
        (
            "%'#7.2f",
            [
                "###0.00".into(),
                "###1.00".into(),
                "##-1.00".into(),
                "###0.32".into(),
                "##-0.32".into(),
                "###2.54".into(),
                "##-2.54".into(),
                format!("{big}.00"),
                format!("-{big}.00"),
            ],
        ),
    ];
    for (format, expected) in table {
        for (v, want) in numbers.iter().zip(expected.iter()) {
            assert_eq!(sp(format, vec![v.clone()]), *want, "{format} with {v:?}");
        }
    }
}

#[test]
fn floats_fixed_misc() {
    // sprintf_f.phpt
    assert_eq!(sp("%3.2f", vec![float(1.2)]), "1.20");
    assert_eq!(sp("%-03.2f", vec![float(1.2)]), "1.20");
    assert_eq!(sp("%5.2f", vec![float(3.4)]), " 3.40");
    assert_eq!(sp("%-05.2f", vec![float(3.4)]), "3.400");
    assert_eq!(sp("%07.2f", vec![float(-5.6)]), "-005.60");
    assert_eq!(sp("%-07.2f", vec![float(-5.6)]), "-5.6000");
    assert_eq!(sp("%3.4f", vec![float(1.2345678e99)]).len(), 105);
    // sprintf_f_2.phpt
    assert_eq!(sp("%.3F", vec![float(100.426)]), "100.426");
    assert_eq!(sp("%.2F", vec![float(100.426)]), "100.43");
    assert_eq!(sp("%01.2f", vec![float(68.75 + 54.35)]), "123.10");
    assert_eq!(sp("%F", vec![float(1.23456789e10)]), "12345678900.000000");
    // Bug #28633.
    assert_eq!(sp("%05.2f", vec![float(0.02)]), "00.02");
    assert_eq!(sp("%05.2f", vec![float(2.02)]), "02.02");
    // general_functions/001.phpt
    // The phpt compares with `==`: "3333.33" == "003333.33".
    assert_eq!(sp("%006.2f", vec![float(10000.0 / 3.0)]), "3333.33");
    assert_eq!(sp("%.2f", vec![st("99.00")]), "99.00");
    // Rounding is exact and half-to-even on the binary value.
    assert_eq!(sp("%.2f", vec![float(2.675)]), "2.67");
    assert_eq!(sp("%.1f", vec![float(0.25)]), "0.2");
    assert_eq!(sp("%.0f", vec![float(0.5)]), "0");
    assert_eq!(sp("%.0f", vec![float(1.5)]), "2");
    assert_eq!(sp("%.0f", vec![float(2.5)]), "2");
    assert_eq!(sp("%.0f", vec![float(-0.4)]), "-0");
    assert_eq!(sp("%.2f", vec![float(-0.001)]), "-0.00");
    assert_eq!(sp("%f", vec![float(-0.0)]), "0.000000");
    assert_eq!(sp("%.10f", vec![float(1e-10)]), "0.0000000001");
    assert_eq!(sp("%.3f", vec![float(0.0005)]), "0.001");
    assert_eq!(
        sp_err("%.", vec![float(1.5)]),
        value_error("Missing format specifier at end of string")
    );
}

#[test]
fn floats_precision_dot_without_digits() {
    // "%.f": a '.' without digits means precision 0.
    assert_eq!(sp("%.f", vec![float(1.5)]), "2");
    assert_eq!(sp("%.e", vec![float(1.5)]), "2e+0");
    assert_eq!(sp("%5.s|", vec![st("abc")]), "  abc|");
}

#[test]
fn precision_truncated_to_53() {
    let mut t = TestVm::new();
    let v = call(&mut t, "sprintf", "%.988f", vec![float(1.23456789e10)]);
    assert_val(
        &v,
        &Value::str("12345678900.00000000000000000000000000000000000000000000000000000"),
    );
    assert_eq!(
        t.output(),
        "\nNotice: sprintf(): Requested precision of 988 digits was truncated to PHP maximum \
         of 53 digits in /t.php on line 3\n"
    );
    let mut t = TestVm::new();
    let v = call(&mut t, "sprintf", "%.60e", vec![float(0.1)]);
    assert_val(
        &v,
        &Value::str("1.00000000000000005551115123125782702118158340454101562e-1"),
    );
    assert!(t.output().contains("Requested precision of 60 digits"));
    assert_eq!(
        sp("%.53f", vec![float(0.1)]),
        "0.10000000000000000555111512312578270211815834045410156"
    );
}

#[test]
fn floats_exponential() {
    // Bug #22207 and #36392.
    assert_eq!(sp("%10.5e\n", vec![float(1.1)]), "1.10000e+0\n");
    assert_eq!(sp("%e", vec![float(1.123456789)]), "1.123457e+0");
    assert_eq!(sp("%.10e", vec![float(1.123456789)]), "1.1234567890e+0");
    assert_eq!(sp("%.0e", vec![float(1.123456789)]), "1e+0");
    assert_eq!(sp("%.1e", vec![float(1.123456789)]), "1.1e+0");
    assert_eq!(sp("%5.1e", vec![float(1.123456789)]), "1.1e+0");
    // general_functions/001.phpt
    assert_eq!(sp("%e", vec![float(1.234E-18)]), "1.234000e-18");
    assert_eq!(sp("%e", vec![float(1.234E+18)]), "1.234000e+18");
    assert_eq!(sp("%e", vec![float(9843243.12)]), "9.843243e+6");
    assert_eq!(sp("%e", vec![float(-9843243.12)]), "-9.843243e+6");
    assert_eq!(sp("%.3e", vec![int(362525200)]), "3.625e+8");
    assert_eq!(sp("%.3e", vec![float(68.75 + 54.35)]), "1.231e+2");
    assert_eq!(sp("%E", vec![int(1234)]), "1.234000E+3");
    assert_eq!(sp("%e", vec![float(1e300)]), "1.000000e+300");
    assert_eq!(sp("%e", vec![float(5e-324)]), "4.940656e-324");
    assert_eq!(sp("%+e", vec![float(1.0)]), "+1.000000e+0");
    assert_eq!(sp("%+012e", vec![float(1.0)]), "+1.000000e+0");
    assert_eq!(sp("%+013e", vec![float(1.0)]), "+01.000000e+0");
    assert_eq!(sp("%-013e|", vec![float(-1.0)]), "-1.000000e+00|");
}

#[test]
fn vprintf_scientific_variations() {
    // vprintf_variation17.phpt
    let mut t = TestVm::new();
    let cases: Vec<(&str, Vec<Value>, &str)> = vec![
        (
            "%e %+e %-e",
            vec![int(0), float(1e0), st("10e2")],
            "0.000000e+0 +1.000000e+0 1.000000e+3",
        ),
        (
            "%le %4e %-4e",
            vec![float(2.2e2), float(1000e-2), float(1000e7)],
            "2.200000e+2 1.000000e+1 1.000000e+10",
        ),
        (
            "%10.4e %-10.4e %.4e",
            vec![float(-22e12), float(10e20), float(1.2e2)],
            "-2.2000e+13 1.0000e+21 1.2000e+2",
        ),
        (
            "%'#20e %'20e %'$20e %'_20e",
            vec![float(1e1), float(1e2), float(-1e3), st("1e2_")],
            "#########1.000000e+1 1.000000e+2 $$$$$$$$-1.000000e+3 _________1.000000e+2",
        ),
        (
            "%3$e %4$e %1$e %2$e",
            vec![float(3e3), float(4e3), float(1e3), float(2e3)],
            "1.000000e+3 2.000000e+3 3.000000e+3 4.000000e+3",
        ),
    ];
    for (format, values, want) in cases {
        let r = t
            .call("vprintf", vec![Value::str(format), list(values)])
            .unwrap();
        assert_eq!(t.output(), want, "{format}");
        assert_val(&r, &Value::Int(want.len() as i64));
    }
}

#[test]
fn floats_general() {
    let table: &[(&str, &str, &str)] = &[
        ("%g", "123456", "-123456"),
        ("%.0g", "1.0e+5", "-1.0e+5"),
        ("%+g", "+123456", "-123456"),
        ("%-g", "123456", "-123456"),
        ("%-1.2g", "1.2e+5", "-1.2e+5"),
        ("%+1.2g", "+1.2e+5", "-1.2e+5"),
        ("%G", "123456", "-123456"),
        ("%.0G", "1.0E+5", "-1.0E+5"),
        ("%+G", "+123456", "-123456"),
        ("%-1.2G", "1.2E+5", "-1.2E+5"),
        ("%+1.2G", "+1.2E+5", "-1.2E+5"),
    ];
    for (format, pos, neg) in table {
        assert_eq!(sp(format, vec![int(123456)]), *pos, "{format}");
        assert_eq!(sp(format, vec![int(-123456)]), *neg, "{format}");
    }
    // printf_h_H.phpt (the "C" locale makes %g and %h identical).
    assert_eq!(
        sp("%g %G %h %H", vec![float(1.25); 4]),
        "1.25 1.25 1.25 1.25"
    );
    assert_eq!(
        sp("%g %G %h %H", vec![float(0.00000125); 4]),
        "1.25e-6 1.25E-6 1.25e-6 1.25E-6"
    );
    assert_eq!(sp("%g", vec![float(0.0001)]), "0.0001");
    assert_eq!(sp("%g", vec![float(0.00001)]), "1.0e-5");
    assert_eq!(sp("%g", vec![int(1234567)]), "1.23457e+6");
    assert_eq!(sp("%g", vec![int(100000)]), "100000");
    assert_eq!(sp("%g", vec![int(0)]), "0");
    assert_eq!(sp("%g", vec![float(-0.0)]), "-0");
    assert_eq!(
        sp("%010g|%-10g|", vec![float(-1.5), float(1.5)]),
        "-0000001.5|1.5       |"
    );
}

#[test]
fn star_width_and_precision() {
    // sprintf_star.phpt
    let f = float(1.234_567_890_123_456_7);
    let fx = float(1.234_567_890_123_456_9e100);
    assert_eq!(sp("%.*f", vec![int(10), f.clone()]), "1.2345678901");
    assert_eq!(sp("%.*G", vec![int(10), f.clone()]), "1.23456789");
    assert_eq!(
        sp("%.*g", vec![int(-1), fx.clone()]),
        "1.2345678901234569e+100"
    );
    assert_eq!(
        sp("%.*G", vec![int(-1), fx.clone()]),
        "1.2345678901234569E+100"
    );
    assert_eq!(
        sp("%.*h", vec![int(-1), fx.clone()]),
        "1.2345678901234569e+100"
    );
    assert_eq!(sp("%.*H", vec![int(-1), fx]), "1.2345678901234569E+100");
    assert_eq!(sp("%.*s", vec![int(3), st("foobar")]), "foo");
    assert_eq!(sp("%*f", vec![int(10), f.clone()]), "  1.234568");
    assert_eq!(sp("%*G", vec![int(10), f.clone()]), "   1.23457");
    assert_eq!(sp("%*s", vec![int(10), st("foobar")]), "    foobar");
    assert_eq!(sp("%*.*f", vec![int(10), int(3), f.clone()]), "     1.235");
    assert_eq!(sp("%*.*G", vec![int(10), int(3), f.clone()]), "      1.23");
    assert_eq!(
        sp("%*.*s", vec![int(10), int(3), st("foobar")]),
        "       foo"
    );
    assert_eq!(sp("%1$.*2$f", vec![f.clone(), int(10)]), "1.2345678901");
    assert_eq!(sp("%.*2$f", vec![f.clone(), int(10)]), "1.2345678901");
    assert_eq!(sp("%2$.*f", vec![int(10), f.clone()]), "1.2345678901");
    assert_eq!(sp("%1$*2$f", vec![f.clone(), int(10)]), "  1.234568");
    assert_eq!(sp("%*2$f", vec![f.clone(), int(10)]), "  1.234568");
    assert_eq!(sp("%2$*f", vec![int(10), f.clone()]), "  1.234568");
    assert_eq!(
        sp("%1$*2$.*3$f", vec![f.clone(), int(10), int(3)]),
        "     1.235"
    );
    assert_eq!(
        sp("%*2$.*3$f", vec![f.clone(), int(10), int(3)]),
        "     1.235"
    );
    assert_eq!(
        sp("%3$*.*f", vec![int(10), int(3), f.clone()]),
        "     1.235"
    );

    // A by-reference array element is dereferenced for `*`.
    let r = Value::Ref(PhpRef::new(int(4)));
    let mut t = TestVm::new();
    let v = t
        .call("vsprintf", vec![Value::str("%*d"), list(vec![r, int(7)])])
        .unwrap();
    assert_val(&v, &Value::str("   7"));

    assert_eq!(
        sp_err("%.*G", vec![st("foo"), float(1.5)]),
        value_error("Precision must be an integer")
    );
    assert_eq!(
        sp_err("%.*G", vec![int(-100), float(1.5)]),
        value_error("Precision must be between -1 and 2147483647")
    );
    assert_eq!(
        sp_err("%.*s", vec![int(-1), st("Foo")]),
        value_error("Precision -1 is only supported for %g, %G, %h and %H")
    );
    assert_eq!(
        sp_err("%*G", vec![int(-1), f.clone()]),
        value_error("Width must be between 0 and 2147483647")
    );
    assert_eq!(
        sp_err("%*d", vec![float(5.0), int(1)]),
        value_error("Width must be an integer")
    );
    assert_eq!(
        sp_err("%*d", vec![int(2147483648), int(1)]),
        value_error("Width must be between 0 and 2147483647")
    );
    assert_eq!(
        sp_err("%9999999999999999999999.f", vec![f.clone()]),
        value_error("Width must be between 0 and 2147483647")
    );
    assert_eq!(
        sp_err("%2147483647d", vec![f.clone()]),
        value_error("Width must be between 0 and 2147483647")
    );
    assert_eq!(
        sp_err("%.9999999999999999999999f", vec![f]),
        value_error("Precision must be between 0 and 2147483647")
    );
}

#[test]
fn positional_arguments() {
    // Bug #26878.
    assert_eq!(
        sp("Int: %1$d and as string: %1$s", vec![st("some string")]),
        "Int: 0 and as string: some string"
    );
    let num = float(100.1);
    let loc = st("world");
    assert_eq!(
        sp(
            "There are %d monkeys in the %s",
            vec![num.clone(), loc.clone()]
        ),
        "There are 100 monkeys in the world"
    );
    assert_eq!(
        sp("The %s contains %d monkeys", vec![num.clone(), loc.clone()]),
        "The 100.1 contains 0 monkeys"
    );
    assert_eq!(
        sp(
            "The %2$s contains %1$d monkeys",
            vec![num.clone(), loc.clone()]
        ),
        "The world contains 100 monkeys"
    );
    // Positional and sequential arguments share no counter.
    assert_eq!(sp("%2$s %s %s", vec![st("a"), st("b")]), "b a b");
    // Positional args may carry modifiers.
    assert_eq!(sp("%1$'*8.3f|%1$-8d|", vec![num]), "*100.100|100     |");
}

#[test]
fn argument_errors() {
    // sprintf_error.phpt
    assert_eq!(
        sp_err("%s", vec![]),
        count_error("2 arguments are required, 1 given")
    );
    assert_eq!(
        sp_err("%s%s", vec![st("one")]),
        count_error("3 arguments are required, 2 given")
    );
    assert_eq!(
        sp_err("%s%s%s", vec![st("one"), st("two")]),
        count_error("4 arguments are required, 3 given")
    );
    assert_eq!(
        sp_err("%s%s", vec![]),
        count_error("3 arguments are required, 1 given")
    );
    assert_eq!(
        sp_err("%s%s%s", vec![]),
        count_error("4 arguments are required, 1 given")
    );
    assert_eq!(
        sp_err("%100$d %d", vec![]),
        count_error("101 arguments are required, 1 given")
    );
    assert_eq!(
        sp_err("foo %", vec![int(42)]),
        value_error("Missing format specifier at end of string")
    );
    // Without an argument for it, a trailing '%' is a missing argument.
    assert_eq!(
        sp_err("foo %", vec![]),
        count_error("2 arguments are required, 1 given")
    );
    // printf_64bit.phpt
    assert_eq!(
        sp_err("%d dingy%sflem%dwombat %s", vec![int(5), st("voudras")]),
        count_error("5 arguments are required, 3 given")
    );
    // The format is parsed to the end before missing arguments are reported
    // (a specifier without argument is not validated, though).
    assert_eq!(
        sp_err("%d %'", vec![]),
        value_error("Missing padding character")
    );
    assert_eq!(
        sp_err("%d %z", vec![]),
        count_error("3 arguments are required, 1 given")
    );
    assert_eq!(
        sp_err("%*d %d", vec![]),
        count_error("3 arguments are required, 1 given")
    );
    // Bug #69751.
    assert_eq!(
        sp_err("%$s, %2$s %1$s", vec![st("a"), st("b")]),
        value_error("Argument number specifier must not be empty")
    );
    assert_eq!(
        sp_err("%3$s, %2$s %1$s", vec![st("a"), st("b")]),
        count_error("4 arguments are required, 3 given")
    );
    assert_eq!(
        sp_err("%2147483648$s, %2$s %1$s", vec![st("a"), st("b")]),
        value_error("Argument number specifier must be greater than zero and less than 2147483647")
    );
    assert_eq!(
        sp_err("%0$s", vec![st("a")]),
        value_error("Argument number specifier must be greater than zero and less than 2147483647")
    );
    // Bug #67249.
    assert_eq!(
        sp_err("%'", vec![st("foo")]),
        value_error("Missing padding character")
    );
    assert_eq!(
        sp_err("%030.-15s", vec![st("x")]),
        value_error("Unknown format specifier \"-\"")
    );
    assert_eq!(
        sp_err("%y", vec![st("x")]),
        value_error("Unknown format specifier \"y\"")
    );
    let mut t = TestVm::new();
    let (class, _) = thrown(t.call("sprintf", vec![]));
    assert_eq!(class, "ArgumentCountError");
}

#[test]
fn unknown_specifier_keeps_raw_bytes() {
    let mut t = TestVm::new();
    let r = t.call("sprintf", vec![Value::bytes(b"%\0x"), int(1)]);
    assert_eq!(thrown(r), value_error("Unknown format specifier \"\0\""));
    let mut t = TestVm::new();
    let r = t.call("sprintf", vec![Value::bytes(b"%\xff"), int(1)]);
    match r {
        Err(php_runtime::Flow::Exception(e)) => {
            let msg = e.get_prop(b"message").expect("message");
            assert_val(&msg, &Value::bytes(b"Unknown format specifier \"\xff\""));
        }
        other => panic!("expected exception, got {other:?}"),
    }
}

#[test]
fn nan_and_infinity() {
    // The requested width is ignored.
    assert_eq!(sp("%f", vec![float(f64::NAN)]), "NaN");
    assert_eq!(sp("%10.2e|", vec![float(f64::NAN)]), "NaN|");
    assert_eq!(sp("%g", vec![float(f64::INFINITY)]), "INF");
    assert_eq!(sp("%10F|", vec![float(f64::NEG_INFINITY)]), "-INF|");
    assert_eq!(sp("%010f", vec![float(f64::NEG_INFINITY)]), "-INF");
    assert_eq!(sp("%+f", vec![float(f64::INFINITY)]), "INF");
    // php_sprintf_appendstring replaces the first byte with the sign.
    assert_eq!(sp("%+05f", vec![float(f64::NAN)]), "+aN");
    assert_eq!(sp("%+05f", vec![float(f64::INFINITY)]), "+NF");
}

#[test]
fn field_width_too_long_is_fatal() {
    let mut t = TestVm::new();
    let r = call_raw(&mut t, "a%2147483646s", vec![st("")]);
    assert!(r.is_err());
    assert!(
        t.output()
            .contains("Fatal error: Field width 2147483646 is too long")
    );
}

fn call_raw(t: &mut TestVm, format: &str, values: Vec<Value>) -> php_runtime::PResult<Value> {
    let mut args = vec![Value::str(format)];
    args.extend(values);
    t.call("sprintf", args)
}

#[test]
fn vsprintf_family() {
    let mut t = TestVm::new();
    let v = t
        .call(
            "vsprintf",
            vec![
                Value::str("%x %X %x"),
                list(vec![int(11), int(132), int(177)]),
            ],
        )
        .unwrap();
    assert_val(&v, &Value::str("b 84 b1"));
    let v = t
        .call("vsprintf", vec![Value::str("format"), list(vec![])])
        .unwrap();
    assert_val(&v, &Value::str("format"));
    // Missing array items are a ValueError.
    let r = t.call("vsprintf", vec![Value::str("%d %d"), list(vec![int(1)])]);
    assert_eq!(
        thrown(r),
        value_error("The arguments array must contain 2 items, 1 given")
    );
    let r = t.call("vprintf", vec![Value::str("%3$s"), list(vec![])]);
    assert_eq!(
        thrown(r),
        value_error("The arguments array must contain 3 items, 0 given")
    );
    // String keys are ignored: values are taken in order.
    let mut a = PhpArray::new();
    a.insert_str("x", st("one"));
    a.insert_str("y", st("two"));
    let v = t
        .call("vsprintf", vec![Value::str("%2$s %1$s"), Value::array(a)])
        .unwrap();
    assert_val(&v, &Value::str("two one"));
    // Wrong types are rejected by the signature.
    let r = t.call("vsprintf", vec![Value::str("Foo: %s"), st("not available")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".to_string(),
            "vsprintf(): Argument #2 ($values) must be of type array, string given".to_string()
        )
    );
}

#[test]
fn printf_writes_and_returns_length() {
    let mut t = TestVm::new();
    let v = call(&mut t, "printf", "%d %s\n", vec![int(5), st("x")]);
    assert_val(&v, &Value::Int(4));
    assert_eq!(t.output(), "5 x\n");
    // printf(3): the format is coerced to a string.
    let v = t.call("printf", vec![int(3)]).unwrap();
    assert_val(&v, &Value::Int(1));
    assert_eq!(t.output(), "3");
    let v = t
        .call(
            "vprintf",
            vec![Value::str("%05.1f"), list(vec![float(2.25)])],
        )
        .unwrap();
    assert_val(&v, &Value::Int(5));
    assert_eq!(t.output(), "002.2");
    // Nothing is printed when formatting fails.
    let r = call_raw2(&mut t, "printf", "abc %d");
    assert_eq!(thrown(r), count_error("2 arguments are required, 1 given"));
    assert_eq!(t.output(), "");
}

fn call_raw2(t: &mut TestVm, name: &str, format: &str) -> php_runtime::PResult<Value> {
    t.call(name, vec![Value::str(format)])
}

#[test]
fn fprintf_argument_checks() {
    let mut t = TestVm::new();
    let (class, msg) = thrown(t.call("fprintf", vec![]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "fprintf() expects at least 2 arguments, 0 given");
    let (_, msg) = thrown(t.call("fprintf", vec![int(3)]));
    assert_eq!(msg, "fprintf() expects at least 2 arguments, 1 given");
    // vfprintf_error4.phpt: the stream is checked before the format.
    let r = t.call("vfprintf", vec![st("foo"), st("%z"), list(vec![st("baz")])]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".to_string(),
            "vfprintf(): Argument #1 ($stream) must be of type resource, string given".to_string()
        )
    );
    let r = t.call("fprintf", vec![Value::Null, st("%d")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".to_string(),
            "fprintf(): Argument #1 ($stream) must be of type resource, null given".to_string()
        )
    );
    let res = php_runtime::ResRef::new("stream", Box::new(()));
    res.close();
    let r = t.call("fprintf", vec![Value::Resource(res), st("%d")]);
    assert_eq!(
        thrown(r),
        (
            "TypeError".to_string(),
            "fprintf(): supplied resource is not a valid stream resource".to_string()
        )
    );
    // vfprintf_error1.phpt
    let (class, msg) = thrown(t.call(
        "vfprintf",
        vec![
            st("x"),
            st("string_val"),
            list(vec![int(1), int(2)]),
            int(10),
        ],
    ));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(msg, "vfprintf() expects exactly 3 arguments, 4 given");
}

#[test]
fn php_formatted_print_entry_point() {
    let mut t = TestVm::new();
    let out =
        super::php_formatted_print(&mut t.vm, &PhpStr::from("%s-%03d"), &[st("a"), int(7)], 1)
            .unwrap();
    assert_eq!(out, b"a-007");
    let r = super::php_formatted_print(&mut t.vm, &PhpStr::from("%s %s"), &[st("a")], 2);
    assert_eq!(
        thrown(r.map(Value::from)),
        count_error("4 arguments are required, 3 given")
    );
}

#[test]
fn conv_fp_helper() {
    assert_eq!(
        php_conv_fp(b'F', 1.5, false, 2, b','),
        (b"1,50".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'F', -1.5, false, 0, b'.'),
        (b"2".to_vec(), true)
    );
    assert_eq!(
        php_conv_fp(b'F', 3.0, true, 0, b'.'),
        (b"3.".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'F', 0.0, true, 0, b'.'),
        (b"0".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'e', 0.0, false, 1, b'.'),
        (b"0.0e+0".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'E', -15.0, false, 1, b'.'),
        (b"1.5E+1".to_vec(), true)
    );
    assert_eq!(
        php_conv_fp(b'e', 2.0, true, 0, b'.'),
        (b"2.e+0".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'e', 1.5e-7, false, 3, b'.'),
        (b"1.500e-7".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'F', f64::INFINITY, false, 3, b'.'),
        (b"INF".to_vec(), false)
    );
    assert_eq!(
        php_conv_fp(b'F', f64::NAN, false, 3, b'.'),
        (b"NAN".to_vec(), false)
    );
}

#[test]
fn argument_type_conversions() {
    // sprintf_variation54.phpt
    let one = list(vec![int(1)]);
    let empty = list(vec![]);
    let res = Value::Resource(php_runtime::ResRef::new("stream", Box::new(())));
    let id = match &res {
        Value::Resource(r) => r.id(),
        _ => unreachable!(),
    };
    for spec in ["d", "u", "x"] {
        let format = format!("%{spec}");
        assert_eq!(sp(&format, vec![empty.clone()]), "0");
        assert_eq!(sp(&format, vec![one.clone()]), "1");
        assert_eq!(sp(&format, vec![float(3.5)]), "3");
        assert_eq!(sp(&format, vec![st("foo")]), "0");
        assert_eq!(sp(&format, vec![Value::Bool(true)]), "1");
    }
    assert_eq!(sp("%d", vec![res.clone()]), id.to_string());
    assert_eq!(sp("%f", vec![res.clone()]), format!("{id}.000000"));
    assert_eq!(sp("%s", vec![res]), format!("Resource id #{id}"));
    assert_eq!(sp("%f", vec![one]), "1.000000");
    assert_eq!(sp("%f", vec![empty]), "0.000000");
    assert_eq!(sp("%f", vec![Value::Null]), "0.000000");

    let mut t = TestVm::new();
    let class = t.vm.find_class("stdClass").expect("stdClass");
    let obj = Value::Object(t.vm.instantiate(&class).unwrap());
    let v = call(&mut t, "sprintf", "%d", vec![obj.clone()]);
    assert_val(&v, &Value::str("1"));
    assert_eq!(
        t.output(),
        "\nWarning: Object of class stdClass could not be converted to int in /t.php on line 3\n"
    );
    let v = call(&mut t, "sprintf", "%f", vec![obj.clone()]);
    assert_val(&v, &Value::str("1.000000"));
    assert!(t.output().contains("could not be converted to float"));
    let r = t.call("sprintf", vec![Value::str("%s"), obj]);
    assert_eq!(
        thrown(r),
        (
            "Error".to_string(),
            "Object of class stdClass could not be converted to string".to_string()
        )
    );
}

#[test]
fn float_to_int_conversion_warning() {
    // bug47842.phpt: %u of a float beyond the int range.
    let mut t = TestVm::new();
    let v = call(&mut t, "sprintf", "%u", vec![float(18446744073709551615.0)]);
    assert_val(&v, &Value::str("0"));
    assert_eq!(
        t.output(),
        "\nWarning: The float 1.8446744073709552E+19 is not representable as an int, cast \
         occurred in /t.php on line 3\n"
    );
}
