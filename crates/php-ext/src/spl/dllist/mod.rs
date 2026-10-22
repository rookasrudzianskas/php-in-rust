//! ext/spl/spl_dllist.c — `SplDoublyLinkedList`, `SplQueue`, `SplStack`.
//!
//! The list is made of reference-counted nodes ([`Node`]) like php-src's
//! `spl_ptr_llist_element`: the object's traverse pointer and every
//! `foreach` iterator keep the node they point at alive, so removing that
//! node from the list (`offsetUnset()`, `pop()`, `IT_MODE_DELETE`, a
//! destructor or `__serialize()` running meanwhile) leaves them on a
//! detached node (no neighbours, no data) exactly like the C code.
//! Released values run their destructors before the operation continues
//! (see [`support::release`]).
//!
//! # Interpreter contract
//! * `foreach` must use [`foreach_iterator`] (php-src's `get_iterator`
//!   handler): it returns an iterator independent of the object's own
//!   position with the iteration mode frozen at the start of the loop
//!   (nested loops over one list, `SplDoublyLinkedList_consistent_iterator_mode.phpt`),
//!   or `Ok(None)` when a user subclass overrides one of the `Iterator`
//!   methods (then iterate through the methods). It throws "An iterator
//!   cannot be used with foreach by reference" for `foreach (... as &$v)`.
//!   A `None` from [`ObjectIterator::current`] ends the loop.
//! * `$list[...]` uses the standard `ArrayAccess` dimension handlers
//!   (php-src installs none): `offsetGet()` / `offsetSet()` (with a `null`
//!   offset for `$list[] = $v`) / `offsetExists()` / `offsetUnset()` are
//!   called through the method table, so user overrides win.
//! * `var_dump()` uses `__debugInfo()` (no `get_debug_info` handler, so a
//!   subclass's `__debugInfo()` wins, bug69264.phpt).
//! * `count_elements` calls a user `count()` override; it never returns
//!   `Ok(None)`. `clone_obj` copies the list and the properties; the
//!   interpreter then calls `__clone()`.

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell, RefMut};
use std::rc::{Rc, Weak};

use php_runtime::class::{ClassConstant, ClassEntry};
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::proptable_to_symtable;
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};

use super::engine::{self, is_overridden, set_private_debug_info_property};
use super::fixedarray::internal_iterator::{ObjectIterator, by_ref_error};
use super::fixedarray::support::{
    exception_set_previous, finish, object_properties_load, parse_long_arg, release, release_value,
    zval_get_long,
};
use crate::standard::var_serialize::{
    var_serialize, var_serialize_destroy, var_serialize_init, var_unserialize,
    var_unserialize_destroy, var_unserialize_init,
};

pub const SPL_DOUBLY_LINKED_LIST: &str = "SplDoublyLinkedList";
pub const SPL_QUEUE: &str = "SplQueue";
pub const SPL_STACK: &str = "SplStack";

pub const IT_KEEP: i32 = 0;
pub const IT_FIFO: i32 = 0;
/// Delete the current element when moving to the next one.
pub const IT_DELETE: i32 = 1;
/// Traverse from the tail.
pub const IT_LIFO: i32 = 2;
pub const IT_MASK: i32 = 3;
/// The LIFO/FIFO bit cannot be changed (`SplStack`, `SplQueue`).
pub const IT_FIX: i32 = 4;

// ---------------------------------------------------------------------------
// The list (spl_ptr_llist)
// ---------------------------------------------------------------------------

/// `spl_ptr_llist_element`. A node removed from the list keeps no
/// neighbours and no data (`Value::Undef`).
pub struct Node {
    data: RefCell<Value>,
    prev: RefCell<Weak<Node>>,
    next: RefCell<Option<Rc<Node>>>,
}

type NodeRef = Rc<Node>;

