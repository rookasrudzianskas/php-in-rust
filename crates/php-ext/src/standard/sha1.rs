//! ext/standard/sha1.c

use php_hash::sha1;
use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

use crate::hash::{check_path, digest_value};
use crate::standard::streams::read_all;

/// `sha1(string $string, bool $binary = false): string`
fn php_sha1(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(digest_value(sha1(args.str(0)).to_vec(), args.bool(1)))
}

/// `sha1_file(string $filename, bool $binary = false): string|false`
fn php_sha1_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    check_path(vm, args.str(0), 1)?;
    match read_all(vm, args.str(0))? {
        Some(contents) => Ok(digest_value(sha1(&contents).to_vec(), args.bool(1))),
        // Opening or reading failed; the stream layer has already warned.
        None => Ok(Value::Bool(false)),
    }
}

static SHA1: NativeFunction = NativeFunction {
    sig: "sha1(string $string, bool $binary = false): string",
    handler: php_sha1,
};
static SHA1_FILE: NativeFunction = NativeFunction {
    sig: "sha1_file(string $filename, bool $binary = false): string|false",
    handler: php_sha1_file,
};

pub fn register(r: &mut Registry) {
    r.function(&SHA1);
    r.function(&SHA1_FILE);
}

#[cfg(test)]
mod tests {
    use crate::testing::{TestVm, thrown};
    use php_runtime::Value;

    fn sha1(t: &mut TestVm, input: &str, raw: bool) -> Value {
        let args = vec![Value::str(input), Value::Bool(raw)];
        t.call("sha1", args).expect("sha1() does not throw")
    }

    fn text(v: &Value) -> String {
        String::from_utf8_lossy(v.as_str().expect("a string").as_bytes()).into_owned()
    }

    fn hex(v: &Value) -> String {
        let bytes = v.as_str().expect("a string").as_bytes();
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// sha1.phpt
    #[test]
    fn sha1_ascii_output() {
        let mut t = TestVm::new();
        for (input, expected) in [
            ("abc", "a9993e364706816aba3e25717850c26c9cd0d89d"),
            (
                "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "84983e441c3bd26ebaae4aa1f95129e5e54670f1",
            ),
            ("a", "86f7e437faa5a7fce15d1ddcb9eaeaea377667b8"),
            (
                "0123456701234567012345670123456701234567012345670123456701234567",
                "e0c094e867ef46c350ef54a7f59dd60bed92ae83",
            ),
            ("", "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        ] {
            assert_eq!(text(&sha1(&mut t, input, false)), expected, "{input:?}");
        }
    }

    /// sha1raw.phpt
    #[test]
    fn sha1_raw_output() {
        let mut t = TestVm::new();
        let raw = sha1(&mut t, "abc", true);
        assert_eq!(raw.as_str().unwrap().len(), 20);
        assert_eq!(hex(&raw), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let raw = sha1(&mut t, "", true);
        assert_eq!(hex(&raw), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn sha1_type_errors() {
        let mut t = TestVm::new();
        let (class, message) = thrown(t.call("sha1", vec![Value::empty_array()]));
        assert_eq!(class, "TypeError");
        assert_eq!(
            message,
            "sha1(): Argument #1 ($string) must be of type string, array given"
        );
    }

    #[test]
    fn sha1_file_rejects_null_bytes() {
        let mut t = TestVm::new();
        let (class, message) = thrown(t.call("sha1_file", vec![Value::bytes(b"a\0b")]));
        assert_eq!(class, "ValueError");
        assert_eq!(
            message,
            "sha1_file(): Argument #1 ($filename) must not contain any null bytes"
        );
    }

    /// sha1.phpt (the `sha1_file()` half), needs the stream layer.
    #[test]
    #[ignore = "needs the stream layer (crate::standard::streams::read_all)"]
    fn sha1_file_of_a_real_file() {
        let path = std::env::temp_dir().join("php_in_rust_sha1_file_test.dat");
        std::fs::write(&path, "abc").unwrap();
        let mut t = TestVm::new();
        let r = t
            .call("sha1_file", vec![Value::str(path.to_str().unwrap())])
            .unwrap();
        assert_eq!(text(&r), "a9993e364706816aba3e25717850c26c9cd0d89d");
        let r = t
            .call("sha1_file", vec![Value::str("/nonexistent/sha1.dat")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        let _ = std::fs::remove_file(path);
    }
}
