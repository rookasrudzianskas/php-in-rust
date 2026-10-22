//! ext/standard/filestat.c — `stat()` and friends (`php_stat`), the
//! `is_*()` checks, `chown`/`chgrp`/`chmod`/`touch`, `clearstatcache`,
//! `disk_*_space` and the realpath-cache introspection functions.
//!
//! `php_stat()` keeps php-src's one-entry caches (`BG(CurrentStatFile)` and
//! `BG(CurrentLStatFile)`) in the stream layer's [`StatCache`], so writes,
//! unlinks, renames, ... done through the plain-files wrapper invalidate
//! them exactly like `php_clear_stat_cache()` does in C.
//!
//! Not modelled:
//! * The realpath cache (`realpath_cache_*` ini settings): paths are always
//!   resolved afresh, so `realpath_cache_size()` is 0, `realpath_cache_get()`
//!   is empty and `clearstatcache(true, ...)` only clears the stat cache.
//! * On macOS php-src uses `statfs()` for `disk_*_space()` (Apple's
//!   `statvfs()` truncates block counts to 32 bits); this port goes through
//!   the stream layer's `statvfs()` binding, which agrees with `statfs()`
//!   for filesystems below 2^32 blocks.

use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::streams::path_param;
use super::streams::plain::{self, PlainWrapper, StatCache, check_open_basedir, clear_stat_cache};
use super::streams::stream::{StatBuf, open};
use super::streams::sys;
use super::streams::wrappers::{
    self, MetaValue, URL_STAT_LINK, URL_STAT_QUIET, WrapperRef, locate_url_wrapper,
};

// Switches for the various filestat functions (php_filestat.h).
pub const FS_PERMS: i32 = 0;
pub const FS_INODE: i32 = 1;
pub const FS_SIZE: i32 = 2;
pub const FS_OWNER: i32 = 3;
pub const FS_GROUP: i32 = 4;
pub const FS_ATIME: i32 = 5;
pub const FS_MTIME: i32 = 6;
pub const FS_CTIME: i32 = 7;
pub const FS_TYPE: i32 = 8;
pub const FS_IS_W: i32 = 9;
pub const FS_IS_R: i32 = 10;
pub const FS_IS_X: i32 = 11;
pub const FS_IS_FILE: i32 = 12;
pub const FS_IS_DIR: i32 = 13;
pub const FS_IS_LINK: i32 = 14;
pub const FS_EXISTS: i32 = 15;
pub const FS_LSTAT: i32 = 16;
pub const FS_STAT: i32 = 17;
pub const FS_LPERMS: i32 = 18;

/// `PHP_STREAM_URL_STAT_IGNORE_OPEN_BASEDIR`.
const URL_STAT_IGNORE_OPEN_BASEDIR: i32 = 4;

const S_IFMT: i64 = 0o170000;
const S_IFIFO: i64 = 0o010000;
const S_IFCHR: i64 = 0o020000;
const S_IFDIR: i64 = 0o040000;
const S_IFBLK: i64 = 0o060000;
const S_IFREG: i64 = 0o100000;
const S_IFLNK: i64 = 0o120000;
const S_IFSOCK: i64 = 0o140000;
const S_IRUSR: i64 = 0o400;
const S_IWUSR: i64 = 0o200;
const S_IXUSR: i64 = 0o100;
const S_IRGRP: i64 = 0o040;
const S_IWGRP: i64 = 0o020;
const S_IXGRP: i64 = 0o010;
const S_IROTH: i64 = 0o004;
const S_IWOTH: i64 = 0o002;
const S_IXOTH: i64 = 0o001;
/// `S_IXROOT` (Zend/zend_virtual_cwd.h).
const S_IXROOT: i64 = S_IXUSR | S_IXGRP | S_IXOTH;

/// Process credentials and `utime(2)`, which std does not expose. std
/// already links the C library, so these are plain declarations
/// (`uid_t`/`gid_t` are `u32` and `time_t` is `long` on Linux and macOS).
#[allow(unsafe_code)]
mod creds {
    use std::ffi::{CString, c_char, c_int, c_long};
    use std::io;

    #[repr(C)]
    struct UtimBuf {
        actime: c_long,
        modtime: c_long,
    }

    unsafe extern "C" {
        fn getuid() -> u32;
        fn getgid() -> u32;
        fn getgroups(size: c_int, list: *mut u32) -> c_int;
        fn utime(path: *const c_char, times: *const UtimBuf) -> c_int;
    }

    pub fn uid() -> u32 {
        // SAFETY: getuid() takes no arguments and cannot fail.
        unsafe { getuid() }
    }

    pub fn gid() -> u32 {
        // SAFETY: getgid() takes no arguments and cannot fail.
        unsafe { getgid() }
    }

    /// The supplementary group IDs (`getgroups`).
    pub fn groups() -> Vec<u32> {
        // SAFETY: a zero size with a null list only returns the count.
        let n = unsafe { getgroups(0, std::ptr::null_mut()) };
        if n <= 0 {
            return Vec::new();
        }
        let mut v = vec![0u32; n as usize];
        // SAFETY: `v` has room for `n` entries.
        let m = unsafe { getgroups(n, v.as_mut_ptr()) };
        v.truncate(m.max(0) as usize);
        v
    }

