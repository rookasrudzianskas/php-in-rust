//! main/streams/plain_wrapper.c — file-descriptor streams ("STDIO" ops),
//! plain directory streams and the `file://` wrapper; plus the path helpers
//! of main/fopen_wrappers.c (`expand_filepath`, open_basedir) and
//! main/php_open_temporary_file.c.

use std::any::Any;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::Child;

use php_runtime::{PResult, PhpArray, PhpStr, ResRef, Vm};

use super::stream::{
    self, CastAs, OptParam, StatBuf, Stream, StreamKind, StreamOps, StreamState, alloc, flags,
    open, opt, stream_notice,
};
use super::sys;
use super::wrappers::{self, MetaValue, URL_STAT_LINK, URL_STAT_QUIET, Wrapper, globals};

pub fn os(p: &[u8]) -> &Path {
    Path::new(std::ffi::OsStr::from_bytes(p))
}

/// `php_stream_parse_fopen_modes`: the open options for a mode string,
/// `None` for an unknown first character.
pub fn parse_fopen_modes(mode: &[u8]) -> Option<OpenOptions> {
    let mut o = OpenOptions::new();
    let plus = mode.contains(&b'+');
    match mode.first()? {
        b'r' => {
            o.read(true);
            if plus {
                o.write(true);
            }
        }
        b'w' => {
            o.write(true).create(true).truncate(true);
            if plus {
                o.read(true);
            }
        }
        b'a' => {
            o.append(true).create(true);
            if plus {
                o.read(true);
            }
        }
        b'x' => {
            o.write(true).create_new(true);
            if plus {
                o.read(true);
            }
        }
        b'c' => {
            o.write(true).create(true);
            if plus {
                o.read(true);
            }
        }
        _ => return None,
    }
    if mode.contains(&b'n') {
        o.custom_flags(sys::OPEN_NONBLOCK);
    }
    o.mode(0o666);
    Some(o)
}

/// `php_stdio_stream_data`.
pub struct StdioOps {
    pub file: Option<File>,
    pub is_seekable: bool,
    pub is_pipe: bool,
    /// A `popen()` stream: the child to wait for on close.
    pub child: Option<Child>,
    /// The stream wraps a C `FILE*` (the CLI's STDIN/STDOUT/STDERR).
    pub has_stdio: bool,
    pub lock_flag: i32,
    /// Path of a `tmpfile()` to unlink on close.
    pub temp_name: Option<PhpStr>,
    /// Flush the VM's output sink before writing (stdout/stderr ordering).
    pub sync_output: bool,
    cached: Option<StatBuf>,
}

impl StdioOps {
    pub fn new(file: File) -> StdioOps {
        StdioOps {
            file: Some(file),
            is_seekable: true,
            is_pipe: false,
            child: None,
            has_stdio: false,
            lock_flag: sys::LOCK_UN,
            temp_name: None,
            sync_output: false,
            cached: None,
        }
    }

    pub fn fd(&self) -> Option<RawFd> {
        self.file.as_ref().map(|f| f.as_raw_fd())
    }

    /// `detect_is_seekable`.
    fn detect_is_seekable(&mut self) {
        if let Some(f) = &self.file {
            if let Ok(m) = f.metadata() {
                use std::os::unix::fs::FileTypeExt;
                let ft = m.file_type();
                #[cfg(target_os = "linux")]
                {
                    use std::os::unix::fs::MetadataExt;
                    if ft.is_char_device() {
                        let rdev = m.rdev();
                        let major = ((rdev >> 8) & 0xfff) | ((rdev >> 32) & !0xfff);
                        let minor = (rdev & 0xff) | ((rdev >> 12) & !0xff);
                        self.is_seekable = major == 1 && matches!(minor, 1 | 2 | 3 | 4 | 5 | 7);
                    } else {
                        self.is_seekable = !ft.is_fifo();
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    self.is_seekable = !(ft.is_fifo() || ft.is_char_device());
                }
                self.is_pipe = ft.is_fifo();
                self.cached = Some(StatBuf::from_metadata(&m));
            }
        }
    }
}

impl Drop for StdioOps {
    fn drop(&mut self) {
        self.file = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.wait();
        }
        if let Some(t) = self.temp_name.take() {
            let _ = std::fs::remove_file(os(t.as_bytes()));
        }
    }
}

fn errno_msg(e: &io::Error) -> (i32, String) {
    let n = sys::errno_of(e);
    (n, sys::strerror(n))
}

