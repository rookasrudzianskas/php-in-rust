//! ext/standard/crypt.c — `crypt()` and the salt-format dispatch of `php_crypt()`.
//!
//! PHP always uses its own implementations of the algorithms
//! (`PHP_USE_PHP_CRYPT_R`): traditional and extended DES ([`des`],
//! crypt_freesec.c), MD5 ([`md5`], php_crypt_r.c), Blowfish ([`blowfish`],
//! crypt_blowfish.c) and SHA-256/SHA-512 ([`sha`], crypt_sha256.c and
//! crypt_sha512.c).

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

mod blowfish;
mod blowfish_tables;
mod des;
mod digest;
mod md5;
mod sha;

/// `PHP_MAX_SALT_LEN`: sha512 crypt has the maximal salt length of 123 characters.
pub const PHP_MAX_SALT_LEN: usize = 123;

/// The contents of the C string starting at `b` (everything up to the first NUL).
fn c_str(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(n) => &b[..n],
        None => b,
    }
}

/// `IS_VALID_SALT_CHARACTER`: used to check DES salts.
fn is_valid_salt_character(c: u8) -> bool {
    c.is_ascii_alphabetic() || (b'.'..=b'9').contains(&c)
}

/// `php_crypt()`: hashes `password` according to the format of `salt` and
/// returns `None` where php-src returns NULL (unknown format, invalid salt,
/// rounds out of range, ...).
///
/// Both arguments are treated as C strings, like in php-src: they end at the
/// first NUL byte, and the salt is never read past that.
pub fn php_crypt(password: &[u8], salt: &[u8]) -> Option<Vec<u8>> {
    let key = c_str(password);
    let salt = c_str(salt);
    let at = |i: usize| salt.get(i).copied().unwrap_or(0);

    if at(0) == b'*' && (at(1) == b'0' || at(1) == b'1') {
        return None;
    }

    if at(0) == b'$' && at(1) == b'1' && at(2) == b'$' {
        Some(md5::md5_crypt(key, salt))
    } else if at(0) == b'$' && at(1) == b'6' && at(2) == b'$' {
        sha::sha_crypt(key, salt, sha::Variant::Sha512)
    } else if at(0) == b'$' && at(1) == b'5' && at(2) == b'$' {
        sha::sha_crypt(key, salt, sha::Variant::Sha256)
    } else if at(0) == b'$' && at(1) == b'2' && at(2) != 0 && at(3) == b'$' {
        blowfish::crypt_blowfish(key, salt)
    } else if at(0) == b'_' || (is_valid_salt_character(at(0)) && is_valid_salt_character(at(1))) {
        // DES fallback.
        des::crypt_extended(key, salt)
    } else {
        // Unknown hash type.
        None
    }
}

/// `php_safe_bcmp()`: a constant-time equality check that always looks at every
/// byte; returns 0 if both inputs match, non-zero if they don't (-1 early when
/// the lengths differ).
pub fn php_safe_bcmp(a: &[u8], b: &[u8]) -> i32 {
    if a.len() != b.len() {
        return -1;
    }
    let mut r = 0u8;
    for (x, y) in a.iter().zip(b) {
        r |= std::hint::black_box(*x ^ *y);
    }
    r as i32
}

/// Hash a string
fn crypt(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let password = args.str(0).clone();
    let salt_in = args.str(1).clone();

    // php-src copies at most PHP_MAX_SALT_LEN bytes of the salt into a
    // NUL-terminated buffer.
    let salt = &salt_in.as_bytes()[..salt_in.len().min(PHP_MAX_SALT_LEN)];
    match php_crypt(password.as_bytes(), salt) {
        Some(result) => Ok(Value::from(result)),
        None => {
            let salt = c_str(salt);
            if salt.first() == Some(&b'*') && salt.get(1) == Some(&b'0') {
                Ok(Value::from("*1"))
            } else {
                Ok(Value::from("*0"))
            }
        }
    }
}

pub static CRYPT: NativeFunction = NativeFunction {
    sig: "crypt(#[\\SensitiveParameter] string $string, string $salt): string",
    handler: crypt,
};

