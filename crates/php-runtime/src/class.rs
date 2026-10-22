//! Classes and functions (`zend_class_entry`, `zend_function`).
//!
//! This layer is data-only: the interpreter builds [`ClassEntry`] values
//! (declaration, inheritance, linking) and wraps them in `Rc` ([`ClassRef`]).
//! Runtime-mutable parts (static properties, lazily evaluated constants) use
//! interior mutability. Internal classes customise behaviour through
//! [`ObjectHandlers`] (the equivalent of `zend_object_handlers` /
//! `create_object` / `get_iterator`).

use std::any::Any;
use std::cell::{OnceCell, RefCell};
use std::fmt;
use std::rc::Rc;

use crate::native::{NativeFunction, Signature};
use crate::object::ObjRef;
use crate::types::TypeDecl;
use crate::util::{NameMap, lower};
use crate::value::{PhpArray, PhpRef, PhpStr, Value};
use crate::vm::{PResult, Vm};

pub type ClassRef = Rc<ClassEntry>;

/// Class flags (`ZEND_ACC_*` subset relevant to classes).
pub mod class_flags {
    pub const FINAL: u32 = 1 << 0;
    pub const ABSTRACT: u32 = 1 << 1;
    pub const INTERFACE: u32 = 1 << 2;
    pub const TRAIT: u32 = 1 << 3;
    pub const ENUM: u32 = 1 << 4;
    pub const READONLY: u32 = 1 << 5;
    pub const ANON: u32 = 1 << 6;
    /// Declared by an extension (`ZEND_INTERNAL_CLASS`).
    pub const INTERNAL: u32 = 1 << 7;
    /// Dynamic properties allowed without deprecation (`#[AllowDynamicProperties]`).
    pub const ALLOW_DYNAMIC_PROPERTIES: u32 = 1 << 8;
    /// Instances cannot be serialized (`ZEND_ACC_NOT_SERIALIZABLE`).
    pub const NOT_SERIALIZABLE: u32 = 1 << 9;
    /// Instances cannot be created with `new` by userland.
    pub const NO_DYNAMIC_INSTANTIATION: u32 = 1 << 10;
    /// Constants have been evaluated.
    pub const CONSTANTS_UPDATED: u32 = 1 << 11;
    /// Dynamic properties are an Error (`ZEND_ACC_NO_DYNAMIC_PROPERTIES`):
    /// "Cannot create dynamic property X::$y".
    pub const NO_DYNAMIC_PROPERTIES: u32 = 1 << 12;
    /// A user class whose inheritance has been resolved (`ZEND_ACC_LINKED`).
    pub const LINKED: u32 = 1 << 13;
}

/// Member flags (visibility etc.) shared by methods, properties and constants.
pub mod member_flags {
    pub const PUBLIC: u32 = 1 << 0;
    pub const PROTECTED: u32 = 1 << 1;
    pub const PRIVATE: u32 = 1 << 2;
    pub const PPP_MASK: u32 = PUBLIC | PROTECTED | PRIVATE;
    pub const STATIC: u32 = 1 << 3;
    pub const FINAL: u32 = 1 << 4;
    pub const ABSTRACT: u32 = 1 << 5;
    pub const READONLY: u32 = 1 << 6;
    /// Asymmetric visibility for writes (`public private(set)`).
    pub const PUBLIC_SET: u32 = 1 << 7;
    pub const PROTECTED_SET: u32 = 1 << 8;
    pub const PRIVATE_SET: u32 = 1 << 9;
    /// Property without backing storage (hooked, virtual).
    pub const VIRTUAL: u32 = 1 << 10;
    pub const DEPRECATED: u32 = 1 << 11;
    /// Promoted constructor property.
    pub const PROMOTED: u32 = 1 << 12;
    /// Enum case constant.
    pub const ENUM_CASE: u32 = 1 << 13;
    /// A redeclared property/method shadowing a parent's private member (`ZEND_ACC_CHANGED`).
    pub const CHANGED: u32 = 1 << 14;
    /// `#[\Override]`.
    pub const OVERRIDE: u32 = 1 << 15;
}

