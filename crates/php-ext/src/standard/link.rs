//! ext/standard/link.c — `readlink`, `linkinfo`, `symlink` and `link`.

use std::os::unix::ffi::OsStrExt;

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use super::streams::path_param;
use super::streams::plain::{self, check_open_basedir};
use super::streams::stream::open;
use super::streams::sys;
use super::streams::wrappers::locate_url_wrapper;

/// `MAXPATHLEN`: `readlink()` reads at most `MAXPATHLEN - 1` bytes.
#[cfg(target_os = "linux")]
const MAXPATHLEN: usize = 4096;
#[cfg(not(target_os = "linux"))]
const MAXPATHLEN: usize = 1024;

fn warn_errno(vm: &mut Vm, e: &std::io::Error) -> PResult<()> {
    vm.warning(sys::strerror(sys::errno_of(e)))
}

/// `zend_dirname`: the directory part of `path` (Unix slashes only).
pub fn zend_dirname(path: &[u8]) -> Vec<u8> {
    if path.is_empty() {
        // Illegal use of this function
        return Vec::new();
    }
    // Strip trailing slashes
    let Some(end) = path.iter().rposition(|&c| c != b'/') else {
        // The path only contained slashes
        return b"/".to_vec();
    };
    // Strip filename
    let Some(end) = path[..=end].iter().rposition(|&c| c == b'/') else {
        // No slash found, therefore return '.'
        return b".".to_vec();
    };
    // Strip slashes which came before the file name
    match path[..=end].iter().rposition(|&c| c != b'/') {
        Some(end) => path[..=end].to_vec(),
        None => b"/".to_vec(),
    }
}

/// `expand_filepath_ex(path, relative_to)`: like `expand_filepath`, with a
/// relative `path` taken relative to `relative_to` instead of the CWD.
fn expand_filepath_ex(path: &[u8], relative_to: &[u8]) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    if path.starts_with(b"/") {
        return plain::expand_filepath(path);
    }
    let mut abs = relative_to.to_vec();
    abs.push(b'/');
    abs.extend_from_slice(path);
    plain::expand_filepath(&abs)
}

/// `php_stream_locate_url_wrapper(path, NULL, STREAM_LOCATE_WRAPPERS_ONLY)`
/// is non-NULL: `path` names a stream wrapper other than plain files.
fn is_url(vm: &mut Vm, path: &[u8]) -> PResult<bool> {
    Ok(locate_url_wrapper(vm, path, open::STREAM_LOCATE_WRAPPERS_ONLY)?.is_some())
}

fn readlink(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let link = path_param(vm, args, 0)?;
    if check_open_basedir(vm, link.as_bytes(), true)? {
        return Ok(Value::Bool(false));
    }
    match std::fs::read_link(plain::os(link.as_bytes())) {
        Ok(target) => {
            let bytes = target.as_os_str().as_bytes();
            // php_sys_readlink(link, buff, MAXPATHLEN-1)
            let n = bytes.len().min(MAXPATHLEN - 1);
            Ok(Value::from(&bytes[..n]))
        }
        Err(e) => {
            warn_errno(vm, &e)?;
            Ok(Value::Bool(false))
        }
    }
}

fn linkinfo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let link = path_param(vm, args, 0)?;
    if link.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let dirname = zend_dirname(link.as_bytes());
    if check_open_basedir(vm, &dirname, true)? {
        return Ok(Value::Bool(false));
    }
    match std::fs::symlink_metadata(plain::os(link.as_bytes())) {
        Ok(m) => {
            use std::os::unix::fs::MetadataExt;
            Ok(Value::Int(m.dev() as i64))
        }
        Err(e) => {
            warn_errno(vm, &e)?;
            Ok(Value::Int(-1))
        }
    }
}

fn symlink(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let topath = path_param(vm, args, 0)?;
    let frompath = path_param(vm, args, 1)?;

    let Some(source_p) = plain::expand_filepath(frompath.as_bytes()) else {
        vm.warning("No such file or directory")?;
        return Ok(Value::Bool(false));
    };
    let dirname = zend_dirname(&source_p);
    let Some(dest_p) = expand_filepath_ex(topath.as_bytes(), &dirname) else {
        vm.warning("No such file or directory")?;
        return Ok(Value::Bool(false));
    };

    if is_url(vm, &source_p)? || is_url(vm, &dest_p)? {
        vm.warning("Unable to symlink to a URL")?;
        return Ok(Value::Bool(false));
    }
    if check_open_basedir(vm, &dest_p, true)? || check_open_basedir(vm, &source_p, true)? {
        return Ok(Value::Bool(false));
    }

    // For the source, an expanded path must be used. For the target the
    // exact string given by the user must be used, relative or not,
    // existing or not: the target is relative to the link itself.
    match std::os::unix::fs::symlink(plain::os(topath.as_bytes()), plain::os(&source_p)) {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            warn_errno(vm, &e)?;
            Ok(Value::Bool(false))
        }
    }
}

