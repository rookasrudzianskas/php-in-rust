//! `SplFileObject` / `SplTempFileObject`.

use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpRef, Value, Vm};

use super::{
    TempDir, debug_strs, is_false, is_true, m, new, obj_of, pairs, str_of, try_m, try_new,
};
use crate::spl::testutil::{entries, list, s, user_class};
use crate::testing::{TestVm, thrown};

const DROP_NEW_LINE: i64 = 1;
const READ_AHEAD: i64 = 2;
const SKIP_EMPTY: i64 = 4;
const READ_CSV: i64 = 8;

/// The `--FILE--` section of a phpt test (what `__FILE__` reads).
fn script(lines: &[&str]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// The fixtures next to the phpt files.
fn fixture(name: &str) -> String {
    let p = format!(
        "{}/../../tests/phpt/ext/spl/tests/SplFileObject/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

const FIVE_LINES: &[&str] = &[
    "<?php",
    "//line 2",
    "//line 3",
    "//line 4",
    "//line 5",
    "$s = new SplFileObject(__FILE__);",
];

/// A test script starting with [`FIVE_LINES`] followed by `rest`.
fn five_lines_and(d: &TempDir, rest: &[&str]) -> String {
    let mut lines = FIVE_LINES.to_vec();
    lines.extend_from_slice(rest);
    d.file("test.php", &script(&lines))
}

fn open(t: &mut TestVm, path: &str) -> ObjRef {
    new(t, "SplFileObject", vec![s(path)])
}

fn int(v: &Value) -> i64 {
    v.as_int().unwrap()
}

fn deprecated(method: &str) -> String {
    format!(
        "\nDeprecated: Method SplFileObject::{method}() is deprecated since 8.6 in /t.php on line 3\n"
    )
}

/// `SplTempFileObject` holding `line {i}\n` for `i` in `0..n`.
fn temp_lines(t: &mut TestVm, n: usize) -> ObjRef {
    let f = new(t, "SplTempFileObject", vec![]);
    for i in 0..n {
        m(t, &f, "fwrite", vec![s(&format!("line {i}\n"))]);
    }
    f
}

fn trim(v: &Value) -> String {
    str_of(v).trim().to_string()
}

#[test]
fn current_next_and_seek() {
    // SplFileObject_current_basic.phpt, _current_variation001.phpt,
    // _next_basic.phpt, _seek_basic.phpt, _key_basic.phpt
    let d = TempDir::new("current");
    let p = five_lines_and(&d, &["", "echo $s->current();", "?>"]);
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "<?php\n");
    m(&mut t, &f, "next", vec![]);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "//line 2\n");
    m(&mut t, &f, "seek", vec![Value::Int(2)]);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "//line 3\n");
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "//line 3\n");
    m(&mut t, &f, "seek", vec![Value::Int(3)]);
    assert_eq!(int(&m(&mut t, &f, "key", vec![])), 3);
    m(&mut t, &f, "rewind", vec![]);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "<?php\n");
}

#[test]
fn next_with_read_ahead() {
    // SplFileObject_next_variation002.phpt
    let d = TempDir::new("readahead");
    let p = five_lines_and(&d, &["$s->seek(2);", "?>"]);
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    for flags in [0, READ_AHEAD] {
        m(&mut t, &f, "setFlags", vec![Value::Int(flags)]);
        m(&mut t, &f, "seek", vec![Value::Int(2)]);
        assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "//line 3\n");
        m(&mut t, &f, "next", vec![]);
        assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "//line 4\n");
    }
}

#[test]
fn seeking_past_the_end() {
    // SplFileObject_next_variation001.phpt (14 lines),
    // SplFileObject_rewind_variation001.phpt (16 lines),
    // SplFileObject_key_error001.phpt / _key_error002.phpt (12 lines),
    // SplFileObject_seek_error002.phpt (11 lines)
    let d = TempDir::new("pastend");
    let mut t = TestVm::new();
    let lines = |n: usize| {
        let mut l = FIVE_LINES.to_vec();
        while l.len() < n - 1 {
            l.push("");
        }
        l.push("?>");
        l
    };
    let p = d.file("14.php", &script(&lines(14)));
    let f = open(&mut t, &p);
    m(&mut t, &f, "seek", vec![Value::Int(13)]);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "?>\n");
    m(&mut t, &f, "next", vec![]);
    assert!(is_false(&m(&mut t, &f, "current", vec![])));
    assert!(is_false(&m(&mut t, &f, "valid", vec![])));

    let p = d.file("16.php", &script(&lines(16)));
    let f = open(&mut t, &p);
    m(&mut t, &f, "seek", vec![Value::Int(15)]);
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "?>\n");
    m(&mut t, &f, "next", vec![]);
    assert!(is_false(&m(&mut t, &f, "current", vec![])));
    assert!(is_false(&m(&mut t, &f, "valid", vec![])));
    m(&mut t, &f, "rewind", vec![]);
    assert!(is_true(&m(&mut t, &f, "valid", vec![])));
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "<?php\n");

    let p = d.file("12.php", &script(&lines(12)));
    for pos in [13, 120] {
        let f = open(&mut t, &p);
        m(&mut t, &f, "seek", vec![Value::Int(pos)]);
        m(&mut t, &f, "next", vec![]);
        assert_eq!(int(&m(&mut t, &f, "key", vec![])), 12);
        assert!(is_false(&m(&mut t, &f, "valid", vec![])));
    }

    let p = d.file("11.php", &script(&lines(11)));
    let f = open(&mut t, &p);
    m(&mut t, &f, "seek", vec![Value::Int(20)]);
    assert!(is_false(&m(&mut t, &f, "current", vec![])));

    // SplFileObject_seek_error_001.phpt
    let (class, msg) = thrown(try_m(&mut t, &f, "seek", vec![Value::Int(-1)]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::seek(): Argument #1 ($line) must be greater than or equal to 0"
    );
}

#[test]
fn key_after_seek_and_fgets() {
    // SplFileObject_key_fgets_and_seek.phpt
    let mut t = TestVm::new();
    let f = temp_lines(&mut t, 100);
    m(&mut t, &f, "seek", vec![Value::Int(50)]);
    for i in 50..53 {
        assert_eq!(int(&m(&mut t, &f, "key", vec![])), i);
        assert_eq!(trim(&m(&mut t, &f, "fgets", vec![])), format!("line {i}"));
    }
}

