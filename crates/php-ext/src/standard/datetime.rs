//! ext/standard/datetime.c

use php_runtime::{Args, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use super::basic::sys;

fn strptime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    vm.engine_deprecated(
        "Function strptime() is deprecated since 8.2, use date_parse_from_format() (for locale-independent parsing), or IntlDateFormatter::parse() (for locale-dependent parsing) instead",
    )?;
    let Some(p) = sys::strptime(args.str(0), args.str(1)) else {
        return Ok(Value::Bool(false));
    };
    let mut a = PhpArray::new();
    a.insert_str("tm_sec", p.tm_sec);
    a.insert_str("tm_min", p.tm_min);
    a.insert_str("tm_hour", p.tm_hour);
    a.insert_str("tm_mday", p.tm_mday);
    a.insert_str("tm_mon", p.tm_mon);
    a.insert_str("tm_year", p.tm_year);
    a.insert_str("tm_wday", p.tm_wday);
    a.insert_str("tm_yday", p.tm_yday);
    a.insert_str("unparsed", Value::bytes(&p.unparsed));
    Ok(Value::array(a))
}

// The `#[\Deprecated]` attribute of the stub is emitted by `strptime()` itself.
pub static STRPTIME: NativeFunction = NativeFunction {
    sig: "strptime(string $timestamp, string $format): array|false",
    handler: strptime,
};

pub fn register(r: &mut Registry) {
    r.function(&STRPTIME);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::Value;

    #[test]
    fn parses_and_reports_unparsed_tail() {
        let mut t = TestVm::new();
        let r = t
            .call(
                "strptime",
                vec![Value::str("2020-02-29 trailing"), Value::str("%Y-%m-%d")],
            )
            .unwrap();
        let out = t.output();
        assert!(
            out.contains("Deprecated: Function strptime() is deprecated since 8.2"),
            "{out}"
        );
        let Value::Array(a) = r else {
            panic!("array expected")
        };
        assert_eq!(a.get_str(b"tm_year").unwrap().as_int(), Some(120));
        assert_eq!(a.get_str(b"tm_mon").unwrap().as_int(), Some(1));
        assert_eq!(a.get_str(b"tm_mday").unwrap().as_int(), Some(29));
        assert_eq!(
            a.get_str(b"unparsed").unwrap().as_str().unwrap(),
            " trailing"
        );
    }

    #[test]
    fn mismatch_is_false() {
        let mut t = TestVm::new();
        let r = t
            .call("strptime", vec![Value::str("nope"), Value::str("%Y")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
    }
}
