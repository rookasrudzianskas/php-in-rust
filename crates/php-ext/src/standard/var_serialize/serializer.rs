//! The serializer of ext/standard/var.c (`php_var_serialize_intern` and
//! helpers).
//!
//! Back-references: every value written increments a counter
//! (`php_serialize_data.n`); references and objects are remembered by
//! identity so that later occurrences are written as `R:n;` (PHP
//! reference) or `r:n;` (same object). php-src skips remembering objects
//! whose refcount proves they cannot occur twice; remembering all of them
//! (and keeping them alive) produces the same output.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use php_runtime::class::{PropPurpose, class_flags};
use php_runtime::value::KeyRef;
use php_runtime::value::float_fmt;
use php_runtime::{ArrayKey, ObjRef, PResult, PhpArray, PhpArrayRef, PhpStr, Value, Vm};

use super::incomplete::{
    INCOMPLETE_CLASS, MAGIC_MEMBER, MAGIC_MEMBER_ENUM_CASE, MAGIC_MEMBER_SERIALIZED,
    is_incomplete_class, lookup_class_name, string_member,
};
use super::{c_str, declared_props, globals, is_serializable, mangle, with_lock};

/// `struct php_serialize_data`: the back-reference table of one serialization
/// (shared by nested `serialize()` calls made from `Serializable::serialize()`).
#[derive(Default)]
pub struct SerializeData {
    /// Identity of a reference or object → its number; `-1` marks objects
    /// whose `Serializable::serialize()` failed (written as `N;`).
    ht: HashMap<usize, i64>,
    /// Keeps remembered values alive so that their addresses stay unique.
    keep: Vec<Value>,
    n: i64,
    incomplete_classes_only: bool,
}

/// `PHP_VAR_SERIALIZE_INIT`: joins the active serialization unless the
/// serialize lock is held or none is active.
pub fn var_serialize_init(vm: &mut Vm) -> Rc<RefCell<SerializeData>> {
    let g = globals(vm);
    if g.lock > 0 || g.ser_level == 0 {
        let d = Rc::new(RefCell::new(SerializeData::default()));
        if g.lock == 0 {
            g.ser_data = Some(d.clone());
            g.ser_level = 1;
        }
        d
    } else {
        g.ser_level += 1;
        g.ser_data.clone().expect("active serialization")
    }
}

/// `PHP_VAR_SERIALIZE_DESTROY`.
pub fn var_serialize_destroy(vm: &mut Vm, d: Rc<RefCell<SerializeData>>) {
    let g = globals(vm);
    if g.lock == 0 {
        g.ser_level -= 1;
        if g.ser_level == 0 {
            g.ser_data = None;
        }
    }
    drop(d);
}

/// `php_var_serialize_set_incomplete_classes_only`: write incomplete objects
/// that carry a raw `C:`/`E:` payload back in their original format.
pub fn var_serialize_set_incomplete_classes_only(d: &Rc<RefCell<SerializeData>>, on: bool) {
    d.borrow_mut().incomplete_classes_only = on;
}

/// `php_var_serialize`: appends the serialization of `value` to `buf`.
pub fn var_serialize(
    vm: &mut Vm,
    buf: &mut Vec<u8>,
    value: &Value,
    data: &Rc<RefCell<SerializeData>>,
) -> PResult<()> {
    let mut s = Serializer {
        buf,
        data: data.clone(),
    };
    s.intern(vm, value)
}

/// Identity of an object (`Z_COUNTED_P`).
fn obj_key(o: &ObjRef) -> usize {
    o.obj() as *const _ as usize
}

/// `if (Z_ISREF_P(data) && Z_REFCOUNT_P(data) == 1) data = Z_REFVAL_P(data);`
fn deref_unshared(v: &Value) -> Cow<'_, Value> {
    match v {
        Value::Ref(r) if r.refcount() == 1 => Cow::Owned(r.get()),
        v => Cow::Borrowed(v),
    }
}

struct Serializer<'a> {
    buf: &'a mut Vec<u8>,
    data: Rc<RefCell<SerializeData>>,
}

