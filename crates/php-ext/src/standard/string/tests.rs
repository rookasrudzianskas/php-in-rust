//! Expectations ported from php-src's `ext/standard/tests/strings/*.phpt`.

use php_runtime::value::array::ArrayKey;
use php_runtime::{PhpArray, PhpRef, Value};

use crate::testing::{TestVm, thrown};

fn s(x: &str) -> Value {
    Value::str(x)
}

fn b(x: &[u8]) -> Value {
    Value::bytes(x)
}

fn bytes(v: Value) -> Vec<u8> {
    match v {
        Value::String(x) => x.to_vec(),
        v => panic!("not a string: {v:?}"),
    }
}

fn call(t: &mut TestVm, f: &str, a: Vec<Value>) -> Vec<u8> {
    bytes(t.call(f, a).unwrap())
}

fn list(v: &Value) -> Vec<Vec<u8>> {
    match v {
        Value::Array(a) => a.values().map(|v| bytes(v.clone())).collect(),
        v => panic!("not an array: {v:?}"),
    }
}

fn arr(items: &[&str]) -> Value {
    Value::array(items.iter().map(|x| Value::str(x)).collect::<PhpArray>())
}

fn int_or_false(v: Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(i),
        Value::Bool(false) => None,
        v => panic!("unexpected {v:?}"),
    }
}

#[test]
fn trim_phpt() {
    let mut t = TestVm::new();
    let ws = b(b" \0\t\nABC \0\t\n");
    assert_eq!(call(&mut t, "trim", vec![s("ABC")]), b"ABC");
    assert_eq!(call(&mut t, "trim", vec![ws.clone()]), b"ABC");
    assert_eq!(call(&mut t, "ltrim", vec![ws.clone()]), b"ABC \0\t\n");
    assert_eq!(call(&mut t, "rtrim", vec![ws.clone()]), b" \0\t\nABC");
    for f in ["trim", "ltrim", "rtrim"] {
        assert_eq!(
            call(&mut t, f, vec![ws.clone(), s("")]),
            b" \0\t\nABC \0\t\n"
        );
    }
    let hi = b(b"ABC\x50\xC1\x60\x90");
    assert_eq!(
        call(&mut t, "trim", vec![hi.clone(), b(b"\x51..\xC0")]),
        b"ABC\x50\xC1"
    );
    assert_eq!(
        call(&mut t, "trim", vec![hi.clone(), b(b"\x51..\xC1")]),
        b"ABC\x50"
    );
    assert_eq!(
        call(&mut t, "trim", vec![hi.clone(), b(b"\x50..\xC1")]),
        b"ABC"
    );
    assert_eq!(call(&mut t, "trim", vec![s("\x0cABC\x0c")]), b"ABC");
    assert_eq!(call(&mut t, "ltrim", vec![s("\x0cABC")]), b"ABC");
    assert_eq!(call(&mut t, "rtrim", vec![s("ABC\x0c")]), b"ABC");
}

