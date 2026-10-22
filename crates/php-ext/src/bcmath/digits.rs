//! Schoolbook big-integer helpers on decimal digit strings.
//!
//! libbcmath multiplies and divides digit arrays by packing eight digits into
//! a `BC_VECTOR` (`recmul.c`, `div.c`). Only the *values* of these routines
//! are observable, so this port packs nine digits per `u32` limb (base 10^9)
//! and uses Knuth's algorithm D for the division. Digit strings are most
//! significant digit first with one byte (0..=9) per digit, like `n_value`.

const BASE: u64 = 1_000_000_000;
const LIMB_DIGITS: usize = 9;

/// Packs MSD-first decimal digits into little-endian base-10^9 limbs
/// (without leading zero limbs).
fn digits_to_limbs(digits: &[u8]) -> Vec<u32> {
    let mut limbs = Vec::with_capacity(digits.len() / LIMB_DIGITS + 1);
    let mut end = digits.len();
    while end > 0 {
        let start = end.saturating_sub(LIMB_DIGITS);
        let limb = digits[start..end]
            .iter()
            .fold(0u32, |acc, &d| acc * 10 + u32::from(d));
        limbs.push(limb);
        end = start;
    }
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
    limbs
}

/// Unpacks limbs into exactly `width` MSD-first digits (zero padded; digits
/// that do not fit are dropped).
fn limbs_to_digits(limbs: &[u32], width: usize) -> Vec<u8> {
    let mut out = vec![0u8; width];
    let mut pos = width;
    'outer: for &limb in limbs {
        let mut v = limb;
        for _ in 0..LIMB_DIGITS {
            if pos == 0 {
                break 'outer;
            }
            pos -= 1;
            out[pos] = (v % 10) as u8;
            v /= 10;
        }
    }
    out
}

/// Product of two limb vectors.
fn mul_limbs(a: &[u32], b: &[u32]) -> Vec<u32> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut res = vec![0u32; a.len() + b.len()];
    for (i, &x) in a.iter().enumerate() {
        if x == 0 {
            continue;
        }
        let x = u64::from(x);
        let mut carry = 0u64;
        for (j, &y) in b.iter().enumerate() {
            let cur = u64::from(res[i + j]) + x * u64::from(y) + carry;
            res[i + j] = (cur % BASE) as u32;
            carry = cur / BASE;
        }
        let mut k = i + b.len();
        while carry > 0 {
            let cur = u64::from(res[k]) + carry;
            res[k] = (cur % BASE) as u32;
            carry = cur / BASE;
            k += 1;
        }
    }
    while res.last() == Some(&0) {
        res.pop();
    }
    res
}

/// Multiplies two digit strings; the result has exactly `a.len() + b.len()`
/// digits (leading zeros included), like the `prodlen` buffer in
/// `bc_multiply`.
pub fn mul_digits(a: &[u8], b: &[u8]) -> Vec<u8> {
    let width = a.len() + b.len();
    let prod = mul_limbs(&digits_to_limbs(a), &digits_to_limbs(b));
    limbs_to_digits(&prod, width)
}

