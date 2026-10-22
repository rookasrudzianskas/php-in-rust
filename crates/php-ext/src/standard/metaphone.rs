//! ext/standard/metaphone.c
//!
//! Based on CPAN's "Text-Metaphone-1.96" by Michael G Schwern.

use php_runtime::class::fn_flags;
use php_runtime::{Args, NativeFunction, PResult, PhpStr, Registry, Value, Vm};

/// Special encoding of "SH".
const SH: u8 = b'X';
/// Special encoding of "TH".
const TH: u8 = b'0';

/// `_codes`: character classes of A-Z for `encode`.
const CODES: [u8; 26] = [
    1, 16, 4, 16, 9, 2, 4, 16, 9, 2, 0, 2, 2, 2, 1, 4, 0, 2, 4, 4, 1, 0, 0, 0, 8,
    0,
    //a  b c  d e f g  h i j k l m n o p q r s t u v w x y z
];

/// `encode`: the class bits of an upper-case letter, 0 for anything else.
fn encode(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        CODES[(c - b'A') as usize]
    } else {
        0
    }
}

/// AEIOU
fn isvowel(c: u8) -> bool {
    encode(c) & 1 != 0
}

/// CGPST: these form diphthongs when preceding H.
fn affecth(c: u8) -> bool {
    encode(c) & 4 != 0
}

/// EIY: these make C and G soft.
fn makesoft(c: u8) -> bool {
    encode(c) & 8 != 0
}

/// BDH: these prevent GH from becoming F.
fn noghtof(c: u8) -> bool {
    encode(c) & 16 != 0
}

/// `Isbreak`
fn isbreak(c: u8) -> bool {
    !c.is_ascii_alphabetic()
}

/// Reads the word like the NUL-terminated C string it is in php-src: bytes
/// past the end (and any embedded NUL) read as `'\0'`.
struct Word<'a> {
    word: &'a [u8],
    /// `w_idx`: the point in the phonization we're at.
    idx: usize,
}

impl Word<'_> {
    fn raw(&self, i: usize) -> u8 {
        self.word.get(i).copied().unwrap_or(0)
    }

    /// `Read_Raw_Curr_Letter`
    fn raw_curr(&self) -> u8 {
        self.raw(self.idx)
    }

    /// `Read_Next_Letter`
    fn next(&self) -> u8 {
        self.raw(self.idx + 1).to_ascii_uppercase()
    }

    /// `Look_Back_Letter(n)`
    fn look_back(&self, n: usize) -> u8 {
        if self.idx >= n {
            self.raw(self.idx - n).to_ascii_uppercase()
        } else {
            0
        }
    }

    /// `Read_Prev_Letter`
    fn prev(&self) -> u8 {
        self.look_back(1)
    }

    /// `Read_After_Next_Letter`: looks two letters down without walking off
    /// the string.
    fn after_next(&self) -> u8 {
        if self.raw(self.idx + 1) != 0 {
            self.raw(self.idx + 2).to_ascii_uppercase()
        } else {
            0
        }
    }

    /// `Look_Ahead_Letter(n)`: safely look ahead an arbitrary number of letters.
    fn look_ahead(&self, n: usize) -> u8 {
        for k in 0..n {
            if self.raw(self.idx + k) == 0 {
                return 0;
            }
        }
        self.raw(self.idx + n).to_ascii_uppercase()
    }
}

