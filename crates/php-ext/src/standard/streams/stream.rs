//! main/streams/streams.c — the generic stream: read buffer, line reading,
//! filtered reads/writes, seeking, options and freeing.
//!
//! A [`Stream`] owns its low-level [`StreamOps`] (plain file, memory, socket,
//! userspace, ...) and lives inside a PHP resource of type `"stream"` as a
//! shared [`StreamHandle`]. The buffering logic is a line-by-line port of
//! php-src so that read sizes, `feof()` and `ftell()` behave identically.

use std::any::Any;
use std::cell::RefCell;
use std::os::fd::RawFd;
use std::rc::Rc;

use php_runtime::errors::level::{E_NOTICE, E_WARNING};
use php_runtime::{PResult, PhpArray, PhpStr, ResRef, Value, Vm};

use super::filter::{
    Brigade, Filter, FilterCtx, FilterStatus, PSFS_FLAG_FLUSH_CLOSE, PSFS_FLAG_FLUSH_INC,
    PSFS_FLAG_NORMAL, Seekable, new_bucket,
};
use super::wrappers::Wrapper;

/// Shared handle to a stream (the payload of a `"stream"` resource).
pub type StreamHandle = Rc<RefCell<Stream>>;

/// `PHP_STREAM_FLAG_*`.
pub mod flags {
    pub const NO_SEEK: u32 = 0x1;
    pub const NO_BUFFER: u32 = 0x2;
    pub const DETECT_EOL: u32 = 0x4;
    pub const EOL_MAC: u32 = 0x8;
    pub const AVOID_BLOCKING: u32 = 0x10;
    pub const NO_CLOSE: u32 = 0x20;
    pub const IS_DIR: u32 = 0x40;
    pub const NO_FCLOSE: u32 = 0x80;
    pub const SUPPRESS_ERRORS: u32 = 0x100;
    pub const NO_RSCR_DTOR_CLOSE: u32 = 0x200;
    pub const NO_IO: u32 = 0x400;
    pub const WAS_WRITTEN: u32 = 0x8000_0000;
}

/// `PHP_STREAM_OPTION_*` and friends.
pub mod opt {
    pub const BLOCKING: i32 = 1;
    pub const READ_BUFFER: i32 = 2;
    pub const WRITE_BUFFER: i32 = 3;
    pub const READ_TIMEOUT: i32 = 4;
    pub const SET_CHUNK_SIZE: i32 = 5;
    pub const LOCKING: i32 = 6;
    pub const XPORT_API: i32 = 7;
    pub const CRYPTO_API: i32 = 8;
    pub const MMAP_API: i32 = 9;
    pub const TRUNCATE_API: i32 = 10;
    pub const META_DATA_API: i32 = 11;
    pub const CHECK_LIVENESS: i32 = 12;
    pub const PIPE_BLOCKING: i32 = 13;
    pub const SYNC_API: i32 = 14;

    pub const BUFFER_NONE: i32 = 0;
    pub const BUFFER_LINE: i32 = 1;
    pub const BUFFER_FULL: i32 = 2;

    pub const TRUNCATE_SUPPORTED: i32 = 0;
    pub const TRUNCATE_SET_SIZE: i32 = 1;

    pub const SYNC_SUPPORTED: i32 = 0;
    pub const SYNC_FSYNC: i32 = 1;
    pub const SYNC_FDSYNC: i32 = 2;

    pub const LOCK_SUPPORTED: i32 = 1;

    pub const RETURN_OK: i32 = 0;
    pub const RETURN_ERR: i32 = -1;
    pub const RETURN_NOTIMPL: i32 = -2;
}

/// `php_stream_open_wrapper` option bits.
pub mod open {
    pub const USE_PATH: i32 = 0x1;
    pub const IGNORE_URL: i32 = 0x2;
    pub const REPORT_ERRORS: i32 = 0x8;
    pub const STREAM_MUST_SEEK: i32 = 0x10;
    pub const STREAM_WILL_CAST: i32 = 0x20;
    pub const STREAM_LOCATE_WRAPPERS_ONLY: i32 = 0x40;
    pub const STREAM_OPEN_FOR_INCLUDE: i32 = 0x80;
    pub const STREAM_USE_URL: i32 = 0x100;
    pub const STREAM_ONLY_GET_HEADERS: i32 = 0x200;
    pub const STREAM_DISABLE_OPEN_BASEDIR: i32 = 0x400;
    pub const STREAM_OPEN_PERSISTENT: i32 = 0x800;
    pub const STREAM_USE_GLOB_DIR_OPEN: i32 = 0x1000;
    pub const STREAM_DISABLE_URL_PROTECTION: i32 = 0x2000;
    pub const STREAM_ASSUME_REALPATH: i32 = 0x4000;
    pub const STREAM_USE_BLOCKING_PIPE: i32 = 0x8000;
}

/// `php_stream_set_option` buffer size (`FG(def_chunk_size)`).
pub const CHUNK_SIZE: usize = 8192;
/// `PHP_STREAM_COPY_ALL`.
pub const COPY_ALL: usize = usize::MAX;

/// `php_stream_statbuf` (a `struct stat`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatBuf {
    pub dev: i64,
    pub ino: i64,
    pub mode: i64,
    pub nlink: i64,
    pub uid: i64,
    pub gid: i64,
    pub rdev: i64,
    pub size: i64,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub blksize: i64,
    pub blocks: i64,
}

impl StatBuf {
    pub fn from_metadata(m: &std::fs::Metadata) -> StatBuf {
        use std::os::unix::fs::MetadataExt;
        StatBuf {
            dev: m.dev() as i64,
            ino: m.ino() as i64,
            mode: m.mode() as i64,
            nlink: m.nlink() as i64,
            uid: m.uid() as i64,
            gid: m.gid() as i64,
            rdev: m.rdev() as i64,
            size: m.size() as i64,
            atime: m.atime(),
            mtime: m.mtime(),
            ctime: m.ctime(),
            blksize: m.blksize() as i64,
            blocks: m.blocks() as i64,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.mode & 0o170000 == 0o040000
    }

    pub fn is_reg(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }

    pub fn is_link(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }

    /// The `stat()` / `fstat()` result array: 13 numeric then 13 named keys.
    pub fn to_php_array(&self) -> PhpArray {
        let vals = [
            ("dev", self.dev),
            ("ino", self.ino),
            ("mode", self.mode),
            ("nlink", self.nlink),
            ("uid", self.uid),
            ("gid", self.gid),
            ("rdev", self.rdev),
            ("size", self.size),
            ("atime", self.atime),
            ("mtime", self.mtime),
            ("ctime", self.ctime),
            ("blksize", self.blksize),
            ("blocks", self.blocks),
        ];
        let mut a = PhpArray::new();
        for (_, v) in vals {
            let _ = a.append(v);
        }
        for (k, v) in vals {
            a.insert_str(k, v);
        }
        a
    }
}

/// Fields of the stream that its ops may read and modify (`stream->eof`, ...).
pub struct StreamState {
    pub eof: bool,
    pub fatal_error: bool,
    pub flags: u32,
    pub mode: PhpStr,
    pub position: i64,
    /// The attached context (`stream->ctx`).
    pub context: Option<ResRef>,
    /// Label of the wrapper that opened the stream (for error reporting).
    pub wrapper_label: Option<&'static str>,
    pub is_persistent: bool,
}

/// Extra data passed to [`StreamOps::set_option`] (`void *ptrparam`).
pub enum OptParam<'a> {
    None,
    /// A size (`size_t *`), e.g. for `WRITE_BUFFER` and `TRUNCATE_API`.
    Size(i64),
    /// A timeout (`struct timeval *`): seconds and microseconds.
    Timeout(i64, i64),
    /// The metadata array (`META_DATA_API`).
    Meta(&'a mut PhpArray),
}

/// What a stream can be cast to (`PHP_STREAM_AS_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastAs {
    Stdio,
    Fd,
    SocketFd,
    FdForSelect,
}