fn link(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let topath = path_param(vm, args, 0)?;
    let frompath = path_param(vm, args, 1)?;

    let source_p = plain::expand_filepath(frompath.as_bytes());
    let dest_p = plain::expand_filepath(topath.as_bytes());
    let (Some(source_p), Some(dest_p)) = (source_p, dest_p) else {
        vm.warning("No such file or directory")?;
        return Ok(Value::Bool(false));
    };

    if is_url(vm, &source_p)? || is_url(vm, &dest_p)? {
        vm.warning("Unable to link to a URL")?;
        return Ok(Value::Bool(false));
    }
    if check_open_basedir(vm, &dest_p, true)? || check_open_basedir(vm, &source_p, true)? {
        return Ok(Value::Bool(false));
    }

    // php_sys_link(topath, frompath): the non-ZTS build links the paths as
    // given.
    match std::fs::hard_link(plain::os(topath.as_bytes()), plain::os(frompath.as_bytes())) {
        Ok(()) => Ok(Value::Bool(true)),
        Err(e) => {
            warn_errno(vm, &e)?;
            Ok(Value::Bool(false))
        }
    }
}

static READLINK: NativeFunction = NativeFunction {
    sig: "readlink(string $path): string|false",
    handler: readlink,
};
static LINKINFO: NativeFunction = NativeFunction {
    sig: "linkinfo(string $path): int|false",
    handler: linkinfo,
};
static SYMLINK: NativeFunction = NativeFunction {
    sig: "symlink(string $target, string $link): bool",
    handler: symlink,
};
static LINK: NativeFunction = NativeFunction {
    sig: "link(string $target, string $link): bool",
    handler: link,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&READLINK, &LINKINFO, &SYMLINK, &LINK]);
}

#[cfg(test)]
mod tests {
    //! Expectations from ext/standard/tests/file/{symlink*,readlink*,
    //! symlink_link_linkinfo_is_link_*}.phpt.

    use super::*;
    use crate::standard::filestat::tests::TempDir;
    use crate::testing::{TestVm, thrown};
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    fn call(t: &mut TestVm, f: &str, args: &[&str]) -> Value {
        t.call(f, args.iter().map(|&a| Value::from(a)).collect())
            .unwrap()
    }

    fn is_true(v: &Value) -> bool {
        matches!(v, Value::Bool(true))
    }

    fn is_false(v: &Value) -> bool {
        matches!(v, Value::Bool(false))
    }

    fn str_of(v: Value) -> String {
        v.as_str()
            .unwrap_or_else(|| panic!("expected string, got {v:?}"))
            .to_string_lossy()
            .into_owned()
    }

    fn warning(f: &str, msg: &str) -> String {
        format!("\nWarning: {f}(): {msg} in /t.php on line 3\n")
    }

    #[test]
    fn dirname_like_zend_dirname() {
        for (path, dir) in [
            ("/a/b", "/a"),
            ("/a/b/", "/a"),
            ("a//b", "a"),
            ("a", "."),
            ("/a", "/"),
            ("/", "/"),
            ("///", "/"),
            ("//a", "/"),
            ("a/b/c", "a/b"),
            ("", ""),
        ] {
            assert_eq!(zend_dirname(path.as_bytes()), dir.as_bytes(), "{path}");
        }
    }