/// `metaphone`: breaks an English phrase down into its phonemes. A
/// `max_phonemes` of 0 means no limit. `traditional` selects the original
/// algorithm (php-src always passes true).
pub fn metaphone(word: &[u8], max_phonemes: usize, traditional: bool) -> Vec<u8> {
    let mut w = Word { word, idx: 0 };
    let mut out: Vec<u8> = Vec::with_capacity(if max_phonemes == 0 {
        word.len()
    } else {
        max_phonemes
    });

    // The first phoneme has to be processed specially. Find our first letter.
    loop {
        let c = w.raw_curr();
        if c.is_ascii_alphabetic() {
            break;
        }
        // On the off chance we were given nothing but crap...
        if c == 0 {
            return out;
        }
        w.idx += 1;
    }

    let curr_letter = w.raw_curr().to_ascii_uppercase();
    match curr_letter {
        // AE becomes E
        b'A' => {
            if w.next() == b'E' {
                out.push(b'E');
                w.idx += 2;
            } else {
                // Remember, preserve vowels at the beginning
                out.push(b'A');
                w.idx += 1;
            }
        }
        // [GKP]N becomes N
        b'G' | b'K' | b'P' => {
            if w.next() == b'N' {
                out.push(b'N');
                w.idx += 2;
            }
        }
        // WH becomes W, WR becomes R, W if followed by a vowel
        b'W' => {
            let next_letter = w.next();
            if next_letter == b'R' {
                out.push(b'R');
                w.idx += 2;
            } else if next_letter == b'H' || isvowel(next_letter) {
                out.push(b'W');
                w.idx += 2;
            }
            // else ignore
        }
        // X becomes S
        b'X' => {
            out.push(b'S');
            w.idx += 1;
        }
        // Vowels are kept (A was done already)
        b'E' | b'I' | b'O' | b'U' => {
            out.push(curr_letter);
            w.idx += 1;
        }
        _ => {}
    }

    // On to the metaphoning
    loop {
        let raw = w.raw_curr();
        if raw == 0 || (max_phonemes != 0 && out.len() >= max_phonemes) {
            break;
        }
        // How many letters to skip because an earlier encoding handled
        // multiple letters.
        let mut skip_letter = 0usize;

        // Ignore non-alphas
        if raw.is_ascii_alphabetic() {
            let curr_letter = raw.to_ascii_uppercase();
            // Note: we can't cache curr_letter from the previous loop
            // because of the skip_letter variable.
            let prev_letter = w.prev();

            // Drop duplicates, except CC
            if curr_letter != prev_letter || curr_letter == b'C' {
                match curr_letter {
                    // B -> B unless in MB
                    b'B' => {
                        if prev_letter != b'M' {
                            out.push(b'B');
                        }
                    }
                    // 'sh' if -CIA- or -CH, but not SCH, except SCHW.
                    // (SCHW is handled in S)
                    // S if -CI-, -CE- or -CY-
                    // dropped if -SCI-, SCE-, -SCY- (handed in S)
                    // else K
                    b'C' => {
                        let next_letter = w.next();
                        if makesoft(next_letter) {
                            // C[IEY]
                            if next_letter == b'I' && w.after_next() == b'A' {
                                // CIA
                                out.push(SH);
                            } else if prev_letter == b'S' {
                                // SC[IEY]: dropped
                            } else {
                                out.push(b'S');
                            }
                        } else if next_letter == b'H' {
                            if !traditional && (prev_letter == b'S' || w.after_next() == b'R') {
                                // Christ, School
                                out.push(b'K');
                            } else {
                                out.push(SH);
                            }
                            skip_letter += 1;
                        } else {
                            out.push(b'K');
                        }
                    }
                    // J if in -DGE-, -DGI- or -DGY-
                    // else T
                    b'D' => {
                        if w.next() == b'G' && makesoft(w.after_next()) {
                            out.push(b'J');
                            skip_letter += 1;
                        } else {
                            out.push(b'T');
                        }
                    }
                    // F if in -GH and not B--GH, D--GH, -H--GH, -H---GH
                    // else dropped if -GNED, -GN,
                    // else dropped if -DGE-, -DGI- or -DGY- (handled in D)
                    // else J if in -GE-, -GI, -GY and not GG
                    // else K
                    b'G' => {
                        let next_letter = w.next();
                        if next_letter == b'H' {
                            if !(noghtof(w.look_back(3)) || w.look_back(4) == b'H') {
                                out.push(b'F');
                                skip_letter += 1;
                            }
                            // else silent
                        } else if next_letter == b'N' {
                            let after_next_letter = w.after_next();
                            if isbreak(after_next_letter)
                                || (after_next_letter == b'E' && w.look_ahead(3) == b'D')
                            {
                                // dropped
                            } else {
                                out.push(b'K');
                            }
                        } else if makesoft(next_letter) && prev_letter != b'G' {
                            out.push(b'J');
                        } else {
                            out.push(b'K');
                        }
                    }
                    // H if before a vowel and not after C,G,P,S,T
                    b'H' => {
                        if isvowel(w.next()) && !affecth(prev_letter) {
                            out.push(b'H');
                        }
                    }
                    // dropped if after C
                    // else K
                    b'K' => {
                        if prev_letter != b'C' {
                            out.push(b'K');
                        }
                    }
                    // F if before H
                    // else P
                    b'P' => {
                        if w.next() == b'H' {
                            out.push(b'F');
                        } else {
                            out.push(b'P');
                        }
                    }
                    // K
                    b'Q' => out.push(b'K'),
                    // 'sh' in -SH-, -SIO- or -SIA- or -SCHW-
                    // else S
                    b'S' => {
                        let next_letter = w.next();
                        if next_letter == b'I' && matches!(w.after_next(), b'O' | b'A') {
                            out.push(SH);
                        } else if next_letter == b'H' {
                            out.push(SH);
                            skip_letter += 1;
                        } else if !traditional
                            && next_letter == b'C'
                            && w.look_ahead(2) == b'H'
                            && w.look_ahead(3) == b'W'
                        {
                            out.push(SH);
                            skip_letter += 2;
                        } else {
                            out.push(b'S');
                        }
                    }
                    // 'sh' in -TIA- or -TIO-
                    // else 'th' before H
                    // else T
                    b'T' => {
                        let next_letter = w.next();
                        if next_letter == b'I' && matches!(w.after_next(), b'O' | b'A') {
                            out.push(SH);
                        } else if next_letter == b'H' {
                            out.push(TH);
                            skip_letter += 1;
                        } else if !(next_letter == b'C' && w.after_next() == b'H') {
                            out.push(b'T');
                        }
                    }
                    // F
                    b'V' => out.push(b'F'),
                    // W before a vowel, else dropped
                    b'W' => {
                        if isvowel(w.next()) {
                            out.push(b'W');
                        }
                    }
                    // KS
                    b'X' => {
                        out.push(b'K');
                        out.push(b'S');
                    }
                    // Y if followed by a vowel
                    b'Y' => {
                        if isvowel(w.next()) {
                            out.push(b'Y');
                        }
                    }
                    // S
                    b'Z' => out.push(b'S'),
                    // No transformation
                    b'F' | b'J' | b'L' | b'M' | b'N' | b'R' => out.push(curr_letter),
                    _ => {}
                }
                w.idx += skip_letter;
            }
        }
        w.idx += 1;
    }

    out
}