impl StreamOps for StdioOps {
    fn label(&self) -> &'static str {
        "STDIO"
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Stdio
    }

    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        if self.sync_output {
            vm.sapi_flush();
        }
        let Some(f) = self.file.as_mut() else {
            return Ok(-1);
        };
        let r = f.write(buf);
        clear_stat_cache(vm);
        match r {
            Ok(n) => Ok(n as isize),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(-1),
            Err(e) => {
                if st.flags & flags::SUPPRESS_ERRORS == 0 {
                    let (n, m) = errno_msg(&e);
                    stream_notice(
                        vm,
                        st,
                        &format!("Write of {} bytes failed with errno={n} {m}", buf.len()),
                    )?;
                }
                Ok(-1)
            }
        }
    }

    fn read(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize> {
        let Some(f) = self.file.as_mut() else {
            return Ok(-1);
        };
        let mut r = f.read(buf);
        if matches!(&r, Err(e) if e.kind() == io::ErrorKind::Interrupted) {
            r = f.read(buf);
        }
        let ret = match r {
            Ok(0) => {
                st.eof = true;
                0
            }
            Ok(n) => n as isize,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => 0,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => -1,
            Err(e) => {
                let (n, m) = errno_msg(&e);
                if st.flags & flags::SUPPRESS_ERRORS == 0 {
                    stream_notice(
                        vm,
                        st,
                        &format!("Read of {} bytes failed with errno={n} {m}", buf.len()),
                    )?;
                }
                if n != sys::EBADF {
                    st.eof = true;
                }
                -1
            }
        };
        clear_stat_cache(vm);
        Ok(ret)
    }

    fn close(&mut self, _vm: &mut Vm, _st: &mut StreamState, close_handle: bool) -> PResult<i32> {
        if !close_handle {
            // NO_CLOSE streams (STDIN/STDOUT/STDERR) hold duplicated
            // descriptors, so dropping ours leaves the process's open.
            self.file = None;
            return Ok(0);
        }
        let mut ret = 0;
        self.file = None;
        if let Some(mut c) = self.child.take() {
            ret = match c.wait() {
                Ok(s) => process_status(&s),
                Err(_) => -1,
            };
        }
        if let Some(t) = self.temp_name.take() {
            let _ = std::fs::remove_file(os(t.as_bytes()));
        }
        Ok(ret)
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        vm: &mut Vm,
        _st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        if !self.is_seekable {
            vm.warning("Cannot seek on this stream")?;
            return Ok((-1, None));
        }
        let Some(f) = self.file.as_mut() else {
            return Ok((-1, None));
        };
        let pos = match whence {
            0 if offset >= 0 => SeekFrom::Start(offset as u64),
            0 => return Ok((-1, None)),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Ok((-1, None)),
        };
        match f.seek(pos) {
            Ok(p) => Ok((0, Some(p as i64))),
            Err(_) => Ok((-1, None)),
        }
    }

    fn cast_fd(&mut self, _as: CastAs) -> Option<RawFd> {
        self.fd()
    }

    fn stat(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<StatBuf>> {
        Ok(self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| StatBuf::from_metadata(&m)))
    }

    fn set_option(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        let fd = self.fd();
        match option {
            opt::BLOCKING => {
                let Some(fd) = fd else {
                    return Ok(-1);
                };
                let Ok(nb) = sys::sys_get_nonblock(fd) else {
                    return Ok(-1);
                };
                let oldval = if nb { 0 } else { 1 };
                if sys::sys_set_nonblock(fd, value == 0).is_err() {
                    return Ok(-1);
                }
                Ok(oldval)
            }
            opt::WRITE_BUFFER => {
                if !self.has_stdio {
                    return Ok(-1);
                }
                Ok(if (0..=2).contains(&value) { 0 } else { -1 })
            }
            opt::LOCKING => {
                let Some(fd) = fd else {
                    return Ok(-1);
                };
                if matches!(param, OptParam::Size(n) if *n == opt::LOCK_SUPPORTED as i64) {
                    return Ok(0);
                }
                match sys::sys_flock(fd, value) {
                    Ok(()) => {
                        self.lock_flag = value;
                        Ok(0)
                    }
                    Err(e) => {
                        globals(vm).last_errno = sys::errno_of(&e);
                        Ok(-1)
                    }
                }
            }
            opt::SYNC_API => match value {
                opt::SYNC_SUPPORTED => Ok(if fd.is_none() {
                    opt::RETURN_ERR
                } else {
                    opt::RETURN_OK
                }),
                opt::SYNC_FSYNC | opt::SYNC_FDSYNC => {
                    let Some(f) = self.file.as_ref() else {
                        return Ok(opt::RETURN_ERR);
                    };
                    let r = if value == opt::SYNC_FDSYNC {
                        f.sync_data()
                    } else {
                        f.sync_all()
                    };
                    Ok(if r.is_ok() {
                        opt::RETURN_OK
                    } else {
                        opt::RETURN_ERR
                    })
                }
                _ => Ok(opt::RETURN_ERR),
            },
            opt::TRUNCATE_API => match value {
                opt::TRUNCATE_SUPPORTED => Ok(if fd.is_none() {
                    opt::RETURN_ERR
                } else {
                    opt::RETURN_OK
                }),
                opt::TRUNCATE_SET_SIZE => {
                    let OptParam::Size(n) = param else {
                        return Ok(opt::RETURN_ERR);
                    };
                    if *n < 0 {
                        return Ok(opt::RETURN_ERR);
                    }
                    let Some(f) = self.file.as_ref() else {
                        return Ok(opt::RETURN_ERR);
                    };
                    Ok(if f.set_len(*n as u64).is_ok() {
                        opt::RETURN_OK
                    } else {
                        opt::RETURN_ERR
                    })
                }
                _ => Ok(opt::RETURN_NOTIMPL),
            },
            opt::META_DATA_API => {
                let Some(fd) = fd else {
                    return Ok(-1);
                };
                let nb = sys::sys_get_nonblock(fd).unwrap_or(false);
                if let OptParam::Meta(a) = param {
                    a.insert_str("timed_out", false);
                    a.insert_str("blocked", !nb);
                    a.insert_str("eof", st.eof);
                }
                Ok(opt::RETURN_OK)
            }
            _ => Ok(opt::RETURN_NOTIMPL),
        }
    }
}

