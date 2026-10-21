//! ext/standard/strnatcmp.c — "natural order" string comparison, plus the
//! `strnatcmp()` / `strnatcasecmp()` functions (string.c) that expose it.
//!
//! The algorithm is shared with `natsort()`, `natcasesort()` and the
//! `SORT_NATURAL` flag of the array sort functions.

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// The byte at `i`, or the C string terminator (`'\0'`) past the end. php-src
/// reads one byte past the end of a `zend_string` (always NUL-terminated).
#[inline]
fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// C `isspace()` in the "C" locale.
#[inline]
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

#[inline]
fn is_digit(c: u8) -> bool {
    c.is_ascii_digit()
}

/// `compare_right`: compares two right-aligned numbers; the longest run of
/// digits wins, otherwise the first differing digit decides.
fn compare_right(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> i32 {
    let mut bias = 0;
    loop {
        let a_end = *ap >= a.len() || !is_digit(a[*ap]);
        let b_end = *bp >= b.len() || !is_digit(b[*bp]);
        if a_end && b_end {
            return bias;
        } else if a_end {
            return -1;
        } else if b_end {
            return 1;
        } else if a[*ap] < b[*bp] {
            if bias == 0 {
                bias = -1;
            }
        } else if a[*ap] > b[*bp] && bias == 0 {
            bias = 1;
        }
        *ap += 1;
        *bp += 1;
    }
}

/// `compare_left`: compares two left-aligned numbers; the first digit that
/// differs wins.
fn compare_left(a: &[u8], ap: &mut usize, b: &[u8], bp: &mut usize) -> i32 {
    loop {
        let a_end = *ap >= a.len() || !is_digit(a[*ap]);
        let b_end = *bp >= b.len() || !is_digit(b[*bp]);
        if a_end && b_end {
            return 0;
        } else if a_end {
            return -1;
        } else if b_end {
            return 1;
        } else if a[*ap] < b[*bp] {
            return -1;
        } else if a[*ap] > b[*bp] {
            return 1;
        }
        *ap += 1;
        *bp += 1;
    }
}

/// `strnatcmp_ex`: returns -1, 0 or 1.
pub fn strnatcmp_ex(a: &[u8], b: &[u8], is_case_insensitive: bool) -> i32 {
    let (a_len, b_len) = (a.len(), b.len());
    if a_len == 0 || b_len == 0 {
        return match a_len.cmp(&b_len) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
        };
    }

    let (mut ap, mut bp) = (0usize, 0usize);
    let mut ca = at(a, ap);
    let mut cb = at(b, bp);

    // Skip over leading zeros.
    while ca == b'0' && ap + 1 < a_len && is_digit(a[ap + 1]) {
        ap += 1;
        ca = at(a, ap);
    }
    while cb == b'0' && bp + 1 < b_len && is_digit(b[bp + 1]) {
        bp += 1;
        cb = at(b, bp);
    }

    loop {
        // Skip consecutive whitespace.
        while is_space(ca) {
            ap += 1;
            ca = at(a, ap);
        }
        while is_space(cb) {
            bp += 1;
            cb = at(b, bp);
        }

        // Process a run of digits.
        if is_digit(ca) && is_digit(cb) {
            let fractional = ca == b'0' || cb == b'0';
            let result = if fractional {
                compare_left(a, &mut ap, b, &mut bp)
            } else {
                compare_right(a, &mut ap, b, &mut bp)
            };
            if result != 0 {
                return result;
            } else if ap == a_len && bp == b_len {
                // End of the strings. Let the caller sort them out.
                return 0;
            } else if ap == a_len {
                return -1;
            } else if bp == b_len {
                return 1;
            } else {
                // Keep on comparing from the current point.
                ca = at(a, ap);
                cb = at(b, bp);
            }
        }

        if is_case_insensitive {
            ca = ca.to_ascii_uppercase();
            cb = cb.to_ascii_uppercase();
        }

        if ca < cb {
            return -1;
        } else if ca > cb {
            return 1;
        }

        ap += 1;
        bp += 1;
        if ap >= a_len && bp >= b_len {
            // The strings compare the same.
            return 0;
        } else if ap >= a_len {
            return -1;
        } else if bp >= b_len {
            return 1;
        }

        ca = at(a, ap);
        cb = at(b, bp);
    }
}

fn strnatcmp(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(
        strnatcmp_ex(args.str(0), args.str(1), false) as i64
    ))
}

