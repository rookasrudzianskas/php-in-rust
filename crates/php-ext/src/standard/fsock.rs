//! ext/standard/fsock.c

use std::time::Duration;

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use super::streams::socket;

/// `php_fsockopen_stream`.
fn fsockopen_stream(vm: &mut Vm, args: &mut Args, persistent: bool) -> PResult<Value> {
    let host = args.str(0).clone();
    let port = args.int(1);
    let timeout = match args.float_opt(4) {
        None => vm.ini_int("default_socket_timeout") as f64,
        Some(t) => t,
    };
    let max = u64::MAX as f64 / 1_000_000.0;
    if timeout != -1.0 && !(0.0..=max).contains(&timeout) {
        return Err(
            vm.argument_value_error(5, &format!("must be -1 or between 0 and {}", max as u64))
        );
    }
    let hostname: Vec<u8> = if port > 0 {
        let mut h = host.to_vec();
        h.extend_from_slice(format!(":{port}").as_bytes());
        h
    } else {
        host.to_vec()
    };
    let tv = if timeout < 0.0 {
        None
    } else {
        Some(Duration::from_micros((timeout * 1_000_000.0) as u64))
    };
    let key = persistent.then(|| {
        let mut k = b"pfsockopen__".to_vec();
        k.extend_from_slice(&host);
        k.extend_from_slice(format!(":{port}").as_bytes());
        k
    });
    match socket::xport_create(vm, &hostname, false, 0, tv, None, key.as_deref())? {
        Ok(res) => {
            if args.is_passed(2) {
                args.by_ref(2).set(Value::Int(0));
            }
            if args.is_passed(3) {
                args.by_ref(3).set(Value::empty_string());
            }
            Ok(Value::Resource(res))
        }
        Err((errno, msg)) => {
            vm.warning(format!(
                "Unable to connect to {}:{port} ({})",
                String::from_utf8_lossy(&host),
                if msg.is_empty() {
                    "Unknown error"
                } else {
                    &msg
                }
            ))?;
            if args.is_passed(2) {
                args.by_ref(2).set(Value::Int(errno as i64));
            }
            if args.is_passed(3) && !msg.is_empty() {
                args.by_ref(3).set(Value::from(msg));
            }
            Ok(Value::Bool(false))
        }
    }
}

fn fsockopen(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    fsockopen_stream(vm, args, false)
}

fn pfsockopen(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    fsockopen_stream(vm, args, true)
}

static FSOCKOPEN: NativeFunction = NativeFunction {
    sig: "fsockopen(string $hostname, int $port = -1, &$error_code = null, &$error_message = null, ?float $timeout = null)",
    handler: fsockopen,
};
static PFSOCKOPEN: NativeFunction = NativeFunction {
    sig: "pfsockopen(string $hostname, int $port = -1, &$error_code = null, &$error_message = null, ?float $timeout = null)",
    handler: pfsockopen,
};

pub fn register(r: &mut Registry) {
    r.functions(&[&FSOCKOPEN, &PFSOCKOPEN]);
}

#[cfg(test)]
mod tests {
    use crate::testing::TestVm;
    use php_runtime::{PhpRef, Value};

    #[test]
    fn fsockopen_connects_and_reports_errors() {
        let mut t = TestVm::new();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port() as i64;
        let fp = t
            .call("fsockopen", vec![Value::str("127.0.0.1"), Value::Int(port)])
            .unwrap();
        assert!(matches!(fp, Value::Resource(_)));
        let (mut peer, _) = l.accept().unwrap();
        std::io::Write::write_all(&mut peer, b"hi\n").unwrap();
        let line = t.call("fgets", vec![fp.clone()]).unwrap();
        assert_eq!(line.as_str().map(|s| s.to_vec()), Some(b"hi\n".to_vec()));
        let m = t.call("stream_get_meta_data", vec![fp.clone()]).unwrap();
        let st = m
            .as_array()
            .unwrap()
            .get_str(b"stream_type")
            .cloned()
            .unwrap();
        assert_eq!(
            st.as_str().map(|s| s.to_vec()),
            Some(b"tcp_socket".to_vec())
        );
        t.call("fclose", vec![fp]).unwrap();
        drop(l);
        let errno = Value::Ref(PhpRef::new(Value::Null));
        let errstr = Value::Ref(PhpRef::new(Value::Null));
        let r = t
            .call(
                "fsockopen",
                vec![
                    Value::str("127.0.0.1"),
                    Value::Int(port),
                    errno.clone(),
                    errstr.clone(),
                    Value::Float(1.0),
                ],
            )
            .unwrap();
        assert_eq!(r.as_bool(), Some(false));
        assert_eq!(
            t.output(),
            format!(
                "\nWarning: fsockopen(): Unable to connect to 127.0.0.1:{port} (Connection refused) in /t.php on line 3\n"
            )
        );
        assert!(errno.unref().as_int().unwrap() > 0);
        assert_eq!(
            errstr.unref().as_str().map(|s| s.to_vec()),
            Some(b"Connection refused".to_vec())
        );
    }

    #[test]
    fn udp_and_timeout_validation() {
        let mut t = TestVm::new();
        let fp = t
            .call(
                "fsockopen",
                vec![Value::str("udp://127.0.0.1"), Value::Int(9)],
            )
            .unwrap();
        assert!(matches!(fp, Value::Resource(_)));
        let (cls, msg) = crate::testing::thrown(t.call(
            "fsockopen",
            vec![
                Value::str("127.0.0.1"),
                Value::Int(1),
                Value::Null,
                Value::Null,
                Value::Float(-2.0),
            ],
        ));
        assert_eq!(cls, "ValueError");
        assert!(
            msg.starts_with("fsockopen(): Argument #5 ($timeout) must be -1 or between 0 and ")
        );
    }
}
