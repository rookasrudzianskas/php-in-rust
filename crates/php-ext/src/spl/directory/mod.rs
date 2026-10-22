//! ext/spl/spl_directory.c — `SplFileInfo`, `DirectoryIterator`,
//! `FilesystemIterator`, `RecursiveDirectoryIterator`, `GlobIterator`,
//! `SplFileObject` and `SplTempFileObject`.
//!
//! * [`info`] — `SplFileInfo` (paths, the `php_stat()` getters,
//!   `getFileInfo()` / `getPathInfo()` / `openFile()`).
//! * [`iter`] — the directory iterators.
//! * [`file`] — `SplFileObject` / `SplTempFileObject` (line reading with
//!   php-src's line-number semantics, CSV, the `f*()` methods).
//!
//! Every object of these classes carries an [`FsObject`]
//! (`spl_filesystem_object`), installed by the classes' `create_object`
//! handler so that user subclasses get it too. Streams are the stream
//! layer's resources (`standard::streams`); the state only keeps their
//! [`ResRef`], and no borrow of the state is held across a stream operation
//! or a call into PHP code.
//!
//! # Interpreter contract
//!
//! * **Method calls** (`spl_filesystem_object_get_method_check`): before
//!   resolving `$obj->method()` on an object whose class is or extends
//!   `GlobIterator` or `SplFileObject`, the interpreter must call
//!   [`get_method_check`]; it throws `Error: The parent constructor was not
//!   called: the object is in an invalid state` for objects whose (user)
//!   constructor did not call the parent constructor. Calls made by
//!   internal code ([`Vm::call_method`]) do not go through it, like
//!   `zend_call_method` in php-src.
//! * **foreach**: php-src gives `DirectoryIterator` and `FilesystemIterator`
//!   (and subclasses that override none of the `Iterator` methods) an
//!   internal `get_iterator`. Iterating them through their `Iterator`
//!   methods ([`super::engine::ObjIterator`]) yields the same keys and
//!   values for constructed objects; the interpreter must call
//!   [`get_iterator_check`] first, which throws `Error: An iterator cannot
//!   be used with foreach by reference` for `foreach ($it as &$v)`. (The
//!   only other difference: php-src's internal iterator yields nothing for
//!   an object whose constructor was never called, where the methods throw
//!   "Object not initialized".)
//! * **var_dump / print_r**: like php-src, these classes declare
//!   `__debugInfo()` instead of a `get_debug_info` handler, so user
//!   overrides of `__debugInfo()` win; [`debug_info`] is the table.
//! * **String casts**: there is no `cast_object` handler (php-src has none
//!   either): `(string)` goes through `__toString()` (an alias of
//!   `getPathname()` / `DirectoryIterator::getFilename()`).
//! * **Destructors**: `dtor_obj` closes the directory or file stream
//!   (`spl_filesystem_object_destroy_object`). php-src runs the userland
//!   `__destruct` before closing; `Host::run_destructor` runs `dtor_obj`
//!   first, so for classes with a userland `__destruct` the stream is left
//!   open and released when the object is freed.
//! * **Warnings turned into exceptions** (`zend_replace_error_handling(EH_THROW,
//!   ...)`): constructors, `openFile()` and the `php_stat()` getters switch
//!   [`Vm::errors`]`.mode` to [`ErrorHandlingMode::Throw`] around the stream
//!   or stat operation (see [`with_error_handling`]), so e.g. "Failed to open
//!   stream" warnings become `RuntimeException`s with the same message.

pub mod file;
pub mod info;
pub mod iter;

#[cfg(test)]
mod tests;

use php_runtime::class::{ClassConstant, ClassEntry, Function, class_flags, fn_flags};
use php_runtime::errors::ErrorHandlingMode;
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, ResRef, Value, Vm,
};

use super::engine;
use crate::standard::streams::glob_wrapper::GlobOps;
use crate::standard::streams::{self, handle_of};

// ---------------------------------------------------------------------------
// Flags (spl_directory.h)
// ---------------------------------------------------------------------------