impl Serializer<'_> {
    fn put(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    fn put_num(&mut self, n: impl std::fmt::Display) {
        self.buf.extend_from_slice(n.to_string().as_bytes());
    }

    /// `php_var_serialize_long`.
    fn long(&mut self, i: i64) {
        self.put(b"i:");
        self.put_num(i);
        self.put(b";");
    }

    /// `php_var_serialize_string`.
    fn string(&mut self, s: &[u8]) {
        self.put(b"s:");
        self.put_num(s.len());
        self.put(b":\"");
        self.put(s);
        self.put(b"\";");
    }

    fn key(&mut self, k: &ArrayKey) {
        match k {
            ArrayKey::Int(i) => self.long(*i),
            ArrayKey::Str(s) => self.string(s),
        }
    }

    /// `php_add_var_hash`: counts `var` and returns the number of its first
    /// occurrence (0 if it is new or not a reference/object).
    fn add_var_hash(&mut self, var: &Value) -> i64 {
        let mut d = self.data.borrow_mut();
        d.n += 1;
        let is_ref = var.is_ref();
        // References to objects are treated as if the reference didn't exist.
        let (key, keep) = match var {
            Value::Ref(r) => match &*r.borrow() {
                Value::Object(o) => (obj_key(o), Value::Object(o.clone())),
                _ => (r.as_ptr() as usize, var.clone()),
            },
            Value::Object(o) => (obj_key(o), var.clone()),
            _ => return 0,
        };
        if let Some(&n) = d.ht.get(&key) {
            // References are only counted once.
            if is_ref && n != -1 {
                d.n -= 1;
            }
            return n;
        }
        let n = d.n;
        d.ht.insert(key, n);
        d.keep.push(keep);
        0
    }

    /// Marks an object whose `Serializable::serialize()` failed, so that
    /// references to it are written as `N;`.
    fn mark_failed(&mut self, obj: &ObjRef) {
        if let Some(n) = self.data.borrow_mut().ht.get_mut(&obj_key(obj)) {
            *n = -1;
        }
    }

    /// `php_var_serialize_intern`.
    fn intern(&mut self, vm: &mut Vm, struc: &Value) -> PResult<()> {
        let already = self.add_var_hash(struc);
        if already != 0 {
            if already == -1 {
                // Reference to an object that failed to serialize.
                self.put(b"N;");
            } else {
                self.put(if struc.is_ref() { b"R:" } else { b"r:" });
                self.put_num(already);
                self.put(b";");
            }
            return Ok(());
        }
        match struc.unref() {
            Value::Bool(false) => self.put(b"b:0;"),
            Value::Bool(true) => self.put(b"b:1;"),
            Value::Null | Value::Undef => self.put(b"N;"),
            Value::Int(i) => self.long(i),
            Value::Float(d) => {
                let s = float_fmt::gcvt(d, vm.serialize_precision as i32, b'E');
                self.put(b"d:");
                self.put(s.as_bytes());
                self.put(b";");
            }
            Value::String(s) => self.string(&s),
            Value::Array(a) => {
                self.put(b"a:");
                self.array_data(vm, &a)?;
            }
            Value::Object(o) => self.object(vm, &o)?,
            Value::Resource(_) | Value::Ref(_) => self.put(b"i:0;"),
        }
        Ok(())
    }

    /// `php_var_serialize_nested_data` for an array.
    fn array_data(&mut self, vm: &mut Vm, arr: &PhpArrayRef) -> PResult<()> {
        // `arr` is a counted handle: the walked table cannot change under us.
        let arr = arr.clone();
        self.put_num(arr.len());
        self.put(b":{");
        for (k, v) in arr.iter() {
            self.key(&k);
            let v = deref_unshared(v);
            if let Value::Array(inner) = &*v {
                if Rc::ptr_eq(inner, &arr) {
                    self.add_var_hash(&Value::Array(arr.clone()));
                    self.put(b"N;");
                    continue;
                }
            }
            self.intern(vm, &v)?;
        }
        self.put(b"}");
        Ok(())
    }

    /// `php_var_serialize_nested_data` for a property table; with
    /// `incomplete_class` the first `__PHP_Incomplete_Class_Name` is skipped.
    fn props_data(
        &mut self,
        vm: &mut Vm,
        props: &[(ArrayKey, Value)],
        count: usize,
        mut incomplete_class: bool,
    ) -> PResult<()> {
        self.put_num(count);
        self.put(b":{");
        if count > 0 {
            for (k, v) in props {
                if incomplete_class && matches!(k, ArrayKey::Str(s) if s.as_bytes() == MAGIC_MEMBER)
                {
                    incomplete_class = false;
                    continue;
                }
                self.key(k);
                let v = deref_unshared(v);
                self.intern(vm, &v)?;
            }
        }
        self.put(b"}");
        Ok(())
    }

    /// `php_var_serialize_class_name`: `O:len:"name":`; returns whether the
    /// object is an `__PHP_Incomplete_Class`.
    fn class_name(&mut self, obj: &ObjRef) -> bool {
        let (name, incomplete) = if is_incomplete_class(obj.class()) {
            let name = lookup_class_name(obj).unwrap_or_else(|| PhpStr::from(INCOMPLETE_CLASS));
            (name, true)
        } else {
            (obj.class().name.clone(), false)
        };
        self.put(b"O:");
        self.put_num(name.len());
        self.put(b":\"");
        self.put(&name);
        self.put(b"\":");
        incomplete
    }

    /// `php_var_serialize_incomplete_raw`: an incomplete object created in
    /// incomplete-classes-only mode is written back in its original format.
    fn incomplete_raw(&mut self, obj: &ObjRef) -> bool {
        if obj.obj().dyn_props.borrow().is_none() {
            return false;
        }
        if let Some(payload) = string_member(obj, MAGIC_MEMBER_SERIALIZED) {
            let Some(class) = lookup_class_name(obj) else {
                return false;
            };
            self.put(b"C:");
            self.put_num(class.len());
            self.put(b":\"");
            self.put(&class);
            self.put(b"\":");
            self.put_num(payload.len());
            self.put(b":{");
            self.put(&payload);
            self.put(b"}");
            return true;
        }
        if let Some(case) = string_member(obj, MAGIC_MEMBER_ENUM_CASE) {
            let Some(class) = lookup_class_name(obj) else {
                return false;
            };
            self.enum_case(&class, &case);
            return true;
        }
        false
    }

    fn enum_case(&mut self, class: &[u8], case: &[u8]) {
        self.put(b"E:");
        self.put_num(class.len() + 1 + case.len());
        self.put(b":\"");
        self.put(class);
        self.put(b":");
        self.put(case);
        self.put(b"\";");
    }

    fn object(&mut self, vm: &mut Vm, obj: &ObjRef) -> PResult<()> {
        let ce = obj.class().clone();
        let incomplete = is_incomplete_class(&ce);
        if incomplete && self.data.borrow().incomplete_classes_only && self.incomplete_raw(obj) {
            return Ok(());
        }
        if ce.has_flag(class_flags::NOT_SERIALIZABLE) {
            let msg = format!("Serialization of '{}' is not allowed", ce.name);
            return Err(vm.throw_error("Exception", msg));
        }
        if ce.is_enum() {
            let case = match obj.get_prop(b"name").map(Value::into_unref) {
                Some(Value::String(s)) => s,
                _ => PhpStr::new(),
            };
            self.enum_case(&ce.name, &case);
            return Ok(());
        }
        if ce.magic.serialize.is_some() || ce.find_method(b"__serialize").is_some() {
            let retval = with_lock(vm, |vm| vm.call_method(obj, b"__serialize", Vec::new()))?;
            let Value::Array(arr) = retval.into_unref() else {
                let msg = format!("{}::__serialize() must return an array", ce.name);
                return Err(vm.throw_type_error(msg));
            };
            self.class_name(obj);
            self.put_num(arr.len());
            self.put(b":{");
            for (k, v) in arr.iter() {
                self.key(&k);
                let v = deref_unshared(v);
                self.intern(vm, &v)?;
            }
            self.put(b"}");
            return Ok(());
        }
        if is_serializable(&ce) {
            return match user_serialize(vm, obj) {
                Ok(Some(payload)) => {
                    self.put(b"C:");
                    self.put_num(ce.name.len());
                    self.put(b":\"");
                    self.put(&ce.name);
                    self.put(b"\":");
                    self.put_num(payload.len());
                    self.put(b":{");
                    self.put(&payload);
                    self.put(b"}");
                    Ok(())
                }
                Ok(None) => {
                    self.mark_failed(obj);
                    self.put(b"N;");
                    Ok(())
                }
                Err(e) => {
                    self.mark_failed(obj);
                    Err(e)
                }
            };
        }
        if !incomplete && ce.find_method(b"__sleep").is_some() {
            let retval = with_lock(vm, |vm| vm.call_method(obj, b"__sleep", Vec::new()))?;
            let Value::Array(names) = retval.into_unref() else {
                vm.warning(format!(
                    "{}::__sleep() should return an array only containing the names of \
                     instance-variables to serialize",
                    ce.name
                ))?;
                // The length of the enclosing container was already written.
                self.put(b"N;");
                return Ok(());
            };
            let props = sleep_props(vm, obj, &names)?;
            self.class_name(obj);
            let count = props.len();
            return self.props_data(vm, &props, count, false);
        }
        let incomplete_class = self.class_name(obj);
        let props = serialize_props(vm, obj)?;
        // php_var_serialize_class_name() decided whether the magic member is
        // excluded from the count.
        let mut count = props.len();
        if count > 0 && incomplete_class {
            count -= 1;
        }
        self.props_data(vm, &props, count, incomplete_class)
    }
}

