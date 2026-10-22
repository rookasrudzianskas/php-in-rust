//! The unserializer: a port of ext/standard/var_unserializer.re (the re2c
//! grammar of `php_var_unserialize`) and `php_unserialize_with_options`
//! from ext/standard/var.c.
//!
//! # Slots and back-references
//! php-src records the address of every zval it fills in `var_hash`, so that
//! a later `R:n;` can turn that slot into a reference in place (and `r:n;`
//! can copy the object it holds). Here each slot filled during a parse is a
//! provisional cell: a [`PhpRef`] stored in its container (array element,
//! declared or dynamic property) as `Value::Ref(cell)`, and recorded in
//! `var_hash`. `R:` turns the cell's *content* into a real reference that
//! both slots share. When a parse finishes, every container slot is
//! replaced by its cell's content, children before parents, so the result
//! only contains real references.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use php_runtime::class::{ClassRef, class_flags, member_flags};
use php_runtime::types::{ClassType, TypeDecl, mask};
use php_runtime::value::convert::mangle_property_name;
use php_runtime::value::numeric::strtod;
use php_runtime::{ArrayKey, Flow, ObjRef, PResult, PhpArray, PhpRef, PhpStr, Value, Vm};

use super::incomplete::{
    INCOMPLETE_CLASS, MAGIC_MEMBER, MAGIC_MEMBER_ENUM_CASE, MAGIC_MEMBER_SERIALIZED,
    is_incomplete_class, set_member, store_class_name,
};
use super::{
    DeclaredProp, c_str, c_text, declared_props, globals, is_serializable, is_valid_class_name,
    with_lock,
};

/// `HT_MAX_SIZE` on 64-bit platforms.
const HT_MAX_SIZE: i64 = 0x4000_0000;

/// A typed property, as remembered for reference type checks.
struct PropType {
    /// Declaring class.
    class: PhpStr,
    /// Unmangled name.
    name: PhpStr,
    ty: TypeDecl,
}

/// Where a provisional cell lives.
enum Loc {
    /// Element `key` of the array held by the parent cell.
    Elem(PhpRef, ArrayKey),
    /// Declared property slot.
    Decl(ObjRef, usize),
    /// Dynamic property.
    Dyn(ObjRef, PhpStr),
}

/// A delayed magic-method call (`VAR_WAKEUP_FLAG` / `VAR_UNSERIALIZE_FLAG`).
enum Delayed {
    Wakeup(ObjRef),
    /// The object and the cell holding its data array.
    Unserialize(ObjRef, PhpRef),
}

/// `struct php_unserialize_data`: state shared by nested `unserialize()`
/// calls made from `Serializable::unserialize()`.
pub struct UnserializeData {
    /// `var_hash` entries (slot cells); `None` once a failed parse voided them.
    entries: Vec<Option<PhpRef>>,
    /// Provisional cells of in-flight parses.
    cells: Vec<(PhpRef, Loc)>,
    delayed: Vec<Delayed>,
    /// Lowercase names; `None` allows every class.
    allowed_classes: Option<Rc<HashSet<Vec<u8>>>>,
    /// `ref_props`: typed property slots that are not references (yet).
    ref_props: HashMap<usize, (PhpRef, Rc<PropType>)>,
    /// Type sources of references created by this unserialization.
    ref_sources: HashMap<usize, (PhpRef, Vec<Rc<PropType>>)>,
    cur_depth: i64,
    max_depth: i64,
    incomplete_classes_only: bool,
}

impl UnserializeData {
    fn new(max_depth: i64) -> UnserializeData {
        UnserializeData {
            entries: Vec::new(),
            cells: Vec::new(),
            delayed: Vec::new(),
            allowed_classes: None,
            ref_props: HashMap::new(),
            ref_sources: HashMap::new(),
            cur_depth: 0,
            max_depth,
            incomplete_classes_only: false,
        }
    }
}

fn ptr(r: &PhpRef) -> usize {
    r.as_ptr() as usize
}

/// `PHP_VAR_UNSERIALIZE_INIT`: joins the active unserialization unless the
/// serialize lock is held or none is active.
pub fn var_unserialize_init(vm: &mut Vm) -> Rc<RefCell<UnserializeData>> {
    let max_depth = vm.ini_int("unserialize_max_depth");
    let g = globals(vm);
    if g.lock > 0 || g.unser_level == 0 {
        let d = Rc::new(RefCell::new(UnserializeData::new(max_depth)));
        if g.lock == 0 {
            g.unser_data = Some(d.clone());
            g.unser_level = 1;
        }
        d
    } else {
        g.unser_level += 1;
        g.unser_data.clone().expect("active unserialization")
    }
}

/// `PHP_VAR_UNSERIALIZE_DESTROY`: the outermost level runs the delayed
/// `__wakeup()` / `__unserialize()` calls.
pub fn var_unserialize_destroy(vm: &mut Vm, d: Rc<RefCell<UnserializeData>>) -> PResult<()> {
    destroy(vm, d, false)
}

/// `php_var_unserialize_set_incomplete_classes_only`: represent every object
/// as `__PHP_Incomplete_Class` without resolving (or running) anything.
pub fn var_unserialize_set_incomplete_classes_only(d: &Rc<RefCell<UnserializeData>>, on: bool) {
    d.borrow_mut().incomplete_classes_only = on;
}

/// `exception_pending` emulates `EG(exception)`: delayed calls are then
/// skipped (`zend_call_function` refuses to run) and the objects are
/// marked as destructed.
fn destroy(vm: &mut Vm, d: Rc<RefCell<UnserializeData>>, exception_pending: bool) -> PResult<()> {
    let g = globals(vm);
    let owner = g.lock > 0 || g.unser_level == 1;
    let r = if owner {
        var_destroy(vm, &d, exception_pending)
    } else {
        Ok(())
    };
    let g = globals(vm);
    if g.lock == 0 {
        g.unser_level -= 1;
        if g.unser_level == 0 {
            g.unser_data = None;
        }
    }
    r
}

/// `var_destroy`.
fn var_destroy(
    vm: &mut Vm,
    d: &Rc<RefCell<UnserializeData>>,
    exception_pending: bool,
) -> PResult<()> {
    let delayed = {
        let mut d = d.borrow_mut();
        d.entries.clear();
        d.cells.clear();
        d.ref_props.clear();
        d.ref_sources.clear();
        std::mem::take(&mut d.delayed)
    };
    let mut failed = exception_pending;
    let mut error: Option<Flow> = None;
    for call in delayed {
        let (obj, r) = match call {
            Delayed::Wakeup(obj) => {
                if failed {
                    obj.mark_destructor_called();
                    continue;
                }
                let r = with_lock(vm, |vm| vm.call_method(&obj, b"__wakeup", Vec::new()));
                (obj, r)
            }
            Delayed::Unserialize(obj, data) => {
                if failed {
                    obj.mark_destructor_called();
                    continue;
                }
                let param = data.borrow().clone();
                let r = with_lock(vm, |vm| vm.call_method(&obj, b"__unserialize", vec![param]));
                (obj, r)
            }
        };
        if let Err(e) = r {
            failed = true;
            obj.mark_destructor_called();
            error.get_or_insert(e);
        }
    }
    error.map_or(Ok(()), Err)
}