    /// `utime(path, times)` with `times` = (modtime, actime); `None` sets
    /// both to the current time.
    pub fn set_times(path: &[u8], times: Option<(i64, i64)>) -> io::Result<()> {
        let p = CString::new(path).map_err(|_| io::Error::from_raw_os_error(2))?;
        let buf = times.map(|(m, a)| UtimBuf {
            actime: a as c_long,
            modtime: m as c_long,
        });
        let ptr = buf
            .as_ref()
            .map_or(std::ptr::null(), |b| b as *const UtimBuf);
        // SAFETY: `p` is NUL-terminated and `ptr` is null or points to a
        // live `struct utimbuf` for the duration of the call.
        if unsafe { utime(p.as_ptr(), ptr) } == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn is_link_operation(t: i32) -> bool {
    matches!(t, FS_TYPE | FS_IS_LINK | FS_LSTAT | FS_LPERMS)
}

fn is_exists_check(t: i32) -> bool {
    matches!(
        t,
        FS_EXISTS | FS_IS_W | FS_IS_R | FS_IS_X | FS_IS_FILE | FS_IS_DIR | FS_IS_LINK | FS_LPERMS
    )
}

fn is_able_check(t: i32) -> bool {
    matches!(t, FS_IS_R | FS_IS_W | FS_IS_X)
}

fn is_access_check(t: i32) -> bool {
    is_able_check(t) || t == FS_EXISTS
}

fn is_plain(w: &WrapperRef) -> bool {
    w.as_any().is::<PlainWrapper>()
}

/// `php_is_stream_path`: `scheme://...`.
fn is_stream_path(p: &[u8]) -> bool {
    let n = p
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
        .count();
    n > 0 && p[n..].starts_with(b"://")
}

fn has_file_scheme(p: &[u8]) -> bool {
    p.len() >= 7 && p[..7].eq_ignore_ascii_case(b"file://")
}

fn warn_bytes(vm: &mut Vm, parts: &[&[u8]]) -> PResult<()> {
    vm.warning(PhpStr::from_vec(parts.concat()))
}

fn warn_errno(vm: &mut Vm, e: &std::io::Error) -> PResult<()> {
    vm.warning(sys::strerror(sys::errno_of(e)))
}

/// `php_stat()`: the value `stat()`, `filesize()`, `is_dir()`, ... return
/// for `filename` (`ty` is one of the `FS_*` switches).
pub fn php_stat(vm: &mut Vm, filename: &PhpStr, ty: i32) -> PResult<Value> {
    let name = filename.as_bytes();
    // Quick check for empty file paths
    if name.is_empty() {
        return Ok(Value::Bool(false));
    }
    let mut wrapper: Option<(WrapperRef, usize)> = None;
    if is_access_check(ty)
        && let Some(loc) = locate_url_wrapper(vm, name, 0)?
    {
        if is_plain(&loc.wrapper) {
            let local = &name[loc.path_offset..];
            if check_open_basedir(vm, local, true)? {
                return Ok(Value::Bool(false));
            }
            // if the wrapper is not found, we need to expand path to
            // match open behavior
            let expanded = if is_stream_path(local) {
                plain::expand_filepath(local)
            } else {
                None
            };
            let path = expanded.as_deref().unwrap_or(local);
            let mode = match ty {
                FS_EXISTS => sys::F_OK,
                FS_IS_W => sys::W_OK,
                FS_IS_R => sys::R_OK,
                _ => sys::X_OK,
            };
            return Ok(Value::Bool(sys::sys_access(path, mode).is_ok()));
        }
        wrapper = Some((loc.wrapper, loc.path_offset));
    }

    let mut flags = 0;
    if is_link_operation(ty) {
        flags |= URL_STAT_LINK;
    }
    if is_exists_check(ty) {
        flags |= URL_STAT_QUIET;
    }

    // Try to hit the cache first
    let cache = vm.ext_data::<StatCache>();
    let slot = if flags & URL_STAT_LINK != 0 {
        &cache.lstat
    } else {
        &cache.stat
    };
    let cached = slot
        .as_ref()
        .filter(|(n, _)| n.as_bytes() == name)
        .map(|(_, sb)| sb.clone());
    let sb = match cached {
        Some(sb) => sb,
        None => {
            if wrapper.is_none()
                && let Some(loc) = locate_url_wrapper(vm, name, 0)?
            {
                if is_plain(&loc.wrapper) && check_open_basedir(vm, &name[loc.path_offset..], true)?
                {
                    return Ok(Value::Bool(false));
                }
                wrapper = Some((loc.wrapper, loc.path_offset));
            }
            let stat = match &wrapper {
                Some((w, off)) if w.has_url_stat() => w.url_stat(
                    vm,
                    &name[*off..],
                    flags | URL_STAT_IGNORE_OPEN_BASEDIR,
                    None,
                )?,
                _ => None,
            };
            let Some(sb) = stat else {
                // Error Occurred
                if !is_exists_check(ty) {
                    let prefix: &[u8] = if is_link_operation(ty) { b"L" } else { b"" };
                    warn_bytes(vm, &[prefix, b"stat failed for ", name])?;
                }
                return Ok(Value::Bool(false));
            };
            // Drop into cache
            let cache = vm.ext_data::<StatCache>();
            if flags & URL_STAT_LINK != 0 {
                cache.lstat = Some((filename.clone(), sb.clone()));
            }
            if flags & URL_STAT_LINK == 0 || !sb.is_link() {
                cache.stat = Some((filename.clone(), sb.clone()));
            }
            sb
        }
    };

    let (mut rmask, mut wmask, mut xmask) = (S_IROTH, S_IWOTH, S_IXOTH);
    if (FS_IS_W..=FS_IS_X).contains(&ty) {
        if sb.uid == i64::from(creds::uid()) {
            (rmask, wmask, xmask) = (S_IRUSR, S_IWUSR, S_IXUSR);
        } else if sb.gid == i64::from(creds::gid())
            || creds::groups().iter().any(|&g| sb.gid == i64::from(g))
        {
            (rmask, wmask, xmask) = (S_IRGRP, S_IWGRP, S_IXGRP);
        }
    }

    if is_able_check(ty) && creds::uid() == 0 {
        // root has special perms on plain_wrapper
        if wrapper.as_ref().is_some_and(|(w, _)| is_plain(w)) {
            if ty == FS_IS_X {
                xmask = S_IXROOT;
            } else {
                return Ok(Value::Bool(true));
            }
        }
    }

    Ok(match ty {
        FS_PERMS | FS_LPERMS => Value::Int(sb.mode),
        FS_INODE => Value::Int(sb.ino),
        FS_SIZE => Value::Int(sb.size),
        FS_OWNER => Value::Int(sb.uid),
        FS_GROUP => Value::Int(sb.gid),
        FS_ATIME => Value::Int(sb.atime),
        FS_MTIME => Value::Int(sb.mtime),
        FS_CTIME => Value::Int(sb.ctime),
        FS_TYPE => return file_type(vm, &sb),
        FS_IS_W => Value::Bool(sb.mode & wmask != 0),
        FS_IS_R => Value::Bool(sb.mode & rmask != 0),
        FS_IS_X => Value::Bool(sb.mode & xmask != 0),
        FS_IS_FILE => Value::Bool(sb.is_reg()),
        FS_IS_DIR => Value::Bool(sb.is_dir()),
        FS_IS_LINK => Value::Bool(sb.is_link()),
        // the false case was done earlier
        FS_EXISTS => Value::Bool(true),
        FS_LSTAT | FS_STAT => Value::from(sb.to_php_array()),
        _ => {
            vm.warning("Didn't understand stat call")?;
            Value::Bool(false)
        }
    })
}

/// The `FS_TYPE` branch of `php_stat`.
fn file_type(vm: &mut Vm, sb: &StatBuf) -> PResult<Value> {
    let name = match sb.mode & S_IFMT {
        S_IFLNK => "link",
        S_IFIFO => "fifo",
        S_IFCHR => "char",
        S_IFDIR => "dir",
        S_IFBLK => "block",
        S_IFREG => "file",
        S_IFSOCK => "socket",
        other => {
            vm.notice(format!("Unknown file type ({other})"))?;
            "unknown"
        }
    };
    Ok(Value::from(name))
}

/// `FileFunction(name, funcnum)`: a `php_stat` front-end taking a path.
macro_rules! file_function {
    ($static:ident, $handler:ident, $sig:literal, $ty:expr) => {
        fn $handler(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            let filename = path_param(vm, args, 0)?;
            php_stat(vm, &filename, $ty)
        }
        static $static: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

file_function!(
    FILEPERMS,
    fileperms,
    "fileperms(string $filename): int|false",
    FS_PERMS
);
file_function!(
    FILEINODE,
    fileinode,
    "fileinode(string $filename): int|false",
    FS_INODE
);
file_function!(
    FILESIZE,
    filesize,
    "filesize(string $filename): int|false",
    FS_SIZE
);
file_function!(
    FILEOWNER,
    fileowner,
    "fileowner(string $filename): int|false",
    FS_OWNER
);
file_function!(
    FILEGROUP,
    filegroup,
    "filegroup(string $filename): int|false",
    FS_GROUP
);
file_function!(
    FILEATIME,
    fileatime,
    "fileatime(string $filename): int|false",
    FS_ATIME
);
file_function!(
    FILEMTIME,
    filemtime,
    "filemtime(string $filename): int|false",
    FS_MTIME
);
file_function!(
    FILECTIME,
    filectime,
    "filectime(string $filename): int|false",
    FS_CTIME
);
file_function!(
    FILETYPE,
    filetype,
    "filetype(string $filename): string|false",
    FS_TYPE
);
file_function!(
    IS_WRITABLE,
    is_writable,
    "is_writable(string $filename): bool",
    FS_IS_W
);
file_function!(
    IS_READABLE,
    is_readable,
    "is_readable(string $filename): bool",
    FS_IS_R
);
file_function!(
    IS_EXECUTABLE,
    is_executable,
    "is_executable(string $filename): bool",
    FS_IS_X
);
file_function!(
    IS_FILE,
    is_file,
    "is_file(string $filename): bool",
    FS_IS_FILE
);
file_function!(IS_DIR, is_dir, "is_dir(string $filename): bool", FS_IS_DIR);
file_function!(
    IS_LINK,
    is_link,
    "is_link(string $filename): bool",
    FS_IS_LINK
);
file_function!(
    FILE_EXISTS,
    file_exists,
    "file_exists(string $filename): bool",
    FS_EXISTS
);
file_function!(
    LSTAT,
    lstat,
    "lstat(string $filename): array|false",
    FS_LSTAT
);
file_function!(STAT, stat, "stat(string $filename): array|false", FS_STAT);

/// `@alias is_writable`.
static IS_WRITEABLE: NativeFunction = NativeFunction {
    sig: "is_writeable(string $filename): bool",
    handler: is_writable,
};

// ---------------------------------------------------------------------------
// chown / chgrp / chmod / touch
// ---------------------------------------------------------------------------

/// The wrapper dispatch shared by `chown`, `chgrp`, `chmod` and `touch`:
/// `Some(wrapper)` when the call must go through `stream_metadata` (a
/// non-plain wrapper, or an explicit `file://` URL), `None` for a direct
/// plain-file operation. The inner `None` means no usable wrapper.
fn metadata_wrapper(vm: &mut Vm, filename: &[u8]) -> PResult<Option<Option<WrapperRef>>> {
    let loc = locate_url_wrapper(vm, filename, 0)?;
    let plain = loc.as_ref().is_some_and(|l| is_plain(&l.wrapper));
    if !plain || has_file_scheme(filename) {
        return Ok(Some(loc.map(|l| l.wrapper).filter(|w| w.has_metadata())));
    }
    Ok(None)
}

/// The `$user` / `$group` argument (`Z_PARAM_STR_OR_LONG`).
enum NameOrId {
    Name(PhpStr),
    Id(i64),
}

fn name_or_id(args: &Args, i: usize) -> NameOrId {
    match args.get(i) {
        Value::String(s) => NameOrId::Name(s.clone()),
        Value::Int(n) => NameOrId::Id(*n),
        Value::Bool(b) => NameOrId::Id(*b as i64),
        Value::Float(f) => NameOrId::Id(*f as i64),
        other => panic!("string|int argument after ZPP: {other:?}"),
    }
}

/// `php_do_chown` (`group == false`) and `php_do_chgrp` (`group == true`).
fn do_chown(vm: &mut Vm, args: &Args, group: bool, link: bool) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let who = name_or_id(args, 1);
    let name = filename.as_bytes();

    if let Some(w) = metadata_wrapper(vm, name)? {
        let Some(w) = w else {
            let func = if group { "chgrp" } else { "chown" };
            vm.warning(format!("Cannot call {func}() for a non-standard stream"))?;
            return Ok(Value::Bool(false));
        };
        let value = match (group, who) {
            (false, NameOrId::Name(n)) => MetaValue::OwnerName(n),
            (false, NameOrId::Id(n)) => MetaValue::Owner(n),
            (true, NameOrId::Name(n)) => MetaValue::GroupName(n),
            (true, NameOrId::Id(n)) => MetaValue::Group(n),
        };
        return Ok(Value::Bool(w.metadata(vm, name, &value, None)?));
    }

    let id = match who {
        NameOrId::Name(n) => {
            let found = if group {
                sys::sys_gid_by_name(n.as_bytes())
            } else {
                sys::sys_uid_by_name(n.as_bytes())
            };
            match found {
                Some(id) => id,
                None => {
                    let what: &[u8] = if group { b"gid" } else { b"uid" };
                    warn_bytes(vm, &[b"Unable to find ", what, b" for ", n.as_bytes()])?;
                    return Ok(Value::Bool(false));
                }
            }
        }
        // (uid_t) / (gid_t) casts: -1 means "leave unchanged".
        NameOrId::Id(n) => n as u32,
    };

    // Check the basedir
    if check_open_basedir(vm, name, true)? {
        return Ok(Value::Bool(false));
    }

    let (uid, gid) = if group {
        (None, Some(id))
    } else {
        (Some(id), None)
    };
    let p = plain::os(name);
    let r = if link {
        std::os::unix::fs::lchown(p, uid, gid)
    } else {
        std::os::unix::fs::chown(p, uid, gid)
    };
    if let Err(e) = r {
        warn_errno(vm, &e)?;
        return Ok(Value::Bool(false));
    }
    clear_stat_cache(vm);
    Ok(Value::Bool(true))
}

fn chown(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_chown(vm, args, false, false)
}

fn lchown(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_chown(vm, args, false, true)
}

fn chgrp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_chown(vm, args, true, false)
}

fn lchgrp(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    do_chown(vm, args, true, true)
}

fn chmod(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let mode = args.int(1);
    let name = filename.as_bytes();

    if let Some(w) = metadata_wrapper(vm, name)? {
        let Some(w) = w else {
            vm.warning("Cannot call chmod() for a non-standard stream")?;
            return Ok(Value::Bool(false));
        };
        return Ok(Value::Bool(w.metadata(
            vm,
            name,
            &MetaValue::Access(mode),
            None,
        )?));
    }

    // Check the basedir
    if check_open_basedir(vm, name, true)? {
        return Ok(Value::Bool(false));
    }

    // `(mode_t) mode`: std hands the value to chmod(2) the same way.
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode as u32);
    if let Err(e) = std::fs::set_permissions(plain::os(name), perms) {
        warn_errno(vm, &e)?;
        return Ok(Value::Bool(false));
    }
    clear_stat_cache(vm);
    Ok(Value::Bool(true))
}

fn touch(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = path_param(vm, args, 0)?;
    let filetime = args.int_opt(1);
    let fileatime = args.int_opt(2);
    let name = filename.as_bytes();

    if name.is_empty() {
        return Ok(Value::Bool(false));
    }

    // (modtime, actime); `None` = now
    let newtime = match (filetime, fileatime) {
        (None, None) => None,
        (Some(m), None) => Some((m, m)),
        (None, Some(_)) => {
            return Err(vm.argument_value_error(
                2,
                "cannot be null when argument #3 ($atime) is an integer",
            ));
        }
        (Some(m), Some(a)) => Some((m, a)),
    };

    if let Some(w) = metadata_wrapper(vm, name)? {
        if let Some(w) = w {
            return Ok(Value::Bool(w.metadata(
                vm,
                name,
                &MetaValue::Touch(newtime),
                None,
            )?));
        }
        if newtime.is_some() {
            vm.warning("Cannot call touch() for a non-standard stream")?;
            return Ok(Value::Bool(false));
        }
        let stream = wrappers::open_wrapper(vm, name, b"c", open::REPORT_ERRORS, None, None)?;
        return Ok(Value::Bool(match stream {
            Some(res) => {
                super::streams::close(vm, &res)?;
                true
            }
            None => false,
        }));
    }

    // Check the basedir
    if check_open_basedir(vm, name, true)? {
        return Ok(Value::Bool(false));
    }

    // create the file if it doesn't exist already
    if sys::sys_access(name, sys::F_OK).is_err()
        && let Err(e) = std::fs::File::create(plain::os(name))
    {
        let reason = sys::strerror(sys::errno_of(&e));
        warn_bytes(
            vm,
            &[
                b"Unable to create file ",
                name,
                b" because ",
                reason.as_bytes(),
            ],
        )?;
        return Ok(Value::Bool(false));
    }

    if let Err(e) = creds::set_times(name, newtime) {
        let reason = sys::strerror(sys::errno_of(&e));
        vm.warning(format!("Utime failed: {reason}"))?;
        return Ok(Value::Bool(false));
    }
    clear_stat_cache(vm);
    Ok(Value::Bool(true))
}

// ---------------------------------------------------------------------------
// clearstatcache, disk space, realpath cache
// ---------------------------------------------------------------------------

/// `php_clear_stat_cache`: there is no realpath cache, so
/// `$clear_realpath_cache` and `$filename` have nothing further to clear.
fn clearstatcache(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let _clear_realpath_cache = args.bool(0);
    let _filename = path_param(vm, args, 1)?;
    clear_stat_cache(vm);
    Ok(Value::Null)
}

/// `disk_total_space` (`total == true`) / `disk_free_space`.
fn disk_space(vm: &mut Vm, args: &Args, total: bool) -> PResult<Value> {
    let path = path_param(vm, args, 0)?;
    let Some(fullpath) = plain::expand_filepath(path.as_bytes()) else {
        return Ok(Value::Bool(false));
    };
    if check_open_basedir(vm, &fullpath, true)? {
        return Ok(Value::Bool(false));
    }
    match sys::sys_statvfs(&fullpath) {
        Ok((unit, blocks, bavail)) => Ok(Value::Float(if total { blocks } else { bavail } * unit)),
        Err(e) => {
            warn_errno(vm, &e)?;
            Ok(Value::Bool(false))
        }
    }
}

fn disk_total_space(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    disk_space(vm, args, true)
}

fn disk_free_space(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    disk_space(vm, args, false)
}

fn realpath_cache_size(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(0))
}

