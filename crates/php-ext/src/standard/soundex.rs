//! ext/standard/soundex.c

use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// `soundex_table`: the digit of each letter A-Z, 0 for the vowels and H, W, Y.
const SOUNDEX_TABLE: [u8; 26] = [
    0, b'1', b'2', b'3', 0, b'1', b'2', 0, 0, b'2', b'2', b'4', b'5', b'5', 0, b'1', b'2', b'6',
    b'2', b'3', 0, b'1', 0, b'2', 0, b'2',
];

/// The soundex key of `s` (Knuth, TAOCP vol. 3): the first letter followed by
/// up to three digits, padded with `0`.
pub fn php_soundex(s: &[u8]) -> [u8; 4] {
    let mut soundex = [b'0'; 4];
    let mut small = 0usize;
    // `last` is `(size_t)-1` initially: it never equals a table value.
    let mut last: Option<u8> = None;

    for &c in s {
        if small >= 4 {
            break;
        }
        // convert chars to upper case and strip non-letter chars
        let code = c.to_ascii_uppercase();
        if !code.is_ascii_uppercase() {
            continue;
        }
        if small == 0 {
            // remember first valid char
            soundex[small] = code;
            small += 1;
            last = Some(SOUNDEX_TABLE[(code - b'A') as usize]);
        } else {
            // ignore sequences of consonants with same soundex code in
            // trail, and vowels unless they separate consonant letters
            let code = SOUNDEX_TABLE[(code - b'A') as usize];
            if Some(code) != last {
                if code != 0 {
                    soundex[small] = code;
                    small += 1;
                }
                last = Some(code);
            }
        }
    }
    soundex
}

/// Calculate the soundex key of a string.
fn soundex(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::String(PhpStr::from_vec(
        php_soundex(args.str(0)).to_vec(),
    )))
}

pub static SOUNDEX: NativeFunction = NativeFunction {
    sig: "soundex(string $string): string",
    handler: soundex,
};

pub fn register(r: &mut Registry) {
    r.function(&SOUNDEX);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestVm;

    fn sx(t: &mut TestVm, arg: Value) -> String {
        match t.call("soundex", vec![arg]).unwrap() {
            Value::String(s) => String::from_utf8(s.to_vec()).unwrap(),
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn soundex_phpt() {
        // soundex.phpt
        let mut t = TestVm::new();
        assert_eq!(sx(&mut t, Value::str("")), "0000");
        // soundex(-1): the int is coerced to the string "-1"
        assert_eq!(sx(&mut t, Value::Int(-1)), "0000");
        let cases = [
            ("From", "F650"),
            ("that", "T300"),
            ("time", "T500"),
            ("on", "O500"),
            ("Sam", "S500"),
            ("thought", "T230"),
            ("he", "H000"),
            ("sensed", "S523"),
            ("a", "A000"),
            ("change", "C520"),
            ("in", "I500"),
            ("Gollum", "G450"),
            ("again.", "A250"),
            (
                "He was more fawning and would-be friendly; but Sam surprised some strange looks in his eyes at times, especially towards Frodo.",
                "H256",
            ),
        ];
        for (s, e) in cases {
            assert_eq!(sx(&mut t, Value::str(s)), e, "{s}");
        }
    }

    #[test]
    fn soundex_basic() {
        // soundex_basic.phpt
        let mut t = TestVm::new();
        let cases = [
            ("Euler", "E460"),
            ("Gauss", "G200"),
            ("Hilbert", "H416"),
            ("Knuth", "K530"),
            ("Lloyd", "L300"),
            ("Lukasiewicz", "L222"),
            ("Ellery", "E460"),
            ("Ghosh", "G200"),
            ("Heilbronn", "H416"),
            ("Kant", "K530"),
            ("Ladd", "L300"),
            ("Lissajous", "L222"),
        ];
        for (s, e) in cases {
            assert_eq!(sx(&mut t, Value::str(s)), e, "{s}");
        }
        assert_ne!(
            sx(&mut t, Value::str("Lukasiewicz")),
            sx(&mut t, Value::str("Ghosh"))
        );
        assert_ne!(
            sx(&mut t, Value::str("Hilbert")),
            sx(&mut t, Value::str("Ladd"))
        );
    }

    #[test]
    fn soundex_non_letters_and_case() {
        let mut t = TestVm::new();
        assert_eq!(sx(&mut t, Value::str("123")), "0000");
        assert_eq!(sx(&mut t, Value::str("1r-o b")), "R100");
        assert_eq!(sx(&mut t, Value::bytes(b"\xe9Tt")), "T000");
        assert_eq!(sx(&mut t, Value::str("tymczak")), "T522");
        assert_eq!(sx(&mut t, Value::str("pfister")), "P236");
    }
}
