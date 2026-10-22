//! ext/standard/dir.c (+ the `Directory` class)

#![allow(clippy::collapsible_if)]

use php_runtime::class::{ClassEntry, Function, PropertyInfo, class_flags, member_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, ResRef, Value, Vm,
};

use super::streams::glob as g;
use super::streams::stream::{flags, open};
use super::streams::{context_param, handle_of, path_param, plain, sys, with_stream, wrappers};

pub const SCANDIR_SORT_ASCENDING: i64 = 0;
pub const SCANDIR_SORT_DESCENDING: i64 = 1;
pub const SCANDIR_SORT_NONE: i64 = 2;

/// `DIRG(default_dir)`: the last directory opened with opendir()/dir().
#[derive(Default)]
struct DirGlobals {
    default_dir: Option<ResRef>,
}

fn set_default_dir(vm: &mut Vm, res: Option<ResRef>) {
    vm.ext_data::<DirGlobals>().default_dir = res;
}

/// `_php_do_opendir`.
fn do_opendir(vm: &mut Vm, args: &mut Args, create_object: bool) -> PResult<Value> {
    let dirname = path_param(vm, args, 0)?;
    let ctx = context_param(vm, args, 1, false)?;
    let Some(res) = wrappers::opendir(vm, &dirname, open::REPORT_ERRORS, ctx.as_ref())? else {
        return Ok(Value::Bool(false));
    };
    if let Some(h) = handle_of(&res) {
        h.borrow_mut().st.flags |= flags::NO_FCLOSE;
    }
    set_default_dir(vm, Some(res.clone()));
    if create_object {
        let ce = vm.find_class(b"Directory").expect("Directory registered");
        let obj = vm.instantiate(&ce)?;
        obj.set_prop(b"path", Value::String(dirname));
        obj.set_prop(b"handle", Value::Resource(res));
        return Ok(Value::Object(obj));
    }
    Ok(Value::Resource(res))
}

fn opendir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_opendir(vm, args, false)
}

fn dir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_opendir(vm, args, true)
}

fn is_dir_stream(res: &ResRef) -> bool {
    handle_of(res).is_some_and(|h| {
        h.try_borrow()
            .is_ok_and(|s| s.st.flags & flags::IS_DIR != 0)
    })
}

/// `php_dir_get_directory_stream_from_user_arg`.
fn dir_from_user_arg(vm: &mut Vm, args: &Args) -> PResult<ResRef> {
    let v = args.get(0).unref();
    let res = match &v {
        Value::Null | Value::Undef => {
            vm.deprecated(
                "Passing null is deprecated, instead the last opened directory stream should be provided",
            )?;
            match vm.ext_data::<DirGlobals>().default_dir.clone() {
                Some(r) if handle_of(&r).is_some() => r,
                _ => return Err(vm.throw_type_error("No resource supplied")),
            }
        }
        _ => super::streams::stream_arg(vm, &v, 1)?,
    };
    if !is_dir_stream(&res) {
        return Err(vm.argument_type_error(1, "must be a valid Directory resource"));
    }
    Ok(res)
}

/// `php_dir_get_directory_stream_from_this`.
fn dir_from_this(vm: &mut Vm, args: &Args) -> PResult<ResRef> {
    let this = args
        .this()
        .cloned()
        .expect("Directory method without $this");
    let Some(Value::Resource(res)) = this.get_prop(b"handle").map(|v| v.unref()) else {
        return Err(vm.throw_error("Error", "Internal directory stream has been altered"));
    };
    if handle_of(&res).is_none() {
        let f = vm.current_function_name();
        return Err(vm.throw_type_error(format!(
            "Directory::{f}(): cannot use Directory resource after it has been closed"
        )));
    }
    if !is_dir_stream(&res) {
        return Err(vm.throw_error("Error", "Internal directory stream has been altered"));
    }
    Ok(res)
}