impl Node {
    fn new(data: Value) -> NodeRef {
        Rc::new(Node {
            data: RefCell::new(data),
            prev: RefCell::new(Weak::new()),
            next: RefCell::new(None),
        })
    }

    fn prev(&self) -> Option<NodeRef> {
        self.prev.borrow().upgrade()
    }

    fn next(&self) -> Option<NodeRef> {
        self.next.borrow().clone()
    }

    fn data(&self) -> Value {
        self.data.borrow().clone()
    }
}

/// `spl_ptr_llist`.
#[derive(Default)]
struct LList {
    head: Option<NodeRef>,
    tail: Option<NodeRef>,
    count: i64,
}

impl LList {
    /// `spl_ptr_llist_push`.
    fn push(&mut self, data: Value) {
        let elem = Node::new(data);
        match &self.tail {
            Some(tail) => {
                *elem.prev.borrow_mut() = Rc::downgrade(tail);
                *tail.next.borrow_mut() = Some(elem.clone());
            }
            None => self.head = Some(elem.clone()),
        }
        self.tail = Some(elem);
        self.count += 1;
    }

    /// `spl_ptr_llist_unshift`.
    fn unshift(&mut self, data: Value) {
        let elem = Node::new(data);
        match self.head.take() {
            Some(head) => {
                *head.prev.borrow_mut() = Rc::downgrade(&elem);
                *elem.next.borrow_mut() = Some(head);
            }
            None => self.tail = Some(elem.clone()),
        }
        self.head = Some(elem);
        self.count += 1;
    }

    /// `spl_ptr_llist_pop`: `None` for an empty list.
    fn pop(&mut self) -> Option<Value> {
        let tail = self.tail.take()?;
        let prev = tail.prev();
        match &prev {
            Some(p) => *p.next.borrow_mut() = None,
            None => self.head = None,
        }
        self.tail = prev;
        self.count -= 1;
        *tail.prev.borrow_mut() = Weak::new();
        Some(tail.data.replace(Value::Undef))
    }

    /// `spl_ptr_llist_shift`: `None` for an empty list.
    fn shift(&mut self) -> Option<Value> {
        let head = self.head.take()?;
        let next = head.next.borrow_mut().take();
        match &next {
            Some(n) => *n.prev.borrow_mut() = Weak::new(),
            None => self.tail = None,
        }
        self.head = next;
        self.count -= 1;
        Some(head.data.replace(Value::Undef))
    }

    /// `spl_ptr_llist_offset`.
    fn offset(&self, offset: i64, backward: bool) -> Option<NodeRef> {
        let mut current = if backward {
            self.tail.clone()
        } else {
            self.head.clone()
        };
        let mut pos = 0;
        while let Some(c) = &current {
            if pos >= offset {
                break;
            }
            pos += 1;
            current = if backward { c.prev() } else { c.next() };
        }
        current
    }

    /// Unlinks `elem` (`offsetUnset()`), leaving it without neighbours.
    fn unlink(&mut self, elem: &NodeRef) {
        let prev = elem.prev();
        let next = elem.next.borrow_mut().take();
        if let Some(p) = &prev {
            *p.next.borrow_mut() = next.clone();
        }
        if let Some(n) = &next {
            *n.prev.borrow_mut() = prev.as_ref().map_or_else(Weak::new, Rc::downgrade);
        }
        if self.head.as_ref().is_some_and(|h| Rc::ptr_eq(h, elem)) {
            self.head = next;
        }
        if self.tail.as_ref().is_some_and(|t| Rc::ptr_eq(t, elem)) {
            self.tail = prev;
        }
        *elem.prev.borrow_mut() = Weak::new();
        self.count -= 1;
    }

    /// Inserts `data` before `elem` (`add()`).
    fn insert_before(&mut self, elem: &NodeRef, data: Value) {
        let new = Node::new(data);
        let prev = elem.prev();
        *new.next.borrow_mut() = Some(elem.clone());
        match &prev {
            None => self.head = Some(new.clone()),
            Some(p) => {
                *new.prev.borrow_mut() = Rc::downgrade(p);
                *p.next.borrow_mut() = Some(new.clone());
            }
        }
        *elem.prev.borrow_mut() = Rc::downgrade(&new);
        self.count += 1;
    }

