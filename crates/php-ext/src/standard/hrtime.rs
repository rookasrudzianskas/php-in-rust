//! ext/standard/hrtime.c

use php_runtime::{Args, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use super::basic::sys;

const NANO_IN_SEC: u64 = 1_000_000_000;

fn hrtime(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let get_as_num = args.bool(0);
    let t = sys::hrtime_ns();
    if get_as_num {
        return Ok(Value::Int(t as i64));
    }
    let mut pair = PhpArray::new();
    let _ = pair.append((t / NANO_IN_SEC) as i64);
    let _ = pair.append((t % NANO_IN_SEC) as i64);
    Ok(Value::array(pair))
}

pub static HRTIME: NativeFunction = NativeFunction {
    sig: "hrtime(bool $as_number = false): array|int|float|false",
    handler: hrtime,
};

pub fn register(r: &mut Registry) {
    r.function(&HRTIME);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::Value;

    #[test]
    fn hrtime_array_and_number() {
        let mut t = TestVm::new();
        let Value::Array(a) = t.call("hrtime", vec![]).unwrap() else {
            panic!("array expected")
        };
        assert_eq!(a.len(), 2);
        assert!(matches!(a.get_int(0), Some(Value::Int(_))));
        let ns = a.get_int(1).unwrap().as_int().unwrap();
        assert!((0..1_000_000_000).contains(&ns));

        let Value::Int(n1) = t.call("hrtime", vec![Value::Bool(true)]).unwrap() else {
            panic!("int expected")
        };
        let Value::Int(n2) = t.call("hrtime", vec![Value::Bool(true)]).unwrap() else {
            panic!("int expected")
        };
        assert!(n2 >= n1 && n1 > 0);
    }
}
