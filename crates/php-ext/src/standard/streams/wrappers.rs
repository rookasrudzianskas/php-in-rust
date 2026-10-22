//! Wrapper registry and the generic open path of main/streams/streams.c
//! (`php_stream_locate_url_wrapper`, `php_stream_open_wrapper_ex`,
//! `php_stream_opendir`, `php_stream_stat_path_ex`, mkdir/rmdir) plus the
//! wrapper error log of main/streams/stream_errors.c.

use std::any::Any;
use std::rc::Rc;

use php_runtime::errors::level::E_WARNING;
use php_runtime::{PResult, PhpStr, ResRef, Value, Vm};

use super::stream::{StatBuf, Stream, StreamHandle, handle_of, open};
use super::{sys, user_wrapper};

/// Value passed to [`Wrapper::metadata`] (`touch`, `chown`, `chmod`).
#[derive(Clone, Debug)]
pub enum MetaValue {
    /// `PHP_STREAM_META_TOUCH`: (mtime, atime); `None` = now.
    Touch(Option<(i64, i64)>),
    OwnerName(PhpStr),
    Owner(i64),
    GroupName(PhpStr),
    Group(i64),
    Access(i64),
}

pub const META_TOUCH: i64 = 1;
pub const META_OWNER_NAME: i64 = 2;
pub const META_OWNER: i64 = 3;
pub const META_GROUP_NAME: i64 = 4;
pub const META_GROUP: i64 = 5;
pub const META_ACCESS: i64 = 6;

pub const URL_STAT_LINK: i32 = 1;
pub const URL_STAT_QUIET: i32 = 2;
pub const MKDIR_RECURSIVE: i32 = 1;

/// `php_stream_wrapper` + `php_stream_wrapper_ops`.
pub trait Wrapper {
    /// `wops->label` ("plainfile", "PHP", "RFC2397", "user-space", "glob").
    fn label(&self) -> &'static str;

    fn is_url(&self) -> bool {
        false
    }

    fn has_opener(&self) -> bool {
        true
    }

    /// `stream_opener`: returns the new stream's resource.
    fn open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        mode: &[u8],
        options: i32,
        opened_path: &mut Option<PhpStr>,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>>;

    fn has_stream_stat(&self) -> bool {
        false
    }

    fn stream_stat(&self, vm: &mut Vm, stream: &mut Stream) -> PResult<Option<StatBuf>> {
        let _ = (vm, stream);
        Ok(None)
    }

    fn has_url_stat(&self) -> bool {
        false
    }

    /// `url_stat`; `Ok(None)` = the stat failed.
    fn url_stat(
        &self,
        vm: &mut Vm,
        url: &[u8],
        flags: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<StatBuf>> {
        let _ = (vm, url, flags, ctx);
        Ok(None)
    }

    fn has_dir_opener(&self) -> bool {
        false
    }

    fn dir_open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        let _ = (vm, path, options, ctx);
        Ok(None)
    }

    fn has_unlink(&self) -> bool {
        false
    }

    fn unlink(&self, vm: &mut Vm, url: &[u8], options: i32, ctx: Option<&ResRef>) -> PResult<bool> {
        let _ = (vm, url, options, ctx);
        Ok(false)
    }

    fn has_rename(&self) -> bool {
        false
    }

    fn rename(
        &self,
        vm: &mut Vm,
        from: &[u8],
        to: &[u8],
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let _ = (vm, from, to, options, ctx);
        Ok(false)
    }

    fn has_mkdir(&self) -> bool {
        false
    }

    fn mkdir(
        &self,
        vm: &mut Vm,
        url: &[u8],
        mode: i64,
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let _ = (vm, url, mode, options, ctx);
        Ok(false)
    }

    fn has_rmdir(&self) -> bool {
        false
    }

    fn rmdir(&self, vm: &mut Vm, url: &[u8], options: i32, ctx: Option<&ResRef>) -> PResult<bool> {
        let _ = (vm, url, options, ctx);
        Ok(false)
    }

    fn has_metadata(&self) -> bool {
        false
    }