    fn values(&self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.count.max(0) as usize);
        let mut current = self.head.clone();
        while let Some(c) = current {
            out.push(c.data());
            current = c.next();
        }
        out
    }

    /// Removes every element, last first (`spl_dllist_object_free_storage`).
    fn drain(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        while let Some(v) = self.pop() {
            out.push(v);
        }
        out
    }
}

impl Drop for LList {
    fn drop(&mut self) {
        // Unlink iteratively: dropping a long chain of `next` links
        // recursively would overflow the stack.
        let mut current = self.head.take();
        self.tail = None;
        while let Some(c) = current {
            current = c.next.borrow_mut().take();
        }
    }
}

// ---------------------------------------------------------------------------
// The object (spl_dllist_object)
// ---------------------------------------------------------------------------

/// `spl_dllist_object`.
pub struct DllistState {
    llist: LList,
    traverse_pointer: Option<NodeRef>,
    traverse_position: i64,
    flags: i32,
    /// A user subclass overrides `count()` (`fptr_count`).
    fptr_count: bool,
}

fn state(obj: &ObjRef) -> RefMut<'_, DllistState> {
    obj.native_mut::<DllistState>()
        .unwrap_or_else(|| panic!("{} without SplDoublyLinkedList state", obj.class().name))
}

/// The elements of a list from head to tail.
pub fn values(obj: &ObjRef) -> Vec<Value> {
    state(obj).llist.values()
}

/// The iterator flags of a list (`getIteratorMode()`).
pub fn flags(obj: &ObjRef) -> i64 {
    state(obj).flags as i64
}

/// `spl_dllist_it_helper_rewind`.
fn it_rewind(tp: &mut Option<NodeRef>, pos: &mut i64, llist: &LList, flags: i32) {
    if flags & IT_LIFO != 0 {
        *pos = llist.count - 1;
        *tp = llist.tail.clone();
    } else {
        *pos = 0;
        *tp = llist.head.clone();
    }
}

/// `spl_dllist_it_helper_move_forward`: returns the element removed in
/// `IT_MODE_DELETE` (to be released by the caller).
fn it_move_forward(
    tp: &mut Option<NodeRef>,
    pos: &mut i64,
    llist: &mut LList,
    flags: i32,
) -> Option<Value> {
    let old = tp.take()?;
    if flags & IT_LIFO != 0 {
        *tp = old.prev();
        *pos -= 1;
        if flags & IT_DELETE != 0 {
            return llist.pop();
        }
    } else {
        *tp = old.next();
        if flags & IT_DELETE != 0 {
            return llist.shift();
        }
        *pos += 1;
    }
    None
}

fn create_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(DllistState {
        llist: LList::default(),
        traverse_pointer: None,
        traverse_position: 0,
        flags: class_flags(ce),
        fptr_count: is_overridden(ce, b"count", SPL_DOUBLY_LINKED_LIST),
    });
    Ok(obj)
}

/// The flags `spl_dllist_object_new_ex` derives from the class hierarchy.
fn class_flags(ce: &ClassRef) -> i32 {
    let mut flags = 0;
    let mut parent = Some(ce.clone());
    while let Some(p) = parent {
        if p.lc_name.as_bytes() == b"splstack" {
            flags |= IT_FIX | IT_LIFO;
        } else if p.lc_name.as_bytes() == b"splqueue" {
            flags |= IT_FIX;
        }
        if p.lc_name.as_bytes() == b"spldoublylinkedlist" {
            break;
        }
        parent = p.parent.clone();
    }
    flags
}

