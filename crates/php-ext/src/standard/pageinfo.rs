//! ext/standard/pageinfo.c

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use super::basic::{BasicState, script_path, sys};

/// `BG(page_uid)`, `BG(page_gid)`, `BG(page_inode)`, `BG(page_mtime)`.
#[derive(Clone, Copy)]
pub(crate) struct PageStat {
    pub uid: i64,
    pub gid: i64,
    pub inode: Option<i64>,
    pub mtime: Option<i64>,
}

#[derive(Default)]
pub(crate) struct PageInfoState {
    stat: Option<PageStat>,
}

/// `php_statpage()`: stats the script once and caches the result.
pub(crate) fn statpage(vm: &mut Vm) -> PageStat {
    if let Some(s) = vm.ext_data::<PageInfoState>().stat {
        return s;
    }
    let _ = vm.ext_data::<BasicState>();
    let stat = match script_stat(vm) {
        Some(s) => s,
        // No source file (`php -r`): only the process ids are known.
        None => PageStat {
            uid: sys::getuid(),
            gid: sys::getgid(),
            inode: None,
            mtime: None,
        },
    };
    vm.ext_data::<PageInfoState>().stat = Some(stat);
    stat
}

/// `sapi_get_stat()`: `stat()` of the primary script.
fn script_stat(vm: &mut Vm) -> Option<PageStat> {
    let path = script_path(vm)?;
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        let md = std::fs::metadata(OsStr::from_bytes(path.as_bytes())).ok()?;
        Some(PageStat {
            uid: md.uid() as i64,
            gid: md.gid() as i64,
            inode: Some(md.ino() as i64),
            mtime: Some(md.mtime()),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// `php_get_current_user()`: the owner of the primary script, `""` without one.
pub(crate) fn current_user(vm: &mut Vm) -> Vec<u8> {
    if script_stat(vm).is_none() {
        return Vec::new();
    }
    let uid = statpage(vm).uid;
    sys::getpwuid_name(uid).unwrap_or_default()
}

fn getmyuid(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let uid = statpage(vm).uid;
    Ok(if uid < 0 {
        Value::Bool(false)
    } else {
        Value::Int(uid)
    })
}

fn getmygid(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let gid = statpage(vm).gid;
    Ok(if gid < 0 {
        Value::Bool(false)
    } else {
        Value::Int(gid)
    })
}

fn getmypid(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(std::process::id() as i64))
}

fn getmyinode(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match statpage(vm).inode {
        Some(i) if i >= 0 => Value::Int(i),
        _ => Value::Bool(false),
    })
}

fn getlastmod(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match statpage(vm).mtime {
        Some(m) if m >= 0 => Value::Int(m),
        _ => Value::Bool(false),
    })
}

pub static GETMYUID: NativeFunction = NativeFunction {
    sig: "getmyuid(): int|false",
    handler: getmyuid,
};
pub static GETMYGID: NativeFunction = NativeFunction {
    sig: "getmygid(): int|false",
    handler: getmygid,
};
pub static GETMYPID: NativeFunction = NativeFunction {
    sig: "getmypid(): int|false",
    handler: getmypid,
};
pub static GETMYINODE: NativeFunction = NativeFunction {
    sig: "getmyinode(): int|false",
    handler: getmyinode,
};
pub static GETLASTMOD: NativeFunction = NativeFunction {
    sig: "getlastmod(): int|false",
    handler: getlastmod,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&GETMYUID, &GETMYGID, &GETMYPID, &GETMYINODE, &GETLASTMOD]);
}

#[cfg(test)]
mod tests {
    use crate::standard::basic::set_main_script;
    use crate::testing::TestVm;
    use php_runtime::Value;

    #[test]
    fn pid_and_ids() {
        let mut t = TestVm::new();
        assert_eq!(
            t.call("getmypid", vec![]).unwrap().as_int(),
            Some(std::process::id() as i64)
        );
        assert!(t.call("getmyuid", vec![]).unwrap().as_int().unwrap() >= 0);
        assert!(t.call("getmygid", vec![]).unwrap().as_int().unwrap() >= 0);
    }

    #[test]
    fn no_script_means_no_inode_or_mtime() {
        let mut t = TestVm::new();
        // `/t.php` (the TestVm script) does not exist: like `php -r`.
        assert!(matches!(
            t.call("getmyinode", vec![]).unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("getlastmod", vec![]).unwrap(),
            Value::Bool(false)
        ));
    }

    #[test]
    fn stats_the_main_script() {
        let mut t = TestVm::new();
        set_main_script(&mut t.vm, concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
        assert!(t.call("getmyinode", vec![]).unwrap().as_int().unwrap() > 0);
        assert!(t.call("getlastmod", vec![]).unwrap().as_int().unwrap() > 1_000_000_000);
    }
}
