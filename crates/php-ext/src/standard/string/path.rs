//! ext/standard/string.c — `basename()`, `dirname()`, `pathinfo()` and the
//! `PATHINFO_*` constants (POSIX semantics: `/` is the only separator).

use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

pub const PATHINFO_DIRNAME: i64 = 1;
pub const PATHINFO_BASENAME: i64 = 2;
pub const PATHINFO_EXTENSION: i64 = 4;
pub const PATHINFO_FILENAME: i64 = 8;
pub const PATHINFO_ALL: i64 =
    PATHINFO_DIRNAME | PATHINFO_BASENAME | PATHINFO_EXTENSION | PATHINFO_FILENAME;

/// `php_basename` (ASCII-compatible locale branch): the last path
/// component of `s` with trailing slashes stripped, minus `suffix` when the
/// component ends with it and is longer than it.
pub fn php_basename<'a>(s: &'a [u8], suffix: &[u8]) -> &'a [u8] {
    let Some(last) = s.iter().rposition(|&c| c != b'/') else {
        return b"";
    };
    let end = last + 1;
    let start = s[..end]
        .iter()
        .rposition(|&c| c == b'/')
        .map_or(0, |p| p + 1);
    let name = &s[start..end];
    if suffix.len() < name.len() && name.ends_with(suffix) {
        &name[..name.len() - suffix.len()]
    } else {
        name
    }
}

/// `zend_dirname` (POSIX): the length of the directory part of `path`,
/// rewriting `path` in place to `/` or `.` where php-src does. Returns 0 for
/// an empty path.
pub fn zend_dirname(path: &mut Vec<u8>) -> usize {
    let len = path.len();
    if len == 0 {
        return 0;
    }
    let slash_or_dot = |path: &mut Vec<u8>, c: u8| {
        path.truncate(1);
        path[0] = c;
        1
    };
    // Strip trailing slashes.
    let Some(end) = path.iter().rposition(|&c| c != b'/') else {
        return slash_or_dot(path, b'/');
    };
    // Strip the file name.
    let Some(end) = path[..end].iter().rposition(|&c| c == b'/') else {
        return slash_or_dot(path, b'.');
    };
    // Strip the slashes before the file name.
    let Some(end) = path[..end].iter().rposition(|&c| c != b'/') else {
        return slash_or_dot(path, b'/');
    };
    path.truncate(end + 1);
    end + 1
}

/// `dirname($path, $levels)` for `levels >= 1` (`_zend_dirname`).
pub fn php_dirname(path: &[u8], levels: i64) -> Vec<u8> {
    let mut ret = path.to_vec();
    if levels == 1 {
        zend_dirname(&mut ret);
        return ret;
    }
    let mut levels = levels;
    loop {
        let before = ret.len();
        let after = zend_dirname(&mut ret);
        ret.truncate(after);
        levels -= 1;
        if after >= before || levels == 0 {
            break;
        }
    }
    ret
}

fn basename(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::bytes(php_basename(args.str(0), args.str(1))))
}

fn dirname(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let levels = args.int(1);
    if levels < 1 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 1"));
    }
    Ok(Value::String(PhpStr::from_vec(php_dirname(
        args.str(0),
        levels,
    ))))
}

/// The directory part as `pathinfo()` reports it: php-src reads the
/// `zend_dirname` result back as a C string, so it stops at the first NUL.
fn pathinfo_dirname(path: &[u8]) -> Vec<u8> {
    let mut d = php_dirname(path, 1);
    if let Some(nul) = d.iter().position(|&c| c == 0) {
        d.truncate(nul);
    }
    d
}