/// Function flags (`ZEND_ACC_*` subset relevant to functions).
pub mod fn_flags {
    pub use super::member_flags::{ABSTRACT, FINAL, PRIVATE, PROTECTED, PUBLIC, STATIC};
    pub const RETURNS_REF: u32 = 1 << 16;
    pub const VARIADIC: u32 = 1 << 17;
    pub const DEPRECATED: u32 = 1 << 18;
    pub const CLOSURE: u32 = 1 << 19;
    pub const GENERATOR: u32 = 1 << 20;
    pub const STRICT_TYPES: u32 = 1 << 21;
    pub const CTOR: u32 = 1 << 22;
    pub const HAS_RETURN_TYPE: u32 = 1 << 23;
    /// Calls to this function are not allowed to be compile-time evaluated / cached.
    pub const NEVER_CACHE: u32 = 1 << 24;
    pub const NODISCARD: u32 = 1 << 25;
    /// Internal function accepting extra named arguments in its variadic
    /// (`Z_PARAM_VARIADIC_WITH_NAMED`, e.g. `call_user_func`).
    pub const NAMED_VARIADIC: u32 = 1 << 26;
}

/// How a function is implemented.
#[derive(Clone)]
pub enum FunctionKind {
    /// A userland function; the payload is the interpreter's lowered body.
    User(Rc<dyn Any>),
    /// A built-in function.
    Native(&'static NativeFunction),
}

/// A function or method (`zend_function`).
pub struct Function {
    /// Name as declared (original case).
    pub name: PhpStr,
    pub kind: FunctionKind,
    pub flags: u32,
    /// Declaring class (original case) for methods.
    pub scope: Option<PhpStr>,
    /// Parsed signature; native functions parse their `sig` lazily.
    sig: OnceCell<Rc<Signature>>,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<dyn Any>>,
    /// `(file, line_start, line_end)` for user functions.
    pub location: Option<(PhpStr, u32, u32)>,
}

/// Methods are functions with a scope and visibility flags.
pub type Method = Function;

/// `#[\Deprecated(message: ..., since: ...)]` of a built-in function, stored in
/// [`Function::attributes`] by [`crate::Registry::deprecated_function`].
#[derive(Clone, Debug)]
pub struct DeprecationNote {
    pub since: &'static str,
    pub message: &'static str,
}

impl DeprecationNote {
    /// The message suffix (`get_deprecation_suffix_from_attribute`):
    /// `" since 8.1"`, `", use X instead"`, `" since 8.1, use X instead"`.
    pub fn suffix(&self) -> String {
        let mut s = String::new();
        if !self.since.is_empty() {
            s.push_str(" since ");
            s.push_str(self.since);
        }
        if !self.message.is_empty() {
            s.push_str(", ");
            s.push_str(self.message);
        }
        s
    }
}

/// Turns a stub-style deprecation description (`"since 8.4, the error level
/// was removed"` or just a message) into the suffix php-src appends after
/// "is deprecated".
pub fn deprecation_suffix_text(desc: &str) -> String {
    let d = desc.trim();
    if d.is_empty() {
        String::new()
    } else if let Some(rest) = d.strip_prefix("since ") {
        format!(" since {rest}")
    } else {
        format!(", {d}")
    }
}

impl Function {
    /// A native function; the name comes from its stub signature.
    pub fn native(nf: &'static NativeFunction) -> Function {
        Function {
            name: PhpStr::from(nf.name()),
            kind: FunctionKind::Native(nf),
            flags: member_flags::PUBLIC,
            scope: None,
            sig: OnceCell::new(),
            doc_comment: None,
            attributes: Vec::new(),
            location: None,
        }
    }

    /// A native method of `scope` with member `flags`.
    pub fn native_method(nf: &'static NativeFunction, scope: &str, flags: u32) -> Function {
        Function {
            scope: Some(PhpStr::from(scope)),
            flags,
            ..Function::native(nf)
        }
    }

    /// A user function with an interpreter payload and a prepared signature.
    pub fn user(name: PhpStr, body: Rc<dyn Any>, sig: Rc<Signature>, flags: u32) -> Function {
        let f = Function {
            name,
            kind: FunctionKind::User(body),
            flags,
            scope: None,
            sig: OnceCell::new(),
            doc_comment: None,
            attributes: Vec::new(),
            location: None,
        };
        let _ = f.sig.set(sig);
        f
    }