/// `zend_user_serialize`: `Ok(None)` when `serialize()` returned `null`.
fn user_serialize(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<PhpStr>> {
    let r = vm.call_method(obj, b"serialize", Vec::new())?;
    match r.into_unref() {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s)),
        _ => {
            let msg = format!(
                "{}::serialize() must return a string or NULL",
                obj.class().name
            );
            Err(vm.throw_error("Exception", msg))
        }
    }
}

/// `zend_get_properties_for(obj, ZEND_PROP_PURPOSE_SERIALIZE)` from a
/// class handler, if the class has one.
fn handler_props(vm: &mut Vm, obj: &ObjRef) -> PResult<Option<PhpArray>> {
    match obj.class().handlers.get_properties_for {
        Some(f) => f(vm, obj, PropPurpose::Serialize),
        None => Ok(None),
    }
}

/// The property table to serialize, keyed by mangled names: initialized
/// declared slots in slot order, then dynamic properties.
fn serialize_props(vm: &mut Vm, obj: &ObjRef) -> PResult<Vec<(ArrayKey, Value)>> {
    if let Some(t) = handler_props(vm, obj)? {
        return Ok(t.iter().map(|(k, v)| (k, v.clone())).collect());
    }
    let mut out = Vec::new();
    {
        let slots = obj.obj().props.borrow();
        for d in declared_props(obj) {
            if let Some(v) = slots.get(d.slot) {
                if !v.is_undef() {
                    out.push((ArrayKey::Str(d.key), deref_unshared(v).into_owned()));
                }
            }
        }
    }
    if let Some(dyn_props) = obj.obj().dyn_props.borrow().as_ref() {
        for (k, v) in dyn_props.iter() {
            out.push((k, deref_unshared(v).into_owned()));
        }
    }
    Ok(out)
}