fn close_dir(vm: &mut Vm, res: &ResRef) -> PResult<()> {
    with_stream(vm, res, |vm, s| {
        s.free(vm, super::streams::stream::PHP_STREAM_FREE_CLOSE)
    })?;
    res.close();
    let g = vm.ext_data::<DirGlobals>();
    if g.default_dir
        .as_ref()
        .is_some_and(|d| ResRef::ptr_eq(d, res))
    {
        g.default_dir = None;
    }
    Ok(())
}

fn read_dir(vm: &mut Vm, res: &ResRef) -> PResult<Value> {
    Ok(
        match with_stream(vm, res, |vm, s| s.readdir(vm))?.flatten() {
            Some(name) => Value::from(name),
            None => Value::Bool(false),
        },
    )
}

fn rewind_dir(vm: &mut Vm, res: &ResRef) -> PResult<()> {
    with_stream(vm, res, |vm, s| s.rewind(vm))?;
    Ok(())
}

fn closedir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_user_arg(vm, args)?;
    close_dir(vm, &res)?;
    Ok(Value::Null)
}

fn rewinddir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_user_arg(vm, args)?;
    rewind_dir(vm, &res)?;
    Ok(Value::Null)
}

fn readdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_user_arg(vm, args)?;
    read_dir(vm, &res)
}

fn directory_close(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_this(vm, args)?;
    close_dir(vm, &res)?;
    Ok(Value::Null)
}

fn directory_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_this(vm, args)?;
    rewind_dir(vm, &res)?;
    Ok(Value::Null)
}

fn directory_read(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let res = dir_from_this(vm, args)?;
    read_dir(vm, &res)
}

fn directory_construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "Error",
        "Cannot directly construct Directory, use dir() instead",
    ))
}

fn directory_clone(vm: &mut Vm, obj: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            obj.class().name
        ),
    ))
}

fn errno_warning(vm: &mut Vm, e: &std::io::Error) -> PResult<()> {
    let n = sys::errno_of(e);
    vm.warning(format!("{} (errno {n})", sys::strerror(n)))
}

fn chroot(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let p = path_param(vm, args, 0)?;
    if let Err(e) = std::os::unix::fs::chroot(plain::os(&p)) {
        errno_warning(vm, &e)?;
        return Ok(Value::Bool(false));
    }
    plain::clear_stat_cache(vm);
    if let Err(e) = std::env::set_current_dir("/") {
        errno_warning(vm, &e)?;
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(true))
}

fn chdir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let p = path_param(vm, args, 0)?;
    if plain::check_open_basedir(vm, &p, true)? {
        return Ok(Value::Bool(false));
    }
    if let Err(e) = std::env::set_current_dir(plain::os(&p)) {
        errno_warning(vm, &e)?;
        return Ok(Value::Bool(false));
    }
    // relative stat-cache entries are no longer valid
    let c = vm.ext_data::<plain::StatCache>();
    if c.stat.as_ref().is_some_and(|(p, _)| !p.starts_with(b"/")) {
        c.stat = None;
    }
    if c.lstat.as_ref().is_some_and(|(p, _)| !p.starts_with(b"/")) {
        c.lstat = None;
    }
    Ok(Value::Bool(true))
}

fn getcwd(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match plain::getcwd_bytes() {
        Some(c) => Value::from(c),
        None => Value::Bool(false),
    })
}

const MAXPATHLEN: usize = g::MAXPATHLEN;