    fn metadata(
        &self,
        vm: &mut Vm,
        url: &[u8],
        value: &MetaValue,
        ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let _ = (vm, url, value, ctx);
        Ok(false)
    }

    fn as_any(&self) -> &dyn Any;
}

pub type WrapperRef = Rc<dyn Wrapper>;

/// Per-request stream globals (`FG(...)` of file.c / streams.c).
#[derive(Default)]
pub struct StreamGlobals {
    init: bool,
    /// `url_stream_wrappers_hash` (module startup registrations).
    pub global: Vec<(PhpStr, WrapperRef)>,
    /// `FG(stream_wrappers)`: the request-local copy once modified.
    pub volatile: Option<Vec<(PhpStr, WrapperRef)>>,
    /// `FG(wrapper_logged_errors)`.
    pub logged_errors: Vec<(String, Vec<String>)>,
    /// errno of the last failed plain-file operation.
    pub last_errno: i32,
    /// `FG(default_context)`.
    pub default_context: Option<ResRef>,
    /// `BG(umask)` (umask in effect before the first `umask()` call).
    pub saved_umask: Option<u32>,
}

pub fn globals(vm: &mut Vm) -> &mut StreamGlobals {
    let g = vm.ext_data::<StreamGlobals>();
    if !g.init {
        g.init = true;
        let plain: WrapperRef = Rc::new(super::plain::PlainWrapper);
        g.global = vec![
            (
                PhpStr::from("php"),
                Rc::new(super::php_wrapper::PhpWrapper) as WrapperRef,
            ),
            (PhpStr::from("file"), plain),
            (
                PhpStr::from("glob"),
                Rc::new(super::glob_wrapper::GlobWrapper) as WrapperRef,
            ),
            (
                PhpStr::from("data"),
                Rc::new(super::memory::DataWrapper) as WrapperRef,
            ),
        ];
    }
    g
}

/// The plain-files wrapper (`php_plain_files_wrapper`).
pub fn plain_wrapper(vm: &mut Vm) -> WrapperRef {
    globals(vm)
        .global
        .iter()
        .find(|(n, _)| n.as_bytes() == b"file")
        .map(|(_, w)| w.clone())
        .expect("file wrapper registered")
}

/// The active wrapper table.
pub fn wrapper_table(vm: &mut Vm) -> Vec<(PhpStr, WrapperRef)> {
    let g = globals(vm);
    g.volatile.clone().unwrap_or_else(|| g.global.clone())
}

fn find_wrapper(table: &[(PhpStr, WrapperRef)], name: &[u8]) -> Option<WrapperRef> {
    table
        .iter()
        .find(|(n, _)| n.as_bytes() == name)
        .map(|(_, w)| w.clone())
}

/// `php_stream_wrapper_scheme_validate`.
pub fn scheme_valid(p: &[u8]) -> bool {
    p.iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
}

/// `php_register_url_stream_wrapper_volatile`.
pub fn register_volatile(vm: &mut Vm, protocol: &[u8], w: WrapperRef) -> bool {
    if !scheme_valid(protocol) {
        return false;
    }
    let g = globals(vm);
    if g.volatile.is_none() {
        g.volatile = Some(g.global.clone());
    }
    let t = g.volatile.as_mut().expect("just set");
    if t.iter().any(|(n, _)| n.as_bytes() == protocol) {
        return false;
    }
    t.push((PhpStr::from_bytes(protocol), w));
    true
}

/// `php_unregister_url_stream_wrapper_volatile`.
pub fn unregister_volatile(vm: &mut Vm, protocol: &[u8]) -> bool {
    let g = globals(vm);
    if g.volatile.is_none() {
        g.volatile = Some(g.global.clone());
    }
    let t = g.volatile.as_mut().expect("just set");
    match t.iter().position(|(n, _)| n.as_bytes() == protocol) {
        Some(i) => {
            t.remove(i);
            true
        }
        None => false,
    }
}

