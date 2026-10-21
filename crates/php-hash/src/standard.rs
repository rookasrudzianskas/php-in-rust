//! The `md5()` / `sha1()` helpers of `ext/standard`.

use crate::hasher::{Engine, to_hex};
use crate::md::Md5;
use crate::sha1::Sha1;

/// `md5($string, true)`: the raw 16-byte MD5 digest.
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut m = Md5::new();
    m.update(data);
    m.finish().try_into().expect("md5 digests are 16 bytes")
}

/// `md5($string)`: the lower-case hexadecimal MD5 digest.
pub fn md5_hex(data: &[u8]) -> String {
    to_hex(&md5(data))
}

/// `sha1($string, true)`: the raw 20-byte SHA-1 digest.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut s = Sha1::new();
    s.update(data);
    s.finish().try_into().expect("sha1 digests are 20 bytes")
}

/// `sha1($string)`: the lower-case hexadecimal SHA-1 digest.
pub fn sha1_hex(data: &[u8]) -> String {
    to_hex(&sha1(data))
}
