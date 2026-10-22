//! Internal enums (`zend_register_internal_enum`) and deprecation messages.

use super::*;
use crate::class::EnumBacking;
use crate::object::{self, ObjRef};

fn reg_enums(r: &mut Registry<'_>) {
    r.enum_class(
        "Suit",
        EnumBacking::String,
        &[("Hearts", Some(Value::str("H"))), ("Spades", Some(Value::str("S")))],
        &[],
    );
    r.enum_class("Pure", EnumBacking::Pure, &[("A", None), ("B", None)], &[]);
}

#[test]
fn internal_enum_cases_are_lazy() {
    let mut t = TestVm::new();
    let before = object::store_top();
    t.vm.register_module("test_enums", reg_enums);
    // Registration allocates no case objects.
    assert_eq!(object::store_top(), before);
    let suit = t.vm.find_class("Suit").unwrap();
    assert!(suit.is_enum());
    assert!(suit.instance_of_name(b"BackedEnum"));
    assert!(suit.instance_of_name(b"UnitEnum"));
    let h = crate::enums::enum_case(&mut t.vm, &suit, b"Hearts")
        .unwrap()
        .unwrap();
    assert_eq!(crate::enums::enum_case_name(&h).unwrap(), "Hearts");
    assert!(matches!(crate::enums::enum_backing_value(&h), Some(Value::String(s)) if s == "H"));
    // The same object every time.
    let h2 = crate::enums::enum_case(&mut t.vm, &suit, b"Hearts")
        .unwrap()
        .unwrap();
    assert!(ObjRef::ptr_eq(&h, &h2));
    let all = crate::enums::enum_cases(&mut t.vm, &suit).unwrap();
    assert_eq!(all.len(), 2);
    assert!(ObjRef::ptr_eq(&all[0], &h));
    let pure = t.vm.find_class("Pure").unwrap();
    assert!(!pure.instance_of_name(b"BackedEnum"));
    assert!(pure.find_method(b"from").is_none());
    assert!(pure.find_method(b"cases").is_some());
}

fn deprecated_fn(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

static DEPRECATED_FN: NativeFunction = NativeFunction {
    sig: "function old_thing(): void {}",
    handler: deprecated_fn,
};

fn reg_deprecated(r: &mut Registry<'_>) {
    r.deprecated_function(&DEPRECATED_FN, "8.1", "");
    r.deprecated_constant("OLD_CONST", 1i64, "since 8.5, use NEW_CONST instead");
    r.deprecated_constant("OLD_CONST2", 1i64, "use X instead");
}

#[test]
fn deprecation_messages() {
    let mut t = TestVm::new();
    t.vm.register_module("test_dep", reg_deprecated);
    t.call("old_thing", vec![]).unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: Function old_thing() is deprecated since 8.1 in /t.php on line 3\n"
    );
    t.vm.fetch_constant("OLD_CONST").unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: Constant OLD_CONST is deprecated since 8.5, use NEW_CONST instead in /t.php on line 3\n"
    );
    t.vm.fetch_constant("OLD_CONST2").unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: Constant OLD_CONST2 is deprecated, use X instead in /t.php on line 3\n"
    );
    t.vm.fetch_constant("E_STRICT").unwrap();
    assert_eq!(
        t.output(),
        "\nDeprecated: Constant E_STRICT is deprecated since 8.4, the error level was removed in /t.php on line 3\n"
    );
}