/// Result of [`locate_url_wrapper`].
pub struct Located {
    pub wrapper: WrapperRef,
    /// Offset into the path where the wrapper-specific part starts
    /// (`path_for_open`).
    pub path_offset: usize,
}

/// `php_stream_locate_url_wrapper`. Emits php-src's warnings for unknown
/// wrappers, remote `file://` hosts and `allow_url_fopen`.
pub fn locate_url_wrapper(vm: &mut Vm, path: &[u8], options: i32) -> PResult<Option<Located>> {
    if options & open::IGNORE_URL != 0 {
        if options & open::STREAM_LOCATE_WRAPPERS_ONLY != 0 {
            return Ok(None);
        }
        return Ok(Some(Located {
            wrapper: plain_wrapper(vm),
            path_offset: 0,
        }));
    }
    let table = wrapper_table(vm);
    let n = path
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
        .count();
    let mut protocol = false;
    if path.get(n) == Some(&b':')
        && n > 1
        && (path[n + 1..].starts_with(b"//") || (n == 4 && path.starts_with(b"data:")))
    {
        protocol = true;
    }
    let mut wrapper = None;
    if protocol {
        wrapper = find_wrapper(&table, &path[..n]);
        if wrapper.is_none() {
            let lower = path[..n].to_ascii_lowercase();
            wrapper = find_wrapper(&table, &lower);
            if wrapper.is_none() {
                let name = String::from_utf8_lossy(&path[..n.min(31)]).into_owned();
                vm.warning(format!(
                    "Unable to find the wrapper \"{name}\" - did you forget to enable it when you configured PHP?"
                ))?;
                protocol = false;
            }
        }
    }
    if !protocol || path[..n].eq_ignore_ascii_case(b"file") {
        let mut offset = 0;
        if protocol {
            let localhost =
                path.len() >= 17 && path[..17].eq_ignore_ascii_case(b"file://localhost/");
            if !localhost && path.get(n + 3).is_some_and(|&c| c != b'/') {
                if options & open::REPORT_ERRORS != 0 {
                    vm.warning(format!(
                        "Remote host file access not supported, {}",
                        String::from_utf8_lossy(path)
                    ))?;
                }
                return Ok(None);
            }
            // skip past protocol and :/
            let mut p = n + 1;
            if localhost {
                p += 11;
            }
            loop {
                p += 1;
                if path.get(p) != Some(&b'/') {
                    break;
                }
            }
            offset = p - 1;
        }
        if options & open::STREAM_LOCATE_WRAPPERS_ONLY != 0 {
            return Ok(None);
        }
        let volatile = globals(vm).volatile.is_some();
        if volatile {
            if let Some(w) = wrapper.filter(|_| protocol) {
                return Ok(Some(Located {
                    wrapper: w,
                    path_offset: offset,
                }));
            }
            if let Some(w) = find_wrapper(&table, b"file") {
                return Ok(Some(Located {
                    wrapper: w,
                    path_offset: offset,
                }));
            }
            if options & open::REPORT_ERRORS != 0 {
                vm.warning("file:// wrapper is disabled in the server configuration")?;
            }
            return Ok(None);
        }
        return Ok(Some(Located {
            wrapper: plain_wrapper(vm),
            path_offset: offset,
        }));
    }
    let wrapper = wrapper.expect("protocol implies wrapper");
    if wrapper.is_url()
        && options & open::STREAM_DISABLE_URL_PROTECTION == 0
        && (!vm.ini_bool("allow_url_fopen")
            || (options & open::STREAM_OPEN_FOR_INCLUDE != 0 && !vm.ini_bool("allow_url_include")))
    {
        if options & open::REPORT_ERRORS != 0 {
            let proto = String::from_utf8_lossy(&path[..n]).into_owned();
            if !vm.ini_bool("allow_url_fopen") {
                vm.warning(format!(
                    "{proto}:// wrapper is disabled in the server configuration by allow_url_fopen=0"
                ))?;
            } else {
                vm.warning(format!(
                    "{proto}:// wrapper is disabled in the server configuration by allow_url_include=0"
                ))?;
            }
        }
        return Ok(None);
    }
    Ok(Some(Located {
        wrapper,
        path_offset: 0,
    }))
}

