//! ext/spl/spl_heap.c — `SplHeap`, `SplMinHeap`, `SplMaxHeap`,
//! `SplPriorityQueue`.
//!
//! The binary heap ([`HeapState`]) reproduces php-src's sifting exactly
//! (the element order is visible through `__debugInfo()` and
//! `__serialize()`), including the intermediate states user code can
//! observe while a comparison runs: the heap is write-locked during
//! `insert()`/`extract()`/`next()` ("Heap cannot be changed when it is
//! already being modified."), and once a comparison throws, the remaining
//! comparisons of that operation return 0 and the heap is marked corrupted
//! ("Heap is corrupted, heap properties are no longer ensured.").
//! Comparisons go through a user `compare()` override when the class has
//! one (php-src's `fptr_cmp`), else through `zend_compare` (`Vm::compare`).
//!
//! # Interpreter contract
//! * `foreach` must use [`foreach_iterator`] (php-src's `get_iterator`):
//!   unlike the `current()` method, its `current` validates the heap
//!   (corrupted heaps throw) and `move_forward` extracts the top element.
//!   It returns `Ok(None)` when a user subclass overrides an `Iterator`
//!   method (iterate through the methods then) and throws "An iterator
//!   cannot be used with foreach by reference" for by-reference loops.
//! * `var_dump()` uses `__debugInfo()` (no `get_debug_info` handler).
//! * `count_elements` calls a user `count()` override and never returns
//!   `Ok(None)`; `clone_obj` copies the heap (flags included) and the
//!   properties, the interpreter then calls `__clone()`.

#[cfg(test)]
mod tests;

use std::cell::{RefCell, RefMut};
use std::rc::Rc;

use php_runtime::class::{ClassConstant, ClassEntry, class_flags};
use php_runtime::types::TypeDecl;
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, Registry, Value, Vm,
};

use super::engine::{self, is_overridden, set_private_debug_info_property};
use super::fixedarray::internal_iterator::{ObjectIterator, by_ref_error};
use super::fixedarray::support::{
    exception_set_previous, object_properties_load, release, zval_get_long,
};

pub const SPL_HEAP: &str = "SplHeap";
pub const SPL_MIN_HEAP: &str = "SplMinHeap";
pub const SPL_MAX_HEAP: &str = "SplMaxHeap";
pub const SPL_PRIORITY_QUEUE: &str = "SplPriorityQueue";

pub const EXTR_MASK: i64 = 3;
pub const EXTR_BOTH: i64 = 3;
pub const EXTR_DATA: i64 = 1;
pub const EXTR_PRIORITY: i64 = 2;

const HEAP_CORRUPTED: u32 = 1;
const HEAP_WRITE_LOCKED: u32 = 2;

/// An element: the value (and its priority for `SplPriorityQueue`).
#[derive(Clone)]
struct Elem {
    data: Value,
    priority: Value,
}

impl Elem {
    fn value(data: Value) -> Elem {
        Elem {
            data,
            priority: Value::Null,
        }
    }

    fn empty() -> Elem {
        Elem::value(Value::Null)
    }
}

/// Which comparison the heap uses (`heap->cmp`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// `spl_ptr_heap_zval_max_cmp` (SplHeap, SplMaxHeap).
    Max,
    /// `spl_ptr_heap_zval_min_cmp`.
    Min,
    /// `spl_ptr_pqueue_elem_cmp` and its specialisations.
    Pq,
}

/// The `SplPriorityQueue` comparator specialisation (only used without a
/// user `compare()`): every priority so far is an int / a float.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PqCmp {
    Generic,
    Long,
    Double,
}

/// `spl_heap_object` + `spl_ptr_heap`.
pub struct HeapState {
    /// `elements[..count]` is the heap; one extra slot may exist while an
    /// insertion sifts.
    elements: Vec<Elem>,
    count: usize,
    /// `heap->flags` (corrupted, write-locked).
    heap_flags: u32,
    /// `intern->flags`: the extraction flags of a priority queue.
    flags: i64,
    kind: Kind,
    pq_cmp: PqCmp,
    /// A user subclass overrides `compare()` / `count()`.
    fptr_cmp: bool,
    fptr_count: bool,
}

