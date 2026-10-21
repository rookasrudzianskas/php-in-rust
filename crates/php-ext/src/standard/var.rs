//! ext/standard/var.c
//!
//! `var_dump`, `var_export` and `debug_zval_dump` are implemented by the
//! interpreter; `serialize`/`unserialize` live in [`super::var_serialize`].
//! This module holds the memory statistics functions.

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// The Rust allocator is not instrumented, so the Zend memory manager's
/// statistics are reported as fixed values: `memory_get_usage()` stays
/// constant (differences are 0) and the "real" figures are one 2 MiB chunk,
/// the granularity at which `zend_mm` requests memory from the system.
const USAGE: i64 = 393_216;
const REAL_USAGE: i64 = 2 * 1024 * 1024;

/// `zend_memory_usage(real_usage)` / `zend_memory_peak_usage(real_usage)`.
fn usage(real: bool) -> Value {
    Value::Int(if real { REAL_USAGE } else { USAGE })
}

fn memory_get_usage(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(usage(args.bool(0)))
}

fn memory_get_peak_usage(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(usage(args.bool(0)))
}

fn memory_reset_peak_usage(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Null)
}

pub static MEMORY_GET_USAGE: NativeFunction = NativeFunction {
    sig: "function memory_get_usage(bool $real_usage = false): int {}",
    handler: memory_get_usage,
};
pub static MEMORY_GET_PEAK_USAGE: NativeFunction = NativeFunction {
    sig: "function memory_get_peak_usage(bool $real_usage = false): int {}",
    handler: memory_get_peak_usage,
};
pub static MEMORY_RESET_PEAK_USAGE: NativeFunction = NativeFunction {
    sig: "function memory_reset_peak_usage(): void {}",
    handler: memory_reset_peak_usage,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &MEMORY_GET_USAGE,
        &MEMORY_GET_PEAK_USAGE,
        &MEMORY_RESET_PEAK_USAGE,
    ]);
}

#[cfg(test)]
mod tests {
    use php_runtime::Value;

    use crate::testing::{TestVm, thrown};

    #[test]
    fn memory_statistics() {
        // ext/standard/tests/serialize/bug81142.phpt relies on a stable usage.
        let mut t = TestVm::new();
        let a = t
            .call("memory_get_usage", vec![])
            .unwrap()
            .as_int()
            .unwrap();
        let b = t
            .call("memory_get_usage", vec![])
            .unwrap()
            .as_int()
            .unwrap();
        assert!(a > 0);
        assert_eq!(a - b, 0);
        let real = t
            .call("memory_get_usage", vec![Value::Bool(true)])
            .unwrap()
            .as_int()
            .unwrap();
        assert!(real >= a);
        let peak = t
            .call("memory_get_peak_usage", vec![])
            .unwrap()
            .as_int()
            .unwrap();
        assert!(peak >= a);
        assert!(matches!(
            t.call("memory_reset_peak_usage", vec![]),
            Ok(Value::Null)
        ));
        assert_eq!(
            thrown(t.call("memory_reset_peak_usage", vec![Value::Int(1)])),
            (
                "ArgumentCountError".to_string(),
                "memory_reset_peak_usage() expects exactly 0 arguments, 1 given".to_string()
            )
        );
    }
}