/// `php_stream_wrapper_log_error` without REPORT_ERRORS: stores the message
/// for a later "Failed to open stream: ..." (or emits it right away when
/// `options` has REPORT_ERRORS).
pub fn log_error(vm: &mut Vm, wrapper_label: &str, options: i32, msg: String) -> PResult<()> {
    if options & open::REPORT_ERRORS != 0 {
        return vm.php_error_docref(E_WARNING, msg);
    }
    let g = globals(vm);
    match g.logged_errors.iter_mut().find(|(n, _)| n == wrapper_label) {
        Some((_, list)) => list.push(msg),
        None => g.logged_errors.push((wrapper_label.to_string(), vec![msg])),
    }
    Ok(())
}

/// `php_stream_wrapper_warn` (REPORT_ERRORS-gated warning).
pub fn wrapper_warn(vm: &mut Vm, options: i32, msg: impl Into<PhpStr>) -> PResult<()> {
    if options & open::REPORT_ERRORS != 0 {
        vm.warning(msg)?;
    }
    Ok(())
}

/// `php_stream_display_wrapper_name_errors`: "<caption>: <logged messages
/// | strerror(errno) | operation failed>".
pub fn display_wrapper_errors(vm: &mut Vm, wrapper_label: &str, caption: &str) -> PResult<()> {
    let html = vm.ini_bool("html_errors");
    let g = globals(vm);
    let msg = if wrapper_label == ":na" {
        "no suitable wrapper could be found".to_string()
    } else if let Some((_, list)) = g.logged_errors.iter().find(|(n, _)| n == wrapper_label) {
        list.join(if html { "<br />\n" } else { "\n" })
    } else if wrapper_label == "plainfile" {
        sys::strerror(g.last_errno)
    } else {
        "operation failed".to_string()
    };
    vm.warning(format!("{caption}: {msg}"))
}

/// `php_stream_tidy_wrapper_name_error_log`.
pub fn tidy_wrapper_errors(vm: &mut Vm, wrapper_label: &str) {
    globals(vm)
        .logged_errors
        .retain(|(n, _)| n != wrapper_label);
}

/// `php_resolve_path`: searches `include_path` (and the executing script's
/// directory) for a relative filename.
pub fn resolve_path(vm: &mut Vm, filename: &[u8]) -> Option<PhpStr> {
    if filename.is_empty() {
        return None;
    }
    let n = filename
        .iter()
        .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'+' || c == b'-' || c == b'.')
        .count();
    // Don't resolve paths which contain protocol (except of file://)
    if n < filename.len()
        && filename[n] == b':'
        && n > 1
        && filename[n + 1..].starts_with(b"//")
        && !filename[..n].eq_ignore_ascii_case(b"file")
    {
        let table = wrapper_table(vm);
        if find_wrapper(&table, &filename[..n]).is_some() {
            return None;
        }
    }
    let filename = if filename.len() > 7 && filename[..7].eq_ignore_ascii_case(b"file://") {
        &filename[7..]
    } else {
        filename
    };
    let is_relative_dot = filename.starts_with(b"./")
        || filename.starts_with(b"../")
        || filename == b"."
        || filename == b"..";
    if filename.starts_with(b"/") || is_relative_dot {
        return super::plain::real_path(filename);
    }
    let include_path = vm.ini_get("include_path").unwrap_or_default();
    for dir in include_path.as_bytes().split(|&c| c == b':') {
        if dir.is_empty() {
            continue;
        }
        let mut p = dir.to_vec();
        p.push(b'/');
        p.extend_from_slice(filename);
        if let Some(r) = super::plain::real_path(&p) {
            return Some(r);
        }
    }
    // check in the calling script's directory
    let cur = vm.current_file.clone();
    if vm.executing && cur.as_bytes().starts_with(b"/") {
        if let Some(slash) = cur.as_bytes().iter().rposition(|&c| c == b'/') {
            let mut p = cur.as_bytes()[..=slash].to_vec();
            p.extend_from_slice(filename);
            if let Some(r) = super::plain::real_path(&p) {
                return Some(r);
            }
        }
    }
    None
}