impl HeapState {
    fn put(&mut self, pos: usize, elem: Elem) {
        if self.elements.len() <= pos {
            self.elements.resize(pos + 1, Elem::empty());
        }
        self.elements[pos] = elem;
    }

    fn get(&self, pos: usize) -> Elem {
        self.elements.get(pos).cloned().unwrap_or_else(Elem::empty)
    }

    /// Drops the slots past `count`.
    fn trim(&mut self) -> Vec<Elem> {
        if self.elements.len() > self.count {
            self.elements.split_off(self.count)
        } else {
            Vec::new()
        }
    }
}

fn state(obj: &ObjRef) -> RefMut<'_, HeapState> {
    obj.native_mut::<HeapState>()
        .unwrap_or_else(|| panic!("{} without SplHeap state", obj.class().name))
}

/// The number of elements of a heap or priority queue.
pub fn count(obj: &ObjRef) -> usize {
    state(obj).count
}

/// Whether a heap is marked corrupted.
pub fn is_corrupted(obj: &ObjRef) -> bool {
    state(obj).heap_flags & HEAP_CORRUPTED != 0
}

/// The internal class whose behaviour `ce` inherits, and its comparator.
fn base_class(ce: &ClassRef) -> (&'static str, Kind) {
    let mut parent = Some(ce.clone());
    while let Some(p) = parent {
        match p.lc_name.as_bytes() {
            b"splpriorityqueue" => return (SPL_PRIORITY_QUEUE, Kind::Pq),
            b"splminheap" => return (SPL_MIN_HEAP, Kind::Min),
            b"splmaxheap" => return (SPL_MAX_HEAP, Kind::Max),
            b"splheap" => return (SPL_HEAP, Kind::Max),
            _ => parent = p.parent.clone(),
        }
    }
    unreachable!("{} is not a heap class", ce.name)
}

fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let (base, kind) = base_class(ce);
    let inherited = !ce.lc_name.eq_ignore_ascii_case(base.as_bytes());
    let obj = ObjRef::new(ce.clone());
    obj.set_native(HeapState {
        elements: Vec::new(),
        count: 0,
        heap_flags: 0,
        flags: if kind == Kind::Pq { EXTR_DATA } else { 0 },
        kind,
        pq_cmp: PqCmp::Generic,
        fptr_cmp: inherited && is_overridden(ce, b"compare", base),
        fptr_count: inherited && is_overridden(ce, b"count", base),
    });
    Ok(obj)
}

/// `spl_heap_object_clone`.
fn clone_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = vm.instantiate(&ce)?;
    {
        let src = state(old);
        let mut dst = state(&new);
        dst.elements = src.elements[..src.count.min(src.elements.len())].to_vec();
        dst.count = src.count;
        dst.heap_flags = src.heap_flags;
        dst.flags = src.flags;
        dst.kind = src.kind;
        dst.pq_cmp = src.pq_cmp;
        dst.fptr_cmp = src.fptr_cmp;
        dst.fptr_count = src.fptr_count;
    }
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    Ok(new)
}