fn pathinfo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let path = args.str(0);
    let opt = args.int(1);
    if !(PATHINFO_DIRNAME..=PATHINFO_ALL).contains(&opt) {
        return Err(vm.argument_value_error(2, "must be one of the PATHINFO_* constants"));
    }
    if opt < PATHINFO_ALL && opt & (opt - 1) != 0 {
        return Err(vm.argument_value_error(2, "must be only one of the PATHINFO_* constants"));
    }
    if opt == PATHINFO_DIRNAME {
        return Ok(Value::from(pathinfo_dirname(path)));
    }
    let base = php_basename(path, b"");
    if opt == PATHINFO_BASENAME {
        return Ok(Value::bytes(base));
    }
    let dot = base.iter().rposition(|&c| c == b'.');
    let extension = dot.map(|p| &base[p + 1..]);
    if opt == PATHINFO_EXTENSION {
        return Ok(Value::bytes(extension.unwrap_or(b"")));
    }
    let filename = dot.map_or(base, |p| &base[..p]);
    if opt == PATHINFO_FILENAME {
        return Ok(Value::bytes(filename));
    }
    let mut arr = PhpArray::new();
    let dir = pathinfo_dirname(path);
    if !dir.is_empty() {
        arr.insert_str("dirname", dir);
    }
    arr.insert_str("basename", base);
    if let Some(ext) = extension {
        arr.insert_str("extension", ext);
    }
    arr.insert_str("filename", filename);
    Ok(Value::array(arr))
}

pub static BASENAME: NativeFunction = NativeFunction {
    sig: "basename(string $path, string $suffix = \"\"): string",
    handler: basename,
};
pub static DIRNAME: NativeFunction = NativeFunction {
    sig: "dirname(string $path, int $levels = 1): string",
    handler: dirname,
};
pub static PATHINFO: NativeFunction = NativeFunction {
    sig: "pathinfo(string $path, int $flags = PATHINFO_ALL): array|string",
    handler: pathinfo,
};