fn realpath_cache_get(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::from(PhpArray::new()))
}

static CHOWN: NativeFunction = NativeFunction {
    sig: "chown(string $filename, string|int $user): bool",
    handler: chown,
};
static CHGRP: NativeFunction = NativeFunction {
    sig: "chgrp(string $filename, string|int $group): bool",
    handler: chgrp,
};
static LCHOWN: NativeFunction = NativeFunction {
    sig: "lchown(string $filename, string|int $user): bool",
    handler: lchown,
};
static LCHGRP: NativeFunction = NativeFunction {
    sig: "lchgrp(string $filename, string|int $group): bool",
    handler: lchgrp,
};
static CHMOD: NativeFunction = NativeFunction {
    sig: "chmod(string $filename, int $permissions): bool",
    handler: chmod,
};
static TOUCH: NativeFunction = NativeFunction {
    sig: "touch(string $filename, ?int $mtime = null, ?int $atime = null): bool",
    handler: touch,
};
static CLEARSTATCACHE: NativeFunction = NativeFunction {
    sig: "clearstatcache(bool $clear_realpath_cache = false, string $filename = \"\"): void",
    handler: clearstatcache,
};
static DISK_TOTAL_SPACE: NativeFunction = NativeFunction {
    sig: "disk_total_space(string $directory): float|false",
    handler: disk_total_space,
};
static DISK_FREE_SPACE: NativeFunction = NativeFunction {
    sig: "disk_free_space(string $directory): float|false",
    handler: disk_free_space,
};
/// `@alias disk_free_space`.
static DISKFREESPACE: NativeFunction = NativeFunction {
    sig: "diskfreespace(string $directory): float|false",
    handler: disk_free_space,
};
static REALPATH_CACHE_GET: NativeFunction = NativeFunction {
    sig: "realpath_cache_get(): array",
    handler: realpath_cache_get,
};
static REALPATH_CACHE_SIZE: NativeFunction = NativeFunction {
    sig: "realpath_cache_size(): int",
    handler: realpath_cache_size,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &FILEATIME,
        &FILECTIME,
        &FILEGROUP,
        &FILEINODE,
        &FILEMTIME,
        &FILEOWNER,
        &FILEPERMS,
        &FILESIZE,
        &FILETYPE,
        &FILE_EXISTS,
        &IS_WRITABLE,
        &IS_WRITEABLE,
        &IS_READABLE,
        &IS_EXECUTABLE,
        &IS_FILE,
        &IS_DIR,
        &IS_LINK,
        &STAT,
        &LSTAT,
        &CHOWN,
        &CHGRP,
        &LCHOWN,
        &LCHGRP,
        &CHMOD,
        &TOUCH,
        &CLEARSTATCACHE,
        &DISK_TOTAL_SPACE,
        &DISK_FREE_SPACE,
        &DISKFREESPACE,
        &REALPATH_CACHE_GET,
        &REALPATH_CACHE_SIZE,
    ]);
}