/// `spl_heap_object_count_elements`.
fn count_elements(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<i64>> {
    let (fptr_count, count) = {
        let st = state(obj);
        (st.fptr_count, st.count)
    };
    if fptr_count {
        let rv = vm.call_method(obj, b"count", Vec::new())?;
        return Ok(Some(zval_get_long(vm, &rv)?));
    }
    Ok(Some(count as i64))
}

// ---------------------------------------------------------------------------
// Comparison and sifting
// ---------------------------------------------------------------------------

/// What the comparator needs, copied out of the state before an operation.
#[derive(Clone, Copy)]
struct CmpCtx {
    kind: Kind,
    pq_cmp: PqCmp,
    fptr_cmp: bool,
}

impl CmpCtx {
    fn of(st: &HeapState) -> CmpCtx {
        CmpCtx {
            kind: st.kind,
            pq_cmp: st.pq_cmp,
            fptr_cmp: st.fptr_cmp,
        }
    }
}

fn three_way<T: PartialOrd>(a: T, b: T) -> i64 {
    if a == b {
        0
    } else if a < b {
        -1
    } else {
        1
    }
}

/// `heap->cmp(x, y, object)`. Once an exception is pending (`pending`),
/// the generic comparators return 0 without calling anything.
fn heap_cmp(
    vm: &mut Vm,
    obj: &ObjRef,
    ctx: CmpCtx,
    x: &Elem,
    y: &Elem,
    pending: &mut Option<Flow>,
) -> i64 {
    if ctx.kind == Kind::Pq && !ctx.fptr_cmp {
        match (ctx.pq_cmp, &x.priority, &y.priority) {
            (PqCmp::Long, Value::Int(a), Value::Int(b)) => return three_way(a, b),
            (PqCmp::Double, Value::Float(a), Value::Float(b)) => return three_way(a, b),
            _ => {}
        }
    }
    if pending.is_some() {
        return 0;
    }
    let (a, b) = match ctx.kind {
        Kind::Pq => (&x.priority, &y.priority),
        _ => (&x.data, &y.data),
    };
    let r = if ctx.fptr_cmp {
        vm.call_method(obj, b"compare", vec![a.clone(), b.clone()])
            .and_then(|rv| zval_get_long(vm, &rv.into_unref()))
            .map(i64::signum)
    } else if ctx.kind == Kind::Min {
        vm.compare(b, a).map(i64::from)
    } else {
        vm.compare(a, b).map(i64::from)
    };
    match r {
        Ok(c) => c,
        Err(e) => {
            *pending = Some(e);
            0
        }
    }
}

/// Ends a write-locked operation: unlock, corrupt on a pending exception.
fn unlock(st: &mut HeapState, pending: &Option<Flow>) {
    st.heap_flags &= !HEAP_WRITE_LOCKED;
    if matches!(pending, Some(Flow::Exception(_))) {
        st.heap_flags |= HEAP_CORRUPTED;
    }
}

/// `spl_ptr_heap_insert`.
fn heap_insert(vm: &mut Vm, obj: &ObjRef, elem: Elem) -> PResult<()> {
    let (ctx, mut pos) = {
        let mut st = state(obj);
        let count = st.count;
        st.put(count, Elem::empty());
        st.heap_flags |= HEAP_WRITE_LOCKED;
        (CmpCtx::of(&st), count)
    };
    let mut pending = None;
    // Sifting up.
    while pos > 0 {
        let parent_idx = (pos - 1) / 2;
        let parent = state(obj).get(parent_idx);
        if heap_cmp(vm, obj, ctx, &parent, &elem, &mut pending) >= 0 {
            break;
        }
        state(obj).put(pos, parent);
        pos = parent_idx;
    }
    let extra = {
        let mut st = state(obj);
        st.count += 1;
        unlock(&mut st, &pending);
        st.put(pos, elem);
        st.trim()
    };
    drop(extra);
    pending.map_or(Ok(()), Err)
}

/// `spl_ptr_heap_delete_top`: `Ok(None)` for an empty heap. Without
/// `keep`, the top element is destroyed first (`next()`).
fn heap_delete_top(vm: &mut Vm, obj: &ObjRef, keep: bool) -> PResult<Option<Elem>> {
    let (ctx, top, bottom, count, limit) = {
        let mut st = state(obj);
        if st.count == 0 {
            return Ok(None);
        }
        let limit = (st.count - 1) / 2;
        st.heap_flags |= HEAP_WRITE_LOCKED;
        let top = if keep {
            st.get(0)
        } else {
            std::mem::replace(&mut st.elements[0], Elem::empty())
        };
        st.count -= 1;
        let count = st.count;
        (CmpCtx::of(&st), top, st.get(count), count, limit)
    };
    let mut pending = None;
    let top = if keep {
        Some(top)
    } else {
        // heap->dtor(): released before sifting.
        release(vm, [top.data, top.priority], &mut pending);
        None
    };
    let mut parent_idx = 0;
    while parent_idx < limit {
        // Find the larger child.
        let mut child_idx = parent_idx * 2 + 1;
        let (left, right) = {
            let st = state(obj);
            (st.get(child_idx), st.get(child_idx + 1))
        };
        let mut child = left;
        if child_idx != count && heap_cmp(vm, obj, ctx, &right, &child, &mut pending) > 0 {
            child_idx += 1;
            child = right;
        }
        // Swap elements between two levels.
        if heap_cmp(vm, obj, ctx, &bottom, &child, &mut pending) < 0 {
            state(obj).put(parent_idx, child);
        } else {
            break;
        }
        parent_idx = child_idx;
    }
    let extra = {
        let mut st = state(obj);
        unlock(&mut st, &pending);
        if parent_idx != count {
            st.put(parent_idx, bottom);
        }
        st.trim()
    };
    drop(extra);
    match pending {
        Some(e) => Err(e),
        None => Ok(top.or(Some(Elem::empty()))),
    }
}

/// `spl_heap_consistency_validations`.
fn validate(vm: &mut Vm, obj: &ObjRef, write: bool) -> PResult<()> {
    let flags = state(obj).heap_flags;
    if flags & HEAP_CORRUPTED != 0 {
        return Err(vm.throw_error(
            "RuntimeException",
            "Heap is corrupted, heap properties are no longer ensured.",
        ));
    }
    if write && flags & HEAP_WRITE_LOCKED != 0 {
        return Err(vm.throw_error(
            "RuntimeException",
            "Heap cannot be changed when it is already being modified.",
        ));
    }
    Ok(())
}

/// `spl_pqueue_extract_helper`.
fn extract_helper(elem: &Elem, flags: i64) -> Value {
    if flags & EXTR_BOTH == EXTR_BOTH {
        let mut a = PhpArray::new();
        a.insert_str("data", elem.data.unref());
        a.insert_str("priority", elem.priority.unref());
        return Value::array(a);
    }
    if flags & EXTR_DATA != 0 {
        return elem.data.unref();
    }
    elem.priority.unref()
}

// ---------------------------------------------------------------------------
// foreach (spl_heap_it / spl_pqueue_it)
// ---------------------------------------------------------------------------

/// The `foreach` iterator of a heap: every step extracts the top.
struct HeapIterator {
    obj: ObjRef,
    /// `zend_user_iterator.value` (priority queues cache the extracted form).
    value: RefCell<Option<Value>>,
}

impl ObjectIterator for HeapIterator {
    fn valid(&self, _vm: &mut Vm) -> PResult<bool> {
        Ok(count(&self.obj) != 0)
    }

    fn current(&self, vm: &mut Vm) -> PResult<Option<Value>> {
        validate(vm, &self.obj, false)?;
        let st = state(&self.obj);
        if st.count == 0 {
            return Ok(None);
        }
        if st.kind != Kind::Pq {
            return Ok(Some(st.get(0).data));
        }
        let mut cached = self.value.borrow_mut();
        if cached.is_none() {
            *cached = Some(extract_helper(&st.get(0), st.flags));
        }
        Ok(cached.clone())
    }

    fn key(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        Ok(Some(Value::Int(count(&self.obj) as i64 - 1)))
    }

    fn move_forward(&self, vm: &mut Vm) -> PResult<()> {
        validate(vm, &self.obj, true)?;
        let r = heap_delete_top(vm, &self.obj, false);
        self.value.borrow_mut().take();
        r.map(drop)
    }

    fn rewind(&self, _vm: &mut Vm) -> PResult<()> {
        // The iterator always points to the top element.
        Ok(())
    }
}

/// `spl_heap_get_iterator` / `spl_pqueue_get_iterator`.
pub fn get_iterator(vm: &mut Vm, obj: &ObjRef, by_ref: bool) -> PResult<Rc<dyn ObjectIterator>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    Ok(Rc::new(HeapIterator {
        obj: obj.clone(),
        value: RefCell::new(None),
    }))
}

