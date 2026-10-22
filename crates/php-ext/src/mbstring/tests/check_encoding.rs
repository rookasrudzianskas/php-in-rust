//! mb_check_encoding, ported from php-src's mb_check_encoding*.phpt.

use php_runtime::{ArrayKey, PhpRef};

use super::*;

const VALID: &str = "Japanese UTF-8 text. 日本語のUTF-8テキスト";
const INVALID: &[u8] =
    b"Japanese UTF-8 text. \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e\xFE\x01\x02\xe3\x81\xaeUTF-8";

/// `[1234, 12.34, TRUE, FALSE, NULL, ...]` followed by `extra`.
fn mixed(extra: Vec<(Option<&[u8]>, Value)>) -> PhpArray {
    let mut a = PhpArray::from_values(vec![
        Value::Int(1234),
        Value::Float(12.34),
        Value::Bool(true),
        Value::Bool(false),
        Value::Null,
    ]);
    for (key, v) in extra {
        match key {
            Some(k) => {
                a.insert(ArrayKey::from_bytes(k), v);
            }
            None => {
                a.append(v).unwrap();
            }
        }
    }
    a
}

fn check(t: &mut TestVm, v: Value) -> bool {
    match call(t, "mb_check_encoding", vec![v]) {
        Value::Bool(b) => b,
        other => panic!("expected bool, got {other:?}"),
    }
}

/// The array in a reference that contains that reference (`$a[] = &$a`).
fn circular(a: PhpArray) -> Value {
    let r = PhpRef::new(Value::Null);
    let mut a = a;
    a.append(Value::Ref(r.clone())).unwrap();
    r.set(Value::array(a));
    r.get()
}

#[test]
fn strings_and_arrays() {
    let mut t = TestVm::new();
    let t = &mut t;
    t.vm.ini_set_system("default_charset", "UTF-8");
    // mb_check_encoding.phpt
    let valid = || {
        mixed(vec![
            (None, s(VALID)),
            (Some(b"key"), s(VALID)),
            (Some(VALID.as_bytes()), s("val")),
        ])
    };
    let arr1 = || mixed(vec![(Some(b"key"), b(INVALID)), (Some(INVALID), s("val"))]);
    let arr2 = || mixed(vec![(Some(INVALID), s("val"))]);
    assert!(check(t, s(VALID)));
    assert!(check(t, Value::array(valid())));
    assert!(!check(t, b(INVALID)));
    assert!(!check(t, Value::array(arr1())));
    assert!(!check(t, Value::array(arr2())));
    assert_eq!(t.output(), "");

    // mb_check_encoding_array.phpt: the self-containing array is reported
    // once; the others fail on an invalid string before reaching it.
    assert!(!check(t, circular(valid())));
    assert!(
        t.output()
            .contains("Warning: mb_check_encoding(): Cannot not handle circular references")
    );
    assert!(!check(t, circular(arr1())));
    assert!(!check(t, circular(arr2())));
    assert_eq!(t.output(), "");
}

#[test]
fn invalid_encodings_and_no_argument() {
    let mut t = TestVm::new();
    let t = &mut t;
    // mb_check_encoding_invalid_encodings.phpt
    let arr = || {
        Value::array(mixed(vec![
            (None, s("Normal string")),
            (Some(b"key"), s("Normal string")),
            (Some(b"Normal string"), s("val")),
        ]))
    };
    for enc in ["BAD", "pass"] {
        let expected = format!(
            "ValueError: mb_check_encoding(): Argument #2 ($encoding) must be a valid encoding, \"{enc}\" given"
        );
        assert_eq!(
            err(t, "mb_check_encoding", vec![s("Normal string"), s(enc)]),
            expected
        );
        assert_eq!(err(t, "mb_check_encoding", vec![arr(), s(enc)]), expected);
    }
    // mb_check_encoding_without_arg.phpt
    assert_eq!(dump_str(t, "mb_check_encoding", vec![]), "bool(true)\n");
    assert!(t.output().contains(
        "Deprecated: mb_check_encoding(): Calling mb_check_encoding() without argument is deprecated"
    ));
}
