//! main/streams/filter.c — buckets, brigades, filter chains and the filter
//! factory registry.
//!
//! A filter is a [`FilterOps`] implementation wrapped in a [`Filter`]
//! (seekability, the "stream filter" resource handed out by
//! `stream_filter_append`). Factories ([`FilterFactory`]) are looked up by
//! name with php-src's wildcard fallback (`convert.foo.bar` → `convert.foo.*`
//! → `convert.*`).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use php_runtime::{PResult, PhpStr, ResRef, Value, Vm};

use super::stream::Stream;

pub const FILTER_READ: i64 = 1;
pub const FILTER_WRITE: i64 = 2;
pub const FILTER_ALL: i64 = 3;

pub const PSFS_FLAG_NORMAL: i32 = 0;
pub const PSFS_FLAG_FLUSH_INC: i32 = 1;
pub const PSFS_FLAG_FLUSH_CLOSE: i32 = 2;

/// `php_stream_filter_status_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterStatus {
    ErrFatal = 0,
    FeedMe = 1,
    PassOn = 2,
}

/// `php_stream_filter_seekable_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seekable {
    Never,
    Start,
    Check,
    Always,
}

/// A bucket (`php_stream_bucket`); shared so that userspace filters can hold
/// it through a `StreamBucket` object.
#[derive(Debug, Default)]
pub struct Bucket {
    pub buf: Vec<u8>,
    pub is_persistent: bool,
}

pub type BucketRef = Rc<RefCell<Bucket>>;

pub fn new_bucket(data: Vec<u8>) -> BucketRef {
    Rc::new(RefCell::new(Bucket {
        buf: data,
        is_persistent: false,
    }))
}

/// `php_stream_bucket_make_writeable` for a bucket already unlinked from its
/// brigade: the bucket itself when nothing else holds it, else a copy.
pub fn bucket_make_writeable(b: BucketRef) -> BucketRef {
    if Rc::strong_count(&b) == 1 {
        return b;
    }
    let (buf, is_persistent) = {
        let bk = b.borrow();
        (bk.buf.clone(), bk.is_persistent)
    };
    Rc::new(RefCell::new(Bucket { buf, is_persistent }))
}

/// `php_stream_bucket_brigade`.
#[derive(Default, Debug)]
pub struct Brigade {
    pub buckets: VecDeque<BucketRef>,
}

impl Brigade {
    pub fn new() -> Brigade {
        Brigade::default()
    }

    /// `php_stream_bucket_append` (appending the current tail again is a no-op,
    /// the workaround php-src keeps for bug #35916).
    pub fn append(&mut self, b: BucketRef) {
        if let Some(t) = self.buckets.back() {
            if Rc::ptr_eq(t, &b) {
                return;
            }
        }
        self.buckets.push_back(b);
    }

    pub fn prepend(&mut self, b: BucketRef) {
        self.buckets.push_front(b);
    }

    pub fn pop_front(&mut self) -> Option<BucketRef> {
        self.buckets.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
    }

    /// Removes `b` wherever it is (`php_stream_bucket_unlink`).
    pub fn unlink(&mut self, b: &BucketRef) {
        if let Some(i) = self.buckets.iter().position(|x| Rc::ptr_eq(x, b)) {
            self.buckets.remove(i);
        }
    }

    pub fn total_len(&self) -> usize {
        self.buckets.iter().map(|b| b.borrow().buf.len()).sum()
    }
}

/// What a filter sees of the stream it is attached to.
pub struct FilterCtx<'a> {
    /// The stream's resource (userspace filters expose it as `$this->stream`).
    pub stream_res: Option<&'a ResRef>,
    pub is_persistent: bool,
}

/// `php_stream_filter_ops`.
pub trait FilterOps {
    /// `fops->label` (used in warnings and `stream_get_meta_data`).
    fn label(&self) -> PhpStr;

    fn filter(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        inp: &mut Brigade,
        out: &mut Brigade,
        consumed: Option<&mut usize>,
        flags: i32,
    ) -> PResult<FilterStatus>;

    /// `fops->seek`: `Ok(false)` is FAILURE.
    fn seek(&mut self, vm: &mut Vm, offset: i64, whence: i32) -> PResult<bool> {
        let _ = (vm, offset, whence);
        Ok(true)
    }

    /// `fops->seek` with what php-src passes alongside: the stream and the
    /// chain the filter sits on (`php_stream_filter_get_chain_type`:
    /// [`FILTER_READ`] or [`FILTER_WRITE`]). Userspace filters need both;
    /// the default ignores them and calls [`FilterOps::seek`].
    fn seek_in_chain(
        &mut self,
        vm: &mut Vm,
        ctx: &FilterCtx<'_>,
        chain: i64,
        offset: i64,
        whence: i32,
    ) -> PResult<bool> {
        let _ = (ctx, chain);
        self.seek(vm, offset, whence)
    }

