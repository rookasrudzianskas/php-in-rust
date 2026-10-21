//! The PHP value model (`zval` and friends).
//!
//! * [`Value`] — the 16-byte tagged value (`zval`).
//! * [`PhpStr`] — binary string (`zend_string`).
//! * [`PhpArray`] / [`PhpArrayRef`] — ordered hash map with PHP semantics (`HashTable`).
//! * [`PhpRef`] — reference box (`zend_reference`).
//! * [`ResRef`] — resource handle (`zend_resource`).
//!
//! Conversions live in [`convert`] (pure helpers plus `Vm`-aware variants that
//! emit PHP's warnings), operators in [`ops`], numeric-string parsing in
//! [`numeric`] and float formatting in [`float_fmt`].

pub mod array;
pub mod convert;
pub mod float_fmt;
pub mod numeric;
pub mod ops;
pub mod string;

use std::any::Any;
use std::borrow::Cow;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::fmt;
use std::ops::Deref;
use std::rc::Rc;

pub use array::{ArrayKey, KeyRef, PhpArray, Pos};
pub use string::PhpStr;

use crate::object::ObjRef;

/// Shared, copy-on-write array handle. Mutate through `Rc::make_mut`, which
/// separates (`SEPARATE_ARRAY`) using `PhpArray`'s PHP-aware `Clone`.
pub type PhpArrayRef = Rc<PhpArray>;

/// A PHP value (`zval`).
#[derive(Clone, Default)]
pub enum Value {
    /// Uninitialised slot (unset variable, uninitialised typed property).
    /// Never user-visible; reads usually produce a warning and `null`.
    #[default]
    Undef,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(PhpStr),
    Array(PhpArrayRef),
    Object(ObjRef),
    Resource(ResRef),
    /// A reference slot; never contains another `Ref`.
    Ref(PhpRef),
}

const _: () = assert!(std::mem::size_of::<Value>() == 16);

/// Borrowed view of a value with references transparently dereferenced.
pub enum ValueRef<'a> {
    Direct(&'a Value),
    Shared(Ref<'a, Value>),
}

impl Deref for ValueRef<'_> {
    type Target = Value;
    fn deref(&self) -> &Value {
        match self {
            ValueRef::Direct(v) => v,
            ValueRef::Shared(r) => r,
        }
    }
}