/// `spl_dllist_object_clone`.
fn clone_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = vm.instantiate(&ce)?;
    let (values, flags) = {
        let st = state(old);
        (st.llist.values(), st.flags)
    };
    {
        let mut st = state(&new);
        for v in values {
            st.llist.push(v);
        }
        st.traverse_pointer = st.llist.head.clone();
        st.traverse_position = 0;
        st.flags = flags | class_flags(&ce);
    }
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    Ok(new)
}

/// `spl_dllist_object_count_elements`.
fn count_elements(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<i64>> {
    let (fptr_count, count) = {
        let st = state(obj);
        (st.fptr_count, st.llist.count)
    };
    if fptr_count {
        let rv = vm.call_method(obj, b"count", Vec::new())?;
        return Ok(Some(zval_get_long(vm, &rv)?));
    }
    Ok(Some(count))
}

/// `spl_dllist_object_get_debug_info` (`__debugInfo()`).
pub fn debug_info(obj: &ObjRef) -> PhpArray {
    let mut table = engine::std_properties(obj);
    let (flags, values) = {
        let st = state(obj);
        (st.flags, st.llist.values())
    };
    set_private_debug_info_property(
        &mut table,
        SPL_DOUBLY_LINKED_LIST,
        "flags",
        Value::Int(flags as i64),
    );
    set_private_debug_info_property(
        &mut table,
        SPL_DOUBLY_LINKED_LIST,
        "dllist",
        Value::array(PhpArray::from_values(values)),
    );
    table
}

// ---------------------------------------------------------------------------
// foreach (spl_dllist_it)
// ---------------------------------------------------------------------------

/// `spl_dllist_it`: a `foreach` iterator with its own position.
struct DllistIterator {
    obj: ObjRef,
    traverse_pointer: RefCell<Option<NodeRef>>,
    traverse_position: Cell<i64>,
    flags: i32,
}

impl ObjectIterator for DllistIterator {
    fn valid(&self, _vm: &mut Vm) -> PResult<bool> {
        Ok(self.traverse_pointer.borrow().is_some())
    }

    fn current(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        let data = self.traverse_pointer.borrow().as_ref().map(|n| n.data());
        Ok(data.filter(|d| !d.is_undef()))
    }

    fn key(&self, _vm: &mut Vm) -> PResult<Option<Value>> {
        Ok(Some(Value::Int(self.traverse_position.get())))
    }

    fn move_forward(&self, vm: &mut Vm) -> PResult<()> {
        let removed = {
            let mut st = state(&self.obj);
            let mut tp = self.traverse_pointer.borrow_mut();
            let mut pos = self.traverse_position.get();
            let removed = it_move_forward(&mut tp, &mut pos, &mut st.llist, self.flags);
            self.traverse_position.set(pos);
            removed
        };
        match removed {
            Some(v) => release_value(vm, v),
            None => Ok(()),
        }
    }

    fn rewind(&self, _vm: &mut Vm) -> PResult<()> {
        let st = state(&self.obj);
        let mut tp = self.traverse_pointer.borrow_mut();
        let mut pos = self.traverse_position.get();
        it_rewind(&mut tp, &mut pos, &st.llist, self.flags);
        self.traverse_position.set(pos);
        Ok(())
    }
}

/// `spl_dllist_get_iterator`.
pub fn get_iterator(vm: &mut Vm, obj: &ObjRef, by_ref: bool) -> PResult<Rc<dyn ObjectIterator>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    let st = state(obj);
    Ok(Rc::new(DllistIterator {
        obj: obj.clone(),
        traverse_pointer: RefCell::new(st.traverse_pointer.clone()),
        traverse_position: Cell::new(st.traverse_position),
        flags: st.flags & IT_MASK,
    }))
}