/// `floor(u / v)` for limb vectors; `v` must be non-empty with a non-zero top
/// limb. Knuth, TAOCP vol. 2, 4.3.1, algorithm D.
fn div_limbs(u: &[u32], v: &[u32]) -> Vec<u32> {
    let n = v.len();
    if u.len() < n {
        return Vec::new();
    }
    if n == 1 {
        let d = u64::from(v[0]);
        let mut q = vec![0u32; u.len()];
        let mut rem = 0u64;
        for i in (0..u.len()).rev() {
            let cur = rem * BASE + u64::from(u[i]);
            q[i] = (cur / d) as u32;
            rem = cur % d;
        }
        while q.last() == Some(&0) {
            q.pop();
        }
        return q;
    }

    // D1: normalise so that the top divisor limb is at least BASE / 2.
    let d = BASE / (u64::from(v[n - 1]) + 1);
    let mut vn = vec![0u32; n];
    let mut carry = 0u64;
    for i in 0..n {
        let cur = u64::from(v[i]) * d + carry;
        vn[i] = (cur % BASE) as u32;
        carry = cur / BASE;
    }
    debug_assert_eq!(carry, 0);
    let m = u.len() - n;
    let mut un = vec![0u32; u.len() + 1];
    carry = 0;
    for i in 0..u.len() {
        let cur = u64::from(u[i]) * d + carry;
        un[i] = (cur % BASE) as u32;
        carry = cur / BASE;
    }
    un[u.len()] = carry as u32;

    let mut q = vec![0u32; m + 1];
    let vtop = u64::from(vn[n - 1]);
    let vsec = u64::from(vn[n - 2]);
    for j in (0..=m).rev() {
        // D3: estimate the quotient limb.
        let num = u64::from(un[j + n]) * BASE + u64::from(un[j + n - 1]);
        let mut qhat = num / vtop;
        let mut rhat = num % vtop;
        while qhat >= BASE || qhat * vsec > rhat * BASE + u64::from(un[j + n - 2]) {
            qhat -= 1;
            rhat += vtop;
            if rhat >= BASE {
                break;
            }
        }
        // D4: multiply and subtract.
        let mut borrow = 0i64;
        let mut carry = 0u64;
        for i in 0..n {
            let p = qhat * u64::from(vn[i]) + carry;
            carry = p / BASE;
            let mut cur = i64::from(un[i + j]) - (p % BASE) as i64 + borrow;
            if cur < 0 {
                cur += BASE as i64;
                borrow = -1;
            } else {
                borrow = 0;
            }
            un[i + j] = cur as u32;
        }
        let mut cur = i64::from(un[j + n]) - carry as i64 + borrow;
        if cur < 0 {
            cur += BASE as i64;
            borrow = -1;
        } else {
            borrow = 0;
        }
        un[j + n] = cur as u32;
        // D5/D6: the estimate was one too large; add the divisor back.
        if borrow < 0 {
            qhat -= 1;
            let mut c = 0u64;
            for i in 0..n {
                let s = u64::from(un[i + j]) + u64::from(vn[i]) + c;
                un[i + j] = (s % BASE) as u32;
                c = s / BASE;
            }
            un[j + n] = ((u64::from(un[j + n]) + c) % BASE) as u32;
        }
        q[j] = qhat as u32;
    }
    while q.last() == Some(&0) {
        q.pop();
    }
    q
}

/// Integer quotient of two digit strings with exactly
/// `a.len() - b.len() + 1` digits. `b` must be non-zero and `a.len() >= b.len()`.
pub fn div_digits(a: &[u8], b: &[u8]) -> Vec<u8> {
    debug_assert!(a.len() >= b.len());
    let width = a.len() - b.len() + 1;
    let q = div_limbs(&digits_to_limbs(a), &digits_to_limbs(b));
    limbs_to_digits(&q, width)
}

