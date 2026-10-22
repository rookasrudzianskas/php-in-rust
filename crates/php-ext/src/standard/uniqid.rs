//! ext/standard/uniqid.c

use std::cell::Cell;

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

use super::basic::sys;
use super::microtime::gettimeofday_now;

thread_local! {
    /// `prev_tv`: the last returned time, so two calls never share a microsecond.
    static PREV_TV: Cell<(i64, i64)> = const { Cell::new((0, 0)) };
}

fn uniqid(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let prefix = args.str(0).clone();
    let more_entropy = args.bool(1);

    // Poll until the microsecond changes (faster than usleep(1), see uniqid.c).
    let (sec, usec) = loop {
        let now = gettimeofday_now();
        if PREV_TV.get() != now {
            break now;
        }
    };
    PREV_TV.set((sec, usec));

    // `(int) tv.tv_sec` truncates; `%08x` prints it as unsigned.
    let sec = sec as i32 as u32;
    let usec = (usec % 0x100000) as i32 as u32;

    let mut out = prefix.to_vec();
    if more_entropy {
        let mut b = [0u8; 4];
        sys::random_bytes_or_fallback(&mut b);
        let bytes = u32::from_ne_bytes(b);
        let seed = (bytes as f64 / u32::MAX as f64) * 10.0;
        out.extend_from_slice(format!("{sec:08x}{usec:05x}{seed:.8}").as_bytes());
    } else {
        out.extend_from_slice(format!("{sec:08x}{usec:05x}").as_bytes());
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

pub static UNIQID: NativeFunction = NativeFunction {
    sig: "uniqid(string $prefix = \"\", bool $more_entropy = false): string",
    handler: uniqid,
};

pub fn register(r: &mut Registry) {
    r.function(&UNIQID);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::Value;

    fn call(t: &mut TestVm, args: Vec<Value>) -> String {
        match t.call("uniqid", args).unwrap() {
            Value::String(s) => s.to_string_lossy().into_owned(),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn format() {
        let mut t = TestVm::new();
        let a = call(&mut t, vec![]);
        assert_eq!(a.len(), 13);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        let p = call(&mut t, vec![Value::str("pre_")]);
        assert_eq!(p.len(), 17);
        assert!(p.starts_with("pre_"));
        // more_entropy appends "%.8F" of a number in [0, 10].
        let e = call(&mut t, vec![Value::str(""), Value::Bool(true)]);
        assert_eq!(e.len(), 13 + 10, "{e}");
        assert_eq!(e.as_bytes()[14], b'.');
    }

    #[test]
    fn consecutive_calls_differ() {
        let mut t = TestVm::new();
        let a = call(&mut t, vec![]);
        let b = call(&mut t, vec![]);
        assert_ne!(a, b);
        assert!(a < b);
    }
}