/// `SPL_FILE_OBJECT_DROP_NEW_LINE`.
pub const DROP_NEW_LINE: i64 = 0x1;
/// `SPL_FILE_OBJECT_READ_AHEAD`.
pub const READ_AHEAD: i64 = 0x2;
/// `SPL_FILE_OBJECT_SKIP_EMPTY`.
pub const SKIP_EMPTY: i64 = 0x4;
/// `SPL_FILE_OBJECT_READ_CSV`.
pub const READ_CSV: i64 = 0x8;
/// `SPL_FILE_OBJECT_MASK`.
pub const FILE_OBJECT_MASK: i64 = 0xF;

pub const CURRENT_AS_FILEINFO: i64 = 0x0;
pub const CURRENT_AS_SELF: i64 = 0x10;
pub const CURRENT_AS_PATHNAME: i64 = 0x20;
pub const CURRENT_MODE_MASK: i64 = 0xF0;
pub const KEY_AS_PATHNAME: i64 = 0x0;
pub const KEY_AS_FILENAME: i64 = 0x100;
pub const KEY_MODE_MASK: i64 = 0xF00;
pub const NEW_CURRENT_AND_KEY: i64 = KEY_AS_FILENAME | CURRENT_AS_FILEINFO;
pub const SKIP_DOTS: i64 = 0x1000;
pub const UNIX_PATHS: i64 = 0x2000;
pub const FOLLOW_SYMLINKS: i64 = 0x4000;
pub const OTHERS_MASK: i64 = 0x7000;

// ---------------------------------------------------------------------------
// Object state
// ---------------------------------------------------------------------------

/// `SPL_FS_OBJ_TYPE`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FsType {
    #[default]
    Info,
    Dir,
    File,
}

/// `spl_filesystem_object`. php-src keeps the directory and file parts in a
/// union; here they are separate fields (only the ones of `ty` are used).
#[derive(Default)]
pub struct FsObject {
    pub ty: FsType,
    /// `intern->path`.
    pub path: Option<PhpStr>,
    /// `intern->orig_path` (the opened stream's path, files only).
    pub orig_path: Option<PhpStr>,
    /// `intern->file_name`; for directories computed lazily from the entry.
    pub file_name: Option<PhpStr>,
    pub flags: i64,
    /// `setFileClass()` (`None`: `SplFileObject`).
    pub file_class: Option<ClassRef>,
    /// `setInfoClass()` (`None`: `SplFileInfo`).
    pub info_class: Option<ClassRef>,

    // Directories.
    pub dirp: Option<ResRef>,
    pub sub_path: Option<PhpStr>,
    pub index: i64,
    /// `u.dir.entry.d_name` (empty: no current entry).
    pub entry: Vec<u8>,

    // Files.
    pub stream: Option<ResRef>,
    /// The `$context` argument (`u.file.zcontext`).
    pub zcontext: Option<ResRef>,
    pub open_mode: Option<PhpStr>,
    pub current_line: Option<PhpStr>,
    /// `u.file.current_zval` (the parsed CSV row).
    pub current_zval: Option<Value>,
    pub max_line_len: usize,
    pub current_line_num: i64,
    pub delimiter: u8,
    pub enclosure: u8,
    /// An escape byte or `csv::CSV_NO_ESCAPE`.
    pub escape: i32,
    pub is_escape_default: bool,
}

/// Runs `f` on the state of `obj` (installed if missing). `f` must not call
/// into the VM.
pub fn with_fs<R>(obj: &ObjRef, f: impl FnOnce(&mut FsObject) -> R) -> R {
    if obj.native::<FsObject>().is_none() {
        obj.set_native(FsObject::default());
    }
    let mut st = obj
        .native_mut::<FsObject>()
        .expect("spl_filesystem_object state");
    f(&mut st)
}

/// `spl_filesystem_object_new`.
fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(FsObject::default());
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// `zend_throw_error(NULL, "Object not initialized")`.
fn not_initialized(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error("Error", "Object not initialized")
}