fn glob(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pattern = path_param(vm, args, 0)?;
    let flags = args.int(1);
    if pattern.len() >= MAXPATHLEN {
        vm.warning(format!(
            "Pattern exceeds the maximum allowed length of {MAXPATHLEN} characters"
        ))?;
        return Ok(Value::Bool(false));
    }
    if (g::GLOB_AVAILABLE_FLAGS as i64 & flags) != flags {
        vm.warning(
            "At least one of the passed flags is invalid or not supported on this platform",
        )?;
        return Ok(Value::Bool(false));
    }
    let passed = (flags as i32) & !g::GLOB_ONLYDIR;
    let paths = match g::php_glob(&pattern, passed) {
        Ok(r) => r.paths,
        Err((code, _)) if code == g::GLOB_NOMATCH => return Ok(Value::empty_array()),
        Err(_) => return Ok(Value::Bool(false)),
    };
    if paths.is_empty() {
        return Ok(Value::empty_array());
    }
    let mut out = PhpArray::new();
    let mut basedir_limit = false;
    let has_obd = vm.ini_get("open_basedir").is_some_and(|v| !v.is_empty());
    for p in paths {
        if has_obd && plain::check_open_basedir(vm, &p, false)? {
            basedir_limit = true;
            continue;
        }
        if flags as i32 & g::GLOB_ONLYDIR != 0 && !g::glob_is_dir(&p) {
            continue;
        }
        let _ = out.append(Value::from(p));
    }
    if basedir_limit && out.is_empty() {
        return Ok(Value::Bool(false));
    }
    Ok(Value::from(out))
}

/// `php_stream_scandir`.
pub fn scandir_entries(
    vm: &mut Vm,
    dir: &[u8],
    ctx: Option<&ResRef>,
) -> PResult<Option<Vec<Vec<u8>>>> {
    let Some(res) = wrappers::opendir(vm, dir, open::REPORT_ERRORS, ctx)? else {
        return Ok(None);
    };
    let mut names = Vec::new();
    while let Some(n) = with_stream(vm, &res, |vm, s| s.readdir(vm))?.flatten() {
        names.push(n);
    }
    with_stream(vm, &res, |vm, s| {
        s.free(vm, super::streams::stream::PHP_STREAM_FREE_CLOSE)
    })?;
    res.close();
    Ok(Some(names))
}

fn scandir(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dirn = path_param(vm, args, 0)?;
    let flags = args.int(1);
    if dirn.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let ctx = if args.is_null(2) {
        None
    } else {
        context_param(vm, args, 2, false)?
    };
    if !matches!(
        flags,
        SCANDIR_SORT_ASCENDING | SCANDIR_SORT_DESCENDING | SCANDIR_SORT_NONE
    ) {
        return Err(vm.argument_value_error(
            2,
            "must be one of the SCANDIR_SORT_ASCENDING, SCANDIR_SORT_DESCENDING, or SCANDIR_SORT_NONE constants",
        ));
    }
    let Some(mut names) = scandir_entries(vm, &dirn, ctx.as_ref())? else {
        let n = wrappers::globals(vm).last_errno;
        vm.warning(format!("(errno {n}): {}", sys::strerror(n)))?;
        return Ok(Value::Bool(false));
    };
    // strcoll in the "C" collation locale is a byte comparison
    match flags {
        SCANDIR_SORT_ASCENDING => names.sort(),
        SCANDIR_SORT_DESCENDING => names.sort_by(|a, b| b.cmp(a)),
        _ => {}
    }
    Ok(Value::from(PhpArray::from_values(
        names.into_iter().map(Value::from).collect(),
    )))
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
        const ALL: &[&NativeFunction] = &[$(&$name),*];
    };
}

natives! {
    OPENDIR: "opendir(string $directory, $context = null)" => opendir;
    DIR: "dir(string $directory, $context = null): Directory|false" => dir;
    CLOSEDIR: "closedir($dir_handle = null): void" => closedir;
    CHDIR: "chdir(string $directory): bool" => chdir;
    CHROOT: "chroot(string $directory): bool" => chroot;
    GETCWD: "getcwd(): string|false" => getcwd;
    REWINDDIR: "rewinddir($dir_handle = null): void" => rewinddir;
    READDIR: "readdir($dir_handle = null): string|false" => readdir;
    SCANDIR: "scandir(string $directory, int $sorting_order = SCANDIR_SORT_ASCENDING, $context = null): array|false" => scandir;
    GLOB: "glob(string $pattern, int $flags = 0): array|false" => glob;
}

