//! Heap accounting for `memory_limit` (zend_alloc's `heap->limit`).
//!
//! [`TrackingAllocator`] counts live heap bytes. When the heap grows past the
//! soft limit (the `memory_limit` ini value) the request is recorded and the
//! interpreter raises PHP's "Allowed memory size of %d bytes exhausted" fatal
//! error at the next statement boundary ([`take_exhausted`]), so the error is
//! reported with the script's file and line like `zend_mm_safe_error`.
//!
//! A request that would grow the heap past the hard ceiling is refused
//! outright: the process prints the same fatal error and exits, so a runaway
//! script can never exhaust the machine's memory. No script may use more than
//! [`MAX_HEAP`], even with `memory_limit=-1` (php-src has no ceiling there;
//! this is a deliberate safety deviation).
//!
//! The binary installs the allocator (`#[global_allocator]`); without it the
//! counters stay at zero and no limit is ever reached.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// Counting allocator over [`System`].
pub struct TrackingAllocator;

/// Live heap bytes.
static USED: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of [`USED`].
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Soft limit: `min(memory_limit, MAX_HEAP)` in bytes.
static SOFT: AtomicUsize = AtomicUsize::new(MAX_HEAP);
/// Hard ceiling: [`SOFT`] plus [`HARD_SLACK`], fixed until the next
/// [`set_limit`].
static HARD: AtomicUsize = AtomicUsize::new(MAX_HEAP + HARD_SLACK);
/// The limit reported in the error message.
static REPORTED: AtomicUsize = AtomicUsize::new(MAX_HEAP);
/// Size of the request that crossed [`SOFT`] (0: none pending).
static EXHAUSTED: AtomicUsize = AtomicUsize::new(0);

/// Headroom above the soft limit while the interpreter unwinds to report the
/// error; past it the process stops immediately.
pub const HARD_SLACK: usize = 256 << 20;
/// Largest heap any script may use, whatever `memory_limit` says
/// (`memory_limit=-1` included).
pub const MAX_HEAP: usize = 4 << 30;

/// Accounts for `size` more bytes; diverges past the hard ceiling.
fn reserve(size: usize) {
    let used = USED.fetch_add(size, Relaxed).saturating_add(size);
    if used > SOFT.load(Relaxed) {
        if used > HARD.load(Relaxed) {
            USED.fetch_sub(size, Relaxed);
            out_of_memory(REPORTED.load(Relaxed), size);
        }
        let _ = EXHAUSTED.compare_exchange(0, size.max(1), Relaxed, Relaxed);
    }
    PEAK.fetch_max(used, Relaxed);
}

fn release(size: usize) {
    USED.fetch_sub(size, Relaxed);
}

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            release(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `System` with `layout`.
        unsafe { System.dealloc(ptr, layout) };
        release(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let old = layout.size();
        if new_size > old {
            reserve(new_size - old);
        }
        // SAFETY: `ptr` was allocated by `System` with `layout`.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            if new_size > old {
                release(new_size - old);
            }
        } else if new_size < old {
            release(old - new_size);
        }
        p
    }
}

/// Sets the limit from `memory_limit` (negative: unlimited, capped at
/// [`MAX_HEAP`]).
pub fn set_limit(bytes: i64) {
    let limit = usize::try_from(bytes).map_or(MAX_HEAP, |b| b.min(MAX_HEAP));
    REPORTED.store(limit, Relaxed);
    SOFT.store(limit, Relaxed);
    HARD.store(limit.saturating_add(HARD_SLACK), Relaxed);
}

/// The pending exhaustion as `(limit, requested size)`, if the heap crossed
/// the soft limit since the last call. Reporting it lifts the soft limit to
/// the hard ceiling for the rest of the process (shutdown functions and
/// destructors still run, like php-src after a memory error).
pub fn take_exhausted() -> Option<(usize, usize)> {
    if EXHAUSTED.load(Relaxed) == 0 {
        return None;
    }
    let size = EXHAUSTED.swap(0, Relaxed);
    if size == 0 {
        return None;
    }
    SOFT.store(HARD.load(Relaxed), Relaxed);
    Some((REPORTED.load(Relaxed), size))
}

/// Live heap bytes (`memory_get_usage(true)`-like).
pub fn used() -> usize {
    USED.load(Relaxed)
}

/// Peak heap bytes.
pub fn peak() -> usize {
    PEAK.load(Relaxed)
}

/// `memory_reset_peak_usage()`.
pub fn reset_peak() {
    PEAK.store(USED.load(Relaxed), Relaxed);
}

/// Reports the exhaustion and exits without allocating (we are inside the
/// allocator, so neither formatting nor the output layer may be used).
fn out_of_memory(limit: usize, size: usize) -> ! {
    let mut buf = [0u8; 160];
    let mut n = 0;
    let mut put = |bytes: &[u8]| {
        for &b in bytes {
            if n < buf.len() {
                buf[n] = b;
                n += 1;
            }
        }
    };
    put(b"\nFatal error: Allowed memory size of ");
    put(digits(limit, &mut [0u8; 20]));
    put(b" bytes exhausted (tried to allocate ");
    put(digits(size, &mut [0u8; 20]));
    put(b" bytes) in Unknown on line 0\n");
    sys::write_stdout(&buf[..n]);
    sys::exit(255)
}

/// Decimal digits of `v`, written into the tail of `buf`.
fn digits(mut v: usize, buf: &mut [u8; 20]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    &buf[i..]
}

#[cfg(unix)]
mod sys {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
        fn _exit(status: c_int) -> !;
    }

    pub fn write_stdout(bytes: &[u8]) {
        // SAFETY: `bytes` is a valid buffer of `bytes.len()` bytes; a short or
        // failed write only loses the message.
        unsafe {
            write(1, bytes.as_ptr().cast(), bytes.len());
        }
    }

    pub fn exit(status: i32) -> ! {
        // SAFETY: `_exit` terminates the process without running any Rust code.
        unsafe { _exit(status) }
    }
}

#[cfg(not(unix))]
mod sys {
    pub fn write_stdout(_bytes: &[u8]) {}

    pub fn exit(_status: i32) -> ! {
        std::process::abort()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_formats_decimal() {
        assert_eq!(digits(0, &mut [0u8; 20]), b"0");
        assert_eq!(digits(134217728, &mut [0u8; 20]), b"134217728");
        assert_eq!(digits(usize::MAX, &mut [0u8; 20]), b"18446744073709551615");
    }

    #[test]
    fn set_limit_caps_at_max_heap() {
        set_limit(128 << 20);
        assert_eq!(REPORTED.load(Relaxed), 128 << 20);
        assert_eq!(SOFT.load(Relaxed), 128 << 20);
        assert_eq!(HARD.load(Relaxed), (128 << 20) + HARD_SLACK);
        set_limit(-1);
        assert_eq!(SOFT.load(Relaxed), MAX_HEAP);
        set_limit(i64::MAX);
        assert_eq!(REPORTED.load(Relaxed), MAX_HEAP);
    }
}