    /// The parsed signature (arginfo).
    pub fn signature(&self) -> &Rc<Signature> {
        self.sig.get_or_init(|| match &self.kind {
            FunctionKind::Native(nf) => Rc::new(
                Signature::parse(nf.sig)
                    .unwrap_or_else(|e| panic!("invalid native signature {:?}: {e}", nf.sig)),
            ),
            FunctionKind::User(_) => {
                Rc::new(Signature::empty(self.name.to_string_lossy().as_ref()))
            }
        })
    }

    pub fn set_signature(&self, sig: Rc<Signature>) -> Result<(), Rc<Signature>> {
        self.sig.set(sig)
    }

    pub fn is_native(&self) -> bool {
        matches!(self.kind, FunctionKind::Native(_))
    }

    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    pub fn is_static(&self) -> bool {
        self.has_flag(member_flags::STATIC)
    }

    pub fn visibility(&self) -> u32 {
        self.flags & member_flags::PPP_MASK
    }

    /// The deprecation note of a built-in function, if any.
    pub fn deprecation_note(&self) -> Option<&DeprecationNote> {
        self.attributes
            .iter()
            .find_map(|a| a.downcast_ref::<DeprecationNote>())
    }

    /// `zend_deprecated_function` message: "Function f() is deprecated since 8.1".
    pub fn deprecation_message(&self) -> String {
        let suffix = self
            .deprecation_note()
            .map(DeprecationNote::suffix)
            .unwrap_or_default();
        match &self.scope {
            Some(c) => format!("Method {c}::{}() is deprecated{suffix}", self.name),
            None => format!("Function {}() is deprecated{suffix}", self.name),
        }
    }

    /// `Class::method` or `function` as used in error messages
    /// (`get_function_or_method_name`).
    pub fn display_name(&self) -> String {
        match &self.scope {
            Some(c) => format!("{c}::{}", self.name),
            None => self.name.to_string(),
        }
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Function({})", self.display_name())
    }
}

/// A declared property (`zend_property_info`).
#[derive(Clone)]
pub struct PropertyInfo {
    pub name: PhpStr,
    pub flags: u32,
    pub ty: Option<TypeDecl>,
    /// Default value; `Undef` for typed properties without default (uninitialised).
    pub default: Value,
    /// Slot in the object's `props` (instance) or the class's `static_members` (static).
    pub slot: usize,
    /// Declaring class (original case).
    pub declaring_class: PhpStr,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<dyn Any>>,
    /// Property hooks (interpreter-defined payload).
    pub hooks: Option<Rc<dyn Any>>,
}

impl PropertyInfo {
    pub fn new(
        name: impl Into<PhpStr>,
        declaring_class: impl Into<PhpStr>,
        flags: u32,
    ) -> PropertyInfo {
        PropertyInfo {
            name: name.into(),
            flags,
            ty: None,
            default: Value::Null,
            slot: 0,
            declaring_class: declaring_class.into(),
            doc_comment: None,
            attributes: Vec::new(),
            hooks: None,
        }
    }

    pub fn is_static(&self) -> bool {
        self.flags & member_flags::STATIC != 0
    }

    pub fn is_readonly(&self) -> bool {
        self.flags & member_flags::READONLY != 0
    }

    pub fn visibility(&self) -> u32 {
        self.flags & member_flags::PPP_MASK
    }
}

/// The value of a class constant, possibly not yet evaluated.
#[derive(Clone)]
pub enum ConstValue {
    Value(Value),
    /// Constant expression to be evaluated by the interpreter on first access.
    Unevaluated(Rc<dyn Any>),
    /// Being evaluated (recursion detection: "Cannot declare self-referencing constant").
    Evaluating,
}

/// A class constant or enum case (`zend_class_constant`).
pub struct ClassConstant {
    pub name: PhpStr,
    pub value: RefCell<ConstValue>,
    pub flags: u32,
    pub declaring_class: PhpStr,
    pub ty: Option<TypeDecl>,
    pub doc_comment: Option<PhpStr>,
    pub attributes: Vec<Rc<dyn Any>>,
}

impl ClassConstant {
    pub fn new(
        name: impl Into<PhpStr>,
        value: Value,
        declaring_class: impl Into<PhpStr>,
    ) -> ClassConstant {
        ClassConstant {
            name: name.into(),
            value: RefCell::new(ConstValue::Value(value)),
            flags: member_flags::PUBLIC,
            declaring_class: declaring_class.into(),
            ty: None,
            doc_comment: None,
            attributes: Vec::new(),
        }
    }