#[test]
fn explode_phpt() {
    let mut t = TestVm::new();
    let e = |t: &mut TestVm, a: Vec<Value>| list(&t.call("explode", a).unwrap());
    assert_eq!(
        e(&mut t, vec![s("\x01"), b(b"a\x01b\0d\x01f\x011\x01d")]),
        vec![&b"a"[..], b"b\0d", b"f", b"1", b"d"]
    );
    assert_eq!(e(&mut t, vec![s("a"), s("")]), vec![&b""[..]]);
    assert_eq!(e(&mut t, vec![s("a"), s("a")]), vec![&b""[..], b""]);
    assert_eq!(e(&mut t, vec![s("abc"), s("acb")]), vec![&b"acb"[..]]);
    assert!(e(&mut t, vec![s("somestring"), s("otherstring"), (-1).into()]).is_empty());
    assert_eq!(e(&mut t, vec![s("a"), s("aaaaaa")]).len(), 7);
    let r = e(&mut t, vec![s("="), s(&"-=48=-".repeat(10))]);
    assert_eq!(r.len(), 21);
    assert_eq!(r[0], b"-");
    assert_eq!(r[1], b"48");
    assert_eq!(r[2], b"--");
    assert_eq!(
        e(
            &mut t,
            vec![s(":"), s("a lazy dog:jumps:over:"), (-1).into()]
        ),
        vec![&b"a lazy dog"[..], b"jumps", b"over"]
    );
    assert_eq!(
        e(
            &mut t,
            vec![s(":"), s("a lazy dog:jumps:over"), (-2).into()]
        ),
        vec![&b"a lazy dog"[..]]
    );
    assert!(
        e(
            &mut t,
            vec![s(":"), s("a lazy dog:jumps:over:"), (-4).into()]
        )
        .is_empty()
    );
    assert!(
        e(
            &mut t,
            vec![
                s(":"),
                s("a lazy dog:jumps:over:"),
                (-40000000000000i64).into()
            ]
        )
        .is_empty()
    );
    assert_eq!(
        e(
            &mut t,
            vec![s(":^:"), s("a lazy dog:^:jumps::over:^:"), (-1).into()]
        ),
        vec![&b"a lazy dog"[..], b"jumps::over"]
    );
    assert_eq!(
        e(
            &mut t,
            vec![s(":^:"), s("a lazy dog:^:jumps::over:^:"), (-2).into()]
        ),
        vec![&b"a lazy dog"[..]]
    );
}

#[test]
fn strrpos_negative_offset_phpt() {
    let mut t = TestVm::new();
    let p = |t: &mut TestVm, f: &str, h: &str, n: &str, o: i64| {
        int_or_false(t.call(f, vec![s(h), s(n), o.into()]).unwrap())
    };
    assert_eq!(p(&mut t, "strrpos", "haysthack", "ha", -9), Some(0));
    assert_eq!(p(&mut t, "strrpos", "haystack", "h", -8), Some(0));
    assert_eq!(p(&mut t, "strrpos", "haystack", "k", -1), Some(7));
    assert_eq!(p(&mut t, "strrpos", "haystack", "ka", -1), None);
    assert_eq!(p(&mut t, "strrpos", "haystack", "a", -3), Some(5));
    assert_eq!(p(&mut t, "strrpos", "haystack", "a", -4), Some(1));
    assert_eq!(
        thrown(t.call("strrpos", vec![s("haystack"), s("h"), (-9).into()])).1,
        "strrpos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
    );
    assert_eq!(p(&mut t, "strripos", "HAYSTHACk", "ha", -9), Some(0));
    assert_eq!(p(&mut t, "strripos", "HAYSTACK", "h", -8), Some(0));
    assert_eq!(p(&mut t, "strripos", "HAYSTACK", "k", -1), Some(7));
    assert_eq!(p(&mut t, "strripos", "HAYSTACK", "ka", -1), None);
    assert_eq!(p(&mut t, "strripos", "HAYSTACK", "a", -3), Some(5));
    assert_eq!(p(&mut t, "strripos", "HAYSTACK", "a", -4), Some(1));
    assert_eq!(
        thrown(t.call("strripos", vec![s("HAYSTACK"), s("h"), (-9).into()])).1,
        "strripos(): Argument #3 ($offset) must be contained in argument #1 ($haystack)"
    );
}

#[test]
fn substr_replace_array_unset_phpt() {
    let mut t = TestVm::new();
    // ['A', 'C', 'B'] with element 1 unset behaves like ['A', 'B'].
    let v = t
        .call(
            "substr_replace",
            vec![arr(&["1 string", "2 string"]), arr(&["A", "B"]), 0.into()],
        )
        .unwrap();
    assert_eq!(list(&v), vec![&b"A"[..], b"B"]);
}

