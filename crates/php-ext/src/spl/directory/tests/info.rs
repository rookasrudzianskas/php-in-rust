//! `SplFileInfo`.

use php_runtime::class::CastKind;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, Value, Vm};

use super::{
    TempDir, debug, debug_strs, is_false, is_true, m, new, obj_of, pairs, str_of, try_m, try_new,
};
use crate::spl::testutil::{s, user_class};
use crate::testing::{TestVm, thrown};

fn noop(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

/// `public function __construct() {}` (a constructor that does not call
/// the parent's).
static EMPTY_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct() {}",
    handler: noop,
};

fn record_ctor(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = args.this().cloned().unwrap();
    this.set_prop(
        b"seen",
        Value::array(PhpArray::from_values(args.values().to_vec())),
    );
    Ok(Value::Null)
}

/// `public function __construct(...$args) { $this->seen = $args; }`.
static RECORD_CTOR: NativeFunction = NativeFunction {
    sig: "public function __construct(mixed ...$args) {}",
    handler: record_ctor,
};

fn to_string_override(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(s("__toString"))
}

static TO_STRING_OVERRIDE: NativeFunction = NativeFunction {
    sig: "public function __toString(): string {}",
    handler: to_string_override,
};

fn info(t: &mut TestVm, path: &str) -> php_runtime::ObjRef {
    new(t, "SplFileInfo", vec![s(path)])
}

#[test]
fn construct_twice_and_coercion() {
    // SplFileInfo_001.phpt
    let mut t = TestVm::new();
    let x = new(&mut t, "splfileinfo", vec![Value::Int(1)]);
    m(&mut t, &x, "__construct", vec![Value::Int(1)]);
    assert_eq!(str_of(&m(&mut t, &x, "getPathname", vec![])), "1");
}

#[test]
fn constructor_rejects_null_bytes() {
    // bug77431.phpt
    let mut t = TestVm::new();
    let (class, msg) = thrown(try_new(&mut t, "SplFileInfo", vec![s("bad\0good")]));
    assert_eq!(class, "ValueError");
    assert_eq!(
        msg,
        "SplFileInfo::__construct(): Argument #1 ($filename) must not contain any null bytes"
    );
}

#[test]
fn debug_info_of_paths() {
    // SplFileinfo_debugInfo_basic.phpt
    let mut t = TestVm::new();
    for (path, file) in [
        ("/path/to/a.txt", "a.txt"),
        ("path/to/b", "b"),
        ("c.txt", "c.txt"),
        ("d", "d"),
        ("~/path/to//e", "e"),
        ("path/to/bbb.txt", "bbb.txt"),
    ] {
        let o = info(&mut t, path);
        assert_eq!(
            debug_strs(&mut t, &o),
            pairs(&[
                ("pathName:SplFileInfo", path),
                ("fileName:SplFileInfo", file)
            ]),
            "{path}"
        );
    }
}

#[test]
fn basename_and_filename() {
    // SplFileinfo_getBasename_basic.phpt, SplFileinfo_getFilename_basic.phpt
    let mut t = TestVm::new();
    for (path, suffix, base, file) in [
        ("/path/to/a.txt", None, "a.txt", "a.txt"),
        ("path/to/b", None, "b", "b"),
        ("c.txt", None, "c.txt", "c.txt"),
        ("d", None, "d", "d"),
        ("~/path/to//e", None, "e", "e"),
        ("path/to/a.txt", Some(".txt"), "a", "a.txt"),
        ("path/to/bbb.txt", Some("b.txt"), "bb", "bbb.txt"),
        ("path/to/ccc.txt", Some("to/ccc.txt"), "ccc.txt", "ccc.txt"),
        ("d.txt", Some("txt"), "d.", "d.txt"),
        ("e.txt", Some("e.txt"), "e.txt", "e.txt"),
        ("f", Some(".txt"), "f", "f"),
    ] {
        let o = info(&mut t, path);
        let args = suffix.map(s).into_iter().collect();
        assert_eq!(str_of(&m(&mut t, &o, "getBasename", args)), base, "{path}");
        assert_eq!(
            str_of(&m(&mut t, &o, "getFilename", vec![])),
            file,
            "{path}"
        );
    }
}