#[test]
fn key_and_fgets_stay_in_sync() {
    // gh8273.phpt
    let mut t = TestVm::new();
    let f = temp_lines(&mut t, 5);
    m(&mut t, &f, "rewind", vec![]);
    let mut out = String::new();
    while is_true(&m(&mut t, &f, "valid", vec![])) {
        let k = int(&m(&mut t, &f, "key", vec![]));
        out.push_str(&format!("{k}: {}", str_of(&m(&mut t, &f, "fgets", vec![]))));
    }
    assert_eq!(
        out,
        "0: line 0\n1: line 1\n2: line 2\n3: line 3\n4: line 4\n"
    );

    // gh8561.phpt
    let kc = |t: &mut TestVm| {
        format!(
            "key={} current={}",
            int(&m(t, &f, "key", vec![])),
            trim(&m(t, &f, "current", vec![]))
        )
    };
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "fgets", vec![]);
    assert_eq!(kc(&mut t), "key=1 current=line 1");
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "fgets", vec![]);
    m(&mut t, &f, "fgets", vec![]);
    assert_eq!(kc(&mut t), "key=2 current=line 2");
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "current", vec![]);
    m(&mut t, &f, "fgets", vec![]);
    assert_eq!(kc(&mut t), "key=1 current=line 1");

    // gh8562.phpt
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "next", vec![]);
    assert_eq!(kc(&mut t), "key=1 current=line 1");
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "next", vec![]);
    m(&mut t, &f, "next", vec![]);
    assert_eq!(kc(&mut t), "key=2 current=line 2");
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "current", vec![]);
    m(&mut t, &f, "next", vec![]);
    assert_eq!(kc(&mut t), "key=1 current=line 1");
}

#[test]
fn next_and_seek_stop_at_the_end() {
    // gh8563.phpt, gh8564.phpt
    let d = TempDir::new("gh8563");
    let mut t = TestVm::new();
    let f1 = new(
        &mut t,
        "SplFileObject",
        vec![s(&d.path("gh8563.txt")), s("w+")],
    );
    let f2 = new(&mut t, "SplTempFileObject", vec![]);
    for i in 0..5 {
        m(&mut t, &f1, "fwrite", vec![s(&format!("line {i}\n"))]);
        m(&mut t, &f2, "fwrite", vec![s(&format!("line {i}\n"))]);
    }
    for f in [&f1, &f2] {
        m(&mut t, f, "rewind", vec![]);
        m(&mut t, f, "seek", vec![Value::Int(10)]);
        assert_eq!(int(&m(&mut t, f, "key", vec![])), 5);
    }
    m(&mut t, &f2, "rewind", vec![]);
    for _ in 0..10 {
        m(&mut t, &f2, "next", vec![]);
    }
    assert_eq!(int(&m(&mut t, &f2, "key", vec![])), 5);
    assert!(is_false(&m(&mut t, &f2, "valid", vec![])));
    m(&mut t, &f2, "rewind", vec![]);
    m(&mut t, &f2, "seek", vec![Value::Int(10)]);
    assert_eq!(int(&m(&mut t, &f2, "key", vec![])), 5);
    assert!(is_false(&m(&mut t, &f2, "valid", vec![])));
}

#[test]
fn fgets_until_eof() {
    // gh21742.phpt, bug46053.phpt
    let d = TempDir::new("gh21742");
    let p = d.file("a", "Line 0\nLine 1\nLine 2\nLine 3\nLine 4\n");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    let mut out = String::new();
    while !is_true(&m(&mut t, &f, "eof", vec![])) {
        out.push_str(&str_of(&m(&mut t, &f, "fgets", vec![])));
    }
    assert_eq!(out, "Line 0\nLine 1\nLine 2\nLine 3\nLine 4\n");
    let p = d.file("empty", "");
    let f = open(&mut t, &p);
    let mut iters = 0;
    while !is_true(&m(&mut t, &f, "eof", vec![])) {
        iters += 1;
        m(&mut t, &f, "fgets", vec![]);
        if iters > 3 {
            break;
        }
    }
    assert_eq!(iters, 1);

    let p = d.file(
        "bug46053.php",
        "<?php\n\n$x = new splfileobject(__FILE__);\n?>\n",
    );
    let f = open(&mut t, &p);
    m(&mut t, &f, "getPathName", vec![]);
    m(&mut t, &f, "seek", vec![Value::Int(10)]);
    m(&mut t, &f, "seek", vec![Value::Int(0)]);
    assert_eq!(trim(&m(&mut t, &f, "fgets", vec![])), "<?php");
}

#[test]
fn skip_empty_seek() {
    // bug62004.phpt
    let d = TempDir::new("bug62004");
    let p = d.file("bug62004.txt", &fixture("bug62004.txt"));
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setFlags", vec![Value::Int(SKIP_EMPTY)]);
    for (i, line) in ["Line 1\n", "Line 2\n", "Line 3\n"].iter().enumerate() {
        m(&mut t, &f, "seek", vec![Value::Int(i as i64)]);
        assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), *line);
    }
}

#[test]
fn seek_keys_with_flags() {
    // gh8121.phpt
    let d = TempDir::new("gh8121");
    let p = d.file("gh8121.csv", &fixture("gh8121.csv"));
    let mut t = TestVm::new();
    for flags in [
        READ_AHEAD | READ_CSV | SKIP_EMPTY | DROP_NEW_LINE,
        READ_AHEAD | SKIP_EMPTY | DROP_NEW_LINE,
        SKIP_EMPTY | DROP_NEW_LINE,
    ] {
        let f = new(&mut t, "SplFileObject", vec![s(&p), s("r")]);
        m(&mut t, &f, "setFlags", vec![Value::Int(flags)]);
        for i in 0..4 {
            m(&mut t, &f, "seek", vec![Value::Int(i)]);
            assert_eq!(int(&m(&mut t, &f, "key", vec![])), i, "flags {flags}");
        }
    }
}