#[test]
fn strtr_phpt() {
    let mut t = TestVm::new();
    let empty_key = |extra: bool| {
        let mut a = PhpArray::new();
        a.insert(ArrayKey::from(""), Value::str("bar"));
        if extra {
            a.insert(ArrayKey::from("x"), Value::str("y"));
        }
        Value::array(a)
    };
    for extra in [false, true] {
        assert_eq!(
            call(&mut t, "strtr", vec![s("foo"), empty_key(extra)]),
            b"foo"
        );
        assert_eq!(
            t.output(),
            "\nWarning: strtr(): Ignoring replacement of empty string in /t.php on line 3\n"
        );
    }
    // A reference element is read through.
    let foo = PhpRef::new(s("foo"));
    let mut a = PhpArray::new();
    a.insert(ArrayKey::from("bar"), Value::Ref(foo));
    assert_eq!(
        call(&mut t, "strtr", vec![s("foobar"), Value::array(a)]),
        b"foofoo"
    );
}

#[test]
fn addcslashes_phpt() {
    let mut t = TestVm::new();
    let string = b(b"goodyear12345NULL\0truefalse\x07\x0b\x0c\x08\n\r\t");
    let ac = |t: &mut TestVm, chars: &[u8]| call(t, "addcslashes", vec![string.clone(), b(chars)]);
    assert_eq!(
        ac(&mut t, b"2"),
        b"goodyear1\\2345NULL\0truefalse\x07\x0b\x0c\x08\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"a"),
        b"goodye\\ar12345NULL\0truef\\alse\x07\x0b\x0c\x08\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"\0"),
        b"goodyear12345NULL\\000truefalse\x07\x0b\x0c\x08\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"\n"),
        b"goodyear12345NULL\0truefalse\x07\x0b\x0c\x08\\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"\x07"),
        b"goodyear12345NULL\0truefalse\\a\x0b\x0c\x08\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"\x08"),
        b"goodyear12345NULL\0truefalse\x07\x0b\x0c\\b\n\r\t"
    );
    assert_eq!(
        ac(&mut t, b"\0..\x1f"),
        b"goodyear12345NULL\\000truefalse\\a\\v\\f\\b\\n\\r\\t"
    );
    assert_eq!(call(&mut t, "addcslashes", vec![s(""), s("")]), b"");
    assert_eq!(
        call(&mut t, "addcslashes", vec![s("kaboemkara!"), s("")]),
        b"kaboemkara!"
    );
    assert_eq!(
        call(&mut t, "addcslashes", vec![s("foobarbaz"), s("bar")]),
        b"foo\\b\\a\\r\\b\\az"
    );
    assert_eq!(
        call(&mut t, "addcslashes", vec![s("foo[ ]"), s("A..z")]),
        b"\\f\\o\\o\\[ \\]"
    );
    assert_eq!(
        call(
            &mut t,
            "addcslashes",
            vec![s("abcdefghijklmnopqrstuvwxyz"), s("a\x65..\x70z")]
        ),
        b"\\abcd\\e\\f\\g\\h\\i\\j\\k\\l\\m\\n\\o\\pqrstuvwxy\\z"
    );
    assert_eq!(
        call(&mut t, "addcslashes", vec![123.into(), 123.into()]),
        b"\\1\\2\\3"
    );
    assert_eq!(
        call(&mut t, "addcslashes", vec![0.into(), 0.into()]),
        b"\\0"
    );
    assert_eq!(call(&mut t, "addcslashes", vec![s("\0"), 0.into()]), b"\0");
    assert_eq!(
        call(
            &mut t,
            "addcslashes",
            vec![Value::Float(-1.234578), 3.into()]
        ),
        b"-1.2\\34578"
    );
    assert_eq!(call(&mut t, "addcslashes", vec![s(" "), s(" ")]), b"\\ ");
    assert_eq!(
        call(
            &mut t,
            "addcslashes",
            vec![b(b"string\0with\0NULL"), b(b"\0")]
        ),
        b"string\\000with\\000NULL"
    );
    assert_eq!(
        call(&mut t, "addcslashes", vec![s("zoo['.']"), s("z..A")]),
        b"\\zoo['\\.']"
    );
    assert_eq!(
        t.output(),
        "\nWarning: addcslashes(): Invalid '..'-range, '..'-range needs to be incrementing in /t.php on line 3\n"
    );
}

