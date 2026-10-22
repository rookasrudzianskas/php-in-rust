//! ext/standard/md5.c

use php_hash::md5;
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use crate::hash::{check_path, digest_value};
use crate::standard::streams::read_all;

/// `md5(string $string, bool $binary = false): string`
fn php_md5(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(digest_value(md5(args.str(0)).to_vec(), args.bool(1)))
}

/// `md5_file(string $filename, bool $binary = false): string|false`
fn php_md5_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    check_path(vm, args.str(0), 1)?;
    match read_all(vm, args.str(0))? {
        Some(contents) => Ok(digest_value(md5(&contents).to_vec(), args.bool(1))),
        // Opening or reading failed; the stream layer has already warned.
        None => Ok(Value::Bool(false)),
    }
}

static MD5: NativeFunction = NativeFunction {
    sig: "md5(string $string, bool $binary = false): string",
    handler: php_md5,
};
static MD5_FILE: NativeFunction = NativeFunction {
    sig: "md5_file(string $filename, bool $binary = false): string|false",
    handler: php_md5_file,
};

pub fn register(r: &mut Registry) {
    r.function(&MD5);
    r.function(&MD5_FILE);
}

#[cfg(test)]
mod tests {
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;

    fn md5(t: &mut TestVm, input: &str, raw: bool) -> Value {
        let args = vec![Value::str(input), Value::Bool(raw)];
        t.call("md5", args).expect("md5() does not throw")
    }

    fn text(v: &Value) -> String {
        String::from_utf8_lossy(v.as_str().expect("a string").as_bytes()).into_owned()
    }

    fn hex(v: &Value) -> String {
        let bytes = v.as_str().expect("a string").as_bytes();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// md5.phpt (RFC 1321 test suite)
    #[test]
    fn md5_ascii_output() {
        let mut t = TestVm::new();
        for (input, expected) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(text(&md5(&mut t, input, false)), expected, "{input:?}");
        }
    }

    /// md5raw.phpt
    #[test]
    fn md5_raw_output() {
        let mut t = TestVm::new();
        for (input, expected) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
        ] {
            let raw = md5(&mut t, input, true);
            assert_eq!(raw.as_str().unwrap().len(), 16);
            assert_eq!(hex(&raw), expected);
        }
    }

    #[test]
    fn md5_binary_flag_is_coerced_to_bool() {
        let mut t = TestVm::new();
        let r = t
            .call("md5", vec![Value::str("abc"), Value::Int(1)])
            .unwrap();
        assert_eq!(r.as_str().unwrap().len(), 16);
        let r = t
            .call("md5", vec![Value::str("abc"), Value::Int(0)])
            .unwrap();
        assert_eq!(r.as_str().unwrap().len(), 32);
    }

    #[test]
    fn md5_type_errors() {
        let mut t = TestVm::new();
        let (class, message) = thrown(t.call("md5", vec![Value::empty_array()]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            message,
            "md5(): Argument #1 ($string) must be of type string, array given"
        );
    }

    #[test]
    fn md5_file_rejects_null_bytes() {
        let mut t = TestVm::new();
        let (class, message) = thrown(t.call("md5_file", vec![Value::bytes(b"a\0b")]));
        assert_eq!(class, "ValueError");
        assert_eq!(
            message,
            "md5_file(): Argument #1 ($filename) must not contain any null bytes"
        );
    }

    /// md5_file.phpt, needs the stream layer.
    #[test]
    #[ignore = "needs the stream layer (crate::standard::streams::read_all)"]
    fn md5_file_of_a_real_file() {
        let path = std::env::temp_dir().join("php_in_rust_md5_file_test.txt");
        std::fs::write(&path, "Add this to the file\n").unwrap();
        let mut t = TestVm::new();
        let r = t
            .call("md5_file", vec![Value::str(path.to_str().unwrap())])
            .unwrap();
        assert_eq!(text(&r), "7f28ec647825e2a70bf67778472cd4a2");
        let r = t
            .call("md5_file", vec![Value::str("/nonexistent/aZrq16u")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        let _ = std::fs::remove_file(path);
    }
}