#[test]
fn fgetc_counts_lines() {
    // fileobject_002.phpt
    let d = TempDir::new("fgetc");
    for (name, contents, last_key) in [
        ("fileobject_001a.txt", "0\n1\n2\n3\n4\n5\n", 6),
        ("fileobject_001b.txt", "0\n1\n2\n3\n4\n5", 5),
    ] {
        let p = d.file(name, contents);
        let mut t = TestVm::new();
        let f = open(&mut t, &p);
        assert_eq!(int(&m(&mut t, &f, "key", vec![])), 0);
        let mut seen = Vec::new();
        loop {
            let c = m(&mut t, &f, "fgetc", vec![]);
            if is_false(&c) {
                break;
            }
            let key = int(&m(&mut t, &f, "key", vec![]));
            assert!(is_false(&m(&mut t, &f, "eof", vec![])));
            seen.push((key, str_of(&c)));
        }
        let expected: Vec<(i64, String)> = contents
            .chars()
            .scan(0, |line, c| {
                if c == '\n' {
                    *line += 1;
                }
                Some((*line, c.to_string()))
            })
            .collect();
        assert_eq!(seen, expected);
        assert!(is_true(&m(&mut t, &f, "eof", vec![])));
        assert_eq!(int(&m(&mut t, &f, "key", vec![])), last_key);
    }
}