#[test]
fn stripcslashes_phpt() {
    let mut t = TestVm::new();
    assert_eq!(
        call(
            &mut t,
            "stripcslashes",
            vec![s("\\H\\e\\l\\l\\o \\W\\or\\l\\d")]
        ),
        b"Hello World"
    );
    assert_eq!(
        call(&mut t, "stripcslashes", vec![s("Hello World\\r\\n")]),
        b"Hello World\r\n"
    );
    assert_eq!(
        call(&mut t, "stripcslashes", vec![s("\\\\Hello World")]),
        b"\\Hello World"
    );
    assert_eq!(
        call(
            &mut t,
            "stripcslashes",
            vec![s("\\x48\\x65\\x6c\\x6c\\x6f \\x57\\x6f\\x72\\x6c\\x64")]
        ),
        b"Hello World"
    );
    assert_eq!(
        call(
            &mut t,
            "stripcslashes",
            vec![s("\\110\\145\\154\\154\\157 \\127\\157\\162\\154\\144")]
        ),
        b"Hello World"
    );
    for (esc, byte) in [
        ("\\a", 7u8),
        ("\\b", 8),
        ("\\f", 12),
        ("\\t", 9),
        ("\\v", 11),
    ] {
        assert_eq!(call(&mut t, "stripcslashes", vec![s(esc)]), vec![byte]);
    }
}

#[test]
fn str_pad_and_misc_defaults() {
    let mut t = TestVm::new();
    // Named-argument style defaults flow through the stub signature.
    assert_eq!(call(&mut t, "str_pad", vec![s("x"), 3.into()]), b"x  ");
    assert_eq!(
        call(&mut t, "ucwords", vec![s("hello   world")]),
        b"Hello   World"
    );
    assert_eq!(
        call(&mut t, "wordwrap", vec![s("a b c d"), 3.into()]),
        b"a b\nc d"
    );
    assert_eq!(call(&mut t, "nl2br", vec![s("a\nb")]), b"a<br />\nb");
    assert_eq!(
        call(&mut t, "chunk_split", vec![s("abcd"), 3.into()]),
        b"abc\r\nd\r\n"
    );
}

/// Every function of the string-functions package resolves in a fresh VM.
#[test]
fn package_functions_are_registered() {
    let t = TestVm::new();
    let names = "bin2hex hex2bin strspn strcspn nl_langinfo strcoll trim rtrim chop ltrim wordwrap \
        explode implode join strtok strtoupper strtolower str_increment str_decrement basename \
        dirname pathinfo stristr strstr strchr strpos stripos strrpos strripos strrchr str_contains \
        str_starts_with str_ends_with chunk_split substr substr_replace quotemeta ord chr ucfirst \
        lcfirst ucwords strtr strrev similar_text addcslashes addslashes stripcslashes stripslashes \
        str_replace str_ireplace hebrev nl2br strip_tags setlocale parse_str str_repeat count_chars \
        localeconv substr_count str_pad sscanf str_rot13 str_word_count str_split strpbrk \
        substr_compare utf8_encode utf8_decode htmlspecialchars htmlspecialchars_decode \
        html_entity_decode htmlentities get_html_translation_table base64_encode base64_decode \
        convert_uuencode convert_uudecode quoted_printable_encode quoted_printable_decode \
        version_compare soundex levenshtein metaphone parse_url urlencode urldecode rawurlencode \
        rawurldecode pack unpack sprintf printf vsprintf vprintf fprintf vfprintf";
    for name in names.split_whitespace() {
        assert!(
            t.vm.find_function(name).is_some(),
            "{name}() is not registered"
        );
    }
    for name in [
        "STR_PAD_LEFT",
        "ENT_QUOTES",
        "PHP_URL_HOST",
        "PATHINFO_ALL",
        "LC_ALL",
    ] {
        assert!(t.vm.constant(name).is_some(), "{name} is not defined");
    }
}