/// The iterator `foreach` uses for a heap (`ce->get_iterator`); `Ok(None)`
/// when the class overrides an `Iterator` method (call the methods then).
pub fn foreach_iterator(
    vm: &mut Vm,
    obj: &ObjRef,
    by_ref: bool,
) -> PResult<Option<Rc<dyn ObjectIterator>>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    let ce = obj.class();
    let base = if state(obj).kind == Kind::Pq {
        SPL_PRIORITY_QUEUE
    } else {
        SPL_HEAP
    };
    let user = [&b"rewind"[..], b"valid", b"key", b"current", b"next"]
        .iter()
        .any(|m| is_overridden(ce, m, base));
    if user {
        return Ok(None);
    }
    get_iterator(vm, obj, false).map(Some)
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn this(args: &Args) -> ObjRef {
    engine::this(args)
}

fn runtime_exception(vm: &mut Vm, msg: &str) -> Flow {
    vm.throw_error("RuntimeException", msg)
}

fn heap_count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(count(&this(args)) as i64))
}

fn heap_is_empty(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(count(&this(args)) == 0))
}

fn heap_insert_method(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, true)?;
    heap_insert(vm, &this, Elem::value(args.get(0).unref()))?;
    Ok(Value::Bool(true))
}

fn heap_extract(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, true)?;
    match heap_delete_top(vm, &this, true)? {
        Some(e) => Ok(e.data.into_unref()),
        None => Err(runtime_exception(vm, "Can't extract from an empty heap")),
    }
}