#[test]
fn constructor_errors() {
    // bug54292.phpt, gh16477.phpt, gh16477-2.phpt, gh16604_2.phpt
    let d = TempDir::new("ctor");
    let p = d.file("gh16604_2.tmp", "hello");
    let mut t = TestVm::new();
    let (class, msg) = thrown(try_new(
        &mut t,
        "SplFileObject",
        vec![s("foo"), Value::empty_array()],
    ));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplFileObject::__construct(): Argument #2 ($mode) must be of type string, array given"
    );
    let f = open(&mut t, &p);
    let (class, msg) = thrown(try_m(&mut t, &f, "__construct", vec![]));
    assert_eq!(class, "ArgumentCountError");
    assert_eq!(
        msg,
        "SplFileObject::__construct() expects at least 1 argument, 0 given"
    );
    m(&mut t, &f, "__debugInfo", vec![]);
    let (class, msg) = thrown(try_m(&mut t, &f, "__construct", vec![s(&p)]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("Error", "Cannot call constructor twice")
    );
    let tf = new(&mut t, "SplTempFileObject", vec![]);
    let (_, msg) = thrown(try_m(&mut t, &tf, "__construct", vec![]));
    assert_eq!(msg, "Cannot call constructor twice");
    m(&mut t, &tf, "__debugInfo", vec![]);
    let (class, msg) = thrown(try_new(&mut t, "SplFileObject", vec![s("a\0b")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::__construct(): Argument #1 ($filename) must not contain any null bytes"
    );
    // SplTempFileObject_constructor_error.phpt
    let (class, msg) = thrown(try_new(&mut t, "SplTempFileObject", vec![s("invalid")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplTempFileObject::__construct(): Argument #1 ($maxMemory) must be of type int, string given"
    );
}

#[test]
fn open_failures() {
    // bug55701.phpt (SplFileObject non-existent file), fileobject_003.phpt
    let d = TempDir::new("openfail");
    let missing = d.path("abcdefghij.abcdefghij");
    let mut t = TestVm::new();
    let (class, msg) = thrown(try_new(&mut t, "SplFileObject", vec![s(&missing)]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(
        msg,
        "SplFileObject::__construct(): Failed to open stream: No such file or directory"
    );
    assert_eq!(t.output(), "");
    for dir in [d.root(), format!("{}/", d.root())] {
        let (class, msg) = thrown(try_new(&mut t, "SplFileObject", vec![s(&dir)]));
        assert_eq!(class, "LogicException");
        assert_eq!(msg, "Cannot use SplFileObject with directories");
        let info = new(&mut t, "SplFileInfo", vec![s(&dir)]);
        let (class, msg) = thrown(try_m(&mut t, &info, "openFile", vec![]));
        assert_eq!(class, "LogicException");
        assert_eq!(msg, "Cannot use SplFileObject with directories");
    }
    let info = new(&mut t, "SplFileInfo", vec![s(&missing)]);
    let (class, msg) = thrown(try_m(&mut t, &info, "openFile", vec![s("r")]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(
        msg,
        "SplFileInfo::openFile(): Failed to open stream: No such file or directory"
    );
    let (class, msg) = thrown(try_new(&mut t, "SplFileObject", vec![s("")]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("ValueError", "Path must not be empty")
    );
    assert!(matches!(
        t.vm.errors.mode,
        php_runtime::errors::ErrorHandlingMode::Normal
    ));
}

#[test]
fn paths_of_file_objects() {
    // fileobject_003.phpt, bug64782.phpt, fileobject_getbasename_basic.phpt
    let d = TempDir::new("paths");
    let p = d.file("fileobject_001a.txt", "0\n");
    let mut t = TestVm::new();
    let f = new(
        &mut t,
        "SplFileObject",
        vec![s(&p), s("r"), Value::Bool(false), Value::Null],
    );
    assert_eq!(str_of(&m(&mut t, &f, "getPathName", vec![])), p);
    assert_eq!(
        str_of(&m(&mut t, &f, "getFileName", vec![])),
        "fileobject_001a.txt"
    );
    assert_eq!(str_of(&m(&mut t, &f, "getPath", vec![])), d.root());
    assert_eq!(
        str_of(&m(&mut t, &f, "getBasename", vec![])),
        "fileobject_001a.txt"
    );
    assert_eq!(
        debug_strs(&mut t, &f),
        pairs(&[
            ("pathName:SplFileInfo", &p),
            ("fileName:SplFileInfo", "fileobject_001a.txt"),
            ("openMode:SplFileObject", "r"),
            ("delimiter:SplFileObject", ","),
            ("enclosure:SplFileObject", "\""),
        ])
    );
    let info = new(&mut t, "SplFileInfo", vec![s(&p)]);
    let fo = obj_of(&m(&mut t, &info, "openFile", vec![]));
    assert_eq!(str_of(&m(&mut t, &fo, "getPathName", vec![])), p);
    assert_eq!(
        str_of(&m(&mut t, &fo, "getFileName", vec![])),
        "fileobject_001a.txt"
    );
    assert_eq!(str_of(&m(&mut t, &fo, "getPath", vec![])), d.root());
    // fileobject_checktype_basic.phpt
    assert!(is_true(&m(&mut t, &f, "isFile", vec![])));
    assert!(is_false(&m(&mut t, &f, "isDir", vec![])));
    assert!(is_false(&m(&mut t, &f, "isLink", vec![])));
}

#[test]
fn include_path() {
    // fileobject_004.phpt
    let d = TempDir::new("incpath");
    let p = d.file("fileobject_004.phpt", "x");
    let mut t = TestVm::new();
    t.vm.ini_set("include_path", d.root());
    let f = new(
        &mut t,
        "SplFileObject",
        vec![s("fileobject_004.phpt"), s("r"), Value::Bool(true)],
    );
    assert_eq!(str_of(&m(&mut t, &f, "getPath", vec![])), d.root());
    assert_eq!(
        str_of(&m(&mut t, &f, "getFilename", vec![])),
        "fileobject_004.phpt"
    );
    assert_eq!(str_of(&m(&mut t, &f, "getRealPath", vec![])), p);
}

#[test]
fn temp_file_objects() {
    // SplTempFileObject_constructor_basic.phpt, _maxmemory_basic.phpt,
    // _memory_lt1_variation.phpt
    let mut t = TestVm::new();
    for (args, name) in [
        (vec![], "php://temp"),
        (vec![Value::Int(1024)], "php://temp/maxmemory:1024"),
        (vec![Value::Int(-1)], "php://memory"),
    ] {
        let f = new(&mut t, "SplTempFileObject", args);
        assert_eq!(
            debug_strs(&mut t, &f),
            pairs(&[
                ("pathName:SplFileInfo", name),
                ("fileName:SplFileInfo", name),
                ("openMode:SplFileObject", "wb"),
                ("delimiter:SplFileObject", ","),
                ("enclosure:SplFileObject", "\""),
            ])
        );
        assert_eq!(str_of(&m(&mut t, &f, "getPath", vec![])), "");
        m(&mut t, &f, "fwrite", vec![s("abc\ndef")]);
        m(&mut t, &f, "rewind", vec![]);
        assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), "abc\n");
        assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), "def");
    }
    // gh17516.phpt
    user_class(&mut t.vm, "SplFileInfoChild", "SplFileInfo", &[]);
    user_class(&mut t.vm, "BadSplFileInfo", "stdClass", &[]);
    let cls = new(&mut t, "SplTempFileObject", vec![]);
    let pi = obj_of(&m(&mut t, &cls, "getPathInfo", vec![s("SplFileInfoChild")]));
    assert_eq!(pi.class().name.to_string(), "SplFileInfoChild");
    assert_eq!(
        debug_strs(&mut t, &pi),
        pairs(&[
            ("pathName:SplFileInfo", "php:"),
            ("fileName:SplFileInfo", "php:")
        ])
    );
    let (class, msg) = thrown(try_m(
        &mut t,
        &cls,
        "getPathInfo",
        vec![s("BadSplFileInfo")],
    ));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplFileInfo::getPathInfo(): Argument #1 ($class) must be a class name derived from SplFileInfo or null, BadSplFileInfo given"
    );
    // gh9883-extra.phpt
    let f = new(&mut t, "SplTempFileObject", vec![]);
    for _ in 0..3 {
        assert_eq!(str_of(&m(&mut t, &f, "__toString", vec![])), "");
    }
}

#[test]
fn to_string_does_not_advance() {
    // gh9883.phpt
    let d = TempDir::new("gh9883");
    let p = d.file(
        "gh9883.php",
        "<?php\n$file_stream = 1;\necho $file_stream;\n",
    );
    let mut t = TestVm::new();
    let f = new(&mut t, "SplFileObject", vec![s(&p), s("rb")]);
    for _ in 0..6 {
        assert_eq!(str_of(&m(&mut t, &f, "__toString", vec![])), "<?php\n");
    }
}

#[test]
fn read_write_and_positions() {
    // bug65545.phpt, SplFileObject_fwrite_variation_00{1,2}.phpt,
    // fileobject_005.phpt, SplFileObject_fflush_basic_001.phpt
    let d = TempDir::new("rw");
    let mut t = TestVm::new();
    let contents = "<?php\n$obj = new SplFileObject(__FILE__, 'r');\n";
    let p = d.file("bug65545.php", contents);
    let f = new(&mut t, "SplFileObject", vec![s(&p), s("r")]);
    assert_eq!(
        str_of(&m(&mut t, &f, "fread", vec![Value::Int(5)])),
        "<?php"
    );
    let (class, msg) = thrown(try_m(&mut t, &f, "fread", vec![Value::Int(0)]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::fread(): Argument #1 ($length) must be greater than 0"
    );
    assert_eq!(int(&m(&mut t, &f, "ftell", vec![])), 5);
    let rest = str_of(&m(
        &mut t,
        &f,
        "fread",
        vec![Value::Int(contents.len() as i64 + 32)],
    ));
    assert_eq!(rest.len(), contents.len() - 5);
    // php_stream_read_to_str() gives "" at EOF
    assert_eq!(str_of(&m(&mut t, &f, "fread", vec![Value::Int(1)])), "");
    assert_eq!(int(&m(&mut t, &f, "fseek", vec![Value::Int(1)])), 0);
    assert_eq!(str_of(&m(&mut t, &f, "fgetc", vec![])), "?");
    assert!(is_true(&m(&mut t, &f, "fflush", vec![])));

    for (len, expected) in [(4, "test"), (12, "test_write")] {
        let p = d.path(&format!("fwrite{len}.txt"));
        let o = new(&mut t, "SplFileObject", vec![s(&p), s("w")]);
        m(&mut t, &o, "fwrite", vec![s("test_write"), Value::Int(len)]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), expected);
    }
    let p = d.path("neg.txt");
    let o = new(&mut t, "SplFileObject", vec![s(&p), s("w")]);
    assert_eq!(
        int(&m(&mut t, &o, "fwrite", vec![s("abc"), Value::Int(-1)])),
        0
    );
    assert_eq!(int(&m(&mut t, &o, "fwrite", vec![s("")])), 0);

    let p = d.file("fileobject_005.txt", "");
    let fo = new(
        &mut t,
        "SplFileObject",
        vec![s(&p), s("w+"), Value::Bool(true)],
    );
    m(&mut t, &fo, "fwrite", vec![s("blahlubba")]);
    assert!(is_true(&m(&mut t, &fo, "ftruncate", vec![Value::Int(4)])));
    m(&mut t, &fo, "rewind", vec![]);
    assert_eq!(str_of(&m(&mut t, &fo, "fgets", vec![])), "blah");
    m(&mut t, &fo, "rewind", vec![]);
    m(&mut t, &fo, "fwrite", vec![s("blahlubba")]);
    // gh17463.phpt
    let (class, msg) = thrown(try_m(&mut t, &fo, "ftruncate", vec![Value::Int(-1)]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::ftruncate(): Argument #1 ($size) must be greater than or equal to 0"
    );
    // SplFileObject_ftruncate_error_001.phpt (a stream without truncation)
    let out = new(&mut t, "SplFileObject", vec![s("php://output"), s("w")]);
    let (class, msg) = thrown(try_m(&mut t, &out, "ftruncate", vec![Value::Int(1)]));
    assert_eq!(class, "LogicException");
    assert_eq!(msg, "Can't truncate file php://output");
}

#[test]
fn passthru_stat_and_lock() {
    // SplFileObject_fpassthru_basic.phpt, SplFileObject_fstat_*.phpt
    let d = TempDir::new("passthru");
    let input = fixture("SplFileObject_testinput.txt");
    let p = d.file("SplFileObject_testinput.txt", &input);
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    assert_eq!(int(&m(&mut t, &f, "fpassthru", vec![])), input.len() as i64);
    assert_eq!(t.output(), input);
    let st = m(&mut t, &f, "fstat", vec![]);
    let e = entries(&st);
    assert_eq!(e.len(), 26);
    assert_eq!(e[13].0, "dev");
    assert!(
        e.iter()
            .any(|(k, v)| k == "size" && v.identical(&Value::Int(input.len() as i64)))
    );
    let wb = PhpRef::new(Value::Null);
    assert!(is_true(&m(
        &mut t,
        &f,
        "flock",
        vec![Value::Int(2), Value::Ref(wb.clone())]
    )));
    assert!(wb.get().identical(&Value::Int(0)));
    assert!(is_true(&m(&mut t, &f, "flock", vec![Value::Int(3)])));
    let (class, msg) = thrown(try_m(&mut t, &f, "flock", vec![Value::Int(0)]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::flock(): Argument #1 ($operation) must be one of LOCK_SH, LOCK_EX, or LOCK_UN"
    );
}

#[test]
fn fscanf() {
    // SplFileObject_fscanf_basic.phpt, bug52573.phpt
    let d = TempDir::new("fscanf");
    let p = d.file("in.txt", &fixture("SplFileObject_testinput.txt"));
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    let r = m(&mut t, &f, "fscanf", vec![s("%s")]);
    let e = entries(&r);
    assert_eq!(e.len(), 1);
    assert_eq!(str_of(&e[0].1), "first,second,third");
    let p = d.file("bug52573.php", "<?php // test\n\n$result = null;\n");
    let f = new(&mut t, "SplFileObject", vec![s(&p), s("r")]);
    let result = PhpRef::new(Value::Null);
    let r = m(
        &mut t,
        &f,
        "fscanf",
        vec![s("<?php // %s"), Value::Ref(result.clone())],
    );
    assert!(r.identical(&Value::Int(1)));
    assert_eq!(str_of(&result.get()), "test");
    let r = m(&mut t, &f, "fscanf", vec![s("<?php // %s")]);
    let e = entries(&r);
    assert_eq!(e.len(), 1);
    assert!(e[0].1.is_null());
    // at the end, fscanf() is not silent
    m(&mut t, &f, "fscanf", vec![s("%s")]);
    let (class, msg) = thrown(try_m(&mut t, &f, "fscanf", vec![s("%s")]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(msg, format!("Cannot read from file {p}"));
}

#[test]
fn max_line_len() {
    // bug67805.phpt, fileobject_setmaxlinelen_*.phpt, fileobject_getmaxlinelen_basic.phpt
    let d = TempDir::new("maxlen");
    let p = d.file(
        "x.php",
        "<?php\n$splFileObject = new SplFileObject(__FILE__);\n",
    );
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setMaxLineLen", vec![Value::Int(3)]);
    assert_eq!(int(&m(&mut t, &f, "getMaxLineLen", vec![])), 3);
    assert_eq!(str_of(&m(&mut t, &f, "getCurrentLine", vec![])), "<?p");
    assert_eq!(str_of(&m(&mut t, &f, "current", vec![])), "hp\n");
    m(&mut t, &f, "setMaxLineLen", vec![Value::Int(7)]);
    assert_eq!(int(&m(&mut t, &f, "getMaxLineLen", vec![])), 7);
    let (class, msg) = thrown(try_m(&mut t, &f, "setMaxLineLen", vec![Value::Int(-1)]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileObject::setMaxLineLen(): Argument #1 ($maxLength) must be greater than or equal to 0"
    );
}

#[test]
fn get_current_line_after_seek() {
    // fileobject_getcurrentline_basic.phpt
    let d = TempDir::new("curline");
    let p = five_lines_and(&d, &["$s->seek(1);", "?>"]);
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "seek", vec![Value::Int(1)]);
    assert_eq!(
        str_of(&m(&mut t, &f, "getCurrentLine", vec![])),
        "//line 2\n"
    );
    assert_eq!(
        str_of(&m(&mut t, &f, "getCurrentLine", vec![])),
        "//line 3\n"
    );
}

fn array_line(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(list(vec![Value::Int(1), Value::Int(2), Value::Int(3)]))
}

static ARRAY_LINE: NativeFunction = NativeFunction {
    sig: "public function getCurrentLine(): array {}",
    handler: array_line,
};

fn upper_line(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // return strtoupper(parent::getCurrentLine())
    let this = args.this().cloned().unwrap();
    let f = vm
        .find_class(b"SplFileObject")
        .unwrap()
        .find_method(b"fgets")
        .cloned()
        .unwrap();
    let line = vm.call_function(
        &f,
        php_runtime::CallArgs {
            this: Some(this),
            ..php_runtime::CallArgs::new(Vec::new())
        },
    )?;
    vm.call_native("strtoupper", vec![line])
}

static UPPER_LINE: NativeFunction = NativeFunction {
    sig: "public function getCurrentLine(): string {}",
    handler: upper_line,
};

#[test]
fn overridden_get_current_line() {
    // SplFileObject_getCurrentLine_invalid_override.phpt
    let d = TempDir::new("override");
    let p = d.file("a.txt", "one\ntwo\n");
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MySplFileObject",
        "SplFileObject",
        &[&ARRAY_LINE],
    );
    let o = new(&mut t, "MySplFileObject", vec![s(&p)]);
    let (class, msg) = thrown(try_m(&mut t, &o, "current", vec![]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "MySplFileObject::getCurrentLine(): Return value must be of type string, array returned"
    );
    // a valid override supplies the lines of current()
    user_class(&mut t.vm, "UpperFile", "SplFileObject", &[&UPPER_LINE]);
    let o = new(&mut t, "UpperFile", vec![s(&p)]);
    assert_eq!(str_of(&m(&mut t, &o, "current", vec![])), "ONE\n");
    m(&mut t, &o, "next", vec![]);
    assert_eq!(str_of(&m(&mut t, &o, "current", vec![])), "TWO\n");
    // parent::getCurrentLine() is fgets(), which counts its lines too
    assert_eq!(int(&m(&mut t, &o, "key", vec![])), 3);
}

#[test]
fn flags_and_children() {
    // SplFileObject_getflags_*.phpt, _haschildren_basic.phpt, _getchildren_basic.phpt
    let d = TempDir::new("getflags");
    let p = d.file("x.csv", "eerste;tweede;derde");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    for flag in [DROP_NEW_LINE, READ_AHEAD, SKIP_EMPTY, READ_CSV] {
        m(&mut t, &f, "setFlags", vec![Value::Int(flag)]);
        assert_eq!(int(&m(&mut t, &f, "getFlags", vec![])), flag);
    }
    m(&mut t, &f, "setFlags", vec![Value::Int(-1)]);
    assert_eq!(int(&m(&mut t, &f, "getFlags", vec![])), 15);
    assert!(is_false(&m(&mut t, &f, "hasChildren", vec![])));
    assert!(m(&mut t, &f, "getChildren", vec![]).is_null());
}

#[test]
fn drop_new_line_keeps_nul_and_cr() {
    // bug80933.phpt
    let mut t = TestVm::new();
    for line in [
        "Lorem ipsum \0 dolor sit amet",
        "Lorem ipsum \r dolor sit amet",
    ] {
        let f = new(&mut t, "SplTempFileObject", vec![]);
        m(&mut t, &f, "fwrite", vec![s(line)]);
        m(&mut t, &f, "rewind", vec![]);
        assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), line);
        m(&mut t, &f, "rewind", vec![]);
        m(&mut t, &f, "setFlags", vec![Value::Int(DROP_NEW_LINE)]);
        assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), line);
    }
    let f = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &f, "fwrite", vec![s("a\r\nb\n")]);
    m(&mut t, &f, "setFlags", vec![Value::Int(DROP_NEW_LINE)]);
    m(&mut t, &f, "rewind", vec![]);
    assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), "a");
    assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), "b");
}

