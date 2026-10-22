//! Internal pure enums (`zend_register_internal_enum`, `zend_enum_add_case_cstr`
//! and `zend_enum_register_funcs`), used for `Random\IntervalBoundary` and
//! `RoundingMode`.
//!
//! The runtime has no enum registration API yet, so this is the one place that
//! builds an internal enum. A case is a class constant flagged
//! `member_flags::ENUM_CASE` whose value is an instance of the enum class with
//! a readonly `name` property.
//!
//! NOTE: case objects are instantiated eagerly, at registration time (php-src
//! creates them lazily on first access), so they occupy the first object
//! handles. When the runtime grows lazy enum-case constants this function is
//! the only code that has to change.

use php_runtime::class::{
    ClassConstant, ClassEntry, ConstValue, EnumBacking, Function, PropertyInfo, class_flags,
    member_flags,
};
use php_runtime::types::TypeDecl;
use php_runtime::{Args, ClassRef, NativeFunction, ObjRef, PResult, Registry, Value, Vm};

fn unit_enum_cases(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error("Error", "Cannot call abstract method UnitEnum::cases()"))
}

static UNIT_ENUM_CASES: NativeFunction = NativeFunction {
    sig: "public static function cases(): array",
    handler: unit_enum_cases,
};

/// `Enum::cases()`: the case objects in declaration order.
fn enum_cases(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let class = match args.scope() {
        Some(c) => c.clone(),
        None => return Err(vm.throw_error("Error", "Enum::cases() called without a class scope")),
    };
    let mut list = Vec::new();
    for c in class.constants.values() {
        if c.flags & member_flags::ENUM_CASE != 0
            && let Some(v) = c.evaluated()
        {
            list.push(v);
        }
    }
    Ok(Value::array(php_runtime::PhpArray::from_values(list)))
}

static ENUM_CASES: NativeFunction = NativeFunction {
    sig: "public static function cases(): array",
    handler: enum_cases,
};

/// The `UnitEnum` interface (registered here when the runtime has not done so).
fn unit_enum_interface(r: &mut Registry<'_>) -> ClassRef {
    if let Some(c) = r.find_class("UnitEnum") {
        return c;
    }
    let mut ce = ClassEntry::new("UnitEnum", class_flags::INTERFACE);
    ce.add_method(Function::native_method(
        &UNIT_ENUM_CASES,
        "UnitEnum",
        member_flags::PUBLIC | member_flags::STATIC | member_flags::ABSTRACT,
    ));
    r.class(ce)
}

/// Registers the pure enum `name` with the given cases.
pub fn register_pure_enum(
    r: &mut Registry<'_>,
    name: &'static str,
    cases: &[&'static str],
) -> ClassRef {
    let unit = unit_enum_interface(r);
    let mut ce = ClassEntry::new(
        name,
        class_flags::FINAL | class_flags::ENUM | class_flags::NO_DYNAMIC_INSTANTIATION,
    );
    ce.enum_backing = Some(EnumBacking::Pure);
    ce.interfaces.push(unit);

    let mut prop = PropertyInfo::new("name", name, member_flags::PUBLIC | member_flags::READONLY);
    prop.ty = Some(TypeDecl::parse("string").expect("valid type"));
    prop.default = Value::Undef;
    ce.declare_property(prop);

    for case in cases {
        let mut c = ClassConstant::new(*case, Value::Null, name);
        c.flags = member_flags::PUBLIC | member_flags::ENUM_CASE;
        ce.add_constant(c);
    }
    ce.add_method(Function::native_method(
        &ENUM_CASES,
        name,
        member_flags::PUBLIC | member_flags::STATIC,
    ));
    let class = r.class(ce);

    for case in cases {
        let obj = ObjRef::new(class.clone());
        obj.set_prop(b"name", Value::str(case));
        if let Some(c) = class.find_constant(case.as_bytes()) {
            *c.value.borrow_mut() = ConstValue::Value(Value::Object(obj));
        }
    }
    class
}

/// The case name of an enum case object (`$case->name`).
pub fn case_name(obj: &ObjRef) -> Option<String> {
    match obj.get_prop(b"name") {
        Some(Value::String(s)) => Some(s.to_string_lossy().into_owned()),
        _ => None,
    }
}
