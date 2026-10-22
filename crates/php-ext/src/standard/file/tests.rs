//! Tests for ext/standard/file.c (expectations from ext/standard/tests/file/*.phpt).

use crate::testing::{TestVm, thrown};
use php_runtime::{PhpRef, Value};

/// A unique scratch directory under the system temp dir.
pub(crate) fn scratch(tag: &str) -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "php-rs-file-{}-{}-{tag}",
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

fn s(v: &str) -> Value {
    Value::str(v)
}

fn as_bytes(v: &Value) -> Vec<u8> {
    match v {
        Value::String(s) => s.to_vec(),
        other => panic!("expected string, got {other:?}"),
    }
}

#[test]
fn fopen_fwrite_fread_roundtrip() {
    let mut t = TestVm::new();
    let d = scratch("rw");
    let f = format!("{d}/a.txt");
    let fp = t.call("fopen", vec![s(&f), s("w+")]).unwrap();
    assert!(matches!(fp, Value::Resource(_)));
    assert_eq!(
        t.call("fwrite", vec![fp.clone(), s("hello\nworld\n")])
            .unwrap()
            .as_int(),
        Some(12)
    );
    assert_eq!(
        t.call("ftell", vec![fp.clone()]).unwrap().as_int(),
        Some(12)
    );
    assert_eq!(
        t.call("rewind", vec![fp.clone()]).unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone()]).unwrap()),
        b"hello\n"
    );
    assert_eq!(
        as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(100)]).unwrap()),
        b"world\n"
    );
    // the greedy plain-file read hit EOF while trying to fill 100 bytes
    assert_eq!(
        t.call("feof", vec![fp.clone()]).unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        t.call("fread", vec![fp.clone(), Value::Int(1)])
            .unwrap()
            .as_str()
            .map(|s| s.len()),
        Some(0)
    );
    assert_eq!(
        t.call("feof", vec![fp.clone()]).unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        t.call("fclose", vec![fp.clone()]).unwrap().as_bool(),
        Some(true)
    );
    let (cls, msg) = thrown(t.call("feof", vec![fp]));
    assert_eq!(cls, "TypeError");
    assert_eq!(
        msg,
        "feof(): Argument #1 ($stream) must be an open stream resource"
    );
}