    /// `fops->dtor`.
    fn dtor(&mut self, vm: &mut Vm) -> PResult<()> {
        let _ = vm;
        Ok(())
    }

    /// Whether destroying the filter must run PHP code (userspace filters).
    fn needs_vm(&self) -> bool {
        false
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
}

/// `php_stream_filter`.
pub struct Filter {
    pub ops: Box<dyn FilterOps>,
    pub read_seekable: Seekable,
    pub write_seekable: Seekable,
    pub is_persistent: bool,
    /// The "stream filter" resource, once handed out to userland.
    pub res: Option<ResRef>,
    pub id: u64,
}

thread_local! {
    static NEXT_FILTER_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

impl Filter {
    /// `php_stream_filter_alloc`.
    pub fn new(
        ops: Box<dyn FilterOps>,
        read_seekable: Seekable,
        write_seekable: Seekable,
    ) -> Filter {
        let id = NEXT_FILTER_ID.with(|c| {
            let v = c.get();
            c.set(v + 1);
            v
        });
        Filter {
            ops,
            read_seekable,
            write_seekable,
            is_persistent: false,
            res: None,
            id,
        }
    }

    pub fn label(&self) -> PhpStr {
        self.ops.label()
    }
}

/// Payload of a "stream filter" resource: which stream and filter it names.
pub struct FilterResource {
    pub stream: Weak<RefCell<Stream>>,
    pub filter_id: u64,
}

/// Creates a filter by name (`create_filter` of a factory).
pub type FilterFactory = fn(&mut Vm, &[u8], &Value, bool) -> PResult<Option<Filter>>;

/// Registered factories (global ones from module startup plus the volatile
/// ones from `stream_filter_register`).
#[derive(Default)]
pub struct FilterRegistry {
    pub factories: Vec<(PhpStr, FilterFactory)>,
    pub user_filters: Vec<(PhpStr, PhpStr)>,
}

impl FilterRegistry {
    pub fn find(&self, name: &[u8]) -> Option<FilterFactory> {
        self.factories
            .iter()
            .find(|(n, _)| n.as_bytes() == name)
            .map(|(_, f)| *f)
    }

    pub fn register(&mut self, name: &[u8], f: FilterFactory) -> bool {
        if self.find(name).is_some() {
            return false;
        }
        self.factories.push((PhpStr::from_bytes(name), f));
        true
    }
}

pub fn registry(vm: &mut Vm) -> &mut FilterRegistry {
    let st = vm.ext_data::<FilterRegistryCell>();
    if !st.init {
        st.init = true;
        super::std_filters::register_factories(&mut st.reg);
    }
    &mut st.reg
}

#[derive(Default)]
pub struct FilterRegistryCell {
    init: bool,
    pub reg: FilterRegistry,
}

/// `php_stream_filter_create`: exact name, then `a.b.*`, `a.*`; warns
/// "Unable to locate filter" / "Unable to create or locate filter".
pub fn create_filter(
    vm: &mut Vm,
    name: &[u8],
    params: &Value,
    persistent: bool,
) -> PResult<Option<Filter>> {
    let mut factory_found = false;
    let mut filter = None;
    if let Some(f) = registry(vm).find(name) {
        factory_found = true;
        filter = f(vm, name, params, persistent)?;
    } else if let Some(period) = name.iter().rposition(|&c| c == b'.') {
        let mut wild: Vec<u8> = name[..period].to_vec();
        loop {
            let mut pat = wild.clone();
            pat.extend_from_slice(b".*");
            if let Some(f) = registry(vm).find(&pat) {
                factory_found = true;
                filter = f(vm, name, params, persistent)?;
                if filter.is_some() {
                    break;
                }
            }
            match wild.iter().rposition(|&c| c == b'.') {
                Some(p) => wild.truncate(p),
                None => break,
            }
        }
    }
    if filter.is_none() {
        let n = String::from_utf8_lossy(name);
        if !factory_found {
            vm.warning(format!("Unable to locate filter \"{n}\""))?;
        } else {
            vm.warning(format!("Unable to create or locate filter \"{n}\""))?;
        }
    }
    Ok(filter)
}

/// `php_stream_filter_parse_write_seek_mode`.
pub fn parse_write_seek_mode(vm: &mut Vm, params: &Value) -> PResult<Option<Seekable>> {
    let Value::Array(a) = &*params.deref() else {
        return Ok(Some(Seekable::Always));
    };
    let Some(v) = a.get_str(b"write_seek_mode").cloned() else {
        return Ok(Some(Seekable::Always));
    };
    let s = vm.to_php_string(&v)?;
    Ok(match s.as_bytes() {
        b"preserve" => Some(Seekable::Always),
        b"reset" => Some(Seekable::Start),
        b"strict" => Some(Seekable::Never),
        _ => {
            vm.warning(
                "\"write_seek_mode\" filter parameter must be one of \"preserve\", \"reset\", or \"strict\"",
            )?;
            None
        }
    })
}