/// The internal type tag of a value (`Z_TYPE`), with `false`/`true` split
/// like php-src so that type-pair dispatch mirrors `zend_operators.c`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum TypeCode {
    Undef = 0,
    Null = 1,
    False = 2,
    True = 3,
    Int = 4,
    Float = 5,
    String = 6,
    Array = 7,
    Object = 8,
    Resource = 9,
    Ref = 10,
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::String(PhpStr::from(s))
    }

    pub fn bytes(b: &[u8]) -> Value {
        Value::String(PhpStr::from_bytes(b))
    }

    pub fn empty_string() -> Value {
        Value::String(PhpStr::new())
    }

    pub fn empty_array() -> Value {
        Value::Array(Rc::new(PhpArray::new()))
    }

    pub fn array(a: PhpArray) -> Value {
        Value::Array(Rc::new(a))
    }

    #[inline]
    pub fn type_code(&self) -> TypeCode {
        match self {
            Value::Undef => TypeCode::Undef,
            Value::Null => TypeCode::Null,
            Value::Bool(false) => TypeCode::False,
            Value::Bool(true) => TypeCode::True,
            Value::Int(_) => TypeCode::Int,
            Value::Float(_) => TypeCode::Float,
            Value::String(_) => TypeCode::String,
            Value::Array(_) => TypeCode::Array,
            Value::Object(_) => TypeCode::Object,
            Value::Resource(_) => TypeCode::Resource,
            Value::Ref(_) => TypeCode::Ref,
        }
    }

    #[inline]
    pub fn is_undef(&self) -> bool {
        matches!(self, Value::Undef)
    }

    /// `null` (or an undefined slot), looking through references.
    pub fn is_null(&self) -> bool {
        match self {
            Value::Null | Value::Undef => true,
            Value::Ref(r) => r.borrow().is_null(),
            _ => false,
        }
    }

    #[inline]
    pub fn is_ref(&self) -> bool {
        matches!(self, Value::Ref(_))
    }

    /// Dereferences a `Ref` for reading.
    #[inline]
    pub fn deref(&self) -> ValueRef<'_> {
        match self {
            Value::Ref(r) => ValueRef::Shared(r.borrow()),
            v => ValueRef::Direct(v),
        }
    }

    /// A copy of the value with references removed (cheap: handles are `Rc`).
    #[inline]
    pub fn unref(&self) -> Value {
        match self {
            Value::Ref(r) => r.get(),
            v => v.clone(),
        }
    }

    /// Like [`Value::unref`] but consuming.
    #[inline]
    pub fn into_unref(self) -> Value {
        match self {
            Value::Ref(r) => r.get(),
            v => v,
        }
    }

    /// Converts `Undef` to `Null` (for user-visible reads).
    pub fn or_null(self) -> Value {
        match self {
            Value::Undef => Value::Null,
            v => v,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&PhpStr> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&PhpArrayRef> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&ObjRef> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_resource(&self) -> Option<&ResRef> {
        match self {
            Value::Resource(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_php_ref(&self) -> Option<&PhpRef> {
        match self {
            Value::Ref(r) => Some(r),
            _ => None,
        }
    }

    /// Type name as used in most error messages (`zend_zval_type_name`):
    /// `null`, `bool`, `int`, `float`, `string`, `array`, `resource`, or the
    /// class name for objects.
    pub fn type_name(&self) -> Cow<'static, str> {
        match &*self.deref() {
            Value::Undef | Value::Null => Cow::Borrowed("null"),
            Value::Bool(_) => Cow::Borrowed("bool"),
            Value::Int(_) => Cow::Borrowed("int"),
            Value::Float(_) => Cow::Borrowed("float"),
            Value::String(_) => Cow::Borrowed("string"),
            Value::Array(_) => Cow::Borrowed("array"),
            Value::Object(o) => Cow::Owned(o.class().name.to_string_lossy().into_owned()),
            Value::Resource(_) => Cow::Borrowed("resource"),
            Value::Ref(_) => unreachable!("references never nest"),
        }
    }

    /// Like [`Value::type_name`] but distinguishes `true`/`false`
    /// (`zend_zval_value_name`, used by "..., X given" messages).
    pub fn value_name(&self) -> Cow<'static, str> {
        match &*self.deref() {
            Value::Bool(true) => Cow::Borrowed("true"),
            Value::Bool(false) => Cow::Borrowed("false"),
            v => v.type_name(),
        }
    }

    /// `gettype()` names (`zend_zval_get_legacy_type`).
    pub fn legacy_type_name(&self) -> &'static str {
        match &*self.deref() {
            Value::Undef | Value::Null => "NULL",
            Value::Bool(_) => "boolean",
            Value::Int(_) => "integer",
            Value::Float(_) => "double",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
            Value::Resource(r) => {
                if r.is_closed() {
                    "resource (closed)"
                } else {
                    "resource"
                }
            }
            Value::Ref(_) => unreachable!("references never nest"),
        }
    }

    /// Identity (`===`), see [`ops::is_identical`].
    pub fn identical(&self, other: &Value) -> bool {
        ops::is_identical(self, other)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Undef => f.write_str("Undef"),
            Value::Null => f.write_str("Null"),
            Value::Bool(b) => write!(f, "Bool({b})"),
            Value::Int(i) => write!(f, "Int({i})"),
            Value::Float(d) => write!(f, "Float({d:?})"),
            Value::String(s) => write!(f, "String({s:?})"),
            Value::Array(a) => write!(f, "Array({:?})", **a),
            Value::Object(o) => write!(f, "Object({}#{})", o.class().name, o.id()),
            Value::Resource(r) => write!(f, "Resource(#{} {})", r.id(), r.type_name()),
            Value::Ref(r) => write!(f, "Ref({:?})", r.borrow()),
        }
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}
impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Int(i as i64)
    }
}
impl From<u32> for Value {
    fn from(i: u32) -> Self {
        Value::Int(i as i64)
    }
}
impl From<usize> for Value {
    fn from(i: usize) -> Self {
        Value::Int(i as i64)
    }
}
impl From<f64> for Value {
    fn from(d: f64) -> Self {
        Value::Float(d)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(PhpStr::from(s))
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(PhpStr::from(s))
    }
}
impl From<Vec<u8>> for Value {
    fn from(s: Vec<u8>) -> Self {
        Value::String(PhpStr::from(s))
    }
}
impl From<&[u8]> for Value {
    fn from(s: &[u8]) -> Self {
        Value::String(PhpStr::from(s))
    }
}
impl From<PhpStr> for Value {
    fn from(s: PhpStr) -> Self {
        Value::String(s)
    }
}
impl From<PhpArray> for Value {
    fn from(a: PhpArray) -> Self {
        Value::Array(Rc::new(a))
    }
}
impl From<PhpArrayRef> for Value {
    fn from(a: PhpArrayRef) -> Self {
        Value::Array(a)
    }
}
impl From<ObjRef> for Value {
    fn from(o: ObjRef) -> Self {
        Value::Object(o)
    }
}
impl From<ResRef> for Value {
    fn from(r: ResRef) -> Self {
        Value::Resource(r)
    }
}
impl From<PhpRef> for Value {
    fn from(r: PhpRef) -> Self {
        Value::Ref(r)
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        match o {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}

/// A PHP reference (`zend_reference`): a shared mutable slot.
///
/// Two variables/elements/properties holding clones of the same `PhpRef`
/// alias each other. A `PhpRef` never contains another `Value::Ref`.
#[derive(Clone)]
pub struct PhpRef(Rc<RefCell<Value>>);

impl PhpRef {
    /// Creates a new reference box; a nested reference is unwrapped so refs never nest.
    pub fn new(v: Value) -> PhpRef {
        PhpRef(Rc::new(RefCell::new(v.into_unref())))
    }

    /// Copy of the referenced value.
    #[inline]
    pub fn get(&self) -> Value {
        self.0.borrow().clone()
    }

    /// Replaces the referenced value (nested references are unwrapped).
    pub fn set(&self, v: Value) -> Value {
        std::mem::replace(&mut *self.0.borrow_mut(), v.into_unref())
    }

    #[inline]
    pub fn borrow(&self) -> Ref<'_, Value> {
        self.0.borrow()
    }

    /// Mutable access. Callers must not store a `Value::Ref` through it.
    #[inline]
    pub fn borrow_mut(&self) -> RefMut<'_, Value> {
        self.0.borrow_mut()
    }