/// Exit status as `pclose()` + `WEXITSTATUS` report it.
pub fn process_status(s: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    match s.code() {
        Some(c) => c,
        None => s.into_raw(),
    }
}

/// `_php_stream_fopen_from_fd`: wraps an open file in a STDIO stream.
pub fn fopen_from_fd(vm: &mut Vm, file: File, mode: &[u8], zero_position: bool) -> ResRef {
    let mut ops = StdioOps::new(file);
    ops.detect_is_seekable();
    let seekable = ops.is_seekable;
    let pos = if seekable && !zero_position {
        ops.file.as_mut().and_then(|f| f.stream_position().ok())
    } else {
        None
    };
    let res = alloc(vm, Box::new(ops), mode);
    let h = stream::handle_of(&res).expect("fresh stream");
    let mut s = h.borrow_mut();
    if !seekable {
        s.st.flags |= flags::NO_SEEK;
        s.st.position = -1;
    } else if zero_position {
        s.st.position = 0;
    } else {
        match pos {
            Some(p) => s.st.position = p as i64,
            None => {
                s.st.flags |= flags::NO_SEEK;
                if let Some(o) = s.ops_as::<StdioOps>() {
                    o.is_seekable = false;
                }
                s.st.position = -1;
            }
        }
    }
    drop(s);
    res
}

/// `_php_stream_fopen_from_pipe` (popen): a non-seekable process pipe.
pub fn fopen_from_pipe(vm: &mut Vm, fd: OwnedFd, child: Child, mode: &[u8]) -> ResRef {
    let mut ops = StdioOps::new(File::from(fd));
    ops.is_seekable = false;
    ops.is_pipe = true;
    ops.child = Some(child);
    let res = alloc(vm, Box::new(ops), mode);
    if let Some(h) = stream::handle_of(&res) {
        h.borrow_mut().st.flags |= flags::NO_SEEK;
    }
    res
}

// ---------------------------------------------------------------------------
// Stat cache (BG(CurrentStatFile) / BG(CurrentLStatFile))
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct StatCache {
    pub stat: Option<(PhpStr, StatBuf)>,
    pub lstat: Option<(PhpStr, StatBuf)>,
}