#[test]
fn trailing_slashes() {
    // gh11338.phpt
    let mut t = TestVm::new();
    for (path, base, file) in [
        ("/dir/anotherdir/basedir//", "basedir", "basedir"),
        ("/dir/anotherdir/basedir/", "basedir", "basedir"),
        ("/dir/anotherdir/basedir", "basedir", "basedir"),
        ("/dir/anotherdir//basedir", "basedir", "basedir"),
        ("///", "", "/"),
        ("//", "", "/"),
        ("/", "", "/"),
        ("", "", ""),
    ] {
        let o = info(&mut t, path);
        assert_eq!(
            str_of(&m(&mut t, &o, "getBasename", vec![])),
            base,
            "{path:?}"
        );
        assert_eq!(
            str_of(&m(&mut t, &o, "getFilename", vec![])),
            file,
            "{path:?}"
        );
    }
    let o = info(&mut t, "/dir/anotherdir/basedir//");
    assert_eq!(str_of(&m(&mut t, &o, "getPath", vec![])), "/dir/anotherdir");
    assert_eq!(
        str_of(&m(&mut t, &o, "getPathname", vec![])),
        "/dir/anotherdir/basedir"
    );
}

#[test]
fn extensions() {
    // SplFileInfo_getExtension_basic.phpt, spl_fileinfo_getextension_leadingdot.phpt
    let d = TempDir::new("ext");
    let mut t = TestVm::new();
    let file = "e8e7a7c6";
    for (ext, expected) in [
        (".txt", "txt"),
        (".extension", "extension"),
        ("..", ""),
        (".", ""),
        ("", ""),
    ] {
        let p = d.file(&format!("{file}{ext}"), "");
        let o = info(&mut t, &p);
        assert_eq!(
            str_of(&m(&mut t, &o, "getExtension", vec![])),
            expected,
            "{ext}"
        );
    }
    let p = d.file(".test", "");
    let o = info(&mut t, &p);
    assert_eq!(str_of(&m(&mut t, &o, "getExtension", vec![])), "test");
}

#[test]
fn stat_getters() {
    // SplFileInfo_getPerms_basic.phpt, fileobject_getsize_basic.phpt,
    // fileobject_checktype_basic.phpt
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let d = TempDir::new("stat");
    let p = d.file("SplFileInfo_getPerms_basic.txt", "foobar");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o557)).unwrap();
    let meta = std::fs::metadata(&p).unwrap();
    let mut t = TestVm::new();
    let o = info(&mut t, &p);
    let int = |v: Value| v.as_int().unwrap();
    assert_eq!(int(m(&mut t, &o, "getPerms", vec![])), 0o100557);
    assert_eq!(int(m(&mut t, &o, "getSize", vec![])), 6);
    assert_eq!(int(m(&mut t, &o, "getInode", vec![])), meta.ino() as i64);
    assert_eq!(
        int(m(&mut t, &o, "getOwner", vec![])),
        i64::from(meta.uid())
    );
    assert_eq!(
        int(m(&mut t, &o, "getGroup", vec![])),
        i64::from(meta.gid())
    );
    assert_eq!(int(m(&mut t, &o, "getMTime", vec![])), meta.mtime());
    assert_eq!(int(m(&mut t, &o, "getATime", vec![])), meta.atime());
    assert_eq!(int(m(&mut t, &o, "getCTime", vec![])), meta.ctime());
    assert_eq!(str_of(&m(&mut t, &o, "getType", vec![])), "file");
    assert!(is_true(&m(&mut t, &o, "isFile", vec![])));
    assert!(is_false(&m(&mut t, &o, "isDir", vec![])));
    assert!(is_false(&m(&mut t, &o, "isLink", vec![])));
    assert!(is_true(&m(&mut t, &o, "isReadable", vec![])));
    let dir = info(&mut t, &d.root());
    assert_eq!(str_of(&m(&mut t, &dir, "getType", vec![])), "dir");
    assert!(is_true(&m(&mut t, &dir, "isDir", vec![])));
    assert!(is_true(&m(&mut t, &dir, "isExecutable", vec![])));
    assert!(is_true(&m(&mut t, &dir, "isWritable", vec![])));
    assert_eq!(t.output(), "");
}