/// `zend_replace_error_handling(EH_THROW, class)` around `f`: warnings raised
/// by `f` become exceptions of `class` carrying the warning's message.
pub fn with_error_handling<R>(
    vm: &mut Vm,
    class: &str,
    f: impl FnOnce(&mut Vm) -> PResult<R>,
) -> PResult<R> {
    let saved = std::mem::replace(
        &mut vm.errors.mode,
        ErrorHandlingMode::Throw(PhpStr::from(class)),
    );
    let r = f(vm);
    vm.errors.mode = saved;
    r
}

/// `zend_deprecated_function()` for a method marked `#[\Deprecated]`.
fn deprecated_method(vm: &mut Vm, class: &str, name: &str, since: &str) -> PResult<()> {
    vm.engine_deprecated(format!(
        "Method {class}::{name}() is deprecated since {since}"
    ))
}

fn is_slash(c: u8) -> bool {
    c == b'/'
}

/// `spl_filesystem_is_dot`.
fn is_dot(name: &[u8]) -> bool {
    name == b"." || name == b".."
}

/// `php_basename` (ASCII-compatible locale branch).
fn basename<'a>(s: &'a [u8], suffix: &[u8]) -> &'a [u8] {
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

/// The extension part of `getExtension()`: what follows the last `.` of
/// the basename of `name`.
fn extension(name: &[u8]) -> PhpStr {
    let base = basename(name, b"");
    match base.iter().rposition(|&c| c == b'.') {
        Some(i) => PhpStr::from_bytes(&base[i + 1..]),
        None => PhpStr::new(),
    }
}

/// The glob state of a directory stream: `None` when `res` is not a
/// `glob://` stream, else the directory of the current entry
/// (`php_glob_stream_get_path`, `None` when empty).
fn glob_path(res: &ResRef) -> Option<Option<Vec<u8>>> {
    let h = handle_of(res)?;
    let s = h.try_borrow().ok()?;
    let g = s.ops_ref::<GlobOps>()?;
    Some(g.path.clone().filter(|p| !p.is_empty()))
}

/// `spl_intern_is_glob`.
fn is_glob(st: &FsObject) -> bool {
    st.dirp.as_ref().is_some_and(|r| glob_path(r).is_some())
}

/// `spl_filesystem_object_get_path`.
fn get_path(st: &FsObject) -> Option<PhpStr> {
    if st.ty == FsType::Dir
        && let Some(g) = st.dirp.as_ref().and_then(glob_path)
    {
        return g.map(PhpStr::from_vec);
    }
    st.path.clone()
}

/// `spl_filesystem_object_get_file_name`: `false` for an object without a
/// file name ("Object not initialized").
fn ensure_file_name(st: &mut FsObject) -> bool {
    if st.file_name.is_some() {
        return true;
    }
    match st.ty {
        FsType::Info | FsType::File => false,
        FsType::Dir => {
            // DEFAULT_SLASH is '/' whatever SPL_FILE_DIR_UNIXPATHS says.
            let name = match get_path(st) {
                None => PhpStr::from_bytes(&st.entry),
                Some(path) => {
                    let mut v = path.to_vec();
                    v.push(b'/');
                    v.extend_from_slice(&st.entry);
                    PhpStr::from_vec(v)
                }
            };
            st.file_name = Some(name);
            true
        }
    }
}

/// [`ensure_file_name`] throwing "Object not initialized".
fn get_file_name(vm: &mut Vm, obj: &ObjRef) -> PResult<PhpStr> {
    let name = with_fs(obj, |st| {
        if ensure_file_name(st) {
            st.file_name.clone()
        } else {
            None
        }
    });
    name.ok_or_else(|| not_initialized(vm))
}

/// `spl_filesystem_object_get_pathname`.
fn get_pathname(st: &mut FsObject) -> Option<PhpStr> {
    match st.ty {
        FsType::Info | FsType::File => st.file_name.clone(),
        FsType::Dir => {
            if st.entry.is_empty() {
                None
            } else {
                ensure_file_name(st);
                st.file_name.clone()
            }
        }
    }
}

