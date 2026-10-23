//! Enums (`Zend/zend_enum.c`): the `UnitEnum`/`BackedEnum` interfaces, the
//! `cases()`/`from()`/`tryFrom()` methods, and lazily created case objects
//! shared by userland and internal enums.
//!
//! An enum case is a class constant flagged [`member_flags::ENUM_CASE`]
//! whose value starts as `ConstValue::Unevaluated(EnumCaseInit)`; the case
//! object is created on first access ([`crate::Vm::class_constant`],
//! [`enum_case`], [`enum_cases`]), so registering internal enums does not
//! consume object handles at startup.
//!
//! Registering an internal enum from an extension:
//! ```ignore
//! r.enum_class("RoundingMode", EnumBacking::Pure,
//!     &[("HalfAwayFromZero", None), ("HalfTowardsZero", None)], &[]);
//! // or build it, add methods/constants, then register:
//! let mut ce = r.new_enum("Random\\IntervalBoundary", EnumBacking::Pure, &[...], &[]);
//! ce.add_method(...);
//! r.class(ce);
//! ```

use std::rc::Rc;

use crate::class::{
    ClassConstant, ClassEntry, ClassRef, ConstValue, EnumBacking, Function, PropertyInfo, class_flags,
    member_flags,
};
use crate::native::{Args, NativeFunction, Registry};
use crate::object::ObjRef;
use crate::types::TypeDecl;
use crate::value::{PhpArray, PhpStr, Value};
use crate::vm::{PResult, Vm};

/// Lazy initializer of an enum case constant.
#[derive(Clone, Debug)]
pub struct EnumCaseInit {
    pub name: PhpStr,
    /// Backing value (already evaluated), `None` for pure enums or when the
    /// value is a constant expression the interpreter evaluates
    /// ([`EnumCaseInit::expr`]).
    pub value: Option<Value>,
    /// Interpreter payload for a backing value that is a constant expression.
    pub expr: Option<Rc<dyn std::any::Any>>,
}

/// Declares the `name` (and `value`) properties of an enum class.
pub fn declare_enum_props(ce: &mut ClassEntry, backing: EnumBacking) {
    let class = ce.name.clone();
    let mut name = PropertyInfo::new("name", class.clone(), member_flags::PUBLIC | member_flags::READONLY);
    name.ty = Some(TypeDecl::parse("string").expect("type"));
    name.default = Value::Undef;
    ce.declare_property(name);
    if backing != EnumBacking::Pure {
        let mut value = PropertyInfo::new("value", class, member_flags::PUBLIC | member_flags::READONLY);
        value.ty = Some(
            TypeDecl::parse(if backing == EnumBacking::Int { "int" } else { "string" }).expect("type"),
        );
        value.default = Value::Undef;
        ce.declare_property(value);
    }
}

/// Adds a case constant to an enum class being built.
pub fn add_case(ce: &mut ClassEntry, name: &str, value: Option<Value>) {
    let init = EnumCaseInit {
        name: PhpStr::from(name),
        value,
        expr: None,
    };
    let c = ClassConstant {
        name: PhpStr::from(name),
        value: std::cell::RefCell::new(ConstValue::Unevaluated(Rc::new(init))),
        flags: member_flags::PUBLIC | member_flags::ENUM_CASE,
        declaring_class: ce.name.clone(),
        ty: None,
        doc_comment: None,
        attributes: Vec::new(),
    };
    ce.add_constant(c);
}

/// Adds the `cases()` (and `from()`/`tryFrom()`) methods.
pub fn add_enum_methods(ce: &mut ClassEntry, backing: EnumBacking) {
    let scope = ce.name.to_string();
    let flags = member_flags::PUBLIC | member_flags::STATIC;
    ce.add_method(Function::native_method(&CASES, &scope, flags));
    if backing != EnumBacking::Pure {
        ce.add_method(Function::native_method(&FROM, &scope, flags));
        ce.add_method(Function::native_method(&TRY_FROM, &scope, flags));
    }
}