pub fn register(r: &mut Registry) {
    r.function(&CRYPT);

    r.constant("CRYPT_SALT_LENGTH", PHP_MAX_SALT_LEN as i64);
    r.constant("CRYPT_STD_DES", 1i64);
    r.constant("CRYPT_EXT_DES", 1i64);
    r.constant("CRYPT_MD5", 1i64);
    r.constant("CRYPT_BLOWFISH", 1i64);
    r.constant("CRYPT_SHA256", 1i64);
    r.constant("CRYPT_SHA512", 1i64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn crypt_str(vm: &mut TestVm, pw: &[u8], salt: &[u8]) -> String {
        let v = vm
            .call("crypt", vec![Value::from(pw), Value::from(salt)])
            .unwrap();
        match v {
            Value::String(s) => s.to_string_lossy().into_owned(),
            other => panic!("expected string, got {other:?}"),
        }
    }

    fn c(pw: &str, salt: &str) -> String {
        let mut vm = TestVm::new();
        crypt_str(&mut vm, pw.as_bytes(), salt.as_bytes())
    }

    #[test]
    fn constants() {
        let vm = TestVm::new();
        assert!(matches!(
            vm.vm.constant("CRYPT_SALT_LENGTH"),
            Some(Value::Int(123))
        ));
        for name in [
            "CRYPT_STD_DES",
            "CRYPT_EXT_DES",
            "CRYPT_MD5",
            "CRYPT_BLOWFISH",
            "CRYPT_SHA256",
            "CRYPT_SHA512",
        ] {
            assert!(
                matches!(vm.vm.constant(name), Some(Value::Int(1))),
                "{name}"
            );
        }
    }

    /// ext/standard/tests/strings/crypt.phpt
    #[test]
    fn crypt_phpt() {
        let mut vm = TestVm::new();
        assert_eq!(crypt_str(&mut vm, b"rasmuslerdorf", b"rl"), "rl.3StKT.4T8M");
        assert_eq!(
            crypt_str(&mut vm, b"rasmuslerdorf", b"_J9..rasm"),
            "_J9..rasmBYk8r9AiWNc"
        );
        assert_eq!(
            crypt_str(&mut vm, b"rasmuslerdorf", b"$1$rasmusle$"),
            "$1$rasmusle$rISCgZzpwk3UhDidwXvin0"
        );
        assert_eq!(
            crypt_str(&mut vm, b"rasmuslerdorf", b"$2a$07$rasmuslerd............"),
            "$2a$07$rasmuslerd............nIdrcHdxcUxWomQX9j6kvERCFjTg7Ra"
        );

        let r = vm.call("crypt", vec![Value::from("rasmuslerdorf")]);
        assert_eq!(
            thrown(r),
            (
                "ArgumentCountError".to_string(),
                "crypt() expects exactly 2 arguments, 1 given".to_string()
            )
        );
    }

    /// ext/standard/tests/strings/crypt_chars.phpt
    #[test]
    fn crypt_chars_phpt() {
        assert_eq!(c("À1234abcd", "99"), "99PxawtsTfX56");
        assert_eq!(c("À9234abcd", "99"), "99jcVcGxUZOWk");
        assert_eq!(c("À1234abcd", "_01234567"), "_01234567IBjxKliXXRQ");
        assert_eq!(c("À9234abcd", "_01234567"), "_012345678OSGpGQRVHA");
    }

    /// ext/standard/tests/strings/crypt_des_error.phpt
    #[test]
    fn crypt_des_error_phpt() {
        assert_eq!(c("foo", "*0"), "*1");
        assert_eq!(c("foo", "*1"), "*0");
        assert_eq!(c("foo", "*"), "*0");
    }

    /// ext/standard/tests/crypt/des_fallback_invalid_salt.phpt
    #[test]
    fn des_fallback_invalid_salt_phpt() {
        assert_eq!(c("test", "$:#"), "*0");
        assert_eq!(c("test", "$:5zd$01\n"), "*0");
    }

    #[test]
    fn short_and_odd_salts_fail() {
        assert_eq!(c("test", ""), "*0");
        assert_eq!(c("test", "a"), "*0");
        assert_eq!(c("test", "$"), "*0");
        assert_eq!(c("test", "$1"), "*0");
        assert_eq!(c("test", "$2"), "*0");
        assert_eq!(c("test", "$2a"), "*0");
        assert_eq!(c("test", "$7$abc"), "*0");
        assert_eq!(c("test", "a:"), "*0");
        // A salt is a C string: a NUL ends it.
        assert_eq!(c("test", "a\0b"), "*0");
        assert_eq!(c("test", "ab\0cd"), c("test", "ab"));
        // Only the first eight characters of a DES password matter.
        assert_eq!(c("12345678", "ab"), c("12345678extra", "ab"));
        // Passwords are C strings too.
        assert_eq!(c("pw\0more", "ab"), c("pw", "ab"));
    }

    /// ext/standard/tests/crypt/bcrypt_invalid_algorithm.phpt and
    /// bcrypt_invalid_cost.phpt
    #[test]
    fn bcrypt_invalid_phpt() {
        for salt in [
            "$23$04$1234567890123456789012345",
            "$20$04$1234567890123456789012345",
            "$2g$04$1234567890123456789012345",
            "$2a$4$1234567891234567891234567",
            "$2a$00$1234567891234567891234567",
            "$2a$01$1234567891234567891234567",
            "$2a$02$1234567891234567891234567",
            "$2a$03$1234567891234567891234567",
            "$2a$32$1234567891234567891234567",
            "$2a$40$1234567891234567891234567",
        ] {
            assert_eq!(c("test", salt), "*0", "{salt}");
        }
    }

    /// ext/standard/tests/crypt/bcrypt_salt_dollar.phpt
    #[test]
    fn bcrypt_salt_dollar_phpt() {
        for i in 0..22 {
            let salt = format!("$2y$04${}$", "0".repeat(i));
            assert_eq!(c("foo", &salt), "*0", "{salt}");
        }
        let salt = format!("$2y$04${}$", "0".repeat(22));
        assert_eq!(
            c("foo", &salt),
            "$2y$04$000000000000000000000u2a2UpVexIt9k3FMJeAVr3c04F5tcI8K"
        );
    }

    /// ext/standard/tests/strings/crypt_blowfish_variation{1,2}.phpt
    #[test]
    fn blowfish_variations_phpt() {
        for cost in 32..=38 {
            let r = c("U*U", &format!("$2a${cost}$CCCCCCCCCCCCCCCCCCCCCC$"));
            assert!(r == "*0" || r == "*1", "{cost}");
        }
        assert_eq!(
            c(
                "U*U",
                "$2a$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
            ),
            "*0"
        );
    }

    /// ext/standard/tests/strings/crypt_blowfish.phpt: `$tests2`.
    #[test]
    fn blowfish_failures_phpt() {
        for salt in [
            "$2a$03$CCCCCCCCCCCCCCCCCCCCC.",
            "$2a$32$CCCCCCCCCCCCCCCCCCCCC.",
            "$2c$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2z$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2`$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2{$05$CCCCCCCCCCCCCCCCCCCCC.",
        ] {
            assert_eq!(c("", salt), "*0", "{salt}");
        }
        assert_eq!(c("", "*0"), "*1");
    }

    #[test]
    fn blowfish_vectors_through_crypt() {
        // Owl crypt_blowfish wrapper vectors.
        let hash = "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW";
        assert_eq!(c("U*U", hash), hash);
        let hash = "$2y$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e";
        let mut vm = TestVm::new();
        assert_eq!(crypt_str(&mut vm, b"\xff\xff\xa3", hash.as_bytes()), hash);
        let hash = "$2x$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e";
        assert_eq!(crypt_str(&mut vm, b"\xa3", hash.as_bytes()), hash);
        // 'chars after 72 are ignored'.
        let hash = "$2a$05$abcdefghijklmnopqrstuu5s2v8.iXieOjg/.AySBTTZIIVFJeBui";
        assert_eq!(
            c(
                "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789chars after 72 are ignored",
                hash
            ),
            hash
        );
        // Trailing garbage after the 29-character setting is ignored.
        assert_eq!(c("U*U", &format!("{hash}garbage"))[..7], hash[..7]);
    }

    /// ext/standard/tests/strings/crypt_sha256.phpt
    #[test]
    fn crypt_sha256_phpt() {
        let tests = [
            (
                "$5$saltstring",
                "Hello world!",
                "$5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5",
            ),
            (
                "$5$rounds=10000$saltstringsaltstring",
                "Hello world!",
                "$5$rounds=10000$saltstringsaltst$3xv.VbSHBb41AL9AvLeujZkZRBAwqFMz2.opqey6IcA",
            ),
            (
                "$5$rounds=1000000000$roundstoohigh",
                "the number of rounds is too high",
                "*0",
            ),
        ];
        for (salt, pw, expected) in tests {
            assert_eq!(c(pw, salt), expected);
        }
    }

    /// ext/standard/tests/strings/crypt_sha512.phpt
    #[test]
    fn crypt_sha512_phpt() {
        let tests = [
            (
                "$6$saltstring",
                "Hello world!",
                "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1",
            ),
            (
                "$6$$bar$",
                "foo",
                "$6$$QMXjqd7rHQZPQ1yHsXkQqC1FBzDiVfTHXL.LaeDAeVV.IzMaV9VU4MQ8kPuZa2SOP1A0RPm772EaFYjpEJtdu.",
            ),
            (
                "$6$rounds=10$roundstoolow",
                "the number of rounds is too low",
                "*0",
            ),
            (
                "$6$rounds=1000000000$roundstoohigh",
                "the number of rounds is too high",
                "*0",
            ),
        ];
        for (salt, pw, expected) in tests {
            assert_eq!(c(pw, salt), expected);
        }
    }

    /// ext/standard/tests/strings/crypt_variation1.phpt
    #[test]
    fn crypt_variation1_phpt() {
        for n in [124usize, 125, 4096] {
            let salt = format!("$5${}", "A".repeat(n));
            assert_eq!(
                c("A", &salt),
                "$5$AAAAAAAAAAAAAAAA$frotiiztWZiwcncxnY5tWG9Ida2WOZEximjLXCleQu6"
            );
        }
    }

    #[test]
    fn php_crypt_helper() {
        assert_eq!(
            php_crypt(b"rasmuslerdorf", b"rl").as_deref(),
            Some(&b"rl.3StKT.4T8M"[..])
        );
        assert_eq!(php_crypt(b"x", b"*0"), None);
        assert_eq!(php_crypt(b"x", b"$"), None);
        assert_eq!(php_safe_bcmp(b"abc", b"abc"), 0);
        assert_ne!(php_safe_bcmp(b"abc", b"abd"), 0);
        assert_eq!(php_safe_bcmp(b"abc", b"ab"), -1);
    }
}