/// Rough classification used for `php_stream_is()` checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stdio,
    Memory,
    Temp,
    Userspace,
    Socket,
    Dir,
    Other,
}

/// Downcasting support for stream ops.
pub trait AsAny: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `php_stream_ops`: the low-level implementation of a stream.
///
/// Return conventions follow C: `read`/`write` return the byte count or
/// `-1`; `close`/`flush` return 0 on success.
pub trait StreamOps: AsAny {
    fn label(&self) -> &'static str;

    fn kind(&self) -> StreamKind {
        StreamKind::Other
    }

    /// `ops->write != NULL`.
    fn can_write(&self) -> bool {
        true
    }

    fn write(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &[u8]) -> PResult<isize> {
        let _ = (vm, st, buf);
        Ok(-1)
    }

    fn read(&mut self, vm: &mut Vm, st: &mut StreamState, buf: &mut [u8]) -> PResult<isize>;

    fn close(&mut self, vm: &mut Vm, st: &mut StreamState, close_handle: bool) -> PResult<i32> {
        let _ = (vm, st, close_handle);
        Ok(0)
    }

    fn flush(&mut self, vm: &mut Vm, st: &mut StreamState) -> PResult<i32> {
        let _ = (vm, st);
        Ok(0)
    }

    /// `ops->seek != NULL`.
    fn can_seek(&self) -> bool {
        false
    }

    /// Returns `(ret, new_offset)`; `new_offset` is only applied when `Some`.
    fn seek(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        offset: i64,
        whence: i32,
    ) -> PResult<(i32, Option<i64>)> {
        let _ = (vm, st, offset, whence);
        Ok((-1, None))
    }

    /// `ops->cast`: a file descriptor for select/proc_open/isatty.
    fn cast_fd(&mut self, as_: CastAs) -> Option<RawFd> {
        let _ = as_;
        None
    }

    /// `ops->stat`; `Ok(None)` = failure / unsupported.
    fn stat(&mut self, vm: &mut Vm, st: &mut StreamState) -> PResult<Option<StatBuf>> {
        let _ = (vm, st);
        Ok(None)
    }

    fn set_option(
        &mut self,
        vm: &mut Vm,
        st: &mut StreamState,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        let _ = (vm, st, option, value, param);
        Ok(opt::RETURN_NOTIMPL)
    }

    /// Next entry of a directory stream (`php_stream_readdir`).
    fn readdir(&mut self, vm: &mut Vm, st: &mut StreamState) -> PResult<Option<Vec<u8>>> {
        let _ = (vm, st);
        Ok(None)
    }

    /// Whether closing must run PHP code (userspace wrappers).
    fn needs_vm_to_close(&self) -> bool {
        false
    }
}

/// `php_stream`.
pub struct Stream {
    pub ops: Box<dyn StreamOps>,
    pub st: StreamState,
    pub wrapper: Option<Rc<dyn Wrapper>>,
    pub readfilters: Vec<Filter>,
    pub writefilters: Vec<Filter>,
    pub orig_path: Option<PhpStr>,
    pub wrapperdata: Option<Value>,
    readbuf: Vec<u8>,
    readpos: usize,
    writepos: usize,
    pub chunk_size: usize,
    /// The stream's own resource while a [`with_stream`] call is active.
    pub res: Option<ResRef>,
    /// Resource id (for messages and identity checks).
    pub res_id: i64,
    closed: bool,
    in_free: bool,
}

/// Payload stored in a `"stream"` resource.
pub struct StreamBox(pub StreamHandle);

thread_local! {
    static DEFERRED: RefCell<Vec<Stream>> = const { RefCell::new(Vec::new()) };
}

struct ClosedOps;

impl StreamOps for ClosedOps {
    fn label(&self) -> &'static str {
        "closed"
    }
    fn read(&mut self, _: &mut Vm, _: &mut StreamState, _: &mut [u8]) -> PResult<isize> {
        Ok(-1)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let needs_vm = self.ops.needs_vm_to_close()
            || !self.writefilters.is_empty()
            || self.readfilters.iter().any(|f| f.ops.needs_vm());
        if needs_vm {
            // The resource was destroyed implicitly; finish the close (filter
            // flushes, userspace stream_close) at the next stream call.
            let placeholder = Stream::raw(Box::new(ClosedOps), b"r", 0);
            let mut taken = std::mem::replace(self, placeholder);
            taken.res = None;
            let _ = DEFERRED.try_with(|d| {
                if let Ok(mut d) = d.try_borrow_mut() {
                    d.push(taken);
                }
            });
        }
        self.closed = true;
    }
}

/// Finishes closing streams whose resources were destroyed implicitly.
/// Called at the start of stream functions (and should be called by the
/// interpreter at statement boundaries / shutdown).
pub fn process_deferred_closes(vm: &mut Vm) -> PResult<()> {
    loop {
        let next = DEFERRED
            .try_with(|d| d.try_borrow_mut().ok().and_then(|mut d| d.pop()))
            .ok()
            .flatten();
        let Some(mut s) = next else {
            return Ok(());
        };
        let flags = PHP_STREAM_FREE_CLOSE | PHP_STREAM_FREE_RSRC_DTOR;
        s.free(vm, flags)?;
    }
}

pub const PHP_STREAM_FREE_CALL_DTOR: i32 = 1;
pub const PHP_STREAM_FREE_RELEASE_STREAM: i32 = 2;
pub const PHP_STREAM_FREE_PRESERVE_HANDLE: i32 = 4;
pub const PHP_STREAM_FREE_RSRC_DTOR: i32 = 8;
pub const PHP_STREAM_FREE_PERSISTENT: i32 = 16;
pub const PHP_STREAM_FREE_KEEP_RSRC: i32 = 64;
pub const PHP_STREAM_FREE_CLOSE: i32 = PHP_STREAM_FREE_CALL_DTOR | PHP_STREAM_FREE_RELEASE_STREAM;