/// The iterator `foreach` uses for `obj` (`ce->get_iterator`): `Ok(None)`
/// when the class overrides an `Iterator` method (php-src then switches to
/// `zend_user_it_get_iterator`, i.e. the methods are called).
pub fn foreach_iterator(
    vm: &mut Vm,
    obj: &ObjRef,
    by_ref: bool,
) -> PResult<Option<Rc<dyn ObjectIterator>>> {
    if by_ref {
        return Err(by_ref_error(vm));
    }
    let ce = obj.class();
    let user = [&b"rewind"[..], b"valid", b"key", b"current", b"next"]
        .iter()
        .any(|m| is_overridden(ce, m, SPL_DOUBLY_LINKED_LIST));
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

/// `zend_argument_error(spl_ce_OutOfRangeException, 1, msg)`.
fn out_of_range(vm: &mut Vm, msg: &str) -> Flow {
    vm.argument_error("OutOfRangeException", 1, msg)
}

fn index_arg(vm: &mut Vm, args: &Args) -> PResult<i64> {
    let v = args.get(0).clone();
    Ok(parse_long_arg(vm, &v, 0, "index", false)?.unwrap_or(0))
}

fn push(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    state(&this(args)).llist.push(v);
    Ok(Value::Null)
}

fn unshift(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = args.get(0).unref();
    state(&this(args)).llist.unshift(v);
    Ok(Value::Null)
}

fn pop(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = state(&this(args)).llist.pop();
    match v {
        Some(v) => Ok(v.into_unref()),
        None => Err(runtime_exception(
            vm,
            "Can't pop from an empty datastructure",
        )),
    }
}

fn shift(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = state(&this(args)).llist.shift();
    match v {
        Some(v) => Ok(v.into_unref()),
        None => Err(runtime_exception(
            vm,
            "Can't shift from an empty datastructure",
        )),
    }
}

fn top(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = state(&this(args)).llist.tail.as_ref().map(|n| n.data());
    match v.filter(|v| !v.is_undef()) {
        Some(v) => Ok(v.unref()),
        None => Err(runtime_exception(
            vm,
            "Can't peek at an empty datastructure",
        )),
    }
}

fn bottom(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let v = state(&this(args)).llist.head.as_ref().map(|n| n.data());
    match v.filter(|v| !v.is_undef()) {
        Some(v) => Ok(v.unref()),
        None => Err(runtime_exception(
            vm,
            "Can't peek at an empty datastructure",
        )),
    }
}

fn count(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(state(&this(args)).llist.count))
}

fn is_empty(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let n = count_elements(vm, &this(args))?.unwrap_or(0);
    Ok(Value::Bool(n == 0))
}

fn set_iterator_mode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let value = args.int(0);
    let this = this(args);
    let flags = state(&this).flags;
    if flags & IT_FIX != 0 && (flags & IT_LIFO) as i64 != (value & IT_LIFO as i64) {
        return Err(runtime_exception(
            vm,
            "Iterators' LIFO/FIFO modes for SplStack/SplQueue objects are frozen",
        ));
    }
    let flags = (value & IT_MASK as i64) as i32 | (flags & IT_FIX);
    state(&this).flags = flags;
    Ok(Value::Int(flags as i64))
}

fn get_iterator_mode(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(state(&this(args)).flags as i64))
}

fn offset_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = index_arg(vm, args)?;
    let count = state(&this(args)).llist.count;
    Ok(Value::Bool(index >= 0 && index < count))
}

/// The element at `index` (counted from the tail in LIFO mode), with
/// php-src's range errors.
fn element_at(vm: &mut Vm, obj: &ObjRef, index: i64) -> PResult<NodeRef> {
    let found = {
        let st = state(obj);
        if index < 0 || index >= st.llist.count {
            None
        } else {
            Some(st.llist.offset(index, st.flags & IT_LIFO != 0))
        }
    };
    match found {
        None => Err(out_of_range(vm, "is out of range")),
        Some(None) => Err(out_of_range(vm, "is an invalid offset")),
        Some(Some(n)) => Ok(n),
    }
}

fn offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = index_arg(vm, args)?;
    let elem = element_at(vm, &this(args), index)?;
    Ok(elem.data().unref())
}

