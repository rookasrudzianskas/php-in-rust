use super::*;
use crate::testing::{TestVm, thrown};
use php_runtime::PhpRef;

fn warn(msg: &str) -> String {
    format!("\nWarning: {msg} in /t.php on line 3\n")
}

const ILLEGAL: &str = "\",\", \";\", \" \", \"\\t\", \"\\r\", \"\\n\", \"\\013\", or \"\\014\"";

#[test]
fn header_before_output_is_silent_and_discarded() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("headers_sent", vec![]).unwrap(),
        Value::Bool(false)
    ));
    let r = t.call("header", vec![Value::str("X-Test: 1")]).unwrap();
    assert!(matches!(r, Value::Null));
    assert_eq!(t.output(), "");
    // the CLI handler keeps no headers
    assert_eq!(
        t.call("headers_list", vec![])
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // empty header lines fail silently
    t.call("header", vec![Value::str("")]).unwrap();
    assert_eq!(t.output(), "");
}

#[test]
fn head_phpt_after_output() {
    let mut t = TestVm::new();
    t.call(
        "header",
        vec![Value::str("HTTP 1.0"), Value::Bool(true), Value::Int(200)],
    )
    .unwrap();
    t.vm.echo(b"NULL\n").unwrap();
    let _ = t.output();
    assert!(matches!(
        t.call("headers_sent", vec![]).unwrap(),
        Value::Bool(true)
    ));
    let r = t.call("header", vec![Value::str("")]).unwrap();
    assert!(matches!(r, Value::Null));
    assert_eq!(
        t.output(),
        warn(
            "Cannot modify header information - headers already sent by (output started at /t.php:3)"
        )
    );
    assert_eq!(
        t.call("headers_list", vec![])
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn headers_sent_by_ref_arguments() {
    // headers_sent_by_ref_args.phpt
    let mut t = TestVm::new();
    let (file, line) = (PhpRef::new(Value::Null), PhpRef::new(Value::Null));
    let r =
        t.vm.call_native_named(
            "headers_sent",
            vec![],
            vec![("line", Value::Ref(line.clone()))],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(line.get().as_int(), Some(0));
    let r =
        t.vm.call_native_named(
            "headers_sent",
            vec![],
            vec![("filename", Value::Ref(file.clone()))],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(file.get().as_str().unwrap(), "");

    t.vm.echo(b"x").unwrap();
    let (file, line) = (PhpRef::new(Value::Null), PhpRef::new(Value::Null));
    t.call(
        "headers_sent",
        vec![Value::Ref(file.clone()), Value::Ref(line.clone())],
    )
    .unwrap();
    assert_eq!(file.get().as_str().unwrap(), "/t.php");
    assert_eq!(line.get().as_int(), Some(3));
}

#[test]
fn http_response_code_phpt() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("http_response_code", vec![]).unwrap(),
        Value::Bool(false)
    ));
    assert!(matches!(
        t.call("http_response_code", vec![Value::Int(201)]).unwrap(),
        Value::Bool(true)
    ));
    assert_eq!(
        t.call("http_response_code", vec![]).unwrap().as_int(),
        Some(201)
    );
    assert_eq!(
        t.call("http_response_code", vec![Value::Int(202)])
            .unwrap()
            .as_int(),
        Some(201)
    );
    t.vm.echo(b"Now we've sent the headers\n").unwrap();
    let _ = t.output();
    let r = t.call("http_response_code", vec![Value::Int(500)]).unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert_eq!(
        t.output(),
        warn(
            "http_response_code(): Cannot set response code - headers already sent (output started at /t.php:3)"
        )
    );
}

fn code_after(header: &str, replace: bool, code: i64) -> Option<i64> {
    let mut t = TestVm::new();
    t.call(
        "header",
        vec![Value::str(header), Value::Bool(replace), Value::Int(code)],
    )
    .unwrap();
    t.call("http_response_code", vec![]).unwrap().as_int()
}

#[test]
fn special_headers_change_the_status() {
    assert_eq!(
        code_after("Location: http://example.com/", true, 0),
        Some(302)
    );
    assert_eq!(code_after("Location: /x", true, 307), Some(307));
    assert_eq!(code_after("WWW-Authenticate: Basic", true, 0), Some(401));
    assert_eq!(code_after("HTTP/1.1 404 Not Found", true, 0), Some(404));
    assert_eq!(code_after("X-Plain: 1", true, 418), Some(418));

    // an explicit 3xx is kept
    let mut t = TestVm::new();
    t.call("http_response_code", vec![Value::Int(301)]).unwrap();
    t.call("header", vec![Value::str("Location: /x")]).unwrap();
    assert_eq!(
        t.call("http_response_code", vec![]).unwrap().as_int(),
        Some(301)
    );

    // http_response_code() after header('HTTP/...') has no effect
    let mut t = TestVm::new();
    t.call("header", vec![Value::str("HTTP/1.1 404 Not Found")])
        .unwrap();
    t.call("http_response_code", vec![Value::Int(500)]).unwrap();
    assert_eq!(
        t.output(),
        warn(
            "http_response_code(): Calling http_response_code() after header('HTTP/...') has no effect"
        )
    );
}

#[test]
fn header_validation_warnings() {
    // A displayed warning is output, which sends the headers: keep the
    // diagnostics silent and read them from error_get_last().
    let mut t = TestVm::new();
    t.vm.ini_set_system("display_errors", "0");
    let last = |t: &mut TestVm| {
        t.vm.errors
            .last_error
            .take()
            .map(|e| e.message.to_string())
            .unwrap_or_default()
    };
    t.call("header", vec![Value::str("X-A: a\nb")]).unwrap();
    assert_eq!(
        last(&mut t),
        "Header may not contain more than a single header, new line detected"
    );
    t.call("header", vec![Value::bytes(b"X-A: a\0b")]).unwrap();
    assert_eq!(last(&mut t), "Header may not contain NUL bytes");
    t.call("header_remove", vec![Value::str("X-A: b")]).unwrap();
    assert_eq!(last(&mut t), "Header to delete may not contain colon.");
    t.call("header_remove", vec![Value::str("X-A")]).unwrap();
    t.call("header_remove", vec![]).unwrap();
    assert_eq!(last(&mut t), "");
    // the same warnings are displayed when display_errors is on
    t.vm.ini_set_system("display_errors", "1");
    t.call("header", vec![Value::str("X-A: a\nb")]).unwrap();
    assert_eq!(
        t.output(),
        warn("Header may not contain more than a single header, new line detected")
    );
}

fn cookie(name: &str, value: Option<&str>) -> Cookie {
    Cookie {
        name: PhpStr::from(name),
        value: value.map(PhpStr::from),
        ..Cookie::default()
    }
}

fn line(t: &mut TestVm, c: &Cookie, enc: bool) -> String {
    String::from_utf8(php_setcookie(&mut t.vm, c, enc).unwrap().unwrap()).unwrap()
}

#[test]
fn set_cookie_lines() {
    // setcookie.phpt
    let mut t = TestVm::new();
    let deleted = "Set-Cookie: name=deleted; expires=Thu, 01 Jan 1970 00:00:01 GMT; Max-Age=0";
    assert_eq!(line(&mut t, &cookie("name", None), true), deleted);
    assert_eq!(line(&mut t, &cookie("name", Some("")), true), deleted);
    assert_eq!(
        line(&mut t, &cookie("name", Some("value")), true),
        "Set-Cookie: name=value"
    );
    assert_eq!(
        line(&mut t, &cookie("name", Some("space value")), true),
        "Set-Cookie: name=space%20value"
    );
    // raw cookies are not encoded
    assert_eq!(
        line(&mut t, &cookie("name", Some("a%2Fb")), false),
        "Set-Cookie: name=a%2Fb"
    );
    assert_eq!(
        line(&mut t, &cookie("name", Some("a/b")), true),
        "Set-Cookie: name=a%2Fb"
    );
    let mut c = cookie("name", Some("value"));
    c.expires = 1_000_000_000;
    assert_eq!(
        line(&mut t, &c, true),
        "Set-Cookie: name=value; expires=Sun, 09 Sep 2001 01:46:40 GMT; Max-Age=0"
    );
    let mut c = cookie("name", Some("value"));
    c.path = Some("/path/".into());
    c.domain = Some("domain.tld".into());
    c.secure = true;
    c.httponly = true;
    c.samesite = Some("Strict".into());
    c.partitioned = true;
    assert_eq!(
        line(&mut t, &c, true),
        "Set-Cookie: name=value; path=/path/; domain=domain.tld; secure; HttpOnly; SameSite=Strict; Partitioned"
    );
    // a future expiry reports the remaining lifetime
    let mut c = cookie("name", Some("value"));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    c.expires = now + 5;
    let l = line(&mut t, &c, true);
    assert!(l.ends_with("Max-Age=5") || l.ends_with("Max-Age=4"), "{l}");
}

#[test]
fn setcookie_function_and_options_array() {
    let mut t = TestVm::new();
    assert!(matches!(
        t.call("setcookie", vec![Value::str("a"), Value::str("b")])
            .unwrap(),
        Value::Bool(true)
    ));
    let mut opts = PhpArray::new();
    opts.insert_str("path", "/x");
    opts.insert_str("secure", true);
    opts.insert_str("partitioned", true);
    let r = t
        .call(
            "setcookie",
            vec![Value::str("a"), Value::str("b"), Value::array(opts)],
        )
        .unwrap();
    assert!(matches!(r, Value::Bool(true)));
    // after output the cookie cannot be set
    t.vm.echo(b"x").unwrap();
    let _ = t.output();
    let r = t
        .call("setrawcookie", vec![Value::str("a"), Value::str("b")])
        .unwrap();
    assert!(matches!(r, Value::Bool(false)));
    assert!(t.output().contains(
        "Cannot modify header information - headers already sent by (output started at /t.php:3)"
    ));
}

#[test]
fn setcookie_errors() {
    let mut t = TestVm::new();
    let (c, m) = thrown(t.call("setcookie", vec![Value::str("")]));
    assert_eq!(
        (c.as_str(), m.as_str()),
        (
            "ValueError",
            "setcookie(): Argument #1 ($name) cannot be empty"
        )
    );
    let (_, m) = thrown(t.call("setcookie", vec![Value::str("a=b")]));
    assert_eq!(
        m,
        format!("setcookie(): Argument #1 ($name) cannot contain \"=\", {ILLEGAL}")
    );
    let (_, m) = thrown(t.call("setrawcookie", vec![Value::str("a"), Value::str("b c")]));
    assert_eq!(
        m,
        format!("setrawcookie(): Argument #2 ($value) cannot contain {ILLEGAL}")
    );
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            Value::Int(0),
            Value::str("/a;b"),
        ],
    ));
    assert_eq!(
        m,
        format!("setcookie(): \"path\" option cannot contain {ILLEGAL}")
    );
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            Value::Int(0),
            Value::str(""),
            Value::str("d,e"),
        ],
    ));
    assert_eq!(
        m,
        format!("setcookie(): \"domain\" option cannot contain {ILLEGAL}")
    );
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            Value::Int(253_402_300_800),
        ],
    ));
    assert_eq!(
        m,
        "setcookie(): \"expires\" option cannot have a year greater than 9999"
    );

    let opts = |k: &str, v: Value| {
        let mut o = PhpArray::new();
        o.insert_str(k, v);
        Value::array(o)
    };
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            opts("partitioned", Value::Bool(true)),
        ],
    ));
    assert_eq!(
        m,
        "setcookie(): \"partitioned\" option cannot be used without \"secure\" option"
    );
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            opts("samesite", Value::str("bogus")),
        ],
    ));
    assert_eq!(
        m,
        "setcookie(): \"samesite\" option must be \"Strict\", \"Lax\", \"None\", or \"\""
    );
    let (_, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            opts("nope", Value::Int(1)),
        ],
    ));
    assert_eq!(m, "setcookie(): option \"nope\" is invalid");
    let numeric = Value::array(PhpArray::from_values(vec![Value::Int(1)]));
    let (_, m) = thrown(t.call("setcookie", vec![Value::str("a"), Value::str("b"), numeric]));
    assert_eq!(m, "setcookie(): option array cannot have numeric keys");
    let (c, m) = thrown(t.call(
        "setcookie",
        vec![
            Value::str("a"),
            Value::str("b"),
            Value::empty_array(),
            Value::str("/p"),
        ],
    ));
    assert_eq!(c, "ArgumentCountError");
    assert_eq!(
        m,
        "setcookie(): Expects exactly 3 arguments when argument #3 ($expires_or_options) is an array"
    );
}

#[test]
fn header_callback_runs_once_via_send_headers() {
    let mut t = TestVm::new();
    // a native function stands in for the callback
    t.call("header_register_callback", vec![Value::str("getmypid")])
        .unwrap();
    send_headers(&mut t.vm).unwrap();
    send_headers(&mut t.vm).unwrap();
    assert!(matches!(
        t.call("headers_sent", vec![]).unwrap(),
        Value::Bool(true)
    ));
    // registering after the headers went out stores nothing
    assert!(matches!(
        t.call("header_register_callback", vec![Value::str("getmypid")])
            .unwrap(),
        Value::Bool(true)
    ));
    assert!(t.vm.ext_data::<HeaderState>().callback.is_none());
    let _ = t.output();
}