#[test]
fn stat_failures_throw_runtime_exceptions() {
    // SplFileInfo_get{Perms,Inode,Owner,Group}_error.phpt,
    // DirectoryIterator_getInode_error.phpt
    let d = TempDir::new("staterr");
    let missing = d.path("not_existing");
    let mut t = TestVm::new();
    let o = info(&mut t, &missing);
    for method in [
        "getPerms", "getInode", "getSize", "getOwner", "getGroup", "getATime", "getMTime",
        "getCTime", "getType",
    ] {
        let (class, msg) = thrown(try_m(&mut t, &o, method, vec![]));
        assert_eq!(class, "RuntimeException");
        // FS_TYPE is an lstat() operation
        let l = if method == "getType" { "L" } else { "" };
        assert_eq!(
            msg,
            format!("SplFileInfo::{method}(): {l}stat failed for {missing}")
        );
    }
    // The is*() checks are quiet.
    for method in [
        "isWritable",
        "isReadable",
        "isExecutable",
        "isFile",
        "isDir",
        "isLink",
    ] {
        assert!(is_false(&m(&mut t, &o, method, vec![])), "{method}");
    }
    assert_eq!(t.output(), "");
    // the error handling mode is restored afterwards
    assert!(matches!(
        t.vm.errors.mode,
        php_runtime::errors::ErrorHandlingMode::Normal
    ));
}