fn pq_insert(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, true)?;
    let elem = Elem {
        data: args.get(0).unref(),
        priority: args.get(1).unref(),
    };
    {
        // Without a user compare(), specialise on the priority type.
        let mut st = state(&this);
        if !st.fptr_cmp {
            let new_cmp = match elem.priority {
                Value::Int(_) => PqCmp::Long,
                Value::Float(_) => PqCmp::Double,
                _ => PqCmp::Generic,
            };
            if st.count == 0 {
                st.pq_cmp = new_cmp;
            } else if new_cmp != st.pq_cmp {
                st.pq_cmp = PqCmp::Generic;
            }
        }
    }
    heap_insert(vm, &this, elem)?;
    Ok(Value::Bool(true))
}

fn pq_extract(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, true)?;
    match heap_delete_top(vm, &this, true)? {
        Some(e) => {
            let flags = state(&this).flags;
            Ok(extract_helper(&e, flags))
        }
        None => Err(runtime_exception(vm, "Can't extract from an empty heap")),
    }
}

fn pq_top(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, false)?;
    let top = {
        let st = state(&this);
        (st.count > 0).then(|| extract_helper(&st.get(0), st.flags))
    };
    top.ok_or_else(|| runtime_exception(vm, "Can't peek at an empty heap"))
}

fn pq_set_extract_flags(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let value = args.int(0) & EXTR_MASK;
    if value == 0 {
        return Err(runtime_exception(
            vm,
            "Must specify at least one extract flag",
        ));
    }
    state(&this(args)).flags = value;
    Ok(Value::Int(value))
}

fn pq_get_extract_flags(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(state(&this(args)).flags))
}

fn heap_recover(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    state(&this(args)).heap_flags &= !HEAP_CORRUPTED;
    Ok(Value::Bool(true))
}

fn heap_is_corrupted(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(is_corrupted(&this(args))))
}

/// `SplPriorityQueue::compare()` / `SplMaxHeap::compare()`.
fn max_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (a, b) = (args.get(0).unref(), args.get(1).unref());
    Ok(Value::Int(vm.compare(&a, &b)? as i64))
}

/// `SplMinHeap::compare()`.
fn min_compare(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (a, b) = (args.get(0).unref(), args.get(1).unref());
    Ok(Value::Int(vm.compare(&b, &a)? as i64))
}

fn heap_top(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, false)?;
    let top = {
        let st = state(&this);
        (st.count > 0).then(|| st.get(0).data.unref())
    };
    top.ok_or_else(|| runtime_exception(vm, "Can't peek at an empty heap"))
}

fn heap_key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(count(&this(args)) as i64 - 1))
}

fn heap_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, true)?;
    heap_delete_top(vm, &this, false)?;
    Ok(Value::Null)
}

fn heap_valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(count(&this(args)) != 0))
}

fn heap_rewind(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    // The iterator always points to the top element.
    Ok(Value::Null)
}

fn heap_current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let st = state(&this);
    Ok(if st.count == 0 {
        Value::Null
    } else {
        st.get(0).data.unref()
    })
}

fn pq_current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let st = state(&this);
    Ok(if st.count == 0 {
        Value::Null
    } else {
        extract_helper(&st.get(0), st.flags)
    })
}

