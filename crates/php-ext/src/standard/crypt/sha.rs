//! SHA-256 (`$5$`) and SHA-512 (`$6$`) based `crypt()` — Ulrich Drepper's
//! algorithm as in ext/standard/crypt_sha256.c and crypt_sha512.c (the two
//! files only differ in the digest, the prefix and the output permutation).

use super::digest::{Digest, algorithm};

/// Maximum salt string length.
const SALT_LEN_MAX: usize = 16;
/// Default number of rounds if not explicitly specified.
const ROUNDS_DEFAULT: u64 = 5000;
/// Minimum number of rounds.
const ROUNDS_MIN: u64 = 1000;
/// Maximum number of rounds.
const ROUNDS_MAX: u64 = 999_999_999;
/// Prefix for the optional rounds specification.
const ROUNDS_PREFIX: &[u8] = b"rounds=";

/// Table with characters for base64 transformation.
const B64T: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Sha256,
    Sha512,
}

impl Variant {
    fn algorithm_name(self) -> &'static str {
        match self {
            Variant::Sha256 => "sha256",
            Variant::Sha512 => "sha512",
        }
    }

    fn digest_len(self) -> usize {
        match self {
            Variant::Sha256 => 32,
            Variant::Sha512 => 64,
        }
    }

    fn salt_prefix(self) -> &'static [u8] {
        match self {
            Variant::Sha256 => b"$5$",
            Variant::Sha512 => b"$6$",
        }
    }
}

