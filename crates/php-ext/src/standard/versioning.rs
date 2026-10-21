//! ext/standard/versioning.c

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// `strlen`: the C functions see a string only up to its first NUL byte.
fn c_str(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |n| &s[..n])
}

/// `s[i]`, reading the NUL terminator past the end.
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `php_canonicalize_version`: inserts dots between runs of digits and
/// non-digits and replaces `-`, `_`, `+` and other separators by dots.
pub fn php_canonicalize_version(version: &[u8]) -> Vec<u8> {
    let version = c_str(version);
    let Some((&first, rest)) = version.split_first() else {
        return Vec::new();
    };
    // `isdig`/`isndig` of versioning.c: digits and everything but digits and '.'
    let isdig = |c: u8| c.is_ascii_digit();
    let isndig = |c: u8| !c.is_ascii_digit() && c != b'.';

    let mut buf = Vec::with_capacity(version.len() * 2 + 1);
    buf.push(first);
    let mut lp = first;

    for &p in rest {
        let lq = *buf.last().unwrap();
        if matches!(p, b'-' | b'_' | b'+') {
            if lq != b'.' {
                buf.push(b'.');
            }
        } else if (isndig(lp) && isdig(p)) || (isdig(lp) && isndig(p)) {
            if lq != b'.' {
                buf.push(b'.');
            }
            buf.push(p);
        } else if !p.is_ascii_alphanumeric() {
            if lq != b'.' {
                buf.push(b'.');
            }
        } else {
            buf.push(p);
        }
        lp = p;
    }

    // Remove an empty last component.
    if buf.last() == Some(&b'.') {
        buf.pop();
    }
    buf
}

/// `special_forms` of `compare_special_version_forms`, in lookup order.
const SPECIAL_FORMS: [(&[u8], i32); 10] = [
    (b"dev", 0),
    (b"alpha", 1),
    (b"a", 1),
    (b"beta", 2),
    (b"b", 2),
    (b"RC", 3),
    (b"rc", 3),
    (b"#", 4),
    (b"pl", 5),
    (b"p", 5),
];

/// `compare_special_version_forms`
fn compare_special_version_forms(form1: &[u8], form2: &[u8]) -> i32 {
    let find = |form: &[u8]| {
        SPECIAL_FORMS
            .iter()
            .find(|(name, _)| form.starts_with(name))
            .map_or(-1, |&(_, order)| order)
    };
    (find(form1) - find(form2)).signum()
}