    pub fn ptr_eq(a: &PhpRef, b: &PhpRef) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }

    /// Number of slots sharing this reference (`GC_REFCOUNT`).
    pub fn refcount(&self) -> usize {
        Rc::strong_count(&self.0)
    }

    pub fn as_ptr(&self) -> *const RefCell<Value> {
        Rc::as_ptr(&self.0)
    }
}

impl fmt::Debug for PhpRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PhpRef({:?})", self.0.borrow())
    }
}

/// A resource handle (`zend_resource`).
#[derive(Clone)]
pub struct ResRef(Rc<Resource>);

pub struct Resource {
    id: i64,
    type_name: Cell<&'static str>,
    closed: Cell<bool>,
    /// Extension-specific payload (stream, process handle, ...).
    pub data: RefCell<Option<Box<dyn Any>>>,
}

thread_local! {
    static NEXT_RESOURCE_ID: Cell<i64> = const { Cell::new(1) };
}

impl ResRef {
    /// Allocates a resource with the next id (ids are never reused, like `zend_list_insert`).
    pub fn new(type_name: &'static str, data: Box<dyn Any>) -> ResRef {
        let id = NEXT_RESOURCE_ID.with(|c| {
            let id = c.get();
            c.set(id + 1);
            id
        });
        ResRef(Rc::new(Resource {
            id,
            type_name: Cell::new(type_name),
            closed: Cell::new(false),
            data: RefCell::new(Some(data)),
        }))
    }

    pub fn id(&self) -> i64 {
        self.0.id
    }

    /// The resource type name (`get_resource_type`); `"Unknown"` once closed.
    pub fn type_name(&self) -> &'static str {
        if self.0.closed.get() {
            "Unknown"
        } else {
            self.0.type_name.get()
        }
    }

    pub fn is_closed(&self) -> bool {
        self.0.closed.get()
    }

    /// Closes the resource, dropping its payload.
    pub fn close(&self) -> Option<Box<dyn Any>> {
        self.0.closed.set(true);
        self.0.data.borrow_mut().take()
    }

    pub fn resource(&self) -> &Resource {
        &self.0
    }

    pub fn ptr_eq(a: &ResRef, b: &ResRef) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }
}

impl fmt::Debug for ResRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Resource(#{} {})", self.id(), self.type_name())
    }
}