/// `spl_heap_object_get_debug_info(ce, obj)`.
pub fn debug_info(obj: &ObjRef, class: &str) -> PhpArray {
    let mut table = engine::std_properties(obj);
    let (flags, corrupted, heap) = {
        let st = state(obj);
        let pq = class == SPL_PRIORITY_QUEUE;
        let heap: Vec<Value> = (0..st.count)
            .map(|i| {
                let e = st.get(i);
                if pq {
                    extract_helper(&e, EXTR_BOTH)
                } else {
                    e.data
                }
            })
            .collect();
        (st.flags, st.heap_flags & HEAP_CORRUPTED != 0, heap)
    };
    set_private_debug_info_property(&mut table, class, "flags", Value::Int(flags));
    set_private_debug_info_property(&mut table, class, "isCorrupted", Value::Bool(corrupted));
    set_private_debug_info_property(
        &mut table,
        class,
        "heap",
        Value::array(PhpArray::from_values(heap)),
    );
    table
}

fn heap_debug_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(debug_info(&this(args), SPL_HEAP)))
}

fn pq_debug_info(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(debug_info(&this(args), SPL_PRIORITY_QUEUE)))
}

/// `spl_heap_serialize_internal`.
fn serialize_internal(vm: &mut Vm, args: &Args, is_pqueue: bool) -> PResult<Value> {
    let this = this(args);
    validate(vm, &this, false)?;
    if state(&this).heap_flags & HEAP_WRITE_LOCKED != 0 {
        return Err(runtime_exception(
            vm,
            "Cannot serialize heap while it is being modified.",
        ));
    }
    let props = engine::std_properties(&this);
    let (flags, elements) = {
        let st = state(&this);
        let elements: Vec<Value> = (0..st.count)
            .map(|i| {
                let e = st.get(i);
                if is_pqueue {
                    let mut entry = PhpArray::new();
                    entry.insert_str("data", e.data);
                    entry.insert_str("priority", e.priority);
                    Value::array(entry)
                } else {
                    e.data
                }
            })
            .collect();
        (st.flags, elements)
    };
    let mut state_ = PhpArray::new();
    state_.insert_str("flags", Value::Int(flags));
    state_.insert_str(
        "heap_elements",
        Value::array(PhpArray::from_values(elements)),
    );
    Ok(Value::array(PhpArray::from_values(vec![
        Value::array(props),
        Value::array(state_),
    ])))
}

fn heap_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    serialize_internal(vm, args, false)
}

fn pq_serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    serialize_internal(vm, args, true)
}

/// `zend_throw_exception_ex(NULL, 0, "Invalid serialization data for %s object", ...)`
/// (chained to a pending exception).
fn invalid_data(vm: &mut Vm, obj: &ObjRef, previous: Option<Flow>) -> Flow {
    let e = vm.create_exception(
        "Exception",
        format!("Invalid serialization data for {} object", obj.class().name),
        0,
    );
    match previous {
        Some(Flow::Exception(p)) => exception_set_previous(&e, p),
        Some(other) => return other,
        None => {}
    }
    Flow::Exception(e)
}

/// `spl_heap_unserialize_internal_state`: `Ok(false)` for invalid data.
fn unserialize_state(vm: &mut Vm, obj: &ObjRef, st: &PhpArray, is_pqueue: bool) -> PResult<bool> {
    let Some(Value::Int(mut flags)) = st.get_str(b"flags").cloned() else {
        return Ok(false);
    };
    if is_pqueue {
        flags &= EXTR_MASK;
        if flags == 0 {
            return Ok(false);
        }
    } else if flags != 0 {
        // Regular heaps have no user-visible flags.
        return Ok(false);
    }
    state(obj).flags = flags;
    let Some(Value::Array(elements)) = st.get_str(b"heap_elements").cloned() else {
        return Ok(false);
    };
    for val in elements.values() {
        let elem = if is_pqueue {
            let Value::Array(entry) = val else {
                return Ok(false);
            };
            if entry.len() != 2 {
                return Ok(false);
            }
            match (entry.get_str(b"data"), entry.get_str(b"priority")) {
                (Some(d), Some(p)) => Elem {
                    data: d.clone(),
                    priority: p.clone(),
                },
                _ => return Ok(false),
            }
        } else {
            Elem::value(val.clone())
        };
        heap_insert(vm, obj, elem)?;
    }
    Ok(true)
}

