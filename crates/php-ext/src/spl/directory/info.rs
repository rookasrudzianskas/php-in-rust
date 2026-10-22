//! `SplFileInfo` (spl_directory.c).

use php_runtime::{Args, ClassRef, NativeFunction, PResult, PhpStr, Value, Vm};

use super::{
    FsType, basename, create_info, create_type_file, create_type_info, deprecated_method,
    ensure_file_name, extension, get_file_name, get_path, get_pathname, info_class,
    info_set_filename, not_initialized, path_arg, short_file_name, spl_class, with_error_handling,
    with_fs,
};
use crate::spl::engine::this;
use crate::standard::filestat::{self, php_stat};
use crate::standard::streams::{plain, sys};

/// `MAXPATHLEN` (readlink truncates to `MAXPATHLEN - 1` bytes).
#[cfg(target_os = "linux")]
const MAXPATHLEN: usize = 4096;
#[cfg(not(target_os = "linux"))]
const MAXPATHLEN: usize = 1024;

/// `SplFileInfo::__construct()`.
fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let path = path_arg(vm, args, 0)?;
    let obj = this(args);
    with_fs(&obj, |st| info_set_filename(st, &path));
    Ok(Value::Null)
}

fn get_path_m(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let path = with_fs(&this(args), |st| get_path(st));
    Ok(Value::String(path.unwrap_or_default()))
}

/// The file name relative to the path, "Object not initialized" without one.
fn short_name(vm: &mut Vm, args: &Args) -> PResult<PhpStr> {
    let name = with_fs(&this(args), |st| {
        let f = st.file_name.clone()?;
        Some(short_file_name(st, &f))
    });
    name.ok_or_else(|| not_initialized(vm))
}

fn get_filename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(short_name(vm, args)?))
}

fn get_extension(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = short_name(vm, args)?;
    Ok(Value::String(extension(&name)))
}

fn get_basename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let suffix = args.str(0).clone();
    let name = short_name(vm, args)?;
    Ok(Value::bytes(basename(&name, &suffix)))
}

fn get_pathname_m(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let path = with_fs(&this(args), get_pathname);
    Ok(Value::String(path.unwrap_or_default()))
}

/// `FileInfoFunction`: `php_stat()` of the file name, its warnings turned
/// into `RuntimeException`s.
fn stat_method(vm: &mut Vm, args: &Args, ty: i32) -> PResult<Value> {
    let file_name = get_file_name(vm, &this(args))?;
    with_error_handling(vm, "RuntimeException", |vm| php_stat(vm, &file_name, ty))
}

macro_rules! stat_methods {
    ($($fn_name:ident => $ty:expr;)*) => {
        $(fn $fn_name(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            stat_method(vm, args, $ty)
        })*
    };
}

stat_methods! {
    get_perms => filestat::FS_PERMS;
    get_inode => filestat::FS_INODE;
    get_size => filestat::FS_SIZE;
    get_owner => filestat::FS_OWNER;
    get_group => filestat::FS_GROUP;
    get_atime => filestat::FS_ATIME;
    get_mtime => filestat::FS_MTIME;
    get_ctime => filestat::FS_CTIME;
    get_type => filestat::FS_TYPE;
    is_writable => filestat::FS_IS_W;
    is_readable => filestat::FS_IS_R;
    is_executable => filestat::FS_IS_X;
    is_file => filestat::FS_IS_FILE;
    is_dir => filestat::FS_IS_DIR;
    is_link => filestat::FS_IS_LINK;
}

/// `SplFileInfo::getLinkTarget()` (`HAVE_SYMLINK` branch).
fn get_link_target(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let file_name = get_file_name(vm, &this(args))?;
    let target = if file_name.starts_with(b"/") {
        file_name.to_vec()
    } else {
        match plain::expand_only(&file_name) {
            Some(p) => p,
            None => {
                vm.warning("No such file or directory")?;
                return Ok(Value::Bool(false));
            }
        }
    };
    match std::fs::read_link(plain::os(&target)) {
        Ok(link) => {
            use std::os::unix::ffi::OsStrExt;
            let bytes = link.as_os_str().as_bytes();
            Ok(Value::bytes(&bytes[..bytes.len().min(MAXPATHLEN - 1)]))
        }
        Err(e) => {
            let mut msg = b"Unable to read link ".to_vec();
            msg.extend_from_slice(&file_name);
            msg.extend_from_slice(b", error: ");
            msg.extend_from_slice(sys::strerror(sys::errno_of(&e)).as_bytes());
            Err(vm.throw_error("RuntimeException", PhpStr::from_vec(msg)))
        }
    }
}

