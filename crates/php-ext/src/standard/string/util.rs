//! Byte-string helpers shared by the string submodules (`zend_memnstr`,
//! `zend_memnrstr`, `zend_memnistr`, `php_charmask`, ...).

#![allow(clippy::collapsible_if)]

use php_runtime::{PResult, Vm};

/// `zend_memnstr`: first occurrence of `needle` in `hay`; an empty needle
/// matches at 0.
pub fn memnstr(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memnstr_from(hay, needle, 0)
}

/// [`memnstr`] searching `hay[from..]`; the result is an absolute index.
pub fn memnstr_from(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if from > hay.len() {
        return None;
    }
    if n == 0 {
        return Some(from);
    }
    let hay_rest = &hay[from..];
    if n > hay_rest.len() {
        return None;
    }
    let first = needle[0];
    if n == 1 {
        return hay_rest.iter().position(|&b| b == first).map(|p| from + p);
    }
    let last = needle[n - 1];
    let end = hay_rest.len() - n;
    let mut i = 0;
    while i <= end {
        i += hay_rest[i..=end].iter().position(|&b| b == first)?;
        if hay_rest[i + n - 1] == last && hay_rest[i + 1..i + n - 1] == needle[1..n - 1] {
            return Some(from + i);
        }
        i += 1;
    }
    None
}

/// `zend_memnistr`: ASCII case-insensitive search. `lc_needle` must already be
/// lowercased, `lc_hay` is the lowercased haystack.
pub fn memnistr_lc(lc_hay: &[u8], lc_needle: &[u8], from: usize) -> Option<usize> {
    memnstr_from(lc_hay, lc_needle, from)
}

/// `zend_memnistr(hay, needle)` on raw (not yet lowercased) inputs.
pub fn memnistr(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return if from <= hay.len() { Some(from) } else { None };
    }
    memnistr_lc(
        &hay.to_ascii_lowercase(),
        &needle.to_ascii_lowercase(),
        from,
    )
}

/// `zend_memnrstr`: last occurrence of `needle` in `hay`; an empty needle
/// matches at `hay.len()`.
pub fn memnrstr(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let n = needle.len();
    if n == 0 {
        return Some(hay.len());
    }
    if n > hay.len() {
        return None;
    }
    let first = needle[0];
    if n == 1 {
        return hay.iter().rposition(|&b| b == first);
    }
    let last = needle[n - 1];
    let mut i = hay.len() - n;
    loop {
        if hay[i] == first && hay[i + n - 1] == last && hay[i + 1..i + n - 1] == needle[1..n - 1] {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// `php_charmask`: builds the 256-entry byte mask for a character list that
/// may contain `a..z` ranges, emitting php-src's warnings for invalid ranges.
pub fn php_charmask(vm: &mut Vm, input: &[u8]) -> PResult<[bool; 256]> {
    let mut mask = [false; 256];
    let len = input.len();
    let mut i = 0;
    while i < len {
        let c = input[i];
        if i + 3 < len && input[i + 1] == b'.' && input[i + 2] == b'.' && input[i + 3] >= c {
            for b in c..=input[i + 3] {
                mask[b as usize] = true;
            }
            i += 3;
        } else if i + 1 < len && input[i] == b'.' && input[i + 1] == b'.' {
            // Error, try to be as helpful as possible (a range ending/starting
            // with '.' won't be captured here).
            if i == 0 {
                vm.warning("Invalid '..'-range, no character to the left of '..'")?;
            } else if i + 2 >= len {
                vm.warning("Invalid '..'-range, no character to the right of '..'")?;
            } else if input[i - 1] > input[i + 2] {
                vm.warning("Invalid '..'-range, '..'-range needs to be incrementing")?;
            } else {
                vm.warning("Invalid '..'-range")?;
            }
        } else {
            mask[c as usize] = true;
        }
        i += 1;
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_search() {
        assert_eq!(memnstr(b"hello world", b"o w"), Some(4));
        assert_eq!(memnstr(b"hello", b""), Some(0));
        assert_eq!(memnstr(b"hello", b"hello!"), None);
        assert_eq!(memnstr(b"aaa", b"aa"), Some(0));
        assert_eq!(memnstr_from(b"abcabc", b"bc", 2), Some(4));
        assert_eq!(memnstr_from(b"abc", b"", 3), Some(3));
        assert_eq!(memnstr_from(b"abc", b"c", 4), None);
        assert_eq!(memnstr(b"a", b"a"), Some(0));
    }

    #[test]
    fn reverse_search() {
        assert_eq!(memnrstr(b"abcabc", b"bc"), Some(4));
        assert_eq!(memnrstr(b"abc", b""), Some(3));
        assert_eq!(memnrstr(b"abc", b"abcd"), None);
        assert_eq!(memnrstr(b"abc", b"a"), Some(0));
        assert_eq!(memnrstr(b"aaa", b"aa"), Some(1));
    }

    #[test]
    fn case_insensitive_search() {
        assert_eq!(memnistr(b"Hello", b"LL", 0), Some(2));
        assert_eq!(memnistr(b"Hello", b"", 2), Some(2));
    }
}