/// `php_clear_stat_cache(0, NULL, 0)`.
pub fn clear_stat_cache(vm: &mut Vm) {
    let c = vm.ext_data::<StatCache>();
    c.stat = None;
    c.lstat = None;
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Textual normalisation of an absolute path (`virtual_file_ex` with
/// CWD_EXPAND): collapses `//`, `.` and `..`.
pub fn normalize(abs: &[u8]) -> Vec<u8> {
    let mut parts: Vec<&[u8]> = Vec::new();
    for comp in abs.split(|&c| c == b'/') {
        match comp {
            b"" | b"." => {}
            b".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let mut out = Vec::with_capacity(abs.len());
    for p in &parts {
        out.push(b'/');
        out.extend_from_slice(p);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

pub fn getcwd_bytes() -> Option<Vec<u8>> {
    std::env::current_dir()
        .ok()
        .map(|p| p.as_os_str().as_bytes().to_vec())
}

/// `expand_filepath_with_mode(..., CWD_EXPAND)`.
pub fn expand_only(path: &[u8]) -> Option<Vec<u8>> {
    if path.is_empty() {
        return None;
    }
    let abs = if path.starts_with(b"/") {
        path.to_vec()
    } else {
        let mut c = getcwd_bytes()?;
        c.push(b'/');
        c.extend_from_slice(path);
        c
    };
    Some(normalize(&abs))
}

/// `expand_filepath` (CWD_FILEPATH): symlinks of the existing prefix are
/// resolved, the rest is expanded textually.
pub fn expand_filepath(path: &[u8]) -> Option<Vec<u8>> {
    let expanded = expand_only(path)?;
    if let Ok(c) = std::fs::canonicalize(os(&expanded)) {
        return Some(c.as_os_str().as_bytes().to_vec());
    }
    // resolve the longest existing prefix
    let mut cut = expanded.len();
    while let Some(i) = expanded[..cut].iter().rposition(|&c| c == b'/') {
        if i == 0 {
            break;
        }
        if let Ok(c) = std::fs::canonicalize(os(&expanded[..i])) {
            let mut out = c.as_os_str().as_bytes().to_vec();
            out.extend_from_slice(&expanded[i..]);
            return Some(out);
        }
        cut = i;
    }
    Some(expanded)
}

/// `VCWD_REALPATH` / `tsrm_realpath`: the file must exist.
pub fn real_path(path: &[u8]) -> Option<PhpStr> {
    let expanded = expand_only(path)?;
    std::fs::canonicalize(os(&expanded))
        .ok()
        .map(|p| PhpStr::from_bytes(p.as_os_str().as_bytes()))
}

/// `php_check_specific_open_basedir`: true when `path` is inside `basedir`.
fn check_specific_open_basedir(basedir: &[u8], path: &[u8]) -> bool {
    let local_basedir: Vec<u8> = if basedir == b"." {
        getcwd_bytes().unwrap_or_else(|| basedir.to_vec())
    } else {
        basedir.to_vec()
    };
    if path.len() > 4095 {
        return false;
    }
    let Some(mut resolved_name) = expand_filepath(path) else {
        return false;
    };
    let mut path_tmp = resolved_name.clone();
    let expanded_ends_with_slash = path_tmp.ends_with(b"/");
    let mut cut = false;
    let mut nesting = 0;
    loop {
        if let Ok(r) = std::fs::canonicalize(os(&path_tmp)) {
            resolved_name = r.as_os_str().as_bytes().to_vec();
            break;
        }
        if nesting == 0 {
            if let Ok(t) = std::fs::read_link(os(&path_tmp)) {
                path_tmp = t.as_os_str().as_bytes().to_vec();
            }
        }
        let Some(slash) = path_tmp.iter().rposition(|&c| c == b'/') else {
            return false;
        };
        cut = true;
        path_tmp.truncate(slash);
        if path_tmp.is_empty() {
            break;
        }
        nesting += 1;
    }
    let Some(mut resolved_basedir) = expand_filepath(&local_basedir) else {
        return false;
    };
    if !resolved_basedir.ends_with(b"/") {
        resolved_basedir.push(b'/');
    }
    if !cut && expanded_ends_with_slash && !resolved_name.ends_with(b"/") {
        resolved_name.push(b'/');
    }
    if resolved_name.starts_with(&resolved_basedir) {
        !(resolved_name.len() > resolved_basedir.len()
            && resolved_name[resolved_basedir.len() - 1] != b'/')
    } else {
        resolved_basedir.len() == resolved_name.len() + 1
            && resolved_basedir.starts_with(&resolved_name)
    }
}

/// `php_check_open_basedir_ex`: `Ok(true)` when access is denied.
pub fn check_open_basedir(vm: &mut Vm, path: &[u8], warn: bool) -> PResult<bool> {
    let Some(obd) = vm.ini_get("open_basedir").filter(|v| !v.is_empty()) else {
        return Ok(false);
    };
    if path.len() > 4095 {
        vm.warning(format!(
            "File name is longer than the maximum allowed path length on this platform (4096): {}",
            String::from_utf8_lossy(path)
        ))?;
        globals(vm).last_errno = sys::EINVAL;
        return Ok(true);
    }
    for dir in obd.as_bytes().split(|&c| c == b':') {
        if dir.is_empty() {
            continue;
        }
        if check_specific_open_basedir(dir, path) {
            return Ok(false);
        }
    }
    if warn {
        vm.warning(format!(
            "open_basedir restriction in effect. File({}) is not within the allowed path(s): ({})",
            String::from_utf8_lossy(path),
            obd
        ))?;
    }
    globals(vm).last_errno = sys::EPERM;
    Ok(true)
}

fn strip_file_scheme(url: &[u8]) -> &[u8] {
    if url.len() >= 7 && url[..7].eq_ignore_ascii_case(b"file://") {
        &url[7..]
    } else {
        url
    }
}

// ---------------------------------------------------------------------------
// Temporary files
// ---------------------------------------------------------------------------

/// `php_get_temporary_directory`.
pub fn temporary_directory(vm: &mut Vm) -> PhpStr {
    if let Some(s) = vm.ini_get("sys_temp_dir").filter(|s| !s.is_empty()) {
        let b = s.as_bytes();
        if b.len() >= 2 && b.ends_with(b"/") {
            return PhpStr::from_bytes(&b[..b.len() - 1]);
        } else if !b.ends_with(b"/") {
            return s;
        }
    }
    if let Some(t) = std::env::var_os("TMPDIR") {
        let b = t.as_bytes();
        if !b.is_empty() {
            return PhpStr::from_bytes(if b.ends_with(b"/") {
                &b[..b.len() - 1]
            } else {
                b
            });
        }
    }
    PhpStr::from("/tmp")
}

fn random_u64() -> u64 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! { static SEED: Cell<u64> = const { Cell::new(0) }; }
    SEED.with(|s| {
        let mut x = s.get();
        if x == 0 {
            let t = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9e37_79b9_7f4a_7c15);
            x = t ^ ((std::process::id() as u64) << 32) ^ 0x2545_f491_4f6c_dd1d;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        x
    })
}

/// `php_do_open_temporary_file`: creates `<dir>/<pfx><13 base32><6 random>`.
fn do_open_temporary_file(dir: &[u8], pfx: &[u8]) -> Option<(File, PhpStr)> {
    if dir.is_empty() {
        return None;
    }
    let real = real_path(dir)?;
    const B32: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
    const XCH: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    for _ in 0..100 {
        let mut rnd = random_u64();
        let mut name = real.to_vec();
        if !name.ends_with(b"/") {
            name.push(b'/');
        }
        name.extend_from_slice(pfx);
        for _ in 0..13 {
            name.push(B32[(rnd % 32) as usize]);
            rnd /= 32;
        }
        let mut r2 = random_u64();
        for _ in 0..6 {
            name.push(XCH[(r2 % XCH.len() as u64) as usize]);
            r2 /= XCH.len() as u64;
        }
        if name.len() >= 4096 {
            return None;
        }
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(os(&name))
        {
            Ok(f) => return Some((f, PhpStr::from_vec(name))),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

pub const TMP_FILE_DEFAULT: u32 = 0;
pub const TMP_FILE_OPEN_BASEDIR_CHECK_ON_FALLBACK: u32 = 1;
pub const TMP_FILE_SILENT: u32 = 2;
pub const TMP_FILE_OPEN_BASEDIR_CHECK_ON_EXPLICIT_DIR: u32 = 4;

/// `php_open_temporary_fd_ex`.
pub fn open_temporary_fd(
    vm: &mut Vm,
    dir: Option<&[u8]>,
    pfx: &[u8],
    flags: u32,
) -> PResult<Option<(File, PhpStr)>> {
    let pfx: &[u8] = if pfx.is_empty() { b"tmp." } else { pfx };
    if let Some(d) = dir.filter(|d| !d.is_empty()) {
        if flags & TMP_FILE_OPEN_BASEDIR_CHECK_ON_EXPLICIT_DIR != 0
            && check_open_basedir(vm, d, true)?
        {
            return Ok(None);
        }
        if let Some(r) = do_open_temporary_file(d, pfx) {
            return Ok(Some(r));
        }
        if flags & TMP_FILE_SILENT == 0 {
            vm.notice("file created in the system's temporary directory")?;
        }
    }
    let temp_dir = temporary_directory(vm);
    if temp_dir.is_empty()
        || (flags & TMP_FILE_OPEN_BASEDIR_CHECK_ON_FALLBACK != 0
            && check_open_basedir(vm, temp_dir.as_bytes(), true)?)
    {
        return Ok(None);
    }
    Ok(do_open_temporary_file(temp_dir.as_bytes(), pfx))
}

/// `php_stream_fopen_temporary_file` / `php_stream_fopen_tmpfile`.
pub fn fopen_temporary_file(
    vm: &mut Vm,
    dir: Option<&[u8]>,
    pfx: &[u8],
) -> PResult<Option<ResRef>> {
    let Some((f, path)) = open_temporary_fd(vm, dir, pfx, TMP_FILE_DEFAULT)? else {
        return Ok(None);
    };
    let res = fopen_from_fd(vm, f, b"r+b", true);
    if let Some(h) = stream::handle_of(&res) {
        let plain = wrappers::plain_wrapper(vm);
        let mut s = h.borrow_mut();
        s.wrapper = Some(plain);
        s.st.wrapper_label = Some("plainfile");
        s.orig_path = Some(path.clone());
        if let Some(o) = s.ops_as::<StdioOps>() {
            o.temp_name = Some(path);
            o.lock_flag = sys::LOCK_UN;
        }
    }
    Ok(Some(res))
}

// ---------------------------------------------------------------------------
// Directory streams
// ---------------------------------------------------------------------------

/// `php_plain_files_dirstream_ops`: entries read through `readdir`; `.`
/// and `..` come first like on the usual Unix filesystems.
pub struct PlainDirOps {
    path: Vec<u8>,
    entries: Vec<Vec<u8>>,
    pos: usize,
}

fn read_dir_entries(path: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    let rd = std::fs::read_dir(os(path))?;
    let mut v: Vec<Vec<u8>> = vec![b".".to_vec(), b"..".to_vec()];
    for e in rd.flatten() {
        v.push(e.file_name().as_bytes().to_vec());
    }
    Ok(v)
}

impl StreamOps for PlainDirOps {
    fn label(&self) -> &'static str {
        "dir"
    }

    fn kind(&self) -> StreamKind {
        StreamKind::Dir
    }

    fn can_write(&self) -> bool {
        false
    }

    fn read(&mut self, _vm: &mut Vm, _st: &mut StreamState, _buf: &mut [u8]) -> PResult<isize> {
        Ok(-1)
    }

    fn readdir(&mut self, _vm: &mut Vm, _st: &mut StreamState) -> PResult<Option<Vec<u8>>> {
        let e = self.entries.get(self.pos).cloned();
        if e.is_some() {
            self.pos += 1;
        }
        Ok(e)
    }

    fn can_seek(&self) -> bool {
        true
    }

    fn seek(
        &mut self,
        _vm: &mut Vm,
        _st: &mut StreamState,
        _offset: i64,
        _whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        if let Ok(e) = read_dir_entries(&self.path) {
            self.entries = e;
        }
        self.pos = 0;
        Ok((0, None))
    }
}

// ---------------------------------------------------------------------------
// The plain-files wrapper
// ---------------------------------------------------------------------------

pub struct PlainWrapper;

/// `_php_stream_fopen`.
pub fn stream_fopen(
    vm: &mut Vm,
    filename: &[u8],
    mode: &[u8],
    opened_path: &mut Option<PhpStr>,
    options: i32,
) -> PResult<Option<ResRef>> {
    let Some(oo) = parse_fopen_modes(mode) else {
        wrappers::log_error(
            vm,
            "plainfile",
            options,
            format!(
                "`{}' is not a valid mode for fopen",
                String::from_utf8_lossy(mode)
            ),
        )?;
        return Ok(None);
    };
    let realpath = if options & open::STREAM_ASSUME_REALPATH != 0 {
        filename.to_vec()
    } else {
        match expand_filepath(filename) {
            Some(p) => p,
            None => {
                globals(vm).last_errno = sys::ENOENT;
                return Ok(None);
            }
        }
    };
    match oo.open(os(&realpath)) {
        Ok(f) => {
            let append = mode.first() == Some(&b'a');
            let res = fopen_from_fd(vm, f, mode, !append);
            clear_stat_cache(vm);
            *opened_path = Some(PhpStr::from_vec(realpath));
            if options & open::STREAM_OPEN_FOR_INCLUDE != 0 {
                if let Some(h) = stream::handle_of(&res) {
                    let mut s = h.borrow_mut();
                    let isreg = s
                        .ops_as::<StdioOps>()
                        .and_then(|o| o.cached.clone())
                        .is_none_or(|sb| sb.is_reg());
                    if !isreg {
                        let _ = s.free(vm, stream::PHP_STREAM_FREE_CLOSE);
                        drop(s);
                        res.close();
                        *opened_path = None;
                        return Ok(None);
                    }
                }
            }
            Ok(Some(res))
        }
        Err(e) => {
            globals(vm).last_errno = sys::errno_of(&e);
            Ok(None)
        }
    }
}

fn mkdir_one(path: &[u8], mode: i64) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .mode(mode as u32 & 0o7777)
        .create(os(path))
}

impl Wrapper for PlainWrapper {
    fn label(&self) -> &'static str {
        "plainfile"
    }

    fn open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        mode: &[u8],
        options: i32,
        opened_path: &mut Option<PhpStr>,
        _ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        if options & open::STREAM_DISABLE_OPEN_BASEDIR == 0 && check_open_basedir(vm, path, true)? {
            return Ok(None);
        }
        stream_fopen(vm, path, mode, opened_path, options)
    }

    fn has_url_stat(&self) -> bool {
        true
    }

    fn url_stat(
        &self,
        vm: &mut Vm,
        url: &[u8],
        flags: i32,
        _ctx: Option<&ResRef>,
    ) -> PResult<Option<StatBuf>> {
        let url = strip_file_scheme(url);
        if flags & 4 == 0 && check_open_basedir(vm, url, flags & URL_STAT_QUIET == 0)? {
            return Ok(None);
        }
        let r = if flags & URL_STAT_LINK != 0 {
            std::fs::symlink_metadata(os(url))
        } else {
            std::fs::metadata(os(url))
        };
        match r {
            Ok(m) => Ok(Some(StatBuf::from_metadata(&m))),
            Err(e) => {
                globals(vm).last_errno = sys::errno_of(&e);
                Ok(None)
            }
        }
    }

    fn has_dir_opener(&self) -> bool {
        true
    }

    fn dir_open(
        &self,
        vm: &mut Vm,
        path: &[u8],
        options: i32,
        ctx: Option<&ResRef>,
    ) -> PResult<Option<ResRef>> {
        if options & open::STREAM_USE_GLOB_DIR_OPEN != 0 {
            return super::glob_wrapper::GlobWrapper.dir_open(vm, path, options, ctx);
        }
        if options & open::STREAM_DISABLE_OPEN_BASEDIR == 0 && check_open_basedir(vm, path, true)? {
            return Ok(None);
        }
        match read_dir_entries(path) {
            Ok(entries) => Ok(Some(alloc(
                vm,
                Box::new(PlainDirOps {
                    path: path.to_vec(),
                    entries,
                    pos: 0,
                }),
                b"r",
            ))),
            Err(e) => {
                globals(vm).last_errno = sys::errno_of(&e);
                Ok(None)
            }
        }
    }

    fn has_unlink(&self) -> bool {
        true
    }

    fn unlink(
        &self,
        vm: &mut Vm,
        url: &[u8],
        options: i32,
        _ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let url = strip_file_scheme(url);
        if check_open_basedir(vm, url, true)? {
            return Ok(false);
        }
        match std::fs::remove_file(os(url)) {
            Ok(()) => {
                clear_stat_cache(vm);
                Ok(true)
            }
            Err(e) => {
                let n = sys::errno_of(&e);
                globals(vm).last_errno = n;
                wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
                Ok(false)
            }
        }
    }

    fn has_rename(&self) -> bool {
        true
    }

    fn rename(
        &self,
        vm: &mut Vm,
        from: &[u8],
        to: &[u8],
        options: i32,
        _ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let from = strip_file_scheme(from);
        let to = strip_file_scheme(to);
        if check_open_basedir(vm, from, true)? || check_open_basedir(vm, to, true)? {
            return Ok(false);
        }
        match std::fs::rename(os(from), os(to)) {
            Ok(()) => {
                clear_stat_cache(vm);
                Ok(true)
            }
            Err(e) => {
                let n = sys::errno_of(&e);
                if n == sys::EXDEV {
                    // copy + unlink across devices
                    let old = sys::sys_umask(0o077);
                    let mut success = false;
                    if std::fs::copy(os(from), os(to)).is_ok() {
                        if let Ok(m) = std::fs::metadata(os(from)) {
                            success = true;
                            let _ = std::fs::set_permissions(os(to), m.permissions());
                            let _ = std::fs::remove_file(os(from));
                        }
                    }
                    sys::sys_umask(old);
                    return Ok(success);
                }
                wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
                Ok(false)
            }
        }
    }

    fn has_mkdir(&self) -> bool {
        true
    }

    fn mkdir(
        &self,
        vm: &mut Vm,
        dir: &[u8],
        mode: i64,
        options: i32,
        _ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let dir = strip_file_scheme(dir);
        if options & wrappers::MKDIR_RECURSIVE == 0 {
            if check_open_basedir(vm, dir, true)? {
                return Ok(false);
            }
            return match mkdir_one(dir, mode) {
                Ok(()) => Ok(true),
                Err(e) => {
                    let n = sys::errno_of(&e);
                    wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
                    Ok(false)
                }
            };
        }
        let Some(buf) = expand_only(dir) else {
            wrappers::wrapper_warn(vm, options, "Invalid path")?;
            return Ok(false);
        };
        if check_open_basedir(vm, &buf, true)? {
            return Ok(false);
        }
        // find the top-most existing ancestor, then create downwards
        let mut prefixes: Vec<usize> = buf
            .iter()
            .enumerate()
            .filter(|&(i, &c)| c == b'/' && i > 0)
            .map(|(i, _)| i)
            .collect();
        prefixes.push(buf.len());
        let mut start = 0;
        for (k, &end) in prefixes.iter().enumerate().rev() {
            if std::fs::metadata(os(&buf[..end])).is_ok() {
                start = k + 1;
                break;
            }
        }
        if start >= prefixes.len() {
            // the full path exists already
            let e = mkdir_one(&buf, mode).err();
            if let Some(e) = e {
                let n = sys::errno_of(&e);
                wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
            }
            return Ok(false);
        }
        for (k, &end) in prefixes.iter().enumerate().skip(start) {
            if let Err(e) = mkdir_one(&buf[..end], mode) {
                let n = sys::errno_of(&e);
                if n != sys::EEXIST || k == prefixes.len() - 1 {
                    wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    fn has_rmdir(&self) -> bool {
        true
    }

    fn rmdir(&self, vm: &mut Vm, url: &[u8], options: i32, _ctx: Option<&ResRef>) -> PResult<bool> {
        let url = strip_file_scheme(url);
        if check_open_basedir(vm, url, true)? {
            return Ok(false);
        }
        match std::fs::remove_dir(os(url)) {
            Ok(()) => {
                clear_stat_cache(vm);
                Ok(true)
            }
            Err(e) => {
                let n = sys::errno_of(&e);
                wrappers::wrapper_warn(vm, options, sys::strerror(n))?;
                Ok(false)
            }
        }
    }

    fn has_metadata(&self) -> bool {
        true
    }

    fn metadata(
        &self,
        vm: &mut Vm,
        url: &[u8],
        value: &MetaValue,
        _ctx: Option<&ResRef>,
    ) -> PResult<bool> {
        let url = strip_file_scheme(url);
        if check_open_basedir(vm, url, true)? {
            return Ok(false);
        }
        let p = os(url);
        let r: io::Result<()> = match value {
            MetaValue::Touch(times) => {
                if std::fs::metadata(p).is_err() {
                    if let Err(e) = File::create(p) {
                        let n = sys::errno_of(&e);
                        vm.warning(format!(
                            "Unable to create file {} because {}",
                            String::from_utf8_lossy(url),
                            sys::strerror(n)
                        ))?;
                        return Ok(false);
                    }
                }
                touch_times(p, *times)
            }
            MetaValue::OwnerName(name) => match sys::sys_uid_by_name(name.as_bytes()) {
                Some(uid) => std::os::unix::fs::chown(p, Some(uid), None),
                None => {
                    vm.warning(format!("Unable to find uid for {name}"))?;
                    return Ok(false);
                }
            },
            MetaValue::Owner(uid) => std::os::unix::fs::chown(p, Some(*uid as u32), None),
            MetaValue::GroupName(name) => match sys::sys_gid_by_name(name.as_bytes()) {
                Some(gid) => std::os::unix::fs::chown(p, None, Some(gid)),
                None => {
                    vm.warning(format!("Unable to find gid for {name}"))?;
                    return Ok(false);
                }
            },
            MetaValue::Group(gid) => std::os::unix::fs::chown(p, None, Some(*gid as u32)),
            MetaValue::Access(mode) => {
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(*mode as u32 & 0o7777))
            }
        };
        if let Err(e) = r {
            let n = sys::errno_of(&e);
            vm.warning(format!("Operation failed: {}", sys::strerror(n)))?;
            return Ok(false);
        }
        clear_stat_cache(vm);
        Ok(true)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `utime(path, times)`: `None` sets both to now.
pub fn touch_times(p: &Path, times: Option<(i64, i64)>) -> io::Result<()> {
    sys::sys_utime(p.as_os_str().as_bytes(), times)
}

/// `is_stream_on_plain_files`-style helper used by `stream_get_meta_data`.
pub fn stdio_meta(s: &mut Stream, vm: &mut Vm, arr: &mut PhpArray) -> PResult<bool> {
    let mut p = OptParam::Meta(arr);
    Ok(s.set_option(vm, opt::META_DATA_API, 0, &mut p)? == opt::RETURN_OK)
}