/// The file name relative to the path (what `getFilename()` returns).
fn short_file_name(st: &FsObject, file_name: &PhpStr) -> PhpStr {
    match get_path(st) {
        // +1 to skip the trailing / of the path in the file name
        Some(p) if !p.is_empty() && p.len() < file_name.len() => {
            PhpStr::from_bytes(&file_name[p.len() + 1..])
        }
        _ => file_name.clone(),
    }
}

/// `spl_filesystem_info_set_filename`.
fn info_set_filename(st: &mut FsObject, path: &[u8]) {
    let mut len = path.len();
    if len > 1 && is_slash(path[len - 1]) {
        loop {
            len -= 1;
            if !(len > 1 && is_slash(path[len - 1])) {
                break;
            }
        }
        st.file_name = Some(PhpStr::from_bytes(&path[..len]));
    } else {
        st.file_name = Some(PhpStr::from_bytes(path));
    }
    while len > 1 && !is_slash(path[len - 1]) {
        len -= 1;
    }
    len = len.saturating_sub(1);
    st.path = Some(PhpStr::from_bytes(&path[..len]));
}

/// Whether the constructor of `ce` is the one declared by the internal
/// class `base` (the `ce->constructor->common.scope != spl_ce_*` tests of
/// php-src, which call an overriding constructor instead).
fn ctor_declared_by(ce: &ClassRef, base: &str) -> bool {
    ce.find_method(b"__construct")
        .and_then(|m| m.scope.as_ref())
        .is_some_and(|s| s.eq_ignore_ascii_case(base.as_bytes()))
}

fn spl_class(vm: &Vm, name: &str) -> ClassRef {
    vm.find_class(name.as_bytes())
        .unwrap_or_else(|| panic!("{name} is registered"))
}

/// `intern->info_class`.
fn info_class(vm: &Vm, obj: &ObjRef) -> ClassRef {
    with_fs(obj, |st| st.info_class.clone()).unwrap_or_else(|| spl_class(vm, "SplFileInfo"))
}

/// `intern->file_class`.
fn file_class(vm: &Vm, obj: &ObjRef) -> ClassRef {
    with_fs(obj, |st| st.file_class.clone()).unwrap_or_else(|| spl_class(vm, "SplFileObject"))
}

/// Instantiates `ce` for `getFileInfo()` & co (`spl_filesystem_object_new`
/// through the class's `create_object`, so properties are initialised).
fn new_instance(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    vm.instantiate(ce)
}

/// `spl_filesystem_object_create_info`: a `ce` object for `file_path`.
fn create_info(vm: &mut Vm, file_path: PhpStr, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = new_instance(vm, ce)?;
    if ctor_declared_by(ce, "SplFileInfo") {
        with_fs(&obj, |st| info_set_filename(st, &file_path));
    } else {
        vm.call_method(&obj, b"__construct", vec![Value::String(file_path)])?;
    }
    Ok(obj)
}

/// "Could not open file" for a directory iterator without a current entry
/// (the first check of `spl_filesystem_object_create_type`).
fn check_source_entry(vm: &mut Vm, source: &ObjRef) -> PResult<()> {
    if with_fs(source, |st| st.ty == FsType::Dir && st.entry.is_empty()) {
        return Err(vm.throw_error("RuntimeException", "Could not open file"));
    }
    Ok(())
}

/// `spl_filesystem_object_create_type(..., SPL_FS_INFO, ce)`.
fn create_type_info(vm: &mut Vm, source: &ObjRef, ce: Option<ClassRef>) -> PResult<ObjRef> {
    check_source_entry(vm, source)?;
    let ce = match ce {
        Some(ce) => ce,
        None => info_class(vm, source),
    };
    let obj = new_instance(vm, &ce)?;
    let file_name = get_file_name(vm, source)?;
    if ctor_declared_by(&ce, "SplFileInfo") {
        let path = with_fs(source, |st| get_path(st));
        with_fs(&obj, |st| {
            st.file_name = Some(file_name);
            st.path = path;
        });
    } else {
        vm.call_method(&obj, b"__construct", vec![Value::String(file_name)])?;
    }
    Ok(obj)
}