fn offset_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index_v = args.get(0).clone();
    let index = parse_long_arg(vm, &index_v, 0, "index", true)?;
    let value = args.get(1).unref();
    let this = this(args);
    match index {
        // $obj[] = ...
        None => state(&this).llist.push(value),
        Some(index) => {
            let elem = element_at(vm, &this, index)?;
            // The element is replaced; the old value is released last.
            let garbage = elem.data.replace(value);
            release_value(vm, garbage)?;
        }
    }
    Ok(Value::Null)
}

fn offset_unset(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = index_arg(vm, args)?;
    let this = this(args);
    let elem = element_at(vm, &this, index)?;
    let data = {
        let mut st = state(&this);
        st.llist.unlink(&elem);
        if st
            .traverse_pointer
            .as_ref()
            .is_some_and(|tp| Rc::ptr_eq(tp, &elem))
        {
            st.traverse_pointer = None;
        }
        elem.data.replace(Value::Undef)
    };
    drop(elem);
    release_value(vm, data)?;
    Ok(Value::Null)
}

fn key(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(state(&this(args)).traverse_position))
}

/// Moves the object's own traverse pointer (`next()`, `prev()`).
fn move_object(vm: &mut Vm, obj: &ObjRef, flip: bool) -> PResult<()> {
    let removed = {
        let mut st = state(obj);
        let flags = if flip { st.flags ^ IT_LIFO } else { st.flags };
        let DllistState {
            llist,
            traverse_pointer,
            traverse_position,
            ..
        } = &mut *st;
        it_move_forward(traverse_pointer, traverse_position, llist, flags)
    };
    match removed {
        Some(v) => release_value(vm, v),
        None => Ok(()),
    }
}

fn prev(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_object(vm, &this(args), true)?;
    Ok(Value::Null)
}

fn next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    move_object(vm, &this(args), false)?;
    Ok(Value::Null)
}

fn valid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(state(&this(args)).traverse_pointer.is_some()))
}

fn rewind(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let mut st = state(&this);
    let flags = st.flags;
    let DllistState {
        llist,
        traverse_pointer,
        traverse_position,
        ..
    } = &mut *st;
    it_rewind(traverse_pointer, traverse_position, llist, flags);
    Ok(Value::Null)
}

fn current(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = state(&this(args))
        .traverse_pointer
        .as_ref()
        .map(|n| n.data());
    Ok(match data {
        Some(d) if !d.is_undef() => d.unref(),
        _ => Value::Null,
    })
}

/// `SplDoublyLinkedList::serialize()`: `i:<flags>;` then `:<element>` for
/// each element, sharing back-references.
fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (flags, head) = {
        let st = state(&this);
        (st.flags, st.llist.head.clone())
    };
    let d = var_serialize_init(vm);
    let mut buf = Vec::new();
    let r = (|| -> PResult<()> {
        var_serialize(vm, &mut buf, &Value::Int(flags as i64), &d)?;
        let mut current = head;
        while let Some(cur) = current {
            buf.push(b':');
            let next = cur.next();
            let data = cur.data();
            var_serialize(vm, &mut buf, &data, &d)?;
            drop(data);
            // SPL_LLIST_CHECK_DELREF_EX(next, break;): stop when the next
            // element was removed meanwhile and nothing else holds it.
            if next.as_ref().is_some_and(|n| Rc::strong_count(n) == 1) {
                break;
            }
            current = next;
        }
        Ok(())
    })();
    var_serialize_destroy(vm, d);
    r?;
    Ok(Value::String(PhpStr::from_vec(buf)))
}

fn unserialize_error(vm: &mut Vm, pos: usize, len: usize, previous: Option<ObjRef>) -> Flow {
    let e = vm.create_exception(
        "UnexpectedValueException",
        format!("Error at offset {pos} of {len} bytes"),
        0,
    );
    if let Some(p) = previous {
        exception_set_previous(&e, p);
    }
    Flow::Exception(e)
}