fn strnatcasecmp(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(
        strnatcmp_ex(args.str(0), args.str(1), true) as i64
    ))
}

pub static STRNATCMP: NativeFunction = NativeFunction {
    sig: "strnatcmp(string $string1, string $string2): int",
    handler: strnatcmp,
};

pub static STRNATCASECMP: NativeFunction = NativeFunction {
    sig: "strnatcasecmp(string $string1, string $string2): int",
    handler: strnatcasecmp,
};

pub fn register(r: &mut Registry) {
    r.function(&STRNATCMP);
    r.function(&STRNATCASECMP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn nat(a: &str, b: &str) -> i32 {
        strnatcmp_ex(a.as_bytes(), b.as_bytes(), false)
    }

    fn natcase(a: &str, b: &str) -> i32 {
        strnatcmp_ex(a.as_bytes(), b.as_bytes(), true)
    }

    #[test]
    fn empty_strings() {
        assert_eq!(nat("", ""), 0);
        assert_eq!(nat("", "a"), -1);
        assert_eq!(nat("a", ""), 1);
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(nat("img12.png", "img10.png"), 1);
        assert_eq!(nat("img2.png", "img10.png"), -1);
        assert_eq!(nat("img10.png", "img10.png"), 0);
        assert_eq!(nat("a9", "a10"), -1);
        assert_eq!(nat("a100", "a99"), 1);
        // Leading zeros of the whole string are skipped.
        assert_eq!(nat("1", "01"), 0);
        assert_eq!(nat("001", "01"), 0);
    }

    #[test]
    fn basic_phpt() {
        // strnatcmp_basic.phpt
        let (a1, b1, c1, d1) = ("abc1", "abc10", "abc15", "abc2");
        let (b2, c2, d2) = ("ABC10", "ABC15", "ABC2");
        assert_eq!(nat(a1, b1), -1);
        assert_eq!(nat(a1, c1), -1);
        assert_eq!(nat(a1, d1), -1);
        assert_eq!(nat(b1, c1), -1);
        assert_eq!(nat(d1, c1), -1);
        assert_eq!(nat(a1, b2), 1);
        assert_eq!(nat(a1, c2), 1);
        assert_eq!(nat(a1, d2), 1);
        assert_eq!(nat(b1, c2), 1);
        assert_eq!(nat(d1, c2), 1);
        assert_eq!(nat(b1, b1), 0);
        assert_eq!(nat(b1, b2), 1);
        assert_eq!(nat(b1, a1), 1);
        assert_eq!(nat(c1, d1), 1);
    }

    #[test]
    fn leftalign_phpt() {
        // strnatcmp_leftalign.phpt
        assert_eq!(nat(" 00", " 0"), 1);
        assert_eq!(nat(" 0", " 00"), -1);
        // Zeros inside the string make the run left-aligned ("fractional").
        assert_eq!(nat("a01", "a1"), -1);
        assert_eq!(nat("a1", "a01"), 1);
        // ... whereas right-aligned runs compare by length first.
        assert_eq!(nat("0.5", "0.15"), -1);
    }

    #[test]
    fn whitespace_is_skipped() {
        assert_eq!(nat("a  b", "a b"), 0);
        assert_eq!(nat("a   1", "a 2"), -1);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(nat("A", "a"), -1);
        assert_eq!(natcase("A", "a"), 0);
        assert_eq!(natcase("IMG10", "img2"), 1);
        assert_eq!(natcase("abc", "ABD"), -1);
    }

    #[test]
    fn prefixes() {
        assert_eq!(nat("abc", "abcd"), -1);
        assert_eq!(nat("abcd", "abc"), 1);
        assert_eq!(nat("1", "1a"), -1);
        assert_eq!(nat("1a", "1"), 1);
    }

    #[test]
    fn functions_are_registered() {
        let mut t = TestVm::new();
        let r = t
            .call("strnatcmp", vec!["img12.png".into(), "img10.png".into()])
            .unwrap();
        assert_eq!(r.as_int(), Some(1));
        // "1" vs "01" after the first character: a leading zero makes the
        // comparison left-aligned and "1" > "0".
        let r = t
            .call("strnatcasecmp", vec!["A1".into(), "a01".into()])
            .unwrap();
        assert_eq!(r.as_int(), Some(1));
        let r = t
            .call("strnatcasecmp", vec!["abc".into(), "ABC".into()])
            .unwrap();
        assert_eq!(r.as_int(), Some(0));
    }
}
