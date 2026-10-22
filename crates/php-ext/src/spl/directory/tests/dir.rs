//! `DirectoryIterator`, `FilesystemIterator`, `RecursiveDirectoryIterator`
//! and `GlobIterator`.

use php_runtime::{Args, NativeFunction, ObjRef, PResult, Value, Vm};

use super::{
    TempDir, debug_strs, is_false, is_true, m, new, obj_of, pairs, str_of, try_m, try_new,
};
use crate::spl::directory::{
    get_iterator_check, get_method_check, uses_internal_iterator, with_fs,
};
use crate::spl::engine::ObjIterator;
use crate::spl::testutil::{entries, s, user_class};
use crate::testing::{TestVm, thrown};

const SKIP_DOTS: i64 = 0x1000;
const KEY_AS_FILENAME: i64 = 0x100;
const CURRENT_AS_SELF: i64 = 0x10;
const CURRENT_AS_PATHNAME: i64 = 0x20;
const FOLLOW_SYMLINKS: i64 = 0x4000;
const UNIX_PATHS: i64 = 0x2000;

fn noop(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

static EMPTY_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct() {}",
    handler: noop,
};

fn current_override(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // parent::current()
    let this = args.this().cloned().unwrap();
    let f = vm
        .find_class(b"FilesystemIterator")
        .unwrap()
        .find_method(b"current")
        .cloned()
        .unwrap();
    vm.call_function(
        &f,
        php_runtime::CallArgs {
            this: Some(this),
            ..php_runtime::CallArgs::new(Vec::new())
        },
    )
}

static CURRENT_OVERRIDE: NativeFunction = NativeFunction {
    sig: "public function current(): string|SplFileInfo|FilesystemIterator {}",
    handler: current_override,
};

/// `foreach ($it as $k => $v) body($k, $v)` through the `Iterator`
/// methods.
fn foreach(t: &mut TestVm, obj: &ObjRef, mut body: impl FnMut(&mut TestVm, Value, Value)) {
    let mut it = ObjIterator::over(obj.clone());
    it.rewind(&mut t.vm).unwrap();
    while it.valid(&mut t.vm).unwrap() {
        let v = it.current(&mut t.vm).unwrap();
        let k = it.key(&mut t.vm).unwrap();
        body(t, k, v);
        it.next(&mut t.vm).unwrap();
    }
}

/// The `(key, value)` pairs of a `foreach` (values that are fresh objects
/// per position, e.g. CURRENT_AS_FILEINFO).
fn iterate(t: &mut TestVm, obj: &ObjRef) -> Vec<(Value, Value)> {
    let mut out = Vec::new();
    foreach(t, obj, |_, k, v| out.push((k, v)));
    out
}

/// Sorted file names of the entries (`getFilename()` of each current).
fn names(t: &mut TestVm, obj: &ObjRef) -> Vec<String> {
    let mut v = Vec::new();
    foreach(t, obj, |t, _, cur| {
        v.push(str_of(&m(t, &obj_of(&cur), "getFilename", vec![])))
    });
    v.sort();
    v
}

fn int(v: &Value) -> i64 {
    v.as_int().unwrap()
}

#[test]
fn directory_iterator_debug_info() {
    // dit_001.phpt
    let d = TempDir::new("dit001");
    let mut t = TestVm::new();
    let it = new(&mut t, "DirectoryIterator", vec![s(&d.root())]);
    let dbg = debug_strs(&mut t, &it);
    assert_eq!(
        dbg,
        pairs(&[
            ("pathName:SplFileInfo", &format!("{}/.", d.root())),
            ("fileName:SplFileInfo", "."),
            ("glob:DirectoryIterator", "false"),
            ("subPathName:RecursiveDirectoryIterator", ""),
        ])
    );
    // the stream must not be fclose()d by userland (gh20678.phpt)
    let res = with_fs(&it, |st| st.dirp.clone()).unwrap();
    let r = t.call("fclose", vec![Value::Resource(res)]).unwrap();
    assert!(is_false(&r));
    assert_eq!(
        t.output(),
        "\nWarning: fclose(): cannot close the provided stream, as it must not be manually closed in /t.php on line 3\n"
    );
}