/// `strtol(p, NULL, 10)` for a string starting with a digit (saturating).
fn strtol_digits(s: &[u8]) -> i64 {
    let mut v: i64 = 0;
    for &c in s.iter().take_while(|c| c.is_ascii_digit()) {
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    v
}

/// `php_version_compare`: -1, 0 or 1.
pub fn php_version_compare(orig_ver1: &[u8], orig_ver2: &[u8]) -> i32 {
    let orig_ver1 = c_str(orig_ver1);
    let orig_ver2 = c_str(orig_ver2);

    if orig_ver1.is_empty() || orig_ver2.is_empty() {
        if orig_ver1.is_empty() && orig_ver2.is_empty() {
            return 0;
        }
        return if orig_ver1.is_empty() { -1 } else { 1 };
    }
    let ver1 = if orig_ver1[0] == b'#' {
        orig_ver1.to_vec()
    } else {
        php_canonicalize_version(orig_ver1)
    };
    let ver2 = if orig_ver2[0] == b'#' {
        orig_ver2.to_vec()
    } else {
        php_canonicalize_version(orig_ver2)
    };

    // `p*` is the start of the current component, `n*` the position of the
    // dot ending it (None once the last component was reached).
    let find_dot = |v: &[u8], p: usize| v[p..].iter().position(|&b| b == b'.').map(|d| p + d);
    let mut p1 = 0usize;
    let mut p2 = 0usize;
    let mut n1 = Some(0usize);
    let mut n2 = Some(0usize);
    let mut compare = 0;

    while at(&ver1, p1) != 0 && at(&ver2, p2) != 0 && n1.is_some() && n2.is_some() {
        n1 = find_dot(&ver1, p1);
        n2 = find_dot(&ver2, p2);
        let c1 = &ver1[p1..n1.unwrap_or(ver1.len())];
        let c2 = &ver2[p2..n2.unwrap_or(ver2.len())];
        let d1 = c1.first().is_some_and(u8::is_ascii_digit);
        let d2 = c2.first().is_some_and(u8::is_ascii_digit);
        if d1 && d2 {
            // compare element numerically
            compare = match strtol_digits(c1).cmp(&strtol_digits(c2)) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
        } else if !d1 && !d2 {
            // compare element names
            compare = compare_special_version_forms(c1, c2);
        } else if d1 {
            // mix of names and numbers
            compare = compare_special_version_forms(b"#N#", c2);
        } else {
            compare = compare_special_version_forms(c1, b"#N#");
        }
        if compare != 0 {
            break;
        }
        if let Some(n) = n1 {
            p1 = n + 1;
        }
        if let Some(n) = n2 {
            p2 = n + 1;
        }
    }
    if compare == 0 {
        if n1.is_some() {
            if at(&ver1, p1).is_ascii_digit() {
                compare = 1;
            } else {
                compare = php_version_compare(&ver1[p1.min(ver1.len())..], b"#N#");
            }
        } else if n2.is_some() {
            if at(&ver2, p2).is_ascii_digit() {
                compare = -1;
            } else {
                compare = php_version_compare(b"#N#", &ver2[p2.min(ver2.len())..]);
            }
        }
    }
    compare
}

/// Compares two "PHP-standardized" version number strings.
fn version_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let compare = php_version_compare(args.str(0), args.str(1));
    let Some(op) = args.str_opt(2) else {
        return Ok(Value::Int(i64::from(compare)));
    };
    let result = match &**op {
        b"<" | b"lt" => compare == -1,
        b"<=" | b"le" => compare != 1,
        b">" | b"gt" => compare == 1,
        b">=" | b"ge" => compare != -1,
        b"==" | b"=" | b"eq" => compare == 0,
        b"!=" | b"<>" | b"ne" => compare != 0,
        _ => {
            return Err(vm.argument_value_error(3, "must be a valid comparison operator"));
        }
    };
    Ok(Value::Bool(result))
}

pub static VERSION_COMPARE: NativeFunction = NativeFunction {
    sig: "version_compare(string $version1, string $version2, ?string $operator = null): int|bool",
    handler: version_compare,
};