/// Break english phrases down into their phonemes.
fn php_metaphone(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let phones = args.int(1);
    if phones < 0 {
        return Err(vm.argument_value_error(2, "must be greater than or equal to 0"));
    }
    let out = metaphone(args.str(0), phones as usize, true);
    Ok(Value::String(PhpStr::from_vec(out)))
}

pub static METAPHONE: NativeFunction = NativeFunction {
    sig: "metaphone(string $string, int $max_phonemes = 0): string",
    handler: php_metaphone,
};

pub fn register(r: &mut Registry) {
    // #[\Deprecated(since: '8.6', message: 'use a userland phonetic matching library instead')]
    r.function_with_flags(&METAPHONE, fn_flags::DEPRECATED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn mp(t: &mut TestVm, arg: Value, max: Option<i64>) -> String {
        let mut args = vec![arg];
        if let Some(m) = max {
            args.push(Value::Int(m));
        }
        match t.call("metaphone", args).unwrap() {
            Value::String(s) => String::from_utf8(s.to_vec()).unwrap(),
            v => panic!("{v:?}"),
        }
    }

    fn m(s: &str) -> String {
        String::from_utf8(metaphone(s.as_bytes(), 0, true)).unwrap()
    }

    #[test]
    fn metaphone_phpt() {
        // metaphone.phpt
        let mut t = TestVm::new();
        assert_eq!(mp(&mut t, Value::str(""), None), "");
        assert_eq!(mp(&mut t, Value::Int(-1), None), "");
        let r = t.call(
            "metaphone",
            vec![Value::str("valid phrase"), Value::Int(-1)],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "metaphone(): Argument #2 ($max_phonemes) must be greater than or equal to 0"
                    .to_string()
            )
        );
        assert_eq!(mp(&mut t, Value::str("valid phrase"), Some(0)), "FLTFRS");
        assert_eq!(
            mp(&mut t, Value::str("valid phrase"), Some(10000)),
            "FLTFRS"
        );

        let cases = [
            (
                "They fell forward, grovelling heedlessly on the cold earth.",
                "0FLFRWRTKRFLNKHTLSLN0KLTR0",
            ),
            (
                "But the shadow of horror wheeled and returned, passing lower now, right above them, sweeping the fen-reek with its ghastly wings.",
                "BT0XTFHRRHLTNTRTRNTPSNKLWRNRFTBF0MSWPNK0FNRKW0TSFSTLWNKS",
            ),
            (
                "And then it was gone, flying back to Mordor with the speed of the wrath of Sauron; and behind it the wind roared away, leaving the Dead Marshes bare and bleak.",
                "ANT0NTWSKNFLYNKBKTMRTRW00SPTF0R0FSRNNTBHNTT0WNTRRTWLFNK0TTMRXSBRNTBLK",
            ),
            (
                "The naked waste, as far as the eye could pierce, even to the distant menace of the mountains, was dappled with the fitful moonlight.",
                "0NKTWSTSFRS0YKLTPRSFNT0TSTNTMNSF0MNTNSWSTPLTW00FTFLMNLFT",
            ),
        ];
        for (s, e) in cases {
            assert_eq!(mp(&mut t, Value::str(s), None), e, "{s}");
        }
    }

    #[test]
    fn metaphone_deprecated_flag() {
        // metaphone_deprecation.phpt: the runtime emits the message for
        // functions registered with fn_flags::DEPRECATED.
        let t = TestVm::new();
        let f = t.vm.find_function("metaphone").expect("registered");
        assert_ne!(f.flags & fn_flags::DEPRECATED, 0);
        assert_eq!(m("programming"), "PRKRMNK");
    }

    #[test]
    fn metaphone_bug_tests() {
        // bug44242.phpt
        assert_eq!(m("CMXFXZ"), "KMKSFKSS");
        assert_eq!(m("CMXFXV"), "KMKSFKSF");
        assert_eq!(m("CMXFXZXZ"), "KMKSFKSSKSS");
        // bug47443.phpt
        assert_eq!(m("scratch"), "SKRX");
        assert_eq!(m("scrath"), "SKR0");
        assert_eq!(m("scratc"), "SKRTK");
        // bug48709.phpt
        for (word, expected) in [
            ("kn", "N"),
            ("gn", "N"),
            ("pn", "N"),
            ("ae", "E"),
            ("wr", "R"),
            ("x", "S"),
            ("wh", "W"),
            ("wa", "W"),
        ] {
            assert_eq!(m(word), expected, "{word}");
        }
    }

    #[test]
    fn initial_letter_exceptions() {
        assert_eq!(m("Aebersold"), "EBRSLT");
        assert_eq!(m("Anna"), "AN");
        assert_eq!(m("Gnome"), "NM");
        assert_eq!(m("Knight"), "NFT");
        assert_eq!(m("Pneumonia"), "NMN");
        assert_eq!(m("Wrack"), "RK");
        assert_eq!(m("Whale"), "WL");
        assert_eq!(m("Wasp"), "WSP");
        assert_eq!(m("Wxyz"), "KSS");
        assert_eq!(m("Xavier"), "SFR");
        assert_eq!(m("Eric"), "ERK");
        assert_eq!(m("Island"), "ISLNT");
        // leading junk is skipped
        assert_eq!(m("  \"gnat"), "NT");
    }

    #[test]
    fn letter_rules() {
        assert_eq!(m("thumb"), "0M");
        assert_eq!(m("Thomas"), "0MS");
        assert_eq!(m("Schmidt"), "SXMTT");
        assert_eq!(m("science"), "SNS");
        assert_eq!(m("facial"), "FXL");
        assert_eq!(m("church"), "XRX");
        assert_eq!(m("judge"), "JJ");
        assert_eq!(m("laugh"), "LF");
        assert_eq!(m("night"), "NFT");
        assert_eq!(m("Phillip"), "FLP");
        assert_eq!(m("nation"), "NXN");
        assert_eq!(m("match"), "MX");
        assert_eq!(m("yellow"), "YL");
        assert_eq!(m("zoo"), "S");
        assert_eq!(m("queen"), "KN");
        assert_eq!(m("exact"), "EKSKT");
        assert_eq!(m("sing"), "SNK");
        assert_eq!(m("signed"), "SNT");
        assert_eq!(m("ghost"), "FST");
        assert_eq!(m("accident"), "AKSTNT");
        assert_eq!(m("hello  world"), "HLWRLT");
        assert_eq!(m("123"), "");
    }

    #[test]
    fn max_phonemes() {
        let mp = |s: &str, n| String::from_utf8(metaphone(s.as_bytes(), n, true)).unwrap();
        assert_eq!(mp("They fell forward", 3), "0FL");
        assert_eq!(mp("They fell forward", 1), "0");
        assert_eq!(mp("They fell forward", 100), "0FLFRWRT");
        // the initial letter is emitted even when the limit is smaller, and a
        // two-phoneme letter such as X can overshoot the limit
        assert_eq!(mp("Xylophone", 1), "S");
        assert_eq!(mp("ax", 2), "AKS");
    }

    #[test]
    fn embedded_nul_ends_the_word() {
        // The C code works on a NUL-terminated string.
        assert_eq!(metaphone(b"abc\0def", 0, true), b"ABK");
        assert_eq!(metaphone(b"\0abc", 0, true), b"");
    }
}
