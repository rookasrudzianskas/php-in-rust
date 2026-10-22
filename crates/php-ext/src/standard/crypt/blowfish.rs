//! bcrypt (`$2a$`, `$2b$`, `$2x$`, `$2y$`) — ext/standard/crypt_blowfish.c.
//!
//! Solar Designer's crypt_blowfish, including the `$2x$` sign-extension bug
//! compatibility and the `$2a$` safety measure for passwords with `\xff`.

use super::blowfish_tables::{P as INIT_P, S as INIT_S};

const BF_N: usize = 16;

/// Magic IV for the 64 Blowfish encryptions done at the end
/// ("OrpheanBeholderScryDoubt" on big-endian).
const BF_MAGIC_W: [u32; 6] = [
    0x4F727068, 0x65616E42, 0x65686F6C, 0x64657253, 0x63727944, 0x6F756274,
];

const BF_ITOA64: &[u8; 64] = b"./ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

const BF_ATOI64: [u8; 0x60] = [
    64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 64, 0, 1, 54, 55, 56, 57, 58, 59, 60, 61,
    62, 63, 64, 64, 64, 64, 64, 64, 64, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
    19, 20, 21, 22, 23, 24, 25, 26, 27, 64, 64, 64, 64, 64, 64, 28, 29, 30, 31, 32, 33, 34, 35, 36,
    37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 64, 64, 64, 64, 64,
];

/// `BF_safe_atoi64`.
fn safe_atoi64(c: u8) -> Option<u32> {
    let t = c.wrapping_sub(0x20) as usize;
    if t >= 0x60 {
        return None;
    }
    let v = BF_ATOI64[t];
    if v > 63 { None } else { Some(v as u32) }
}

/// `BF_decode`: decodes `size` bytes from the base64 variant in `src`
/// (reads past the end see NUL, which is invalid).
fn bf_decode(src: &[u8], size: usize) -> Option<Vec<u8>> {
    let at = |i: usize| src.get(i).copied().unwrap_or(0);
    let mut out = Vec::with_capacity(size);
    let mut sp = 0usize;
    loop {
        let c1 = safe_atoi64(at(sp))?;
        let c2 = safe_atoi64(at(sp + 1))?;
        sp += 2;
        out.push(((c1 << 2) | ((c2 & 0x30) >> 4)) as u8);
        if out.len() >= size {
            break;
        }
        let c3 = safe_atoi64(at(sp))?;
        sp += 1;
        out.push((((c2 & 0x0F) << 4) | ((c3 & 0x3C) >> 2)) as u8);
        if out.len() >= size {
            break;
        }
        let c4 = safe_atoi64(at(sp))?;
        sp += 1;
        out.push((((c3 & 0x03) << 6) | c4) as u8);
        if out.len() >= size {
            break;
        }
    }
    Some(out)
}

/// `BF_encode`: encodes the first `size` bytes of `src`.
fn bf_encode(out: &mut Vec<u8>, src: &[u8], size: usize) {
    let mut sp = 0usize;
    loop {
        let mut c1 = src[sp] as u32;
        sp += 1;
        out.push(BF_ITOA64[(c1 >> 2) as usize]);
        c1 = (c1 & 0x03) << 4;
        if sp >= size {
            out.push(BF_ITOA64[c1 as usize]);
            break;
        }

        let mut c2 = src[sp] as u32;
        sp += 1;
        c1 |= c2 >> 4;
        out.push(BF_ITOA64[c1 as usize]);
        c1 = (c2 & 0x0f) << 2;
        if sp >= size {
            out.push(BF_ITOA64[c1 as usize]);
            break;
        }

        c2 = src[sp] as u32;
        sp += 1;
        c1 |= c2 >> 6;
        out.push(BF_ITOA64[c1 as usize]);
        out.push(BF_ITOA64[(c2 & 0x3f) as usize]);
        if sp >= size {
            break;
        }
    }
}

struct Ctx {
    s: [[u32; 256]; 4],
    p: [u32; BF_N + 2],
}

impl Ctx {
    /// `BF_ROUND`.
    #[inline(always)]
    fn round(&self, l: u32, r: &mut u32, n: usize) {
        let mut tmp3 =
            self.s[1][((l >> 16) & 0xff) as usize].wrapping_add(self.s[0][(l >> 24) as usize]);
        tmp3 ^= self.s[2][((l >> 8) & 0xff) as usize];
        *r ^= self.p[n + 1];
        tmp3 = tmp3.wrapping_add(self.s[3][(l & 0xff) as usize]);
        *r ^= tmp3;
    }