/// `base^exponent` for a digit string (`bc_standard_raise`), by squaring.
/// The result has no leading zeros (a single `0` for a zero base).
pub fn pow_digits(base: &[u8], mut exponent: u64) -> Vec<u8> {
    let base_limbs = digits_to_limbs(base);
    let mut result = vec![1u32];
    let mut square = base_limbs;
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = mul_limbs(&result, &square);
        }
        exponent >>= 1;
        if exponent > 0 {
            square = mul_limbs(&square, &square);
        }
    }
    // Width: enough digits for every limb.
    let mut digits = limbs_to_digits(&result, result.len() * LIMB_DIGITS);
    let first = digits.iter().position(|&d| d != 0).unwrap_or(digits.len());
    digits.drain(..first);
    if digits.is_empty() {
        digits.push(0);
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digits(s: &str) -> Vec<u8> {
        s.bytes().map(|b| b - b'0').collect()
    }

    fn text(d: &[u8]) -> String {
        d.iter().map(|&x| (x + b'0') as char).collect()
    }

    #[test]
    fn multiplication() {
        assert_eq!(text(&mul_digits(&digits("99"), &digits("99"))), "9801");
        assert_eq!(text(&mul_digits(&digits("5"), &digits("7"))), "35");
        // Leading zeros of the product are kept up to the full width.
        assert_eq!(text(&mul_digits(&digits("002"), &digits("003"))), "000006");
        let a = "9".repeat(40);
        let expect = format!("{}8{}1", "9".repeat(39), "0".repeat(39));
        assert_eq!(text(&mul_digits(&digits(&a), &digits(&a))), expect);
    }

    #[test]
    fn division() {
        assert_eq!(text(&div_digits(&digits("100"), &digits("7"))), "014");
        assert_eq!(text(&div_digits(&digits("10"), &digits("10"))), "1");
        // Multi-limb divisor (Knuth D).
        let a = format!("123456789012345678901234567890{}", "0".repeat(10));
        let q = text(&div_digits(&digits(&a), &digits("1234567890123456789")));
        assert_eq!(q, "1000000000000000000009");
        let a = format!("1{}", "0".repeat(40));
        let q = text(&div_digits(&digits(&a), &digits("12345678901234567890")));
        assert_eq!(q, "0810000007290000066347");
    }

    #[test]
    fn division_exactness_random() {
        // A deterministic LCG feeds a round-trip check: (a / b) * b + r == a.
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        for _ in 0..200 {
            let la = 1 + next() % 60;
            let lb = 1 + next() % la.min(40);
            let mut a: Vec<u8> = (0..la).map(|_| (next() % 10) as u8).collect();
            let mut b: Vec<u8> = (0..lb).map(|_| (next() % 10) as u8).collect();
            a[0] = 1 + (a[0] % 9);
            b[0] = 1 + (b[0] % 9);
            let q = div_digits(&a, &b);
            let qb = mul_digits(&q, &b);
            // q * b <= a and a - q * b < b, compared as digit strings.
            let qb = {
                let first = qb.iter().position(|&d| d != 0).unwrap_or(qb.len());
                qb[first..].to_vec()
            };
            let cmp = |x: &[u8], y: &[u8]| (x.len(), x.to_vec()).cmp(&(y.len(), y.to_vec()));
            assert_ne!(
                cmp(&qb, &a),
                std::cmp::Ordering::Greater,
                "{} / {}",
                text(&a),
                text(&b)
            );
            // (q + 1) * b > a
            let one = {
                let mut v = vec![0u8; q.len()];
                *v.last_mut().unwrap() = 1;
                v
            };
            let mut q1 = vec![0u8; q.len() + 1];
            let mut carry = 0;
            for i in (0..q.len()).rev() {
                let s = q[i] + one[i] + carry;
                q1[i + 1] = s % 10;
                carry = s / 10;
            }
            q1[0] = carry;
            let q1b = mul_digits(&q1, &b);
            let first = q1b.iter().position(|&d| d != 0).unwrap_or(q1b.len());
            assert_eq!(
                cmp(&q1b[first..], &a),
                std::cmp::Ordering::Greater,
                "{} / {}",
                text(&a),
                text(&b)
            );
        }
    }

    #[test]
    fn powers() {
        assert_eq!(text(&pow_digits(&digits("2"), 10)), "1024");
        assert_eq!(text(&pow_digits(&digits("15"), 2)), "225");
        assert_eq!(text(&pow_digits(&digits("10"), 3)), "1000");
        assert_eq!(text(&pow_digits(&digits("7"), 1)), "7");
        assert_eq!(
            text(&pow_digits(&digits("2"), 100)),
            "1267650600228229401496703205376"
        );
    }
}