#[cfg(test)]
pub(crate) mod tests {
    //! Expectations from ext/standard/tests/file/{stat*,lstat*,file*,is_*,
    //! chmod*,chown*,chgrp*,lchown*,touch*,disk*,clearstatcache*,
    //! realpath_cache*,statcache-corruption}.phpt.

    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::{ArrayKey, ResRef};
    use std::any::Any;
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::standard::streams::wrappers::{Wrapper, register_volatile};

    /// A unique scratch directory, removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> TempDir {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let d =
                std::env::temp_dir().join(format!("php-filestat-{}-{tag}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }

        pub fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn call(t: &mut TestVm, f: &str, args: Vec<Value>) -> Value {
        t.call(f, args).unwrap()
    }

    fn call1(t: &mut TestVm, f: &str, path: &str) -> Value {
        call(t, f, vec![Value::from(path)])
    }

    fn bool_of(v: Value) -> bool {
        v.as_bool()
            .unwrap_or_else(|| panic!("expected bool, got {v:?}"))
    }

    fn int_of(v: Value) -> i64 {
        v.as_int()
            .unwrap_or_else(|| panic!("expected int, got {v:?}"))
    }

    fn str_of(v: Value) -> String {
        v.as_str()
            .unwrap_or_else(|| panic!("expected string, got {v:?}"))
            .to_string_lossy()
            .into_owned()
    }

    fn is_false(v: &Value) -> bool {
        matches!(v, Value::Bool(false))
    }

    fn warning(f: &str, msg: &str) -> String {
        format!("\nWarning: {f}(): {msg} in /t.php on line 3\n")
    }

    fn is_root() -> bool {
        creds::uid() == 0
    }

    fn stat_array(t: &mut TestVm, f: &str, path: &str) -> PhpArray {
        match call1(t, f, path) {
            Value::Array(a) => (*a).clone(),
            other => panic!("expected array, got {other:?}"),
        }
    }

    fn arr_int(a: &PhpArray, key: &str) -> i64 {
        a.get_str(key.as_bytes()).and_then(Value::as_int).unwrap()
    }

    const NAMES: [&str; 13] = [
        "dev", "ino", "mode", "nlink", "uid", "gid", "rdev", "size", "atime", "mtime", "ctime",
        "blksize", "blocks",
    ];

    #[test]
    fn stat_array_has_numeric_then_named_keys() {
        let d = TempDir::new("stat");
        let f = d.path("file.txt");
        fs::write(&f, "hello").unwrap();
        let m = fs::metadata(&f).unwrap();
        let mut t = TestVm::new();
        let a = stat_array(&mut t, "stat", &f);
        assert_eq!(a.len(), 26);
        let keys: Vec<ArrayKey> = a.keys().collect();
        for (i, k) in keys.iter().take(13).enumerate() {
            assert_eq!(k.as_int(), Some(i as i64));
        }
        for (k, name) in keys.iter().skip(13).zip(NAMES) {
            assert_eq!(k.as_str().unwrap().as_bytes(), name.as_bytes());
        }
        for (i, name) in NAMES.iter().enumerate() {
            assert_eq!(
                a.get_int(i as i64).and_then(Value::as_int),
                Some(arr_int(&a, name)),
                "{name}"
            );
        }
        assert_eq!(arr_int(&a, "size"), 5);
        assert_eq!(arr_int(&a, "dev"), m.dev() as i64);
        assert_eq!(arr_int(&a, "ino"), m.ino() as i64);
        assert_eq!(arr_int(&a, "mode"), m.mode() as i64);
        assert_eq!(arr_int(&a, "nlink"), 1);
        assert_eq!(arr_int(&a, "uid"), m.uid() as i64);
        assert_eq!(arr_int(&a, "gid"), m.gid() as i64);
        assert_eq!(arr_int(&a, "mtime"), m.mtime());
        assert_eq!(arr_int(&a, "blksize"), m.blksize() as i64);
        assert_eq!(arr_int(&a, "blocks"), m.blocks() as i64);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn single_field_functions_match_metadata() {
        let d = TempDir::new("fields");
        let f = d.path("file.txt");
        fs::write(&f, "0123456789").unwrap();
        let m = fs::metadata(&f).unwrap();
        let mut t = TestVm::new();
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 10);
        assert_eq!(int_of(call1(&mut t, "fileinode", &f)), m.ino() as i64);
        assert_eq!(int_of(call1(&mut t, "fileowner", &f)), m.uid() as i64);
        assert_eq!(int_of(call1(&mut t, "filegroup", &f)), m.gid() as i64);
        assert_eq!(int_of(call1(&mut t, "fileperms", &f)), m.mode() as i64);
        assert_eq!(int_of(call1(&mut t, "fileatime", &f)), m.atime());
        assert_eq!(int_of(call1(&mut t, "filemtime", &f)), m.mtime());
        assert_eq!(int_of(call1(&mut t, "filectime", &f)), m.ctime());
        // fileperms_variation3: double slashes are fine
        let dbl = f.replace("/file.txt", "//file.txt");
        assert_eq!(int_of(call1(&mut t, "fileperms", &dbl)), m.mode() as i64);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn stat_failures_warn_with_the_function_name() {
        let d = TempDir::new("fail");
        let missing = d.path("temp.tmp");
        let mut t = TestVm::new();
        assert!(is_false(&call1(&mut t, "stat", &missing)));
        assert_eq!(
            t.output(),
            warning("stat", &format!("stat failed for {missing}"))
        );
        assert!(is_false(&call1(&mut t, "lstat", &missing)));
        assert_eq!(
            t.output(),
            warning("lstat", &format!("Lstat failed for {missing}"))
        );
        // lstat_stat_error: a scalar argument is a (relative) path
        assert!(is_false(&call(&mut t, "lstat", vec![Value::Int(22)])));
        assert_eq!(t.output(), warning("lstat", "Lstat failed for 22"));
        assert!(is_false(&call(&mut t, "stat", vec![Value::Int(22)])));
        assert_eq!(t.output(), warning("stat", "stat failed for 22"));
        for f in [
            "filesize",
            "fileperms",
            "fileinode",
            "fileowner",
            "filegroup",
            "fileatime",
            "filemtime",
            "filectime",
        ] {
            assert!(is_false(&call1(&mut t, f, &missing)), "{f}");
            assert_eq!(
                t.output(),
                warning(f, &format!("stat failed for {missing}")),
                "{f}"
            );
        }
        // filetype is a link operation
        assert!(is_false(&call1(&mut t, "filetype", &missing)));
        assert_eq!(
            t.output(),
            warning("filetype", &format!("Lstat failed for {missing}"))
        );
        // fileperms_variation3: a trailing slash after a file, wildcards
        let f = d.path("f.tmp");
        fs::write(&f, "").unwrap();
        for p in [format!("{f}/"), d.path("*.tmp")] {
            assert!(is_false(&call1(&mut t, "fileperms", &p)));
            assert_eq!(
                t.output(),
                warning("fileperms", &format!("stat failed for {p}"))
            );
        }
    }

    #[test]
    fn empty_filename_is_false_without_warning() {
        let mut t = TestVm::new();
        for f in [
            "stat",
            "lstat",
            "filesize",
            "filetype",
            "fileperms",
            "file_exists",
            "is_file",
            "is_dir",
            "is_link",
            "is_readable",
            "is_writable",
            "is_executable",
        ] {
            assert!(is_false(&call1(&mut t, f, "")), "{f}");
        }
        assert_eq!(t.output(), "");
    }

    #[test]
    fn null_bytes_throw_value_error() {
        let mut t = TestVm::new();
        for f in [
            "is_dir",
            "is_file",
            "is_readable",
            "is_writable",
            "is_executable",
            "fileowner",
            "filegroup",
            "fileinode",
            "fileperms",
            "file_exists",
            "stat",
        ] {
            let r = t.call(f, vec![Value::from("file_with_null_byte.tmp\0")]);
            assert_eq!(
                thrown(r),
                (
                    "ValueError".to_string(),
                    format!("{f}(): Argument #1 ($filename) must not contain any null bytes")
                )
            );
        }
        let r = t.call("chmod", vec![Value::from("a\0b"), Value::Int(0)]);
        assert_eq!(
            thrown(r).1,
            "chmod(): Argument #1 ($filename) must not contain any null bytes"
        );
        let r = t.call(
            "clearstatcache",
            vec![Value::Bool(true), Value::from("a\0")],
        );
        assert_eq!(
            thrown(r).1,
            "clearstatcache(): Argument #2 ($filename) must not contain any null bytes"
        );
    }

    #[test]
    fn existence_checks_are_quiet() {
        let d = TempDir::new("quiet");
        let missing = d.path("nope");
        let mut t = TestVm::new();
        for f in [
            "file_exists",
            "is_file",
            "is_dir",
            "is_link",
            "is_readable",
            "is_writable",
            "is_writeable",
            "is_executable",
        ] {
            assert!(is_false(&call1(&mut t, f, &missing)), "{f}");
        }
        // symlink_link_linkinfo_is_link_error2
        assert!(is_false(&call1(&mut t, "is_link", " ")));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn is_checks_on_files_dirs_and_links() {
        let d = TempDir::new("kinds");
        let file = d.path("file");
        let dir = d.path("dir");
        let flink = d.path("flink");
        let dlink = d.path("dlink");
        let dangling = d.path("dangling");
        fs::write(&file, "x").unwrap();
        fs::create_dir(&dir).unwrap();
        symlink(&file, &flink).unwrap();
        symlink(&dir, &dlink).unwrap();
        symlink(d.path("missing"), &dangling).unwrap();
        let mut t = TestVm::new();
        let check = |t: &mut TestVm, p: &str, exp: [bool; 4], ty: &str| {
            for (f, e) in ["file_exists", "is_file", "is_dir", "is_link"]
                .into_iter()
                .zip(exp)
            {
                assert_eq!(bool_of(call1(t, f, p)), e, "{f}({p})");
            }
            assert_eq!(str_of(call1(t, "filetype", p)), ty, "filetype({p})");
        };
        check(&mut t, &file, [true, true, false, false], "file");
        check(&mut t, &dir, [true, false, true, false], "dir");
        check(&mut t, &flink, [true, true, false, true], "link");
        check(&mut t, &dlink, [true, false, true, true], "link");
        check(&mut t, &dangling, [false, false, false, true], "link");
        // filetype_basic
        assert_eq!(str_of(call1(&mut t, "filetype", ".")), "dir");
        assert_eq!(t.output(), "");
    }

    #[test]
    fn char_device_type() {
        let mut t = TestVm::new();
        assert_eq!(str_of(call1(&mut t, "filetype", "/dev/null")), "char");
        assert!(!bool_of(call1(&mut t, "is_file", "/dev/null")));
        assert!(bool_of(call1(&mut t, "is_readable", "/dev/null")));
        assert!(bool_of(call1(&mut t, "is_writable", "/dev/null")));
    }

    #[test]
    fn file_urls_go_through_the_plain_wrapper() {
        let d = TempDir::new("url");
        let f = d.path("f");
        fs::write(&f, "abc").unwrap();
        let url = format!("file://{f}");
        let mut t = TestVm::new();
        assert_eq!(int_of(call1(&mut t, "filesize", &url)), 3);
        assert!(bool_of(call1(&mut t, "file_exists", &url)));
        assert!(bool_of(call1(&mut t, "is_file", &url)));
        assert!(bool_of(call1(&mut t, "is_readable", &url)));
        assert_eq!(stat_array(&mut t, "stat", &url).len(), 26);
        // chmod/touch on file:// URLs use the wrapper's metadata op
        assert!(bool_of(call(
            &mut t,
            "chmod",
            vec![Value::from(url.as_str()), Value::Int(0o600)]
        )));
        assert_eq!(fs::metadata(&f).unwrap().mode() & 0o7777, 0o600);
        assert!(bool_of(call(
            &mut t,
            "touch",
            vec![Value::from(url.as_str()), Value::Int(1234)]
        )));
        assert_eq!(fs::metadata(&f).unwrap().mtime(), 1234);
        assert_eq!(int_of(call1(&mut t, "filemtime", &f)), 1234);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn unknown_wrapper_falls_back_to_plain_files() {
        let mut t = TestVm::new();
        assert!(is_false(&call1(&mut t, "is_file", "foo://bar")));
        assert_eq!(
            t.output(),
            warning(
                "is_file",
                "Unable to find the wrapper \"foo\" - did you forget to enable it when you configured PHP?"
            )
        );
    }

    #[test]
    fn chmod_changes_fileperms() {
        let d = TempDir::new("chmod");
        let f = d.path("f");
        let sub = d.path("sub");
        fs::write(&f, "").unwrap();
        fs::create_dir(&sub).unwrap();
        let mut t = TestVm::new();
        assert_eq!(int_of(call1(&mut t, "fileperms", &f)) & 0o170000, 0o100000);
        for mode in [0o640, 0o755, 0o4777, 0o000] {
            assert!(bool_of(call(
                &mut t,
                "chmod",
                vec![Value::from(f.as_str()), Value::Int(mode)]
            )));
            // chmod clears the stat cache
            assert_eq!(int_of(call1(&mut t, "fileperms", &f)), 0o100000 | mode);
        }
        assert!(bool_of(call(
            &mut t,
            "chmod",
            vec![Value::from(sub.as_str()), Value::Int(0o711)]
        )));
        assert_eq!(int_of(call1(&mut t, "fileperms", &sub)), 0o040711);
        // chmod_error
        assert!(is_false(&call(
            &mut t,
            "chmod",
            vec![Value::from("___nonExisitingFile___"), Value::Int(0o777)]
        )));
        assert_eq!(t.output(), warning("chmod", "No such file or directory"));
    }

    #[test]
    fn stat_cache_holds_until_cleared() {
        let d = TempDir::new("cache");
        let f = d.path("f");
        fs::write(&f, "12345").unwrap();
        let mut t = TestVm::new();
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 5);
        // modified behind PHP's back: the cached result is returned
        fs::write(&f, "123456789012345").unwrap();
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 5);
        assert_eq!(arr_int(&stat_array(&mut t, "stat", &f), "size"), 5);
        // a different spelling of the path misses the cache
        let other = f.replace("/f", "//f");
        assert_eq!(int_of(call1(&mut t, "filesize", &other)), 15);
        call(&mut t, "clearstatcache", vec![]);
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 15);
        // clearstatcache(true, $filename) clears everything too
        fs::write(&f, "1").unwrap();
        call(
            &mut t,
            "clearstatcache",
            vec![Value::Bool(true), Value::from("/foo/bar")],
        );
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 1);
        // the plain wrapper's writes clear it as well
        fs::write(&f, "12").unwrap();
        clear_stat_cache(&mut t.vm);
        assert_eq!(int_of(call1(&mut t, "filesize", &f)), 2);
        assert!(matches!(
            call(&mut t, "clearstatcache", vec![]),
            Value::Null
        ));
    }

    #[test]
    fn statcache_corruption() {
        let d = TempDir::new("corrupt");
        let f = d.path("f");
        fs::write(&f, "abc").unwrap();
        let mut t = TestVm::new();
        let a = stat_array(&mut t, "stat", &f);
        assert!(!bool_of(call1(&mut t, "is_link", &f)));
        let b = stat_array(&mut t, "stat", &f);
        for name in NAMES {
            assert_eq!(arr_int(&a, name), arr_int(&b, name), "{name}");
        }
    }

    #[test]
    fn lstat_and_stat_caches_are_separate() {
        let d = TempDir::new("lcache");
        let file = d.path("file");
        let link = d.path("link");
        fs::write(&file, "abc").unwrap();
        symlink(&file, &link).unwrap();
        let mut t = TestVm::new();
        let l = stat_array(&mut t, "lstat", &link);
        assert_eq!(arr_int(&l, "mode") & 0o170000, 0o120000);
        // the lstat of a link does not feed the stat cache
        let s = stat_array(&mut t, "stat", &link);
        assert_eq!(arr_int(&s, "mode") & 0o170000, 0o100000);
        assert_eq!(arr_int(&s, "size"), 3);
        assert!(bool_of(call1(&mut t, "is_link", &link)));
        assert!(bool_of(call1(&mut t, "is_file", &link)));
        // ... but the lstat of a non-link does
        let file2 = d.path("file2");
        fs::write(&file2, "abcd").unwrap();
        assert_eq!(arr_int(&stat_array(&mut t, "lstat", &file2), "size"), 4);
        fs::write(&file2, "abcdefgh").unwrap();
        assert_eq!(int_of(call1(&mut t, "filesize", &file2)), 4);
        call(&mut t, "clearstatcache", vec![]);
        assert_eq!(int_of(call1(&mut t, "filesize", &file2)), 8);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn access_checks_use_access() {
        if is_root() {
            return;
        }
        let d = TempDir::new("access");
        let f = d.path("f");
        fs::write(&f, "").unwrap();
        let mut t = TestVm::new();
        let set = |mode: u32| {
            fs::set_permissions(&f, fs::Permissions::from_mode(mode)).unwrap();
        };
        let checks = |t: &mut TestVm| {
            [
                bool_of(call1(t, "is_readable", &f)),
                bool_of(call1(t, "is_writable", &f)),
                bool_of(call1(t, "is_writeable", &f)),
                bool_of(call1(t, "is_executable", &f)),
            ]
        };
        set(0o000);
        assert_eq!(checks(&mut t), [false, false, false, false]);
        set(0o400);
        assert_eq!(checks(&mut t), [true, false, false, false]);
        set(0o200);
        assert_eq!(checks(&mut t), [false, true, true, false]);
        set(0o700);
        assert_eq!(checks(&mut t), [true, true, true, true]);
        // is_executable_variation1: directories are "executable"
        assert!(bool_of(call1(&mut t, "is_executable", ".")));
        assert!(bool_of(call1(&mut t, "is_executable", &d.path(""))));
        assert!(!bool_of(call1(&mut t, "is_executable", &format!("{f}/"))));
        assert!(!bool_of(call1(&mut t, "is_executable", &d.path("*"))));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn touch_creates_files_and_sets_times() {
        let d = TempDir::new("touch");
        let f = d.path("touch.dat");
        let mut t = TestVm::new();
        let touch = |t: &mut TestVm, args: Vec<Value>| {
            let mut v = vec![Value::from(f.as_str())];
            v.extend(args);
            bool_of(call(t, "touch", v))
        };
        assert!(touch(&mut t, vec![]));
        assert!(fs::metadata(&f).is_ok());
        let now = int_of(call1(&mut t, "filemtime", &f));
        assert!(now > 1_600_000_000);
        // touch doesn't alter file contents
        fs::write(&f, "Here is a test line").unwrap();
        assert!(touch(&mut t, vec![]));
        assert_eq!(fs::read(&f).unwrap(), b"Here is a test line");
        let _ = fs::remove_file(&f);
        assert!(touch(&mut t, vec![Value::Int(101)]));
        assert_eq!(int_of(call1(&mut t, "filemtime", &f)), 101);
        assert_eq!(int_of(call1(&mut t, "fileatime", &f)), 101);
        let _ = fs::remove_file(&f);
        assert!(touch(&mut t, vec![Value::Int(-1)]));
        assert_eq!(
            int_of(call1(&mut t, "filemtime", &f)),
            fs::metadata(&f).unwrap().mtime()
        );
        let _ = fs::remove_file(&f);
        assert!(touch(&mut t, vec![Value::Int(100), Value::Int(100)]));
        assert_eq!(int_of(call1(&mut t, "filemtime", &f)), 100);
        let _ = fs::remove_file(&f);
        assert!(touch(&mut t, vec![Value::Int(100), Value::Int(-100)]));
        assert_eq!(int_of(call1(&mut t, "filemtime", &f)), 100);
        // touch_basic: all parameters
        assert!(touch(&mut t, vec![Value::Int(10000), Value::Int(20470)]));
        let a = stat_array(&mut t, "stat", &f);
        assert_eq!(arr_int(&a, "mtime"), 10000);
        assert_eq!(arr_int(&a, "atime"), 20470);
        // explicit nulls are "now"
        assert!(touch(&mut t, vec![Value::Null, Value::Null]));
        assert!(int_of(call1(&mut t, "filemtime", &f)) >= now);
        // directories can be touched too
        let sub = d.path("sub");
        fs::create_dir(&sub).unwrap();
        assert!(bool_of(call(
            &mut t,
            "touch",
            vec![Value::from(sub.as_str()), Value::Int(500)]
        )));
        assert_eq!(fs::metadata(&sub).unwrap().mtime(), 500);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn touch_errors() {
        let mut t = TestVm::new();
        assert!(is_false(&call1(
            &mut t,
            "touch",
            "/no/such/file/or/directory"
        )));
        assert_eq!(
            t.output(),
            warning(
                "touch",
                "Unable to create file /no/such/file/or/directory because No such file or directory"
            )
        );
        let r = t.call(
            "touch",
            vec![
                Value::from("/no/such/file/or/directory"),
                Value::Null,
                Value::Int(1599492068),
            ],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "touch(): Argument #2 ($mtime) cannot be null when argument #3 ($atime) is an integer"
                    .to_string()
            )
        );
        // an empty path is false before the argument checks
        let r = t.call("touch", vec![Value::from(""), Value::Null, Value::Int(1)]);
        assert!(is_false(&r.unwrap()));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn chown_and_chgrp_on_plain_files() {
        let d = TempDir::new("chown");
        let f = d.path("f");
        let link = d.path("link");
        fs::write(&f, "").unwrap();
        symlink(d.path("missing"), &link).unwrap();
        let m = fs::metadata(&f).unwrap();
        let (uid, gid) = (m.uid() as i64, m.gid() as i64);
        let mut t = TestVm::new();
        let run = |t: &mut TestVm, func: &str, p: &str, who: Value| {
            bool_of(call(t, func, vec![Value::from(p), who]))
        };
        assert!(run(&mut t, "chown", &f, Value::Int(uid)));
        assert!(run(&mut t, "chgrp", &f, Value::Int(gid)));
        // -1 leaves the owner unchanged
        assert!(run(&mut t, "chown", &f, Value::Int(-1)));
        assert!(run(&mut t, "lchown", &link, Value::Int(uid)));
        assert!(run(&mut t, "lchgrp", &link, Value::Int(gid)));
        assert_eq!(t.output(), "");

        assert!(!run(
            &mut t,
            "chown",
            &f,
            Value::from("no_such_user_php_rs")
        ));
        assert_eq!(
            t.output(),
            warning("chown", "Unable to find uid for no_such_user_php_rs")
        );
        assert!(!run(
            &mut t,
            "lchgrp",
            &f,
            Value::from("no_such_group_php_rs")
        ));
        assert_eq!(
            t.output(),
            warning("lchgrp", "Unable to find gid for no_such_group_php_rs")
        );
        // chown.phpt / chgrp.phpt
        let missing = "sjhgfskhagkfdgskjfhgskfsdgfkdsajf";
        for func in ["chown", "chgrp", "lchown", "lchgrp"] {
            assert!(!run(&mut t, func, missing, Value::Int(0)));
            assert_eq!(t.output(), warning(func, "No such file or directory"));
        }
        // lchown_error: a bad user
        if !is_root() {
            assert!(!run(&mut t, "lchown", &f, Value::Int(-5)));
            let out = t.output();
            assert!(
                out == warning("lchown", "Operation not permitted")
                    || out == warning("lchown", "Invalid argument"),
                "{out}"
            );
        }
    }

    #[test]
    fn disk_space() {
        let d = TempDir::new("disk");
        let mut t = TestVm::new();
        let free = call1(&mut t, "disk_free_space", "/").as_float().unwrap();
        let total = call1(&mut t, "disk_total_space", "/").as_float().unwrap();
        assert!(free > 0.0 && total >= free, "{free} {total}");
        assert!(call1(&mut t, "diskfreespace", "/").as_float().is_some());
        // a file works as well as a directory (disk_free_space_error)
        let f = d.path("disk_free_space.tmp");
        fs::write(&f, " Garbage data for the temporary file").unwrap();
        assert!(call1(&mut t, "disk_free_space", &f).as_float().is_some());
        assert!(call1(&mut t, "disk_total_space", &f).as_float().is_some());
        assert_eq!(t.output(), "");
        // disk.phpt
        for f in ["disk_free_space", "disk_total_space", "diskfreespace"] {
            assert!(is_false(&call(&mut t, f, vec![Value::Int(-1)])));
            assert_eq!(t.output(), warning(f, "No such file or directory"));
            assert!(is_false(&call1(&mut t, f, "/some/path/here")));
            assert_eq!(t.output(), warning(f, "No such file or directory"));
            assert!(is_false(&call1(&mut t, f, &d.path("dir1"))));
            assert_eq!(t.output(), warning(f, "No such file or directory"));
            assert!(is_false(&call1(&mut t, f, "")));
            assert_eq!(t.output(), "");
        }
    }

    #[test]
    fn realpath_cache_is_empty() {
        let mut t = TestVm::new();
        assert_eq!(int_of(call(&mut t, "realpath_cache_size", vec![])), 0);
        match call(&mut t, "realpath_cache_get", vec![]) {
            Value::Array(a) => assert!(a.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn open_basedir_restrictions() {
        let d = TempDir::new("basedir");
        let inside = d.path("f");
        fs::write(&inside, "x").unwrap();
        let mut t = TestVm::new();
        let base = d.path("");
        let base = base.trim_end_matches('/').to_string();
        assert!(t.vm.ini_set_system("open_basedir", base.as_str()));
        assert_eq!(int_of(call1(&mut t, "filesize", &inside)), 1);
        assert!(bool_of(call1(&mut t, "file_exists", &inside)));
        let denied = |f: &str| {
            warning(
                f,
                &format!(
                    "open_basedir restriction in effect. File(/etc/hosts) is not within the allowed path(s): ({base})"
                ),
            )
        };
        for f in ["stat", "file_exists", "is_readable", "is_file", "filesize"] {
            assert!(is_false(&call1(&mut t, f, "/etc/hosts")), "{f}");
            assert_eq!(t.output(), denied(f), "{f}");
        }
        assert!(is_false(&call(
            &mut t,
            "chmod",
            vec![Value::from("/etc/hosts"), Value::Int(0o644)]
        )));
        assert_eq!(t.output(), denied("chmod"));
        assert!(is_false(&call1(&mut t, "touch", "/etc/hosts")));
        assert_eq!(t.output(), denied("touch"));
    }

    // -- non-plain wrappers --------------------------------------------------

    /// A wrapper recording its `url_stat` and `metadata` calls.
    #[derive(Default)]
    struct MockWrapper {
        sb: RefCell<Option<StatBuf>>,
        stats: RefCell<Vec<(String, i32)>>,
        meta: RefCell<Vec<(String, String)>>,
    }

    impl Wrapper for MockWrapper {
        fn label(&self) -> &'static str {
            "mock"
        }

        fn open(
            &self,
            _vm: &mut Vm,
            _path: &[u8],
            _mode: &[u8],
            _options: i32,
            _opened_path: &mut Option<PhpStr>,
            _ctx: Option<&ResRef>,
        ) -> PResult<Option<ResRef>> {
            Ok(None)
        }

        fn has_url_stat(&self) -> bool {
            true
        }

        fn url_stat(
            &self,
            _vm: &mut Vm,
            url: &[u8],
            flags: i32,
            _ctx: Option<&ResRef>,
        ) -> PResult<Option<StatBuf>> {
            self.stats
                .borrow_mut()
                .push((String::from_utf8_lossy(url).into_owned(), flags));
            Ok(self.sb.borrow().clone())
        }

        fn has_metadata(&self) -> bool {
            true
        }

        fn metadata(
            &self,
            _vm: &mut Vm,
            url: &[u8],
            value: &MetaValue,
            _ctx: Option<&ResRef>,
        ) -> PResult<bool> {
            self.meta.borrow_mut().push((
                String::from_utf8_lossy(url).into_owned(),
                format!("{value:?}"),
            ));
            Ok(true)
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn mock(t: &mut TestVm) -> Rc<MockWrapper> {
        let w = Rc::new(MockWrapper::default());
        assert!(register_volatile(&mut t.vm, b"mock", w.clone()));
        w
    }

    #[test]
    fn metadata_goes_through_the_wrapper() {
        let mut t = TestVm::new();
        let w = mock(&mut t);
        let u = || Value::from("mock://a");
        assert!(bool_of(call(&mut t, "chmod", vec![u(), Value::Int(0o644)])));
        assert!(bool_of(call(
            &mut t,
            "chown",
            vec![u(), Value::from("bob")]
        )));
        assert!(bool_of(call(&mut t, "chown", vec![u(), Value::Int(7)])));
        assert!(bool_of(call(&mut t, "lchown", vec![u(), Value::Int(8)])));
        assert!(bool_of(call(
            &mut t,
            "chgrp",
            vec![u(), Value::from("staff")]
        )));
        assert!(bool_of(call(&mut t, "lchgrp", vec![u(), Value::Int(9)])));
        assert!(bool_of(call(&mut t, "touch", vec![u()])));
        assert!(bool_of(call(&mut t, "touch", vec![u(), Value::Int(5)])));
        assert!(bool_of(call(
            &mut t,
            "touch",
            vec![u(), Value::Int(5), Value::Int(6)]
        )));
        let meta: Vec<String> = w.meta.borrow().iter().map(|(_, v)| v.clone()).collect();
        assert_eq!(
            meta,
            [
                "Access(420)",
                "OwnerName(\"bob\")",
                "Owner(7)",
                "Owner(8)",
                "GroupName(\"staff\")",
                "Group(9)",
                "Touch(None)",
                "Touch(Some((5, 5)))",
                "Touch(Some((5, 6)))",
            ]
        );
        assert!(w.meta.borrow().iter().all(|(url, _)| url == "mock://a"));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn wrappers_without_metadata_cannot_be_changed() {
        let mut t = TestVm::new();
        let u = || Value::from("data://text/plain,x");
        assert!(is_false(&call(&mut t, "chmod", vec![u(), Value::Int(0)])));
        assert_eq!(
            t.output(),
            warning("chmod", "Cannot call chmod() for a non-standard stream")
        );
        assert!(is_false(&call(&mut t, "chown", vec![u(), Value::Int(0)])));
        assert_eq!(
            t.output(),
            warning("chown", "Cannot call chown() for a non-standard stream")
        );
        assert!(is_false(&call(&mut t, "lchgrp", vec![u(), Value::Int(0)])));
        assert_eq!(
            t.output(),
            warning("lchgrp", "Cannot call chgrp() for a non-standard stream")
        );
        assert!(is_false(&call(&mut t, "touch", vec![u(), Value::Int(1)])));
        assert_eq!(
            t.output(),
            warning("touch", "Cannot call touch() for a non-standard stream")
        );
        // without times, touch() opens the stream in "c" mode
        assert!(bool_of(call1(&mut t, "touch", "php://memory")));
        assert_eq!(t.output(), "");
        // and stat() needs url_stat
        assert!(is_false(&call1(&mut t, "stat", "php://memory")));
        assert_eq!(t.output(), warning("stat", "stat failed for php://memory"));
        assert!(is_false(&call1(&mut t, "file_exists", "php://memory")));
        assert_eq!(t.output(), "");
    }

    fn mock_sb(mode: i64, uid: i64, gid: i64) -> StatBuf {
        StatBuf {
            mode,
            uid,
            gid,
            size: 42,
            nlink: 1,
            ..StatBuf::default()
        }
    }

    #[test]
    fn stat_through_a_wrapper() {
        let mut t = TestVm::new();
        let w = mock(&mut t);
        let uid = i64::from(creds::uid());
        *w.sb.borrow_mut() = Some(mock_sb(S_IFREG | 0o640, uid, -1));
        assert_eq!(int_of(call1(&mut t, "filesize", "mock://x")), 42);
        // served from the stat cache
        assert_eq!(int_of(call1(&mut t, "fileperms", "mock://x")), 0o100640);
        assert_eq!(w.stats.borrow().len(), 1);
        call(&mut t, "clearstatcache", vec![]);
        assert!(bool_of(call1(&mut t, "is_file", "mock://x")));
        assert!(bool_of(call1(&mut t, "file_exists", "mock://x")));
        assert!(!bool_of(call1(&mut t, "is_link", "mock://y")));
        assert_eq!(stat_array(&mut t, "lstat", "mock://z").len(), 26);
        let flags: Vec<(String, i32)> = w.stats.borrow()[1..].to_vec();
        assert_eq!(
            flags,
            [
                ("mock://x".to_string(), URL_STAT_QUIET | 4),
                ("mock://y".to_string(), URL_STAT_LINK | URL_STAT_QUIET | 4),
                ("mock://z".to_string(), URL_STAT_LINK | 4),
            ]
        );
        assert_eq!(t.output(), "");

        *w.sb.borrow_mut() = None;
        call(&mut t, "clearstatcache", vec![]);
        assert!(is_false(&call1(&mut t, "stat", "mock://x")));
        assert_eq!(t.output(), warning("stat", "stat failed for mock://x"));
        assert!(is_false(&call1(&mut t, "lstat", "mock://x")));
        assert_eq!(t.output(), warning("lstat", "Lstat failed for mock://x"));
        assert!(is_false(&call1(&mut t, "is_dir", "mock://x")));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn wrapper_permissions_use_the_owner_group_and_other_bits() {
        if is_root() {
            return;
        }
        let mut t = TestVm::new();
        let w = mock(&mut t);
        let uid = i64::from(creds::uid());
        let gid = i64::from(creds::gid());
        let other = 0x7fff_fff0;
        let mut check = |sb: StatBuf| {
            *w.sb.borrow_mut() = Some(sb);
            call(&mut t, "clearstatcache", vec![]);
            [
                bool_of(call1(&mut t, "is_readable", "mock://p")),
                bool_of(call1(&mut t, "is_writable", "mock://p")),
                bool_of(call1(&mut t, "is_executable", "mock://p")),
            ]
        };
        // owner bits
        assert_eq!(
            check(mock_sb(S_IFREG | 0o477, uid, gid)),
            [true, false, false]
        );
        assert_eq!(
            check(mock_sb(S_IFREG | 0o300, uid, gid)),
            [false, true, true]
        );
        // group bits
        assert_eq!(
            check(mock_sb(S_IFREG | 0o747, other, gid)),
            [true, false, false]
        );
        assert_eq!(
            check(mock_sb(S_IFREG | 0o037, other, gid)),
            [false, true, true]
        );
        // other bits
        assert_eq!(
            check(mock_sb(S_IFREG | 0o774, other, other)),
            [true, false, false]
        );
        assert_eq!(
            check(mock_sb(S_IFREG | 0o003, other, other)),
            [false, true, true]
        );
    }

    #[test]
    fn unknown_file_type_notice() {
        let mut t = TestVm::new();
        let w = mock(&mut t);
        *w.sb.borrow_mut() = Some(mock_sb(0o644, 0, 0));
        assert_eq!(str_of(call1(&mut t, "filetype", "mock://q")), "unknown");
        assert_eq!(
            t.output(),
            "\nNotice: filetype(): Unknown file type (0) in /t.php on line 3\n"
        );
        for (mode, name) in [
            (S_IFIFO, "fifo"),
            (S_IFCHR, "char"),
            (S_IFDIR, "dir"),
            (S_IFBLK, "block"),
            (S_IFREG, "file"),
            (S_IFLNK, "link"),
            (S_IFSOCK, "socket"),
        ] {
            *w.sb.borrow_mut() = Some(mock_sb(mode | 0o644, 0, 0));
            call(&mut t, "clearstatcache", vec![]);
            assert_eq!(str_of(call1(&mut t, "filetype", "mock://q")), name);
        }
    }
}