/// `php_var_unserialize`: parses one value starting at `*pos`. `Ok(None)`
/// is a syntax error (`*pos` is where it was detected). The value may be a
/// reference (when a later `R:` pointed at it).
pub fn var_unserialize(
    vm: &mut Vm,
    d: &Rc<RefCell<UnserializeData>>,
    buf: &[u8],
    pos: &mut usize,
) -> PResult<Option<Value>> {
    let (entries_start, cells_start) = {
        let d = d.borrow();
        (d.entries.len(), d.cells.len())
    };
    let root = PhpRef::new(Value::Null);
    let mut p = Parser {
        buf,
        pos: *pos,
        d: d.clone(),
    };
    let r = p.parse(vm, &root, true);
    *pos = p.pos;
    if !matches!(r, Ok(true)) {
        // Forbid the use of these entries by other unserialize() calls in
        // the same context.
        for e in d.borrow_mut().entries.iter_mut().skip(entries_start) {
            *e = None;
        }
    }
    finish_cells(d, cells_start);
    let value = root.borrow().clone();
    match r {
        Ok(true) => Ok(Some(value)),
        Ok(false) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mutable access to the array a cell holds (through a reference).
fn with_array<R>(cell: &PhpRef, f: impl FnOnce(&mut PhpArray) -> R) -> Option<R> {
    let mut b = cell.borrow_mut();
    match &mut *b {
        Value::Array(a) => Some(f(Rc::make_mut(a))),
        Value::Ref(r) => match &mut *r.borrow_mut() {
            Value::Array(a) => Some(f(Rc::make_mut(a))),
            _ => None,
        },
        _ => None,
    }
}

/// Replaces `slot` by the content of `cell` if it still holds the cell.
fn unwrap_slot(slot: &mut Value, cell: &PhpRef) {
    if matches!(slot, Value::Ref(c) if PhpRef::ptr_eq(c, cell)) {
        *slot = cell.borrow().clone();
    }
}

/// Unwraps the provisional cells created since `start`, innermost first.
fn finish_cells(d: &Rc<RefCell<UnserializeData>>, start: usize) {
    let cells: Vec<(PhpRef, Loc)> = d.borrow_mut().cells.drain(start..).collect();
    for (cell, loc) in cells.into_iter().rev() {
        match loc {
            Loc::Elem(parent, key) => {
                with_array(&parent, |arr| {
                    if let Some(slot) = arr.get_mut(&key) {
                        unwrap_slot(slot, &cell);
                    }
                });
            }
            Loc::Decl(obj, slot) => {
                if let Some(v) = obj.obj().props.borrow_mut().get_mut(slot) {
                    unwrap_slot(v, &cell);
                }
            }
            Loc::Dyn(obj, key) => {
                if let Some(t) = obj.obj().dyn_props.borrow_mut().as_mut() {
                    if let Some(v) = t.get_mut(&ArrayKey::Str(key)) {
                        unwrap_slot(v, &cell);
                    }
                }
            }
        }
    }
}

fn set(cell: &PhpRef, v: Value) {
    *cell.borrow_mut() = v;
}

/// `object_init_ex` without a constructor call.
fn object_init_ex(vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let kind = if ce.is_interface() {
        Some("interface")
    } else if ce.is_trait() {
        Some("trait")
    } else if ce.is_enum() {
        Some("enum")
    } else if ce.is_abstract() {
        Some("abstract class")
    } else {
        None
    };
    if let Some(kind) = kind {
        let msg = format!("Cannot instantiate {kind} {}", ce.name);
        return Err(vm.throw_error("Error", msg));
    }
    vm.instantiate(ce)
}

/// Result of `is_property_visibility_changed`.
enum Visibility {
    Unchanged,
    /// The key names a declared property: use its table key.
    Changed(PhpStr),
    Fail,
}

/// An unmangled property name: the class part (if mangled) and the name.
type Unmangled<'k> = (Option<&'k [u8]>, &'k [u8]);

/// `zend_unmangle_property_name_ex`: `Ok(None)` for malformed names (after
/// the notice).
fn unmangle<'k>(vm: &mut Vm, key: &'k [u8]) -> PResult<Option<Unmangled<'k>>> {
    if key.is_empty() || key[0] != 0 {
        return Ok(Some((None, key)));
    }
    let len = key.len();
    if len < 3 || key[1] == 0 {
        vm.engine_notice("Illegal member variable name")?;
        return Ok(None);
    }
    let strnlen = |s: &[u8], max: usize| {
        s.iter()
            .take(max)
            .position(|&b| b == 0)
            .unwrap_or(max.min(s.len()))
    };
    let mut class_len = strnlen(&key[1..], len - 2);
    if class_len >= len - 2 || key[class_len + 1] != 0 {
        vm.engine_notice("Corrupt member variable name")?;
        return Ok(None);
    }
    let class = &key[1..1 + class_len];
    let anon_len = strnlen(&key[class_len + 2..], len - class_len - 2);
    if class_len + anon_len + 2 != len {
        class_len += anon_len + 1;
    }
    Ok(Some((Some(class), &key[class_len + 2..])))
}

/// `zend_get_unmangled_property_name` as printed by `%s`.
fn unmangled_name(vm: &mut Vm, key: &[u8]) -> PResult<String> {
    Ok(match unmangle(vm, key)? {
        Some((_, prop)) => c_text(prop),
        None => c_text(key),
    })
}

/// Removes, after each NUL, the rest of a `%s`-formatted name (up to the
/// closing quote), as C's `%s` would not have printed it.
fn cut_nul_names(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut skipping = false;
    for c in msg.chars() {
        if c == '\0' {
            skipping = true;
        } else if skipping && c == '"' {
            skipping = false;
            out.push(c);
        } else if !skipping {
            out.push(c);
        }
    }
    out
}

struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
    d: Rc<RefCell<UnserializeData>>,
}