#[test]
fn constructor_errors() {
    // DirectoryIterator_empty_constructor.phpt, bug54291.phpt,
    // bug78863.phpt, bug61828.phpt
    let d = TempDir::new("ctor");
    let mut t = TestVm::new();
    let (class, msg) = thrown(try_new(&mut t, "DirectoryIterator", vec![s("")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "DirectoryIterator::__construct(): Argument #1 ($directory) must not be empty"
    );
    let (class, msg) = thrown(try_new(&mut t, "DirectoryIterator", vec![s("\0/abc")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "DirectoryIterator::__construct(): Argument #1 ($directory) must not contain any null bytes"
    );
    let (_, msg) = thrown(try_new(
        &mut t,
        "RecursiveDirectoryIterator",
        vec![s(&format!("{}\0/sub", d.root()))],
    ));
    assert_eq!(
        msg,
        "RecursiveDirectoryIterator::__construct(): Argument #1 ($directory) must not contain any null bytes"
    );
    let x = new(&mut t, "DirectoryIterator", vec![s(&d.root())]);
    let (class, msg) = thrown(try_m(&mut t, &x, "__construct", vec![s("/tmp")]));
    assert_eq!(class, "Error");
    assert_eq!(msg, "Directory object is already initialized");
}

#[test]
fn missing_directories_throw_unexpected_value() {
    let d = TempDir::new("missing");
    let missing = d.path("nope");
    let mut t = TestVm::new();
    for class in [
        "DirectoryIterator",
        "FilesystemIterator",
        "RecursiveDirectoryIterator",
    ] {
        let (c, msg) = thrown(try_new(&mut t, class, vec![s(&missing)]));
        assert_eq!(c, "UnexpectedValueException");
        assert_eq!(
            msg,
            format!("{class}::__construct(): Failed to open directory: No such file or directory")
        );
    }
    assert_eq!(t.output(), "");
    assert!(matches!(
        t.vm.errors.mode,
        php_runtime::errors::ErrorHandlingMode::Normal
    ));
}

#[test]
fn glob_defaults() {
    // dit_002.phpt
    let d = TempDir::new("dit002");
    d.file("a", "");
    d.file("b", "");
    let pattern = format!("glob://{}/*", d.root());
    let mut t = TestVm::new();
    let obj = new(&mut t, "DirectoryIterator", vec![s(&pattern)]);
    let cur = obj_of(&m(&mut t, &obj, "current", vec![]));
    assert_eq!(cur.class().name.to_string(), "DirectoryIterator");
    for (class, flags) in [
        ("FilesystemIterator", SKIP_DOTS),
        ("RecursiveDirectoryIterator", 0),
        ("GlobIterator", 0),
    ] {
        let obj = new(&mut t, class, vec![s(&pattern)]);
        let cur = obj_of(&m(&mut t, &obj, "current", vec![]));
        assert_eq!(cur.class().name.to_string(), "SplFileInfo");
        assert_eq!(int(&m(&mut t, &obj, "getFlags", vec![])), flags, "{class}");
        for (flag, expected) in [(0, "SplFileInfo"), (CURRENT_AS_SELF, class)] {
            m(&mut t, &obj, "setFlags", vec![Value::Int(flag)]);
            m(&mut t, &obj, "rewind", vec![]);
            assert_eq!(int(&m(&mut t, &obj, "getFlags", vec![])), flag);
            let cur = obj_of(&m(&mut t, &obj, "current", vec![]));
            assert_eq!(cur.class().name.to_string(), expected);
        }
        m(
            &mut t,
            &obj,
            "setFlags",
            vec![Value::Int(CURRENT_AS_PATHNAME)],
        );
        m(&mut t, &obj, "rewind", vec![]);
        assert_eq!(int(&m(&mut t, &obj, "getFlags", vec![])), 32);
        assert_eq!(
            str_of(&m(&mut t, &obj, "current", vec![])),
            format!("{}/a", d.root())
        );
    }
    let g = new(&mut t, "GlobIterator", vec![s(&format!("{}/*", d.root()))]);
    assert_eq!(int(&m(&mut t, &g, "count", vec![])), 2);
    let dbg = debug_strs(&mut t, &g);
    assert_eq!(
        dbg[2],
        (
            "glob:DirectoryIterator".to_string(),
            format!("glob://{}/*", d.root())
        )
    );
}

#[test]
fn foreach_over_filesystem_iterator() {
    // dit_003.phpt, filesystemiterator_leak.phpt
    let d = TempDir::new("dit003");
    d.file("x", "");
    let mut t = TestVm::new();
    let it = new(&mut t, "FilesystemIterator", vec![s(&d.root())]);
    let all = iterate(&mut t, &it);
    assert_eq!(all.len(), 1);
    assert_eq!(str_of(&all[0].0), d.path("x"));
    assert_eq!(obj_of(&all[0].1).class().name.to_string(), "SplFileInfo");
}

#[test]
fn foreach_by_reference_is_an_error() {
    // DirectoryIterator_by_reference.phpt
    let d = TempDir::new("byref");
    let mut t = TestVm::new();
    let it = new(&mut t, "DirectoryIterator", vec![s(&d.root())]);
    get_iterator_check(&mut t.vm, &it, false).unwrap();
    let (class, msg) = thrown(get_iterator_check(&mut t.vm, &it, true).map(|_| Value::Null));
    assert_eq!(class, "Error");
    assert_eq!(msg, "An iterator cannot be used with foreach by reference");
    let fi = new(&mut t, "FilesystemIterator", vec![s(&d.root())]);
    assert!(get_iterator_check(&mut t.vm, &fi, true).is_err());
    // a userland override of an Iterator method switches to the methods
    let ce = user_class(
        &mut t.vm,
        "MyFsIterator",
        "FilesystemIterator",
        &[&CURRENT_OVERRIDE],
    );
    assert!(!uses_internal_iterator(&t.vm, &ce));
    let ce = user_class(&mut t.vm, "PlainSub", "DirectoryIterator", &[]);
    assert!(uses_internal_iterator(&t.vm, &ce));
    let f = new(&mut t, "SplTempFileObject", vec![]);
    get_iterator_check(&mut t.vm, &f, true).unwrap();
}

#[test]
fn cloning_keeps_the_position() {
    // dit_004.phpt, dit_005.phpt
    let d = TempDir::new("dit004");
    for f in ["file1", "file2", "file3"] {
        d.file(f, "");
    }
    let mut t = TestVm::new();
    for class in ["DirectoryIterator", "FilesystemIterator"] {
        let a = new(&mut t, class, vec![s(&d.root())]);
        let clone = |t: &mut TestVm, o: &ObjRef| {
            (o.class().handlers.clone_obj.unwrap())(&mut t.vm, o).unwrap()
        };
        let b = clone(&mut t, &a);
        assert_eq!(
            str_of(&m(&mut t, &b, "__toString", vec![])),
            str_of(&m(&mut t, &a, "__toString", vec![]))
        );
        assert!(m(&mut t, &a, "key", vec![]).identical(&m(&mut t, &b, "key", vec![])));
        for _ in 0..3 {
            m(&mut t, &a, "next", vec![]);
        }
        let c = clone(&mut t, &a);
        assert_eq!(
            str_of(&m(&mut t, &c, "__toString", vec![])),
            str_of(&m(&mut t, &a, "__toString", vec![]))
        );
        assert!(m(&mut t, &a, "key", vec![]).identical(&m(&mut t, &c, "key", vec![])));
        if class == "DirectoryIterator" {
            assert_eq!(int(&m(&mut t, &c, "key", vec![])), 3);
        }
    }
    // GlobIterator and SplFileObject are not cloneable
    let g = new(&mut t, "GlobIterator", vec![s(&format!("{}/*", d.root()))]);
    let (class, msg) =
        thrown((g.class().handlers.clone_obj.unwrap())(&mut t.vm, &g).map(Value::Object));
    assert_eq!(class, "Error");
    assert_eq!(
        msg,
        "Trying to clone an uncloneable object of class GlobIterator"
    );
}

#[test]
fn seek() {
    // dit_006.phpt, bug70561.phpt
    let d = TempDir::new("dit006");
    for f in ["a", "b", "c", "d"] {
        d.file(f, "");
    }
    let mut t = TestVm::new();
    let di = new(&mut t, "DirectoryIterator", vec![s(&d.root())]);
    let count = |t: &mut TestVm, di: &ObjRef| {
        let mut n = 0;
        while is_true(&m(t, di, "valid", vec![])) {
            n += 1;
            m(t, di, "next", vec![]);
        }
        n
    };
    m(&mut t, &di, "seek", vec![Value::Int(2)]);
    let n = count(&mut t, &di);
    m(&mut t, &di, "seek", vec![Value::Int(0)]);
    let mm = count(&mut t, &di);
    m(&mut t, &di, "rewind", vec![]);
    let o = count(&mut t, &di);
    assert_eq!((n, mm, o), (4, 6, 6));
    let (class, msg) = thrown(try_m(&mut t, &di, "seek", vec![Value::Int(o + 1)]));
    assert_eq!(class, "OutOfBoundsException");
    assert_eq!(msg, "Seek position 7 is out of range");
    assert!(is_false(&m(&mut t, &di, "valid", vec![])));
}

#[test]
fn seek_and_current_pathnames() {
    // bug68557.phpt
    let d = TempDir::new("bug68557");
    d.file("a", "");
    d.file("b", "");
    let mut t = TestVm::new();
    let di = new(&mut t, "DirectoryIterator", vec![s(&d.root())]);
    let mut paths = Vec::new();
    for i in 0..3 {
        m(&mut t, &di, "seek", vec![Value::Int(i)]);
        let cur = obj_of(&m(&mut t, &di, "current", vec![]));
        paths.push(str_of(&m(&mut t, &cur, "getPathname", vec![])));
    }
    for i in [0, 1, 2, 0] {
        m(&mut t, &di, "seek", vec![Value::Int(i)]);
        let cur = obj_of(&m(&mut t, &di, "current", vec![]));
        assert_eq!(
            str_of(&m(&mut t, &cur, "getPathname", vec![])),
            paths[i as usize]
        );
    }
}

#[test]
fn entry_accessors() {
    // DirectoryIterator_getBasename_basic_test.phpt,
    // DirectoryIterator_getExtension_basic.phpt, bug42364.phpt
    let d = TempDir::new("entries");
    for f in ["test.txt", "test.extension", "test"] {
        d.file(f, "");
    }
    let mut t = TestVm::new();
    let dir = new(
        &mut t,
        "DirectoryIterator",
        vec![s(&format!("{}/", d.root()))],
    );
    while !is_true(&m(&mut t, &dir, "isFile", vec![])) {
        m(&mut t, &dir, "next", vec![]);
    }
    let base = str_of(&m(&mut t, &dir, "getBasename", vec![s(".txt")]));
    let file = str_of(&m(&mut t, &dir, "getFilename", vec![]));
    assert_eq!(base, file.strip_suffix(".txt").unwrap_or(&file));
    let mut exts = Vec::new();
    let mut count = 0;
    foreach(&mut t, &dir, |t, _, cur| {
        let e = obj_of(&cur);
        count += 1;
        assert!(matches!(m(t, &e, "getRealPath", vec![]), Value::String(_)));
        if !is_true(&m(t, &e, "isDot", vec![])) {
            exts.push(str_of(&m(t, &e, "getExtension", vec![])));
        }
    });
    assert_eq!(count, 5);
    exts.sort();
    assert_eq!(exts, ["", "extension", "txt"]);
    // the path of a DirectoryIterator has its trailing slash removed
    assert_eq!(str_of(&m(&mut t, &dir, "getPath", vec![])), d.root());
}

#[test]
fn stat_of_the_current_entry() {
    // DirectoryIterator_get{Group,Owner,Inode}_basic.phpt
    use std::os::unix::fs::MetadataExt;
    let d = TempDir::new("owner");
    let sub = d.dir("DirectoryIterator_getOwner_basic");
    let meta = std::fs::metadata(&sub).unwrap();
    let mut t = TestVm::new();
    let dir = new(&mut t, "DirectoryIterator", vec![s(&sub)]);
    assert_eq!(str_of(&m(&mut t, &dir, "getFilename", vec![])), ".");
    assert_eq!(
        int(&m(&mut t, &dir, "getOwner", vec![])),
        i64::from(meta.uid())
    );
    assert_eq!(
        int(&m(&mut t, &dir, "getGroup", vec![])),
        i64::from(meta.gid())
    );
    assert_eq!(int(&m(&mut t, &dir, "getInode", vec![])), meta.ino() as i64);
}

#[test]
fn uninitialized_directory_iterator() {
    // DirectoryIterator_uninitialized.phpt
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "MyDirectoryIterator",
        "DirectoryIterator",
        &[&EMPTY_CTOR],
    );
    let it = new(&mut t, "MyDirectoryIterator", vec![]);
    for method in [
        "key",
        "current",
        "valid",
        "rewind",
        "next",
        "isDot",
        "getFilename",
    ] {
        let (class, msg) = thrown(try_m(&mut t, &it, method, vec![]));
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Error", "Object not initialized"),
            "{method}"
        );
    }
    // DirectoryIterator is not guarded by the get_method check
    get_method_check(&mut t.vm, &it).unwrap();
}

#[test]
fn filesystem_iterator_flags() {
    // filesystemiterator_flags.phpt
    let d = TempDir::new("flags");
    let mut t = TestVm::new();
    let it = new(&mut t, "FileSystemIterator", vec![s(&d.root())]);
    let flags = |t: &mut TestVm| {
        let f = int(&m(t, &it, "getFlags", vec![]));
        [f, f & 0xF0, f & 0xF00, f & 0x7000]
    };
    assert_eq!(flags(&mut t), [0x1000, 0, 0, 0x1000]);
    m(
        &mut t,
        &it,
        "setFlags",
        vec![Value::Int(
            CURRENT_AS_SELF | KEY_AS_FILENAME | SKIP_DOTS | UNIX_PATHS,
        )],
    );
    assert_eq!(flags(&mut t), [0x3110, 0x10, 0x100, 0x3000]);
    m(&mut t, &it, "setFlags", vec![Value::Int(-1)]);
    assert_eq!(flags(&mut t), [0x7FF0, 0xF0, 0xF00, 0x7000]);
}

#[test]
fn dots() {
    // filesystemiterator_no_skip_dots.phpt, bug64228.phpt, bug62433.phpt
    let d = TempDir::new("dots");
    d.file("file", "");
    let mut t = TestVm::new();
    let it = new(
        &mut t,
        "FileSystemIterator",
        vec![s(&d.root()), Value::Int(0)],
    );
    assert_eq!(names(&mut t, &it), [".", "..", "file"]);
    let empty = TempDir::new("empty");
    let it = new(
        &mut t,
        "RecursiveDirectoryIterator",
        vec![s(&empty.root()), Value::Int(0)],
    );
    assert_eq!(names(&mut t, &it), [".", ".."]);
    let keys = |t: &mut TestVm, flags: Option<i64>| -> Vec<String> {
        let mut args = vec![s(&d.root())];
        args.extend(flags.map(Value::Int));
        let it = new(t, "RecursiveDirectoryIterator", args);
        let arr = t
            .call("iterator_to_array", vec![Value::Object(it)])
            .unwrap();
        entries(&arr).into_iter().map(|(k, _)| k).collect()
    };
    let dots = keys(&mut t, None);
    assert!(dots.contains(&format!("{}/.", d.root())));
    assert!(dots.contains(&format!("{}/..", d.root())));
    let ndots = keys(&mut t, Some(SKIP_DOTS));
    assert!(!ndots.contains(&format!("{}/.", d.root())));
    assert_eq!(ndots, [d.path("file")]);
}

/// `RecursiveIteratorIterator` (LEAVES_ONLY) over a
/// `RecursiveDirectoryIterator`, collecting `$f(leaf iterator)`.
fn leaves(
    t: &mut TestVm,
    it: &ObjRef,
    f: &dyn Fn(&mut TestVm, &ObjRef) -> String,
    out: &mut Vec<String>,
) {
    m(t, it, "rewind", vec![]);
    while is_true(&m(t, it, "valid", vec![])) {
        if is_true(&m(t, it, "hasChildren", vec![])) {
            let child = obj_of(&m(t, it, "getChildren", vec![]));
            leaves(t, &child, f, out);
        } else {
            out.push(f(t, it));
        }
        m(t, it, "next", vec![]);
    }
}

#[test]
fn sub_paths() {
    // RecursiveDirectoryIterator_getSubPath_basic.phpt,
    // RecursiveDirectoryIterator_getSubPathname_basic.phpt
    let d = TempDir::new("subpath");
    d.dir("depth02/depth1/depth2");
    d.file("depth02/depth1/depth2/getSubPathname_test_2.tmp", "");
    d.file("depth02/depth1/getSubPathname_test_3.tmp", "");
    d.file("depth02/getSubPathname_test_1.tmp", "");
    let mut t = TestVm::new();
    let it = new(
        &mut t,
        "RecursiveDirectoryIterator",
        vec![s(&d.path("depth02"))],
    );
    let mut list = Vec::new();
    leaves(
        &mut t,
        &it,
        &|t, it| str_of(&m(t, it, "getSubPathname", vec![])),
        &mut list,
    );
    list.sort();
    assert_eq!(
        list,
        [
            ".",
            "..",
            "depth1/.",
            "depth1/..",
            "depth1/depth2/.",
            "depth1/depth2/..",
            "depth1/depth2/getSubPathname_test_2.tmp",
            "depth1/getSubPathname_test_3.tmp",
            "getSubPathname_test_1.tmp",
        ]
    );
    let mut list = Vec::new();
    leaves(
        &mut t,
        &it,
        &|t, it| str_of(&m(t, it, "getSubPath", vec![])),
        &mut list,
    );
    list.sort();
    assert_eq!(
        list,
        [
            "",
            "",
            "",
            "depth1",
            "depth1",
            "depth1",
            "depth1/depth2",
            "depth1/depth2",
            "depth1/depth2",
        ]
    );
    // the children keep the flags, the info class and the class
    m(
        &mut t,
        &it,
        "setFlags",
        vec![Value::Int(SKIP_DOTS | CURRENT_AS_PATHNAME)],
    );
    m(&mut t, &it, "rewind", vec![]);
    while !is_true(&m(&mut t, &it, "hasChildren", vec![])) {
        m(&mut t, &it, "next", vec![]);
    }
    let child = obj_of(&m(&mut t, &it, "getChildren", vec![]));
    assert_eq!(
        int(&m(&mut t, &child, "getFlags", vec![])),
        SKIP_DOTS | CURRENT_AS_PATHNAME
    );
    assert_eq!(
        debug_strs(&mut t, &child)[3],
        (
            "subPathName:RecursiveDirectoryIterator".to_string(),
            "depth1".to_string()
        )
    );
}

#[test]
fn children_of_subclasses() {
    // bug66405.phpt (CURRENT_AS_PATHNAME with an overridden current())
    let d = TempDir::new("bug66405");
    d.file("file1.txt", "");
    d.file("file2.md", "");
    d.dir("testsubdir");
    d.file("testsubdir/file3.csv", "");
    let mut t = TestVm::new();
    user_class(
        &mut t.vm,
        "Bug66405",
        "RecursiveDirectoryIterator",
        &[&CURRENT_OVERRIDE],
    );
    let rdi = new(
        &mut t,
        "Bug66405",
        vec![s(&d.root()), Value::Int(CURRENT_AS_PATHNAME | SKIP_DOTS)],
    );
    let mut list = Vec::new();
    leaves(
        &mut t,
        &rdi,
        &|t, it| {
            let cur = m(t, it, "current", vec![]);
            format!("{} {}", it.class().name, str_of(&cur))
        },
        &mut list,
    );
    list.sort();
    assert_eq!(
        list,
        [
            format!("Bug66405 {}", d.path("file1.txt")),
            format!("Bug66405 {}", d.path("file2.md")),
            format!("Bug66405 {}", d.path("testsubdir/file3.csv")),
        ]
    );
    // bug47534.phpt
    let it = new(
        &mut t,
        "RecursiveDirectoryIterator",
        vec![s(&d.root()), Value::Int(CURRENT_AS_PATHNAME)],
    );
    m(&mut t, &it, "rewind", vec![]);
    assert!(matches!(
        m(&mut t, &it, "current", vec![]),
        Value::String(_)
    ));
    let it = new(&mut t, "RecursiveDirectoryIterator", vec![s(&d.root())]);
    m(&mut t, &it, "rewind", vec![]);
    assert!(matches!(
        m(&mut t, &it, "current", vec![]),
        Value::Object(_)
    ));
}

#[test]
fn has_children_and_symlinks() {
    // RecursiveDirectoryIterator_hasChildren.phpt
    let d = TempDir::new("symlink");
    let target = TempDir::new("symtarget");
    let dir = d.dir("symlinktest");
    std::os::unix::fs::symlink(target.root(), format!("{dir}/symlink")).unwrap();
    let mut t = TestVm::new();
    let it = new(
        &mut t,
        "RecursiveDirectoryIterator",
        vec![
            s(&dir),
            Value::Int(SKIP_DOTS | FOLLOW_SYMLINKS | KEY_AS_FILENAME),
        ],
    );
    assert_eq!(str_of(&m(&mut t, &it, "key", vec![])), "symlink");
    assert!(is_true(&m(&mut t, &it, "hasChildren", vec![])));
    m(
        &mut t,
        &it,
        "setFlags",
        vec![Value::Int(SKIP_DOTS | KEY_AS_FILENAME)],
    );
    assert_eq!(str_of(&m(&mut t, &it, "key", vec![])), "symlink");
    assert!(is_false(&m(&mut t, &it, "hasChildren", vec![])));
    assert!(is_true(&m(
        &mut t,
        &it,
        "hasChildren",
        vec![Value::Bool(true)]
    )));
}

#[test]
fn key_flags() {
    // bug80724.phpt
    let d = TempDir::new("bug80724");
    d.file("x", "");
    let mut t = TestVm::new();
    let has_sep = |k: &Value| str_of(k).contains(&format!("{}/", d.root()));
    for flags in [KEY_AS_FILENAME, FOLLOW_SYMLINKS | KEY_AS_FILENAME] {
        let it = new(
            &mut t,
            "FilesystemIterator",
            vec![s(&d.root()), Value::Int(flags)],
        );
        let first = iterate(&mut t, &it);
        assert!(!has_sep(&first[0].0));
        m(&mut t, &it, "rewind", vec![]);
        assert!(!has_sep(&m(&mut t, &it, "key", vec![])));
        m(&mut t, &it, "setFlags", vec![Value::Int(0)]);
        assert!(has_sep(&m(&mut t, &it, "key", vec![])));
        m(&mut t, &it, "setFlags", vec![Value::Int(KEY_AS_FILENAME)]);
        assert!(!has_sep(&m(&mut t, &it, "key", vec![])));
    }
}

#[test]
fn link_target_of_entries() {
    // bug68825.phpt
    let d = TempDir::new("bug68825");
    let target = d.file("target.php", "");
    let dir = d.dir("bug68825");
    std::os::unix::fs::symlink(&target, format!("{dir}/bug")).unwrap();
    let mut t = TestVm::new();
    let di = new(&mut t, "DirectoryIterator", vec![s(&dir)]);
    let mut found = Vec::new();
    foreach(&mut t, &di, |t, _, e| {
        let e = obj_of(&e);
        if str_of(&m(t, &e, "getFilename", vec![])) == "bug" {
            found.push(str_of(&m(t, &e, "getLinkTarget", vec![])));
        }
    });
    assert_eq!(found, [target]);
}

#[test]
fn glob_patterns() {
    // bug51068.phpt (with absolute patterns)
    let d = TempDir::new("bug51068");
    d.file("bug.51068", "");
    d.dir("bug.51068.dir");
    d.file("bug.51068.dir/lvl2.bug.51068", "");
    let mut t = TestVm::new();
    let r = d.root();
    for (pattern, expected) in [
        (format!("glob://{r}/*.51068"), "bug.51068"),
        (
            format!("glob://{r}/bug.51068.dir/*.51068"),
            "lvl2.bug.51068",
        ),
        (format!("glob://{r}/bug.51068.dir"), "bug.51068.dir"),
    ] {
        let iter = new(&mut t, "DirectoryIterator", vec![s(&pattern)]);
        let mut seen = Vec::new();
        foreach(&mut t, &iter, |t, _, f| {
            let f = obj_of(&f);
            seen.push(str_of(&m(t, &f, "getFilename", vec![])));
            assert!(int(&m(t, &f, "getSize", vec![])) >= 0);
        });
        assert_eq!(seen, [expected]);
    }
    // gh12721.phpt ("With slash")
    d.file("file1.gh12721", "hello");
    let g = new(&mut t, "GlobIterator", vec![s(&format!("{r}/*.gh12721"))]);
    let all = iterate(&mut t, &g);
    assert_eq!(all.len(), 1);
    let fi = obj_of(&all[0].1);
    assert_eq!(
        str_of(&m(&mut t, &fi, "getFilename", vec![])),
        "file1.gh12721"
    );
    assert_eq!(str_of(&m(&mut t, &fi, "getExtension", vec![])), "gh12721");
    assert_eq!(
        str_of(&m(&mut t, &fi, "getBasename", vec![])),
        "file1.gh12721"
    );
    let info = obj_of(&m(&mut t, &fi, "getFileInfo", vec![]));
    assert_eq!(
        debug_strs(&mut t, &info),
        pairs(&[
            ("pathName:SplFileInfo", &d.path("file1.gh12721")),
            ("fileName:SplFileInfo", "file1.gh12721"),
        ])
    );
}

#[test]
fn glob_without_matches() {
    // GlobIterator_internal_null_pointer.phpt, bug55701.phpt
    let d = TempDir::new("nomatch");
    let pattern = format!("{}/*.abcdefghij", d.root());
    let mut t = TestVm::new();
    let o = new(&mut t, "GlobIterator", vec![s(&pattern)]);
    for method in [
        "getATime",
        "getCTime",
        "getGroup",
        "getInode",
        "getMTime",
        "getOwner",
        "getPerms",
        "getSize",
        "getType",
        "isDir",
        "isExecutable",
        "isFile",
        "isLink",
        "isReadable",
        "isWritable",
    ] {
        assert!(is_false(&m(&mut t, &o, method, vec![])), "{method}");
    }
    for method in [
        "getBasename",
        "getExtension",
        "getFilename",
        "getPath",
        "getPathname",
        "__toString",
    ] {
        assert_eq!(str_of(&m(&mut t, &o, method, vec![])), "", "{method}");
    }
    assert!(m(&mut t, &o, "getPathInfo", vec![]).is_null());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(str_of(&m(&mut t, &o, "getRealPath", vec![])), cwd);
    assert_eq!(
        debug_strs(&mut t, &o),
        pairs(&[
            ("pathName:SplFileInfo", ""),
            ("fileName:SplFileInfo", ""),
            ("glob:DirectoryIterator", &format!("glob://{pattern}")),
            ("subPathName:RecursiveDirectoryIterator", ""),
        ])
    );
    assert_eq!(int(&m(&mut t, &o, "count", vec![])), 0);
    for method in ["rewind", "getFlags", "valid", "key", "next", "isDot"] {
        m(&mut t, &o, method, vec![]);
    }
    m(&mut t, &o, "setFlags", vec![Value::Int(0)]);
    m(&mut t, &o, "seek", vec![Value::Int(0)]);
    let (class, _) = thrown(try_m(&mut t, &o, "current", vec![]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(t.output(), "");
    get_method_check(&mut t.vm, &o).unwrap();
}

#[test]
fn parent_constructor_not_called() {
    // bug55701.phpt, bug54384.phpt, gh8318.phpt, GlobIterator_constructor_count.phpt
    let mut t = TestVm::new();
    for (class, parent) in [
        ("GlobIteratorChild", "GlobIterator"),
        ("SplFileObjectChild", "SplFileObject"),
        ("SplTempFileObjectChild", "SplTempFileObject"),
    ] {
        user_class(&mut t.vm, class, parent, &[&EMPTY_CTOR]);
        let o = new(&mut t, class, vec![]);
        let (c, msg) = thrown(get_method_check(&mut t.vm, &o).map(|_| Value::Null));
        assert_eq!(c, "Error");
        assert_eq!(
            msg,
            "The parent constructor was not called: the object is in an invalid state"
        );
    }
    // count() calls the method directly
    let o = new(&mut t, "GlobIteratorChild", vec![]);
    let (c, msg) = thrown(try_m(&mut t, &o, "count", vec![]));
    assert_eq!(
        (c.as_str(), msg.as_str()),
        ("Error", "GlobIterator is not initialized")
    );
    let o = new(&mut t, "SplFileObjectChild", vec![]);
    let (_, msg) = thrown(try_m(&mut t, &o, "rewind", vec![]));
    assert_eq!(msg, "Object not initialized");
    // constructed objects pass
    let f = new(&mut t, "SplTempFileObject", vec![]);
    get_method_check(&mut t.vm, &f).unwrap();
    let other = new(&mut t, "SplFileInfo", vec![s("x")]);
    get_method_check(&mut t.vm, &other).unwrap();
}

#[test]
fn glob_and_open_basedir() {
    // bug65069.phpt
    let d = TempDir::new("bug65069");
    let dirname = d.dir("x/bug65069");
    for f in ["wonder12345", "wonder.txt", "file.text"] {
        d.file(&format!("x/bug65069/{f}"), "");
    }
    let mut t = TestVm::new();
    assert!(t.vm.ini_set("open_basedir", dirname.clone()).is_some());
    let g = new(&mut t, "GlobIterator", vec![s(&format!("{dirname}/*.txt"))]);
    assert_eq!(names(&mut t, &g), ["wonder.txt"]);
    let g = new(
        &mut t,
        "GlobIterator",
        vec![s(&format!("{}/*/*/*", d.root()))],
    );
    assert_eq!(
        names(&mut t, &g),
        ["file.text", "wonder.txt", "wonder12345"]
    );
    let g = new(&mut t, "GlobIterator", vec![s(&format!("{dirname}/*.php"))]);
    assert_eq!(int(&m(&mut t, &g, "count", vec![])), 0);
    let g = new(&mut t, "GlobIterator", vec![s(&d.root())]);
    assert!(iterate(&mut t, &g).is_empty());
    let g = new(&mut t, "GlobIterator", vec![s(&d.path("never-exists"))]);
    assert!(iterate(&mut t, &g).is_empty());
}