/// Looks a property up by table key for `__sleep()`: the value (`Undef`
/// for an uninitialized or unset declared slot) and whether it is typed.
fn sleep_lookup(
    obj: &ObjRef,
    table: &Option<PhpArray>,
    declared: &[super::DeclaredProp],
    key: &[u8],
) -> Option<(Value, bool)> {
    if let Some(t) = table {
        return t.get_by(KeyRef::Str(key)).map(|v| (v.clone(), false));
    }
    if let Some(d) = declared.iter().find(|d| d.key.as_bytes() == key) {
        let v = obj.obj().props.borrow().get(d.slot).cloned()?;
        return Some((v, d.info.ty.is_some()));
    }
    obj.obj()
        .dyn_props
        .borrow()
        .as_ref()
        .and_then(|t| t.get_by(KeyRef::Str(key)).cloned())
        .map(|v| (v, false))
}

/// `php_var_serialize_get_sleep_props`: resolves the names `__sleep()`
/// returned (plain, then private and protected mangling) to properties.
fn sleep_props(vm: &mut Vm, obj: &ObjRef, names: &PhpArrayRef) -> PResult<Vec<(ArrayKey, Value)>> {
    let ce = obj.class().clone();
    let table = handler_props(vm, obj)?;
    let declared = declared_props(obj);
    let mut out: Vec<(ArrayKey, Value)> = Vec::with_capacity(names.len());
    for name_val in names.values() {
        let name_val = name_val.unref();
        if name_val.is_undef() {
            continue;
        }
        if !matches!(name_val, Value::String(_)) {
            vm.warning(format!(
                "{}::__sleep() should return an array only containing the names of \
                 instance-variables to serialize",
                ce.name
            ))?;
        }
        let name = vm.to_php_string(&name_val)?;
        let candidates = [name.clone(), mangle(&ce.name, &name), mangle(b"*", &name)];
        let mut found = false;
        for key in candidates {
            let Some((val, typed)) = sleep_lookup(obj, &table, &declared, &key) else {
                continue;
            };
            if val.is_undef() {
                // Uninitialized typed properties are skipped; unset untyped
                // ones do not exist.
                if typed {
                    found = true;
                    break;
                }
                continue;
            }
            found = true;
            if out
                .iter()
                .any(|(k, _)| matches!(k, ArrayKey::Str(s) if *s == key))
            {
                let mut msg = b"\"".to_vec();
                msg.extend_from_slice(c_str(&name));
                msg.extend_from_slice(b"\" is returned from __sleep() multiple times");
                vm.warning(PhpStr::from_vec(msg))?;
                break;
            }
            out.push((ArrayKey::Str(key), val));
            break;
        }
        if !found {
            let mut msg = b"\"".to_vec();
            msg.extend_from_slice(c_str(&name));
            msg.extend_from_slice(
                b"\" returned as member variable from __sleep() but does not exist",
            );
            vm.warning(PhpStr::from_vec(msg))?;
        }
    }
    Ok(out)
}
