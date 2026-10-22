//! ext/standard/microtime.c

use std::time::{SystemTime, UNIX_EPOCH};

use php_runtime::{Args, NativeFunction, PResult, PhpArray, Registry, Value, Vm};

use super::basic::{default_timezone, sys};

const MICRO_IN_SEC: f64 = 1_000_000.0;
const SEC_IN_MIN: i64 = 60;

/// `gettimeofday(2)`: seconds and microseconds since the Unix epoch.
pub(crate) fn gettimeofday_now() -> (i64, i64) {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_secs() as i64, d.subsec_micros() as i64)
}

/// `_php_gettimeofday` for `microtime()` (mode 0) and `gettimeofday()` (mode 1).
fn gettimeofday_impl(vm: &mut Vm, args: &Args, mode: bool) -> PResult<Value> {
    let get_as_float = args.bool(0);
    let (sec, usec) = gettimeofday_now();

    if get_as_float {
        return Ok(Value::Float(sec as f64 + usec as f64 / MICRO_IN_SEC));
    }

    if mode {
        let tz = default_timezone(vm);
        let (offset, is_dst) = match php_timelib::builtin_tzinfo(&tz) {
            Some(info) => {
                let o = info.get_time_zone_info(sec);
                (o.offset as i64, o.is_dst)
            }
            None => (0, false),
        };
        let mut a = PhpArray::new();
        a.insert_str("sec", sec);
        a.insert_str("usec", usec);
        // `-offset->offset / SEC_IN_MIN` is C integer division (truncating).
        a.insert_str("minuteswest", -offset / SEC_IN_MIN);
        a.insert_str("dsttime", is_dst as i64);
        Ok(Value::array(a))
    } else {
        Ok(Value::from(format!(
            "{:.8} {}",
            usec as f64 / MICRO_IN_SEC,
            sec
        )))
    }
}

fn microtime(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    gettimeofday_impl(vm, args, false)
}

fn gettimeofday(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    gettimeofday_impl(vm, args, true)
}

fn getrusage(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let who_children = args.int(0) == 1;
    let Some(u) = sys::getrusage(who_children) else {
        return Ok(Value::Bool(false));
    };
    let mut a = PhpArray::new();
    for (k, v) in [
        ("ru_oublock", u.ru_oublock),
        ("ru_inblock", u.ru_inblock),
        ("ru_msgsnd", u.ru_msgsnd),
        ("ru_msgrcv", u.ru_msgrcv),
        ("ru_maxrss", u.ru_maxrss),
        ("ru_ixrss", u.ru_ixrss),
        ("ru_idrss", u.ru_idrss),
        ("ru_minflt", u.ru_minflt),
        ("ru_majflt", u.ru_majflt),
        ("ru_nsignals", u.ru_nsignals),
        ("ru_nvcsw", u.ru_nvcsw),
        ("ru_nivcsw", u.ru_nivcsw),
        ("ru_nswap", u.ru_nswap),
        ("ru_utime.tv_usec", u.utime_usec),
        ("ru_utime.tv_sec", u.utime_sec),
        ("ru_stime.tv_usec", u.stime_usec),
        ("ru_stime.tv_sec", u.stime_sec),
    ] {
        a.insert_str(k, v);
    }
    Ok(Value::array(a))
}

pub static MICROTIME: NativeFunction = NativeFunction {
    sig: "microtime(bool $as_float = false): string|float",
    handler: microtime,
};
pub static GETTIMEOFDAY: NativeFunction = NativeFunction {
    sig: "gettimeofday(bool $as_float = false): array|float",
    handler: gettimeofday,
};
pub static GETRUSAGE: NativeFunction = NativeFunction {
    sig: "getrusage(int $mode = 0): array|false",
    handler: getrusage,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&MICROTIME, &GETTIMEOFDAY, &GETRUSAGE]);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::Value;

    #[test]
    fn microtime_formats() {
        let mut t = TestVm::new();
        let Value::String(s) = t.call("microtime", vec![]).unwrap() else {
            panic!("string expected")
        };
        let s = s.to_string_lossy().into_owned();
        let (frac, sec) = s.split_once(' ').unwrap();
        assert!(frac.starts_with("0.") && frac.len() == 10, "{s}");
        assert!(sec.parse::<i64>().unwrap() > 1_600_000_000);
        let Value::Float(f) = t.call("microtime", vec![Value::Bool(true)]).unwrap() else {
            panic!("float expected")
        };
        assert!(f > 1.6e9);
    }

    #[test]
    fn gettimeofday_array_and_float() {
        let mut t = TestVm::new();
        let Value::Array(a) = t.call("gettimeofday", vec![]).unwrap() else {
            panic!("array expected")
        };
        let keys: Vec<String> = a.keys().map(|k| k.to_string()).collect();
        assert_eq!(keys, ["sec", "usec", "minuteswest", "dsttime"]);
        let usec = a.get_str(b"usec").unwrap().as_int().unwrap();
        assert!((0..1_000_000).contains(&usec));
        // The default timezone in the test VM is UTC.
        assert_eq!(a.get_str(b"minuteswest").unwrap().as_int(), Some(0));
        assert_eq!(a.get_str(b"dsttime").unwrap().as_int(), Some(0));
        assert!(matches!(
            t.call("gettimeofday", vec![Value::Bool(true)]).unwrap(),
            Value::Float(_)
        ));
    }

    #[test]
    fn getrusage_keys() {
        let mut t = TestVm::new();
        let Value::Array(a) = t.call("getrusage", vec![]).unwrap() else {
            panic!("array expected")
        };
        assert_eq!(a.len(), 17);
        assert!(a.get_str(b"ru_utime.tv_sec").is_some());
        assert!(a.get_str(b"ru_stime.tv_usec").is_some());
        let Value::Array(c) = t.call("getrusage", vec![Value::Int(1)]).unwrap() else {
            panic!("array expected")
        };
        assert_eq!(c.len(), 17);
    }
}
