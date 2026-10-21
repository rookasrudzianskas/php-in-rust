//! MD5-based `crypt()` (`$1$`) — `php_md5_crypt_r()` in ext/standard/php_crypt_r.c.

use super::digest::{Digest, algorithm};

const MD5_MAGIC: &[u8] = b"$1$";

const ITOA64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `to64()`: appends `n` base64 digits of `v`, least significant first.
fn to64(out: &mut Vec<u8>, mut v: u32, n: usize) {
    for _ in 0..n {
        out.push(ITOA64[(v & 0x3f) as usize]);
        v >>= 6;
    }
}

/// `pw` and `salt` are C-string contents (no NUL).
pub fn md5_crypt(pw: &[u8], salt: &[u8]) -> Vec<u8> {
    let md5 = algorithm("md5");
    let pwl = pw.len();

    // If the salt starts with the magic string, skip that.
    let mut sp = salt;
    if sp.starts_with(MD5_MAGIC) {
        sp = &sp[MD5_MAGIC.len()..];
    }

    // It stops at the first '$', max 8 chars.
    let sl = sp
        .iter()
        .take(8)
        .take_while(|&&c| c != 0 && c != b'$')
        .count();
    let sp = &sp[..sl];

    let mut ctx = Digest::new(md5);

    // The password first, since that is what is most unknown.
    ctx.update(pw);
    // Then our magic string, then the raw salt.
    ctx.update(MD5_MAGIC);
    ctx.update(sp);

    // Then just as many characters of the MD5(pw, salt, pw).
    let mut ctx1 = Digest::new(md5);
    ctx1.update(pw);
    ctx1.update(sp);
    ctx1.update(pw);
    let fin = ctx1.finish();

    let mut pl = pwl as isize;
    while pl > 0 {
        ctx.update(&fin[..pl.min(16) as usize]);
        pl -= 16;
    }

    // Then something really weird: `final` was wiped, so a set bit hashes a
    // NUL byte and a clear bit the first character of the password.
    let mut i = pwl;
    while i != 0 {
        if i & 1 != 0 {
            ctx.update(&[0]);
        } else {
            ctx.update(&pw[..1]);
        }
        i >>= 1;
    }

    let mut out = Vec::with_capacity(MD5_MAGIC.len() + sl + 1 + 22);
    out.extend_from_slice(MD5_MAGIC);
    out.extend_from_slice(sp);
    out.push(b'$');

    let mut fin = ctx.finish();

    // And now, just to make sure things don't run too fast.
    for i in 0..1000u32 {
        let mut ctx1 = Digest::new(md5);
        if i & 1 != 0 {
            ctx1.update(pw);
        } else {
            ctx1.update(&fin);
        }
        if i % 3 != 0 {
            ctx1.update(sp);
        }
        if i % 7 != 0 {
            ctx1.update(pw);
        }
        if i & 1 != 0 {
            ctx1.update(&fin);
        } else {
            ctx1.update(pw);
        }
        fin = ctx1.finish();
    }

    let f = |i: usize| fin[i] as u32;
    to64(&mut out, (f(0) << 16) | (f(6) << 8) | f(12), 4);
    to64(&mut out, (f(1) << 16) | (f(7) << 8) | f(13), 4);
    to64(&mut out, (f(2) << 16) | (f(8) << 8) | f(14), 4);
    to64(&mut out, (f(3) << 16) | (f(9) << 8) | f(15), 4);
    to64(&mut out, (f(4) << 16) | (f(10) << 8) | f(5), 4);
    to64(&mut out, f(11), 2);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5c(pw: &str, salt: &str) -> String {
        String::from_utf8(md5_crypt(pw.as_bytes(), salt.as_bytes())).unwrap()
    }

    #[test]
    fn php_vector() {
        assert_eq!(
            md5c("rasmuslerdorf", "$1$rasmusle$"),
            "$1$rasmusle$rISCgZzpwk3UhDidwXvin0"
        );
    }

    #[test]
    fn independent_vectors() {
        // Cross-checked with LibreSSL's `openssl passwd -1`.
        assert_eq!(
            md5c("password", "$1$saltsalt$"),
            "$1$saltsalt$qjXMvbEw8oaL.CzflDtaK/"
        );
        assert_eq!(md5c("", "$1$"), "$1$$qRPK7m23GJusamGpoGLby/");
    }

    #[test]
    fn salt_is_truncated_to_eight_chars() {
        let a = md5c("pw", "$1$0123456789$");
        assert!(a.starts_with("$1$01234567$"));
        assert_eq!(a.len(), 3 + 8 + 1 + 22);
        // Without the magic prefix.
        assert_eq!(md5c("pw", "01234567"), a);
    }
}