/// `SplDoublyLinkedList::unserialize()`.
fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let buf = args.str(0).clone();
    if buf.is_empty() {
        return Ok(Value::Null);
    }
    let this = this(args);
    let old = state(&this).llist.drain();
    let mut pending = None;
    release(vm, old, &mut pending);
    finish(pending)?;

    let buf = buf.as_bytes();
    let d = var_unserialize_init(vm);
    let mut pos = 0usize;
    let parsed = (|| -> PResult<bool> {
        match var_unserialize(vm, &d, buf, &mut pos)?.map(|v| v.unref()) {
            Some(Value::Int(flags)) => state(&this).flags = flags as i32,
            _ => return Ok(false),
        }
        while buf.get(pos) == Some(&b':') {
            pos += 1;
            match var_unserialize(vm, &d, buf, &mut pos)? {
                Some(elem) => state(&this).llist.push(elem),
                None => return Ok(false),
            }
        }
        Ok(buf.get(pos).copied().unwrap_or(0) == 0)
    })();
    let destroyed = var_unserialize_destroy(vm, d);
    match parsed {
        Ok(true) => destroyed.map(|()| Value::Null),
        Ok(false) => {
            destroyed?;
            Err(unserialize_error(vm, pos, buf.len(), None))
        }
        Err(Flow::Exception(e)) => Err(unserialize_error(vm, pos, buf.len(), Some(e))),
        Err(other) => Err(other),
    }
}

fn magic_serialize(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let this = this(args);
    let (flags, values) = {
        let st = state(&this);
        (st.flags, st.llist.values())
    };
    let members = proptable_to_symtable(&engine::std_properties(&this));
    Ok(Value::array(PhpArray::from_values(vec![
        Value::Int(flags as i64),
        Value::array(PhpArray::from_values(values)),
        Value::array(members),
    ])))
}

fn magic_unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = args.array(0).clone();
    let this = this(args);
    let (flags, storage, members) = match (data.get_int(0), data.get_int(1), data.get_int(2)) {
        (Some(Value::Int(f)), Some(Value::Array(s)), Some(Value::Array(m))) => {
            (*f, s.clone(), m.clone())
        }
        _ => {
            return Err(vm.throw_error(
                "UnexpectedValueException",
                "Incomplete or ill-typed serialization data",
            ));
        }
    };
    {
        let mut st = state(&this);
        st.flags = flags as i32;
        for v in storage.values() {
            st.llist.push(v.clone());
        }
    }
    object_properties_load(vm, &this, &members, SPL_DOUBLY_LINKED_LIST)?;
    Ok(Value::Null)
}

fn add(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let index = args.int(0);
    let value = args.get(1).unref();
    let this = this(args);
    let mut st = state(&this);
    if index < 0 || index > st.llist.count {
        drop(st);
        return Err(out_of_range(vm, "is out of range"));
    }
    if index == st.llist.count {
        // Not inserting before any entry: push.
        st.llist.push(value);
    } else {
        let lifo = st.flags & IT_LIFO != 0;
        let elem = st
            .llist
            .offset(index, lifo)
            .expect("index is within the list");
        st.llist.insert_before(&elem, value);
    }
    Ok(Value::Null)
}