#[test]
fn read_ahead_iteration() {
    // bug81477.phpt (LimitIterator(1, -1) seeks to 1, then iterates)
    let d = TempDir::new("bug81477");
    let p = d.file("bug81477.csv", "foo,bar\nbaz,bat\nmore,data\n");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setFlags", vec![Value::Int(READ_AHEAD)]);
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "seek", vec![Value::Int(1)]);
    let mut rows = Vec::new();
    while is_true(&m(&mut t, &f, "valid", vec![])) {
        rows.push(str_of(&m(&mut t, &f, "current", vec![])));
        m(&mut t, &f, "next", vec![]);
    }
    assert_eq!(rows, ["baz,bat\n", "more,data\n"]);
}

#[test]
fn uncloneable_and_closed_streams() {
    // bug72888.phpt, bug79710.phpt, bug81691.phpt, gh16217.phpt
    let d = TempDir::new("closed");
    let p = d.file("bug81691.php", "<?php\n$obj = 1;\n");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    let (class, msg) =
        thrown((f.class().handlers.clone_obj.unwrap())(&mut t.vm, &f).map(Value::Object));
    assert_eq!(class, "Error");
    assert_eq!(
        msg,
        "Trying to clone an uncloneable object of class SplFileObject"
    );
    // fclose() of the object's stream is refused
    let res = crate::spl::directory::with_fs(&f, |st| st.stream.clone()).unwrap();
    assert!(is_false(
        &t.call("fclose", vec![Value::Resource(res)]).unwrap()
    ));
    assert_eq!(
        t.output(),
        "\nWarning: fclose(): cannot close the provided stream, as it must not be manually closed in /t.php on line 3\n"
    );
    assert_eq!(str_of(&m(&mut t, &f, "fgets", vec![])), "<?php\n");
    // the destructor closes the stream; the object stays usable for errors
    let tf = new(&mut t, "SplTempFileObject", vec![]);
    (tf.class().handlers.dtor_obj.unwrap())(&mut t.vm, &tf).unwrap();
    let (class, msg) = thrown(try_m(&mut t, &tf, "flock", vec![Value::Int(2)]));
    assert_eq!(
        (class.as_str(), msg.as_str()),
        ("Error", "Object not initialized")
    );
    crate::spl::directory::get_method_check(&mut t.vm, &tf).unwrap();
    // uninitialized objects
    user_class(&mut t.vm, "Uninit", "SplFileObject", &[]);
    let ce = t.vm.find_class(b"Uninit").unwrap();
    let u = t.vm.instantiate(&ce).unwrap();
    for method in [
        "next",
        "current",
        "fgets",
        "eof",
        "rewind",
        "__toString",
        "fflush",
    ] {
        let (class, msg) = thrown(try_m(&mut t, &u, method, vec![]));
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Error", "Object not initialized"),
            "{method}"
        );
    }
    m(&mut t, &u, "setFlags", vec![Value::Int(READ_AHEAD)]);
    let (_, msg) = thrown(try_m(&mut t, &u, "next", vec![]));
    assert_eq!(msg, "Object not initialized");
    let (_, msg) = thrown(try_m(&mut t, &u, "fputcsv", vec![Value::empty_array()]));
    assert_eq!(msg, "Object not initialized");
    assert_eq!(t.output(), deprecated("fputcsv"));
    assert!(is_false(&m(&mut t, &u, "valid", vec![])));
    assert_eq!(int(&m(&mut t, &u, "key", vec![])), 0);
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