    /// The value if already evaluated.
    pub fn evaluated(&self) -> Option<Value> {
        match &*self.value.borrow() {
            ConstValue::Value(v) => Some(v.clone()),
            _ => None,
        }
    }
}

/// Target of an object cast (`cast_object` handler types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CastKind {
    Bool,
    Int,
    Float,
    String,
    /// `_IS_NUMBER`: int or float (arithmetic).
    Number,
}

/// Binary/unary operators passed to the `do_operation` handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    ShiftLeft,
    ShiftRight,
    BitOr,
    BitAnd,
    BitXor,
    BitNot,
    BoolXor,
    BoolNot,
}

/// Purpose of a property-table request (`zend_prop_purpose`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropPurpose {
    Debug,
    ArrayCast,
    Serialize,
    VarExport,
    Json,
    GetObjectVars,
}

pub type CreateObjectFn = fn(&mut Vm, &ClassRef) -> PResult<ObjRef>;
pub type CloneObjectFn = fn(&mut Vm, &ObjRef) -> PResult<ObjRef>;
/// Returns `Ok(None)` when the cast is not supported (FAILURE).
pub type CastObjectFn = fn(&mut Vm, &ObjRef, CastKind) -> PResult<Option<Value>>;
/// `compare` handler: receives the two operands (at least one is an object of this class).
pub type CompareFn = fn(&mut Vm, &Value, &Value) -> PResult<i32>;
/// `do_operation`: `Ok(None)` means "not handled".
pub type DoOperationFn = fn(&mut Vm, OpKind, &Value, &Value) -> PResult<Option<Value>>;
pub type CountElementsFn = fn(&mut Vm, &ObjRef) -> PResult<Option<i64>>;
pub type PropertiesForFn = fn(&mut Vm, &ObjRef, PropPurpose) -> PResult<Option<PhpArray>>;
pub type ReadDimensionFn = fn(&mut Vm, &ObjRef, Option<&Value>, DimFetch) -> PResult<Value>;
pub type WriteDimensionFn = fn(&mut Vm, &ObjRef, Option<&Value>, Value) -> PResult<()>;
pub type HasDimensionFn = fn(&mut Vm, &ObjRef, &Value, bool) -> PResult<bool>;
pub type UnsetDimensionFn = fn(&mut Vm, &ObjRef, &Value) -> PResult<()>;
pub type ReadPropertyFn = fn(&mut Vm, &ObjRef, &PhpStr) -> PResult<Option<Value>>;
pub type WritePropertyFn = fn(&mut Vm, &ObjRef, &PhpStr, Value) -> PResult<bool>;
pub type GetIteratorFn = fn(&mut Vm, &ObjRef, bool) -> PResult<Rc<dyn Any>>;
pub type DtorFn = fn(&mut Vm, &ObjRef) -> PResult<()>;
pub type GetClosureFn = fn(&mut Vm, &ObjRef) -> PResult<Option<Value>>;
/// `has_property`: `Ok(None)` selects the standard behaviour; the bool is `check_empty`.
pub type HasPropertyFn = fn(&mut Vm, &ObjRef, &PhpStr, bool) -> PResult<Option<bool>>;
/// `unset_property`: returns `true` when handled.
pub type UnsetPropertyFn = fn(&mut Vm, &ObjRef, &PhpStr) -> PResult<bool>;

/// What `has_property` checks (`ZEND_PROPERTY_ISSET`, `ZEND_PROPERTY_NOT_EMPTY`,
/// `ZEND_PROPERTY_EXISTS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyCheck {
    /// `isset()`: exists and is not `null`.
    Isset,
    /// `!empty()`: exists and is truthy.
    NotEmpty,
    /// `property_exists()`: exists (even when `null`).
    Exists,
}

/// `has_property` with the check mode: `Ok(None)` selects the standard behaviour.
pub type HasPropertyModeFn = fn(&mut Vm, &ObjRef, &PhpStr, PropertyCheck) -> PResult<Option<bool>>;