    /// `BF_ENCRYPT`: encrypts one block (`BF_N` is hardcoded to 16 rounds).
    #[inline(always)]
    fn encrypt(&self, mut l: u32, mut r: u32) -> (u32, u32) {
        l ^= self.p[0];
        let mut n = 0;
        while n < BF_N {
            self.round(l, &mut r, n);
            self.round(r, &mut l, n + 1);
            n += 2;
        }
        (r ^ self.p[BF_N + 1], l)
    }

    /// `BF_body`: re-keys the whole P array and all S-boxes.
    fn body(&mut self) {
        let (mut l, mut r) = (0u32, 0u32);
        for i in (0..BF_N + 2).step_by(2) {
            (l, r) = self.encrypt(l, r);
            self.p[i] = l;
            self.p[i + 1] = r;
        }
        for box_i in 0..4 {
            for j in (0..256).step_by(2) {
                (l, r) = self.encrypt(l, r);
                self.s[box_i][j] = l;
                self.s[box_i][j + 1] = r;
            }
        }
    }
}

/// `BF_set_key`. `key` is the C string (no NUL); the NUL terminator takes
/// part in the cyclic reading like in php-src.
fn bf_set_key(
    key: &[u8],
    expanded: &mut [u32; BF_N + 2],
    initial: &mut [u32; BF_N + 2],
    flags: u8,
) {
    let bug = (flags & 1) as usize;
    let safety = ((flags as u32) & 2) << 15;
    let mut sign = 0u32;
    let mut diff = 0u32;
    let mut pos = 0usize;

    for i in 0..BF_N + 2 {
        let mut tmp = [0u32; 2];
        for j in 0..4 {
            let c = key.get(pos).copied().unwrap_or(0);
            tmp[0] = (tmp[0] << 8) | c as u32; // correct
            tmp[1] = (tmp[1] << 8) | (c as i8 as i32 as u32); // bug: sign extension
            // Sign extension in the first char has no effect; for chars 2, 3,
            // 4 of each block, bit 7 of "sign" records it.
            if j != 0 {
                sign |= tmp[1] & 0x80;
            }
            if c == 0 {
                pos = 0;
            } else {
                pos += 1;
            }
        }
        diff |= tmp[0] ^ tmp[1]; // non-zero on any differences
        expanded[i] = tmp[bug];
        initial[i] = INIT_P[i] ^ tmp[bug];
    }

    // "diff" is zero iff the correct and buggy algorithms produced exactly the
    // same result; if so and "sign" is non-zero we have a collision between
    // the correct hash and a set of buggy passwords, and deviate by flipping
    // bit 16 of the initial (not the expanded) key.
    diff |= diff >> 16;
    diff &= 0xffff;
    diff = diff.wrapping_add(0xffff); // bit 16 set iff "diff" was non-zero
    sign <<= 9;
    sign &= !diff & safety;
    initial[0] ^= sign;
}

/// `flags_by_subtype`: indexed by the letter after `$2`.
fn flags_by_subtype(c: u8) -> u8 {
    match c {
        b'a' => 2,
        b'b' => 4,
        b'x' => 1,
        b'y' => 4,
        _ => 0,
    }
}