fn row(v: &Value) -> Vec<String> {
    entries(v).into_iter().map(|(_, v)| str_of(&v)).collect()
}

#[test]
fn fgetcsv_and_csv_control() {
    // SplFileObject_fgetcsv_basic.phpt, _getCsvControl_basic_001.phpt,
    // _fgetcsv_escape_default.phpt
    let d = TempDir::new("csv");
    let p = d.file("a.csv", "field1,field2,field3,5\n");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    let ctl = m(&mut t, &f, "getCsvControl", vec![]);
    assert_eq!(row(&ctl), [",", "\"", "\\"]);
    assert_eq!(t.output(), deprecated("getCsvControl"));
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    let r = m(&mut t, &f, "fgetcsv", vec![]);
    assert_eq!(row(&r), ["field1", "field2", "field3", "5"]);
    assert_eq!(
        t.output(),
        format!("{}{}", deprecated("setCsvControl"), deprecated("fgetcsv"))
    );
    let ctl = m(&mut t, &f, "getCsvControl", vec![]);
    assert_eq!(row(&ctl), [",", "\"", ""]);
    t.output();

    let p = d.file("b.csv", r#""aa\"","bb","\"c""#);
    let f = open(&mut t, &p);
    let r = m(&mut t, &f, "fgetcsv", vec![]);
    assert_eq!(row(&r), [r#"aa\""#, "bb", r#"\"c"#]);
    assert_eq!(
        t.output(),
        format!(
            "{}\nDeprecated: SplFileObject::fgetcsv(): the $escape parameter must be provided, as its default value will change, either explicitly or via SplFileObject::setCsvControl() in /t.php on line 3\n",
            deprecated("fgetcsv")
        )
    );
}

#[test]
fn csv_argument_errors() {
    // SplFileObject_fgetcsv_{delimiter,enclosure,escape}_error.phpt,
    // SplFileObject_setCsvControl_error00{1,2,3}.phpt
    let d = TempDir::new("csverr");
    let p = d.file("a.csv", "'groene appelen'|10\n");
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    for (method, args, msg) in [
        (
            "fgetcsv",
            vec![s("invalid")],
            "SplFileObject::fgetcsv(): Argument #1 ($separator) must be a single character",
        ),
        (
            "fgetcsv",
            vec![s(","), s("invalid")],
            "SplFileObject::fgetcsv(): Argument #2 ($enclosure) must be a single character",
        ),
        (
            "fgetcsv",
            vec![s(","), s("\""), s("invalid")],
            "SplFileObject::fgetcsv(): Argument #3 ($escape) must be empty or a single character",
        ),
        (
            "setCsvControl",
            vec![s("||")],
            "SplFileObject::setCsvControl(): Argument #1 ($separator) must be a single character",
        ),
        (
            "setCsvControl",
            vec![s("|"), s("''")],
            "SplFileObject::setCsvControl(): Argument #2 ($enclosure) must be a single character",
        ),
        (
            "setCsvControl",
            vec![s("|"), s("'"), s("three")],
            "SplFileObject::setCsvControl(): Argument #3 ($escape) must be empty or a single character",
        ),
        (
            "fputcsv",
            vec![Value::empty_array(), s("ab")],
            "SplFileObject::fputcsv(): Argument #2 ($separator) must be a single character",
        ),
    ] {
        let (class, m2) = thrown(try_m(&mut t, &f, method, args));
        assert_eq!(class, "ValueError");
        assert_eq!(m2, msg);
        assert_eq!(t.output(), deprecated(method));
    }
    // setCsvControl() without $escape
    m(&mut t, &f, "setCsvControl", vec![s("|")]);
    assert_eq!(
        t.output(),
        format!(
            "{}\nDeprecated: SplFileObject::setCsvControl(): the $escape parameter must be provided as its default value will change in /t.php on line 3\n",
            deprecated("setCsvControl")
        )
    );
}

#[test]
fn read_csv_iteration() {
    // SplFileObject_setCsvControl_basic.phpt
    let d = TempDir::new("readcsv");
    let p = d.file(
        "csv_control_data_basic.csv",
        "'groene appelen'|10\n'gele bananen'|20\n'rode kersen'|30",
    );
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setFlags", vec![Value::Int(READ_CSV)]);
    m(&mut t, &f, "setCsvControl", vec![s("|"), s("'"), s("")]);
    t.output();
    let mut out = Vec::new();
    m(&mut t, &f, "rewind", vec![]);
    while is_true(&m(&mut t, &f, "valid", vec![])) {
        let r = row(&m(&mut t, &f, "current", vec![]));
        out.push(format!("{} : {}", r[0], r[1]));
        m(&mut t, &f, "next", vec![]);
    }
    assert_eq!(
        out,
        [
            "groene appelen : 10",
            "gele bananen : 20",
            "rode kersen : 30"
        ]
    );
    // no deprecation for the internal reads
    assert_eq!(t.output(), "");
}

#[test]
fn csv_with_skip_empty_and_drop_new_line() {
    // bug69181.phpt
    let d = TempDir::new("bug69181");
    let p = d.file(
        "bug69181.csv",
        "\"foo\n\nbar\nbaz\",qux\n\n\"foo\nbar\nbaz\",qux",
    );
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    m(
        &mut t,
        &f,
        "setFlags",
        vec![Value::Int(SKIP_EMPTY | DROP_NEW_LINE | READ_CSV)],
    );
    let arr = t
        .call("iterator_to_array", vec![Value::Object(f.clone())])
        .unwrap();
    let e = entries(&arr);
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].0, "0");
    assert_eq!(row(&e[0].1), ["foo\n\nbar\nbaz", "qux"]);
    assert_eq!(e[1].0, "2");
    assert_eq!(row(&e[1].1), ["foo\nbar\nbaz", "qux"]);
    m(&mut t, &f, "rewind", vec![]);
    let mut records = Vec::new();
    loop {
        let r = m(&mut t, &f, "fgetcsv", vec![]);
        if is_false(&r) {
            break;
        }
        records.push(row(&r));
    }
    assert_eq!(
        records,
        [vec!["foo\n\nbar\nbaz", "qux"], vec!["foo\nbar\nbaz", "qux"]]
    );
}

#[test]
fn csv_after_seek_and_blank_files() {
    // bug46569.phpt, fgetcsv_blank_file.phpt, bug75917.phpt, bug78976.phpt
    let d = TempDir::new("csvseek");
    let p = d.file("bug46569.csv", &fixture("csv/bug46569.csv"));
    let mut t = TestVm::new();
    let f = open(&mut t, &p);
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    m(&mut t, &f, "seek", vec![Value::Int(1)]);
    assert_eq!(row(&m(&mut t, &f, "fgetcsv", vec![])), ["second", "line"]);

    let f = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    m(&mut t, &f, "fwrite", vec![s("")]);
    m(&mut t, &f, "rewind", vec![]);
    assert!(is_false(&m(&mut t, &f, "fgetcsv", vec![])));
    m(&mut t, &f, "setFlags", vec![Value::Int(SKIP_EMPTY)]);
    m(&mut t, &f, "rewind", vec![]);
    assert!(is_false(&m(&mut t, &f, "fgetcsv", vec![])));

    let tmp = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &tmp, "setCsvControl", vec![s(","), s("\""), s("")]);
    for r in [
        vec!["john", "doe", "john.doe@example.com", "0123456789"],
        vec!["jane", "doe", "jane.doe@example.com"],
    ] {
        let fields = list(r.into_iter().map(s).collect());
        m(&mut t, &tmp, "fputcsv", vec![fields]);
    }
    m(&mut t, &tmp, "setFlags", vec![Value::Int(0)]);
    m(&mut t, &tmp, "seek", vec![Value::Int(23)]);
    assert!(is_false(&m(&mut t, &tmp, "current", vec![])));
    m(
        &mut t,
        &tmp,
        "setFlags",
        vec![Value::Int(READ_CSV | SKIP_EMPTY)],
    );
    m(&mut t, &tmp, "seek", vec![Value::Int(23)]);
    assert!(is_false(&m(&mut t, &tmp, "current", vec![])));

    let mem = new(&mut t, "SplFileObject", vec![s("php://memory"), s("r")]);
    m(&mut t, &mem, "setCsvControl", vec![s(","), s("\""), s("")]);
    let r = m(
        &mut t,
        &mem,
        "fputcsv",
        vec![list(vec![s("foo"), s("bar")])],
    );
    assert!(is_false(&r));
}