    #[test]
    fn symlink_readlink_and_linkinfo() {
        let d = TempDir::new("symlink");
        let filename = d.path("symlink.dat");
        let link = d.path("symlink.link");
        let mut t = TestVm::new();
        // the target need not exist
        assert!(is_true(&call(&mut t, "symlink", &[&filename, &link])));
        assert_eq!(str_of(call(&mut t, "readlink", &[&link])), filename);
        let dev = fs::symlink_metadata(&link).unwrap().dev() as i64;
        assert_eq!(call(&mut t, "linkinfo", &[&link]).as_int(), Some(dev));
        assert!(is_true(&call(&mut t, "is_link", &[&link])));
        assert_eq!(t.output(), "");
        fs::remove_file(&link).unwrap();

        assert!(is_false(&call(&mut t, "readlink", &[&link])));
        assert_eq!(t.output(), warning("readlink", "No such file or directory"));
        assert_eq!(call(&mut t, "linkinfo", &[&link]).as_int(), Some(-1));
        assert_eq!(t.output(), warning("linkinfo", "No such file or directory"));

        fs::write(&filename, "").unwrap();
        let dir = d.path("");
        let dir = dir.trim_end_matches('/');
        assert!(is_false(&call(&mut t, "symlink", &[&filename, dir])));
        assert_eq!(t.output(), warning("symlink", "File exists"));
        assert!(is_true(&call(&mut t, "symlink", &[&filename, &link])));
        fs::remove_file(&link).unwrap();
        fs::write(&link, "").unwrap();
        assert!(is_false(&call(&mut t, "symlink", &[&filename, &link])));
        assert_eq!(t.output(), warning("symlink", "File exists"));
        fs::remove_file(&link).unwrap();

        assert!(is_true(&call(&mut t, "link", &[&filename, &link])));
        fs::remove_file(&filename).unwrap();
        fs::remove_file(&link).unwrap();
        assert!(is_false(&call(&mut t, "link", &[&filename, &link])));
        assert_eq!(t.output(), warning("link", "No such file or directory"));
    }