/// `BF_crypt()`; `min` is the minimum iteration count (not cost).
fn bf_crypt(key: &[u8], setting: &[u8], min: u32) -> Option<Vec<u8>> {
    let set = |i: usize| setting.get(i).copied().unwrap_or(0);

    if set(0) != b'$'
        || set(1) != b'2'
        || !set(2).is_ascii_lowercase()
        || flags_by_subtype(set(2)) == 0
        || set(3) != b'$'
        || set(4) < b'0'
        || set(4) > b'3'
        || !set(5).is_ascii_digit()
        || (set(4) == b'3' && set(5) > b'1')
        || set(6) != b'$'
    {
        return None;
    }

    let count: u32 = 1u32 << ((set(4) - b'0') as u32 * 10 + (set(5) - b'0') as u32);
    if count < min {
        return None;
    }
    let salt_bytes = bf_decode(setting.get(7..).unwrap_or(&[]), 16)?;
    let mut salt = [0u32; 4];
    for (i, w) in salt.iter_mut().enumerate() {
        *w = u32::from_be_bytes([
            salt_bytes[4 * i],
            salt_bytes[4 * i + 1],
            salt_bytes[4 * i + 2],
            salt_bytes[4 * i + 3],
        ]);
    }

    let mut expanded_key = [0u32; BF_N + 2];
    let mut ctx = Ctx {
        s: INIT_S,
        p: [0; BF_N + 2],
    };
    bf_set_key(key, &mut expanded_key, &mut ctx.p, flags_by_subtype(set(2)));

    let (mut l, mut r) = (0u32, 0u32);
    for i in (0..BF_N + 2).step_by(2) {
        l ^= salt[i & 2];
        r ^= salt[(i & 2) + 1];
        (l, r) = ctx.encrypt(l, r);
        ctx.p[i] = l;
        ctx.p[i + 1] = r;
    }

    for box_i in 0..4 {
        for j in (0..256).step_by(4) {
            l ^= salt[(BF_N + 2) & 3];
            r ^= salt[(BF_N + 3) & 3];
            (l, r) = ctx.encrypt(l, r);
            ctx.s[box_i][j] = l;
            ctx.s[box_i][j + 1] = r;

            l ^= salt[(BF_N + 4) & 3];
            r ^= salt[(BF_N + 5) & 3];
            (l, r) = ctx.encrypt(l, r);
            ctx.s[box_i][j + 2] = l;
            ctx.s[box_i][j + 3] = r;
        }
    }

    let mut remaining = count;
    loop {
        for i in (0..BF_N + 2).step_by(2) {
            ctx.p[i] ^= expanded_key[i];
            ctx.p[i + 1] ^= expanded_key[i + 1];
        }

        ctx.body();

        for i in (0..BF_N).step_by(4) {
            ctx.p[i] ^= salt[0];
            ctx.p[i + 1] ^= salt[1];
            ctx.p[i + 2] ^= salt[2];
            ctx.p[i + 3] ^= salt[3];
        }
        ctx.p[16] ^= salt[0];
        ctx.p[17] ^= salt[1];

        ctx.body();

        remaining -= 1;
        if remaining == 0 {
            break;
        }
    }

    let mut output = [0u32; 6];
    for i in (0..6).step_by(2) {
        let (mut l, mut r) = (BF_MAGIC_W[i], BF_MAGIC_W[i + 1]);
        for _ in 0..64 {
            (l, r) = ctx.encrypt(l, r);
        }
        output[i] = l;
        output[i + 1] = r;
    }

    let mut out = Vec::with_capacity(7 + 22 + 31);
    out.extend_from_slice(&[set(0), set(1), set(2), set(3), set(4), set(5), set(6)]);
    out.extend((7..7 + 22 - 1).map(set));
    out.push(BF_ITOA64[(BF_ATOI64[set(7 + 22 - 1) as usize - 0x20] & 0x30) as usize]);

    // This has to be bug-compatible with the original implementation, so only
    // encode 23 of the 24 bytes.
    let mut out_bytes = [0u8; 24];
    for (i, w) in output.iter().enumerate() {
        out_bytes[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
    }
    bf_encode(&mut out, &out_bytes, 23);
    Some(out)
}

/// `php_crypt_blowfish_rn()` without the runtime self-test (covered by this
/// module's unit tests): `key` and `setting` are the C strings' contents.
pub fn crypt_blowfish(key: &[u8], setting: &[u8]) -> Option<Vec<u8>> {
    bf_crypt(key, setting, 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bf(key: &[u8], setting: &str) -> Option<String> {
        crypt_blowfish(key, setting.as_bytes()).map(|v| String::from_utf8(v).unwrap())
    }

    /// `php_crypt_blowfish_rn()`'s own runtime self-test.
    #[test]
    fn builtin_self_test() {
        let test_key = b"8b \xd0\xc1\xd2\xcf\xcc\xd8";
        for (sub, expect) in [
            ('a', "i1D709vfamulimlGcq0qq3UvuUasvEa"),
            ('b', "i1D709vfamulimlGcq0qq3UvuUasvEa"),
            ('y', "i1D709vfamulimlGcq0qq3UvuUasvEa"),
            ('x', "VUrPmXD6q/nVSSp7pNDhCR9071IfIRe"),
        ] {
            let setting = format!("$2{sub}$00$abcdefghijklmnopqrstuu");
            let out = bf_crypt(test_key, setting.as_bytes(), 1).unwrap();
            assert_eq!(&out[..29], setting.as_bytes());
            assert_eq!(String::from_utf8_lossy(&out[29..]), expect, "$2{sub}$");
        }

        let k = b"\xff\xa3\x33\x34\xff\xff\xff\xa3\x33\x34\x35";
        let (mut ae, mut ai, mut ye, mut yi) = ([0; 18], [0; 18], [0; 18], [0; 18]);
        bf_set_key(k, &mut ae, &mut ai, 2);
        bf_set_key(k, &mut ye, &mut yi, 4);
        ai[0] ^= 0x10000; // undo the safety (for comparison)
        assert_eq!(ai[0], 0xdb9c59bc);
        assert_eq!(ye[17], 0x33343500);
        assert_eq!(ae, ye);
        assert_eq!(ai, yi);
    }

    #[test]
    fn owl_vector() {
        assert_eq!(
            bf(
                b"U*U",
                "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
            )
            .unwrap(),
            "$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW"
        );
    }

    /// Hashes produced by Apache's `htpasswd -B` (apr-util's bcrypt), which
    /// give an independent check of cost handling and key encoding.
    #[test]
    fn htpasswd_vectors() {
        let long = "x".repeat(100);
        let tests: [(&[u8], &str); 4] = [
            (
                b"a",
                "$2y$04$cBbrkH4gajn.SbLSE1dD/uSXaWxMTR79V.yzs9.DrY5LJFrRrydGq",
            ),
            (
                b"correct horse battery staple",
                "$2y$05$YEooAogQIwcSlF76XDecDugDCBi2NETZzXkFsuwIPxNyN9WjKekKa",
            ),
            (
                "p\u{e4}ssword".as_bytes(),
                "$2y$06$NOl38xWI3KEW08iOsgSfbOJmC5.kR9jwHTWL793PPIJR2Ag.45oGu",
            ),
            (
                long.as_bytes(),
                "$2y$04$yP6VhyH.uob84JyOgQP4EeOR77oGKeAGRkalJhIptiE33uTiYsAN2",
            ),
        ];
        for (pw, hash) in tests {
            assert_eq!(bf(pw, hash).as_deref(), Some(hash));
        }
    }

    #[test]
    fn rasmus_vector() {
        assert_eq!(
            bf(b"rasmuslerdorf", "$2a$07$rasmuslerd............").unwrap(),
            "$2a$07$rasmuslerd............nIdrcHdxcUxWomQX9j6kvERCFjTg7Ra"
        );
    }

    #[test]
    fn invalid_settings() {
        // ext/standard/tests/strings/crypt_blowfish.phpt ($tests2) and
        // crypt/bcrypt_invalid_*.phpt
        for s in [
            "$2a$03$CCCCCCCCCCCCCCCCCCCCC.",
            "$2a$32$CCCCCCCCCCCCCCCCCCCCC.",
            "$2c$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2z$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2`$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2{$05$CCCCCCCCCCCCCCCCCCCCC.",
            "$2a$4$1234567891234567891234567",
            "$2a$00$1234567891234567891234567",
            "$2a$40$1234567891234567891234567",
            "$2a$33$CCCCCCCCCCCCCCCCCCCCCC$",
            "$2a$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW",
            "$2y$04$000000000000000000000$",
        ] {
            assert_eq!(bf(b"", s), None, "{s}");
        }
    }

    #[test]
    fn dollar_in_salt() {
        // bcrypt_salt_dollar.phpt: the 22nd character may be anything valid.
        assert_eq!(
            bf(b"foo", "$2y$04$0000000000000000000000$").unwrap(),
            "$2y$04$000000000000000000000u2a2UpVexIt9k3FMJeAVr3c04F5tcI8K"
        );
    }

    /// The vectors of ext/standard/tests/strings/crypt_blowfish.phpt, which
    /// come from Owl's crypt_blowfish wrapper.c.
    #[test]
    fn php_crypt_blowfish_phpt() {
        let repeat = |s: &[u8], n: usize| s.repeat(n);
        let tests: Vec<(&str, Vec<u8>)> = vec![
            ("$2a$05$CCCCCCCCCCCCCCCCCCCCC.E5YPO9kmyuRGyh0XouQYb4YMJKvyOeW", b"U*U".to_vec()),
            ("$2a$05$CCCCCCCCCCCCCCCCCCCCC.VGOzA784oUp/Z0DY336zx7pLYAy0lwK", b"U*U*".to_vec()),
            ("$2a$05$XXXXXXXXXXXXXXXXXXXXXOAcXxm9kjPGEMsLznoKqmqw7tc8WCx4a", b"U*U*U".to_vec()),
            ("$2a$05$abcdefghijklmnopqrstuu5s2v8.iXieOjg/.AySBTTZIIVFJeBui",
             b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789chars after 72 are ignored".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e", b"\xa3".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e", b"\xff\xff\xa3".to_vec()),
            ("$2y$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e", b"\xff\xff\xa3".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.nqd1wy.pTMdcvrRWxyiGL2eMz.2a85.", b"\xff\xff\xa3".to_vec()),
            ("$2b$05$/OK.fbVrR/bpIqNJ5ianF.CE5elHaaO4EbggVDjb8P19RukzXSM3e", b"\xff\xff\xa3".to_vec()),
            ("$2y$05$/OK.fbVrR/bpIqNJ5ianF.Sa7shbm4.OzKpvFnX1pQLmQW96oUlCq", b"\xa3".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.Sa7shbm4.OzKpvFnX1pQLmQW96oUlCq", b"\xa3".to_vec()),
            ("$2b$05$/OK.fbVrR/bpIqNJ5ianF.Sa7shbm4.OzKpvFnX1pQLmQW96oUlCq", b"\xa3".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.o./n25XVfn6oAPaUvHe.Csk4zRfsYPi", b"1\xa3345".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.o./n25XVfn6oAPaUvHe.Csk4zRfsYPi", b"\xff\xa3345".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.o./n25XVfn6oAPaUvHe.Csk4zRfsYPi", b"\xff\xa334\xff\xff\xff\xa3345".to_vec()),
            ("$2y$05$/OK.fbVrR/bpIqNJ5ianF.o./n25XVfn6oAPaUvHe.Csk4zRfsYPi", b"\xff\xa334\xff\xff\xff\xa3345".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.ZC1JEJ8Z4gPfpe1JOr/oyPXTWl9EFd.", b"\xff\xa334\xff\xff\xff\xa3345".to_vec()),
            ("$2y$05$/OK.fbVrR/bpIqNJ5ianF.nRht2l/HRhr6zmCp9vYUvvsqynflf9e", b"\xff\xa3345".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.nRht2l/HRhr6zmCp9vYUvvsqynflf9e", b"\xff\xa3345".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.6IflQkJytoRVc1yuaNtHfiuq.FRlSIS", b"\xa3ab".to_vec()),
            ("$2x$05$/OK.fbVrR/bpIqNJ5ianF.6IflQkJytoRVc1yuaNtHfiuq.FRlSIS", b"\xa3ab".to_vec()),
            ("$2y$05$/OK.fbVrR/bpIqNJ5ianF.6IflQkJytoRVc1yuaNtHfiuq.FRlSIS", b"\xa3ab".to_vec()),
            ("$2x$05$6bNw2HLQYeqHYyBfLMsv/OiwqTymGIGzFsA4hOTWebfehXHNprcAS", b"\xd1\x91".to_vec()),
            ("$2x$05$6bNw2HLQYeqHYyBfLMsv/O9LIGgn8OMzuDoHfof8AQimSGfcSWxnS", b"\xd0\xc1\xd2\xcf\xcc\xd8".to_vec()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.swQOIzjOiJ9GHEPuhEkvqrUyvWhEMx6",
             [repeat(b"\xaa", 72), b"chars after 72 are ignored as usual".to_vec()].concat()),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.R9xrDjiycxMbQE2bp.vgqlYpW5wx2yy", repeat(b"\xaa\x55", 36)),
            ("$2a$05$/OK.fbVrR/bpIqNJ5ianF.9tQZzcJfm3uj2NvJ/n5xkhpqLrMpWCe", repeat(b"\x55\xaa\xff", 24)),
            ("$2a$05$CCCCCCCCCCCCCCCCCCCCC.7uG0VCzI2bS7j6ymqJi9CdcdxiRTWNy", b"".to_vec()),
        ];
        for (i, (hash, pw)) in tests.iter().enumerate() {
            assert_eq!(bf(pw, hash).as_deref(), Some(*hash), "test {i}");
        }
    }
}