/// `Z_PARAM_RESOURCE_OR_NULL` for an untyped `$context` parameter at index
/// `i`.
fn resource_or_null(vm: &mut Vm, args: &Args, i: usize) -> PResult<Option<ResRef>> {
    match args.get(i).unref() {
        Value::Null | Value::Undef => Ok(None),
        Value::Resource(r) => Ok(Some(r)),
        other => Err(vm.argument_type_error(
            i + 1,
            &format!(
                "must be of type resource or null, {} given",
                other.value_name()
            ),
        )),
    }
}

/// `Z_PARAM_PATH` validation of a string argument at index `i`.
fn path_arg(vm: &mut Vm, args: &Args, i: usize) -> PResult<PhpStr> {
    let s = args.str(i).clone();
    if s.contains(&0) {
        return Err(vm.argument_value_error(i + 1, "must not contain any null bytes"));
    }
    Ok(s)
}

/// `spl_filesystem_object_create_type(..., SPL_FS_FILE)` (`openFile()`):
/// `args` are `openFile()`'s (`$mode`, `$useIncludePath`, `$context`).
fn create_type_file(vm: &mut Vm, source: &ObjRef, args: &Args) -> PResult<ObjRef> {
    check_source_entry(vm, source)?;
    let ce = file_class(vm, source);
    let open_mode = args.str(0).clone();
    let use_include_path = args.bool(1);
    let context = resource_or_null(vm, args, 2)?;
    let obj = new_instance(vm, &ce)?;
    let file_name = get_file_name(vm, source)?;
    if ctor_declared_by(&ce, "SplFileObject") {
        let path = with_fs(source, |st| get_path(st));
        with_fs(&obj, |st| {
            st.file_name = Some(file_name);
            st.path = path;
            st.open_mode = Some(open_mode);
            st.zcontext = context;
        });
        // spl_filesystem_file_open() can generate E_WARNINGs which we want
        // to promote to exceptions
        with_error_handling(vm, "RuntimeException", |vm| {
            file::file_open(vm, &obj, use_include_path)
        })?;
    } else {
        vm.call_method(
            &obj,
            b"__construct",
            vec![Value::String(file_name), Value::String(open_mode)],
        )?;
    }
    Ok(obj)
}

// ---------------------------------------------------------------------------
// Directory streams
// ---------------------------------------------------------------------------

/// `spl_filesystem_dir_read`: the next entry (empty at the end).
fn dir_read(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let dirp = with_fs(obj, |st| {
        // invalidate
        st.file_name = None;
        st.dirp.clone()
    });
    let entry = match dirp {
        Some(res) => streams::with_stream(vm, &res, |vm, s| s.readdir(vm))?.flatten(),
        None => None,
    };
    with_fs(obj, |st| st.entry = entry.unwrap_or_default());
    Ok(())
}

/// Reads entries until one that is not `.`/`..` when SKIP_DOTS is set.
fn dir_read_skipping(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let skip_dots = with_fs(obj, |st| st.flags & SKIP_DOTS != 0);
    loop {
        dir_read(vm, obj)?;
        if !(skip_dots && with_fs(obj, |st| is_dot(&st.entry))) {
            return Ok(());
        }
    }
}

/// `php_stream_rewinddir` on the object's directory stream.
fn dir_rewind(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    if let Some(res) = with_fs(obj, |st| st.dirp.clone()) {
        streams::with_stream(vm, &res, |vm, s| s.rewind(vm))?;
    }
    Ok(())
}