fn debug_info_method(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::array(debug_info(&this(args))))
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
    ADD = "public function add(int $index, mixed $value): void {}" => add;
    POP = "public function pop(): mixed {}" => pop;
    SHIFT = "public function shift(): mixed {}" => shift;
    PUSH = "public function push(mixed $value): void {}" => push;
    UNSHIFT = "public function unshift(mixed $value): void {}" => unshift;
    TOP = "public function top(): mixed {}" => top;
    BOTTOM = "public function bottom(): mixed {}" => bottom;
    DEBUG_INFO = "public function __debugInfo(): array {}" => debug_info_method;
    COUNT = "public function count(): int {}" => count;
    IS_EMPTY = "public function isEmpty(): bool {}" => is_empty;
    SET_ITERATOR_MODE = "public function setIteratorMode(int $mode): int {}" => set_iterator_mode;
    GET_ITERATOR_MODE = "public function getIteratorMode(): int {}" => get_iterator_mode;
    OFFSET_EXISTS = "public function offsetExists($index): bool {}" => offset_exists;
    OFFSET_GET = "public function offsetGet($index): mixed {}" => offset_get;
    OFFSET_SET = "public function offsetSet($index, mixed $value): void {}" => offset_set;
    OFFSET_UNSET = "public function offsetUnset($index): void {}" => offset_unset;
    REWIND = "public function rewind(): void {}" => rewind;
    CURRENT = "public function current(): mixed {}" => current;
    KEY = "public function key(): int {}" => key;
    PREV = "public function prev(): void {}" => prev;
    NEXT = "public function next(): void {}" => next;
    VALID = "public function valid(): bool {}" => valid;
    UNSERIALIZE = "public function unserialize(string $data): void {}" => unserialize;
    SERIALIZE = "public function serialize(): string {}" => serialize;
    MAGIC_SERIALIZE = "public function __serialize(): array {}" => magic_serialize;
    MAGIC_UNSERIALIZE = "public function __unserialize(array $data): void {}" => magic_unserialize;
    // SplQueue (@implementation-alias SplDoublyLinkedList::push / ::shift)
    ENQUEUE = "public function enqueue(mixed $value): void {}" => push;
    DEQUEUE = "public function dequeue(): mixed {}" => shift;
}

fn int_constant(ce: &mut ClassEntry, name: &str, value: i32) {
    let mut c = ClassConstant::new(name, Value::Int(value as i64), SPL_DOUBLY_LINKED_LIST);
    c.ty = TypeDecl::parse("int").ok();
    ce.add_constant(c);
}

/// `PHP_MINIT_FUNCTION(spl_dllist)`.
pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(SPL_DOUBLY_LINKED_LIST, 0);
    for iface in ["Iterator", "Countable", "ArrayAccess", "Serializable"] {
        let iface = engine::class(r, iface);
        engine::implement(&mut ce, &iface);
    }
    int_constant(&mut ce, "IT_MODE_LIFO", IT_LIFO);
    int_constant(&mut ce, "IT_MODE_FIFO", IT_FIFO);
    int_constant(&mut ce, "IT_MODE_DELETE", IT_DELETE);
    int_constant(&mut ce, "IT_MODE_KEEP", IT_KEEP);
    engine::add_methods(
        &mut ce,
        &[
            &ADD,
            &POP,
            &SHIFT,
            &PUSH,
            &UNSHIFT,
            &TOP,
            &BOTTOM,
            &DEBUG_INFO,
            &COUNT,
            &IS_EMPTY,
            &SET_ITERATOR_MODE,
            &GET_ITERATOR_MODE,
            &OFFSET_EXISTS,
            &OFFSET_GET,
            &OFFSET_SET,
            &OFFSET_UNSET,
            &REWIND,
            &CURRENT,
            &KEY,
            &PREV,
            &NEXT,
            &VALID,
            &UNSERIALIZE,
            &SERIALIZE,
            &MAGIC_SERIALIZE,
            &MAGIC_UNSERIALIZE,
        ],
    );
    let h = &mut ce.handlers;
    h.create_object = Some(create_object);
    h.clone_obj = Some(clone_object);
    h.count_elements = Some(count_elements);
    let dllist = r.class(ce);

    let mut queue = ClassEntry::new_subclass(SPL_QUEUE, 0, &dllist);
    engine::add_methods(&mut queue, &[&ENQUEUE, &DEQUEUE]);
    r.class(queue);

    r.class(ClassEntry::new_subclass(SPL_STACK, 0, &dllist));
}