    #[test]
    fn dot_paths() {
        let mut t = TestVm::new();
        assert!(is_false(&call(&mut t, "symlink", &[".", "."])));
        assert!(t.output().starts_with("\nWarning: symlink(): "));
        assert!(is_false(&call(&mut t, "link", &[".", "."])));
        assert!(t.output().starts_with("\nWarning: link(): "));
        assert!(is_false(&call(&mut t, "readlink", &["."])));
        assert_eq!(t.output(), warning("readlink", "Invalid argument"));
        let dev = fs::symlink_metadata(".").unwrap().dev() as i64;
        assert_eq!(call(&mut t, "linkinfo", &["."]).as_int(), Some(dev));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn relative_targets_are_stored_verbatim() {
        // symlink_to_symlink.phpt
        let d = TempDir::new("relsym");
        let prefix = d.path("symlink_to_symlink.php");
        let p = |s: &str| format!("{prefix}{s}");
        fs::write(p("_file"), "data").unwrap();
        let mut t = TestVm::new();
        for (target, link) in [
            ("symlink_to_symlink.php_file".to_string(), p("_link1")),
            ("symlink_to_symlink.php_link1".to_string(), p("_link2")),
            (
                "symlink_to_symlink.php_nonexistent".to_string(),
                p("_link3"),
            ),
            (p("_file"), p("_link4")),
            (p("_link4"), p("_link5")),
        ] {
            assert!(is_true(&call(&mut t, "symlink", &[&target, &link])));
            assert_eq!(str_of(call(&mut t, "readlink", &[&link])), target);
        }
        // relative targets resolve against the link's directory
        for l in ["_link1", "_link2", "_link5"] {
            assert!(is_true(&call(&mut t, "is_file", &[&p(l)])), "{l}");
        }
        assert!(is_false(&call(&mut t, "file_exists", &[&p("_link3")])));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn readlink_errors() {
        let d = TempDir::new("readlink");
        let f = d.path("file");
        fs::write(&f, "").unwrap();
        let mut t = TestVm::new();
        let missing = d.path("readlink_error.tmp");
        assert!(is_false(&call(&mut t, "readlink", &[&missing])));
        assert_eq!(t.output(), warning("readlink", "No such file or directory"));
        assert!(is_false(&call(&mut t, "readlink", &[&f])));
        assert_eq!(t.output(), warning("readlink", "Invalid argument"));
        assert!(is_false(&call(&mut t, "readlink", &[&d.path("")])));
        assert_eq!(t.output(), warning("readlink", "Invalid argument"));
        // readlink_realpath_basic1: a trailing slash after a link
        let link = d.path("link");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        assert!(is_false(&call(&mut t, "readlink", &[&format!("{link}/")])));
        assert!(t.output().starts_with("\nWarning: readlink(): "));
        assert_eq!(str_of(call(&mut t, "readlink", &[&link])), f);
        let dotted = link.replace("/link", "//./link");
        assert_eq!(str_of(call(&mut t, "readlink", &[&dotted])), f);
    }

    #[test]
    fn symlink_and_linkinfo_argument_errors() {
        let d = TempDir::new("symerr");
        let filename = d.path("error1.tmp");
        let linkname = d.path("link_error1.tmp");
        fs::write(&filename, "").unwrap();
        let mut t = TestVm::new();
        for args in [["", linkname.as_str()], [filename.as_str(), ""]] {
            assert!(is_false(&call(&mut t, "symlink", &args)));
            assert_eq!(t.output(), warning("symlink", "No such file or directory"));
        }
        let r = t.call(
            "symlink",
            vec![Value::Bool(false), Value::from(linkname.as_str())],
        );
        assert!(is_false(&r.unwrap()));
        assert_eq!(t.output(), warning("symlink", "No such file or directory"));
        let r = t.call("linkinfo", vec![Value::from("")]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "linkinfo(): Argument #1 ($path) must not be empty".to_string()
            )
        );
        let r = t.call("readlink", vec![Value::from("a\0b")]);
        assert_eq!(
            thrown(r).1,
            "readlink(): Argument #1 ($path) must not contain any null bytes"
        );
        let r = t.call("symlink", vec![Value::from("a"), Value::from("b\0")]);
        assert_eq!(
            thrown(r).1,
            "symlink(): Argument #2 ($link) must not contain any null bytes"
        );
    }

    #[test]
    fn hard_links() {
        let d = TempDir::new("hard");
        let filename = d.path("error2.tmp");
        let linkname = d.path("link_error2.tmp");
        fs::write(&filename, "content").unwrap();
        let mut t = TestVm::new();
        // symlink_link_linkinfo_is_link_error2
        for args in [
            ["", linkname.as_str()],
            [" ", linkname.as_str()],
            [filename.as_str(), ""],
        ] {
            assert!(is_false(&call(&mut t, "link", &args)));
            assert_eq!(t.output(), warning("link", "No such file or directory"));
        }
        assert!(is_true(&call(&mut t, "link", &[&filename, &linkname])));
        let a = call(&mut t, "fileinode", &[&filename]).as_int();
        let b = call(&mut t, "fileinode", &[&linkname]).as_int();
        assert!(a.is_some() && a == b);
        assert_eq!(fs::metadata(&filename).unwrap().nlink(), 2);
        assert_eq!(str_of(call(&mut t, "filetype", &[&linkname])), "file");
        assert!(is_false(&call(&mut t, "is_link", &[&linkname])));
        assert!(is_false(&call(&mut t, "link", &[&filename, &linkname])));
        assert_eq!(t.output(), warning("link", "File exists"));
        // a symlink's filetype is "link"
        let sym = d.path("sym");
        assert!(is_true(&call(&mut t, "symlink", &[&filename, &sym])));
        assert_eq!(str_of(call(&mut t, "filetype", &[&sym])), "link");
        assert_eq!(t.output(), "");
    }

    #[test]
    fn open_basedir_restrictions() {
        let d = TempDir::new("linkbase");
        let base = d.path("");
        let base = base.trim_end_matches('/').to_string();
        let inside = d.path("link");
        let mut t = TestVm::new();
        assert!(t.vm.ini_set_system("open_basedir", base.as_str()));
        let denied = |f: &str, p: &str| {
            warning(
                f,
                &format!(
                    "open_basedir restriction in effect. File({p}) is not within the allowed path(s): ({base})"
                ),
            )
        };
        assert!(is_false(&call(&mut t, "readlink", &["/etc/hosts"])));
        assert_eq!(t.output(), denied("readlink", "/etc/hosts"));
        assert!(is_false(&call(&mut t, "linkinfo", &["/etc/hosts"])));
        assert_eq!(t.output(), denied("linkinfo", "/etc"));
        // the target is checked before the link
        assert!(is_false(&call(&mut t, "symlink", &["/etc/hosts", &inside])));
        assert!(t.output().contains("File(/"));
        assert!(is_false(&call(&mut t, "link", &["/etc/hosts", &inside])));
        assert!(t.output().contains("File(/"));
        let target = d.path("target");
        fs::write(&target, "").unwrap();
        assert!(is_true(&call(&mut t, "symlink", &[&target, &inside])));
        assert_eq!(str_of(call(&mut t, "readlink", &[&inside])), target);
        assert_eq!(t.output(), "");
    }
}
