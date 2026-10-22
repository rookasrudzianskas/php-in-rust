//! ext/spl/spl_exceptions.c — the SPL exception hierarchy.

use php_runtime::Registry;
use php_runtime::class::ClassEntry;

pub const LOGIC_EXCEPTION: &str = "LogicException";
pub const BAD_FUNCTION_CALL_EXCEPTION: &str = "BadFunctionCallException";
pub const BAD_METHOD_CALL_EXCEPTION: &str = "BadMethodCallException";
pub const DOMAIN_EXCEPTION: &str = "DomainException";
pub const INVALID_ARGUMENT_EXCEPTION: &str = "InvalidArgumentException";
pub const LENGTH_EXCEPTION: &str = "LengthException";
pub const OUT_OF_RANGE_EXCEPTION: &str = "OutOfRangeException";
pub const RUNTIME_EXCEPTION: &str = "RuntimeException";
pub const OUT_OF_BOUNDS_EXCEPTION: &str = "OutOfBoundsException";
pub const OVERFLOW_EXCEPTION: &str = "OverflowException";
pub const RANGE_EXCEPTION: &str = "RangeException";
pub const UNDERFLOW_EXCEPTION: &str = "UnderflowException";
pub const UNEXPECTED_VALUE_EXCEPTION: &str = "UnexpectedValueException";

/// `PHP_MINIT_FUNCTION(spl_exceptions)`.
pub fn register(r: &mut Registry) {
    let exception = r
        .find_class("Exception")
        .expect("Exception is a core class");
    let sub =
        |r: &mut Registry, name: &str, parent| r.class(ClassEntry::new_subclass(name, 0, parent));
    let logic = sub(r, LOGIC_EXCEPTION, &exception);
    let bad_function = sub(r, BAD_FUNCTION_CALL_EXCEPTION, &logic);
    sub(r, BAD_METHOD_CALL_EXCEPTION, &bad_function);
    sub(r, DOMAIN_EXCEPTION, &logic);
    sub(r, INVALID_ARGUMENT_EXCEPTION, &logic);
    sub(r, LENGTH_EXCEPTION, &logic);
    sub(r, OUT_OF_RANGE_EXCEPTION, &logic);

    let runtime = sub(r, RUNTIME_EXCEPTION, &exception);
    sub(r, OUT_OF_BOUNDS_EXCEPTION, &runtime);
    sub(r, OVERFLOW_EXCEPTION, &runtime);
    sub(r, RANGE_EXCEPTION, &runtime);
    sub(r, UNDERFLOW_EXCEPTION, &runtime);
    sub(r, UNEXPECTED_VALUE_EXCEPTION, &runtime);
}
