//! ext/standard/crc32.c

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// `crc32(string $string): int` — the CRC-32 (`crc32b`) as an unsigned value
/// (PHP integers are 64-bit, so the result never goes negative).
fn php_crc32(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(i64::from(php_hash::crc32(args.str(0)))))
}

static CRC32: NativeFunction = NativeFunction {
    sig: "crc32(string $string): int",
    handler: php_crc32,
};

pub fn register(r: &mut Registry) {
    r.function(&CRC32);
}

#[cfg(test)]
mod tests {
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;

    fn crc32(t: &mut TestVm, input: &str) -> i64 {
        match t.call("crc32", vec![Value::str(input)]) {
            Ok(Value::Int(n)) => n,
            other => panic!("crc32() returned {other:?}"),
        }
    }

    /// crc32.phpt
    #[test]
    fn crc32_known_values() {
        let mut t = TestVm::new();
        for (input, expected) in [
            ("foo", 2356372769),
            ("bar", 1996459178),
            ("baz", 2015626392),
            ("grldsajkopallkjasd", 824412087),
            ("1234567890123456", 509595063),
            ("1234567890123456abc", 1890929711),
            ("12345678901234561234567890123456", 156217630),
            ("12345678901234561234567890123456abc", 941689929),
            (
                "123456789012345612345678901234561234567890123456",
                1939457775,
            ),
            (
                "123456789012345612345678901234561234567890123456abc",
                2213121284,
            ),
            (
                "1234567890123456123456789012345612345678901234561234567890123456",
                522627406,
            ),
            (
                "1234567890123456123456789012345612345678901234561234567890123456abc",
                3806880586,
            ),
        ] {
            assert_eq!(crc32(&mut t, input), expected, "{input:?}");
        }
    }

    /// crc32_basic.phpt
    #[test]
    fn crc32_basic() {
        let mut t = TestVm::new();
        assert_eq!(crc32(&mut t, "string_val1234"), 256895812);
        assert_eq!(crc32(&mut t, ""), 0);
        assert_eq!(crc32(&mut t, "123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_arity_and_type_errors() {
        let mut t = TestVm::new();
        let (class, message) = thrown(t.call("crc32", vec![]));
        assert_eq!(class, "ArgumentCountError");
        assert_eq!(message, "crc32() expects exactly 1 argument, 0 given");
        let (class, message) = thrown(t.call("crc32", vec![Value::empty_array()]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            message,
            "crc32(): Argument #1 ($string) must be of type string, array given"
        );
    }
}