/// `php_stream_open_wrapper_ex`. On failure the "Failed to open stream"
/// warning has been emitted (when `options` has REPORT_ERRORS).
pub fn open_wrapper(
    vm: &mut Vm,
    path: &[u8],
    mode: &[u8],
    options: i32,
    opened_path: Option<&mut Option<PhpStr>>,
    context: Option<&ResRef>,
) -> PResult<Option<ResRef>> {
    super::stream::process_deferred_closes(vm)?;
    if path.is_empty() {
        return Err(vm.throw_value_error("Path must not be empty"));
    }
    let mut options = options;
    let mut path_buf: Vec<u8> = path.to_vec();
    let mut resolved = None;
    if options & open::USE_PATH != 0 {
        if let Some(r) = resolve_path(vm, path) {
            path_buf = r.to_vec();
            resolved = Some(r);
            options |= open::STREAM_ASSUME_REALPATH;
            options &= !open::USE_PATH;
        }
    }
    let path = path_buf.as_slice();
    let Some(loc) = locate_url_wrapper(vm, path, options)? else {
        if options & open::REPORT_ERRORS != 0 {
            vm.warning("Failed to open stream: no suitable wrapper could be found")?;
        }
        return Ok(None);
    };
    let wrapper = loc.wrapper;
    if options & open::STREAM_USE_URL != 0 && !wrapper.is_url() {
        wrapper_warn(vm, options, "This function may only be used against URLs")?;
        return Ok(None);
    }
    if !wrapper.has_opener() {
        wrapper_warn(
            vm,
            options,
            "Failed to open stream: wrapper does not support stream open",
        )?;
        return Ok(None);
    }
    let label = wrapper.label();
    let mut op = None;
    let stream = wrapper.open(
        vm,
        &path[loc.path_offset..],
        mode,
        options & !open::REPORT_ERRORS,
        &mut op,
        context,
    )?;
    let Some(res) = stream else {
        if options & open::REPORT_ERRORS != 0 {
            display_wrapper_errors(vm, label, "Failed to open stream")?;
        }
        tidy_wrapper_errors(vm, label);
        return Ok(None);
    };
    tidy_wrapper_errors(vm, label);
    let h = handle_of(&res).expect("opener returns a stream");
    {
        let mut s = h.borrow_mut();
        if s.wrapper.is_none() {
            s.wrapper = Some(wrapper.clone());
        }
        s.st.wrapper_label = Some(label);
        s.orig_path = Some(PhpStr::from_bytes(path));
        if s.st.context.is_none() {
            if let Some(c) = context {
                let is_default = globals(vm)
                    .default_context
                    .as_ref()
                    .is_some_and(|d| ResRef::ptr_eq(d, c));
                if !is_default {
                    s.st.context = Some(c.clone());
                }
            }
        }
    }
    if let Some(slot) = opened_path {
        *slot = op.or(resolved);
    }
    // if opened for append, we need to revise our idea of the initial file position
    let mut s = h.borrow_mut();
    if s.ops.can_seek()
        && s.st.flags & super::stream::flags::NO_SEEK == 0
        && mode.contains(&b'a')
        && s.st.position == 0
    {
        let Stream { ops, st, .. } = &mut *s;
        if let (0, Some(p)) = ops.seek(vm, st, 0, 1)? {
            st.position = p;
        }
    }
    Ok(Some(res))
}