#[test]
fn to_string_with_csv() {
    // bug77024.phpt, gh13685.phpt
    let mut t = TestVm::new();
    let f = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    m(
        &mut t,
        &f,
        "fputcsv",
        vec![list(vec![s("foo"), s("bar"), s("baz")])],
    );
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "setFlags", vec![Value::Int(READ_CSV)]);
    assert_eq!(
        str_of(&m(&mut t, &f, "__toString", vec![])),
        "foo,bar,baz\n"
    );

    let contents = "\"A\", \"B\", \"C\"\n\"D\", \"E\", \"F\"";
    let f = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &f, "fwrite", vec![s(contents)]);
    m(&mut t, &f, "rewind", vec![]);
    let mut seen = Vec::new();
    loop {
        let r = m(&mut t, &f, "fgetcsv", vec![s(","), s("\""), s("")]);
        if is_false(&r) {
            break;
        }
        seen.push(str_of(&m(&mut t, &f, "__toString", vec![])));
    }
    seen.push(str_of(&m(&mut t, &f, "__toString", vec![])));
    assert_eq!(seen, ["\"A\", \"B\", \"C\"\n", "\"D\", \"E\", \"F\"", ""]);

    let f = new(&mut t, "SplTempFileObject", vec![]);
    m(&mut t, &f, "fwrite", vec![s(contents)]);
    m(&mut t, &f, "rewind", vec![]);
    m(&mut t, &f, "setFlags", vec![Value::Int(READ_CSV)]);
    m(&mut t, &f, "setCsvControl", vec![s(","), s("\""), s("")]);
    let mut seen = Vec::new();
    m(&mut t, &f, "rewind", vec![]);
    while is_true(&m(&mut t, &f, "valid", vec![])) {
        m(&mut t, &f, "current", vec![]);
        seen.push(str_of(&m(&mut t, &f, "__toString", vec![])));
        m(&mut t, &f, "next", vec![]);
    }
    seen.push(str_of(&m(&mut t, &f, "__toString", vec![])));
    assert_eq!(seen, ["\"A\", \"B\", \"C\"\n", "\"D\", \"E\", \"F\"", ""]);
}