impl Parser<'_> {
    fn max(&self) -> usize {
        self.buf.len()
    }

    /// The byte at `i`; 0 past the end (php-src strings are NUL-terminated).
    fn at(&self, i: usize) -> u8 {
        self.buf.get(i).copied().unwrap_or(0)
    }

    fn digits_end(&self, mut i: usize) -> usize {
        while self.at(i).is_ascii_digit() {
            i += 1;
        }
        i
    }

    /// Matches `uiv` followed by `term`; returns the index after `term`.
    fn uiv_then(&self, i: usize, term: u8) -> Option<usize> {
        let end = self.digits_end(i);
        (end > i && self.at(end) == term).then_some(end + 1)
    }

    /// `parse_uiv` (wrapping like `size_t` arithmetic).
    fn parse_uiv(&self, mut i: usize) -> u64 {
        let mut r: u64 = 0;
        while let c @ b'0'..=b'9' = self.at(i) {
            r = r.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
            i += 1;
        }
        r
    }

    /// `parse_iv2`: optional sign and digits; saturates with a warning.
    fn parse_iv2(&self, vm: &mut Vm, mut i: usize) -> PResult<(i64, usize)> {
        let mut neg = false;
        if self.at(i) == b'-' {
            neg = true;
            i += 1;
        } else if self.at(i) == b'+' {
            i += 1;
        }
        while self.at(i) == b'0' {
            i += 1;
        }
        let start = i;
        let mut result: u64 = 0;
        while let c @ b'0'..=b'9' = self.at(i) {
            result = result.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
            i += 1;
        }
        // Number too long or overflow (MAX_LENGTH_OF_LONG - 1 = 19 digits).
        if i - start > 19 || result > i64::MAX as u64 + u64::from(neg) {
            vm.warning("Numerical result out of range")?;
            return Ok((if neg { i64::MIN } else { i64::MAX }, i));
        }
        let v = if neg {
            (result as i64).wrapping_neg()
        } else {
            result as i64
        };
        Ok((v, i))
    }

    fn new_cell(&self, loc: Loc) -> PhpRef {
        let cell = PhpRef::new(Value::Null);
        self.d.borrow_mut().cells.push((cell.clone(), loc));
        cell
    }

    /// `php_var_unserialize_internal`. With `hash == false` (array and
    /// property keys) back-references and compound values are rejected.
    fn parse(&mut self, vm: &mut Vm, rval: &PhpRef, hash: bool) -> PResult<bool> {
        let start = self.pos;
        if start >= self.max() {
            return Ok(false);
        }
        let c = self.buf[start];
        if hash && c != b'R' {
            self.d.borrow_mut().entries.push(Some(rval.clone()));
        }
        match c {
            b'R' | b'r' if self.at(start + 1) == b':' => {
                Ok(self.back_ref(rval, hash, start, c == b'R'))
            }
            b'N' if self.at(start + 1) == b';' => {
                self.pos = start + 2;
                set(rval, Value::Null);
                Ok(true)
            }
            b'b' if self.at(start + 1) == b':'
                && matches!(self.at(start + 2), b'0' | b'1')
                && self.at(start + 3) == b';' =>
            {
                self.pos = start + 4;
                set(rval, Value::Bool(self.at(start + 2) == b'1'));
                Ok(true)
            }
            b'i' if self.at(start + 1) == b':' => self.int(vm, rval, start),
            b'd' if self.at(start + 1) == b':' => Ok(self.double(rval, start)),
            b's' | b'S' if self.at(start + 1) == b':' => self.string(vm, rval, start, c == b'S'),
            b'a' if self.at(start + 1) == b':' => self.array(vm, rval, hash, start),
            b'O' | b'C' if self.at(start + 1) == b':' => self.object(vm, rval, hash, start),
            b'E' if self.at(start + 1) == b':' => self.enum_case(vm, rval, hash, start),
            b'}' => {
                // Less data than announced.
                vm.warning("Unexpected end of serialized data")?;
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// `"R:" uiv ";"` and `"r:" uiv ";"`.
    fn back_ref(&mut self, rval: &PhpRef, hash: bool, start: usize, is_ref: bool) -> bool {
        let Some(end) = self.uiv_then(start + 2, b';') else {
            return false;
        };
        self.pos = end;
        if !hash {
            return false;
        }
        let id = self.parse_uiv(start + 2).wrapping_sub(1) as i64;
        if id < 0 {
            return false;
        }
        let target = {
            let d = self.d.borrow();
            match d.entries.get(id as usize) {
                Some(Some(t)) => t.clone(),
                _ => return false,
            }
        };
        if PhpRef::ptr_eq(&target, rval) {
            return false;
        }
        if is_ref {
            let already_ref = matches!(*target.borrow(), Value::Ref(_));
            if !already_ref {
                let old = std::mem::take(&mut *target.borrow_mut());
                let real = PhpRef::new(old);
                *target.borrow_mut() = Value::Ref(real.clone());
                let mut d = self.d.borrow_mut();
                if let Some((_, info)) = d.ref_props.get(&ptr(&target)) {
                    let info = info.clone();
                    d.ref_sources.insert(ptr(&real), (real.clone(), vec![info]));
                }
            }
            let v = target.borrow().clone();
            set(rval, v);
        } else {
            let v = target.borrow().unref();
            if !matches!(v, Value::Object(_)) {
                return false;
            }
            set(rval, v);
        }
        true
    }

    /// `"i:" iv ";"`.
    fn int(&mut self, vm: &mut Vm, rval: &PhpRef, start: usize) -> PResult<bool> {
        let mut i = start + 2;
        if matches!(self.at(i), b'+' | b'-') {
            i += 1;
        }
        let end = self.digits_end(i);
        if end == i || self.at(end) != b';' {
            return Ok(false);
        }
        self.pos = end + 1;
        let (v, _) = self.parse_iv2(vm, start + 2)?;
        set(rval, Value::Int(v));
        Ok(true)
    }

    /// `"d:" ("NAN" | "-"? "INF") ";"` and `"d:" (iv | nv | nvexp) ";"`.
    fn double(&mut self, rval: &PhpRef, start: usize) -> bool {
        let s = start + 2;
        let rest = &self.buf[s.min(self.max())..];
        for (pat, v) in [
            (&b"NAN;"[..], f64::NAN),
            (b"INF;", f64::INFINITY),
            (b"-INF;", f64::NEG_INFINITY),
        ] {
            if rest.starts_with(pat) {
                self.pos = s + pat.len();
                set(rval, Value::Float(v));
                return true;
            }
        }
        let mut i = s;
        if matches!(self.at(i), b'+' | b'-') {
            i += 1;
        }
        let int_end = self.digits_end(i);
        let int_digits = int_end - i;
        i = int_end;
        if self.at(i) == b'.' {
            let frac_end = self.digits_end(i + 1);
            if int_digits == 0 && frac_end == i + 1 {
                return false;
            }
            i = frac_end;
        } else if int_digits == 0 {
            return false;
        }
        if matches!(self.at(i), b'e' | b'E') {
            let mut j = i + 1;
            if matches!(self.at(j), b'+' | b'-') {
                j += 1;
            }
            let exp_end = self.digits_end(j);
            if exp_end == j {
                return false;
            }
            i = exp_end;
        }
        if self.at(i) != b';' {
            return false;
        }
        self.pos = i + 1;
        set(rval, Value::Float(strtod(&self.buf[s..i]).0));
        true
    }

    /// `"s:" uiv ":" ["]` and the deprecated escaped `"S:" uiv ":" ["]`.
    fn string(&mut self, vm: &mut Vm, rval: &PhpRef, start: usize, escaped: bool) -> PResult<bool> {
        let Some(colon_end) = self.uiv_then(start + 2, b':') else {
            return Ok(false);
        };
        if self.at(colon_end) != b'"' {
            return Ok(false);
        }
        let cursor = colon_end + 1;
        let len = self.parse_uiv(start + 2);
        let maxlen = (self.max() - cursor.min(self.max())) as u64;
        if maxlen < len {
            self.pos = start + 2;
            return Ok(false);
        }
        let len = len as usize;
        let (bytes, mut cur) = if escaped {
            // unserialize_str
            let end = self.max();
            let mut out = Vec::with_capacity(len);
            let mut cur = cursor;
            for _ in 0..len {
                if cur >= end {
                    return Ok(false);
                }
                if self.buf[cur] != b'\\' {
                    out.push(self.buf[cur]);
                } else {
                    let mut ch: u8 = 0;
                    for _ in 0..2 {
                        cur += 1;
                        let d = match self.at(cur) {
                            c @ b'0'..=b'9' => c - b'0',
                            c @ b'a'..=b'f' => c - b'a' + 10,
                            c @ b'A'..=b'F' => c - b'A' + 10,
                            _ => return Ok(false),
                        };
                        ch = (ch << 4).wrapping_add(d);
                    }
                    out.push(ch);
                }
                cur += 1;
            }
            (out, cur)
        } else {
            (self.buf[cursor..cursor + len].to_vec(), cursor + len)
        };
        if self.at(cur) != b'"' {
            self.pos = cur;
            return Ok(false);
        }
        if self.at(cur + 1) != b';' {
            self.pos = cur + 1;
            return Ok(false);
        }
        cur += 2;
        self.pos = cur;
        set(rval, Value::String(PhpStr::from_vec(bytes)));
        if escaped {
            vm.deprecated("Unserializing the 'S' format is deprecated")?;
        }
        Ok(true)
    }

    /// `finish_nested_data`.
    fn finish_nested(&mut self) -> bool {
        if self.pos >= self.max() || self.buf[self.pos] != b'}' {
            return false;
        }
        self.pos += 1;
        true
    }

    /// `"a:" uiv ":" "{"`.
    fn array(&mut self, vm: &mut Vm, rval: &PhpRef, hash: bool, start: usize) -> PResult<bool> {
        let Some(colon_end) = self.uiv_then(start + 2, b':') else {
            return Ok(false);
        };
        if self.at(colon_end) != b'{' {
            return Ok(false);
        }
        let cursor = colon_end + 1;
        // parse_iv() rather than parse_uiv() checks the data range.
        let (elements, _) = self.parse_iv2(vm, start + 2)?;
        self.pos = cursor;
        if !hash {
            return Ok(false);
        }
        if !(0..HT_MAX_SIZE).contains(&elements)
            || elements as u64 > (self.max() - cursor) as u64 / 2
        {
            return Ok(false);
        }
        if elements == 0 {
            set(rval, Value::empty_array());
            return Ok(self.finish_nested());
        }
        set(
            rval,
            Value::array(PhpArray::with_capacity(elements as usize)),
        );
        if !self.nested_array(vm, rval, elements)? {
            return Ok(false);
        }
        Ok(self.finish_nested())
    }

    /// The depth check at the start of `process_nested_*_data`.
    fn enter_nested(&mut self, vm: &mut Vm) -> PResult<bool> {
        let (max, cur) = {
            let d = self.d.borrow();
            (d.max_depth, d.cur_depth)
        };
        if max > 0 && cur >= max {
            vm.warning(format!(
                "Maximum depth of {max} exceeded. The depth limit can be changed using the \
                 max_depth unserialize() option or the unserialize_max_depth ini setting"
            ))?;
            return Ok(false);
        }
        self.d.borrow_mut().cur_depth += 1;
        Ok(true)
    }

    /// Fails unless the previous value ended with `;` or `}`.
    fn check_separator(&mut self, remaining: i64) -> bool {
        if remaining > 0 && !matches!(self.at(self.pos.wrapping_sub(1)), b';' | b'}') {
            self.pos -= 1;
            return false;
        }
        true
    }

    /// Parses a key (`var_hash == NULL`).
    fn key(&mut self, vm: &mut Vm) -> PResult<Option<Value>> {
        let key = PhpRef::new(Value::Undef);
        Ok(self.parse(vm, &key, false)?.then(|| key.get()))
    }

    /// `process_nested_array_data`: fills the array held by `container`.
    fn nested_array(&mut self, vm: &mut Vm, container: &PhpRef, elements: i64) -> PResult<bool> {
        if !self.enter_nested(vm)? {
            return Ok(false);
        }
        let r = self.nested_array_elems(vm, container, elements);
        self.d.borrow_mut().cur_depth -= 1;
        r
    }

    fn nested_array_elems(
        &mut self,
        vm: &mut Vm,
        container: &PhpRef,
        mut elements: i64,
    ) -> PResult<bool> {
        while elements > 0 {
            elements -= 1;
            let key = match self.key(vm)? {
                Some(Value::Int(i)) => ArrayKey::Int(i),
                // ZEND_HANDLE_NUMERIC
                Some(Value::String(s)) => ArrayKey::from_php_str(s),
                _ => return Ok(false),
            };
            let Some(slot) = self.array_slot(container, key) else {
                return Ok(false);
            };
            if !self.parse(vm, &slot, true)? {
                return Ok(false);
            }
            if !self.check_separator(elements) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `zend_hash_lookup`: the cell of element `key` (an existing element's
    /// cell is reused and reset to `null`).
    fn array_slot(&self, container: &PhpRef, key: ArrayKey) -> Option<PhpRef> {
        let existing = with_array(container, |arr| match arr.get_mut(&key) {
            Some(Value::Ref(c)) => {
                set(c, Value::Null);
                Some(Some(c.clone()))
            }
            Some(_) => Some(None),
            None => None,
        })?;
        if let Some(Some(cell)) = existing {
            return Some(cell);
        }
        let cell = self.new_cell(Loc::Elem(container.clone(), key.clone()));
        let v = Value::Ref(cell.clone());
        with_array(container, |arr| {
            arr.insert(key, v);
        })?;
        Some(cell)
    }

    /// `object ":" uiv ":" ["]` (`O:` and `C:`).
    fn object(&mut self, vm: &mut Vm, rval: &PhpRef, hash: bool, start: usize) -> PResult<bool> {
        let Some(colon_end) = self.uiv_then(start + 2, b':') else {
            return Ok(false);
        };
        if self.at(colon_end) != b'"' {
            return Ok(false);
        }
        if !hash {
            return Ok(false);
        }
        let custom = self.buf[start] == b'C';
        let cursor = colon_end + 1;
        let len = self.parse_uiv(start + 2);
        let maxlen = (self.max() - cursor.min(self.max())) as u64;
        if maxlen < len || len == 0 {
            self.pos = start + 2;
            return Ok(false);
        }
        let name_end = cursor + len as usize;
        if self.at(name_end) != b'"' {
            self.pos = name_end;
            return Ok(false);
        }
        if self.at(name_end + 1) != b':' {
            self.pos = name_end + 1;
            return Ok(false);
        }
        let name = &self.buf[cursor..name_end];
        // Runtime definition keys and leading namespace separators are not
        // allowed.
        if name[0] == 0 || name[0] == b'\\' {
            return Ok(false);
        }
        let class_name = PhpStr::from_bytes(name);
        let Some((ce, incomplete)) = self.resolve_class(vm, &class_name)? else {
            return Ok(false);
        };
        self.pos = name_end;

        if ce.has_flag(class_flags::NOT_SERIALIZABLE) {
            let msg = format!("Unserialization of '{}' is not allowed", ce.name);
            return Err(vm.throw_error("Exception", msg));
        }
        if custom {
            let keep_payload = self.d.borrow().incomplete_classes_only;
            let ok = self.object_custom(vm, rval, &ce, keep_payload)?;
            if ok && incomplete {
                if let Value::Object(o) = rval.borrow().clone() {
                    store_class_name(&o, &class_name);
                }
            }
            return Ok(ok);
        }
        if self.pos as i64 >= self.max() as i64 - 2 {
            vm.engine_warning("Bad unserialize data")?;
            return Ok(false);
        }
        let (elements, end) = self.parse_iv2(vm, self.pos + 2)?;
        self.pos = end;
        if elements < 0 || elements as u64 > (self.max() - name_end) as u64 / 2 {
            return Ok(false);
        }
        let cur = self.pos;
        if self.at(cur) != b':' {
            return Ok(false);
        }
        if self.at(cur + 1) != b'{' {
            self.pos = cur + 1;
            return Ok(false);
        }
        self.pos += 2;

        let has_unserialize = !incomplete
            && (ce.magic.unserialize.is_some() || ce.find_method(b"__unserialize").is_some());
        // A Serializable class only lands here with data that does not come
        // from its serializer, unless it also has __unserialize().
        if is_serializable(&ce) && !has_unserialize {
            vm.engine_warning(format!(
                "Erroneous data format for unserializing '{}'",
                ce.name
            ))?;
            return Ok(false);
        }
        let obj = object_init_ex(vm, &ce)?;
        set(rval, Value::Object(obj.clone()));
        if incomplete {
            store_class_name(&obj, &class_name);
        }
        self.object_common(vm, &obj, elements, has_unserialize)
    }

    /// The class lookup of the object rule: `Ok(None)` rejects the data;
    /// the flag tells whether an incomplete object must be created.
    fn resolve_class(
        &mut self,
        vm: &mut Vm,
        class_name: &PhpStr,
    ) -> PResult<Option<(ClassRef, bool)>> {
        let incomplete = |vm: &Vm| {
            vm.find_class(INCOMPLETE_CLASS)
                .map(|ce| (ce, true))
                .expect("__PHP_Incomplete_Class is registered")
        };
        if self.d.borrow().incomplete_classes_only {
            // Do not look the class up at all, so that no user code can run.
            if !is_valid_class_name(class_name) {
                return Ok(None);
            }
            return Ok(Some(incomplete(vm)));
        }
        let lc = class_name.to_ascii_lowercase();
        let allowed = match &self.d.borrow().allowed_classes {
            None => true,
            Some(set) => !set.is_empty() && set.contains(lc.as_bytes()),
        };
        if !allowed {
            if !is_valid_class_name(class_name) {
                return Ok(None);
            }
            return Ok(Some(incomplete(vm)));
        }
        if let Some(ce) = vm.find_class(class_name) {
            if !ce.has_flag(class_flags::ANON) {
                return Ok(Some((ce, false)));
            }
        }
        if !is_valid_class_name(class_name) {
            return Ok(None);
        }
        if let Some(ce) = with_lock(vm, |vm| vm.lookup_class(class_name))? {
            return Ok(Some((ce, false)));
        }
        let callback = match vm.ini_get("unserialize_callback_func") {
            Some(cb) if !cb.is_empty() => cb,
            _ => return Ok(Some(incomplete(vm))),
        };
        let func = Value::String(callback.clone());
        let host = vm.host.clone();
        if let Err(err) = host.check_callable(vm, &func) {
            let msg = format!(
                "Invalid callback {}, {}",
                c_text(&callback),
                cut_nul_names(&err)
            );
            return Err(vm.throw_error("Error", msg));
        }
        with_lock(vm, |vm| {
            vm.call(&func, vec![Value::String(class_name.clone())])
        })?;
        // The callback may have defined the class.
        match with_lock(vm, |vm| vm.lookup_class(class_name))? {
            Some(ce) => Ok(Some((ce, false))),
            None => {
                let mut msg = b"Function ".to_vec();
                msg.extend_from_slice(c_str(&callback));
                msg.extend_from_slice(b"() hasn't defined the class it was called for");
                vm.warning(PhpStr::from_vec(msg))?;
                Ok(Some(incomplete(vm)))
            }
        }
    }

    /// `object_custom`: the `C:` payload goes to `Serializable::unserialize()`.
    fn object_custom(
        &mut self,
        vm: &mut Vm,
        rval: &PhpRef,
        ce: &ClassRef,
        keep_payload: bool,
    ) -> PResult<bool> {
        let (datalen, end) = self.parse_iv2(vm, self.pos + 2)?;
        self.pos = end;
        let max = self.max();
        if max.saturating_sub(self.pos) < 2 {
            return Ok(false);
        }
        if self.at(self.pos) != b':' {
            return Ok(false);
        }
        if self.at(self.pos + 1) != b'{' {
            self.pos += 1;
            return Ok(false);
        }
        self.pos += 2;
        let present = (max - self.pos) as i64;
        if datalen < 0 || present <= datalen {
            vm.engine_warning(format!(
                "Insufficient data for unserializing - {datalen} required, {present} present"
            ))?;
            return Ok(false);
        }
        let datalen = datalen as usize;
        // The payload must be terminated by '}'.
        if self.at(self.pos + datalen) != b'}' {
            self.pos += datalen;
            return Ok(false);
        }
        let payload = PhpStr::from_bytes(&self.buf[self.pos..self.pos + datalen]);
        if keep_payload {
            // The class was not resolved: keep the payload verbatim so that
            // the object serializes back to the same bytes.
            let obj = object_init_ex(vm, ce)?;
            set_member(&obj, MAGIC_MEMBER_SERIALIZED, Value::String(payload));
            set(rval, Value::Object(obj));
        } else if !is_serializable(ce) {
            vm.engine_warning(format!("Class {} has no unserializer", ce.name))?;
            // __PHP_Incomplete_Class has no internal state, an empty
            // instance is safe.
            if !is_incomplete_class(ce) {
                return Ok(false);
            }
            let obj = object_init_ex(vm, ce)?;
            set(rval, Value::Object(obj));
        } else {
            // zend_user_unserialize: the object is visible to nested
            // unserialize() calls while its unserialize() method runs.
            let obj = object_init_ex(vm, ce)?;
            set(rval, Value::Object(obj.clone()));
            vm.call_method(&obj, b"unserialize", vec![Value::String(payload)])?;
        }
        self.pos += datalen + 1;
        Ok(true)
    }

    /// `object_common`: properties, or the `__unserialize()` data array.
    fn object_common(
        &mut self,
        vm: &mut Vm,
        obj: &ObjRef,
        elements: i64,
        has_unserialize: bool,
    ) -> PResult<bool> {
        if has_unserialize {
            if elements >= HT_MAX_SIZE {
                return Ok(false);
            }
            let data = PhpRef::new(Value::array(PhpArray::with_capacity(elements as usize)));
            match self.nested_array(vm, &data, elements) {
                Ok(true) => {}
                r => {
                    obj.mark_destructor_called();
                    return r;
                }
            }
            // Delay __unserialize() until the end of the unserialization.
            self.d
                .borrow_mut()
                .delayed
                .push(Delayed::Unserialize(obj.clone(), data));
            return Ok(self.finish_nested());
        }
        let ce = obj.class().clone();
        let has_wakeup = !is_incomplete_class(&ce) && ce.find_method(b"__wakeup").is_some();
        let existing = obj.obj().props.borrow().len()
            + obj
                .obj()
                .dyn_props
                .borrow()
                .as_ref()
                .map_or(0, PhpArray::len);
        if elements >= HT_MAX_SIZE - existing as i64 {
            return Ok(false);
        }
        match self.nested_object(vm, obj, elements) {
            Ok(true) => {}
            r => {
                if has_wakeup {
                    obj.mark_destructor_called();
                }
                return r;
            }
        }
        if has_wakeup {
            // Delay __wakeup() until the end of the unserialization.
            self.d
                .borrow_mut()
                .delayed
                .push(Delayed::Wakeup(obj.clone()));
        }
        Ok(self.finish_nested())
    }

    /// `process_nested_object_data`.
    fn nested_object(&mut self, vm: &mut Vm, obj: &ObjRef, elements: i64) -> PResult<bool> {
        if !self.enter_nested(vm)? {
            return Ok(false);
        }
        let r = self.nested_object_props(vm, obj, elements);
        self.d.borrow_mut().cur_depth -= 1;
        r
    }

    fn nested_object_props(
        &mut self,
        vm: &mut Vm,
        obj: &ObjRef,
        mut elements: i64,
    ) -> PResult<bool> {
        let declared = declared_props(obj);
        while elements > 0 {
            elements -= 1;
            let key = match self.key(vm)? {
                Some(Value::String(s)) => {
                    if self.d.borrow().incomplete_classes_only
                        && [
                            MAGIC_MEMBER,
                            MAGIC_MEMBER_SERIALIZED,
                            MAGIC_MEMBER_ENUM_CASE,
                        ]
                        .contains(&s.as_bytes())
                    {
                        // Data carrying the bookkeeping members is rejected.
                        return Ok(false);
                    }
                    s
                }
                // Object properties never have integer keys.
                Some(Value::Int(i)) => PhpStr::from(i.to_string()),
                _ => return Ok(false),
            };
            let Some((cell, info)) = self.prop_slot(vm, obj, &declared, key)? else {
                return Ok(false);
            };
            let parsed = self.parse(vm, &cell, true);
            if !matches!(parsed, Ok(true)) {
                if let Some((slot, _)) = &info {
                    restore_default(&cell, obj, *slot);
                }
                return parsed;
            }
            if let Some((slot, info)) = info {
                if let Err(e) = self.verify_prop(vm, &info, &cell) {
                    restore_default(&cell, obj, slot);
                    return Err(e);
                }
                let content = cell.borrow().clone();
                let mut d = self.d.borrow_mut();
                match content {
                    Value::Ref(real) => {
                        d.ref_sources
                            .entry(ptr(&real))
                            .or_insert_with(|| (real.clone(), Vec::new()))
                            .1
                            .push(info);
                    }
                    // Remember the property of this slot, to add a type
                    // source if it is turned into a reference later on.
                    _ => {
                        d.ref_props.insert(ptr(&cell), (cell.clone(), info));
                    }
                }
            }
            if !self.check_separator(elements) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Finds (or creates) the slot for property table key `key`; returns its
    /// cell and, for typed declared properties, the slot and type.
    #[allow(clippy::type_complexity)]
    fn prop_slot(
        &mut self,
        vm: &mut Vm,
        obj: &ObjRef,
        declared: &[DeclaredProp],
        key: PhpStr,
    ) -> PResult<Option<(PhpRef, Option<(usize, Rc<PropType>)>)>> {
        if let Some(dp) = declared.iter().find(|d| d.key == key) {
            return Ok(Some(self.declared_slot(obj, dp)));
        }
        let ce = obj.class().clone();
        let is_dynamic = obj
            .obj()
            .dyn_props
            .borrow()
            .as_ref()
            .is_some_and(|t| t.contains_key(&ArrayKey::Str(key.clone())));
        let key = match self.visibility_changed(vm, &ce, &key)? {
            Visibility::Fail => return Ok(None),
            Visibility::Changed(new_key) => new_key,
            Visibility::Unchanged => {
                if !is_dynamic {
                    if ce.has_flag(class_flags::READONLY) || ce.is_enum() {
                        let msg = format!(
                            "Cannot create dynamic property {}::${}",
                            ce.name,
                            unmangled_name(vm, &key)?
                        );
                        return Err(vm.throw_error("Error", msg));
                    }
                    if !ce.has_flag(class_flags::ALLOW_DYNAMIC_PROPERTIES) {
                        let msg = format!(
                            "Creation of dynamic property {}::${} is deprecated",
                            ce.name,
                            unmangled_name(vm, &key)?
                        );
                        vm.engine_deprecated(msg)?;
                    }
                }
                return Ok(Some((self.dyn_slot(obj, key), None)));
            }
        };
        // second_try: the declared property's own table key.
        if let Some(dp) = declared.iter().find(|d| d.key == key) {
            return Ok(Some(self.declared_slot(obj, dp)));
        }
        Ok(Some((self.dyn_slot(obj, key), None)))
    }

    /// The cell of a declared property slot (reset to `null`).
    fn declared_slot(
        &self,
        obj: &ObjRef,
        dp: &DeclaredProp,
    ) -> (PhpRef, Option<(usize, Rc<PropType>)>) {
        let info = dp.info.ty.as_ref().map(|ty| {
            Rc::new(PropType {
                class: dp.info.declaring_class.clone(),
                name: dp.info.name.clone(),
                ty: ty.clone(),
            })
        });
        let existing = match obj.obj().props.borrow().get(dp.slot) {
            Some(Value::Ref(c)) => Some(c.clone()),
            _ => None,
        };
        let cell = match existing {
            Some(c) => {
                if info.is_some() {
                    let mut d = self.d.borrow_mut();
                    // If the value is overwritten, remove the old type source.
                    if let Value::Ref(real) = &*c.borrow() {
                        if let Some((_, sources)) = d.ref_sources.get_mut(&ptr(real)) {
                            if let Some(i) = sources.iter().position(|s| {
                                s.name == dp.info.name && s.class == dp.info.declaring_class
                            }) {
                                sources.remove(i);
                            }
                        }
                    }
                    d.ref_props.remove(&ptr(&c));
                }
                set(&c, Value::Null);
                c
            }
            None => {
                let c = self.new_cell(Loc::Decl(obj.clone(), dp.slot));
                if let Some(slot) = obj.obj().props.borrow_mut().get_mut(dp.slot) {
                    *slot = Value::Ref(c.clone());
                }
                c
            }
        };
        (cell, info.map(|i| (dp.slot, i)))
    }

    /// The cell of a dynamic property (created if missing, reset to `null`).
    fn dyn_slot(&self, obj: &ObjRef, key: PhpStr) -> PhpRef {
        let k = ArrayKey::Str(key.clone());
        let existing = match obj
            .obj()
            .dyn_props
            .borrow()
            .as_ref()
            .and_then(|t| t.get(&k))
        {
            Some(Value::Ref(c)) => Some(c.clone()),
            _ => None,
        };
        if let Some(c) = existing {
            set(&c, Value::Null);
            return c;
        }
        let c = self.new_cell(Loc::Dyn(obj.clone(), key));
        obj.obj()
            .dyn_props
            .borrow_mut()
            .get_or_insert_with(PhpArray::new)
            .insert(k, Value::Ref(c.clone()));
        c
    }

    /// `is_property_visibility_changed`: a key whose mangling does not match
    /// the declared property (e.g. a now-protected property serialized as
    /// public) is redirected to the declared slot.
    fn visibility_changed(
        &mut self,
        vm: &mut Vm,
        ce: &ClassRef,
        key: &PhpStr,
    ) -> PResult<Visibility> {
        if ce.properties.is_empty() {
            return Ok(Visibility::Unchanged);
        }
        let Some((class, prop)) = unmangle(vm, key)? else {
            return Ok(Visibility::Fail);
        };
        let info = match class {
            None => ce.properties.get(key),
            Some(c) if c == b"*" || ce.name.eq_ignore_ascii_case(c) => ce.properties.get(prop),
            Some(_) => None,
        };
        let Some(info) = info else {
            return Ok(Visibility::Unchanged);
        };
        if info.flags & member_flags::VIRTUAL == 0 {
            return Ok(Visibility::Changed(mangle_property_name(
                &info.name,
                info.flags,
                &info.declaring_class,
            )));
        }
        let msg = format!(
            "Cannot unserialize value for virtual property {}::${}",
            info.declaring_class,
            c_text(key)
        );
        vm.warning(msg)?;
        Ok(Visibility::Fail)
    }

    /// `zend_verify_prop_assignable_by_ref(info, data, strict = 1)` with the
    /// type sources known to this unserialization. Throws on failure.
    fn verify_prop(&mut self, vm: &mut Vm, info: &PropType, cell: &PhpRef) -> PResult<()> {
        let content = cell.borrow().clone();
        if let Value::Ref(real) = &content {
            let sources = self
                .d
                .borrow()
                .ref_sources
                .get(&ptr(real))
                .map(|(_, s)| s.clone())
                .filter(|s| !s.is_empty());
            if let Some(sources) = sources {
                let val = real.get();
                if type_accepts(info, &val) {
                    return Ok(());
                }
                // A coercion (int to float) would be needed: it conflicts
                // with the other properties holding the reference.
                if info.ty.mask & mask::FLOAT != 0
                    && matches!(val, Value::Int(_))
                    && vm.verify_weak_scalar(info.ty.mask, &val, false)?.is_some()
                {
                    let src = &sources[0];
                    let msg = format!(
                        "Reference with value of type {} held by property {}::${} of type {} \
                         is not compatible with property {}::${} of type {}",
                        val.type_name(),
                        src.class,
                        src.name,
                        src.ty.to_type_string(),
                        info.class,
                        info.name,
                        info.ty.to_type_string()
                    );
                    return Err(vm.throw_type_error(msg));
                }
                return Err(prop_type_error(vm, info, &val));
            }
        }
        let val = content.unref();
        if type_accepts(info, &val) {
            return Ok(());
        }
        // Strict mode only allows int to float.
        if let (true, Value::Int(i)) = (info.ty.mask & mask::FLOAT != 0, &val) {
            let f = Value::Float(*i as f64);
            match &content {
                Value::Ref(real) => {
                    real.set(f);
                }
                _ => set(cell, f),
            }
            return Ok(());
        }
        Err(prop_type_error(vm, info, &val))
    }

    /// `"E:" uiv ":" ["]`.
    fn enum_case(&mut self, vm: &mut Vm, rval: &PhpRef, hash: bool, start: usize) -> PResult<bool> {
        let Some(colon_end) = self.uiv_then(start + 2, b':') else {
            return Ok(false);
        };
        if self.at(colon_end) != b'"' {
            return Ok(false);
        }
        if !hash {
            return Ok(false);
        }
        let cursor = colon_end + 1;
        let len = self.parse_uiv(start + 2);
        let maxlen = (self.max() - cursor.min(self.max())) as u64;
        if maxlen < len || len == 0 {
            self.pos = start + 2;
            return Ok(false);
        }
        let end = cursor + len as usize;
        if self.at(end) != b'"' {
            self.pos = end;
            return Ok(false);
        }
        if self.at(end + 1) != b';' {
            self.pos = end + 1;
            return Ok(false);
        }
        let s = &self.buf[cursor..end];
        let Some(colon) = s.iter().position(|&b| b == b':') else {
            let mut msg = b"Invalid enum name '".to_vec();
            msg.extend_from_slice(s);
            msg.extend_from_slice(b"' (missing colon)");
            vm.warning(PhpStr::from_vec(msg))?;
            return Ok(false);
        };
        let enum_name = PhpStr::from_bytes(&s[..colon]);
        let case_name = PhpStr::from_bytes(&s[colon + 1..]);
        if !is_valid_class_name(&enum_name) {
            return Ok(false);
        }
        if self.d.borrow().incomplete_classes_only {
            // Keep the case name, so that the value serializes back to the
            // same bytes.
            self.pos = end + 2;
            let ic = vm
                .find_class(INCOMPLETE_CLASS)
                .expect("__PHP_Incomplete_Class is registered");
            let obj = object_init_ex(vm, &ic)?;
            store_class_name(&obj, &enum_name);
            set_member(&obj, MAGIC_MEMBER_ENUM_CASE, Value::String(case_name));
            set(rval, Value::Object(obj));
            return Ok(true);
        }
        let Some(ce) = with_lock(vm, |vm| vm.lookup_class(&enum_name))? else {
            vm.warning(format!("Class '{}' not found", c_text(&enum_name)))?;
            return Ok(false);
        };
        if !ce.is_enum() {
            vm.warning(format!("Class '{}' is not an enum", c_text(&enum_name)))?;
            return Ok(false);
        }
        self.pos = end + 2;
        let Some(c) = ce.find_constant(&case_name).cloned() else {
            vm.warning(format!(
                "Undefined constant {}::{}",
                c_text(&enum_name),
                c_text(&case_name)
            ))?;
            return Ok(false);
        };
        if c.flags & member_flags::ENUM_CASE == 0 {
            vm.warning(format!(
                "{}::{} is not an enum case",
                c_text(&enum_name),
                c_text(&case_name)
            ))?;
            return Ok(false);
        }
        let value = with_lock(vm, |vm| vm.class_constant(&ce.name, &case_name))?;
        match value.map(Value::into_unref) {
            Some(v @ Value::Object(_)) => {
                set(rval, v);
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// `var_restore_prop_default`.
fn restore_default(cell: &PhpRef, obj: &ObjRef, slot: usize) {
    let default = obj
        .class()
        .default_properties
        .get(slot)
        .cloned()
        .unwrap_or(Value::Undef);
    set(cell, default);
}

/// `i_zend_check_property_type` without coercion: exact type, class type
/// (`self`/`parent` resolved against the declaring class).
fn type_accepts(info: &PropType, v: &Value) -> bool {
    if info.ty.matches_exact(v) {
        return true;
    }
    let Value::Object(o) = v else {
        return false;
    };
    info.ty.classes.iter().any(|c| match c {
        ClassType::Name(n) if n.eq_ignore_ascii_case(b"self") => {
            o.class().instance_of_name(&info.class)
        }
        ClassType::Name(n) if n.eq_ignore_ascii_case(b"parent") => {
            let mut ce = Some(o.class().clone());
            while let Some(c) = ce {
                if c.name.eq_ignore_ascii_case(&info.class) {
                    return c.parent.as_ref().is_some_and(|p| o.class().instance_of(p));
                }
                ce = c.parent.clone();
            }
            false
        }
        _ => false,
    })
}

/// `zend_verify_property_type_error`.
fn prop_type_error(vm: &mut Vm, info: &PropType, v: &Value) -> Flow {
    let msg = format!(
        "Cannot assign {} to property {}::${} of type {}",
        v.value_name(),
        info.class,
        info.name,
        info.ty.to_type_string()
    );
    vm.throw_type_error(msg)
}

/// Applies the `$options` of `unserialize()` to the context.
fn apply_options(
    vm: &mut Vm,
    d: &Rc<RefCell<UnserializeData>>,
    options: &PhpArray,
    function_name: &str,
) -> PResult<()> {
    let classes = options.get_str(b"allowed_classes").map(Value::unref);
    let class_hash = match &classes {
        None | Some(Value::Bool(true)) => None,
        Some(Value::Bool(false)) => Some(HashSet::new()),
        Some(Value::Array(list)) => {
            let mut set = HashSet::with_capacity(list.len());
            for entry in list.values() {
                let entry = entry.unref();
                if !matches!(entry, Value::String(_) | Value::Object(_)) {
                    let msg = format!(
                        "{function_name}(): Option \"allowed_classes\" must be an array of class \
                         names, {} given",
                        entry.value_name()
                    );
                    return Err(vm.throw_type_error(msg));
                }
                let name = vm.to_php_string(&entry)?;
                if !is_valid_class_name(&name) {
                    let msg = format!(
                        "{function_name}(): Option \"allowed_classes\" must be an array of class \
                         names, \"{}\" given",
                        c_text(&name)
                    );
                    return Err(vm.throw_value_error(msg));
                }
                set.insert(name.to_ascii_lowercase().to_vec());
            }
            Some(set)
        }
        Some(other) => {
            let msg = format!(
                "{function_name}(): Option \"allowed_classes\" must be of type array|bool, {} \
                 given",
                other.value_name()
            );
            return Err(vm.throw_type_error(msg));
        }
    };
    d.borrow_mut().allowed_classes = class_hash.map(Rc::new);

    if let Some(max_depth) = options.get_str(b"max_depth").map(Value::unref) {
        let Value::Int(max_depth) = max_depth else {
            let msg = format!(
                "{function_name}(): Option \"max_depth\" must be of type int, {} given",
                max_depth.value_name()
            );
            return Err(vm.throw_type_error(msg));
        };
        if max_depth < 0 {
            let msg = format!(
                "{function_name}(): Option \"max_depth\" must be greater than or equal to 0"
            );
            return Err(vm.throw_value_error(msg));
        }
        let mut d = d.borrow_mut();
        d.max_depth = max_depth;
        // A nested call with its own max_depth counts from zero again.
        d.cur_depth = 0;
    }
    Ok(())
}

/// `php_unserialize_with_options`: `unserialize()` with its warnings,
/// returning `false` for malformed data.
pub fn unserialize_with_options(
    vm: &mut Vm,
    buf: &[u8],
    options: Option<&PhpArray>,
    function_name: &str,
) -> PResult<Value> {
    let d = var_unserialize_init(vm);
    let prev = {
        let d = d.borrow();
        (d.allowed_classes.clone(), d.max_depth, d.cur_depth)
    };
    let setup = match options {
        Some(options) => apply_options(vm, &d, options, function_name),
        None => Ok(()),
    };
    let result = setup.and_then(|()| {
        let mut pos = 0;
        match var_unserialize(vm, &d, buf, &mut pos)? {
            None => {
                vm.warning(format!("Error at offset {pos} of {} bytes", buf.len()))?;
                Ok(Value::Bool(false))
            }
            Some(v) => {
                if pos < buf.len() {
                    vm.warning(format!(
                        "Extra data starting at offset {pos} of {} bytes",
                        buf.len()
                    ))?;
                }
                Ok(v)
            }
        }
    });
    // Reset to the previous options in case this is a nested call.
    {
        let mut d = d.borrow_mut();
        d.allowed_classes = prev.0;
        d.max_depth = prev.1;
        d.cur_depth = prev.2;
    }
    let destroyed = destroy(vm, d, result.is_err());
    let value = result?;
    destroyed?;
    // Unwrap the reference only now: delayed __wakeup() calls may have
    // changed the value.
    Ok(value.into_unref())
}