#[test]
fn fopen_missing_file_warns() {
    let mut t = TestVm::new();
    let d = scratch("missing");
    let r = t
        .call("fopen", vec![s(&format!("{d}/nope.txt")), s("r")])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: No such file or directory in /t.php on line 3\n"
    );
    let r = t.call("fopen", vec![s(&format!("{d}/x")), s("z")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: `z' is not a valid mode for fopen in /t.php on line 3\n"
    );
    let (cls, msg) = thrown(t.call("fopen", vec![s(""), s("r")]));
    assert_eq!(
        (cls.as_str(), msg.as_str()),
        ("ValueError", "Path must not be empty")
    );
    let (cls, msg) = thrown(t.call("fopen", vec![s("a\0b"), s("r")]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "fopen(): Argument #1 ($filename) must not contain any null bytes"
    );
}

#[test]
fn file_put_and_get_contents() {
    let mut t = TestVm::new();
    let d = scratch("fpc");
    let f = format!("{d}/b.txt");
    assert_eq!(
        t.call("file_put_contents", vec![s(&f), s("abc")])
            .unwrap()
            .as_int(),
        Some(3)
    );
    assert_eq!(
        t.call("file_put_contents", vec![s(&f), s("def"), Value::Int(8)])
            .unwrap()
            .as_int(),
        Some(3)
    );
    assert_eq!(
        as_bytes(&t.call("file_get_contents", vec![s(&f)]).unwrap()),
        b"abcdef"
    );
    let arr = Value::from(php_runtime::PhpArray::from_values(vec![
        s("x"),
        Value::Int(1),
        s(""),
    ]));
    assert_eq!(
        t.call("file_put_contents", vec![s(&f), arr])
            .unwrap()
            .as_int(),
        Some(2)
    );
    assert_eq!(
        as_bytes(&t.call("file_get_contents", vec![s(&f)]).unwrap()),
        b"x1"
    );
    // offset / length
    t.call("file_put_contents", vec![s(&f), s("0123456789")])
        .unwrap();
    let r = t
        .call(
            "file_get_contents",
            vec![
                s(&f),
                Value::Bool(false),
                Value::Null,
                Value::Int(3),
                Value::Int(4),
            ],
        )
        .unwrap();
    assert_eq!(as_bytes(&r), b"3456");
    let r = t
        .call(
            "file_get_contents",
            vec![s(&f), Value::Bool(false), Value::Null, Value::Int(-2)],
        )
        .unwrap();
    assert_eq!(as_bytes(&r), b"89");
    let (cls, msg) = thrown(t.call(
        "file_get_contents",
        vec![
            s(&f),
            Value::Bool(false),
            Value::Null,
            Value::Int(0),
            Value::Int(-1),
        ],
    ));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "file_get_contents(): Argument #5 ($length) must be greater than or equal to 0"
    );
    // a directory
    let r = t.call("file_get_contents", vec![s(&d)]).unwrap();
    assert_eq!(as_bytes(&r), b"");
    // "Read of %d bytes": the size is the directory's st_size + 8192
    let out = t.output();
    assert!(
        out.starts_with("\nNotice: file_get_contents(): Read of ")
            && out.ends_with(" bytes failed with errno=21 Is a directory in /t.php on line 3\n"),
        "{out}"
    );
}

#[test]
fn file_function_flags() {
    let mut t = TestVm::new();
    let d = scratch("file");
    let f = format!("{d}/c.txt");
    std::fs::write(&f, "a\r\n\nb\nc").unwrap();
    let get = |v: Value| -> Vec<Vec<u8>> {
        match v {
            Value::Array(a) => a.values().map(as_bytes).collect(),
            o => panic!("{o:?}"),
        }
    };
    let r = get(t.call("file", vec![s(&f)]).unwrap());
    assert_eq!(
        r,
        vec![
            b"a\r\n".to_vec(),
            b"\n".to_vec(),
            b"b\n".to_vec(),
            b"c".to_vec()
        ]
    );
    let r = get(t.call("file", vec![s(&f), Value::Int(2)]).unwrap());
    assert_eq!(
        r,
        vec![b"a".to_vec(), b"".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );
    let r = get(t.call("file", vec![s(&f), Value::Int(6)]).unwrap());
    assert_eq!(r, vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]);
    let (cls, msg) = thrown(t.call("file", vec![s(&f), Value::Int(64)]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "file(): Argument #2 ($flags) must be a valid flag value"
    );
}

#[test]
fn fgets_length_semantics() {
    let mut t = TestVm::new();
    let d = scratch("fgets");
    let f = format!("{d}/d.txt");
    std::fs::write(&f, "line1\nline2\n").unwrap();
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    // length includes the terminating NUL: at most length-1 bytes
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone(), Value::Int(3)]).unwrap()),
        b"li"
    );
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone(), Value::Int(100)]).unwrap()),
        b"ne1\n"
    );
    assert_eq!(as_bytes(&t.call("fgetc", vec![fp.clone()]).unwrap()), b"l");
    let (cls, msg) = thrown(t.call("fgets", vec![fp.clone(), Value::Int(0)]));
    assert_eq!(cls, "ValueError");
    assert_eq!(msg, "fgets(): Argument #2 ($length) must be greater than 0");
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone()]).unwrap()),
        b"ine2\n"
    );
    assert_eq!(
        t.call("fgets", vec![fp.clone()]).unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(t.call("fgetc", vec![fp]).unwrap().as_bool(), Some(false));
}

#[test]
fn read_on_write_only_stream_notices() {
    let mut t = TestVm::new();
    let d = scratch("wo");
    let f = format!("{d}/e.txt");
    let fp = t.call("fopen", vec![s(&f), s("w")]).unwrap();
    let r = t.call("fread", vec![fp.clone(), Value::Int(10)]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    let out = t.output();
    assert!(
        out.starts_with(
            "\nNotice: fread(): Read of 8192 bytes failed with errno=9 Bad file descriptor"
        ),
        "{out}"
    );
    t.call("fclose", vec![fp]).unwrap();
}

#[test]
fn fseek_ftell_and_append_mode() {
    let mut t = TestVm::new();
    let d = scratch("seek");
    let f = format!("{d}/f.txt");
    std::fs::write(&f, "0123456789").unwrap();
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    assert_eq!(
        t.call("fseek", vec![fp.clone(), Value::Int(4)])
            .unwrap()
            .as_int(),
        Some(0)
    );
    assert_eq!(
        as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(2)]).unwrap()),
        b"45"
    );
    assert_eq!(
        t.call("fseek", vec![fp.clone(), Value::Int(-1), Value::Int(1)])
            .unwrap()
            .as_int(),
        Some(0)
    );
    assert_eq!(t.call("ftell", vec![fp.clone()]).unwrap().as_int(), Some(5));
    assert_eq!(
        t.call("fseek", vec![fp.clone(), Value::Int(-3), Value::Int(2)])
            .unwrap()
            .as_int(),
        Some(0)
    );
    assert_eq!(
        as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(10)]).unwrap()),
        b"789"
    );
    assert_eq!(
        t.call("fseek", vec![fp.clone(), Value::Int(-1)])
            .unwrap()
            .as_int(),
        Some(-1)
    );
    t.call("fclose", vec![fp]).unwrap();
    let fp = t.call("fopen", vec![s(&f), s("a")]).unwrap();
    assert_eq!(t.call("ftell", vec![fp.clone()]).unwrap().as_int(), Some(0));
    t.call("fwrite", vec![fp.clone(), s("X")]).unwrap();
    t.call("fclose", vec![fp]).unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"0123456789X");
}

#[test]
fn memory_and_temp_streams() {
    let mut t = TestVm::new();
    for url in ["php://memory", "php://temp", "php://temp/maxmemory:4"] {
        let fp = t.call("fopen", vec![s(url), s("w+")]).unwrap();
        t.call("fwrite", vec![fp.clone(), s("hello world")])
            .unwrap();
        t.call("rewind", vec![fp.clone()]).unwrap();
        assert_eq!(
            as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(5)]).unwrap()),
            b"hello",
            "{url}"
        );
        assert_eq!(t.call("ftell", vec![fp.clone()]).unwrap().as_int(), Some(5));
        let st = t.call("fstat", vec![fp.clone()]).unwrap();
        let size = st.as_array().unwrap().get_str(b"size").cloned();
        assert_eq!(size.and_then(|v| v.as_int()), Some(11), "{url}");
        assert_eq!(
            t.call("ftruncate", vec![fp.clone(), Value::Int(2)])
                .unwrap()
                .as_bool(),
            Some(true)
        );
        t.call("rewind", vec![fp.clone()]).unwrap();
        assert_eq!(
            as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(50)]).unwrap()),
            b"he"
        );
        t.call("fclose", vec![fp]).unwrap();
    }
}

#[test]
fn data_urls() {
    let mut t = TestVm::new();
    let r = t
        .call(
            "file_get_contents",
            vec![s("data://text/plain;base64,SGVsbG8=")],
        )
        .unwrap();
    assert_eq!(as_bytes(&r), b"Hello");
    let r = t
        .call("file_get_contents", vec![s("data:,a%20b+c")])
        .unwrap();
    assert_eq!(as_bytes(&r), b"a b c");
    let r = t
        .call("fopen", vec![s("data://text/plain"), s("r")])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: rfc2397: no comma in URL in /t.php on line 3\n"
    );
    let r = t.call("fopen", vec![s("data://foo,bar"), s("r")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: rfc2397: illegal media type in /t.php on line 3\n"
    );
}

#[test]
fn copy_rename_unlink_mkdir_rmdir() {
    let mut t = TestVm::new();
    let d = scratch("ops");
    let a = format!("{d}/a");
    let b = format!("{d}/b");
    std::fs::write(&a, "data").unwrap();
    assert_eq!(
        t.call("copy", vec![s(&a), s(&b)]).unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(std::fs::read(&b).unwrap(), b"data");
    assert_eq!(
        t.call("copy", vec![s(&d), s(&b)]).unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(
        t.output(),
        "\nWarning: copy(): The first argument to copy() function cannot be a directory in /t.php on line 3\n"
    );
    let c = format!("{d}/c");
    assert_eq!(
        t.call("rename", vec![s(&b), s(&c)]).unwrap().as_bool(),
        Some(true)
    );
    assert!(!std::path::Path::new(&b).exists());
    assert_eq!(t.call("unlink", vec![s(&c)]).unwrap().as_bool(), Some(true));
    assert_eq!(
        t.call("unlink", vec![s(&c)]).unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(
        t.output(),
        "\nWarning: unlink(): No such file or directory in /t.php on line 3\n"
    );
    let deep = format!("{d}/x/y/z");
    assert_eq!(
        t.call("mkdir", vec![s(&deep)]).unwrap().as_bool(),
        Some(false)
    );
    assert_eq!(
        t.output(),
        "\nWarning: mkdir(): No such file or directory in /t.php on line 3\n"
    );
    assert_eq!(
        t.call(
            "mkdir",
            vec![s(&deep), Value::Int(0o777), Value::Bool(true)]
        )
        .unwrap()
        .as_bool(),
        Some(true)
    );
    assert!(std::path::Path::new(&deep).is_dir());
    assert_eq!(
        t.call(
            "mkdir",
            vec![s(&deep), Value::Int(0o777), Value::Bool(true)]
        )
        .unwrap()
        .as_bool(),
        Some(false)
    );
    assert_eq!(
        t.output(),
        "\nWarning: mkdir(): File exists in /t.php on line 3\n"
    );
    assert_eq!(
        t.call("rmdir", vec![s(&deep)]).unwrap().as_bool(),
        Some(true)
    );
    assert_eq!(
        t.call("rmdir", vec![s(&format!("{d}/x"))])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        t.output(),
        "\nWarning: rmdir(): Directory not empty in /t.php on line 3\n"
    );
}

#[test]
fn tmpfile_and_tempnam() {
    let mut t = TestVm::new();
    let fp = t.call("tmpfile", vec![]).unwrap();
    t.call("fwrite", vec![fp.clone(), s("tmp")]).unwrap();
    t.call("rewind", vec![fp.clone()]).unwrap();
    assert_eq!(
        as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(10)]).unwrap()),
        b"tmp"
    );
    t.call("fclose", vec![fp]).unwrap();
    let d = scratch("tempnam");
    let p = t.call("tempnam", vec![s(&d), s("pfx")]).unwrap();
    let p = String::from_utf8(as_bytes(&p)).unwrap();
    assert!(p.starts_with(&format!("{d}/pfx")), "{p}");
    assert!(std::path::Path::new(&p).is_file());
    let p2 = t
        .call("tempnam", vec![s(&format!("{d}/missing")), s("x")])
        .unwrap();
    assert!(p2.as_str().is_some());
    assert_eq!(
        t.output(),
        "\nNotice: tempnam(): file created in the system's temporary directory in /t.php on line 3\n"
    );
    let _ = std::fs::remove_file(as_bytes(&p2).iter().map(|&c| c as char).collect::<String>());
}

#[test]
fn flock_on_files() {
    let mut t = TestVm::new();
    let d = scratch("flock");
    let f = format!("{d}/l");
    let fp = t.call("fopen", vec![s(&f), s("w")]).unwrap();
    let wb = Value::Ref(PhpRef::new(Value::Null));
    assert_eq!(
        t.call("flock", vec![fp.clone(), Value::Int(2), wb.clone()])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    assert_eq!(wb.unref().as_int(), Some(0));
    assert_eq!(
        t.call("flock", vec![fp.clone(), Value::Int(3)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    let (cls, msg) = thrown(t.call("flock", vec![fp, Value::Int(0)]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "flock(): Argument #2 ($operation) must be one of LOCK_SH, LOCK_EX, or LOCK_UN"
    );
}

#[test]
fn readfile_and_fpassthru_echo() {
    let mut t = TestVm::new();
    let d = scratch("readfile");
    let f = format!("{d}/r");
    std::fs::write(&f, "abcdef").unwrap();
    assert_eq!(t.call("readfile", vec![s(&f)]).unwrap().as_int(), Some(6));
    assert_eq!(t.output(), "abcdef");
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    t.call("fread", vec![fp.clone(), Value::Int(2)]).unwrap();
    assert_eq!(t.call("fpassthru", vec![fp]).unwrap().as_int(), Some(4));
    assert_eq!(t.output(), "cdef");
    let r = t.call("readfile", vec![s(&format!("{d}/none"))]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: readfile(): Failed to open stream: No such file or directory in /t.php on line 3\n"
    );
}

#[test]
fn popen_and_pclose() {
    let mut t = TestVm::new();
    let fp = t.call("popen", vec![s("echo hi; exit 3"), s("r")]).unwrap();
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone()]).unwrap()),
        b"hi\n"
    );
    assert_eq!(t.call("pclose", vec![fp]).unwrap().as_int(), Some(3));
    let (cls, msg) = thrown(t.call("popen", vec![s("true"), s("rw")]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "popen(): Argument #2 ($mode) must be one of \"r\", \"rb\", \"w\", or \"wb\""
    );
}

#[test]
fn get_meta_tags_parses_head() {
    let mut t = TestVm::new();
    let d = scratch("meta");
    let f = format!("{d}/m.html");
    std::fs::write(
        &f,
        "<html><head><meta name=\"author\" content=\"name\">\n<meta name=\"keywords\" content=\"php documentation\">\n<meta name=\"DESCRIPTION\" content=\"a php manual\">\n<meta name=\"geo.position\" content=\"49.33;-86.59\">\n</head> <!-- parsing stops here -->",
    )
    .unwrap();
    let r = t.call("get_meta_tags", vec![s(&f)]).unwrap();
    let a = r.as_array().unwrap();
    let keys: Vec<String> = a.keys().map(|k| k.to_php_str().to_string()).collect();
    assert_eq!(
        keys,
        vec!["author", "keywords", "description", "geo_position"]
    );
    assert_eq!(
        as_bytes(a.get_str(b"geo_position").unwrap()),
        b"49.33;-86.59"
    );
}

#[test]
fn realpath_fnmatch_and_temp_dir() {
    let mut t = TestVm::new();
    let d = scratch("realpath");
    let r = t
        .call(
            "realpath",
            vec![s(&format!("{d}/./../{}", d.rsplit('/').next().unwrap()))],
        )
        .unwrap();
    assert_eq!(as_bytes(&r), d.as_bytes());
    assert_eq!(
        t.call("realpath", vec![s(&format!("{d}/nope"))])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    assert_eq!(
        t.call("fnmatch", vec![s("*.txt"), s("a.txt")])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        t.call("fnmatch", vec![s("[!a]*"), s("abc")])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    let (cls, msg) = thrown(t.call("fnmatch", vec![s("a\0"), s("a")]));
    assert_eq!(cls, "ValueError");
    assert_eq!(
        msg,
        "fnmatch(): Argument #1 ($pattern) must not contain any null bytes"
    );
    assert!(
        t.call("sys_get_temp_dir", vec![])
            .unwrap()
            .as_str()
            .is_some()
    );
}

#[test]
fn stdout_constants_are_writable() {
    let mut t = TestVm::new();
    let out = t.vm.constant("STDERR").unwrap();
    assert_eq!(
        t.call("fwrite", vec![out, s("")]).unwrap().as_int(),
        Some(0)
    );
    let o = t.call("fopen", vec![s("php://output"), s("w")]).unwrap();
    assert_eq!(
        t.call("fwrite", vec![o, s("via output")]).unwrap().as_int(),
        Some(10)
    );
    assert_eq!(t.output(), "via output");
}

#[test]
fn php_filter_url_without_resource_throws() {
    let mut t = TestVm::new();
    let (cls, msg) = thrown(t.call("fopen", vec![s("php://filter/read=string.rot13"), s("r")]));
    assert_eq!(
        (cls.as_str(), msg.as_str()),
        ("Error", "No URL resource specified")
    );
    let r = t.call("fopen", vec![s("php://nope"), s("r")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: operation failed in /t.php on line 3\n"
    );
}

#[test]
fn data_urls_are_not_writable() {
    // bug79467.phpt
    let mut t = TestVm::new();
    let r = t
        .call(
            "file_put_contents",
            vec![s("data://text/plain,cccc"), s("data")],
        )
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nNotice: file_put_contents(): Stream is not writable in /t.php on line 3\n"
    );
}

#[test]
fn remote_file_urls_are_rejected() {
    // fopen_variation14.phpt
    let mut t = TestVm::new();
    let r = t
        .call("fopen", vec![s("file://dir/x.tmp"), s("r")])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Remote host file access not supported, file://dir/x.tmp in /t.php on line 3\n\nWarning: fopen(): Failed to open stream: no suitable wrapper could be found in /t.php on line 3\n"
    );
    let d = scratch("fileurl");
    let f = format!("{d}/x");
    std::fs::write(&f, "via url").unwrap();
    let r = t
        .call("file_get_contents", vec![s(&format!("file://{f}"))])
        .unwrap();
    assert_eq!(as_bytes(&r), b"via url");
    let r = t
        .call(
            "file_get_contents",
            vec![s(&format!("file://localhost{f}"))],
        )
        .unwrap();
    assert_eq!(as_bytes(&r), b"via url");
}

#[test]
fn write_to_read_only_stream_notices() {
    let mut t = TestVm::new();
    let d = scratch("ro");
    let f = format!("{d}/r");
    std::fs::write(&f, "x").unwrap();
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    assert_eq!(
        t.call("fwrite", vec![fp.clone(), s("data")])
            .unwrap()
            .as_bool(),
        Some(false)
    );
    let out = t.output();
    assert!(
        out.starts_with(
            "\nNotice: fwrite(): Write of 4 bytes failed with errno=9 Bad file descriptor"
        ),
        "{out}"
    );
    assert_eq!(
        t.call("fwrite", vec![fp.clone(), s("data"), Value::Int(0)])
            .unwrap()
            .as_int(),
        Some(0)
    );
    t.call("fclose", vec![fp]).unwrap();
}

#[test]
fn exclusive_and_create_modes() {
    let mut t = TestVm::new();
    let d = scratch("modes");
    let f = format!("{d}/m");
    let fp = t.call("fopen", vec![s(&f), s("x")]).unwrap();
    assert!(matches!(fp, Value::Resource(_)));
    t.call("fwrite", vec![fp.clone(), s("12345")]).unwrap();
    t.call("fclose", vec![fp]).unwrap();
    let r = t.call("fopen", vec![s(&f), s("x+")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: File exists in /t.php on line 3\n"
    );
    // "c" keeps the contents, "w" truncates
    let fp = t.call("fopen", vec![s(&f), s("c+")]).unwrap();
    assert_eq!(
        as_bytes(&t.call("fread", vec![fp.clone(), Value::Int(10)]).unwrap()),
        b"12345"
    );
    t.call("fclose", vec![fp]).unwrap();
    let fp = t.call("fopen", vec![s(&f), s("w")]).unwrap();
    t.call("fclose", vec![fp]).unwrap();
    assert_eq!(std::fs::read(&f).unwrap(), b"");
}

#[test]
fn include_path_lookup() {
    let mut t = TestVm::new();
    let d = scratch("incpath");
    std::fs::write(format!("{d}/inc.txt"), "found").unwrap();
    t.vm.ini_set("include_path", format!(".:{d}"));
    let r = t
        .call("file_get_contents", vec![s("inc.txt"), Value::Bool(true)])
        .unwrap();
    assert_eq!(as_bytes(&r), b"found");
    let r = t
        .call("stream_resolve_include_path", vec![s("inc.txt")])
        .unwrap();
    assert_eq!(as_bytes(&r), format!("{d}/inc.txt").as_bytes());
    let fp = t
        .call("fopen", vec![s("inc.txt"), s("r"), Value::Bool(true)])
        .unwrap();
    assert!(matches!(fp, Value::Resource(_)));
}

#[test]
fn open_basedir_restriction() {
    let mut t = TestVm::new();
    let d = scratch("obd");
    let inside = format!("{d}/in.txt");
    std::fs::write(&inside, "ok").unwrap();
    t.vm.ini_set("open_basedir", d.clone());
    assert_eq!(
        as_bytes(&t.call("file_get_contents", vec![s(&inside)]).unwrap()),
        b"ok"
    );
    let r = t.call("file_get_contents", vec![s("/etc/hosts")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        format!(
            "\nWarning: file_get_contents(): open_basedir restriction in effect. File(/etc/hosts) is not within the allowed path(s): ({d}) in /t.php on line 3\n\nWarning: file_get_contents(): Failed to open stream: Operation not permitted in /t.php on line 3\n"
        )
    );
}

#[test]
fn flock_would_block_between_handles() {
    let mut t = TestVm::new();
    let d = scratch("flock2");
    let f = format!("{d}/l");
    let a = t.call("fopen", vec![s(&f), s("w")]).unwrap();
    let b = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    assert_eq!(
        t.call("flock", vec![a.clone(), Value::Int(2)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    let wb = Value::Ref(PhpRef::new(Value::Null));
    let r = t
        .call("flock", vec![b.clone(), Value::Int(2 | 4), wb.clone()])
        .unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(wb.unref().as_int(), Some(1));
    t.call("fclose", vec![a]).unwrap();
    assert_eq!(
        t.call("flock", vec![b, Value::Int(2 | 4)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
}

#[test]
fn php_fd_and_std_streams() {
    let mut t = TestVm::new();
    let fp = t.call("fopen", vec![s("php://fd/1"), s("w")]).unwrap();
    assert!(matches!(fp, Value::Resource(_)));
    let r = t.call("fopen", vec![s("php://fd/x"), s("w")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert_eq!(
        t.output(),
        "\nWarning: fopen(): Failed to open stream: php://fd/ stream must be specified in the form php://fd/<orig fd> in /t.php on line 3\n"
    );
    let r = t.call("fopen", vec![s("php://fd/-1"), s("w")]).unwrap();
    assert_eq!(r.as_bool(), Some(false));
    assert!(t.output().contains(
        "Failed to open stream: The file descriptors must be non-negative numbers smaller than"
    ));
    for url in ["php://stdin", "php://STDERR"] {
        let fp = t.call("fopen", vec![s(url), s("r")]).unwrap();
        let m = t.call("stream_get_meta_data", vec![fp]).unwrap();
        let a = m.as_array().unwrap();
        assert_eq!(as_bytes(a.get_str(b"wrapper_type").unwrap()), b"PHP");
        assert_eq!(as_bytes(a.get_str(b"uri").unwrap()), url.as_bytes());
    }
}

#[test]
fn tmpfile_metadata_and_fstat() {
    let mut t = TestVm::new();
    let fp = t.call("tmpfile", vec![]).unwrap();
    let m = t.call("stream_get_meta_data", vec![fp.clone()]).unwrap();
    let a = m.as_array().unwrap();
    assert_eq!(as_bytes(a.get_str(b"wrapper_type").unwrap()), b"plainfile");
    assert_eq!(as_bytes(a.get_str(b"mode").unwrap()), b"r+b");
    let uri = String::from_utf8(as_bytes(a.get_str(b"uri").unwrap())).unwrap();
    assert!(std::path::Path::new(&uri).exists());
    t.call("fwrite", vec![fp.clone(), s("12345")]).unwrap();
    let st = t.call("fstat", vec![fp.clone()]).unwrap();
    let st = st.as_array().unwrap();
    assert_eq!(st.len(), 26);
    assert_eq!(st.get_int(7).and_then(|v| v.as_int()), Some(5));
    assert_eq!(st.get_str(b"size").and_then(|v| v.as_int()), Some(5));
    assert_eq!(
        t.call("ftruncate", vec![fp.clone(), Value::Int(2)])
            .unwrap()
            .as_bool(),
        Some(true)
    );
    t.call("fclose", vec![fp]).unwrap();
    // the temporary file is removed on close
    assert!(!std::path::Path::new(&uri).exists());
}

#[test]
fn mac_line_endings_with_auto_detect() {
    let mut t = TestVm::new();
    let d = scratch("eol");
    let f = format!("{d}/mac");
    std::fs::write(&f, "a\rb\rc").unwrap();
    t.vm.ini_set("auto_detect_line_endings", "1");
    let fp = t.call("fopen", vec![s(&f), s("r")]).unwrap();
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone()]).unwrap()),
        b"a\r"
    );
    assert_eq!(
        as_bytes(&t.call("fgets", vec![fp.clone()]).unwrap()),
        b"b\r"
    );
    assert_eq!(as_bytes(&t.call("fgets", vec![fp]).unwrap()), b"c");
    let r = t.call("file", vec![s(&f)]).unwrap();
    assert_eq!(r.as_array().unwrap().len(), 3);
}