/// `spl_filesystem_dir_open`: opens `path` (reporting errors, which the
/// constructors turn into `UnexpectedValueException`s) and reads the first
/// entry.
fn dir_open(vm: &mut Vm, obj: &ObjRef, path: &[u8]) -> PResult<()> {
    with_fs(obj, |st| st.ty = FsType::Dir);
    let ctx = streams::wrappers::globals(vm).default_context.clone();
    let opened =
        streams::wrappers::opendir(vm, path, streams::stream::open::REPORT_ERRORS, ctx.as_ref());
    let (dirp, error) = match opened {
        Ok(d) => (d, None),
        Err(e) => (None, Some(e)),
    };
    if let Some(h) = dirp.as_ref().and_then(handle_of) {
        // we prevent potential UAF with conflicting explicit fclose(),
        // relying on the object destructor for this
        if let Ok(mut s) = h.try_borrow_mut() {
            s.st.flags |= streams::stream::flags::NO_FCLOSE;
        }
    }
    let failed = dirp.is_none();
    with_fs(obj, |st| {
        let len = path.len();
        st.path = Some(if len > 1 && is_slash(path[len - 1]) {
            PhpStr::from_bytes(&path[..len - 1])
        } else {
            PhpStr::from_bytes(path)
        });
        st.dirp = dirp;
        st.index = 0;
        if failed {
            st.entry.clear();
        }
    });
    if let Some(e) = error {
        return Err(e);
    }
    if failed {
        // open failed w/out notice (turned to exception due to EH_THROW)
        let mut msg = b"Failed to open directory \"".to_vec();
        msg.extend_from_slice(path);
        msg.push(b'"');
        return Err(vm.throw_error("UnexpectedValueException", PhpStr::from_vec(msg)));
    }
    dir_read_skipping(vm, obj)
}

// ---------------------------------------------------------------------------
// Object handlers
// ---------------------------------------------------------------------------

/// `spl_filesystem_object_clone`.
fn clone_obj(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = ObjRef::new(ce);
    new.set_native(FsObject::default());
    let (ty, flags, path, file_name, index, file_class, info_class) = with_fs(old, |st| {
        (
            st.ty,
            st.flags,
            st.path.clone(),
            st.file_name.clone(),
            st.index,
            st.file_class.clone(),
            st.info_class.clone(),
        )
    });
    with_fs(&new, |st| st.flags = flags);
    match ty {
        FsType::Dir => {
            dir_open(vm, &new, &path.unwrap_or_default())?;
            // read until we hit the position in which we were before
            let mut i = 0;
            while i < index {
                dir_read_skipping(vm, &new)?;
                i += 1;
            }
            with_fs(&new, |st| st.index = i);
        }
        // SplFileObject is not cloneable (clone_forbidden); copy what an
        // SplFileInfo would.
        FsType::Info | FsType::File => with_fs(&new, |st| {
            st.path = path;
            st.file_name = file_name;
        }),
    }
    with_fs(&new, |st| {
        st.file_class = file_class;
        st.info_class = info_class;
    });
    for (name, value) in old.properties() {
        new.set_prop(&name, value);
    }
    Ok(new)
}

/// `clone_obj = NULL` (`spl_filesystem_object_check_handlers`).
fn clone_forbidden(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    Err(vm.throw_error(
        "Error",
        format!(
            "Trying to clone an uncloneable object of class {}",
            old.class().name
        ),
    ))
}

/// `spl_filesystem_object_destroy_object`: closes the directory or file
/// stream (see the module docs for classes with a userland `__destruct`).
fn dtor_obj(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    if obj.class().magic.destructor.is_some() {
        return Ok(());
    }
    close_streams(vm, obj)
}

/// Closes the object's stream(s) like `spl_filesystem_object_destroy_object`.
pub fn close_streams(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let (dirp, stream) = with_fs(obj, |st| match st.ty {
        FsType::Dir => (st.dirp.take(), None),
        FsType::File => (None, st.stream.take()),
        FsType::Info => (None, None),
    });
    if let Some(res) = dirp {
        streams::close(vm, &res)?;
    }
    if let Some(res) = stream {
        streams::close(vm, &res)?;
    }
    Ok(())
}