/// `SplFileInfo::getRealPath()`.
fn get_real_path(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = with_fs(&this(args), |st| {
        if st.ty == FsType::Dir && st.file_name.is_none() && !st.entry.is_empty() {
            ensure_file_name(st);
        }
        st.orig_path.clone().or_else(|| st.file_name.clone())
    });
    Ok(match filename.and_then(|f| plain::real_path(&f)) {
        Some(p) => Value::String(p),
        None => Value::Bool(false),
    })
}

/// `SplFileInfo::openFile()`.
fn open_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = create_type_file(vm, &this(args), args)?;
    Ok(Value::Object(obj))
}

/// `Z_PARAM_CLASS` / `zend_parse_arg_class` against `base`: the class
/// named by argument #1 (`None` when the argument is null and `nullable`).
fn class_arg(
    vm: &mut Vm,
    args: &Args,
    base: Option<&ClassRef>,
    nullable: bool,
) -> PResult<Option<ClassRef>> {
    let name = match args.get(0) {
        Value::String(s) => s.clone(),
        _ if nullable => return Ok(None),
        _ => PhpStr::new(),
    };
    let ce = vm.lookup_class(&name)?;
    let ok = match (&ce, base) {
        (Some(ce), Some(base)) => ce.instance_of(base),
        (Some(_), None) => true,
        (None, _) => false,
    };
    if ok {
        return Ok(ce);
    }
    let msg = match base {
        Some(base) => format!(
            "must be a class name derived from {}, \"{name}\" given",
            base.name
        ),
        None if nullable => format!("must be a valid class name or null, \"{name}\" given"),
        None => format!("must be a valid class name, \"{name}\" given"),
    };
    Err(vm.argument_type_error(1, &msg))
}

fn set_file_class(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let base = spl_class(vm, "SplFileObject");
    let ce = class_arg(vm, args, Some(&base), false)?;
    with_fs(&this(args), |st| st.file_class = ce);
    Ok(Value::Null)
}

fn set_info_class(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let base = spl_class(vm, "SplFileInfo");
    let ce = class_arg(vm, args, Some(&base), false)?;
    with_fs(&this(args), |st| st.info_class = ce);
    Ok(Value::Null)
}

/// `SplFileInfo::getFileInfo()`: `$class` must derive from the current
/// info class.
fn get_file_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let base = info_class(vm, &obj);
    let ce = class_arg(vm, args, Some(&base), true)?.unwrap_or(base);
    Ok(Value::Object(create_type_info(vm, &obj, Some(ce))?))
}

/// `SplFileInfo::getPathInfo()`.
fn get_path_info(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let ce = match class_arg(vm, args, None, true)? {
        None => info_class(vm, &obj),
        Some(ce) => {
            let base = spl_class(vm, "SplFileInfo");
            if !ce.instance_of(&base) {
                return Err(vm.argument_type_error(
                    1,
                    &format!(
                        "must be a class name derived from {} or null, {} given",
                        base.name, ce.name
                    ),
                ));
            }
            ce
        }
    };
    let path = with_fs(&obj, get_pathname);
    match path {
        Some(path) if !path.is_empty() => {
            let dpath = crate::standard::link::zend_dirname(&path);
            Ok(Value::Object(create_info(
                vm,
                PhpStr::from_vec(dpath),
                &ce,
            )?))
        }
        _ => Ok(Value::Null),
    }
}

fn debug_info_m(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(super::debug_info(&this(args))))
}