/// Creates the object for a case (`zend_enum_new`).
pub fn new_case_object(class: &ClassRef, init: &EnumCaseInit, value: Option<Value>) -> ObjRef {
    let obj = ObjRef::new(class.clone());
    obj.set_prop(b"name", Value::String(init.name.clone()));
    if let Some(v) = value.or_else(|| init.value.clone()) {
        obj.set_prop(b"value", v);
    }
    obj
}

/// The case object `Class::NAME` (`zend_enum_get_case`), created on first use.
pub fn enum_case(vm: &mut Vm, class: &ClassRef, name: &[u8]) -> PResult<Option<ObjRef>> {
    match class.find_constant(name) {
        Some(c) if c.flags & member_flags::ENUM_CASE != 0 => {}
        _ => return Ok(None),
    }
    match vm.class_constant(class.name.as_bytes(), name)? {
        Some(Value::Object(o)) => Ok(Some(o)),
        _ => Ok(None),
    }
}

/// All case objects in declaration order (`cases()`).
pub fn enum_cases(vm: &mut Vm, class: &ClassRef) -> PResult<Vec<ObjRef>> {
    let names: Vec<PhpStr> = class
        .constants
        .values()
        .filter(|c| c.flags & member_flags::ENUM_CASE != 0)
        .map(|c| c.name.clone())
        .collect();
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        if let Some(o) = enum_case(vm, class, &n)? {
            out.push(o);
        }
    }
    Ok(out)
}