/// `spl_filesystem_object_get_debug_info`: the table `__debugInfo()`
/// returns.
pub fn debug_info(obj: &ObjRef) -> PhpArray {
    let mut table = engine::std_properties(obj);
    let set = engine::set_private_debug_info_property;
    with_fs(obj, |st| {
        let path_name = get_pathname(st).unwrap_or_default();
        set(
            &mut table,
            "SplFileInfo",
            "pathName",
            Value::String(path_name),
        );
        if let Some(file_name) = st.file_name.clone() {
            let short = short_file_name(st, &file_name);
            set(&mut table, "SplFileInfo", "fileName", Value::String(short));
        }
        if st.ty == FsType::Dir {
            let glob = if is_glob(st) {
                Value::String(st.path.clone().unwrap_or_default())
            } else {
                Value::Bool(false)
            };
            set(&mut table, "DirectoryIterator", "glob", glob);
            let sub = st.sub_path.clone().unwrap_or_default();
            set(
                &mut table,
                "RecursiveDirectoryIterator",
                "subPathName",
                Value::String(sub),
            );
        }
        if st.ty == FsType::File {
            let mode = st.open_mode.clone().unwrap_or_default();
            set(&mut table, "SplFileObject", "openMode", Value::String(mode));
            let d = Value::bytes(&[st.delimiter]);
            set(&mut table, "SplFileObject", "delimiter", d);
            let e = Value::bytes(&[st.enclosure]);
            set(&mut table, "SplFileObject", "enclosure", e);
        }
    });
    table
}

/// `spl_filesystem_object_get_method_check`: the hook the interpreter calls
/// before resolving `$obj->method()` (see the module docs).
pub fn get_method_check(vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
    let ce = obj.class();
    if !ce.instance_of_name(b"GlobIterator") && !ce.instance_of_name(b"SplFileObject") {
        return Ok(());
    }
    let invalid = obj
        .native::<FsObject>()
        .is_none_or(|st| st.dirp.is_none() && st.stream.is_none() && st.orig_path.is_none());
    if invalid {
        return Err(vm.throw_error(
            "Error",
            "The parent constructor was not called: the object is in an invalid state",
        ));
    }
    Ok(())
}

/// Whether `foreach` over an object of `ce` uses php-src's internal
/// directory iterator (`spl_filesystem_dir_get_iterator` /
/// `spl_filesystem_tree_get_iterator`): the class extends
/// `DirectoryIterator` and none of the `Iterator` methods is overridden by
/// a userland class (`zend_implement_iterator`).
pub fn uses_internal_iterator(vm: &Vm, ce: &ClassRef) -> bool {
    ce.instance_of_name(b"DirectoryIterator")
        && ["rewind", "valid", "key", "current", "next"]
            .iter()
            .all(|m| {
                ce.find_method(m.as_bytes())
                    .and_then(|f| f.scope.as_ref())
                    .and_then(|s| vm.find_class(s.as_bytes()))
                    .is_some_and(|c| c.is_internal())
            })
}

