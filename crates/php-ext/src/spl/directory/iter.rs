//! `DirectoryIterator`, `FilesystemIterator`, `RecursiveDirectoryIterator`
//! and `GlobIterator` (spl_directory.c).

use php_runtime::{Args, NativeFunction, ObjRef, PResult, PhpStr, Value, Vm};

use super::{
    CURRENT_AS_FILEINFO, CURRENT_AS_PATHNAME, CURRENT_AS_SELF, CURRENT_MODE_MASK, FOLLOW_SYMLINKS,
    KEY_AS_FILENAME, KEY_AS_PATHNAME, KEY_MODE_MASK, OTHERS_MASK, SKIP_DOTS, basename,
    create_type_info, dir_open, dir_read, dir_read_skipping, dir_rewind, extension, get_file_name,
    glob_path, is_dot, not_initialized, path_arg, with_error_handling, with_fs,
};
use crate::spl::engine::this;
use crate::standard::filestat::{FS_IS_DIR, FS_LPERMS, php_stat};
use crate::standard::streams::glob_wrapper::GlobOps;
use crate::standard::streams::handle_of;

const S_IFMT: i64 = 0o170000;
const S_IFDIR: i64 = 0o040000;
const S_IFLNK: i64 = 0o120000;

/// `DIT_CTOR_FLAGS`: the constructor takes `$flags`.
const DIT_CTOR_FLAGS: i64 = 0x1;
/// `DIT_CTOR_GLOB`: the path is a glob pattern.
const DIT_CTOR_GLOB: i64 = 0x2;

/// `spl_filesystem_object_construct`.
fn construct(vm: &mut Vm, args: &Args, ctor_flags: i64) -> PResult<Value> {
    let path = path_arg(vm, args, 0)?;
    let mut flags = ctor_flags & !DIT_CTOR_FLAGS;
    if ctor_flags & DIT_CTOR_FLAGS != 0 {
        flags |= KEY_AS_PATHNAME | CURRENT_AS_FILEINFO;
        if args.is_passed(1) {
            flags = args.int(1);
        }
    } else {
        flags |= KEY_AS_PATHNAME | CURRENT_AS_SELF;
    }
    if path.is_empty() {
        return Err(vm.argument_value_error(1, "must not be empty"));
    }
    let obj = this(args);
    if with_fs(&obj, |st| st.path.is_some()) {
        return Err(vm.throw_error("Error", "Directory object is already initialized"));
    }
    with_fs(&obj, |st| st.flags = flags);
    // spl_filesystem_dir_open() may emit an E_WARNING
    with_error_handling(vm, "UnexpectedValueException", |vm| {
        if ctor_flags & DIT_CTOR_GLOB != 0 && !path.starts_with(b"glob://") {
            let mut p = b"glob://".to_vec();
            p.extend_from_slice(&path);
            dir_open(vm, &obj, &p)
        } else {
            dir_open(vm, &obj, &path)
        }
    })?;
    Ok(Value::Null)
}

/// `CHECK_DIRECTORY_ITERATOR_IS_INITIALIZED`.
fn check_initialized(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    if with_fs(obj, |st| st.dirp.is_none()) {
        return Err(not_initialized(vm));
    }
    Ok(())
}

/// The current entry, after [`check_initialized`].
fn entry(vm: &mut Vm, args: &Args) -> PResult<Vec<u8>> {
    let obj = this(args);
    check_initialized(vm, &obj)?;
    Ok(with_fs(&obj, |st| st.entry.clone()))
}

// ---------------------------------------------------------------------------
// DirectoryIterator
// ---------------------------------------------------------------------------

fn di_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, 0)
}

fn di_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    check_initialized(vm, &obj)?;
    with_fs(&obj, |st| st.index = 0);
    dir_rewind(vm, &obj)?;
    dir_read(vm, &obj)?;
    Ok(Value::Null)
}

fn di_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    check_initialized(vm, &obj)?;
    Ok(Value::Int(with_fs(&obj, |st| st.index)))
}

fn di_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    check_initialized(vm, &obj)?;
    Ok(Value::Object(obj))
}

fn di_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    check_initialized(vm, &obj)?;
    with_fs(&obj, |st| st.index += 1);
    dir_read_skipping(vm, &obj)?;
    with_fs(&obj, |st| st.file_name = None);
    Ok(Value::Null)
}

/// `DirectoryIterator::seek()`: moves with the (possibly overridden)
/// `rewind()`, `valid()` and `next()`.
fn di_seek(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let pos = args.int(0);
    let obj = this(args);
    check_initialized(vm, &obj)?;
    if with_fs(&obj, |st| st.index) > pos {
        // we first rewind
        vm.call_method(&obj, b"rewind", Vec::new())?;
    }
    while with_fs(&obj, |st| st.index) < pos {
        let valid = vm.call_method(&obj, b"valid", Vec::new())?;
        if !vm.to_bool(&valid.into_unref())? {
            return Err(vm.throw_error(
                "OutOfBoundsException",
                format!("Seek position {pos} is out of range"),
            ));
        }
        vm.call_method(&obj, b"next", Vec::new())?;
    }
    Ok(Value::Null)
}