fn unserialize_internal(vm: &mut Vm, args: &Args, is_pqueue: bool) -> PResult<Value> {
    let this = this(args);
    let data = args.array(0).clone();
    if !is_pqueue {
        validate(vm, &this, true)?;
    }
    if data.len() != 2 {
        return Err(invalid_data(vm, &this, None));
    }
    let Some(Value::Array(props)) = data.get_int(0).cloned() else {
        return Err(invalid_data(vm, &this, None));
    };
    let scope = if is_pqueue {
        SPL_PRIORITY_QUEUE
    } else {
        SPL_HEAP
    };
    if let Err(e) = object_properties_load(vm, &this, &props, scope) {
        return Err(invalid_data(vm, &this, Some(e)));
    }
    let Some(Value::Array(st)) = data.get_int(1).cloned() else {
        return Err(invalid_data(vm, &this, None));
    };
    match unserialize_state(vm, &this, &st, is_pqueue) {
        Ok(true) => {}
        Ok(false) => return Err(invalid_data(vm, &this, None)),
        Err(e) => return Err(invalid_data(vm, &this, Some(e))),
    }
    validate(vm, &this, false)?;
    Ok(Value::Null)
}

fn heap_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    unserialize_internal(vm, args, false)
}

fn pq_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    unserialize_internal(vm, args, true)
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

macro_rules! methods {
    ($($name:ident = $sig:literal => $handler:expr;)*) => {
        $(static $name: NativeFunction = NativeFunction { sig: $sig, handler: $handler };)*
    };
}

methods! {
    // SplPriorityQueue
    PQ_COMPARE = "public function compare(mixed $priority1, mixed $priority2): int {}" => max_compare;
    PQ_INSERT = "public function insert(mixed $value, mixed $priority): true {}" => pq_insert;
    PQ_SET_EXTRACT_FLAGS = "public function setExtractFlags(int $flags): int {}" => pq_set_extract_flags;
    PQ_TOP = "public function top(): mixed {}" => pq_top;
    PQ_EXTRACT = "public function extract(): mixed {}" => pq_extract;
    PQ_COUNT = "public function count(): int {}" => heap_count;
    PQ_IS_EMPTY = "public function isEmpty(): bool {}" => heap_is_empty;
    PQ_REWIND = "public function rewind(): void {}" => heap_rewind;
    PQ_CURRENT = "public function current(): mixed {}" => pq_current;
    PQ_KEY = "public function key(): int {}" => heap_key;
    PQ_NEXT = "public function next(): void {}" => heap_next;
    PQ_VALID = "public function valid(): bool {}" => heap_valid;
    PQ_RECOVER = "public function recoverFromCorruption(): true {}" => heap_recover;
    PQ_IS_CORRUPTED = "public function isCorrupted(): bool {}" => heap_is_corrupted;
    PQ_GET_EXTRACT_FLAGS = "public function getExtractFlags(): int {}" => pq_get_extract_flags;
    PQ_DEBUG_INFO = "public function __debugInfo(): array {}" => pq_debug_info;
    PQ_SERIALIZE = "public function __serialize(): array {}" => pq_serialize;
    PQ_UNSERIALIZE = "public function __unserialize(array $data): void {}" => pq_unserialize;
    // SplHeap
    HEAP_EXTRACT = "public function extract(): mixed {}" => heap_extract;
    HEAP_INSERT = "public function insert(mixed $value): true {}" => heap_insert_method;
    HEAP_TOP = "public function top(): mixed {}" => heap_top;
    HEAP_COUNT = "public function count(): int {}" => heap_count;
    HEAP_IS_EMPTY = "public function isEmpty(): bool {}" => heap_is_empty;
    HEAP_REWIND = "public function rewind(): void {}" => heap_rewind;
    HEAP_CURRENT = "public function current(): mixed {}" => heap_current;
    HEAP_KEY = "public function key(): int {}" => heap_key;
    HEAP_NEXT = "public function next(): void {}" => heap_next;
    HEAP_VALID = "public function valid(): bool {}" => heap_valid;
    HEAP_RECOVER = "public function recoverFromCorruption(): true {}" => heap_recover;
    HEAP_COMPARE = "abstract protected function compare(mixed $value1, mixed $value2): int;" => engine::abstract_method;
    HEAP_IS_CORRUPTED = "public function isCorrupted(): bool {}" => heap_is_corrupted;
    HEAP_DEBUG_INFO = "public function __debugInfo(): array {}" => heap_debug_info;
    HEAP_SERIALIZE = "public function __serialize(): array {}" => heap_serialize;
    HEAP_UNSERIALIZE = "public function __unserialize(array $data): void {}" => heap_unserialize;
    // SplMinHeap / SplMaxHeap
    MIN_COMPARE = "protected function compare(mixed $value1, mixed $value2): int {}" => min_compare;
    MAX_COMPARE = "protected function compare(mixed $value1, mixed $value2): int {}" => max_compare;
}