/// The case name of an enum object.
pub fn enum_case_name(obj: &ObjRef) -> Option<PhpStr> {
    if !obj.class().is_enum() {
        return None;
    }
    match obj.get_prop(b"name") {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

/// The backing value of a backed enum case object.
pub fn enum_backing_value(obj: &ObjRef) -> Option<Value> {
    if !obj.class().is_enum() {
        return None;
    }
    obj.get_prop(b"value")
}

fn called_enum(vm: &mut Vm, args: &Args) -> PResult<ClassRef> {
    if let Some(c) = args.scope() {
        return Ok(c.clone());
    }
    let scope = args.function().scope.clone().unwrap_or_default();
    match vm.find_class(scope.as_bytes()) {
        Some(c) => Ok(c),
        None => Err(vm.throw_error("Error", "Cannot call enum method without a class")),
    }
}

fn cases(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let class = called_enum(vm, args)?;
    let cases = enum_cases(vm, &class)?;
    Ok(Value::array(PhpArray::from_values(
        cases.into_iter().map(Value::Object).collect(),
    )))
}

fn from_base(vm: &mut Vm, args: &mut Args, try_: bool) -> PResult<Value> {
    let class = called_enum(vm, args)?;
    let fname = format!("{}::{}", class.name, if try_ { "tryFrom" } else { "from" });
    let arg = args.get(0).clone();
    let key = match class.enum_backing {
        Some(EnumBacking::Int) => match &arg {
            Value::Int(i) => Value::Int(*i),
            Value::String(s) => match crate::value::numeric::is_numeric_str(s) {
                Some(crate::value::numeric::Numeric::Int(i)) => Value::Int(i),
                Some(crate::value::numeric::Numeric::Float(d))
                    if crate::value::numeric::is_long_compatible(d, d as i64) =>
                {
                    Value::Int(d as i64)
                }
                _ => {
                    return Err(vm.throw_type_error(format!(
                        "{fname}(): Argument #1 ($value) must be of type int, string given"
                    )));
                }
            },
            other => Value::Int(crate::value::convert::to_int_silent(other)),
        },
        _ => match &arg {
            Value::Int(i) => Value::String(crate::value::convert::long_to_str(*i)),
            Value::String(_) => arg.clone(),
            other => Value::String(vm.to_php_string(other)?),
        },
    };
    for case in enum_cases(vm, &class)? {
        if let Some(v) = case.get_prop(b"value") {
            if crate::value::ops::is_identical(&v, &key) {
                return Ok(Value::Object(case));
            }
        }
    }
    if try_ {
        return Ok(Value::Null);
    }
    let msg = match &key {
        Value::Int(i) => format!("{i} is not a valid backing value for enum {}", class.name),
        Value::String(s) => format!("\"{s}\" is not a valid backing value for enum {}", class.name),
        _ => unreachable!(),
    };
    Err(vm.throw_value_error(msg))
}

fn from(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    from_base(vm, args, false)
}

fn try_from(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    from_base(vm, args, true)
}

pub static CASES: NativeFunction = NativeFunction {
    sig: "public static function cases(): array {}",
    handler: cases,
};
pub static FROM: NativeFunction = NativeFunction {
    sig: "public static function from(int|string $value): static {}",
    handler: from,
};
pub static TRY_FROM: NativeFunction = NativeFunction {
    sig: "public static function tryFrom(int|string $value): ?static {}",
    handler: try_from,
};

/// Abstract interface method signatures.
static UNIT_CASES: NativeFunction = NativeFunction {
    sig: "public static function cases(): array {}",
    handler: cases,
};

/// Registers `UnitEnum` and `BackedEnum` (Core).
pub fn register_interfaces(r: &mut Registry<'_>) {
    let mut unit = ClassEntry::new("UnitEnum", class_flags::INTERFACE);
    unit.add_method(Function::native_method(
        &UNIT_CASES,
        "UnitEnum",
        member_flags::PUBLIC | member_flags::STATIC | member_flags::ABSTRACT,
    ));
    let unit = r.class(unit);
    let mut backed = ClassEntry::new("BackedEnum", class_flags::INTERFACE);
    backed.interfaces.push(unit.clone());
    backed.methods = unit.methods.clone();
    for nf in [&FROM, &TRY_FROM] {
        backed.add_method(Function::native_method(
            nf,
            "BackedEnum",
            member_flags::PUBLIC | member_flags::STATIC | member_flags::ABSTRACT,
        ));
    }
    r.class(backed);
}

impl Registry<'_> {
    /// Builds an internal enum class (not yet registered): `cases` are
    /// `(name, backing value)` pairs, created lazily on first access.
    /// Add methods/constants to the result, then register it with
    /// [`Registry::class`].
    pub fn new_enum(
        &mut self,
        name: &str,
        backing: EnumBacking,
        cases: &[(&str, Option<Value>)],
        interfaces: &[&str],
    ) -> ClassEntry {
        let mut ce = ClassEntry::new(
            name,
            class_flags::ENUM | class_flags::FINAL | class_flags::NO_DYNAMIC_PROPERTIES,
        );
        ce.enum_backing = Some(backing);
        let unit = self.vm.find_class(b"UnitEnum").expect("UnitEnum is registered by Core");
        ce.interfaces.push(unit);
        if backing != EnumBacking::Pure {
            let b = self.vm.find_class(b"BackedEnum").expect("BackedEnum is registered by Core");
            ce.interfaces.push(b);
        }
        for i in interfaces {
            if let Some(c) = self.vm.find_class(i.as_bytes()) {
                for sub in c.interfaces.clone() {
                    if !ce.interfaces.iter().any(|x| x.lc_name == sub.lc_name) {
                        ce.interfaces.push(sub);
                    }
                }
                if !ce.interfaces.iter().any(|x| x.lc_name == c.lc_name) {
                    ce.interfaces.push(c);
                }
            }
        }
        declare_enum_props(&mut ce, backing);
        add_enum_methods(&mut ce, backing);
        for (n, v) in cases {
            add_case(&mut ce, n, v.clone());
        }
        ce
    }

    /// Builds and registers an internal enum (`zend_register_internal_enum`
    /// + `zend_enum_add_case_cstr`).
    pub fn enum_class(
        &mut self,
        name: &str,
        backing: EnumBacking,
        cases: &[(&str, Option<Value>)],
        interfaces: &[&str],
    ) -> ClassRef {
        let ce = self.new_enum(name, backing, cases, interfaces);
        self.class(ce)
    }
}