fn di_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(!entry(vm, args)?.is_empty()))
}

fn di_get_filename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::bytes(&entry(vm, args)?))
}

fn di_get_extension(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(extension(&entry(vm, args)?)))
}

fn di_get_basename(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let suffix = args.str(0).clone();
    Ok(Value::bytes(basename(&entry(vm, args)?, &suffix)))
}

fn di_is_dot(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(is_dot(&entry(vm, args)?)))
}

// ---------------------------------------------------------------------------
// FilesystemIterator
// ---------------------------------------------------------------------------

fn fi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, DIT_CTOR_FLAGS | SKIP_DOTS)
}

/// `FilesystemIterator::rewind()` (skips dots, no initialisation check).
fn fi_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    with_fs(&obj, |st| st.index = 0);
    dir_rewind(vm, &obj)?;
    dir_read_skipping(vm, &obj)?;
    Ok(Value::Null)
}

/// `FilesystemIterator::key()`: the file name or the path name.
fn fi_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let (flags, entry) = with_fs(&obj, |st| (st.flags, st.entry.clone()));
    if flags & KEY_MODE_MASK == KEY_AS_FILENAME {
        return Ok(Value::bytes(&entry));
    }
    Ok(Value::String(get_file_name(vm, &obj)?))
}

/// `FilesystemIterator::current()`: the path name, an `SplFileInfo` (of
/// the info class) or `$this`.
fn fi_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let mode = with_fs(&obj, |st| st.flags & CURRENT_MODE_MASK);
    if mode == CURRENT_AS_PATHNAME {
        Ok(Value::String(get_file_name(vm, &obj)?))
    } else if mode == CURRENT_AS_FILEINFO {
        get_file_name(vm, &obj)?;
        Ok(Value::Object(create_type_info(vm, &obj, None)?))
    } else {
        Ok(Value::Object(obj))
    }
}

fn fi_get_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = with_fs(&this(args), |st| st.flags);
    Ok(Value::Int(
        flags & (KEY_MODE_MASK | CURRENT_MODE_MASK | OTHERS_MASK),
    ))
}

fn fi_set_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let flags = args.int(0);
    let mask = KEY_MODE_MASK | CURRENT_MODE_MASK | OTHERS_MASK;
    with_fs(&this(args), |st| {
        st.flags = (st.flags & !mask) | (flags & mask)
    });
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// RecursiveDirectoryIterator
// ---------------------------------------------------------------------------

fn rdi_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, DIT_CTOR_FLAGS)
}

/// `RecursiveDirectoryIterator::hasChildren()`. php-src first looks at the
/// entry's `d_type`; the stream layer does not report it, so the `lstat()`
/// fallback (which gives the same answers) is always used.
fn rdi_has_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let allow_links = args.bool(0);
    let obj = this(args);
    let (entry, flags) = with_fs(&obj, |st| (st.entry.clone(), st.flags));
    if entry.is_empty() || is_dot(&entry) {
        return Ok(Value::Bool(false));
    }
    let file_name = get_file_name(vm, &obj)?;
    let mode = match php_stat(vm, &file_name, FS_LPERMS)? {
        Value::Int(m) => m,
        other => return Ok(other),
    };
    if mode & S_IFMT != S_IFLNK {
        return Ok(Value::Bool(mode & S_IFMT == S_IFDIR));
    }
    if !allow_links && flags & FOLLOW_SYMLINKS == 0 {
        return Ok(Value::Bool(false));
    }
    php_stat(vm, &file_name, FS_IS_DIR)
}

/// `RecursiveDirectoryIterator::getChildren()`: a new object of the same
/// class (constructed with the entry's path and the flags) with the sub
/// path, info class and file class carried over.
fn rdi_get_children(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let file_name = get_file_name(vm, &obj)?;
    let (flags, sub_path, entry, info_class, file_class) = with_fs(&obj, |st| {
        (
            st.flags,
            st.sub_path.clone(),
            st.entry.clone(),
            st.info_class.clone(),
            st.file_class.clone(),
        )
    });
    let ce = obj.class().clone();
    let child = vm.instantiate(&ce)?;
    vm.call_method(
        &child,
        b"__construct",
        vec![Value::String(file_name), Value::Int(flags)],
    )?;
    let sub = match sub_path {
        Some(s) if !s.is_empty() => {
            let mut v = s.to_vec();
            v.push(b'/');
            v.extend_from_slice(&entry);
            v
        }
        _ => entry,
    };
    with_fs(&child, |st| {
        st.sub_path = Some(PhpStr::from_vec(sub));
        st.info_class = info_class;
        st.file_class = file_class;
    });
    Ok(Value::Object(child))
}

fn rdi_get_sub_path(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let sub = with_fs(&this(args), |st| st.sub_path.clone());
    Ok(Value::String(sub.unwrap_or_default()))
}

fn rdi_get_sub_pathname(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (sub, entry) = with_fs(&this(args), |st| (st.sub_path.clone(), st.entry.clone()));
    Ok(match sub {
        Some(s) => {
            let mut v = s.to_vec();
            v.push(b'/');
            v.extend_from_slice(&entry);
            Value::from(v)
        }
        None => Value::from(entry),
    })
}