/// Allocates a stream resource (`php_stream_alloc`): the resource id is
/// taken now, like `zend_register_resource` in php-src.
pub fn alloc(vm: &mut Vm, ops: Box<dyn StreamOps>, mode: &[u8]) -> ResRef {
    alloc_ex(vm, ops, mode, false)
}

/// [`alloc`] for persistent streams (resource type `"persistent stream"`).
pub fn alloc_ex(vm: &mut Vm, ops: Box<dyn StreamOps>, mode: &[u8], persistent: bool) -> ResRef {
    let _ = vm;
    let mut s = Stream::raw(ops, mode, 0);
    s.st.is_persistent = persistent;
    if vm_auto_detect_line_endings(vm) {
        s.st.flags |= flags::DETECT_EOL;
    }
    let h: StreamHandle = Rc::new(RefCell::new(s));
    let res = ResRef::new(
        if persistent {
            "persistent stream"
        } else {
            "stream"
        },
        Box::new(StreamBox(h.clone())),
    );
    h.borrow_mut().res_id = res.id();
    res
}

fn vm_auto_detect_line_endings(vm: &Vm) -> bool {
    vm.ini_bool("auto_detect_line_endings")
}

/// The stream behind a resource, if it is an open stream.
pub fn handle_of(res: &ResRef) -> Option<StreamHandle> {
    if res.is_closed() {
        return None;
    }
    let tn = res.type_name();
    if tn != "stream" && tn != "persistent stream" {
        return None;
    }
    let data = res.resource().data.borrow();
    data.as_ref()?
        .downcast_ref::<StreamBox>()
        .map(|b| b.0.clone())
}

/// Runs `f` with the stream behind `res` mutably borrowed (and its own
/// resource recorded in [`Stream::res`] for filters and wrappers).
pub fn with_stream<R>(
    vm: &mut Vm,
    res: &ResRef,
    f: impl FnOnce(&mut Vm, &mut Stream) -> PResult<R>,
) -> PResult<Option<R>> {
    let Some(h) = handle_of(res) else {
        return Ok(None);
    };
    let Ok(mut s) = h.try_borrow_mut() else {
        vm.warning("Stream is already in use")?;
        return Ok(None);
    };
    let prev = s.res.replace(res.clone());
    let r = f(vm, &mut s);
    s.res = prev;
    r.map(Some)
}

/// Reports a stream error (`php_stream_error`) in the default "error" mode.
pub fn stream_error(vm: &mut Vm, st: &StreamState, severity: i64, msg: &str) -> PResult<()> {
    let _ = st;
    vm.php_error_docref(severity, msg)
}

pub fn stream_warn(vm: &mut Vm, st: &StreamState, msg: &str) -> PResult<()> {
    stream_error(vm, st, E_WARNING, msg)
}

pub fn stream_notice(vm: &mut Vm, st: &StreamState, msg: &str) -> PResult<()> {
    stream_error(vm, st, E_NOTICE, msg)
}

impl Stream {
    fn raw(ops: Box<dyn StreamOps>, mode: &[u8], res_id: i64) -> Stream {
        let mode = if mode.len() > 15 { &mode[..15] } else { mode };
        Stream {
            ops,
            st: StreamState {
                eof: false,
                fatal_error: false,
                flags: 0,
                mode: PhpStr::from_bytes(mode),
                position: 0,
                context: None,
                wrapper_label: None,
                is_persistent: false,
            },
            wrapper: None,
            readfilters: Vec::new(),
            writefilters: Vec::new(),
            orig_path: None,
            wrapperdata: None,
            readbuf: Vec::new(),
            readpos: 0,
            writepos: 0,
            chunk_size: CHUNK_SIZE,
            res: None,
            res_id,
            closed: false,
            in_free: false,
        }
    }

