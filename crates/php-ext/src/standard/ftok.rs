//! ext/standard/ftok.c — `ftok()`: a System V IPC key from a pathname and
//! a project identifier, computed by the C library's `ftok(3)`.

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use super::streams::path_param;
use super::streams::plain::check_open_basedir;
use super::streams::sys;

/// `ftok(3)`, which std does not expose (std already links the C library;
/// `key_t` is `int` on Linux and macOS).
#[allow(unsafe_code)]
mod c {
    use std::ffi::{CString, c_char, c_int};
    use std::io;

    unsafe extern "C" {
        #[link_name = "ftok"]
        fn c_ftok(path: *const c_char, id: c_int) -> c_int;
    }

    /// `ftok(path, id)`; the error carries `errno` when the key is -1.
    pub fn ftok(path: &[u8], id: c_char) -> Result<i32, io::Error> {
        let p = CString::new(path).map_err(|_| io::Error::from_raw_os_error(2))?;
        // SAFETY: `p` is a valid NUL-terminated string for the call.
        let k = unsafe { c_ftok(p.as_ptr(), c_int::from(id)) };
        if k == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(k)
        }
    }
}

fn ftok(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pathname = path_param(vm, args, 0)?;
    let proj = args.str(1).clone();

    if pathname.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    if proj.len() != 1 {
        return Err(vm.argument_value_error(2, "must be a single character"));
    }
    if check_open_basedir(vm, pathname.as_bytes(), true)? {
        return Ok(Value::Int(-1));
    }

    // `proj[0]` is a C `char`, sign-extended where `char` is signed.
    match c::ftok(pathname.as_bytes(), proj.as_bytes()[0] as std::ffi::c_char) {
        Ok(k) => Ok(Value::Int(i64::from(k))),
        Err(e) => {
            let reason = sys::strerror(sys::errno_of(&e));
            vm.warning(format!("ftok() failed - {reason}"))?;
            Ok(Value::Int(-1))
        }
    }
}

static FTOK: NativeFunction = NativeFunction {
    sig: "ftok(string $filename, string $project_id): int",
    handler: ftok,
};

pub fn register(r: &mut Registry) {
    r.function(&FTOK);
}

#[cfg(test)]
mod tests {
    use crate::standard::filestat::tests::TempDir;
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    fn ftok(t: &mut TestVm, path: &str, proj: &str) -> Value {
        t.call("ftok", vec![Value::from(path), Value::from(proj)])
            .unwrap()
    }

    #[test]
    fn argument_validation() {
        let mut t = TestVm::new();
        let r = t.call("ftok", vec![Value::from(""), Value::from("t")]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "ftok(): Argument #1 ($filename) must not be empty".to_string()
            )
        );
        for proj in ["", "ab"] {
            let r = t.call("ftok", vec![Value::from("/"), Value::from(proj)]);
            assert_eq!(
                thrown(r),
                (
                    "ValueError".to_string(),
                    "ftok(): Argument #2 ($project_id) must be a single character".to_string()
                )
            );
        }
        let r = t.call("ftok", vec![Value::from("a\0"), Value::from("t")]);
        assert_eq!(
            thrown(r).1,
            "ftok(): Argument #1 ($filename) must not contain any null bytes"
        );
    }

    #[test]
    fn missing_file_warns() {
        let d = TempDir::new("ftok-missing");
        let mut t = TestVm::new();
        assert_eq!(ftok(&mut t, &d.path("nope"), "t").as_int(), Some(-1));
        assert_eq!(
            t.output(),
            "\nWarning: ftok(): ftok() failed - No such file or directory in /t.php on line 3\n"
        );
    }

    #[test]
    fn key_from_inode_device_and_project() {
        let d = TempDir::new("ftok");
        let f = d.path("f");
        fs::write(&f, "").unwrap();
        let m = fs::metadata(&f).unwrap();
        let mut t = TestVm::new();
        let k = ftok(&mut t, &f, "B").as_int().unwrap();
        // the classic libc formula (glibc, BSD, macOS)
        let expect =
            ((m.ino() & 0xffff) | ((m.dev() as u64 & 0xff) << 16) | (u64::from(b'B') << 24)) as i32;
        assert_eq!(k, i64::from(expect));
        assert_eq!(ftok(&mut t, &f, "B").as_int(), Some(k));
        assert_ne!(ftok(&mut t, &f, "C").as_int(), Some(k));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn open_basedir_gives_minus_one() {
        let d = TempDir::new("ftok-basedir");
        let base = d.path("");
        let base = base.trim_end_matches('/').to_string();
        let mut t = TestVm::new();
        assert!(t.vm.ini_set_system("open_basedir", base.as_str()));
        assert_eq!(ftok(&mut t, "/etc/hosts", "t").as_int(), Some(-1));
        assert_eq!(
            t.output(),
            format!(
                "\nWarning: ftok(): open_basedir restriction in effect. File(/etc/hosts) is not within the allowed path(s): ({base}) in /t.php on line 3\n"
            )
        );
    }
}