/// Result of a `get_property_ptr_ptr` handler.
#[derive(Clone)]
pub enum PropertyPtr {
    /// Use the standard behaviour (`zend_std_get_property_ptr_ptr`).
    Std,
    /// No direct pointer (php-src's `NULL`): the engine falls back to
    /// `read_property` (writes into the result are temporaries) and, for
    /// compound assignments, `write_property`.
    Null,
    /// A direct reference to the property value.
    Ref(PhpRef),
}

/// `get_property_ptr_ptr`: called for write fetches (`$o->p[] = ..`,
/// `$o->p->q = ..`, `&$o->p`, `$o->p =& $r`). An `Err` (for example
/// "Cannot modify readonly property") aborts the operation.
pub type GetPropertyPtrFn = fn(&mut Vm, &ObjRef, &PhpStr) -> PResult<PropertyPtr>;
/// `get_method` + call: `Ok(None)` selects the standard method lookup.
pub type CallMethodFn = fn(&mut Vm, &ObjRef, &PhpStr, &[Value]) -> PResult<Option<Value>>;

/// An iterator produced by [`ObjectHandlers::get_iterator`]. The handler
/// returns it as `Rc<RefCell<Box<dyn InternalIterator>>>` inside the
/// `Rc<dyn Any>` (`zend_object_iterator`), and `foreach` drives it.
pub trait InternalIterator {
    fn rewind(&mut self, vm: &mut Vm) -> PResult<()>;
    fn valid(&mut self, vm: &mut Vm) -> PResult<bool>;
    fn current(&mut self, vm: &mut Vm) -> PResult<Value>;
    fn key(&mut self, vm: &mut Vm) -> PResult<Value>;
    fn next(&mut self, vm: &mut Vm) -> PResult<()>;
}

/// Fetch mode for `read_dimension` (`BP_VAR_R`, `BP_VAR_IS`, `BP_VAR_W`, `BP_VAR_RW`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DimFetch {
    Read,
    IsSet,
    Write,
    ReadWrite,
}

/// Hooks for internal classes (`zend_object_handlers` and `zend_class_entry`
/// callbacks). `None` selects the standard behaviour. Subclasses inherit the
/// handlers of their parent (the interpreter copies them when linking).
#[derive(Clone, Copy, Default)]
pub struct ObjectHandlers {
    pub create_object: Option<CreateObjectFn>,
    pub clone_obj: Option<CloneObjectFn>,
    pub cast_object: Option<CastObjectFn>,
    pub compare: Option<CompareFn>,
    pub do_operation: Option<DoOperationFn>,
    pub count_elements: Option<CountElementsFn>,
    pub get_debug_info: Option<PropertiesForFn>,
    pub get_properties_for: Option<PropertiesForFn>,
    pub read_dimension: Option<ReadDimensionFn>,
    pub write_dimension: Option<WriteDimensionFn>,
    pub has_dimension: Option<HasDimensionFn>,
    pub unset_dimension: Option<UnsetDimensionFn>,
    pub read_property: Option<ReadPropertyFn>,
    pub write_property: Option<WritePropertyFn>,
    pub get_iterator: Option<GetIteratorFn>,
    /// Internal destructor, run in addition to (before) a userland `__destruct`.
    pub dtor_obj: Option<DtorFn>,
    pub get_closure: Option<GetClosureFn>,
    pub has_property: Option<HasPropertyFn>,
    pub unset_property: Option<UnsetPropertyFn>,
    /// `has_property` with the check mode; takes precedence over `has_property`.
    pub has_property_mode: Option<HasPropertyModeFn>,
    pub get_property_ptr_ptr: Option<GetPropertyPtrFn>,
    /// Intercepts `$obj->method(...)` calls (e.g. `__PHP_Incomplete_Class`).
    pub call_method: Option<CallMethodFn>,
}

/// Cached magic methods (`ce->constructor`, `ce->__get`, ...), filled by
/// [`ClassEntry::link_magic_methods`].
#[derive(Clone, Default)]
pub struct MagicMethods {
    pub constructor: Option<Rc<Method>>,
    pub destructor: Option<Rc<Method>>,
    pub clone: Option<Rc<Method>>,
    pub get: Option<Rc<Method>>,
    pub set: Option<Rc<Method>>,
    pub isset: Option<Rc<Method>>,
    pub unset: Option<Rc<Method>>,
    pub call: Option<Rc<Method>>,
    pub call_static: Option<Rc<Method>>,
    pub tostring: Option<Rc<Method>>,
    pub serialize: Option<Rc<Method>>,
    pub unserialize: Option<Rc<Method>>,
    pub debug_info: Option<Rc<Method>>,
    pub invoke: Option<Rc<Method>>,
}