    /// Typed access to the stream's ops.
    pub fn ops_as<T: 'static>(&mut self) -> Option<&mut T> {
        let o: &mut dyn StreamOps = &mut *self.ops;
        o.as_any_mut().downcast_mut::<T>()
    }

    pub fn ops_ref<T: 'static>(&self) -> Option<&T> {
        let o: &dyn StreamOps = &*self.ops;
        o.as_any().downcast_ref::<T>()
    }

    pub fn kind(&self) -> StreamKind {
        self.ops.kind()
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn is_filtered(&self) -> bool {
        !self.readfilters.is_empty() || !self.writefilters.is_empty()
    }

    /// Bytes buffered but not yet consumed.
    pub fn buffered(&self) -> usize {
        self.writepos - self.readpos
    }

    /// `php_stream_free`.
    pub fn free(&mut self, vm: &mut Vm, close_options: i32) -> PResult<i32> {
        if self.closed || self.in_free {
            return Ok(1);
        }
        self.in_free = true;
        let mut preserve_handle = close_options & PHP_STREAM_FREE_PRESERVE_HANDLE != 0;
        if self.st.flags & flags::NO_CLOSE != 0
            || (self.st.flags & flags::NO_RSCR_DTOR_CLOSE != 0
                && close_options & PHP_STREAM_FREE_RSRC_DTOR != 0)
        {
            preserve_handle = true;
        }
        let flush_result =
            if self.st.flags & flags::WAS_WRITTEN != 0 || !self.writefilters.is_empty() {
                self.flush_ex(vm, true)?
            } else {
                0
            };
        let mut ret = 1;
        if close_options & PHP_STREAM_FREE_CALL_DTOR != 0 {
            ret = self.ops.close(vm, &mut self.st, !preserve_handle)?;
            if ret == 0 {
                ret = flush_result;
            }
        }
        if close_options & PHP_STREAM_FREE_RELEASE_STREAM != 0 {
            let mut filters = std::mem::take(&mut self.readfilters);
            filters.append(&mut std::mem::take(&mut self.writefilters));
            for mut f in filters {
                if let Some(r) = f.res.take() {
                    r.close();
                }
                f.ops.dtor(vm)?;
            }
            self.wrapperdata = None;
            self.readbuf = Vec::new();
        }
        self.closed = true;
        self.in_free = false;
        Ok(ret)
    }

    fn filter_ctx(&self) -> (Option<ResRef>, bool) {
        (self.res.clone(), self.st.is_persistent)
    }

    /// `php_stream_fill_read_buffer`.
    pub fn fill_read_buffer(&mut self, vm: &mut Vm, size: usize) -> PResult<bool> {
        if self.closed {
            return Ok(false);
        }
        let old_eof = self.st.eof;
        let r = self.fill_read_buffer_inner(vm, size)?;
        if old_eof != self.st.eof {
            self.notify_completed(vm)?;
        }
        Ok(r)
    }

    /// `php_stream_notify_completed` on the stream's context.
    fn notify_completed(&self, vm: &mut Vm) -> PResult<()> {
        let Some(ctx) = &self.st.context else {
            return Ok(());
        };
        let Some(c) = super::context::of(ctx) else {
            return Ok(());
        };
        let notifier = c.borrow().notifier.clone();
        if let Some(cb) = notifier {
            let args = vec![
                Value::Int(8),
                Value::Int(0),
                Value::Null,
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
            ];
            vm.call(&cb, args)?;
        }
        Ok(())
    }

    fn fill_read_buffer_inner(&mut self, vm: &mut Vm, size: usize) -> PResult<bool> {
        if !self.readfilters.is_empty() {
            let to_read_now = size.min(self.chunk_size);
            let mut chunk_buf = vec![0u8; self.chunk_size];
            let (res, persistent) = self.filter_ctx();
            while !self.st.eof && self.writepos - self.readpos < to_read_now {
                let mut brig_in = Brigade::new();
                let mut brig_out = Brigade::new();
                let justread = self.ops.read(vm, &mut self.st, &mut chunk_buf)?;
                if justread < 0 && self.writepos == self.readpos {
                    return Ok(false);
                }
                if justread > 0 {
                    brig_in.append(new_bucket(chunk_buf[..justread as usize].to_vec()));
                }
                let flags = if self.st.eof {
                    PSFS_FLAG_FLUSH_CLOSE
                } else if justread > 0 {
                    PSFS_FLAG_NORMAL
                } else {
                    PSFS_FLAG_FLUSH_INC
                };
                let mut status = FilterStatus::ErrFatal;
                let ctx = FilterCtx {
                    stream_res: res.as_ref(),
                    is_persistent: persistent,
                };
                let (mut inp, mut out) = (&mut brig_in, &mut brig_out);
                for f in self.readfilters.iter_mut() {
                    status = f.ops.filter(vm, &ctx, inp, out, None, flags)?;
                    if status != FilterStatus::PassOn {
                        break;
                    }
                    std::mem::swap(&mut inp, &mut out);
                    out.clear();
                }
                match status {
                    FilterStatus::PassOn => {
                        while let Some(b) = inp.pop_front() {
                            let data = std::mem::take(&mut b.borrow_mut().buf);
                            self.append_to_readbuf(&data);
                        }
                    }
                    FilterStatus::FeedMe => {}
                    FilterStatus::ErrFatal => {
                        self.st.eof = true;
                        self.st.fatal_error = true;
                        return Ok(false);
                    }
                }
                if justread <= 0 {
                    break;
                }
            }
            Ok(true)
        } else {
            if self.writepos - self.readpos < size {
                // reduce buffer memory consumption if possible, to avoid a realloc
                if !self.readbuf.is_empty() && self.readbuf.len() - self.writepos < self.chunk_size
                {
                    if self.writepos > self.readpos {
                        self.readbuf.copy_within(self.readpos..self.writepos, 0);
                    }
                    self.writepos -= self.readpos;
                    self.readpos = 0;
                }
                if self.readbuf.len() - self.writepos < self.chunk_size {
                    let newlen = self.readbuf.len() + self.chunk_size;
                    self.readbuf.resize(newlen, 0);
                }
                let wp = self.writepos;
                let justread = {
                    let Stream {
                        ops, st, readbuf, ..
                    } = self;
                    ops.read(vm, st, &mut readbuf[wp..])?
                };
                if justread < 0 {
                    return Ok(false);
                }
                self.writepos += justread as usize;
            }
            Ok(true)
        }
    }

    fn append_to_readbuf(&mut self, data: &[u8]) {
        if !self.readbuf.is_empty() && self.readbuf.len() - self.writepos < data.len() {
            if self.writepos > self.readpos {
                self.readbuf.copy_within(self.readpos..self.writepos, 0);
            }
            self.writepos -= self.readpos;
            self.readpos = 0;
        }
        if self.readbuf.len() - self.writepos < data.len() {
            let newlen = self.readbuf.len() + data.len();
            self.readbuf.resize(newlen, 0);
        }
        self.readbuf[self.writepos..self.writepos + data.len()].copy_from_slice(data);
        self.writepos += data.len();
    }

    /// `php_stream_read` keeps reading only for plain files, php://memory
    /// and php://temp.
    fn is_greedy(&self) -> bool {
        self.wrapper
            .as_ref()
            .is_some_and(|w| w.label() == "plainfile")
            || matches!(self.ops.kind(), StreamKind::Memory | StreamKind::Temp)
    }

    /// `php_stream_read`: up to `buf.len()` bytes, `-1` on error.
    pub fn read(&mut self, vm: &mut Vm, buf: &mut [u8]) -> PResult<isize> {
        if self.closed {
            return Ok(-1);
        }
        let mut size = buf.len();
        let mut didread: usize = 0;
        while size > 0 {
            if self.writepos > self.readpos {
                let toread = (self.writepos - self.readpos).min(size);
                buf[didread..didread + toread]
                    .copy_from_slice(&self.readbuf[self.readpos..self.readpos + toread]);
                self.readpos += toread;
                size -= toread;
                didread += toread;
            }
            if size == 0 {
                break;
            }
            let toread: isize;
            if self.readfilters.is_empty()
                && (self.st.flags & flags::NO_BUFFER != 0 || self.chunk_size == 1)
            {
                toread = self
                    .ops
                    .read(vm, &mut self.st, &mut buf[didread..didread + size])?;
                if toread < 0 {
                    if didread == 0 {
                        return Ok(toread);
                    }
                    break;
                }
            } else {
                if !self.fill_read_buffer(vm, size)? {
                    if didread == 0 {
                        return Ok(-1);
                    }
                    break;
                }
                let n = (self.writepos - self.readpos).min(size);
                if n > 0 {
                    buf[didread..didread + n]
                        .copy_from_slice(&self.readbuf[self.readpos..self.readpos + n]);
                    self.readpos += n;
                }
                toread = n as isize;
            }
            if toread > 0 {
                didread += toread as usize;
                size -= toread as usize;
            } else {
                break;
            }
            // just break anyway, to avoid greedy read for file://, php://memory, and php://temp
            if !self.is_greedy() {
                break;
            }
        }
        if didread > 0 {
            self.st.position += didread as i64;
        }
        Ok(didread as isize)
    }

    /// `php_stream_read_to_str`.
    pub fn read_to_str(&mut self, vm: &mut Vm, len: usize) -> PResult<Option<Vec<u8>>> {
        let mut buf = vec![0u8; len];
        let n = self.read(vm, &mut buf)?;
        if n < 0 {
            return Ok(None);
        }
        buf.truncate(n as usize);
        Ok(Some(buf))
    }

    /// `php_stream_eof`.
    pub fn eof(&mut self, vm: &mut Vm) -> PResult<bool> {
        if self.writepos > self.readpos {
            return Ok(false);
        }
        if !self.st.eof
            && self.set_option(vm, opt::CHECK_LIVENESS, 0, &mut OptParam::None)? == opt::RETURN_ERR
        {
            self.st.eof = true;
        }
        Ok(self.st.eof)
    }

    /// `php_stream_getc`.
    pub fn getc(&mut self, vm: &mut Vm) -> PResult<Option<u8>> {
        let mut b = [0u8; 1];
        Ok((self.read(vm, &mut b)? > 0).then_some(b[0]))
    }

    /// `php_stream_stat`.
    pub fn stat(&mut self, vm: &mut Vm) -> PResult<Option<StatBuf>> {
        if let Some(w) = self.wrapper.clone() {
            if w.has_stream_stat() {
                return w.stream_stat(vm, self);
            }
        }
        self.ops.stat(vm, &mut self.st)
    }

    /// `php_stream_locate_eol` over `buf` (or the read buffer when `None`).
    pub fn locate_eol(&mut self, buf: Option<&[u8]>) -> Option<usize> {
        let data: &[u8] = match buf {
            Some(b) => b,
            None => &self.readbuf[self.readpos..self.writepos],
        };
        if self.st.flags & flags::DETECT_EOL != 0 {
            let cr = data.iter().position(|&c| c == b'\r');
            let lf = data.iter().position(|&c| c == b'\n');
            match (cr, lf) {
                (Some(c), l) if l != Some(c + 1) && !l.is_some_and(|l| l < c) => {
                    self.st.flags ^= flags::DETECT_EOL;
                    self.st.flags |= flags::EOL_MAC;
                    Some(c)
                }
                (_, Some(l)) => {
                    self.st.flags ^= flags::DETECT_EOL;
                    Some(l)
                }
                _ => None,
            }
        } else if self.st.flags & flags::EOL_MAC != 0 {
            data.iter().position(|&c| c == b'\r')
        } else {
            data.iter().position(|&c| c == b'\n')
        }
    }

    /// `php_stream_get_line`. `maxlen` is the C buffer size (including the
    /// terminating NUL, so at most `maxlen - 1` bytes are returned); `None`
    /// means "allocate as needed" (`buf == NULL`).
    pub fn get_line(&mut self, vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
        let grow_mode = maxlen.is_none();
        let mut maxlen = maxlen.unwrap_or(0);
        if !grow_mode && maxlen == 0 {
            return Ok(None);
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let avail = self.writepos - self.readpos;
            if avail > 0 {
                let mut done = false;
                let mut cpysz = match self.locate_eol(None) {
                    Some(eol) => {
                        done = true;
                        eol + 1
                    }
                    None => avail,
                };
                if !grow_mode && cpysz >= maxlen - 1 {
                    cpysz = maxlen - 1;
                    done = true;
                }
                out.extend_from_slice(&self.readbuf[self.readpos..self.readpos + cpysz]);
                self.st.position += cpysz as i64;
                self.readpos += cpysz;
                maxlen = maxlen.saturating_sub(cpysz);
                if done {
                    break;
                }
            } else if self.st.eof {
                break;
            } else {
                let toread = if grow_mode {
                    self.chunk_size
                } else {
                    (maxlen - 1).min(self.chunk_size)
                };
                if !self.fill_read_buffer(vm, toread)? && self.st.fatal_error {
                    return Ok(None);
                }
                if self.writepos - self.readpos == 0 {
                    break;
                }
            }
        }
        if out.is_empty() {
            return Ok(None);
        }
        Ok(Some(out))
    }

    fn search_delim(&self, maxlen: usize, skiplen: usize, delim: &[u8]) -> Option<usize> {
        let seek_len = (self.writepos - self.readpos).min(maxlen);
        if seek_len <= skiplen {
            return None;
        }
        let hay = &self.readbuf[self.readpos + skiplen..self.readpos + seek_len];
        if delim.len() == 1 {
            hay.iter().position(|&c| c == delim[0]).map(|p| p + skiplen)
        } else {
            hay.windows(delim.len())
                .position(|w| w == delim)
                .map(|p| p + skiplen)
        }
    }

    /// `php_stream_get_record` (`stream_get_line`).
    pub fn get_record(
        &mut self,
        vm: &mut Vm,
        maxlen: usize,
        delim: &[u8],
    ) -> PResult<Option<Vec<u8>>> {
        if maxlen == 0 {
            return Ok(None);
        }
        let has_delim = !delim.is_empty();
        let mut found = if has_delim {
            self.search_delim(maxlen, 0, delim)
        } else {
            None
        };
        let mut buffered_len = self.buffered();
        while found.is_none() && buffered_len < maxlen {
            let to_read_now = (maxlen - buffered_len).min(self.chunk_size);
            if !self.fill_read_buffer(vm, buffered_len + to_read_now)? && self.st.fatal_error {
                return Ok(None);
            }
            let just_read = self.buffered() - buffered_len;
            if just_read == 0 {
                break;
            }
            if has_delim {
                let skip = buffered_len.saturating_sub(delim.len() - 1);
                found = self.search_delim(maxlen, skip, delim);
                if found.is_some() {
                    break;
                }
            }
            buffered_len += just_read;
        }
        let tent_ret_len = if let Some(f) = found.filter(|_| has_delim) {
            f
        } else if !has_delim && self.buffered() >= maxlen {
            maxlen
        } else if (self.buffered() < maxlen && !self.st.eof)
            || (self.buffered() == 0 && self.st.eof)
        {
            // not enough data yet (non-blocking), or nothing at all at EOF
            return Ok(None);
        } else {
            self.buffered().min(maxlen)
        };
        let mut ret = vec![0u8; tent_ret_len];
        let n = self.read(vm, &mut ret)?;
        ret.truncate(n.max(0) as usize);
        if found.is_some() {
            self.readpos += delim.len();
            self.st.position += delim.len() as i64;
        }
        Ok(Some(ret))
    }

    /// `php_stream_write_buffer`.
    fn write_buffer(&mut self, vm: &mut Vm, buf: &[u8]) -> PResult<isize> {
        if self.ops.can_seek()
            && self.st.flags & flags::NO_SEEK == 0
            && self.readpos != self.writepos
        {
            self.readpos = 0;
            self.writepos = 0;
            let pos = self.st.position;
            if let (0, Some(n)) = self.ops.seek(vm, &mut self.st, pos, 0)? {
                self.st.position = n;
            }
        }
        let chunk_size = if self.ops.kind() == StreamKind::Userspace {
            self.chunk_size
        } else {
            buf.len()
        };
        let mut didwrite: usize = 0;
        while didwrite < buf.len() {
            let end = (didwrite + chunk_size.max(1)).min(buf.len());
            let justwrote = self.ops.write(vm, &mut self.st, &buf[didwrite..end])?;
            if justwrote <= 0 {
                if didwrite == 0 {
                    return Ok(justwrote);
                }
                return Ok(didwrite as isize);
            }
            didwrite += justwrote as usize;
            self.st.position += justwrote as i64;
        }
        Ok(didwrite as isize)
    }

    /// `php_stream_write_filtered`: returns the bytes consumed by the first filter.
    fn write_filtered(&mut self, vm: &mut Vm, buf: Option<&[u8]>, flags: i32) -> PResult<isize> {
        let mut consumed: usize = 0;
        let mut brig_in = Brigade::new();
        let mut brig_out = Brigade::new();
        if let Some(b) = buf {
            brig_in.append(new_bucket(b.to_vec()));
        }
        let (res, persistent) = self.filter_ctx();
        let ctx = FilterCtx {
            stream_res: res.as_ref(),
            is_persistent: persistent,
        };
        let mut status = FilterStatus::ErrFatal;
        let (mut inp, mut out) = (&mut brig_in, &mut brig_out);
        let mut filters = std::mem::take(&mut self.writefilters);
        let mut result = Ok(());
        for (i, f) in filters.iter_mut().enumerate() {
            let c = if i == 0 { Some(&mut consumed) } else { None };
            match f.ops.filter(vm, &ctx, inp, out, c, flags) {
                Ok(s) => status = s,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
            if status != FilterStatus::PassOn {
                break;
            }
            std::mem::swap(&mut inp, &mut out);
            out.clear();
        }
        self.writefilters = filters;
        result?;
        let mut ret = consumed as isize;
        match status {
            FilterStatus::PassOn => {
                while let Some(b) = inp.pop_front() {
                    let data = std::mem::take(&mut b.borrow_mut().buf);
                    if self.write_buffer(vm, &data)? < 0 {
                        ret = -1;
                    }
                }
            }
            FilterStatus::ErrFatal => ret = -1,
            FilterStatus::FeedMe => {}
        }
        Ok(ret)
    }

    fn flush_ex(&mut self, vm: &mut Vm, closing: bool) -> PResult<i32> {
        if !self.writefilters.is_empty() && self.ops.can_write() {
            self.write_filtered(
                vm,
                None,
                if closing {
                    PSFS_FLAG_FLUSH_CLOSE
                } else {
                    PSFS_FLAG_FLUSH_INC
                },
            )?;
        }
        self.st.flags &= !flags::WAS_WRITTEN;
        self.ops.flush(vm, &mut self.st)
    }

    /// `php_stream_flush`: 0 on success.
    pub fn flush(&mut self, vm: &mut Vm) -> PResult<i32> {
        if self.closed {
            return Ok(-1);
        }
        self.flush_ex(vm, false)
    }

    /// `php_stream_write`: bytes written or `-1`.
    pub fn write(&mut self, vm: &mut Vm, buf: &[u8]) -> PResult<isize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.closed {
            return Ok(-1);
        }
        if !self.ops.can_write() {
            stream_notice(vm, &self.st, "Stream is not writable")?;
            return Ok(-1);
        }
        let bytes = if !self.writefilters.is_empty() {
            self.write_filtered(vm, Some(buf), PSFS_FLAG_NORMAL)?
        } else {
            self.write_buffer(vm, buf)?
        };
        if bytes != 0 {
            self.st.flags |= flags::WAS_WRITTEN;
        }
        Ok(bytes)
    }

    /// `php_stream_tell`.
    pub fn tell(&self) -> i64 {
        self.st.position
    }

    fn filters_seekable(&self, vm: &mut Vm, start: bool, read_chain: bool) -> PResult<bool> {
        let chain = if read_chain {
            &self.readfilters
        } else {
            &self.writefilters
        };
        for f in chain {
            let s = if read_chain {
                f.read_seekable
            } else {
                f.write_seekable
            };
            if s == Seekable::Never {
                vm.warning(format!("Stream filter {} is never seekable", f.label()))?;
                return Ok(false);
            }
            if !start && s == Seekable::Start {
                vm.warning(format!(
                    "Stream filter {} is seekable only to start position",
                    f.label()
                ))?;
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn filters_seek_all(
        &mut self,
        vm: &mut Vm,
        start: bool,
        offset: i64,
        whence: i32,
    ) -> PResult<bool> {
        let (res, persistent) = self.filter_ctx();
        let ctx = FilterCtx {
            stream_res: res.as_ref(),
            is_persistent: persistent,
        };
        for read_chain in [false, true] {
            let (chain, chain_type) = if read_chain {
                (&mut self.readfilters, super::filter::FILTER_READ)
            } else {
                (&mut self.writefilters, super::filter::FILTER_WRITE)
            };
            for f in chain.iter_mut() {
                let s = if read_chain {
                    f.read_seekable
                } else {
                    f.write_seekable
                };
                if ((s == Seekable::Start && start) || s == Seekable::Check)
                    && !f.ops.seek_in_chain(vm, &ctx, chain_type, offset, whence)?
                {
                    let l = f.label();
                    vm.warning(format!("Stream filter seeking for {l} failed"))?;
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// `php_stream_seek`: 0 on success, -1 on failure.
    pub fn seek(&mut self, vm: &mut Vm, offset: i64, whence: i32) -> PResult<i32> {
        if self.closed {
            return Ok(-1);
        }
        let mut offset = offset;
        let mut whence = whence;
        let is_start = whence == 0 && offset == 0;
        if !self.writefilters.is_empty() {
            self.flush(vm)?;
            if !self.filters_seekable(vm, is_start, false)? {
                return Ok(-1);
            }
        }
        if !self.readfilters.is_empty() && !self.filters_seekable(vm, is_start, true)? {
            return Ok(-1);
        }
        if self.st.flags & flags::NO_BUFFER == 0 {
            let buffered = (self.writepos - self.readpos) as i64;
            match whence {
                1 if offset > 0 && offset <= buffered => {
                    self.readpos += offset as usize;
                    self.st.position += offset;
                    self.st.eof = false;
                    self.st.fatal_error = false;
                    return Ok(if self.filters_seek_all(vm, is_start, offset, whence)? {
                        0
                    } else {
                        -1
                    });
                }
                0 if offset > self.st.position && offset <= self.st.position + buffered => {
                    self.readpos += (offset - self.st.position) as usize;
                    self.st.position = offset;
                    self.st.eof = false;
                    self.st.fatal_error = false;
                    return Ok(if self.filters_seek_all(vm, is_start, offset, whence)? {
                        0
                    } else {
                        -1
                    });
                }
                _ => {}
            }
        }
        if self.ops.can_seek() && self.st.flags & flags::NO_SEEK == 0 {
            match whence {
                1 => {
                    offset = if offset > i64::MAX - self.st.position {
                        i64::MAX
                    } else {
                        self.st.position + offset
                    };
                    whence = 0;
                }
                0 if offset < 0 => return Ok(-1),
                _ => {}
            }
            let (ret, newpos) = self.ops.seek(vm, &mut self.st, offset, whence)?;
            if let Some(p) = newpos {
                self.st.position = p;
            }
            if self.st.flags & flags::NO_SEEK == 0 || ret == 0 {
                if ret == 0 {
                    self.st.eof = false;
                    self.st.fatal_error = false;
                }
                self.readpos = 0;
                self.writepos = 0;
                return Ok(if self.filters_seek_all(vm, is_start, offset, whence)? {
                    ret
                } else {
                    -1
                });
            }
        }
        // emulate forward moving seeks with reads
        if whence == 1 && offset >= 0 {
            let mut tmp = [0u8; 1024];
            while offset > 0 {
                let n = (offset as usize).min(tmp.len());
                let didread = self.read(vm, &mut tmp[..n])?;
                if didread <= 0 {
                    return Ok(-1);
                }
                offset -= didread as i64;
            }
            self.st.eof = false;
            self.st.fatal_error = false;
            return Ok(0);
        }
        stream_warn(vm, &self.st, "Stream does not support seeking")?;
        Ok(-1)
    }

    /// `php_stream_set_option`.
    pub fn set_option(
        &mut self,
        vm: &mut Vm,
        option: i32,
        value: i32,
        param: &mut OptParam<'_>,
    ) -> PResult<i32> {
        if self.closed {
            return Ok(opt::RETURN_ERR);
        }
        let mut ret = self
            .ops
            .set_option(vm, &mut self.st, option, value, param)?;
        if ret == opt::RETURN_NOTIMPL {
            match option {
                opt::SET_CHUNK_SIZE => {
                    let old = self.chunk_size.min(i32::MAX as usize) as i32;
                    self.chunk_size = value.max(0) as usize;
                    return Ok(old);
                }
                opt::READ_BUFFER => {
                    if value == opt::BUFFER_NONE {
                        self.st.flags |= flags::NO_BUFFER;
                    } else if self.st.flags & flags::NO_BUFFER != 0 {
                        self.st.flags ^= flags::NO_BUFFER;
                    }
                    ret = opt::RETURN_OK;
                }
                _ => {}
            }
        }
        Ok(ret)
    }

    pub fn sync(&mut self, vm: &mut Vm, data_only: bool) -> PResult<i32> {
        let op = if data_only {
            opt::SYNC_FDSYNC
        } else {
            opt::SYNC_FSYNC
        };
        self.set_option(vm, opt::SYNC_API, op, &mut OptParam::None)
    }

    pub fn sync_supported(&mut self, vm: &mut Vm) -> PResult<bool> {
        Ok(
            self.set_option(vm, opt::SYNC_API, opt::SYNC_SUPPORTED, &mut OptParam::None)?
                == opt::RETURN_OK,
        )
    }

    pub fn truncate_supported(&mut self, vm: &mut Vm) -> PResult<bool> {
        Ok(self.set_option(
            vm,
            opt::TRUNCATE_API,
            opt::TRUNCATE_SUPPORTED,
            &mut OptParam::None,
        )? == opt::RETURN_OK)
    }

    pub fn truncate_set_size(&mut self, vm: &mut Vm, size: i64) -> PResult<i32> {
        self.set_option(
            vm,
            opt::TRUNCATE_API,
            opt::TRUNCATE_SET_SIZE,
            &mut OptParam::Size(size),
        )
    }

    /// `php_stream_supports_lock`.
    pub fn supports_lock(&mut self, vm: &mut Vm) -> PResult<bool> {
        Ok(self.set_option(
            vm,
            opt::LOCKING,
            0,
            &mut OptParam::Size(opt::LOCK_SUPPORTED as i64),
        )? == 0)
    }

    /// `php_stream_lock`: 0 on success.
    pub fn lock(&mut self, vm: &mut Vm, mode: i32) -> PResult<i32> {
        self.set_option(vm, opt::LOCKING, mode, &mut OptParam::None)
    }

    /// `php_stream_passthru`: copies the rest of the stream to the output.
    pub fn passthru(&mut self, vm: &mut Vm) -> PResult<isize> {
        let mut buf = vec![0u8; 8192];
        let mut bcount: usize = 0;
        loop {
            let b = self.read(vm, &mut buf)?;
            if b <= 0 {
                if b < 0 && bcount == 0 {
                    return Ok(b);
                }
                break;
            }
            vm.echo(&buf[..b as usize])?;
            bcount += b as usize;
        }
        Ok(bcount as isize)
    }

    /// `php_stream_copy_to_mem`; `None` when nothing was read.
    pub fn copy_to_mem(&mut self, vm: &mut Vm, maxlen: usize) -> PResult<Option<Vec<u8>>> {
        if maxlen == 0 {
            return Ok(Some(Vec::new()));
        }
        let maxlen = if maxlen == COPY_ALL { 0 } else { maxlen };
        let mut out = Vec::new();
        if maxlen > 0 && maxlen < 4 * CHUNK_SIZE {
            out.resize(maxlen, 0);
            let mut len = 0;
            while len < maxlen && !self.eof(vm)? {
                let r = self.read(vm, &mut out[len..])?;
                if r <= 0 {
                    break;
                }
                len += r as usize;
            }
            if len == 0 {
                return Ok(None);
            }
            out.truncate(len);
            return Ok(Some(out));
        }
        // Size the buffer from fstat like php-src (the read sizes show up in
        // "Read of N bytes failed" notices).
        let mut buflen = match self.stat(vm)? {
            Some(sb) if sb.size > 0 => {
                let n = (sb.size - self.st.position).max(0) as usize + CHUNK_SIZE;
                if maxlen > 0 && n > maxlen { maxlen } else { n }
            }
            _ => CHUNK_SIZE,
        };
        out.resize(buflen, 0);
        let min_room = CHUNK_SIZE / 4;
        let mut len = 0usize;
        loop {
            let r = self.read(vm, &mut out[len..buflen])?;
            if r <= 0 {
                break;
            }
            len += r as usize;
            if len + min_room >= buflen {
                if maxlen == len {
                    break;
                }
                buflen = if maxlen > 0 && buflen + CHUNK_SIZE > maxlen {
                    maxlen
                } else {
                    buflen + CHUNK_SIZE
                };
                out.resize(buflen, 0);
            }
        }
        if len == 0 {
            return Ok(None);
        }
        out.truncate(len);
        Ok(Some(out))
    }

    /// Next directory entry (`php_stream_readdir`).
    pub fn readdir(&mut self, vm: &mut Vm) -> PResult<Option<Vec<u8>>> {
        if self.closed {
            return Ok(None);
        }
        let r = self.ops.readdir(vm, &mut self.st)?;
        if r.is_none() {
            self.st.eof = true;
        }
        Ok(r)
    }

    /// `php_stream_rewind` for directory streams (`rewinddir`).
    pub fn rewind(&mut self, vm: &mut Vm) -> PResult<i32> {
        self.seek(vm, 0, 0)
    }

    /// `php_stream_cast(..., PHP_STREAM_AS_FD*)`.
    pub fn cast_fd(&mut self, as_: CastAs) -> Option<RawFd> {
        self.ops.cast_fd(as_)
    }

    /// Appends (or prepends) a filter to the read or write chain
    /// (`php_stream_filter_append_ex` / `prepend_ex`), winding buffered data
    /// through a newly appended read filter.
    pub fn attach_filter(
        &mut self,
        vm: &mut Vm,
        filter: Filter,
        read_chain: bool,
        append: bool,
    ) -> PResult<bool> {
        if !read_chain {
            if append {
                self.writefilters.push(filter);
            } else {
                self.writefilters.insert(0, filter);
            }
            return Ok(true);
        }
        if !append {
            self.readfilters.insert(0, filter);
            return Ok(true);
        }
        self.readfilters.push(filter);
        if self.writepos > self.readpos {
            let mut inp = Brigade::new();
            let mut out = Brigade::new();
            inp.append(new_bucket(
                self.readbuf[self.readpos..self.writepos].to_vec(),
            ));
            let mut consumed = 0usize;
            let (res, persistent) = self.filter_ctx();
            let ctx = FilterCtx {
                stream_res: res.as_ref(),
                is_persistent: persistent,
            };
            let f = self.readfilters.last_mut().expect("just pushed");
            let mut status = f.ops.filter(
                vm,
                &ctx,
                &mut inp,
                &mut out,
                Some(&mut consumed),
                PSFS_FLAG_NORMAL,
            )?;
            if self.readpos + consumed > self.writepos {
                status = FilterStatus::ErrFatal;
            }
            match status {
                FilterStatus::ErrFatal => {
                    stream_warn(vm, &self.st, "Filter failed to process pre-buffered data")?;
                    self.readfilters.pop();
                    return Ok(false);
                }
                FilterStatus::FeedMe => {
                    self.readpos = 0;
                    self.writepos = 0;
                }
                FilterStatus::PassOn => {
                    self.readpos = 0;
                    self.writepos = 0;
                    while let Some(b) = out.pop_front() {
                        let data = std::mem::take(&mut b.borrow_mut().buf);
                        if self.readbuf.len() - self.writepos < data.len() {
                            let n = self.readbuf.len() + data.len();
                            self.readbuf.resize(n, 0);
                        }
                        self.readbuf[self.writepos..self.writepos + data.len()]
                            .copy_from_slice(&data);
                        self.writepos += data.len();
                    }
                }
            }
        }
        Ok(true)
    }

    /// `php_stream_filter_flush` for the filter at `index` of a chain.
    pub fn flush_filter(
        &mut self,
        vm: &mut Vm,
        read_chain: bool,
        index: usize,
        finish: bool,
    ) -> PResult<bool> {
        let mut flags = if finish {
            PSFS_FLAG_FLUSH_CLOSE
        } else {
            PSFS_FLAG_FLUSH_INC
        };
        let (res, persistent) = self.filter_ctx();
        let ctx = FilterCtx {
            stream_res: res.as_ref(),
            is_persistent: persistent,
        };
        let mut a = Brigade::new();
        let mut b = Brigade::new();
        let (mut inp, mut out) = (&mut a, &mut b);
        let mut chain = if read_chain {
            std::mem::take(&mut self.readfilters)
        } else {
            std::mem::take(&mut self.writefilters)
        };
        let mut result: PResult<Option<bool>> = Ok(None);
        for f in chain.iter_mut().skip(index) {
            match f.ops.filter(vm, &ctx, inp, out, None, flags) {
                Ok(FilterStatus::FeedMe) => {
                    result = Ok(Some(true));
                    break;
                }
                Ok(FilterStatus::ErrFatal) => {
                    result = Ok(Some(false));
                    break;
                }
                Ok(FilterStatus::PassOn) => {}
                Err(e) => {
                    result = Err(e);
                    break;
                }
            }
            std::mem::swap(&mut inp, &mut out);
            out.clear();
            flags = PSFS_FLAG_NORMAL;
        }
        if read_chain {
            self.readfilters = chain;
        } else {
            self.writefilters = chain;
        }
        if let Some(r) = result? {
            return Ok(r);
        }
        if inp.total_len() == 0 {
            return Ok(true);
        }
        if read_chain {
            if self.readpos > 0 {
                self.readbuf.copy_within(self.readpos..self.writepos, 0);
                self.writepos -= self.readpos;
                self.readpos = 0;
            }
            while let Some(bk) = inp.pop_front() {
                let data = std::mem::take(&mut bk.borrow_mut().buf);
                self.append_to_readbuf(&data);
            }
        } else {
            while let Some(bk) = inp.pop_front() {
                let data = std::mem::take(&mut bk.borrow_mut().buf);
                let n = self.ops.write(vm, &mut self.st, &data)?;
                if n > 0 {
                    self.st.position += n as i64;
                }
            }
        }
        Ok(true)
    }

    /// Removes the filter with `id` from whichever chain holds it
    /// (`php_stream_filter_remove`), optionally flushing it first.
    pub fn remove_filter(&mut self, vm: &mut Vm, id: u64, flush: bool) -> PResult<Option<bool>> {
        for read_chain in [true, false] {
            let chain = if read_chain {
                &self.readfilters
            } else {
                &self.writefilters
            };
            if let Some(idx) = chain.iter().position(|f| f.id == id) {
                let flushed = if flush {
                    self.flush_filter(vm, read_chain, idx, true)?
                } else {
                    true
                };
                let chain = if read_chain {
                    &mut self.readfilters
                } else {
                    &mut self.writefilters
                };
                if idx < chain.len() && chain[idx].id == id {
                    let mut f = chain.remove(idx);
                    if let Some(r) = f.res.take() {
                        r.close();
                    }
                    f.ops.dtor(vm)?;
                }
                return Ok(Some(flushed));
            }
        }
        Ok(None)
    }
}

/// `php_stream_copy_to_stream_ex`: copies up to `maxlen` bytes (`COPY_ALL`
/// for everything). Returns `(success, copied)`.
pub fn copy_to_stream(
    vm: &mut Vm,
    src: &mut Stream,
    dest: Option<&mut Stream>,
    maxlen: usize,
) -> PResult<(bool, usize)> {
    if maxlen == 0 {
        return Ok((true, 0));
    }
    let maxlen = if maxlen == COPY_ALL { 0 } else { maxlen };
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut haveread = 0usize;
    let mut dest = dest;
    loop {
        let mut readchunk = buf.len();
        if maxlen != 0 && maxlen - haveread < readchunk {
            readchunk = maxlen - haveread;
        }
        let didread = src.read(vm, &mut buf[..readchunk])?;
        if didread <= 0 {
            return Ok((didread >= 0, haveread));
        }
        let chunk = buf[..didread as usize].to_vec();
        haveread += didread as usize;
        let mut written = 0;
        while written < chunk.len() {
            let didwrite = match dest.as_deref_mut() {
                Some(d) => d.write(vm, &chunk[written..])?,
                None => src.write(vm, &chunk[written..])?,
            };
            if didwrite <= 0 {
                return Ok((false, haveread - (chunk.len() - written)));
            }
            written += didwrite as usize;
        }
        if maxlen != 0 && maxlen == haveread {
            break;
        }
    }
    Ok((true, haveread))
}
