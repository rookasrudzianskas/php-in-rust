//! Tests for ext/standard/streamsfuncs.c (expectations from ext/standard/tests/streams).

use crate::testing::{TestVm, thrown};
use php_runtime::{PhpArray, PhpRef, Value};

fn s(v: &str) -> Value {
    Value::str(v)
}

fn bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.to_vec(),
        o => panic!("expected string, got {o:?}"),
    }
}

fn keys(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .keys()
        .map(|k| k.to_php_str().to_string())
        .collect()
}

fn scratch(tag: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "php-rs-sf-{}-{}-{tag}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn meta_data_for_plain_memory_temp_and_data() {
    let mut t = TestVm::new();
    let d = scratch("meta");
    let f = format!("{d}/x");
    std::fs::write(&f, "abc").unwrap();
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    let m = t.call("stream_get_meta_data", vec![fp]).unwrap();
    assert_eq!(
        keys(&m),
        [
            "timed_out",
            "blocked",
            "eof",
            "wrapper_type",
            "stream_type",
            "mode",
            "unread_bytes",
            "seekable",
            "uri"
        ]
    );
    let a = m.as_array().unwrap();
    assert_eq!(bytes(a.get_str(b"wrapper_type").unwrap()), b"plainfile");
    assert_eq!(bytes(a.get_str(b"stream_type").unwrap()), b"STDIO");
    assert_eq!(a.get_str(b"seekable").unwrap().as_bool(), Some(true));
    assert_eq!(bytes(a.get_str(b"uri").unwrap()), f.as_bytes());

    let fp = t.call("fopen", vec![s("php://memory"), s("r+")]).unwrap();
    let m = t.call("stream_get_meta_data", vec![fp]).unwrap();
    let a = m.as_array().unwrap();
    assert_eq!(bytes(a.get_str(b"stream_type").unwrap()), b"MEMORY");
    assert_eq!(bytes(a.get_str(b"mode").unwrap()), b"w+b");
    assert_eq!(a.get_str(b"eof").unwrap().as_bool(), Some(false));

    let fp = t.call("fopen", vec![s("php://temp"), s("r+")]).unwrap();
    let m = t.call("stream_get_meta_data", vec![fp]).unwrap();
    assert_eq!(
        keys(&m),
        [
            "wrapper_type",
            "stream_type",
            "mode",
            "unread_bytes",
            "seekable",
            "uri"
        ]
    );

    let fp = t
        .call("fopen", vec![s("data:text/plain;foo=bar,x"), s("r")])
        .unwrap();
    let m = t.call("stream_get_meta_data", vec![fp]).unwrap();
    assert_eq!(
        keys(&m),
        [
            "mediatype",
            "foo",
            "base64",
            "wrapper_type",
            "stream_type",
            "mode",
            "unread_bytes",
            "seekable",
            "uri"
        ]
    );
    let a = m.as_array().unwrap();
    assert_eq!(bytes(a.get_str(b"mediatype").unwrap()), b"text/plain");
    assert_eq!(bytes(a.get_str(b"stream_type").unwrap()), b"RFC2397");
    assert_eq!(bytes(a.get_str(b"wrapper_type").unwrap()), b"RFC2397");
}

