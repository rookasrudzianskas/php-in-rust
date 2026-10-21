//! ext/standard/string.c — `hebrev()`.

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `isheb()`: a Hebrew letter in ISO-8859-8.
fn is_heb(c: u8) -> bool {
    (224..=250).contains(&c)
}

/// `_isblank()`.
fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// `_isnewline()`.
fn is_newline(c: u8) -> bool {
    c == b'\n' || c == b'\r'
}

/// Mirrors brackets and slashes inside a Hebrew (right-to-left) block.
fn mirror(c: u8) -> u8 {
    match c {
        b'(' => b')',
        b')' => b'(',
        b'[' => b']',
        b']' => b'[',
        b'{' => b'}',
        b'}' => b'{',
        b'<' => b'>',
        b'>' => b'<',
        b'\\' => b'/',
        b'/' => b'\\',
        c => c,
    }
}

/// Converts logical Hebrew text to visual text (`PHP_FUNCTION(hebrev)`),
/// breaking lines after at most `max_chars` characters (0 = no limit).
pub fn hebrev_bytes(s: &[u8], max_chars: i64) -> Vec<u8> {
    let len = s.len();
    if len == 0 {
        return Vec::new();
    }
    // php-src reads one byte past the end (the NUL terminator).
    let at = |i: usize| s.get(i).copied().unwrap_or(0);

    // Pass 1: emit the blocks in reverse order into `heb` (filled from the
    // end like php-src's `target` pointer), reversing the characters of Hebrew
    // blocks and mirroring their brackets. php-src's `tmp` pointer always
    // equals `block_end`; after the first block both sit one byte before
    // `block_start`, and the scan loops look at `block_end + 1`.
    let mut heb = vec![0u8; len];
    let mut target = len;
    let mut block_start = 0usize;
    let mut block_end = 0usize;
    let mut hebrew = is_heb(s[0]);
    loop {
        if hebrew {
            while is_heb_block_char(at(block_end + 1)) && block_end < len - 1 {
                block_end += 1;
            }
            for &c in &s[block_start..block_end + 1] {
                target -= 1;
                heb[target] = mirror(c);
            }
        } else {
            while !is_heb(at(block_end + 1)) && at(block_end + 1) != b'\n' && block_end < len - 1 {
                block_end += 1;
            }
            // Trailing blanks/punctuation belong to the following Hebrew block.
            while is_trimmable(s[block_end]) && block_end > block_start {
                block_end -= 1;
            }
            for &c in s[block_start..block_end + 1].iter().rev() {
                target -= 1;
                heb[target] = c;
            }
        }
        hebrew = !hebrew;
        block_start = block_end + 1;
        if block_end >= len - 1 {
            break;
        }
    }
    debug_assert_eq!(target, 0);
    break_lines(&mut heb, max_chars)
}

/// Characters that extend a Hebrew block.
fn is_heb_block_char(c: u8) -> bool {
    is_heb(c) || is_blank(c) || c.is_ascii_punctuation() || c == b'\n'
}

/// Characters trimmed from the end of a non-Hebrew block.
fn is_trimmable(c: u8) -> bool {
    (is_blank(c) || c.is_ascii_punctuation()) && c != b'/' && c != b'-'
}

/// Pass 2 of `hebrev()`: splits the visual text into lines of at most
/// `max_chars` characters (trying not to break words), emitting the lines
/// from the last one to the first.
fn break_lines(heb: &mut [u8], max_chars: i64) -> Vec<u8> {
    let len = heb.len();
    let mut out = Vec::with_capacity(len);
    let mut begin = len - 1;
    let mut end = len - 1;
    loop {
        let mut char_count: i64 = 0;
        while (max_chars == 0 || (max_chars > 0 && char_count < max_chars)) && begin > 0 {
            char_count += 1;
            begin -= 1;
            if is_newline(heb[begin]) {
                while begin > 0 && is_newline(heb[begin - 1]) {
                    begin -= 1;
                    char_count += 1;
                }
                break;
            }
        }
        if max_chars >= 0 && char_count == max_chars {
            // Try to avoid breaking words.
            let mut new_char_count = char_count;
            let mut new_begin = begin;
            while new_char_count > 0 {
                if is_blank(heb[new_begin]) || is_newline(heb[new_begin]) {
                    break;
                }
                new_begin += 1;
                new_char_count -= 1;
            }
            if new_char_count > 0 {
                begin = new_begin;
            }
        }
        let orig_begin = begin;
        if is_blank(heb[begin]) {
            heb[begin] = b'\n';
        }
        // Skip leading newlines, copy the content, then the newlines.
        while begin <= end && is_newline(heb[begin]) {
            begin += 1;
        }
        if begin <= end {
            out.extend_from_slice(&heb[begin..=end]);
        }
        out.extend(
            heb[orig_begin..=end]
                .iter()
                .take_while(|&&c| is_newline(c))
                .copied(),
        );
        if orig_begin == 0 {
            break;
        }
        begin = orig_begin - 1;
        end = begin;
    }
    out
}

fn hebrev(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(hebrev_bytes(
        args.str(0),
        args.int(1),
    ))))
}

pub static HEBREV: NativeFunction = NativeFunction {
    sig: "hebrev(string $string, int $max_chars_per_line = 0): string",
    handler: hebrev,
};

pub fn register(r: &mut Registry) {
    r.function(&HEBREV);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    const TEXT: &str = "The hebrev function converts logical Hebrew text to visual text.\nThe function tries to avoid breaking words.\n";

    fn call(t: &mut TestVm, args: Vec<Value>) -> Vec<u8> {
        match t.call("hebrev", args).unwrap() {
            Value::String(s) => s.to_vec(),
            v => panic!("unexpected {v:?}"),
        }
    }

    #[test]
    fn basic() {
        // hebrev_basic.phpt
        let mut t = TestVm::new();
        let got = call(&mut t, vec![Value::str(TEXT)]);
        assert_eq!(
            String::from_utf8(got).unwrap(),
            ".The hebrev function converts logical Hebrew text to visual text\n.The function tries to avoid breaking words\n"
        );
        let got = call(&mut t, vec![Value::str(TEXT), Value::Int(15)]);
        assert_eq!(
            String::from_utf8(got).unwrap(),
            "to visual text\nHebrew text\nlogical\nconverts\nhebrev function\n.The\nbreaking words\ntries to avoid\n.The function\n"
        );
        assert_eq!(call(&mut t, vec![Value::str("")]), b"");
    }

    #[test]
    fn hebrew_blocks() {
        assert_eq!(hebrev_bytes(b"\xe0\xe1\xe2 abc", 0), b"abc \xe2\xe1\xe0");
        assert_eq!(hebrev_bytes(b"\xe0(\xe1)", 0), b"(\xe1)\xe0");
        assert_eq!(hebrev_bytes(b"a.", 0), b".a");
        assert_eq!(hebrev_bytes(b"x", 0), b"x");
        assert_eq!(hebrev_bytes(b"\xe0", 0), b"\xe0");
    }

    #[test]
    fn negative_width_keeps_length() {
        let out = hebrev_bytes(TEXT.as_bytes(), -1);
        assert_eq!(out.len(), TEXT.len());
    }
}