fn set_handlers(ce: &mut ClassEntry) {
    let h = &mut ce.handlers;
    h.create_object = Some(create_object);
    h.clone_obj = Some(clone_object);
    h.count_elements = Some(count_elements);
}

/// `PHP_MINIT_FUNCTION(spl_heap)`.
pub fn register(r: &mut Registry) {
    let iterator = engine::class(r, "Iterator");
    let countable = engine::class(r, "Countable");

    let mut heap = ClassEntry::new(SPL_HEAP, class_flags::ABSTRACT);
    engine::implement(&mut heap, &iterator);
    engine::implement(&mut heap, &countable);
    engine::add_methods(
        &mut heap,
        &[
            &HEAP_EXTRACT,
            &HEAP_INSERT,
            &HEAP_TOP,
            &HEAP_COUNT,
            &HEAP_IS_EMPTY,
            &HEAP_REWIND,
            &HEAP_CURRENT,
            &HEAP_KEY,
            &HEAP_NEXT,
            &HEAP_VALID,
            &HEAP_RECOVER,
            &HEAP_COMPARE,
            &HEAP_IS_CORRUPTED,
            &HEAP_DEBUG_INFO,
            &HEAP_SERIALIZE,
            &HEAP_UNSERIALIZE,
        ],
    );
    set_handlers(&mut heap);
    let heap = r.class(heap);

    let mut min = ClassEntry::new_subclass(SPL_MIN_HEAP, 0, &heap);
    engine::add_methods(&mut min, &[&MIN_COMPARE]);
    r.class(min);
    let mut max = ClassEntry::new_subclass(SPL_MAX_HEAP, 0, &heap);
    engine::add_methods(&mut max, &[&MAX_COMPARE]);
    r.class(max);

    let mut pq = ClassEntry::new(SPL_PRIORITY_QUEUE, 0);
    engine::implement(&mut pq, &iterator);
    engine::implement(&mut pq, &countable);
    for (name, value) in [
        ("EXTR_BOTH", EXTR_BOTH),
        ("EXTR_PRIORITY", EXTR_PRIORITY),
        ("EXTR_DATA", EXTR_DATA),
    ] {
        let mut c = ClassConstant::new(name, Value::Int(value), SPL_PRIORITY_QUEUE);
        c.ty = TypeDecl::parse("int").ok();
        pq.add_constant(c);
    }
    engine::add_methods(
        &mut pq,
        &[
            &PQ_COMPARE,
            &PQ_INSERT,
            &PQ_SET_EXTRACT_FLAGS,
            &PQ_TOP,
            &PQ_EXTRACT,
            &PQ_COUNT,
            &PQ_IS_EMPTY,
            &PQ_REWIND,
            &PQ_CURRENT,
            &PQ_KEY,
            &PQ_NEXT,
            &PQ_VALID,
            &PQ_RECOVER,
            &PQ_IS_CORRUPTED,
            &PQ_GET_EXTRACT_FLAGS,
            &PQ_DEBUG_INFO,
            &PQ_SERIALIZE,
            &PQ_UNSERIALIZE,
        ],
    );
    set_handlers(&mut pq);
    r.class(pq);
}