#[test]
fn get_contents_and_copy() {
    let mut t = TestVm::new();
    let fp = t.call("fopen", vec![s("php://memory"), s("w+")]).unwrap();
    t.call("fwrite", vec![fp.clone(), s("0123456789")]).unwrap();
    let r = t
        .call(
            "stream_get_contents",
            vec![fp.clone(), Value::Null, Value::Int(2)],
        )
        .unwrap();
    assert_eq!(bytes(&r), b"23456789");
    let r = t
        .call(
            "stream_get_contents",
            vec![fp.clone(), Value::Int(3), Value::Int(1)],
        )
        .unwrap();
    assert_eq!(bytes(&r), b"123");
    let (cls, msg) = thrown(t.call("stream_get_contents", vec![fp.clone(), Value::Int(-2)]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "stream_get_contents(): Argument #2 ($length) must be greater than or equal to -1"
    );
    let dst = t.call("fopen", vec![s("php://memory"), s("w+")]).unwrap();
    let n = t
        .call(
            "stream_copy_to_stream",
            vec![fp.clone(), dst.clone(), Value::Int(4), Value::Int(5)],
        )
        .unwrap();
    assert_eq!(n.as_int(), Some(4));
    t.call("rewind", vec![dst.clone()]).unwrap();
    assert_eq!(
        bytes(&t.call("stream_get_contents", vec![dst]).unwrap()),
        b"5678"
    );
}

#[test]
fn get_line_with_delimiters() {
    let mut t = TestVm::new();
    let fp = t.call("fopen", vec![s("php://memory"), s("w+")]).unwrap();
    t.call("fwrite", vec![fp.clone(), s("a||b||c")]).unwrap();
    t.call("rewind", vec![fp.clone()]).unwrap();
    let mut got = Vec::new();
    loop {
        let r = t
            .call(
                "stream_get_line",
                vec![fp.clone(), Value::Int(100), s("||")],
            )
            .unwrap();
        match r {
            Value::String(x) => got.push(x.to_vec()),
            _ => break,
        }
    }
    assert_eq!(got, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    let (cls, _) = thrown(t.call("stream_get_line", vec![fp, Value::Int(-1)]));
    assert_eq!(cls, "ValueError");
}

#[test]
fn wrappers_and_transports() {
    let mut t = TestVm::new();
    let w = t.call("stream_get_wrappers", vec![]).unwrap();
    let names: Vec<Vec<u8>> = w.as_array().unwrap().values().map(bytes).collect();
    for want in ["php", "file", "glob", "data"] {
        assert!(names.contains(&want.as_bytes().to_vec()), "{want}");
    }
    let tr = t.call("stream_get_transports", vec![]).unwrap();
    assert_eq!(tr.as_array().unwrap().len(), 4);
    assert_eq!(
        t.call("stream_is_local", vec![s("/etc/passwd")])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        t.call("stream_is_local", vec![s("data:,x")])
            .unwrap()
            .as_bool(),
        Some(false)
    );
}

#[test]
fn contexts_options_and_params() {
    let mut t = TestVm::new();
    let mut inner = PhpArray::new();
    inner.insert_str("method", "POST");
    let mut opts = PhpArray::new();
    opts.insert_str("http", Value::from(inner));
    let ctx = t
        .call("stream_context_create", vec![Value::from(opts)])
        .unwrap();
    let o = t
        .call("stream_context_get_options", vec![ctx.clone()])
        .unwrap();
    let http = o.as_array().unwrap().get_str(b"http").unwrap().clone();
    assert_eq!(
        bytes(http.as_array().unwrap().get_str(b"method").unwrap()),
        b"POST"
    );
    t.call(
        "stream_context_set_option",
        vec![ctx.clone(), s("http"), s("timeout"), Value::Int(5)],
    )
    .unwrap();
    let o = t
        .call("stream_context_get_options", vec![ctx.clone()])
        .unwrap();
    let http = o.as_array().unwrap().get_str(b"http").unwrap().clone();
    assert_eq!(http.as_array().unwrap().len(), 2);
    let p = t
        .call("stream_context_get_params", vec![ctx.clone()])
        .unwrap();
    assert_eq!(keys(&p), ["options"]);
    // malformed options
    let bad = Value::from(PhpArray::from_values(vec![s("x")]));
    let (cls, msg) = thrown(t.call("stream_context_create", vec![bad]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "Options should have the form [\"wrappername\"][\"optionname\"] = $value"
    );
    // two-argument form is deprecated
    let mut o2 = PhpArray::new();
    o2.insert_str("ftp", Value::from(PhpArray::new()));
    t.call(
        "stream_context_set_option",
        vec![ctx.clone(), Value::from(o2)],
    )
    .unwrap();
    assert!(t.output().contains(
        "Deprecated: Calling stream_context_set_option() with 2 arguments is deprecated, use stream_context_set_options() instead"
    ));
    let (cls, msg) = thrown(t.call("stream_context_set_option", vec![ctx.clone(), s("a")]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "stream_context_set_option(): Argument #3 ($option_name) cannot be null when argument #2 ($wrapper_or_options) is a string"
    );
    t.output();
    let d1 = t.call("stream_context_get_default", vec![]).unwrap();
    let d2 = t.call("stream_context_get_default", vec![]).unwrap();
    assert!(Value::identical(&d1, &d2));
    let (cls, msg) = thrown(t.call("stream_context_get_options", vec![s("x")]));
    assert_eq!(cls, "TypeError");
    assert_eq!(
        msg,
        "stream_context_get_options(): Argument #1 ($stream_or_context) must be of type resource, string given"
    );
}

#[test]
fn chunk_size_and_buffers() {
    let mut t = TestVm::new();
    let fp = t.call("fopen", vec![s("php://memory"), s("w+")]).unwrap();
    assert_eq!(
        t.call("stream_set_chunk_size", vec![fp.clone(), Value::Int(100)])
            .unwrap()
            .as_int(),
        Some(8192)
    );
    assert_eq!(
        t.call("stream_set_chunk_size", vec![fp.clone(), Value::Int(10)])
            .unwrap()
            .as_int(),
        Some(100)
    );
    let (cls, msg) = thrown(t.call("stream_set_chunk_size", vec![fp.clone(), Value::Int(0)]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "stream_set_chunk_size(): Argument #2 ($size) must be greater than 0"
    );
    assert_eq!(
        t.call("stream_set_read_buffer", vec![fp.clone(), Value::Int(0)])
            .unwrap()
            .as_int(),
        Some(0)
    );
    assert_eq!(
        t.call("stream_set_write_buffer", vec![fp.clone(), Value::Int(0)])
            .unwrap()
            .as_int(),
        Some(-1)
    );
    assert_eq!(
        t.call("stream_set_timeout", vec![fp.clone(), Value::Int(1)])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        t.call("stream_supports_lock", vec![fp.clone()])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        t.call("stream_isatty", vec![fp]).unwrap().as_bool(),
        Some(false)
    );
}

#[test]
fn socket_pair_roundtrip_and_select() {
    let mut t = TestVm::new();
    let pair = t
        .call(
            "stream_socket_pair",
            vec![Value::Int(1), Value::Int(1), Value::Int(0)],
        )
        .unwrap();
    let a = pair.as_array().unwrap().get_int(0).unwrap().clone();
    let b = pair.as_array().unwrap().get_int(1).unwrap().clone();
    assert_eq!(
        t.call("fwrite", vec![a.clone(), s("ping\n")])
            .unwrap()
            .as_int(),
        Some(5)
    );
    let read = Value::Ref(PhpRef::new(Value::from(PhpArray::from_values(vec![
        b.clone(),
    ]))));
    let n = t
        .call(
            "stream_select",
            vec![
                read.clone(),
                Value::Ref(PhpRef::new(Value::Null)),
                Value::Ref(PhpRef::new(Value::Null)),
                Value::Int(1),
            ],
        )
        .unwrap();
    assert_eq!(n.as_int(), Some(1));
    assert_eq!(read.unref().as_array().unwrap().len(), 1);
    assert_eq!(bytes(&t.call("fgets", vec![b.clone()]).unwrap()), b"ping\n");
    let m = t.call("stream_get_meta_data", vec![b.clone()]).unwrap();
    assert_eq!(
        bytes(m.as_array().unwrap().get_str(b"stream_type").unwrap()),
        b"generic_socket"
    );
    // nothing to read: select times out with 0
    let read = Value::Ref(PhpRef::new(Value::from(PhpArray::from_values(vec![
        b.clone(),
    ]))));
    let n = t
        .call(
            "stream_select",
            vec![
                read.clone(),
                Value::Ref(PhpRef::new(Value::Null)),
                Value::Ref(PhpRef::new(Value::Null)),
                Value::Int(0),
                Value::Int(1000),
            ],
        )
        .unwrap();
    assert_eq!(n.as_int(), Some(0));
    assert_eq!(read.unref().as_array().unwrap().len(), 0);
    let (cls, msg) = thrown(t.call(
        "stream_select",
        vec![
            Value::Ref(PhpRef::new(Value::Null)),
            Value::Ref(PhpRef::new(Value::Null)),
            Value::Ref(PhpRef::new(Value::Null)),
            Value::Int(0),
        ],
    ));
    assert_eq!(
        (cls.as_str(), msg.as_str()),
        ("ValueError", "No stream arrays were passed")
    );
    assert_eq!(
        t.call("stream_socket_shutdown", vec![a.clone(), Value::Int(1)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    let (cls, _) = thrown(t.call("stream_socket_shutdown", vec![a, Value::Int(7)]));
    assert_eq!(cls, "ValueError");
}

#[test]
fn tcp_server_client_accept() {
    let mut t = TestVm::new();
    let errno = Value::Ref(PhpRef::new(Value::Null));
    let errstr = Value::Ref(PhpRef::new(Value::Null));
    let srv = t
        .call(
            "stream_socket_server",
            vec![s("tcp://127.0.0.1:0"), errno.clone(), errstr.clone()],
        )
        .unwrap();
    assert!(matches!(srv, Value::Resource(_)), "{srv:?} {}", t.output());
    assert_eq!(errno.unref().as_int(), Some(0));
    let name = t
        .call(
            "stream_socket_get_name",
            vec![srv.clone(), Value::Bool(false)],
        )
        .unwrap();
    let name = String::from_utf8(bytes(&name)).unwrap();
    assert!(name.starts_with("127.0.0.1:"), "{name}");
    let client = t
        .call("stream_socket_client", vec![s(&format!("tcp://{name}"))])
        .unwrap();
    assert!(matches!(client, Value::Resource(_)));
    let peer = Value::Ref(PhpRef::new(Value::Null));
    let conn = t
        .call(
            "stream_socket_accept",
            vec![srv.clone(), Value::Float(2.0), peer.clone()],
        )
        .unwrap();
    assert!(matches!(conn, Value::Resource(_)));
    assert!(bytes(&peer.unref()).starts_with(b"127.0.0.1:"));
    t.call("fwrite", vec![client.clone(), s("hello")]).unwrap();
    assert_eq!(
        bytes(&t.call("fread", vec![conn.clone(), Value::Int(5)]).unwrap()),
        b"hello"
    );
    t.call("fclose", vec![client]).unwrap();
    assert_eq!(
        bytes(&t.call("fread", vec![conn.clone(), Value::Int(5)]).unwrap()),
        b""
    );
    assert_eq!(t.call("feof", vec![conn]).unwrap().as_bool(), Some(true));
    // accept with nothing pending times out
    let r = t
        .call("stream_socket_accept", vec![srv, Value::Float(0.05)])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    let out = t.output();
    assert!(
        out.contains("stream_socket_accept(): Accept failed: "),
        "{out}"
    );
}

#[test]
fn client_connection_failures() {
    let mut t = TestVm::new();
    let errno = Value::Ref(PhpRef::new(Value::Null));
    let errstr = Value::Ref(PhpRef::new(Value::Null));
    let r = t
        .call(
            "stream_socket_client",
            vec![s("foo://bar"), errno.clone(), errstr.clone()],
        )
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: stream_socket_client(): Unable to connect to foo://bar (Unable to find the socket transport \"foo\" - did you forget to enable it when you configured PHP?) in /t.php on line 3\n"
    );
    assert_eq!(errno.unref().as_int(), Some(0));
    let r = t
        .call("stream_socket_client", vec![s("tcp://localhost")])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: stream_socket_client(): Unable to connect to tcp://localhost (Failed to parse address \"localhost\") in /t.php on line 3\n"
    );
}

#[test]
fn filters_append_and_remove_without_factories() {
    let mut t = TestVm::new();
    let fp = t.call("fopen", vec![s("php://memory"), s("w+")]).unwrap();
    let r = t
        .call("stream_filter_append", vec![fp.clone(), s("no.such")])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: stream_filter_append(): Unable to locate filter \"no.such\" in /t.php on line 3\n"
    );
    let (cls, msg) = thrown(t.call("stream_filter_remove", vec![fp]));
    assert_eq!(cls, "TypeError");
    assert_eq!(
        msg,
        "stream_filter_remove(): supplied resource is not a valid stream filter resource"
    );
}

#[test]
fn meta_data_for_directories_and_servers() {
    // stream_get_meta_data_dir_basic.phpt
    let mut t = TestVm::new();
    let d = scratch("metadir");
    let h = t.call("opendir", vec![s(&d)]).unwrap();
    let m = t.call("stream_get_meta_data", vec![h]).unwrap();
    assert_eq!(
        keys(&m),
        [
            "timed_out",
            "blocked",
            "eof",
            "wrapper_type",
            "stream_type",
            "mode",
            "unread_bytes",
            "seekable"
        ]
    );
    let a = m.as_array().unwrap();
    assert_eq!(bytes(a.get_str(b"stream_type").unwrap()), b"dir");
    assert_eq!(bytes(a.get_str(b"mode").unwrap()), b"r");
    assert_eq!(a.get_str(b"seekable").unwrap().as_bool(), Some(true));
    assert_eq!(a.get_str(b"eof").unwrap().as_bool(), Some(false));
    // stream_get_meta_data_socket_basic.phpt
    let srv = t
        .call("stream_socket_server", vec![s("tcp://127.0.0.1:0")])
        .unwrap();
    let m = t.call("stream_get_meta_data", vec![srv]).unwrap();
    assert_eq!(
        keys(&m),
        [
            "timed_out",
            "blocked",
            "eof",
            "stream_type",
            "mode",
            "unread_bytes",
            "seekable",
            "uri"
        ]
    );
    let a = m.as_array().unwrap();
    assert_eq!(bytes(a.get_str(b"stream_type").unwrap()), b"tcp_socket");
    assert_eq!(bytes(a.get_str(b"mode").unwrap()), b"r+");
    assert_eq!(a.get_str(b"seekable").unwrap().as_bool(), Some(false));
}

#[test]
fn temp_stream_keeps_position_when_switching_to_file() {
    // temp_stream_seek.phpt
    let mut t = TestVm::new();
    let f = t
        .call("fopen", vec![s("php://temp/maxmemory:1024"), s("r+")])
        .unwrap();
    t.call("fwrite", vec![f.clone(), Value::from("1".repeat(738))])
        .unwrap();
    t.call("fseek", vec![f.clone(), Value::Int(0), Value::Int(0)])
        .unwrap();
    t.call("fwrite", vec![f.clone(), Value::from("2".repeat(512))])
        .unwrap();
    t.call("fseek", vec![f.clone(), Value::Int(0), Value::Int(0)])
        .unwrap();
    assert_eq!(
        bytes(&t.call("fread", vec![f.clone(), Value::Int(16)]).unwrap()),
        b"2222222222222222"
    );
    t.call("fseek", vec![f.clone(), Value::Int(0), Value::Int(2)])
        .unwrap();
    assert_eq!(t.call("ftell", vec![f]).unwrap().as_int(), Some(738));
}

#[test]
fn socket_pair_fread() {
    // stream_socket_pair.phpt
    let mut t = TestVm::new();
    let p = t
        .call(
            "stream_socket_pair",
            vec![Value::Int(1), Value::Int(1), Value::Int(0)],
        )
        .unwrap();
    let a = p.as_array().unwrap();
    let (x, y) = (a.get_int(0).unwrap().clone(), a.get_int(1).unwrap().clone());
    t.call("fwrite", vec![x.clone(), s("foo")]).unwrap();
    assert_eq!(
        bytes(&t.call("fread", vec![y, Value::Int(3)]).unwrap()),
        b"foo"
    );
    assert_eq!(t.call("fclose", vec![x]).unwrap().as_bool(), Some(true));
}

#[test]
fn context_param_errors() {
    // stream_context_create_error.phpt, stream_context_set_params_notification_error.phpt
    let mut t = TestVm::new();
    let mut ssl = PhpArray::new();
    ssl.insert_str("verify_peer", false);
    let mut opts = PhpArray::new();
    opts.insert_str("ssl", Value::from(ssl));
    let mut bad = PhpArray::new();
    bad.insert_str("ssl", "abc");
    let mut params = PhpArray::new();
    params.insert_str("options", Value::from(bad.clone()));
    let (cls, msg) = thrown(t.call(
        "stream_context_create",
        vec![Value::from(opts.clone()), Value::from(params)],
    ));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "Options should have the form [\"wrappername\"][\"optionname\"] = $value"
    );
    let mut params = PhpArray::new();
    params.insert_str("options", false);
    let (cls, msg) = thrown(t.call(
        "stream_context_create",
        vec![Value::from(opts), Value::from(params)],
    ));
    assert_eq!(
        (cls.as_str(), msg.as_str()),
        ("TypeError", "Invalid stream/context parameter")
    );
    let ctx = t.call("stream_context_create", vec![]).unwrap();
    let mut p = PhpArray::new();
    p.insert_str("notification", "fn_not_exist");
    let (cls, msg) = thrown(t.call("stream_context_set_params", vec![ctx, Value::from(p)]));
    assert_eq!(cls, "TypeError");
    assert_eq!(
        msg,
        "stream_context_set_params(): Argument #1 ($context) must be an array with valid callbacks as values, function \"fn_not_exist\" not found or invalid function name"
    );
}

#[test]
fn timeout_on_closed_and_plain_streams() {
    // stream_set_timeout_error.phpt
    let mut t = TestVm::new();
    let srv = t
        .call("stream_socket_server", vec![s("tcp://127.0.0.1:0")])
        .unwrap();
    let name = bytes(
        &t.call(
            "stream_socket_get_name",
            vec![srv.clone(), Value::Bool(false)],
        )
        .unwrap(),
    );
    let client = t
        .call(
            "fsockopen",
            vec![Value::from(format!(
                "tcp://{}",
                String::from_utf8_lossy(&name)
            ))],
        )
        .unwrap();
    assert_eq!(
        t.call("stream_set_timeout", vec![client.clone(), Value::Int(10)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    t.call("fclose", vec![client.clone()]).unwrap();
    let (cls, msg) = thrown(t.call("stream_set_timeout", vec![client, Value::Int(10)]));
    assert_eq!(cls, "TypeError");
    assert_eq!(
        msg,
        "stream_set_timeout(): Argument #1 ($stream) must be an open stream resource"
    );
}

#[test]
fn select_preserves_keys_and_emulates_buffered_reads() {
    // stream_select_preserve_keys.phpt, stream_select_null_usec.phpt
    let mut t = TestVm::new();
    let d = scratch("sel");
    let f = format!("{d}/f");
    std::fs::write(&f, "abc").unwrap();
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    let mut arr = PhpArray::new();
    arr.insert(php_runtime::ArrayKey::Int(1), fp.clone());
    arr.insert_str("myindex", fp.clone());
    let read = Value::Ref(PhpRef::new(Value::from(arr)));
    let nul = || Value::Ref(PhpRef::new(Value::Null));
    t.call(
        "stream_select",
        vec![read.clone(), nul(), nul(), Value::Int(0)],
    )
    .unwrap();
    assert_eq!(keys(&read.unref()), ["1", "myindex"]);
    t.call("fread", vec![fp.clone(), Value::Int(1)]).unwrap();
    // data is buffered now: the emulated read path keeps both keys too
    let n = t
        .call(
            "stream_select",
            vec![read.clone(), nul(), nul(), Value::Int(0)],
        )
        .unwrap();
    assert_eq!(n.as_int(), Some(2));
    assert_eq!(keys(&read.unref()), ["1", "myindex"]);
    t.call(
        "stream_select",
        vec![read.clone(), nul(), nul(), Value::Null, Value::Int(0)],
    )
    .unwrap();
    let (cls, msg) = thrown(t.call(
        "stream_select",
        vec![read, nul(), nul(), Value::Null, Value::Int(1)],
    ));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "stream_select(): Argument #5 ($microseconds) must be null when argument #4 ($seconds) is null"
    );
}

#[test]
fn get_contents_offsets_on_tmpfile() {
    // stream_get_contents_001.phpt
    let mut t = TestVm::new();
    let tmp = t.call("tmpfile", vec![]).unwrap();
    t.call("fwrite", vec![tmp.clone(), s("12345")]).unwrap();
    let get = |t: &mut TestVm, args: Vec<Value>| -> Vec<u8> {
        let mut a = vec![tmp.clone()];
        a.extend(args);
        bytes(&t.call("stream_get_contents", a).unwrap())
    };
    assert_eq!(get(&mut t, vec![Value::Int(2), Value::Int(5)]), b"");
    assert_eq!(get(&mut t, vec![Value::Int(2)]), b"");
    assert_eq!(get(&mut t, vec![Value::Int(2), Value::Int(3)]), b"45");
    assert_eq!(get(&mut t, vec![Value::Int(2), Value::Int(-1)]), b"");
}

#[test]
fn copy_into_append_mode_and_from_spilled_temp() {
    // stream_copy_to_stream_file_to_file_append.phpt,
    // stream_copy_to_stream_temp_source_partially_read.phpt
    let mut t = TestVm::new();
    let d = scratch("copyappend");
    let src = format!("{d}/src");
    let dst = format!("{d}/dst");
    std::fs::write(&src, "b".repeat(3000)).unwrap();
    std::fs::write(&dst, "PREFIX-").unwrap();
    let a = t.call("fopen", vec![s(&src), s("r")]).unwrap();
    let b = t.call("fopen", vec![s(&dst), s("a")]).unwrap();
    let n = t
        .call("stream_copy_to_stream", vec![a.clone(), b.clone()])
        .unwrap();
    assert_eq!(n.as_int(), Some(3000));
    t.call("fclose", vec![a]).unwrap();
    t.call("fclose", vec![b]).unwrap();
    let r = std::fs::read(&dst).unwrap();
    assert_eq!(r.len(), 3007);
    assert!(r.starts_with(b"PREFIX-b"));

    let src = t
        .call("fopen", vec![s("php://temp/maxmemory:16"), s("r+")])
        .unwrap();
    let data = format!("{}{}", "A".repeat(2000), "B".repeat(2000));
    t.call("fwrite", vec![src.clone(), Value::from(data)])
        .unwrap();
    t.call("rewind", vec![src.clone()]).unwrap();
    let first = t
        .call("fread", vec![src.clone(), Value::Int(2000)])
        .unwrap();
    assert_eq!(bytes(&first).len(), 2000);
    let dst = t.call("tmpfile", vec![]).unwrap();
    let n = t
        .call("stream_copy_to_stream", vec![src, dst.clone()])
        .unwrap();
    assert_eq!(n.as_int(), Some(2000));
    t.call("rewind", vec![dst.clone()]).unwrap();
    assert_eq!(
        bytes(&t.call("stream_get_contents", vec![dst]).unwrap()),
        "B".repeat(2000).as_bytes()
    );
}