/// Backing type of an enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnumBacking {
    Pure,
    Int,
    String,
}

/// A class, interface, trait or enum (`zend_class_entry`).
pub struct ClassEntry {
    /// Name as declared.
    pub name: PhpStr,
    /// Lowercased name (class-table key).
    pub lc_name: PhpStr,
    pub flags: u32,
    pub parent: Option<ClassRef>,
    /// All implemented interfaces, including inherited ones (resolved).
    pub interfaces: Vec<ClassRef>,
    /// Constants and enum cases in declaration order (case-sensitive names).
    pub constants: NameMap<Rc<ClassConstant>>,
    /// Declared properties in declaration order, inherited ones included.
    pub properties: NameMap<PropertyInfo>,
    /// Default values of the instance property slots.
    pub default_properties: Vec<Value>,
    /// Static property storage; inherited statics share the parent's `PhpRef`.
    pub static_members: RefCell<Vec<PhpRef>>,
    /// Methods keyed by lowercase name, inherited ones included.
    pub methods: NameMap<Rc<Method>>,
    pub magic: MagicMethods,
    pub handlers: ObjectHandlers,
    pub enum_backing: Option<EnumBacking>,
    pub attributes: Vec<Rc<dyn Any>>,
    pub doc_comment: Option<PhpStr>,
    /// `(file, line_start, line_end)` for user classes.
    pub location: Option<(PhpStr, u32, u32)>,
    /// Extension that declared the class (internal classes).
    pub module: Option<&'static str>,
    /// Interpreter-specific payload (trait info, compiled defaults, ...).
    pub ext: Option<Rc<dyn Any>>,
}

impl ClassEntry {
    /// A new empty class definition.
    pub fn new(name: impl Into<PhpStr>, flags: u32) -> ClassEntry {
        let name = name.into();
        ClassEntry {
            lc_name: lower(&name),
            name,
            flags,
            parent: None,
            interfaces: Vec::new(),
            constants: NameMap::new(),
            properties: NameMap::new(),
            default_properties: Vec::new(),
            static_members: RefCell::new(Vec::new()),
            methods: NameMap::new(),
            magic: MagicMethods::default(),
            handlers: ObjectHandlers::default(),
            enum_backing: None,
            attributes: Vec::new(),
            doc_comment: None,
            location: None,
            module: None,
            ext: None,
        }
    }

