//! ext/filter/callback_filter.c — `FILTER_CALLBACK`.

use php_runtime::{PResult, Value, Vm};

use super::Outcome;

/// `php_filter_callback()`: passes the value to the callable given as the
/// `"options"` argument and keeps its return value (`null` when it returns
/// nothing).
pub(super) fn php_filter_callback(
    vm: &mut Vm,
    value: &mut Value,
    _flags: i64,
    options: Option<&Value>,
) -> PResult<Outcome> {
    let host = vm.host.clone();
    let callable = match options {
        Some(callable) if host.check_callable(vm, callable).is_ok() => callable.clone(),
        _ => {
            *value = Value::Null;
            let fname = vm.current_function_name();
            return Err(vm.throw_type_error(format!("{fname}(): Option must be a valid callback")));
        }
    };

    let arg = std::mem::take(value);
    *value = match vm.call(&callable, vec![arg])?.into_unref() {
        Value::Undef => Value::Null,
        ret => ret,
    };
    Ok(Outcome::Success)
}