pub fn register(r: &mut Registry) {
    r.function(&VERSION_COMPARE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn vc(t: &mut TestVm, a: &str, b: &str) -> i64 {
        match t
            .call("version_compare", vec![Value::str(a), Value::str(b)])
            .unwrap()
        {
            Value::Int(i) => i,
            v => panic!("{v:?}"),
        }
    }

    fn vc_op(t: &mut TestVm, a: &str, b: &str, op: &str) -> bool {
        match t
            .call(
                "version_compare",
                vec![Value::str(a), Value::str(b), Value::str(op)],
            )
            .unwrap()
        {
            Value::Bool(b) => b,
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn canonicalize() {
        let c = |s: &str| String::from_utf8(php_canonicalize_version(s.as_bytes())).unwrap();
        assert_eq!(c(""), "");
        assert_eq!(c("1.0-dev"), "1.0.dev");
        assert_eq!(c("1.0a1"), "1.0.a.1");
        assert_eq!(c("1.0RC1"), "1.0.RC.1");
        assert_eq!(c("5.2.0rc1+build_5"), "5.2.0.rc.1.build.5");
        assert_eq!(c("1.2."), "1.2");
        assert_eq!(c("1.2.."), "1.2");
        assert_eq!(c("1--2"), "1.2");
        assert_eq!(c("."), "");
    }

    #[test]
    fn compare_basic() {
        // version_compare.phpt: TESTING COMPARE
        let mut t = TestVm::new();
        let cases: &[(&str, &str, i64)] = &[
            ("1", "2", -1),
            ("10", "2", 1),
            ("1.0", "1.1", -1),
            ("1.2", "1.0.1", 1),
            ("1.2.", "1.2.", 0),
            ("1.2..", "1.2..", 0),
        ];
        for &(a, b, e) in cases {
            assert_eq!(vc(&mut t, a, b), e, "{a} vs {b}");
        }
    }

    /// The special forms of version_compare.phpt with their expected rank.
    const FORMS: [(&str, i32); 7] = [
        ("-dev", 0),
        ("a1", 1),
        ("b1", 2),
        ("RC1", 3),
        ("rc1", 3),
        ("", 4),
        ("pl1", 5),
    ];

    #[test]
    fn compare_special_forms() {
        let mut t = TestVm::new();
        for (f1, r1) in FORMS {
            for (f2, r2) in FORMS {
                let v1 = format!("1.0{f1}");
                let v2 = format!("1.0{f2}");
                let expected = i64::from((r1 - r2).signum());
                assert_eq!(vc(&mut t, &v1, &v2), expected, "{v1} vs {v2}");
                assert_eq!(
                    php_version_compare(v1.as_bytes(), v2.as_bytes()),
                    expected as i32
                );
            }
        }
    }

    #[test]
    fn operators() {
        // version_compare.phpt: TESTING OPERATORS
        let mut t = TestVm::new();
        let ops = [
            "lt", "<", "le", "<=", "gt", ">", "ge", ">=", "eq", "=", "==", "ne", "<>", "!=",
        ];
        for (f1, r1) in FORMS {
            for (f2, r2) in FORMS {
                let v1 = format!("1.0{f1}");
                let v2 = format!("1.0{f2}");
                let cmp = (r1 - r2).signum();
                for op in ops {
                    let expected = match op {
                        "lt" | "<" => cmp < 0,
                        "le" | "<=" => cmp <= 0,
                        "gt" | ">" => cmp > 0,
                        "ge" | ">=" => cmp >= 0,
                        "eq" | "=" | "==" => cmp == 0,
                        _ => cmp != 0,
                    };
                    assert_eq!(vc_op(&mut t, &v1, &v2, op), expected, "{v1} {op} {v2}");
                }
            }
        }
    }

    #[test]
    fn invalid_operator() {
        // version_compare_invalid_operator.phpt, version_compare_op_abbrev.phpt
        let mut t = TestVm::new();
        let r = t.call(
            "version_compare",
            vec![Value::str("1.2"), Value::str("2.1"), Value::str("??")],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "version_compare(): Argument #3 ($operator) must be a valid comparison operator"
                    .to_string()
            )
        );
        for op in ["", "l", "g", "e", "!", "n", "LT", " <"] {
            let r = t.call(
                "version_compare",
                vec![Value::str("1"), Value::str("2"), Value::str(op)],
            );
            assert_eq!(thrown(r).0, "ValueError", "'{op}'");
        }
        // an explicit null operator behaves like no operator
        let r = t.call(
            "version_compare",
            vec![Value::str("1"), Value::str("2"), Value::Null],
        );
        assert!(matches!(r.unwrap(), Value::Int(-1)));
    }

    #[test]
    fn compare_edge_cases() {
        let mut t = TestVm::new();
        assert_eq!(vc(&mut t, "", ""), 0);
        assert_eq!(vc(&mut t, "1", ""), 1);
        assert_eq!(vc(&mut t, "", "1"), -1);
        assert_eq!(vc(&mut t, "5.2.0", "5.2"), 1);
        assert_eq!(vc(&mut t, "5.2", "5.2.0"), -1);
        assert_eq!(vc(&mut t, "5.2.0", "5.2.0-dev"), 1);
        assert_eq!(vc(&mut t, "1.0.0", "1.0.0pl1"), -1);
        assert_eq!(vc(&mut t, "1.0-alpha", "1.0a"), 0);
        assert_eq!(vc(&mut t, "1.0beta", "1.0b"), 0);
        assert_eq!(vc(&mut t, "1.0+foo", "1.0.foo"), 0);
        // numbers are compared numerically
        assert_eq!(vc(&mut t, "1.10", "1.9"), 1);
        assert_eq!(vc(&mut t, "1.01", "1.1"), 0);
        assert_eq!(
            vc(&mut t, "99999999999999999999", "99999999999999999998"),
            0
        );
        // '#'-prefixed strings are not canonicalized
        assert_eq!(vc(&mut t, "#a", "#a"), 0);
        // C: a trailing dot leaves an empty rest that compares below "#N#"
        assert_eq!(vc(&mut t, "#a.", "#a."), -1);
        // C strings: everything after a NUL byte is ignored
        assert_eq!(php_version_compare(b"1.0\0junk", b"1.0"), 0);
    }
}