static DIRECTORY_CLOSE: NativeFunction = NativeFunction {
    sig: "public function close(): void",
    handler: directory_close,
};
static DIRECTORY_REWIND: NativeFunction = NativeFunction {
    sig: "public function rewind(): void",
    handler: directory_rewind,
};
static DIRECTORY_READ: NativeFunction = NativeFunction {
    sig: "public function read(): string|false",
    handler: directory_read,
};
static DIRECTORY_CONSTRUCT: NativeFunction = NativeFunction {
    sig: "private function __construct()",
    handler: directory_construct,
};

fn register_directory_class(r: &mut Registry) {
    use member_flags::*;
    let mut ce = ClassEntry::new(
        "Directory",
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    for (name, ty) in [("path", "string"), ("handle", "mixed")] {
        let mut p = PropertyInfo::new(name, "Directory", PUBLIC | READONLY);
        p.ty = Some(TypeDecl::parse(ty).expect("valid type"));
        p.default = Value::Undef;
        ce.declare_property(p);
    }
    for nf in [&DIRECTORY_CLOSE, &DIRECTORY_REWIND, &DIRECTORY_READ] {
        ce.add_method(Function::native_method(nf, "Directory", PUBLIC));
    }
    // `get_constructor` throws for `new Directory()` (dir_class_get_constructor)
    ce.add_method(Function::native_method(
        &DIRECTORY_CONSTRUCT,
        "Directory",
        PRIVATE,
    ));
    ce.handlers.clone_obj = Some(directory_clone);
    r.class(ce);
}

pub fn register(r: &mut Registry) {
    r.constant("DIRECTORY_SEPARATOR", "/");
    r.constant("PATH_SEPARATOR", ":");
    for (n, v) in [
        ("GLOB_BRACE", g::GLOB_BRACE),
        ("GLOB_ERR", g::GLOB_ERR),
        ("GLOB_MARK", g::GLOB_MARK),
        ("GLOB_NOCHECK", g::GLOB_NOCHECK),
        ("GLOB_NOESCAPE", g::GLOB_NOESCAPE),
        ("GLOB_NOSORT", g::GLOB_NOSORT),
        ("GLOB_ONLYDIR", g::GLOB_ONLYDIR),
        ("GLOB_AVAILABLE_FLAGS", g::GLOB_AVAILABLE_FLAGS),
    ] {
        r.constant(n, v as i64);
    }
    r.constant("SCANDIR_SORT_ASCENDING", SCANDIR_SORT_ASCENDING);
    r.constant("SCANDIR_SORT_DESCENDING", SCANDIR_SORT_DESCENDING);
    r.constant("SCANDIR_SORT_NONE", SCANDIR_SORT_NONE);
    register_directory_class(r);
    r.functions(ALL);
}

/// Exposed for `Directory`-like users: the directory path of a `glob://`
/// stream (`_php_glob_stream_get_path`).
pub fn glob_stream_path(res: &ResRef) -> Option<PhpStr> {
    let h = handle_of(res)?;
    let mut s = h.borrow_mut();
    s.ops_as::<super::streams::glob_wrapper::GlobOps>()
        .and_then(|o| o.path.clone())
        .map(PhpStr::from_vec)
}

#[cfg(test)]
mod tests {
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;

    fn scratch(tag: &str) -> String {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "php-rs-dir-{}-{}-{tag}",
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

    fn strs(v: &Value) -> Vec<String> {
        v.as_array()
            .unwrap()
            .values()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn opendir_readdir_rewind_closedir() {
        let mut t = TestVm::new();
        let d = scratch("rd");
        for f in ["b", "a", "c"] {
            std::fs::write(format!("{d}/{f}"), "").unwrap();
        }
        let h = t.call("opendir", vec![Value::str(&d)]).unwrap();
        let mut names = Vec::new();
        while let Value::String(s) = t.call("readdir", vec![h.clone()]).unwrap() {
            names.push(s.to_string());
        }
        names.sort();
        assert_eq!(names, [".", "..", "a", "b", "c"]);
        t.call("rewinddir", vec![h.clone()]).unwrap();
        assert_eq!(
            t.call("readdir", vec![h.clone()])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string(),
            "."
        );
        // fclose refuses directory handles
        assert_eq!(
            t.call("fclose", vec![h.clone()]).unwrap().as_bool(),
            Some(false)
        );
        assert_eq!(
            t.output(),
            "\nWarning: fclose(): cannot close the provided stream, as it must not be manually closed in /t.php on line 3\n"
        );
        t.call("closedir", vec![h.clone()]).unwrap();
        let (cls, msg) = thrown(t.call("readdir", vec![h]));
        assert_eq!(cls, "TypeError");
        assert_eq!(
            msg,
            "readdir(): Argument #1 ($dir_handle) must be an open stream resource"
        );
    }

    #[test]
    fn default_dir_handle_is_deprecated() {
        let mut t = TestVm::new();
        let d = scratch("def");
        t.call("opendir", vec![Value::str(&d)]).unwrap();
        let r = t.call("readdir", vec![]).unwrap();
        assert!(r.as_str().is_some());
        assert_eq!(
            t.output(),
            "\nDeprecated: readdir(): Passing null is deprecated, instead the last opened directory stream should be provided in /t.php on line 3\n"
        );
        let fp = t
            .call("fopen", vec![Value::str("php://memory"), Value::str("r")])
            .unwrap();
        let (cls, msg) = thrown(t.call("readdir", vec![fp]));
        assert_eq!(cls, "TypeError");
        assert_eq!(
            msg,
            "readdir(): Argument #1 ($dir_handle) must be a valid Directory resource"
        );
    }

    #[test]
    fn opendir_failure_warns() {
        let mut t = TestVm::new();
        let d = scratch("fail");
        let r = t
            .call("opendir", vec![Value::str(&format!("{d}/nope"))])
            .unwrap();
        assert_eq!(r.as_bool(), Some(false));
        assert_eq!(
            t.output(),
            "\nWarning: opendir(): Failed to open directory: No such file or directory in /t.php on line 3\n"
        );
        let r = t
            .call("scandir", vec![Value::str(&format!("{d}/nope"))])
            .unwrap();
        assert_eq!(r.as_bool(), Some(false));
        assert_eq!(
            t.output(),
            "\nWarning: scandir(): Failed to open directory: No such file or directory in /t.php on line 3\n\nWarning: scandir(): (errno 2): No such file or directory in /t.php on line 3\n"
        );
    }

    #[test]
    fn scandir_sorting() {
        let mut t = TestVm::new();
        let d = scratch("scan");
        for f in ["B", "a", "c"] {
            std::fs::write(format!("{d}/{f}"), "").unwrap();
        }
        let r = t.call("scandir", vec![Value::str(&d)]).unwrap();
        assert_eq!(strs(&r), [".", "..", "B", "a", "c"]);
        let r = t
            .call("scandir", vec![Value::str(&d), Value::Int(1)])
            .unwrap();
        assert_eq!(strs(&r), ["c", "a", "B", "..", "."]);
        let (cls, msg) = thrown(t.call("scandir", vec![Value::str(&d), Value::Int(7)]));
        assert_eq!(cls, "ValueError");
        assert_eq!(
            msg,
            "scandir(): Argument #2 ($sorting_order) must be one of the SCANDIR_SORT_ASCENDING, SCANDIR_SORT_DESCENDING, or SCANDIR_SORT_NONE constants"
        );
        let (cls, msg) = thrown(t.call("scandir", vec![Value::str("")]));
        assert_eq!(cls, "ValueError");
        assert_eq!(msg, "scandir(): Argument #1 ($directory) must not be empty");
    }

    #[test]
    fn dir_object_and_methods() {
        let mut t = TestVm::new();
        let d = scratch("obj");
        std::fs::write(format!("{d}/x"), "").unwrap();
        let o = t.call("dir", vec![Value::str(&d)]).unwrap();
        let Value::Object(obj) = &o else {
            panic!("{o:?}")
        };
        assert_eq!(obj.class().name.to_string(), "Directory");
        assert_eq!(
            obj.get_prop(b"path").unwrap().as_str().unwrap().to_string(),
            d
        );
        let read = obj.class().find_method(b"read").cloned().unwrap();
        let call = |t: &mut TestVm, m: &std::rc::Rc<php_runtime::Function>| {
            t.vm.call_function(
                m,
                php_runtime::CallArgs {
                    this: Some(obj.clone()),
                    ..php_runtime::CallArgs::default()
                },
            )
        };
        let mut n = 0;
        while call(&mut t, &read).unwrap().as_str().is_some() {
            n += 1;
        }
        assert_eq!(n, 3);
        let close = obj.class().find_method(b"close").cloned().unwrap();
        call(&mut t, &close).unwrap();
        let (cls, msg) = thrown(call(&mut t, &read));
        assert_eq!(cls, "TypeError");
        assert_eq!(
            msg,
            "Directory::read(): cannot use Directory resource after it has been closed"
        );
    }

    #[test]
    fn glob_function_and_flags() {
        let mut t = TestVm::new();
        let d = scratch("glob");
        std::fs::create_dir(format!("{d}/sub")).unwrap();
        std::fs::write(format!("{d}/a.txt"), "").unwrap();
        std::fs::write(format!("{d}/b.txt"), "").unwrap();
        let r = t
            .call("glob", vec![Value::str(&format!("{d}/*.txt"))])
            .unwrap();
        assert_eq!(strs(&r), [format!("{d}/a.txt"), format!("{d}/b.txt")]);
        let onlydir = 1i64 << 30;
        let r = t
            .call(
                "glob",
                vec![Value::str(&format!("{d}/*")), Value::Int(onlydir)],
            )
            .unwrap();
        assert_eq!(strs(&r), [format!("{d}/sub")]);
        let r = t
            .call("glob", vec![Value::str(&format!("{d}/*")), Value::Int(8)])
            .unwrap();
        assert_eq!(strs(&r)[2], format!("{d}/sub/"));
        let r = t
            .call("glob", vec![Value::str(&format!("{d}/zzz*"))])
            .unwrap();
        assert_eq!(r.as_array().unwrap().len(), 0);
        let r = t
            .call("glob", vec![Value::str("x"), Value::Int(1 << 20)])
            .unwrap();
        assert_eq!(r.as_bool(), Some(false));
        assert_eq!(
            t.output(),
            "\nWarning: glob(): At least one of the passed flags is invalid or not supported on this platform in /t.php on line 3\n"
        );
        // glob:// directory streams
        let h = t
            .call("opendir", vec![Value::str(&format!("glob://{d}/*.txt"))])
            .unwrap();
        let mut names = Vec::new();
        while let Value::String(s) = t.call("readdir", vec![h.clone()]).unwrap() {
            names.push(s.to_string());
        }
        assert_eq!(names, ["a.txt", "b.txt"]);
    }

    #[test]
    fn getcwd_returns_a_path() {
        let mut t = TestVm::new();
        let r = t.call("getcwd", vec![]).unwrap();
        assert!(r.as_str().unwrap().as_bytes().starts_with(b"/"));
        let r = t
            .call("chdir", vec![Value::str("/definitely/not/here")])
            .unwrap();
        assert_eq!(r.as_bool(), Some(false));
        assert_eq!(
            t.output(),
            "\nWarning: chdir(): No such file or directory (errno 2) in /t.php on line 3\n"
        );
    }
}