// ---------------------------------------------------------------------------
// GlobIterator
// ---------------------------------------------------------------------------

fn glob_construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    construct(vm, args, DIT_CTOR_FLAGS | DIT_CTOR_GLOB)
}

/// `GlobIterator::count()`: the number of matches.
fn glob_count(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let dirp = with_fs(&this(args), |st| st.dirp.clone());
    let count = dirp
        .filter(|r| glob_path(r).is_some())
        .and_then(|r| handle_of(&r))
        .and_then(|h| {
            h.try_borrow()
                .ok()
                .and_then(|s| s.ops_ref::<GlobOps>().map(GlobOps::count))
        });
    match count {
        Some(n) => Ok(Value::Int(n as i64)),
        // This can happen by avoiding constructors in specially-crafted code.
        None => Err(vm.throw_error("Error", "GlobIterator is not initialized")),
    }
}

macro_rules! natives {
    ($($name:ident: $sig:expr => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

natives! {
    DI_CONSTRUCT: "public function __construct(string $directory) {}" => di_construct;
    DI_GET_FILENAME: "public function getFilename(): string {}" => di_get_filename;
    DI_GET_EXTENSION: "public function getExtension(): string {}" => di_get_extension;
    DI_GET_BASENAME: r#"public function getBasename(string $suffix = ""): string {}"# => di_get_basename;
    DI_IS_DOT: "public function isDot(): bool {}" => di_is_dot;
    DI_REWIND: "public function rewind(): void {}" => di_rewind;
    DI_VALID: "public function valid(): bool {}" => di_valid;
    DI_KEY: "public function key(): int|string {}" => di_key;
    DI_CURRENT: "public function current(): string|SplFileInfo|static {}" => di_current;
    DI_NEXT: "public function next(): void {}" => di_next;
    DI_SEEK: "public function seek(int $offset): void {}" => di_seek;
    // @implementation-alias DirectoryIterator::getFilename
    DI_TO_STRING: "public function __toString(): string {}" => di_get_filename;

    FI_CONSTRUCT: "public function __construct(string $directory, int $flags = FilesystemIterator::KEY_AS_PATHNAME | FilesystemIterator::CURRENT_AS_FILEINFO | FilesystemIterator::SKIP_DOTS) {}" => fi_construct;
    FI_REWIND: "public function rewind(): void {}" => fi_rewind;
    FI_KEY: "public function key(): string {}" => fi_key;
    FI_CURRENT: "public function current(): string|SplFileInfo|static {}" => fi_current;
    FI_GET_FLAGS: "public function getFlags(): int {}" => fi_get_flags;
    FI_SET_FLAGS: "public function setFlags(int $flags): void {}" => fi_set_flags;

    RDI_CONSTRUCT: "public function __construct(string $directory, int $flags = FilesystemIterator::KEY_AS_PATHNAME | FilesystemIterator::CURRENT_AS_FILEINFO) {}" => rdi_construct;
    RDI_HAS_CHILDREN: "public function hasChildren(bool $allowLinks = false): bool {}" => rdi_has_children;
    RDI_GET_CHILDREN: "public function getChildren(): RecursiveDirectoryIterator {}" => rdi_get_children;
    RDI_GET_SUB_PATH: "public function getSubPath(): string {}" => rdi_get_sub_path;
    RDI_GET_SUB_PATHNAME: "public function getSubPathname(): string {}" => rdi_get_sub_pathname;

    GLOB_CONSTRUCT: "public function __construct(string $pattern, int $flags = FilesystemIterator::KEY_AS_PATHNAME | FilesystemIterator::CURRENT_AS_FILEINFO) {}" => glob_construct;
    GLOB_COUNT: "public function count(): int {}" => glob_count;
}

pub(super) static DIRECTORY_ITERATOR_METHODS: &[&NativeFunction] = &[
    &DI_CONSTRUCT,
    &DI_GET_FILENAME,
    &DI_GET_EXTENSION,
    &DI_GET_BASENAME,
    &DI_IS_DOT,
    &DI_REWIND,
    &DI_VALID,
    &DI_KEY,
    &DI_CURRENT,
    &DI_NEXT,
    &DI_SEEK,
    &DI_TO_STRING,
];

pub(super) static FILESYSTEM_ITERATOR_METHODS: &[&NativeFunction] = &[
    &FI_CONSTRUCT,
    &FI_REWIND,
    &FI_KEY,
    &FI_CURRENT,
    &FI_GET_FLAGS,
    &FI_SET_FLAGS,
];

pub(super) static RECURSIVE_DIRECTORY_ITERATOR_METHODS: &[&NativeFunction] = &[
    &RDI_CONSTRUCT,
    &RDI_HAS_CHILDREN,
    &RDI_GET_CHILDREN,
    &RDI_GET_SUB_PATH,
    &RDI_GET_SUB_PATHNAME,
];

pub(super) static GLOB_ITERATOR_METHODS: &[&NativeFunction] = &[&GLOB_CONSTRUCT, &GLOB_COUNT];