/// The `get_iterator` part the interpreter must honour for `foreach` over
/// these objects (see the module docs): by-reference iteration is an error
/// for the internal iterators.
pub fn get_iterator_check(vm: &mut Vm, obj: &ObjRef, by_ref: bool) -> PResult<()> {
    if by_ref && uses_internal_iterator(vm, obj.class()) {
        return Err(vm.throw_error(
            "Error",
            "An iterator cannot be used with foreach by reference",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Adds `methods` and the `#[\Deprecated]` ones (`deprecated`).
fn add_methods(
    ce: &mut ClassEntry,
    methods: &[&'static NativeFunction],
    deprecated: &[&'static NativeFunction],
) {
    engine::add_methods(ce, methods);
    let scope = ce.name.to_string_lossy().into_owned();
    for nf in deprecated {
        ce.add_method(Function::native_method(
            nf,
            &scope,
            engine::method_flags(nf.sig) | fn_flags::DEPRECATED,
        ));
    }
}

/// A `public const int` class constant.
fn int_constant(ce: &mut ClassEntry, name: &str, value: i64) {
    let mut c = ClassConstant::new(name, Value::Int(value), ce.name.clone());
    c.ty = Some(TypeDecl::parse("int").expect("valid type"));
    ce.add_constant(c);
}

/// `PHP_MINIT_FUNCTION(spl_directory)`.
pub fn register(r: &mut Registry) {
    let ns = class_flags::NOT_SERIALIZABLE;

    let mut ce = ClassEntry::new("SplFileInfo", ns);
    engine::implement(&mut ce, &engine::class(r, "Stringable"));
    add_methods(&mut ce, info::METHODS, info::DEPRECATED_METHODS);
    ce.handlers.create_object = Some(create_object);
    ce.handlers.clone_obj = Some(clone_obj);
    ce.handlers.dtor_obj = Some(dtor_obj);
    let spl_file_info = r.class(ce);

    let mut ce = ClassEntry::new_subclass("DirectoryIterator", ns, &spl_file_info);
    engine::implement(&mut ce, &engine::class(r, "SeekableIterator"));
    add_methods(&mut ce, iter::DIRECTORY_ITERATOR_METHODS, &[]);
    let directory_iterator = r.class(ce);

    let mut ce = ClassEntry::new_subclass("FilesystemIterator", ns, &directory_iterator);
    for (name, value) in [
        ("CURRENT_MODE_MASK", CURRENT_MODE_MASK),
        ("CURRENT_AS_PATHNAME", CURRENT_AS_PATHNAME),
        ("CURRENT_AS_FILEINFO", CURRENT_AS_FILEINFO),
        ("CURRENT_AS_SELF", CURRENT_AS_SELF),
        ("KEY_MODE_MASK", KEY_MODE_MASK),
        ("KEY_AS_PATHNAME", KEY_AS_PATHNAME),
        ("FOLLOW_SYMLINKS", FOLLOW_SYMLINKS),
        ("KEY_AS_FILENAME", KEY_AS_FILENAME),
        ("NEW_CURRENT_AND_KEY", NEW_CURRENT_AND_KEY),
        ("OTHER_MODE_MASK", OTHERS_MASK),
        ("SKIP_DOTS", SKIP_DOTS),
        ("UNIX_PATHS", UNIX_PATHS),
    ] {
        int_constant(&mut ce, name, value);
    }
    add_methods(&mut ce, iter::FILESYSTEM_ITERATOR_METHODS, &[]);
    let filesystem_iterator = r.class(ce);

    let mut ce = ClassEntry::new_subclass("RecursiveDirectoryIterator", ns, &filesystem_iterator);
    engine::implement(&mut ce, &engine::class(r, "RecursiveIterator"));
    add_methods(&mut ce, iter::RECURSIVE_DIRECTORY_ITERATOR_METHODS, &[]);
    r.class(ce);

    let mut ce = ClassEntry::new_subclass("GlobIterator", ns, &filesystem_iterator);
    engine::implement(&mut ce, &engine::class(r, "Countable"));
    add_methods(&mut ce, iter::GLOB_ITERATOR_METHODS, &[]);
    ce.handlers.clone_obj = Some(clone_forbidden);
    r.class(ce);

    let mut ce = ClassEntry::new_subclass("SplFileObject", ns, &spl_file_info);
    engine::implement(&mut ce, &engine::class(r, "RecursiveIterator"));
    engine::implement(&mut ce, &engine::class(r, "SeekableIterator"));
    for (name, value) in [
        ("DROP_NEW_LINE", DROP_NEW_LINE),
        ("READ_AHEAD", READ_AHEAD),
        ("SKIP_EMPTY", SKIP_EMPTY),
        ("READ_CSV", READ_CSV),
    ] {
        int_constant(&mut ce, name, value);
    }
    add_methods(&mut ce, file::METHODS, file::DEPRECATED_METHODS);
    ce.handlers.clone_obj = Some(clone_forbidden);
    let spl_file_object = r.class(ce);

    let mut ce = ClassEntry::new_subclass("SplTempFileObject", ns, &spl_file_object);
    add_methods(&mut ce, file::TEMP_METHODS, &[]);
    r.class(ce);
}