    /// Starts a subclass definition inheriting members and handlers from `parent`.
    pub fn new_subclass(name: impl Into<PhpStr>, flags: u32, parent: &ClassRef) -> ClassEntry {
        let mut ce = ClassEntry::new(name, flags);
        ce.parent = Some(parent.clone());
        ce.interfaces = parent.interfaces.clone();
        ce.constants = parent.constants.clone();
        ce.properties = parent.properties.clone();
        ce.default_properties = parent.default_properties.clone();
        *ce.static_members.borrow_mut() = parent.static_members.borrow().clone();
        ce.methods = parent.methods.clone();
        ce.magic = parent.magic.clone();
        ce.handlers = parent.handlers;
        ce
    }

    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }

    pub fn is_interface(&self) -> bool {
        self.has_flag(class_flags::INTERFACE)
    }

    pub fn is_trait(&self) -> bool {
        self.has_flag(class_flags::TRAIT)
    }

    pub fn is_enum(&self) -> bool {
        self.has_flag(class_flags::ENUM)
    }

    pub fn is_abstract(&self) -> bool {
        self.has_flag(class_flags::ABSTRACT)
    }

    pub fn is_internal(&self) -> bool {
        self.has_flag(class_flags::INTERNAL)
    }

    /// Whether objects of this class need destructor processing.
    pub fn has_destructor(&self) -> bool {
        self.magic.destructor.is_some() || self.handlers.dtor_obj.is_some()
    }

    /// `instanceof_function`: same class, a parent, or an implemented interface.
    pub fn instance_of(&self, other: &ClassEntry) -> bool {
        if std::ptr::eq(self, other) || self.lc_name == other.lc_name {
            return true;
        }
        if other.is_interface() {
            return self.interfaces.iter().any(|i| i.lc_name == other.lc_name)
                || (self.is_interface() && self.lc_name == other.lc_name);
        }
        let mut p = self.parent.as_ref();
        while let Some(c) = p {
            if c.lc_name == other.lc_name {
                return true;
            }
            p = c.parent.as_ref();
        }
        false
    }

    /// `instanceof` by (any case) class name.
    pub fn instance_of_name(&self, name: &[u8]) -> bool {
        let lc = name.to_ascii_lowercase();
        let lc = lc.strip_prefix(b"\\").unwrap_or(&lc);
        if self.lc_name.as_bytes() == lc {
            return true;
        }
        if self.interfaces.iter().any(|i| i.lc_name.as_bytes() == lc) {
            return true;
        }
        let mut p = self.parent.as_ref();
        while let Some(c) = p {
            if c.lc_name.as_bytes() == lc {
                return true;
            }
            p = c.parent.as_ref();
        }
        false
    }

    /// Method lookup by (any case) name.
    pub fn find_method(&self, name: &[u8]) -> Option<&Rc<Method>> {
        if name.iter().any(|c| c.is_ascii_uppercase()) {
            self.methods.get(&name.to_ascii_lowercase())
        } else {
            self.methods.get(name)
        }
    }

    pub fn find_property(&self, name: &[u8]) -> Option<&PropertyInfo> {
        self.properties.get(name)
    }

    pub fn find_constant(&self, name: &[u8]) -> Option<&Rc<ClassConstant>> {
        self.constants.get(name)
    }

    /// Adds (or overrides) a method keyed by its lowercase name.
    pub fn add_method(&mut self, m: Method) -> Rc<Method> {
        let rc = Rc::new(m);
        self.methods.insert(lower(&rc.name), rc.clone());
        rc
    }

    /// Declares an instance or static property, allocating its slot.
    /// Redeclaring an inherited property reuses the inherited slot.
    pub fn declare_property(&mut self, mut info: PropertyInfo) {
        if let Some(existing) = self.properties.get(&info.name) {
            let shadows_private = existing.flags & member_flags::PRIVATE != 0
                && !existing
                    .declaring_class
                    .eq_ignore_ascii_case(&info.declaring_class);
            if existing.is_static() == info.is_static() && !shadows_private {
                info.slot = existing.slot;
                if info.is_static() {
                    self.static_members.borrow_mut()[info.slot] = PhpRef::new(info.default.clone());
                } else {
                    self.default_properties[info.slot] = info.default.clone();
                }
                self.properties.insert(info.name.clone(), info);
                return;
            }
        }
        if info.is_static() {
            let mut sm = self.static_members.borrow_mut();
            info.slot = sm.len();
            sm.push(PhpRef::new(info.default.clone()));
        } else {
            info.slot = self.default_properties.len();
            self.default_properties.push(info.default.clone());
        }
        self.properties.insert(info.name.clone(), info);
    }

    pub fn add_constant(&mut self, c: ClassConstant) {
        self.constants.insert(c.name.clone(), Rc::new(c));
    }

    /// Fills [`ClassEntry::magic`] from the method table.
    pub fn link_magic_methods(&mut self) {
        let get = |n: &str| self.methods.get(n.as_bytes()).cloned();
        let mut m = MagicMethods {
            constructor: get("__construct"),
            destructor: get("__destruct"),
            clone: get("__clone"),
            get: get("__get"),
            set: get("__set"),
            isset: get("__isset"),
            unset: get("__unset"),
            call: get("__call"),
            call_static: get("__callstatic"),
            tostring: get("__tostring"),
            serialize: get("__serialize"),
            unserialize: get("__unserialize"),
            debug_info: get("__debuginfo"),
            invoke: get("__invoke"),
        };
        if m.constructor.is_none() {
            m.constructor = self.magic.constructor.clone();
        }
        self.magic = m;
    }

    /// Kind keyword for messages (`zend_get_object_type`): "class", "interface", "trait", "enum".
    pub fn kind_name(&self) -> &'static str {
        if self.is_interface() {
            "interface"
        } else if self.is_trait() {
            "trait"
        } else if self.is_enum() {
            "enum"
        } else {
            "class"
        }
    }
}

impl fmt::Debug for ClassEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ClassEntry({})", self.name)
    }
}
