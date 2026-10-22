//! Tests of spl_directory.c, porting the expectations of
//! ext/spl/tests/{SplFileInfo_*,SplFileinfo_*,spl_fileinfo_*,
//! DirectoryIterator_*,dit_*,filesystemiterator_*,
//! RecursiveDirectoryIterator_*,GlobIterator_*,SplTempFileObject_*,
//! SplFileObject/*,bug*,gh*}.phpt on real files in scratch directories.

mod dir;
mod file;
mod info;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use php_runtime::{ObjRef, PResult, Value};

use crate::spl::testutil::{call, new_obj};
use crate::testing::TestVm;

/// A unique scratch directory, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("php-spl-dir-{}-{tag}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        // canonical, so that realpath() results compare equal
        TempDir(fs::canonicalize(&d).unwrap())
    }

    /// The directory itself.
    pub fn root(&self) -> String {
        self.0.to_str().unwrap().to_string()
    }

    /// `root/name`.
    pub fn path(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_string()
    }

    /// Writes `root/name`.
    pub fn file(&self, name: &str, contents: &str) -> String {
        let p = self.path(name);
        fs::write(&p, contents).unwrap();
        p
    }

    /// Creates `root/name` (and parents).
    pub fn dir(&self, name: &str) -> String {
        let p = self.path(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// `new $class(...$args)`, unwrapped.
pub fn new(t: &mut TestVm, class: &str, args: Vec<Value>) -> ObjRef {
    new_obj(&mut t.vm, class, args).unwrap_or_else(|e| panic!("new {class}: {e:?}"))
}

/// `new $class(...$args)`.
pub fn try_new(t: &mut TestVm, class: &str, args: Vec<Value>) -> PResult<Value> {
    new_obj(&mut t.vm, class, args).map(Value::Object)
}

/// `$obj->$name(...$args)`, unwrapped.
pub fn m(t: &mut TestVm, obj: &ObjRef, name: &str, args: Vec<Value>) -> Value {
    call(&mut t.vm, obj, name, args)
        .unwrap_or_else(|e| panic!("{}::{name}(): {e:?}", obj.class().name))
}

/// `$obj->$name(...$args)`.
pub fn try_m(t: &mut TestVm, obj: &ObjRef, name: &str, args: Vec<Value>) -> PResult<Value> {
    call(&mut t.vm, obj, name, args)
}

/// The string inside a value (panics otherwise).
pub fn str_of(v: &Value) -> String {
    match v.unref() {
        Value::String(s) => s.to_string_lossy().into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

/// The object inside a value (panics otherwise).
pub fn obj_of(v: &Value) -> ObjRef {
    match v.unref() {
        Value::Object(o) => o,
        other => panic!("expected an object, got {other:?}"),
    }
}

pub fn is_false(v: &Value) -> bool {
    v.identical(&Value::Bool(false))
}

pub fn is_true(v: &Value) -> bool {
    v.identical(&Value::Bool(true))
}

/// `__debugInfo()` as `("prop:Class", value)` pairs (private names
/// unmangled, `var_dump` style).
pub fn debug(t: &mut TestVm, obj: &ObjRef) -> Vec<(String, Value)> {
    let table = m(t, obj, "__debugInfo", vec![]);
    crate::spl::testutil::array_of(&table)
        .iter()
        .map(|(k, v)| {
            let key = k.to_php_str().to_vec();
            let name = match key.strip_prefix(b"\0") {
                Some(rest) => {
                    let i = rest.iter().position(|&c| c == 0).unwrap();
                    format!(
                        "{}:{}",
                        String::from_utf8_lossy(&rest[i + 1..]),
                        String::from_utf8_lossy(&rest[..i])
                    )
                }
                None => String::from_utf8_lossy(&key).into_owned(),
            };
            (name, v.clone())
        })
        .collect()
}

/// [`debug`] with string values only (`bool(false)` as `"false"`).
pub fn debug_strs(t: &mut TestVm, obj: &ObjRef) -> Vec<(String, String)> {
    debug(t, obj)
        .into_iter()
        .map(|(k, v)| {
            let v = match v.unref() {
                Value::String(s) => s.to_string_lossy().into_owned(),
                Value::Bool(false) => "false".to_string(),
                other => format!("{other:?}"),
            };
            (k, v)
        })
        .collect()
}

/// Owned `(key, value)` string pairs for comparisons.
pub fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    p.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn signatures_parse() {
    let t = TestVm::new();
    for class in [
        "SplFileInfo",
        "DirectoryIterator",
        "FilesystemIterator",
        "RecursiveDirectoryIterator",
        "GlobIterator",
        "SplFileObject",
        "SplTempFileObject",
    ] {
        let ce = t.vm.find_class(class.as_bytes()).unwrap();
        for f in ce.methods.values() {
            let _ = f.signature();
        }
    }
}

#[test]
fn class_hierarchy_and_interfaces() {
    let t = TestVm::new();
    let ce = |n: &str| t.vm.find_class(n.as_bytes()).unwrap();
    let parent = |n: &str| ce(n).parent.as_ref().unwrap().name.to_string();
    assert_eq!(parent("DirectoryIterator"), "SplFileInfo");
    assert_eq!(parent("FilesystemIterator"), "DirectoryIterator");
    assert_eq!(parent("RecursiveDirectoryIterator"), "FilesystemIterator");
    assert_eq!(parent("GlobIterator"), "FilesystemIterator");
    assert_eq!(parent("SplFileObject"), "SplFileInfo");
    assert_eq!(parent("SplTempFileObject"), "SplFileObject");
    let ifaces = |n: &str| -> Vec<String> {
        ce(n)
            .interfaces
            .iter()
            .map(|i| i.name.to_string())
            .collect()
    };
    assert_eq!(ifaces("SplFileInfo"), ["Stringable"]);
    assert_eq!(
        ifaces("DirectoryIterator"),
        ["Stringable", "SeekableIterator", "Traversable", "Iterator"]
    );
    assert!(ifaces("RecursiveDirectoryIterator").contains(&"RecursiveIterator".to_string()));
    assert!(ifaces("GlobIterator").contains(&"Countable".to_string()));
    let sfo = ifaces("SplFileObject");
    assert!(sfo.contains(&"RecursiveIterator".to_string()));
    assert!(sfo.contains(&"SeekableIterator".to_string()));
    for c in ["SplFileInfo", "DirectoryIterator", "SplFileObject"] {
        assert!(ce(c).has_flag(php_runtime::class::class_flags::NOT_SERIALIZABLE));
    }
    // FilesystemIterator / SplFileObject constants
    for (class, name, value) in [
        ("FilesystemIterator", "CURRENT_MODE_MASK", 0xF0),
        ("FilesystemIterator", "CURRENT_AS_PATHNAME", 0x20),
        ("FilesystemIterator", "CURRENT_AS_FILEINFO", 0),
        ("FilesystemIterator", "CURRENT_AS_SELF", 0x10),
        ("FilesystemIterator", "KEY_MODE_MASK", 0xF00),
        ("FilesystemIterator", "KEY_AS_PATHNAME", 0),
        ("FilesystemIterator", "FOLLOW_SYMLINKS", 0x4000),
        ("FilesystemIterator", "KEY_AS_FILENAME", 0x100),
        ("FilesystemIterator", "NEW_CURRENT_AND_KEY", 0x100),
        ("FilesystemIterator", "OTHER_MODE_MASK", 0x7000),
        ("FilesystemIterator", "SKIP_DOTS", 0x1000),
        ("FilesystemIterator", "UNIX_PATHS", 0x2000),
        ("GlobIterator", "SKIP_DOTS", 0x1000),
        ("SplFileObject", "DROP_NEW_LINE", 1),
        ("SplFileObject", "READ_AHEAD", 2),
        ("SplFileObject", "SKIP_EMPTY", 4),
        ("SplFileObject", "READ_CSV", 8),
        ("SplTempFileObject", "READ_CSV", 8),
    ] {
        let c = ce(class).find_constant(name.as_bytes()).cloned().unwrap();
        assert!(
            c.evaluated().unwrap().identical(&Value::Int(value)),
            "{class}::{name}"
        );
    }
    // _bad_state_ex is final; the CSV methods are deprecated
    let f = ce("SplFileInfo")
        .find_method(b"_bad_state_ex")
        .cloned()
        .unwrap();
    assert!(f.has_flag(php_runtime::class::member_flags::FINAL));
    let f = ce("SplFileObject")
        .find_method(b"fgetcsv")
        .cloned()
        .unwrap();
    assert!(f.has_flag(php_runtime::class::fn_flags::DEPRECATED));
}