/// C `strtoul(s, &endp, 10)`: the value and the index `endp` points at
/// (`0` when no conversion was performed).
fn strtoul10(s: &[u8]) -> (u64, usize) {
    let mut i = 0;
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut val: u64 = 0;
    let mut overflow = false;
    while i < s.len() && s[i].is_ascii_digit() {
        match val
            .checked_mul(10)
            .and_then(|v| v.checked_add((s[i] - b'0') as u64))
        {
            Some(v) => val = v,
            None => overflow = true,
        }
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    if overflow {
        return (u64::MAX, i);
    }
    (if neg { val.wrapping_neg() } else { val }, i)
}

/// `b64_from_24bit()`.
fn b64_from_24bit(out: &mut Vec<u8>, b2: u8, b1: u8, b0: u8, n: usize) {
    let mut w = ((b2 as u32) << 16) | ((b1 as u32) << 8) | b0 as u32;
    for _ in 0..n {
        out.push(B64T[(w & 0x3f) as usize]);
        w >>= 6;
    }
}

/// `php_sha256_crypt_r()` / `php_sha512_crypt_r()`: `key` and `salt` are the
/// C strings' contents. `None` is php-src's NULL return (rounds out of range).
pub fn sha_crypt(key: &[u8], salt: &[u8], variant: Variant) -> Option<Vec<u8>> {
    let algo = algorithm(variant.algorithm_name());
    let dlen = variant.digest_len();
    let prefix = variant.salt_prefix();

    let mut rounds = ROUNDS_DEFAULT;
    let mut rounds_custom = false;

    // Find the beginning of the salt string. The prefix should normally
    // always be present. Just in case it is not.
    let mut salt = salt;
    if salt.starts_with(prefix) {
        salt = &salt[prefix.len()..];
    }

    if salt.starts_with(ROUNDS_PREFIX) {
        let num = &salt[ROUNDS_PREFIX.len()..];
        let (srounds, end) = strtoul10(num);
        if num.get(end) == Some(&b'$') {
            salt = &num[end + 1..];
            if !(ROUNDS_MIN..=ROUNDS_MAX).contains(&srounds) {
                return None;
            }
            rounds = srounds;
            rounds_custom = true;
        }
    }

    // The salt ends at the first `$` and has at most 16 characters.
    let salt_len = salt
        .iter()
        .take_while(|&&c| c != b'$')
        .count()
        .min(SALT_LEN_MAX);
    let salt = &salt[..salt_len];
    let key_len = key.len();

    // Prepare for the real work.
    let mut ctx = Digest::new(algo);
    ctx.update(key);
    ctx.update(salt);

    // Compute the alternate sum with input KEY, SALT and KEY; the final
    // result will be added to the first context.
    let mut alt_ctx = Digest::new(algo);
    alt_ctx.update(key);
    alt_ctx.update(salt);
    alt_ctx.update(key);
    let alt_result = alt_ctx.finish();

    // Add for any character in the key one byte of the alternate sum.
    let mut cnt = key_len;
    while cnt > dlen {
        ctx.update(&alt_result);
        cnt -= dlen;
    }
    ctx.update(&alt_result[..cnt]);

    // Take the binary representation of the length of the key and for every 1
    // add the alternate sum, for every 0 the key.
    let mut cnt = key_len;
    while cnt > 0 {
        if cnt & 1 != 0 {
            ctx.update(&alt_result);
        } else {
            ctx.update(key);
        }
        cnt >>= 1;
    }

    // Create the intermediate result.
    let mut alt_result = ctx.finish();

    // Start computation of the P byte sequence: for every character in the
    // password add the entire password.
    let mut alt_ctx = Digest::new(algo);
    for _ in 0..key_len {
        alt_ctx.update(key);
    }
    let temp_result = alt_ctx.finish();
    let p_bytes: Vec<u8> = (0..key_len).map(|i| temp_result[i % dlen]).collect();

    // Start computation of the S byte sequence.
    let mut alt_ctx = Digest::new(algo);
    for _ in 0..16 + alt_result[0] as usize {
        alt_ctx.update(salt);
    }
    let temp_result = alt_ctx.finish();
    let s_bytes: Vec<u8> = (0..salt_len).map(|i| temp_result[i % dlen]).collect();

    // Repeatedly run the collected hash value through the digest to burn CPU
    // cycles.
    for cnt in 0..rounds {
        let mut ctx = Digest::new(algo);

        // Add key or last result.
        if cnt & 1 != 0 {
            ctx.update(&p_bytes);
        } else {
            ctx.update(&alt_result);
        }
        // Add salt for numbers not divisible by 3.
        if cnt % 3 != 0 {
            ctx.update(&s_bytes);
        }
        // Add key for numbers not divisible by 7.
        if cnt % 7 != 0 {
            ctx.update(&p_bytes);
        }
        // Add key or last result.
        if cnt & 1 != 0 {
            ctx.update(&alt_result);
        } else {
            ctx.update(&p_bytes);
        }
        alt_result = ctx.finish();
    }

    // Construct the result string: prefix, optional rounds, salt, '$', hash.
    let mut out = Vec::with_capacity(prefix.len() + 20 + salt_len + 1 + 86);
    out.extend_from_slice(prefix);
    if rounds_custom {
        out.extend_from_slice(ROUNDS_PREFIX);
        out.extend_from_slice(rounds.to_string().as_bytes());
        out.push(b'$');
    }
    out.extend_from_slice(salt);
    out.push(b'$');

    let r = &alt_result;
    match variant {
        Variant::Sha256 => {
            const ORDER: [(usize, usize, usize); 10] = [
                (0, 10, 20),
                (21, 1, 11),
                (12, 22, 2),
                (3, 13, 23),
                (24, 4, 14),
                (15, 25, 5),
                (6, 16, 26),
                (27, 7, 17),
                (18, 28, 8),
                (9, 19, 29),
            ];
            for (a, b, c) in ORDER {
                b64_from_24bit(&mut out, r[a], r[b], r[c], 4);
            }
            b64_from_24bit(&mut out, 0, r[31], r[30], 3);
        }
        Variant::Sha512 => {
            const ORDER: [(usize, usize, usize); 21] = [
                (0, 21, 42),
                (22, 43, 1),
                (44, 2, 23),
                (3, 24, 45),
                (25, 46, 4),
                (47, 5, 26),
                (6, 27, 48),
                (28, 49, 7),
                (50, 8, 29),
                (9, 30, 51),
                (31, 52, 10),
                (53, 11, 32),
                (12, 33, 54),
                (34, 55, 13),
                (56, 14, 35),
                (15, 36, 57),
                (37, 58, 16),
                (59, 17, 38),
                (18, 39, 60),
                (40, 61, 19),
                (62, 20, 41),
            ];
            for (a, b, c) in ORDER {
                b64_from_24bit(&mut out, r[a], r[b], r[c], 4);
            }
            b64_from_24bit(&mut out, 0, 0, r[63], 2);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s256(pw: &str, salt: &str) -> Option<String> {
        sha_crypt(pw.as_bytes(), salt.as_bytes(), Variant::Sha256)
            .map(|v| String::from_utf8(v).unwrap())
    }

    fn s512(pw: &str, salt: &str) -> Option<String> {
        sha_crypt(pw.as_bytes(), salt.as_bytes(), Variant::Sha512)
            .map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn strtoul_semantics() {
        assert_eq!(strtoul10(b"1000$x"), (1000, 4));
        assert_eq!(strtoul10(b"  +12$"), (12, 5));
        assert_eq!(strtoul10(b"$abc"), (0, 0));
        assert_eq!(strtoul10(b"x1"), (0, 0));
        assert_eq!(strtoul10(b"-1$"), (u64::MAX, 2));
        assert_eq!(strtoul10(b"99999999999999999999$"), (u64::MAX, 20));
    }

    /// The SHA-256 vectors of crypt_sha256.c / crypt_sha256.phpt.
    #[test]
    fn sha256_drepper_vectors() {
        let tests: [(&str, &str, &str); 7] = [
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
                "$5$rounds=5000$toolongsaltstring",
                "This is just a test",
                "$5$rounds=5000$toolongsaltstrin$Un/5jzAHMgOGZ5.mWJpuVolil07guHPvOW8mGRcvxa5",
            ),
            (
                "$5$rounds=1400$anotherlongsaltstring",
                "a very much longer text to encrypt.  This one even stretches over morethan one line.",
                "$5$rounds=1400$anotherlongsalts$Rx.j8H.h8HjEDGomFU8bDkXm3XIUnzyxf12oP84Bnq1",
            ),
            (
                "$5$rounds=77777$short",
                "we have a short salt string but not a short password",
                "$5$rounds=77777$short$JiO1O3ZpDAxGJeaDIuqCoEFysAe1mZNJRs3pw0KQRd/",
            ),
            (
                "$5$rounds=123456$asaltof16chars..",
                "a short string",
                "$5$rounds=123456$asaltof16chars..$gP3VQ/6X7UUEW3HkBn2w1/Ptq2jxPyzV/cZKmF/wJvD",
            ),
            (
                "$5$rounds=1000$roundstoolow",
                "the minimum number is still observed",
                "$5$rounds=1000$roundstoolow$yfvwcWrQ8l/K0DAWyuPMDNHpIVlTQebY9l/gL972bIC",
            ),
        ];
        for (salt, pw, expected) in tests {
            assert_eq!(s256(pw, salt).as_deref(), Some(expected), "{salt}");
        }
    }

    /// The SHA-512 vectors of crypt_sha512.c / crypt_sha512.phpt.
    #[test]
    fn sha512_drepper_vectors() {
        let tests: [(&str, &str, &str); 7] = [
            (
                "$6$saltstring",
                "Hello world!",
                "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1",
            ),
            (
                "$6$rounds=10000$saltstringsaltstring",
                "Hello world!",
                "$6$rounds=10000$saltstringsaltst$OW1/O6BYHV6BcXZu8QVeXbDWra3Oeqh0sbHbbMCVNSnCM/UrjmM0Dp8vOuZeHBy/YTBmSK6H9qs/y3RnOaw5v.",
            ),
            (
                "$6$rounds=5000$toolongsaltstring",
                "This is just a test",
                "$6$rounds=5000$toolongsaltstrin$lQ8jolhgVRVhY4b5pZKaysCLi0QBxGoNeKQzQ3glMhwllF7oGDZxUhx1yxdYcz/e1JSbq3y6JMxxl8audkUEm0",
            ),
            (
                "$6$rounds=1400$anotherlongsaltstring",
                "a very much longer text to encrypt.  This one even stretches over morethan one line.",
                "$6$rounds=1400$anotherlongsalts$POfYwTEok97VWcjxIiSOjiykti.o/pQs.wPvMxQ6Fm7I6IoYN3CmLs66x9t0oSwbtEW7o7UmJEiDwGqd8p4ur1",
            ),
            (
                "$6$rounds=77777$short",
                "we have a short salt string but not a short password",
                "$6$rounds=77777$short$WuQyW2YR.hBNpjjRhpYD/ifIw05xdfeEyQoMxIXbkvr0gge1a1x3yRULJ5CCaUeOxFmtlcGZelFl5CxtgfiAc0",
            ),
            (
                "$6$rounds=123456$asaltof16chars..",
                "a short string",
                "$6$rounds=123456$asaltof16chars..$BtCwjqMJGx5hrJhZywWvt0RLE8uZ4oPwcelCjmw2kSYu.Ec6ycULevoBK25fs2xXgMNrCzIMVcgEJAstJeonj1",
            ),
            (
                "$6$$bar$",
                "foo",
                "$6$$QMXjqd7rHQZPQ1yHsXkQqC1FBzDiVfTHXL.LaeDAeVV.IzMaV9VU4MQ8kPuZa2SOP1A0RPm772EaFYjpEJtdu.",
            ),
        ];
        for (salt, pw, expected) in tests {
            assert_eq!(s512(pw, salt).as_deref(), Some(expected), "{salt}");
        }
    }

    #[test]
    fn rounds_out_of_range_fail() {
        assert_eq!(s256("x", "$5$rounds=1000000000$roundstoohigh"), None);
        assert_eq!(s256("x", "$5$rounds=10$roundstoolow"), None);
        assert_eq!(s512("x", "$6$rounds=1000000000$roundstoohigh"), None);
        assert_eq!(s512("x", "$6$rounds=10$roundstoolow"), None);
        assert_eq!(s512("x", "$6$rounds=-1$salt"), None);
        assert_eq!(s512("x", "$6$rounds=$salt"), None);
    }

    #[test]
    fn malformed_rounds_are_part_of_the_salt() {
        // No '$' after the number: "rounds=..." is the salt text itself.
        let out = s256("x", "$5$rounds=2000").unwrap();
        assert!(out.starts_with("$5$rounds=2000$"));
        assert!(!out.starts_with("$5$rounds=2000$rounds"));
        let out = s256("x", "$5$rounds=12abc$z").unwrap();
        assert!(out.starts_with("$5$rounds=12abc$"));
    }

    /// crypt_variation1.phpt: salts are cut at 16 characters.
    #[test]
    fn long_salt_is_truncated() {
        let salt = format!("$5${}", "A".repeat(124));
        assert_eq!(
            s256("A", &salt).as_deref(),
            Some("$5$AAAAAAAAAAAAAAAA$frotiiztWZiwcncxnY5tWG9Ida2WOZEximjLXCleQu6")
        );
    }
}