#[test]
fn link_target() {
    // spl_fileinfo_getlinktarget_basic.phpt
    let d = TempDir::new("link");
    let target = d.file("target.php", "<?php");
    let link = d.path("test_link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut t = TestVm::new();
    let o = info(&mut t, &link);
    assert!(is_true(&m(&mut t, &o, "isLink", vec![])));
    assert_eq!(str_of(&m(&mut t, &o, "getLinkTarget", vec![])), target);
    let o = info(&mut t, &target);
    let (class, msg) = thrown(try_m(&mut t, &o, "getLinkTarget", vec![]));
    assert_eq!(class, "RuntimeException");
    assert_eq!(
        msg,
        format!("Unable to read link {target}, error: Invalid argument")
    );
    // An empty path cannot be expanded.
    let o = info(&mut t, "");
    assert!(is_false(&m(&mut t, &o, "getLinkTarget", vec![])));
    assert_eq!(
        t.output(),
        "\nWarning: SplFileInfo::getLinkTarget(): No such file or directory in /t.php on line 3\n"
    );
}

#[test]
fn uninitialized_objects() {
    // bug79987.phpt
    let mut t = TestVm::new();
    user_class(&mut t.vm, "BadSplFileInfo", "SplFileInfo", &[&EMPTY_CTOR]);
    let x = new(&mut t, "BadSplFileInfo", vec![]);
    for method in [
        "getLinkTarget",
        "getFilename",
        "getExtension",
        "getBasename",
        "getPerms",
    ] {
        let (class, msg) = thrown(try_m(&mut t, &x, method, vec![]));
        assert_eq!(
            (class.as_str(), msg.as_str()),
            ("Error", "Object not initialized")
        );
    }
    assert_eq!(str_of(&m(&mut t, &x, "getPathname", vec![])), "");
    assert_eq!(str_of(&m(&mut t, &x, "getPath", vec![])), "");
    assert!(is_false(&m(&mut t, &x, "getRealPath", vec![])));
    assert!(m(&mut t, &x, "getPathInfo", vec![]).is_null());
    assert_eq!(
        debug_strs(&mut t, &x),
        pairs(&[("pathName:SplFileInfo", "")])
    );
    // gh7809.phpt: cloning it works
    let c = t.vm.object_cast(&x, CastKind::Bool).unwrap();
    assert!(c.unwrap().identical(&Value::Bool(true)));
    let clone = (x.class().handlers.clone_obj.unwrap())(&mut t.vm, &x).unwrap();
    assert_eq!(clone.class().name.to_string(), "BadSplFileInfo");
}

#[test]
fn file_and_info_classes() {
    // SplFileInfo_setFileClass_basic.phpt, SplFileInfo_setInfoClass_basic.phpt
    let d = TempDir::new("classes");
    let p = d.file("x.php", "<?php\n");
    let mut t = TestVm::new();
    user_class(&mut t.vm, "MyFileObject", "SplFileObject", &[]);
    user_class(&mut t.vm, "MyInfoObject", "SplFileInfo", &[]);
    let o = info(&mut t, &p);
    m(&mut t, &o, "setFileClass", vec![s("MyFileObject")]);
    let f = obj_of(&m(&mut t, &o, "openFile", vec![]));
    assert_eq!(f.class().name.to_string(), "MyFileObject");
    m(&mut t, &o, "setFileClass", vec![s("SplFileObject")]);
    let f = obj_of(&m(&mut t, &o, "openFile", vec![]));
    assert_eq!(f.class().name.to_string(), "SplFileObject");
    m(&mut t, &o, "setFileClass", vec![]);

    m(&mut t, &o, "setInfoClass", vec![s("MyInfoObject")]);
    let i = obj_of(&m(&mut t, &o, "getFileInfo", vec![]));
    assert_eq!(i.class().name.to_string(), "MyInfoObject");
    let i = obj_of(&m(&mut t, &o, "getPathInfo", vec![]));
    assert_eq!(i.class().name.to_string(), "MyInfoObject");
    assert_eq!(str_of(&m(&mut t, &i, "getPathname", vec![])), d.root());
    m(&mut t, &o, "setInfoClass", vec![s("SplFileInfo")]);
    let i = obj_of(&m(&mut t, &o, "getFileInfo", vec![]));
    assert_eq!(i.class().name.to_string(), "SplFileInfo");
    let i = obj_of(&m(&mut t, &o, "getPathInfo", vec![]));
    assert_eq!(i.class().name.to_string(), "SplFileInfo");
}

#[test]
fn class_arguments_are_checked() {
    // SplFileInfo_setFileClass_error.phpt, SplFileInfo_setInfoClass_error.phpt
    let mut t = TestVm::new();
    let o = info(&mut t, "/x");
    let (class, msg) = thrown(try_m(&mut t, &o, "setFileClass", vec![s("stdClass")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplFileInfo::setFileClass(): Argument #1 ($class) must be a class name derived from SplFileObject, \"stdClass\" given"
    );
    let (_, msg) = thrown(try_m(&mut t, &o, "setInfoClass", vec![s("stdClass")]));
    assert_eq!(
        msg,
        "SplFileInfo::setInfoClass(): Argument #1 ($class) must be a class name derived from SplFileInfo, \"stdClass\" given"
    );
    let (_, msg) = thrown(try_m(&mut t, &o, "setInfoClass", vec![s("NoSuchClass")]));
    assert_eq!(
        msg,
        "SplFileInfo::setInfoClass(): Argument #1 ($class) must be a class name derived from SplFileInfo, \"NoSuchClass\" given"
    );
    let (_, msg) = thrown(try_m(&mut t, &o, "getFileInfo", vec![s("stdClass")]));
    assert_eq!(
        msg,
        "SplFileInfo::getFileInfo(): Argument #1 ($class) must be a class name derived from SplFileInfo, \"stdClass\" given"
    );
    // gh17516.phpt (the derivation check of getPathInfo())
    let (class, msg) = thrown(try_m(&mut t, &o, "getPathInfo", vec![s("stdClass")]));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplFileInfo::getPathInfo(): Argument #1 ($class) must be a class name derived from SplFileInfo or null, stdClass given"
    );
    let (_, msg) = thrown(try_m(&mut t, &o, "getPathInfo", vec![s("NoSuchClass")]));
    assert_eq!(
        msg,
        "SplFileInfo::getPathInfo(): Argument #1 ($class) must be a valid class name or null, \"NoSuchClass\" given"
    );
    // bug46051.phpt
    let (class, msg) = thrown(try_m(
        &mut t,
        &o,
        "openFile",
        vec![s(""), Value::Bool(false), Value::empty_array()],
    ));
    assert_eq!(class, "TypeError");
    assert_eq!(
        msg,
        "SplFileInfo::openFile(): Argument #3 ($context) must be of type resource or null, array given"
    );
    assert_eq!(str_of(&m(&mut t, &o, "getPathname", vec![])), "/x");
}

#[test]
fn file_info_copies() {
    // fileobject_getfileinfo_basic.phpt
    let d = TempDir::new("fileinfo");
    let p = d.file("fileobject_getfileinfo_basic.php", "<?php\n");
    let mut t = TestVm::new();
    let f = new(&mut t, "SplFileObject", vec![s(&p)]);
    let fi = obj_of(&m(&mut t, &f, "getFileInfo", vec![]));
    assert_eq!(fi.class().name.to_string(), "SplFileInfo");
    assert_eq!(
        debug_strs(&mut t, &fi),
        pairs(&[
            ("pathName:SplFileInfo", &p),
            ("fileName:SplFileInfo", "fileobject_getfileinfo_basic.php")
        ])
    );
    assert_eq!(str_of(&m(&mut t, &fi, "__toString", vec![])), p);
    let root = d.root();
    let name = root.rsplit('/').next().unwrap().to_string();
    for dir in [root.clone(), format!("{root}/")] {
        let i = info(&mut t, &dir);
        let fi = obj_of(&m(&mut t, &i, "getFileInfo", vec![]));
        assert_eq!(
            debug_strs(&mut t, &fi),
            pairs(&[
                ("pathName:SplFileInfo", &root),
                ("fileName:SplFileInfo", &name)
            ])
        );
    }
}

#[test]
fn overridden_constructors_are_called() {
    // spl_filesystem_object_create_type / create_info call a user
    // constructor with the path (and the mode for openFile()).
    let d = TempDir::new("ctor");
    let p = d.file("a.txt", "");
    let mut t = TestVm::new();
    user_class(&mut t.vm, "RecInfo", "SplFileInfo", &[&RECORD_CTOR]);
    user_class(&mut t.vm, "RecFile", "SplFileObject", &[&RECORD_CTOR]);
    let o = info(&mut t, &p);
    let i = obj_of(&m(&mut t, &o, "getFileInfo", vec![s("RecInfo")]));
    let seen = |o: &php_runtime::ObjRef| -> Vec<String> {
        crate::spl::testutil::entries(&o.get_prop(b"seen").unwrap())
            .into_iter()
            .map(|(_, v)| match v {
                Value::String(s) => s.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    };
    assert_eq!(seen(&i), vec![p.clone()]);
    let i = obj_of(&m(&mut t, &o, "getPathInfo", vec![s("RecInfo")]));
    assert_eq!(seen(&i), [d.root()]);
    m(&mut t, &o, "setFileClass", vec![s("RecFile")]);
    let f = obj_of(&m(&mut t, &o, "openFile", vec![s("r+")]));
    assert_eq!(seen(&f), [p.clone(), "r+".to_string()]);
}

#[test]
fn real_path_and_path_info() {
    // bug48361.phpt
    let d = TempDir::new("realpath");
    let p = d.file("bug48361.php", "");
    let mut t = TestVm::new();
    let o = info(&mut t, &format!("{}/./bug48361.php", d.root()));
    assert_eq!(str_of(&m(&mut t, &o, "getRealPath", vec![])), p);
    let pi = obj_of(&m(&mut t, &o, "getPathInfo", vec![]));
    assert_eq!(str_of(&m(&mut t, &pi, "getRealPath", vec![])), d.root());
    let o = info(&mut t, &d.path("missing"));
    assert!(is_false(&m(&mut t, &o, "getRealPath", vec![])));
}

#[test]
fn string_and_bool_casts() {
    // bug62328.phpt, bug64023.phpt, bug65213.phpt: no cast_object handler,
    // so a userland __toString() wins.
    let mut t = TestVm::new();
    assert!(
        t.vm.find_class(b"SplFileInfo")
            .unwrap()
            .handlers
            .cast_object
            .is_none()
    );
    let o = info(&mut t, "/some/file");
    let v = t.vm.object_cast(&o, CastKind::String).unwrap().unwrap();
    assert_eq!(str_of(&v), "/some/file");
    user_class(
        &mut t.vm,
        "SplFileInfo62328",
        "SplFileInfo",
        &[&TO_STRING_OVERRIDE],
    );
    let o = new(&mut t, "SplFileInfo62328", vec![s("/")]);
    let v = t.vm.object_cast(&o, CastKind::String).unwrap().unwrap();
    assert_eq!(str_of(&v), "__toString");
    let o = info(&mut t, ".");
    let v = t.vm.object_cast(&o, CastKind::Bool).unwrap().unwrap();
    assert!(v.identical(&Value::Bool(true)));
}

#[test]
fn cloning_info_objects() {
    // fileobject_003.phpt (the SplFileInfo part)
    let d = TempDir::new("clone");
    let p = d.file("fileobject_001a.txt", "0\n");
    let mut t = TestVm::new();
    let o = info(&mut t, &p);
    o.set_prop(b"dyn", Value::Int(7));
    let c = (o.class().handlers.clone_obj.unwrap())(&mut t.vm, &o).unwrap();
    assert!(!php_runtime::ObjRef::ptr_eq(&o, &c));
    assert_eq!(debug(&mut t, &o).len(), debug(&mut t, &c).len());
    assert_eq!(debug_strs(&mut t, &c), debug_strs(&mut t, &o));
    assert!(c.get_prop(b"dyn").unwrap().identical(&Value::Int(7)));
    assert_eq!(
        str_of(&m(&mut t, &c, "getPathname", vec![])),
        str_of(&m(&mut t, &o, "getPathname", vec![]))
    );
}

#[test]
fn debug_info_includes_properties() {
    // bug69264.phpt: parent::__debugInfo() has the two path entries
    let mut t = TestVm::new();
    let o = info(&mut t, "/a/b");
    o.set_prop(b"extra", Value::Int(1));
    let d = debug(&mut t, &o);
    assert_eq!(d[0].0, "extra");
    assert_eq!(d.len(), 3);
}

#[test]
fn bad_state_ex_is_deprecated() {
    let mut t = TestVm::new();
    let o = info(&mut t, "/x");
    let (class, msg) = thrown(try_m(&mut t, &o, "_bad_state_ex", vec![]));
    assert_eq!(class, "Error");
    assert_eq!(
        msg,
        "The parent constructor was not called: the object is in an invalid state"
    );
    assert_eq!(
        t.output(),
        "\nDeprecated: Method SplFileInfo::_bad_state_ex() is deprecated since 8.2 in /t.php on line 3\n"
    );
}