/// `SplFileInfo::_bad_state_ex()` (`#[\Deprecated(since: '8.2')]`).
fn bad_state_ex(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, "SplFileInfo", "_bad_state_ex", "8.2")?;
    Err(vm.throw_error(
        "Error",
        "The parent constructor was not called: the object is in an invalid state",
    ))
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

natives! {
    CONSTRUCT: "public function __construct(string $filename) {}" => construct;
    GET_PATH: "public function getPath(): string {}" => get_path_m;
    GET_FILENAME: "public function getFilename(): string {}" => get_filename;
    GET_EXTENSION: "public function getExtension(): string {}" => get_extension;
    GET_BASENAME: r#"public function getBasename(string $suffix = ""): string {}"# => get_basename;
    GET_PATHNAME: "public function getPathname(): string {}" => get_pathname_m;
    GET_PERMS: "public function getPerms(): int|false {}" => get_perms;
    GET_INODE: "public function getInode(): int|false {}" => get_inode;
    GET_SIZE: "public function getSize(): int|false {}" => get_size;
    GET_OWNER: "public function getOwner(): int|false {}" => get_owner;
    GET_GROUP: "public function getGroup(): int|false {}" => get_group;
    GET_ATIME: "public function getATime(): int|false {}" => get_atime;
    GET_MTIME: "public function getMTime(): int|false {}" => get_mtime;
    GET_CTIME: "public function getCTime(): int|false {}" => get_ctime;
    GET_TYPE: "public function getType(): string|false {}" => get_type;
    IS_WRITABLE: "public function isWritable(): bool {}" => is_writable;
    IS_READABLE: "public function isReadable(): bool {}" => is_readable;
    IS_EXECUTABLE: "public function isExecutable(): bool {}" => is_executable;
    IS_FILE: "public function isFile(): bool {}" => is_file;
    IS_DIR: "public function isDir(): bool {}" => is_dir;
    IS_LINK: "public function isLink(): bool {}" => is_link;
    GET_LINK_TARGET: "public function getLinkTarget(): string|false {}" => get_link_target;
    GET_REAL_PATH: "public function getRealPath(): string|false {}" => get_real_path;
    GET_FILE_INFO: "public function getFileInfo(?string $class = null): SplFileInfo {}" => get_file_info;
    GET_PATH_INFO: "public function getPathInfo(?string $class = null): ?SplFileInfo {}" => get_path_info;
    OPEN_FILE: r#"public function openFile(string $mode = "r", bool $useIncludePath = false, $context = null): SplFileObject {}"# => open_file;
    SET_FILE_CLASS: "public function setFileClass(string $class = SplFileObject::class): void {}" => set_file_class;
    SET_INFO_CLASS: "public function setInfoClass(string $class = SplFileInfo::class): void {}" => set_info_class;
    // @implementation-alias SplFileInfo::getPathname
    TO_STRING: "public function __toString(): string {}" => get_pathname_m;
    DEBUG_INFO: "public function __debugInfo(): array {}" => debug_info_m;
    // #[\Deprecated(since: '8.2')]
    BAD_STATE_EX: "final public function _bad_state_ex(): void {}" => bad_state_ex;
}

pub(super) static METHODS: &[&NativeFunction] = &[
    &CONSTRUCT,
    &GET_PATH,
    &GET_FILENAME,
    &GET_EXTENSION,
    &GET_BASENAME,
    &GET_PATHNAME,
    &GET_PERMS,
    &GET_INODE,
    &GET_SIZE,
    &GET_OWNER,
    &GET_GROUP,
    &GET_ATIME,
    &GET_MTIME,
    &GET_CTIME,
    &GET_TYPE,
    &IS_WRITABLE,
    &IS_READABLE,
    &IS_EXECUTABLE,
    &IS_FILE,
    &IS_DIR,
    &IS_LINK,
    &GET_LINK_TARGET,
    &GET_REAL_PATH,
    &GET_FILE_INFO,
    &GET_PATH_INFO,
    &OPEN_FILE,
    &SET_FILE_CLASS,
    &SET_INFO_CLASS,
    &TO_STRING,
    &DEBUG_INFO,
];

pub(super) static DEPRECATED_METHODS: &[&NativeFunction] = &[&BAD_STATE_EX];