pub fn register(r: &mut Registry) {
    r.constant("PATHINFO_DIRNAME", PATHINFO_DIRNAME);
    r.constant("PATHINFO_BASENAME", PATHINFO_BASENAME);
    r.constant("PATHINFO_EXTENSION", PATHINFO_EXTENSION);
    r.constant("PATHINFO_FILENAME", PATHINFO_FILENAME);
    r.constant("PATHINFO_ALL", PATHINFO_ALL);
    r.functions(&[&BASENAME, &DIRNAME, &PATHINFO]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn s(v: Value) -> Vec<u8> {
        match v {
            Value::String(s) => s.to_vec(),
            v => panic!("unexpected {v:?}"),
        }
    }

    fn base(t: &mut TestVm, path: &[u8], suffix: Option<&[u8]>) -> Vec<u8> {
        let mut args = vec![Value::bytes(path)];
        if let Some(suf) = suffix {
            args.push(Value::bytes(suf));
        }
        s(t.call("basename", args).unwrap())
    }

    fn dir(t: &mut TestVm, path: &[u8], levels: Option<i64>) -> Vec<u8> {
        let mut args = vec![Value::bytes(path)];
        if let Some(l) = levels {
            args.push(Value::Int(l));
        }
        s(t.call("dirname", args).unwrap())
    }

    /// `pathinfo($path)` as `(key, value)` pairs.
    fn info(t: &mut TestVm, path: &[u8]) -> Vec<(String, String)> {
        let Value::Array(a) = t.call("pathinfo", vec![Value::bytes(path)]).unwrap() else {
            panic!("not an array")
        };
        a.iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.as_str().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
        p.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn basename_basic() {
        let mut t = TestVm::new();
        // basename.phpt / basename_basic.phpt
        for p in [
            "bar",
            "/foo/bar",
            "foo/bar",
            "/bar",
            "bar/",
            "/bar/",
            "/foo/bar/",
            "foo/bar/",
        ] {
            assert_eq!(base(&mut t, p.as_bytes(), None), b"bar", "{p}");
        }
        for p in ["bar.gz", "/foo/bar.gz", "foo/bar.gz/", "/bar.gz/"] {
            assert_eq!(base(&mut t, p.as_bytes(), Some(b".gz")), b"bar", "{p}");
        }
        assert_eq!(base(&mut t, b"bar.gz", Some(b"bar.gz")), b"bar.gz");
        for p in ["/.gz", ".gz", "/foo/.gz", ".gz/", "/foo/.gz/", "foo/.gz/"] {
            assert_eq!(base(&mut t, p.as_bytes(), Some(b".gz")), b".gz", "{p}");
        }
        assert_eq!(base(&mut t, b"foo\0bar", None), b"foo\0bar");
        assert_eq!(base(&mut t, b"/foo\0bar/", None), b"foo\0bar");
        assert_eq!(base(&mut t, b"foo\0bar/test", None), b"test");
        assert_eq!(base(&mut t, b"/foo\0bar/bar.gz", Some(b".gz")), b"bar");
        // basename_invalid_path.phpt
        assert_eq!(base(&mut t, b"\xff", None), b"\xff");
        assert_eq!(base(&mut t, b"a\xffb", None), b"a\xffb");
    }

    #[test]
    fn basename_variation() {
        let mut t = TestVm::new();
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("~/home/user/bar", None, "bar"),
            ("~/home/user/bar", Some(" "), "bar"),
            ("~/home/user/bar.tar", Some(".tar"), "bar"),
            ("~/home/user/bar.tar/", Some("~"), "bar.tar"),
            ("hostname:/home/user/tbar.gz", Some("bar.gz"), "t"),
            ("hostname:/home/user/tbar.gz", Some("/bar.gz"), "tbar.gz"),
            ("hostname:/home/user/My Pics.gz/", Some("Pics.gz"), "My "),
            ("hostname:/home/user/My Pics.gz/", Some(".gz"), "My Pics"),
            (
                "hostname:/home/user/My Pics.gz/",
                Some("My Pics.gz"),
                "My Pics.gz",
            ),
            ("10.5", Some(".5"), "10"),
            ("10.5", Some("10.5"), "10.5"),
            ("105", Some("5"), "10"),
            ("10/10.gz", None, "10.gz"),
            ("/bar.gz/", Some("/bar.gz/"), "bar.gz"),
            (" ", Some(" "), " "),
            ("", None, ""),
            ("/", None, ""),
            ("//", None, ""),
        ];
        for &(p, suf, want) in cases {
            assert_eq!(
                base(&mut t, p.as_bytes(), suf.map(str::as_bytes)),
                want.as_bytes(),
                "{p:?} {suf:?}"
            );
        }
    }

    #[test]
    fn dirname_basic() {
        let mut t = TestVm::new();
        // dirname_basic.phpt
        let cases: &[(&[u8], &[u8])] = &[
            (b"bar", b"."),
            (b"/foo/bar", b"/foo"),
            (b"foo/bar", b"foo"),
            (b"/bar", b"/"),
            (b"bar/", b"."),
            (b"/bar/", b"/"),
            (b"/foo/bar/", b"/foo"),
            (b"foo/bar/", b"foo"),
            (b"/foo/bar.gz", b"/foo"),
            (b"bar.gz/", b"."),
            (b"/.gz", b"/"),
            (b".gz", b"."),
            (b"/foo/.gz/", b"/foo"),
            (b"foo\0bar", b"."),
            (b"/foo\0bar/", b"/"),
            (b"/foo\0bar/t.gz", b"/foo\0bar"),
            // dirname_variation.phpt
            (b"~/home/user/bar/", b"~/home/user"),
            (b"hostname:/home/user/My Pics", b"hostname:/home/user"),
            (b"/10.5/", b"/"),
            (b"10/10.gz", b"10"),
            (b" ", b"."),
            (b"", b""),
            (b"/", b"/"),
            (b"///", b"/"),
            (b"//usr//", b"/"),
        ];
        for &(p, want) in cases {
            assert_eq!(dir(&mut t, p, None), want, "{:?}", PhpStr::from(p));
        }
    }

    #[test]
    fn dirname_levels() {
        // dirname_multi.phpt, dirname_error.phpt
        let mut t = TestVm::new();
        let r = t.call("dirname", vec![Value::str("/foo/bar/baz"), Value::Int(0)]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".into(),
                "dirname(): Argument #2 ($levels) must be greater than or equal to 1".into()
            )
        );
        assert_eq!(dir(&mut t, b"/foo/bar/baz", Some(1)), b"/foo/bar");
        assert_eq!(dir(&mut t, b"/foo/bar/baz", Some(2)), b"/foo");
        assert_eq!(dir(&mut t, b"/foo/bar/baz", Some(3)), b"/");
        assert_eq!(dir(&mut t, b"/foo/bar/baz", Some(4)), b"/");
        assert_eq!(dir(&mut t, b"/foo/bar/baz", Some(i64::MAX)), b"/");
        assert_eq!(dir(&mut t, b"a/b/c", Some(5)), b".");
        assert_eq!(dir(&mut t, b"", Some(3)), b"");
    }

    #[test]
    fn pathinfo_all() {
        // pathinfo.phpt
        let mut t = TestVm::new();
        assert_eq!(
            info(&mut t, b""),
            pairs(&[("basename", ""), ("filename", "")])
        );
        assert_eq!(
            info(&mut t, b"."),
            pairs(&[
                ("dirname", "."),
                ("basename", "."),
                ("extension", ""),
                ("filename", "")
            ])
        );
        assert_eq!(
            info(&mut t, b".."),
            pairs(&[
                ("dirname", "."),
                ("basename", ".."),
                ("extension", ""),
                ("filename", ".")
            ])
        );
        assert_eq!(
            info(&mut t, b"/"),
            pairs(&[("dirname", "/"), ("basename", ""), ("filename", "")])
        );
        assert_eq!(
            info(&mut t, b"/."),
            pairs(&[
                ("dirname", "/"),
                ("basename", "."),
                ("extension", ""),
                ("filename", "")
            ])
        );
        assert_eq!(
            info(&mut t, b".cvsignore"),
            pairs(&[
                ("dirname", "."),
                ("basename", ".cvsignore"),
                ("extension", "cvsignore"),
                ("filename", "")
            ])
        );
        // pathinfo_basic2.phpt
        assert_eq!(
            info(&mut t, b"/usr/include/../arpa/./inet.h"),
            pairs(&[
                ("dirname", "/usr/include/../arpa/."),
                ("basename", "inet.h"),
                ("extension", "h"),
                ("filename", "inet")
            ])
        );
        assert_eq!(
            info(&mut t, b"c:\\..\\dir1"),
            pairs(&[
                ("dirname", "."),
                ("basename", "c:\\..\\dir1"),
                ("extension", "\\dir1"),
                ("filename", "c:\\.")
            ])
        );
        // The array's dirname stops at a NUL, like php-src's C string.
        assert_eq!(
            info(&mut t, b"\0a/b"),
            pairs(&[("basename", "b"), ("filename", "b")])
        );
    }

    #[test]
    fn pathinfo_flags() {
        let mut t = TestVm::new();
        let path = Value::str("/dir/strings/pathinfo.php");
        let one = |t: &mut TestVm, flag: &str| {
            let f = t.vm.constant(flag).unwrap();
            s(t.call("pathinfo", vec![path.clone(), f]).unwrap())
        };
        assert_eq!(one(&mut t, "PATHINFO_BASENAME"), b"pathinfo.php");
        assert_eq!(one(&mut t, "PATHINFO_FILENAME"), b"pathinfo");
        assert_eq!(one(&mut t, "PATHINFO_EXTENSION"), b"php");
        assert_eq!(one(&mut t, "PATHINFO_DIRNAME"), b"/dir/strings");
        let r = t.call("pathinfo", vec![Value::str("a.b"), Value::Int(4 | 8)]);
        assert_eq!(
            thrown(r).1,
            "pathinfo(): Argument #2 ($flags) must be only one of the PATHINFO_* constants"
        );
        for bad in [0, 16] {
            let r = t.call("pathinfo", vec![Value::str("a.b"), Value::Int(bad)]);
            assert_eq!(
                thrown(r),
                (
                    "ValueError".into(),
                    "pathinfo(): Argument #2 ($flags) must be one of the PATHINFO_* constants"
                        .into()
                )
            );
        }
        assert_eq!(
            s(t.call("pathinfo", vec![Value::str("afile"), Value::Int(4)])
                .unwrap()),
            b""
        );
        assert_eq!(t.vm.constant("PATHINFO_ALL").unwrap().as_int(), Some(15));
    }
}