#[test]
fn fputcsv_lines() {
    // SplFileObject_fputcsv.phpt (a few rows), _fputcsv_variation15.phpt
    let d = TempDir::new("fputcsv");
    let p = d.path("out.csv");
    let mut t = TestVm::new();
    let fo = new(&mut t, "SplFileObject", vec![s(&p), s("w")]);
    m(&mut t, &fo, "setCsvControl", vec![s(","), s("\""), s("\\")]);
    for (v, written) in [
        ("aaa,bbb", 8),
        ("aaa,\"bbb\"", 14),
        ("aaa,bbb   ", 13),
        ("aaa,\"\\\"bbb,ccc", 18),
    ] {
        let fields = list(v.split(',').map(s).collect());
        assert_eq!(
            int(&m(&mut t, &fo, "fputcsv", vec![fields])),
            written,
            "{v}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&p).unwrap(),
        "aaa,bbb\naaa,\"\"\"bbb\"\"\"\naaa,\"bbb   \"\naaa,\"\"\"\\\"bbb\",ccc\n"
    );
    let f = new(&mut t, "SplTempFileObject", vec![]);
    for r in ["\\", "\\\""] {
        m(
            &mut t,
            &f,
            "fputcsv",
            vec![list(vec![s(r)]), s(","), s("\""), s("")],
        );
    }
    m(
        &mut t,
        &f,
        "fputcsv",
        vec![
            list(vec![s("a"), s("b")]),
            s(";"),
            s("\""),
            s(""),
            s("\r\n"),
        ],
    );
    m(&mut t, &f, "rewind", vec![]);
    let mut out = String::new();
    while is_true(&m(&mut t, &f, "valid", vec![])) {
        out.push_str(&str_of(&m(&mut t, &f, "current", vec![])));
        m(&mut t, &f, "next", vec![]);
    }
    assert_eq!(out, "\\\n\"\\\"\"\"\na;b\r\n");
}

#[test]
fn reflection_of_csv_parameters() {
    // bug60201.phpt, bug68479.phpt
    let t = TestVm::new();
    let ce = t.vm.find_class(b"SplFileObject").unwrap();
    let names = |m: &str| -> Vec<String> {
        ce.find_method(m.as_bytes())
            .unwrap()
            .signature()
            .params
            .iter()
            .map(|p| p.name.to_string())
            .collect()
    };
    assert_eq!(names("setCsvControl"), ["separator", "enclosure", "escape"]);
    assert_eq!(
        names("fputcsv"),
        ["fields", "separator", "enclosure", "escape", "eol"]
    );
}

#[test]
fn file_class_with_default_properties() {
    // bug51374.phpt (the object comes from create_object)
    let mut t = TestVm::new();
    user_class(&mut t.vm, "Foo", "SplFileObject", &[]);
    let info = new(&mut t, "SplFileInfo", vec![s("php://temp")]);
    m(&mut t, &info, "setFileClass", vec![s("Foo")]);
    let file = obj_of(&m(&mut t, &info, "openFile", vec![s("r")]));
    assert_eq!(file.class().name.to_string(), "Foo");
    assert_eq!(
        str_of(&m(&mut t, &file, "getPathname", vec![])),
        "php://temp"
    );
}