/// `php_stream_opendir`.
pub fn opendir(
    vm: &mut Vm,
    path: &[u8],
    options: i32,
    context: Option<&ResRef>,
) -> PResult<Option<ResRef>> {
    super::stream::process_deferred_closes(vm)?;
    if path.is_empty() {
        return Ok(None);
    }
    let Some(loc) = locate_url_wrapper(vm, path, options)? else {
        if options & open::REPORT_ERRORS != 0 {
            vm.warning("Failed to open directory")?;
        }
        return Ok(None);
    };
    let wrapper = loc.wrapper;
    if !wrapper.has_dir_opener() {
        wrapper_warn(vm, options, "Failed to open directory: not implemented")?;
        return Ok(None);
    }
    let label = wrapper.label();
    let r = wrapper.dir_open(
        vm,
        &path[loc.path_offset..],
        options & !open::REPORT_ERRORS,
        context,
    )?;
    match &r {
        Some(res) => {
            if let Some(h) = handle_of(res) {
                let mut s = h.borrow_mut();
                if s.wrapper.is_none() {
                    s.wrapper = Some(wrapper.clone());
                }
                s.st.wrapper_label = Some(label);
                s.st.flags |= super::stream::flags::NO_BUFFER | super::stream::flags::IS_DIR;
            }
        }
        None => {
            if options & open::REPORT_ERRORS != 0 {
                display_wrapper_errors(vm, label, "Failed to open directory")?;
            }
        }
    }
    tidy_wrapper_errors(vm, label);
    Ok(r)
}

/// `php_stream_stat_path_ex`: `Err(())`-like `None` result means "no
/// url_stat support" (-1 in C without calling the wrapper).
pub enum StatResult {
    Ok(StatBuf),
    Failed,
    Unsupported,
}

pub fn stat_path(
    vm: &mut Vm,
    path: &[u8],
    flags: i32,
    context: Option<&ResRef>,
) -> PResult<StatResult> {
    let Some(loc) = locate_url_wrapper(vm, path, 0)? else {
        return Ok(StatResult::Unsupported);
    };
    if !loc.wrapper.has_url_stat() {
        return Ok(StatResult::Unsupported);
    }
    Ok(
        match loc
            .wrapper
            .url_stat(vm, &path[loc.path_offset..], flags, context)?
        {
            Some(sb) => StatResult::Ok(sb),
            None => StatResult::Failed,
        },
    )
}

/// `php_stream_mkdir`.
pub fn mkdir(
    vm: &mut Vm,
    path: &[u8],
    mode: i64,
    options: i32,
    context: Option<&ResRef>,
) -> PResult<bool> {
    let Some(loc) = locate_url_wrapper(vm, path, 0)? else {
        return Ok(false);
    };
    if !loc.wrapper.has_mkdir() {
        return Ok(false);
    }
    loc.wrapper.mkdir(vm, path, mode, options, context)
}

/// `php_stream_rmdir`.
pub fn rmdir(vm: &mut Vm, path: &[u8], options: i32, context: Option<&ResRef>) -> PResult<bool> {
    let Some(loc) = locate_url_wrapper(vm, path, 0)? else {
        return Ok(false);
    };
    if !loc.wrapper.has_rmdir() {
        return Ok(false);
    }
    loc.wrapper.rmdir(vm, path, options, context)
}

/// The labels of the registered wrappers, in order (`stream_get_wrappers`).
pub fn wrapper_names(vm: &mut Vm) -> Vec<PhpStr> {
    wrapper_table(vm).into_iter().map(|(n, _)| n).collect()
}

/// Whether a wrapper is a userspace wrapper (`stream_wrapper_register`).
pub fn is_user_wrapper(w: &WrapperRef) -> bool {
    w.as_any().is::<user_wrapper::UserWrapper>()
}

/// Opens `path` for reading with REPORT_ERRORS (helper for other modules).
pub fn open_for_read(
    vm: &mut Vm,
    path: &[u8],
    use_include_path: bool,
) -> PResult<Option<StreamHandle>> {
    let opts = open::REPORT_ERRORS | if use_include_path { open::USE_PATH } else { 0 };
    Ok(open_wrapper(vm, path, b"rb", opts, None, None)?.and_then(|r| handle_of(&r)))
}

/// Converts a context argument (resource or null) like `php_stream_context_from_zval`.
pub fn context_arg(vm: &mut Vm, v: &Value, no_default: bool) -> PResult<Option<ResRef>> {
    super::context::from_zval(vm, v, no_default)
}
