//! Unicode character properties, compatible with PCRE2 10.44.
//!
//! The data is Unicode 15.0.0 (the version PCRE2 10.43/10.44 ship) and every
//! numeric value uses the enum numbering of PCRE2's `pcre2_ucp.h` and
//! `pcre2_internal.h`, so code ported from PCRE2 can use these values
//! directly. The tables live in the generated `tables.rs`; regenerate them
//! with `python3 crates/php-regex/tools/gen_unicode.py`.
//!
//! Code points above U+10FFFF get the values of PCRE2's `dummy_ucd_record`:
//! category Cn, script Unknown, grapheme break Other, bidi class L, no
//! boolean properties and no other case.

// The engine uses only part of this API, and the module may be declared
// `pub(crate)`, which makes unused items and re-exports warn.
#![allow(dead_code, unused_imports)]

mod tables;

pub use self::tables::bprops;
pub use self::tables::scripts;

/// The highest Unicode code point.
pub const MAX_CODE_POINT: u32 = 0x10_FFFF;

// ---------------------------------------------------------------------------
// General types (ucp_C ... ucp_Z)
// ---------------------------------------------------------------------------

pub const GT_C: u8 = 0;
pub const GT_L: u8 = 1;
pub const GT_M: u8 = 2;
pub const GT_N: u8 = 3;
pub const GT_P: u8 = 4;
pub const GT_S: u8 = 5;
pub const GT_Z: u8 = 6;

// ---------------------------------------------------------------------------
// Particular general categories (ucp_Cc ... ucp_Zs)
// ---------------------------------------------------------------------------

/// Control
pub const UCP_CC: u8 = 0;
/// Format
pub const UCP_CF: u8 = 1;
/// Unassigned
pub const UCP_CN: u8 = 2;
/// Private use
pub const UCP_CO: u8 = 3;
/// Surrogate
pub const UCP_CS: u8 = 4;
/// Lower case letter
pub const UCP_LL: u8 = 5;
/// Modifier letter
pub const UCP_LM: u8 = 6;
/// Other letter
pub const UCP_LO: u8 = 7;
/// Title case letter
pub const UCP_LT: u8 = 8;
/// Upper case letter
pub const UCP_LU: u8 = 9;
/// Spacing mark
pub const UCP_MC: u8 = 10;
/// Enclosing mark
pub const UCP_ME: u8 = 11;
/// Non-spacing mark
pub const UCP_MN: u8 = 12;
/// Decimal number
pub const UCP_ND: u8 = 13;
/// Letter number
pub const UCP_NL: u8 = 14;
/// Other number
pub const UCP_NO: u8 = 15;
/// Connector punctuation
pub const UCP_PC: u8 = 16;
/// Dash punctuation
pub const UCP_PD: u8 = 17;
/// Close punctuation
pub const UCP_PE: u8 = 18;
/// Final punctuation
pub const UCP_PF: u8 = 19;
/// Initial punctuation
pub const UCP_PI: u8 = 20;
/// Other punctuation
pub const UCP_PO: u8 = 21;
/// Open punctuation
pub const UCP_PS: u8 = 22;
/// Currency symbol
pub const UCP_SC: u8 = 23;
/// Modifier symbol
pub const UCP_SK: u8 = 24;
/// Mathematical symbol
pub const UCP_SM: u8 = 25;
/// Other symbol
pub const UCP_SO: u8 = 26;
/// Line separator
pub const UCP_ZL: u8 = 27;
/// Paragraph separator
pub const UCP_ZP: u8 = 28;
/// Space separator
pub const UCP_ZS: u8 = 29;

// ---------------------------------------------------------------------------
// Grapheme cluster break properties (ucp_gbCR ... ucp_gbExtended_Pictographic)
// ---------------------------------------------------------------------------

pub const GB_CR: u8 = 0;
pub const GB_LF: u8 = 1;
pub const GB_CONTROL: u8 = 2;
pub const GB_EXTEND: u8 = 3;
pub const GB_PREPEND: u8 = 4;
pub const GB_SPACING_MARK: u8 = 5;
/// Hangul syllable type L
pub const GB_L: u8 = 6;
/// Hangul syllable type V
pub const GB_V: u8 = 7;
/// Hangul syllable type T
pub const GB_T: u8 = 8;
/// Hangul syllable type LV
pub const GB_LV: u8 = 9;
/// Hangul syllable type LVT
pub const GB_LVT: u8 = 10;
pub const GB_REGIONAL_INDICATOR: u8 = 11;
pub const GB_OTHER: u8 = 12;
pub const GB_ZWJ: u8 = 13;
pub const GB_EXTENDED_PICTOGRAPHIC: u8 = 14;

// ---------------------------------------------------------------------------
// Bidi classes (ucp_bidiAL ... ucp_bidiWS)
// ---------------------------------------------------------------------------

/// Arabic_Letter
pub const BIDI_AL: u8 = 0;
/// Arabic_Number
pub const BIDI_AN: u8 = 1;
/// Paragraph_Separator
pub const BIDI_B: u8 = 2;
/// Boundary_Neutral
pub const BIDI_BN: u8 = 3;
/// Common_Separator
pub const BIDI_CS: u8 = 4;
/// European_Number
pub const BIDI_EN: u8 = 5;
/// European_Separator
pub const BIDI_ES: u8 = 6;
/// European_Terminator
pub const BIDI_ET: u8 = 7;
/// First_Strong_Isolate
pub const BIDI_FSI: u8 = 8;
/// Left_To_Right
pub const BIDI_L: u8 = 9;
/// Left_To_Right_Embedding
pub const BIDI_LRE: u8 = 10;
/// Left_To_Right_Isolate
pub const BIDI_LRI: u8 = 11;
/// Left_To_Right_Override
pub const BIDI_LRO: u8 = 12;
/// Nonspacing_Mark
pub const BIDI_NSM: u8 = 13;
/// Other_Neutral
pub const BIDI_ON: u8 = 14;
/// Pop_Directional_Format
pub const BIDI_PDF: u8 = 15;
/// Pop_Directional_Isolate
pub const BIDI_PDI: u8 = 16;
/// Right_To_Left
pub const BIDI_R: u8 = 17;
/// Right_To_Left_Embedding
pub const BIDI_RLE: u8 = 18;
/// Right_To_Left_Isolate
pub const BIDI_RLI: u8 = 19;
/// Right_To_Left_Override
pub const BIDI_RLO: u8 = 20;
/// Segment_Separator
pub const BIDI_S: u8 = 21;
/// White_Space
pub const BIDI_WS: u8 = 22;

// ---------------------------------------------------------------------------
// Property types (PT_* in pcre2_internal.h)
// ---------------------------------------------------------------------------

/// Any property - matches all chars
pub const PT_ANY: u8 = 0;
/// L& - the union of Lu, Ll, Lt
pub const PT_LAMP: u8 = 1;
/// Specified general characteristic (e.g. L)
pub const PT_GC: u8 = 2;
/// Specified particular characteristic (e.g. Lu)
pub const PT_PC: u8 = 3;
/// Script only (e.g. Han)
pub const PT_SC: u8 = 4;
/// Script extensions (includes SC)
pub const PT_SCX: u8 = 5;
/// Alphanumeric - the union of L and N
pub const PT_ALNUM: u8 = 6;
/// Perl space - general category Z plus 9,10,12,13
pub const PT_SPACE: u8 = 7;
/// POSIX space - Z plus 9,10,11,12,13
pub const PT_PXSPACE: u8 = 8;
/// Word - L, N, Mn, or Pc
pub const PT_WORD: u8 = 9;
/// Pseudo-property: match character list
pub const PT_CLIST: u8 = 10;
/// Universal Character nameable character
pub const PT_UCNC: u8 = 11;
/// Specified bidi class
pub const PT_BIDICL: u8 = 12;
/// Boolean property
pub const PT_BOOL: u8 = 13;
/// `[:graph:]` - characters that mark the paper
pub const PT_PXGRAPH: u8 = 14;
/// `[:print:]` - `[:graph:]` plus non-control spaces
pub const PT_PXPRINT: u8 = 15;
/// `[:punct:]` - punctuation characters
pub const PT_PXPUNCT: u8 = 16;
/// `[:xdigit:]` - hex digits
pub const PT_PXXDIGIT: u8 = 17;

/// Number of scripts (`ucp_Script_Count`).
pub const SCRIPT_COUNT: usize = tables::SCRIPT_COUNT;
/// Number of boolean properties (`ucp_Bprop_Count`).
pub const BPROP_COUNT: usize = tables::BPROP_COUNT;

// ---------------------------------------------------------------------------
// General category
// ---------------------------------------------------------------------------

/// `PRIV(ucp_gentype)`: particular category -> general type.
static GENTYPE: [u8; 30] = [
    GT_C, GT_C, GT_C, GT_C, GT_C, // Cc, Cf, Cn, Co, Cs
    GT_L, GT_L, GT_L, GT_L, GT_L, // Ll, Lm, Lo, Lt, Lu
    GT_M, GT_M, GT_M, // Mc, Me, Mn
    GT_N, GT_N, GT_N, // Nd, Nl, No
    GT_P, GT_P, GT_P, GT_P, GT_P, GT_P, GT_P, // Pc, Pd, Pe, Pf, Pi, Po, Ps
    GT_S, GT_S, GT_S, GT_S, // Sc, Sk, Sm, So
    GT_Z, GT_Z, GT_Z, // Zl, Zp, Zs
];

/// General type (`GT_*`) of a particular category (`UCP_*`), like PCRE2's
/// `PRIV(ucp_gentype)`. Out-of-range input is treated as C.
#[inline]
pub fn gentype(chartype: u8) -> u8 {
    GENTYPE.get(usize::from(chartype)).copied().unwrap_or(GT_C)
}

/// Particular general category (`UCP_*`) of `c` (PCRE2's `UCD_CHARTYPE`).
#[inline]
pub fn chartype(c: u32) -> u8 {
    if c > MAX_CODE_POINT {
        return UCP_CN;
    }
    let block = usize::from(tables::CHARTYPE_STAGE1[(c >> 7) as usize]);
    tables::CHARTYPE_STAGE2[block * 128 + (c & 127) as usize]
}

/// General type (`GT_*`) of `c` (PCRE2's `UCD_CATEGORY`).
#[inline]
pub fn category(c: u32) -> u8 {
    gentype(chartype(c))
}

// ---------------------------------------------------------------------------
// Range-table helpers
// ---------------------------------------------------------------------------

/// Index of the range containing `c` in a table of range starts that begins
/// at 0.
#[inline]
fn range_index(starts: &[u32], c: u32) -> usize {
    starts.partition_point(|&s| s <= c) - 1
}

/// Membership test for an inversion list.
#[inline]
fn in_inversion_list(list: &[u32], c: u32) -> bool {
    list.partition_point(|&s| s <= c) % 2 == 1
}

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

/// Script of `c` (`scripts::*`, PCRE2's `UCD_SCRIPT`); `scripts::UNKNOWN`
/// for unassigned, private-use and surrogate code points.
#[inline]
pub fn script(c: u32) -> u8 {
    if c > MAX_CODE_POINT {
        return scripts::UNKNOWN;
    }
    tables::SCRIPT_VALUES[range_index(&tables::SCRIPT_STARTS, c)]
}

/// The Script_Extensions of `c` as a sorted list of script ids, or an empty
/// slice when Script_Extensions(c) is just {script(c)} (PCRE2's scriptx
/// value 0). When non-empty, the list may or may not contain `script(c)`,
/// exactly as in Unicode's ScriptExtensions.txt.
#[inline]
pub fn script_extensions(c: u32) -> &'static [u8] {
    if c > MAX_CODE_POINT {
        return &[];
    }
    let set = tables::SCX_VALUES[range_index(&tables::SCX_STARTS, c)];
    tables::SCX_SETS[usize::from(set)]
}

/// True when `script(c) == script` or `script` is one of `c`'s
/// Script_Extensions; this is PCRE2's PT_SCX test.
#[inline]
pub fn script_ext_has(c: u32, script: u8) -> bool {
    self::script(c) == script || script_extensions(c).contains(&script)
}

/// Number of scripts, including Unknown (`ucp_Script_Count`).
#[inline]
pub fn script_count() -> usize {
    SCRIPT_COUNT
}

/// Unicode long name of a script id, e.g. `"Latin"` or `"Old_Turkic"`;
/// `"Unknown"` for out-of-range ids.
pub fn script_name(s: u8) -> &'static str {
    tables::SCRIPT_NAMES
        .get(usize::from(s))
        .copied()
        .unwrap_or("Unknown")
}

// ---------------------------------------------------------------------------
// Grapheme clusters
// ---------------------------------------------------------------------------

/// Grapheme cluster break property of `c` (`GB_*`, PCRE2's
/// `UCD_GRAPHBREAK`). Extended_Pictographic code points (which all have the
/// Unicode value Other) get `GB_EXTENDED_PICTOGRAPHIC`.
#[inline]
pub fn gbprop(c: u32) -> u8 {
    if c > MAX_CODE_POINT {
        return GB_OTHER;
    }
    tables::GB_VALUES[range_index(&tables::GB_STARTS, c)]
}

const ESZ: u32 = (1 << GB_EXTEND) | (1 << GB_SPACING_MARK) | (1 << GB_ZWJ);

/// PCRE2's `ucp_gbtable` (pcre2_tables.c): bit `1 << rgb` is set in
/// `GB_TABLE[lgb]` when there is no grapheme break between a character with
/// break property `lgb` and a following one with `rgb`.
///
/// The table is PCRE2's, not a literal transcription of UAX #29, and it only
/// gives Unicode's rules together with PCRE2's `\X` loop (pcre2_extuni.c):
///
/// * Regional indicators: when `lgb` and `rgb` are both RI, the break is
///   suppressed only if an even number of RIs precede the left character.
/// * Emoji sequences: when `lgb` is Extended_Pictographic and `rgb` is Extend
///   or ZWJ, `lgb` is *not* updated, so it stays Extended_Pictographic. With
///   the ExtPict row allowing ExtPict this implements GB11. The ZWJ row does
///   not allow ExtPict. As in PCRE2, `ExtPict ExtPict` and
///   `ExtPict Extend ExtPict` are not broken either.
/// * The RI row allows only RI (so RI followed by Extend breaks), and the
///   Prepend row does not include ExtPict. Both match PCRE2 10.43/10.44.
pub const GB_TABLE: [u32; 15] = [
    1 << GB_LF, // CR
    0,          // LF
    0,          // Control
    ESZ,        // Extend
    ESZ | (1 << GB_PREPEND)
        | (1 << GB_L)
        | (1 << GB_V)
        | (1 << GB_T)
        | (1 << GB_LV)
        | (1 << GB_LVT)
        | (1 << GB_OTHER)
        | (1 << GB_REGIONAL_INDICATOR), // Prepend
    ESZ,        // SpacingMark
    ESZ | (1 << GB_L) | (1 << GB_V) | (1 << GB_LV) | (1 << GB_LVT), // L
    ESZ | (1 << GB_V) | (1 << GB_T), // V
    ESZ | (1 << GB_T), // T
    ESZ | (1 << GB_V) | (1 << GB_T), // LV
    ESZ | (1 << GB_T), // LVT
    1 << GB_REGIONAL_INDICATOR, // Regional_Indicator
    ESZ,        // Other
    ESZ,        // ZWJ
    ESZ | (1 << GB_EXTENDED_PICTOGRAPHIC), // Extended_Pictographic
];

/// `GB_TABLE[lgb] & (1 << rgb) != 0`: no break between `lgb` and `rgb`
/// according to the table alone (see [`GB_TABLE`] for the extra rules).
#[inline]
pub fn gb_no_break(lgb: u8, rgb: u8) -> bool {
    GB_TABLE
        .get(usize::from(lgb))
        .is_some_and(|row| rgb < 32 && row & (1 << rgb) != 0)
}

// ---------------------------------------------------------------------------
// Bidi class and boolean properties
// ---------------------------------------------------------------------------

/// Bidi class of `c` (`BIDI_*`, PCRE2's `UCD_BIDICLASS`).
#[inline]
pub fn bidi_class(c: u32) -> u8 {
    if c > MAX_CODE_POINT {
        return BIDI_L;
    }
    tables::BIDI_VALUES[range_index(&tables::BIDI_STARTS, c)]
}

/// Whether `c` has boolean property `prop` (`bprops::*`); false for an
/// out-of-range `prop`.
///
/// As in PCRE2, Bidi_Mirrored is true only for characters that have a
/// Bidi_Mirroring_Glyph (BidiMirroring.txt), so e.g. U+2140 DOUBLE-STRUCK
/// N-ARY SUMMATION does not have it.
#[inline]
pub fn bool_prop(c: u32, prop: u8) -> bool {
    if c > MAX_CODE_POINT {
        return false;
    }
    match tables::BPROP_LISTS.get(usize::from(prop)) {
        Some(list) => in_inversion_list(list, c),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Case
// ---------------------------------------------------------------------------

/// The "other case" of `c` (PCRE2's `UCD_OTHERCASE`), or `c` itself if it
/// has none.
///
/// Derived from Simple_Case_Folding like PCRE2: if `c` folds to `f != c`
/// the result is `f`; a folding target gets the lowest code point that folds
/// to it. Non-ASCII members of a caseless set that contains an ASCII letter
/// (U+017F LONG S, U+212A KELVIN SIGN) have no other case, which is what
/// PCRE2_EXTRA_CASELESS_RESTRICT relies on; use [`caseless_set`] for them.
#[inline]
pub fn other_case(c: u32) -> u32 {
    if c < 128 {
        let b = c as u8;
        return if b.is_ascii_alphabetic() { c ^ 0x20 } else { c };
    }
    match tables::OTHER_CASE.binary_search_by_key(&c, |&(k, _)| k) {
        Ok(i) => tables::OTHER_CASE[i].1,
        Err(_) => c,
    }
}

/// The full case-equivalence class of `c` (sorted ascending, including `c`)
/// when it has three or more members, else `None` (PCRE2's
/// `UCD_CASESET`/`ucd_caseless_sets`). Examples: {K, k, U+212A},
/// {S, s, U+017F}, {U+03A3, U+03C2, U+03C3}.
#[inline]
pub fn caseless_set(c: u32) -> Option<&'static [u32]> {
    if c < 0x4B {
        return None;
    }
    tables::CASELESS_MEMBERS
        .binary_search_by_key(&c, |&(k, _)| k)
        .ok()
        .map(|i| tables::CASELESS_SETS[usize::from(tables::CASELESS_MEMBERS[i].1)])
}

// ---------------------------------------------------------------------------
// Property names
// ---------------------------------------------------------------------------

/// Looks up a `\p{..}` property name in PCRE2 10.44's `utt` table and
/// returns `(ptype, pvalue)`, e.g. `b"lu"` -> `(PT_PC, UCP_LU)`,
/// `b"greek"` -> `(PT_SCX, scripts::GREEK)`, `b"bidial"` ->
/// `(PT_BIDICL, BIDI_AL)`.
///
/// `name` must already be normalized as PCRE2's `get_ucp` does it: ASCII
/// lowercase with `_`, `-` and white space removed. For `\p{sc:..}`,
/// `\p{scx:..}` and `\p{bc:..}` the caller looks up the value part (with
/// `"bidi"` prepended for `bc`) and applies get_ucp's script-type rules.
pub fn lookup_property(name: &[u8]) -> Option<(u8, u16)> {
    tables::PROPERTY_NAMES
        .binary_search_by(|&(n, _, _)| n.as_bytes().cmp(name))
        .ok()
        .map(|i| {
            let (_, ptype, pvalue) = tables::PROPERTY_NAMES[i];
            (ptype, pvalue)
        })
}

/// All `(name, ptype, pvalue)` entries of the property name table, sorted
/// bytewise by name.
pub fn property_names() -> &'static [(&'static str, u8, u16)] {
    &tables::PROPERTY_NAMES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char) -> u32 {
        c as u32
    }

    #[test]
    fn enum_values_match_pcre2() {
        assert_eq!(UCP_CC, 0);
        assert_eq!(UCP_ZS, 29);
        assert_eq!(GT_Z, 6);
        assert_eq!(GB_EXTENDED_PICTOGRAPHIC, 14);
        assert_eq!(BIDI_WS, 22);
        assert_eq!(PT_PXXDIGIT, 17);
        assert_eq!(scripts::LATIN, 0);
        assert_eq!(scripts::OLD_UYGHUR, 67);
        assert_eq!(scripts::UNKNOWN, 68);
        assert_eq!(scripts::COMMON, 69);
        assert_eq!(scripts::INHERITED, 84);
        assert_eq!(scripts::NAG_MUNDARI, 163);
        assert_eq!(script_count(), 164);
        assert_eq!(bprops::ASCII, 0);
        assert_eq!(bprops::XID_START, 51);
        assert_eq!(BPROP_COUNT, 52);
        assert_eq!(script_name(scripts::LATIN), "Latin");
        assert_eq!(script_name(scripts::UNKNOWN), "Unknown");
        assert_eq!(script_name(scripts::OLD_TURKIC), "Old_Turkic");
        assert_eq!(script_name(scripts::NAG_MUNDARI), "Nag_Mundari");
        assert_eq!(script_name(200), "Unknown");
        for s in [
            "Cypro_Minoan",
            "Old_Uyghur",
            "Tangsa",
            "Toto",
            "Vithkuqi",
            "Kawi",
            "Nag_Mundari",
        ] {
            assert!(
                (0..script_count()).any(|i| script_name(i as u8) == s),
                "{s}"
            );
        }
    }

    #[test]
    fn gentype_of_every_category() {
        let expected = [
            (UCP_CC, GT_C),
            (UCP_CF, GT_C),
            (UCP_CN, GT_C),
            (UCP_CO, GT_C),
            (UCP_CS, GT_C),
            (UCP_LL, GT_L),
            (UCP_LM, GT_L),
            (UCP_LO, GT_L),
            (UCP_LT, GT_L),
            (UCP_LU, GT_L),
            (UCP_MC, GT_M),
            (UCP_ME, GT_M),
            (UCP_MN, GT_M),
            (UCP_ND, GT_N),
            (UCP_NL, GT_N),
            (UCP_NO, GT_N),
            (UCP_PC, GT_P),
            (UCP_PD, GT_P),
            (UCP_PE, GT_P),
            (UCP_PF, GT_P),
            (UCP_PI, GT_P),
            (UCP_PO, GT_P),
            (UCP_PS, GT_P),
            (UCP_SC, GT_S),
            (UCP_SK, GT_S),
            (UCP_SM, GT_S),
            (UCP_SO, GT_S),
            (UCP_ZL, GT_Z),
            (UCP_ZP, GT_Z),
            (UCP_ZS, GT_Z),
        ];
        for (ct, gt) in expected {
            assert_eq!(gentype(ct), gt, "chartype {ct}");
        }
        assert_eq!(gentype(30), GT_C);
        assert_eq!(category(ch('A')), GT_L);
        assert_eq!(category(ch('5')), GT_N);
        assert_eq!(category(ch(' ')), GT_Z);
    }

    #[test]
    fn chartype_samples() {
        let cases: &[(u32, u8)] = &[
            (0x00, UCP_CC),
            (0x0A, UCP_CC),
            (0x7F, UCP_CC),
            (ch(' '), UCP_ZS),
            (ch('!'), UCP_PO),
            (ch('$'), UCP_SC),
            (ch('('), UCP_PS),
            (ch(')'), UCP_PE),
            (ch('+'), UCP_SM),
            (ch('-'), UCP_PD),
            (ch('0'), UCP_ND),
            (ch('9'), UCP_ND),
            (ch('A'), UCP_LU),
            (ch('Z'), UCP_LU),
            (ch('^'), UCP_SK),
            (ch('_'), UCP_PC),
            (ch('a'), UCP_LL),
            (ch('z'), UCP_LL),
            (0x85, UCP_CC),
            (0xA0, UCP_ZS),
            (0xAA, UCP_LO),
            (0xAB, UCP_PI),
            (0xAD, UCP_CF),
            (0xB5, UCP_LL),
            (0xBB, UCP_PF),
            (0xBC, UCP_NO),
            (0xC0, UCP_LU),
            (0xDF, UCP_LL),
            (0xF7, UCP_SM),
            (0xFF, UCP_LL),
            (0x01C5, UCP_LT),
            (0x02B0, UCP_LM),
            (0x0300, UCP_MN),
            (0x0378, UCP_CN),
            (0x0903, UCP_MC),
            (0x1734, UCP_MC), // Mn -> Mc in Unicode 14
            (0x16EE, UCP_NL),
            (0x20DD, UCP_ME),
            (0x2028, UCP_ZL),
            (0x2029, UCP_ZP),
            (0x3042, UCP_LO),
            (0x4E00, UCP_LO),
            (0x9FFF, UCP_LO),  // CJK, new in Unicode 14
            (0x31350, UCP_LO), // CJK Extension H, new in Unicode 15
            (0x1E4D0, UCP_LO), // NAG MUNDARI LETTER O, Unicode 15
            (0x1F600, UCP_SO),
            (0x1FAE8, UCP_SO), // SHAKING FACE, Unicode 15
            (0x1FAFF, UCP_CN),
            (0xD800, UCP_CS),
            (0xDBFF, UCP_CS),
            (0xDFFF, UCP_CS),
            (0xE000, UCP_CO),
            (0xF8FF, UCP_CO),
            (0xFDD0, UCP_CN),
            (0xFFFE, UCP_CN),
            (0xFFFF, UCP_CN),
            (0xF0000, UCP_CO),
            (0x10FFFD, UCP_CO),
            (0x10FFFE, UCP_CN),
            (0x10FFFF, UCP_CN),
            (0x11_0000, UCP_CN),
            (u32::MAX, UCP_CN),
        ];
        for &(c, ct) in cases {
            assert_eq!(chartype(c), ct, "U+{c:04X}");
        }
    }

    #[test]
    fn every_code_point_is_consistent() {
        let mut assigned = 0u32;
        for c in 0..=MAX_CODE_POINT {
            let ct = chartype(c);
            assert!(ct <= UCP_ZS, "U+{c:04X}");
            let sc = script(c);
            assert!(usize::from(sc) < script_count(), "U+{c:04X}");
            // Unassigned, private-use and surrogate code points (and only
            // those) have script Unknown.
            assert_eq!(
                matches!(ct, UCP_CN | UCP_CO | UCP_CS),
                sc == scripts::UNKNOWN,
                "U+{c:04X}"
            );
            if ct != UCP_CN {
                assigned += 1;
            }
            assert!(gbprop(c) <= GB_EXTENDED_PICTOGRAPHIC);
            assert!(bidi_class(c) <= BIDI_WS);
            // Every scx script other than the character's own sorts before
            // Unknown (the PCRE2 enum is ordered that way).
            for &s in script_extensions(c) {
                assert!(s < scripts::UNKNOWN || s == sc, "U+{c:04X}");
            }
            let o = other_case(c);
            assert!(o <= MAX_CODE_POINT);
            if let Some(set) = caseless_set(c) {
                assert!(set.contains(&c));
                assert!(set.contains(&o), "U+{c:04X}");
            } else if o != c {
                assert_eq!(other_case(o), c, "U+{c:04X} pair");
            }
        }
        // Unicode 15.0: 149,186 characters plus 65 controls, 137,468
        // private-use and 2,048 surrogate code points.
        assert_eq!(assigned, 149_186 + 65 + 137_468 + 2_048);
    }

    #[test]
    fn hangul_syllable_break_properties() {
        for c in 0xAC00..=0xD7A3u32 {
            let expected = if (c - 0xAC00) % 28 == 0 {
                GB_LV
            } else {
                GB_LVT
            };
            assert_eq!(gbprop(c), expected, "U+{c:04X}");
            assert_eq!(script(c), scripts::HANGUL);
        }
    }

    #[test]
    fn scripts_of_samples() {
        let cases: &[(u32, u8)] = &[
            (ch('A'), scripts::LATIN),
            (ch('z'), scripts::LATIN),
            (0xE9, scripts::LATIN),
            (ch('1'), scripts::COMMON),
            (ch(' '), scripts::COMMON),
            (0x03B1, scripts::GREEK),
            (0x2C80, scripts::COPTIC),
            (0x044F, scripts::CYRILLIC),
            (0x05D0, scripts::HEBREW),
            (0x0627, scripts::ARABIC),
            (0x0640, scripts::COMMON),
            (0x0300, scripts::INHERITED),
            (0x0905, scripts::DEVANAGARI),
            (0x0E01, scripts::THAI),
            (0x3042, scripts::HIRAGANA),
            (0x30A2, scripts::KATAKANA),
            (0x3105, scripts::BOPOMOFO),
            (0x4E00, scripts::HAN),
            (0x31350, scripts::HAN),
            (0x16FE2, scripts::HAN), // Common -> Han in Unicode 14
            (0xAC00, scripts::HANGUL),
            (0x10C00, scripts::OLD_TURKIC),
            (0x1F600, scripts::COMMON),
            (0x0378, scripts::UNKNOWN),
            (0xD800, scripts::UNKNOWN),
            (0xE000, scripts::UNKNOWN),
            (0x11_0000, scripts::UNKNOWN),
            // Scripts added after Unicode 13.
            (0x10570, scripts::VITHKUQI),
            (0x12F90, scripts::CYPRO_MINOAN),
            (0x10F70, scripts::OLD_UYGHUR),
            (0x16A70, scripts::TANGSA),
            (0x1E290, scripts::TOTO),
            (0x11F04, scripts::KAWI),
            (0x1E4D0, scripts::NAG_MUNDARI),
            (0x1CF00, scripts::INHERITED), // ZNAMENNY COMBINING MARK
            (0x1CF50, scripts::COMMON),    // ZNAMENNY NEUME
            (0x1E030, scripts::CYRILLIC),  // MODIFIER LETTER CYRILLIC SMALL A
        ];
        for &(c, sc) in cases {
            assert_eq!(
                script(c),
                sc,
                "U+{c:04X}: {} vs {}",
                script_name(script(c)),
                script_name(sc)
            );
        }
    }

    #[test]
    fn script_extensions_samples() {
        // ARABIC TATWEEL: Script=Common, scx = {Adlm Arab Mand Mani Ougr Phlp
        // Rohg Sogd Syrc}.
        let tatweel = 0x0640;
        assert_eq!(script(tatweel), scripts::COMMON);
        for s in [
            scripts::ARABIC,
            scripts::SYRIAC,
            scripts::ADLAM,
            scripts::MANDAIC,
            scripts::MANICHAEAN,
            scripts::OLD_UYGHUR,
            scripts::PSALTER_PAHLAVI,
            scripts::HANIFI_ROHINGYA,
            scripts::SOGDIAN,
        ] {
            assert!(script_ext_has(tatweel, s), "{}", script_name(s));
        }
        assert!(script_ext_has(tatweel, scripts::COMMON));
        assert!(!script_ext_has(tatweel, scripts::LATIN));
        assert!(!script_ext_has(tatweel, scripts::HEBREW));
        assert_eq!(script_extensions(tatweel).len(), 9);
        assert!(script_extensions(tatweel).windows(2).all(|w| w[0] < w[1]));

        assert!(script_extensions(ch('A')).is_empty());
        assert!(script_ext_has(ch('A'), scripts::LATIN));
        assert!(!script_ext_has(ch('A'), scripts::GREEK));

        // DEVANAGARI STRESS SIGN UDATTA: Inherited, used by many Indic scripts.
        assert_eq!(script(0x0951), scripts::INHERITED);
        assert!(script_ext_has(0x0951, scripts::DEVANAGARI));
        assert!(script_ext_has(0x0951, scripts::BENGALI));
        assert!(script_ext_has(0x0951, scripts::LATIN));
        assert!(!script_ext_has(0x0951, scripts::ARABIC));

        // IDEOGRAPHIC COMMA and KATAKANA-HIRAGANA PROLONGED SOUND MARK.
        for s in [scripts::HAN, scripts::HIRAGANA, scripts::KATAKANA] {
            assert!(script_ext_has(0x3001, s));
        }
        assert!(script_ext_has(0x30FC, scripts::HIRAGANA));
        assert!(script_ext_has(0x30FC, scripts::KATAKANA));
        assert!(!script_ext_has(0x30FC, scripts::HAN));

        // ARABIC-INDIC DIGIT ZERO lists its own script.
        assert_eq!(script(0x0660), scripts::ARABIC);
        assert!(script_extensions(0x0660).contains(&scripts::ARABIC));
        assert!(script_ext_has(0x0660, scripts::THAANA));

        assert!(script_extensions(0x11_0000).is_empty());
        assert!(!script_ext_has(0x11_0000, scripts::COMMON));
        assert!(script_ext_has(0x11_0000, scripts::UNKNOWN));
    }

    #[test]
    fn grapheme_break_samples() {
        let cases: &[(u32, u8)] = &[
            (0x0D, GB_CR),
            (0x0A, GB_LF),
            (0x00, GB_CONTROL),
            (0x7F, GB_CONTROL),
            (0xAD, GB_CONTROL),
            (0x2028, GB_CONTROL),
            (0xD800, GB_OTHER),    // surrogates are not Control
            (0xE0000, GB_CONTROL), // unassigned default ignorable
            (ch('a'), GB_OTHER),
            (ch(' '), GB_OTHER),
            (0x0378, GB_OTHER),
            (0x0300, GB_EXTEND),
            (0x200C, GB_EXTEND),
            (0x1F3FB, GB_EXTEND), // emoji modifier
            (0xFE0F, GB_EXTEND),
            (0x200D, GB_ZWJ),
            (0x0600, GB_PREPEND),
            (0x0890, GB_PREPEND),  // ARABIC POUND MARK ABOVE, Unicode 14
            (0x11F02, GB_PREPEND), // KAWI SIGN REPHA, Unicode 15
            (0x0903, GB_SPACING_MARK),
            (0x0E33, GB_SPACING_MARK),
            (0x11720, GB_OTHER), // no longer SpacingMark in Unicode 14
            (0x1100, GB_L),
            (0x1160, GB_V),
            (0x11A8, GB_T),
            (0xAC00, GB_LV),
            (0xAC01, GB_LVT),
            (0x1F1E6, GB_REGIONAL_INDICATOR),
            (0x1F1FF, GB_REGIONAL_INDICATOR),
            (0x00A9, GB_EXTENDED_PICTOGRAPHIC),
            (0x1F600, GB_EXTENDED_PICTOGRAPHIC),
            (0x1FAE8, GB_EXTENDED_PICTOGRAPHIC),
            (0x1FC00, GB_EXTENDED_PICTOGRAPHIC), // reserved, still ExtPict
            (0x11_0000, GB_OTHER),
        ];
        for &(c, gb) in cases {
            assert_eq!(gbprop(c), gb, "U+{c:04X}");
        }
    }

    #[test]
    fn grapheme_break_table() {
        // ucp_gbtable as compiled into PCRE2 10.43 (Unicode 15.0.0).
        assert_eq!(
            GB_TABLE,
            [
                0x0002, 0x0000, 0x0000, 0x2028, 0x3FF8, 0x2028, 0x26E8, 0x21A8, 0x2128, 0x21A8,
                0x2128, 0x0800, 0x2028, 0x2028, 0x6028,
            ]
        );
        assert!(gb_no_break(GB_CR, GB_LF));
        assert!(!gb_no_break(GB_LF, GB_CR));
        assert!(!gb_no_break(GB_CONTROL, GB_EXTEND));
        assert!(!gb_no_break(GB_OTHER, GB_OTHER));
        assert!(gb_no_break(GB_OTHER, GB_EXTEND));
        assert!(gb_no_break(GB_OTHER, GB_ZWJ));
        assert!(gb_no_break(GB_OTHER, GB_SPACING_MARK));
        assert!(gb_no_break(GB_PREPEND, GB_OTHER));
        assert!(!gb_no_break(GB_PREPEND, GB_CONTROL));
        assert!(gb_no_break(GB_L, GB_LVT));
        assert!(!gb_no_break(GB_LV, GB_L));
        assert!(gb_no_break(GB_LVT, GB_T));
        assert!(gb_no_break(GB_REGIONAL_INDICATOR, GB_REGIONAL_INDICATOR));
        assert!(!gb_no_break(GB_REGIONAL_INDICATOR, GB_EXTEND));
        assert!(!gb_no_break(GB_ZWJ, GB_EXTENDED_PICTOGRAPHIC));
        assert!(gb_no_break(
            GB_EXTENDED_PICTOGRAPHIC,
            GB_EXTENDED_PICTOGRAPHIC
        ));
        assert!(!gb_no_break(15, GB_OTHER));
        assert!(!gb_no_break(GB_OTHER, 40));
    }

    #[test]
    fn bidi_class_samples() {
        let cases: &[(u32, u8)] = &[
            (ch('A'), BIDI_L),
            (0x05D0, BIDI_R),
            (0x0627, BIDI_AL),
            (ch('0'), BIDI_EN),
            (0x0660, BIDI_AN),
            (ch(' '), BIDI_WS),
            (0x0A, BIDI_B),
            (0x09, BIDI_S),
            (ch(','), BIDI_CS),
            (ch('+'), BIDI_ES),
            (ch('$'), BIDI_ET),
            (ch('!'), BIDI_ON),
            (0x0300, BIDI_NSM),
            (0x00AD, BIDI_BN),
            (0x202A, BIDI_LRE),
            (0x202B, BIDI_RLE),
            (0x202C, BIDI_PDF),
            (0x202D, BIDI_LRO),
            (0x202E, BIDI_RLO),
            (0x2066, BIDI_LRI),
            (0x2067, BIDI_RLI),
            (0x2068, BIDI_FSI),
            (0x2069, BIDI_PDI),
            (0x4E00, BIDI_L),
            (0x0378, BIDI_L),   // unassigned, default L
            (0x05FF, BIDI_R),   // unassigned in the Hebrew block
            (0x0870, BIDI_AL),  // Arabic Extended-B, Unicode 14
            (0x10EC0, BIDI_AL), // unassigned in Arabic Extended-C
            (0x11_0000, BIDI_L),
        ];
        for &(c, b) in cases {
            assert_eq!(bidi_class(c), b, "U+{c:04X}");
        }
    }

    #[test]
    fn boolean_property_samples() {
        use bprops::*;
        let yes: &[(u32, u8)] = &[
            (ch('A'), ASCII),
            (0x7F, ASCII),
            (ch('f'), ASCII_HEX_DIGIT),
            (ch('F'), HEX_DIGIT),
            (0xFF10, HEX_DIGIT),
            (ch('A'), ALPHABETIC),
            (0x0905, ALPHABETIC),
            (0x093E, ALPHABETIC), // vowel sign (Other_Alphabetic)
            (0x200E, BIDI_CONTROL),
            (ch('('), BIDI_MIRRORED),
            (0x2E55, BIDI_MIRRORED), // Unicode 14 bracket
            (ch('\''), CASE_IGNORABLE),
            (ch('a'), CASED),
            (ch('A'), CHANGES_WHEN_CASEFOLDED),
            (ch('a'), CHANGES_WHEN_CASEMAPPED),
            (ch('A'), CHANGES_WHEN_LOWERCASED),
            (ch('a'), CHANGES_WHEN_TITLECASED),
            (ch('a'), CHANGES_WHEN_UPPERCASED),
            (ch('-'), DASH),
            (0x00AD, DEFAULT_IGNORABLE_CODE_POINT),
            (0x0149, DEPRECATED),
            (ch('^'), DIACRITIC),
            (ch('#'), EMOJI),
            (0x1F600, EMOJI),
            (0x1FAE8, EMOJI), // Unicode 15
            (ch('#'), EMOJI_COMPONENT),
            (0x1F3FB, EMOJI_MODIFIER),
            (0x261D, EMOJI_MODIFIER_BASE),
            (0x1FAF7, EMOJI_MODIFIER_BASE), // Unicode 15
            (0x1F600, EMOJI_PRESENTATION),
            (0x00A9, EXTENDED_PICTOGRAPHIC),
            (0x00B7, EXTENDER),
            (ch('a'), GRAPHEME_BASE),
            (0x0300, GRAPHEME_EXTEND),
            (0x094D, GRAPHEME_LINK),
            (0x2FF0, IDS_BINARY_OPERATOR),
            (0x2FF2, IDS_TRINARY_OPERATOR),
            (ch('_'), ID_CONTINUE),
            (ch('a'), ID_START),
            (0x4E00, IDEOGRAPHIC),
            (0x200D, JOIN_CONTROL),
            (0x0E40, LOGICAL_ORDER_EXCEPTION),
            (ch('a'), LOWERCASE),
            (0x02B0, LOWERCASE),
            (ch('+'), MATH),
            (0xFDD0, NONCHARACTER_CODE_POINT),
            (0x10FFFF, NONCHARACTER_CODE_POINT),
            (ch('!'), PATTERN_SYNTAX),
            (ch(' '), PATTERN_WHITE_SPACE),
            (0x0600, PREPENDED_CONCATENATION_MARK),
            (ch('"'), QUOTATION_MARK),
            (0x2E80, RADICAL),
            (0x1F1E6, REGIONAL_INDICATOR),
            (ch('.'), SENTENCE_TERMINAL),
            (ch('i'), SOFT_DOTTED),
            (ch(','), TERMINAL_PUNCTUATION),
            (0x31350, UNIFIED_IDEOGRAPH),
            (ch('A'), UPPERCASE),
            (0xFE0F, VARIATION_SELECTOR),
            (ch(' '), WHITE_SPACE),
            (0x85, WHITE_SPACE),
            (0x3000, WHITE_SPACE),
            (ch('_'), XID_CONTINUE),
            (ch('a'), XID_START),
        ];
        for &(c, p) in yes {
            assert!(bool_prop(c, p), "U+{c:04X} prop {p}");
        }
        let no: &[(u32, u8)] = &[
            (0x80, ASCII),
            (ch('g'), ASCII_HEX_DIGIT),
            (0xFF10, ASCII_HEX_DIGIT),
            (ch('1'), ALPHABETIC),
            (0x2140, BIDI_MIRRORED), // mirrored, but no mirroring glyph
            (ch('a'), CHANGES_WHEN_LOWERCASED),
            (ch('1'), EMOJI_PRESENTATION),
            (ch('#'), EXTENDED_PICTOGRAPHIC),
            (0x0300, GRAPHEME_BASE),
            (ch('_'), ID_START),
            (ch('_'), XID_START),
            (ch('A'), LOWERCASE),
            (ch('a'), UPPERCASE),
            (ch('a'), WHITE_SPACE),
            (0x200B, WHITE_SPACE),
            (0xFFFD, NONCHARACTER_CODE_POINT),
            (0x0378, GRAPHEME_BASE),
        ];
        for &(c, p) in no {
            assert!(!bool_prop(c, p), "U+{c:04X} prop {p}");
        }
        assert!(!bool_prop(ch('A'), 52));
        assert!(!bool_prop(ch('A'), u8::MAX));
        assert!(!bool_prop(0x11_0000, NONCHARACTER_CODE_POINT));
        assert!(!bool_prop(u32::MAX, ASCII));
        for c in 0..0x110u32 {
            assert_eq!(bool_prop(c, ASCII), c < 128);
        }
    }

    #[test]
    fn other_case_samples() {
        let cases: &[(u32, u32)] = &[
            (ch('a'), ch('A')),
            (ch('A'), ch('a')),
            (ch('k'), ch('K')),
            (ch('K'), ch('k')),
            (ch('s'), ch('S')),
            (ch('S'), ch('s')),
            (ch('i'), ch('I')),
            (ch('I'), ch('i')),
            (ch('1'), ch('1')),
            (ch('@'), ch('@')),
            (0x017F, 0x017F), // LONG S: set with ASCII members, no other case
            (0x212A, 0x212A), // KELVIN SIGN: likewise
            (0x0130, 0x0130), // Turkic dotted capital I: only T/F foldings
            (0x0131, 0x0131), // dotless i
            (0x00B5, 0x03BC),
            (0x039C, 0x03BC),
            (0x03BC, 0x00B5),
            (0x03A3, 0x03C3),
            (0x03C3, 0x03A3),
            (0x03C2, 0x03C3),
            (0x00DF, 0x1E9E),
            (0x1E9E, 0x00DF),
            (0x00C5, 0x00E5),
            (0x212B, 0x00E5),
            (0x01C4, 0x01C6),
            (0x01C5, 0x01C6),
            (0x01C6, 0x01C4),
            (0x0410, 0x0430),
            (0x13A0, 0xAB70), // Cherokee folds to upper case
            (0xAB70, 0x13A0),
            (0x2C2F, 0x2C5F),   // Unicode 14 Glagolitic pair
            (0x10570, 0x10597), // Unicode 14 Vithkuqi pair
            (0x10597, 0x10570),
            (0x4E00, 0x4E00),
            (0x11_0000, 0x11_0000),
        ];
        for &(c, o) in cases {
            assert_eq!(other_case(c), o, "U+{c:04X}");
        }
        for c in 0..128u32 {
            let b = c as u8;
            let expected = if b.is_ascii_uppercase() {
                u32::from(b.to_ascii_lowercase())
            } else {
                u32::from(b.to_ascii_uppercase())
            };
            assert_eq!(other_case(c), expected);
            // The ASCII fast path agrees with the table.
            let table = tables::OTHER_CASE
                .binary_search_by_key(&c, |&(k, _)| k)
                .map_or(c, |i| tables::OTHER_CASE[i].1);
            assert_eq!(table, expected);
        }
    }

    #[test]
    fn caseless_set_samples() {
        let k: &[u32] = &[0x4B, 0x6B, 0x212A];
        let s: &[u32] = &[0x53, 0x73, 0x17F];
        let sigma: &[u32] = &[0x3A3, 0x3C2, 0x3C3];
        let theta: &[u32] = &[0x398, 0x3B8, 0x3D1, 0x3F4];
        let mu: &[u32] = &[0xB5, 0x39C, 0x3BC];
        let aring: &[u32] = &[0xC5, 0xE5, 0x212B];
        let dz: &[u32] = &[0x1C4, 0x1C5, 0x1C6];
        for set in [k, s, sigma, theta, mu, aring, dz] {
            for &c in set {
                assert_eq!(caseless_set(c), Some(set), "U+{c:04X}");
            }
        }
        for c in [
            ch('a'),
            ch('A'),
            ch('i'),
            ch('I'),
            0x130,
            0x131,
            0xDF,
            0x1E9E,
            0x4E00,
            0x11_0000,
        ] {
            assert_eq!(caseless_set(c), None, "U+{c:04X}");
        }
        for set in tables::CASELESS_SETS {
            assert!(set.len() >= 3);
            assert!(set.windows(2).all(|w| w[0] < w[1]));
            for &c in set {
                assert_eq!(caseless_set(c), Some(set));
            }
        }
        assert!(tables::CASELESS_MEMBERS.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(tables::CASELESS_MEMBERS[0].0, 0x4B);
    }

    #[test]
    fn lookup_property_samples() {
        let cases: &[(&str, u8, u16)] = &[
            ("any", PT_ANY, 0),
            ("l&", PT_LAMP, 0),
            ("lc", PT_LAMP, 0),
            ("c", PT_GC, GT_C as u16),
            ("l", PT_GC, GT_L as u16),
            ("z", PT_GC, GT_Z as u16),
            ("cc", PT_PC, UCP_CC as u16),
            ("lu", PT_PC, UCP_LU as u16),
            ("sc", PT_PC, UCP_SC as u16),
            ("zs", PT_PC, UCP_ZS as u16),
            ("xan", PT_ALNUM, 0),
            ("xps", PT_PXSPACE, 0),
            ("xsp", PT_SPACE, 0),
            ("xwd", PT_WORD, 0),
            ("xuc", PT_UCNC, 0),
            ("latin", PT_SCX, scripts::LATIN as u16),
            ("latn", PT_SCX, scripts::LATIN as u16),
            ("greek", PT_SCX, scripts::GREEK as u16),
            ("grek", PT_SCX, scripts::GREEK as u16),
            ("arabic", PT_SCX, scripts::ARABIC as u16),
            ("han", PT_SCX, scripts::HAN as u16),
            ("hani", PT_SCX, scripts::HAN as u16),
            ("coptic", PT_SCX, scripts::COPTIC as u16),
            ("copt", PT_SCX, scripts::COPTIC as u16),
            ("qaac", PT_SCX, scripts::COPTIC as u16),
            ("cyprominoan", PT_SCX, scripts::CYPRO_MINOAN as u16),
            ("cpmn", PT_SCX, scripts::CYPRO_MINOAN as u16),
            ("olduyghur", PT_SCX, scripts::OLD_UYGHUR as u16),
            ("ougr", PT_SCX, scripts::OLD_UYGHUR as u16),
            ("common", PT_SC, scripts::COMMON as u16),
            ("zyyy", PT_SC, scripts::COMMON as u16),
            ("inherited", PT_SC, scripts::INHERITED as u16),
            ("zinh", PT_SC, scripts::INHERITED as u16),
            ("qaai", PT_SC, scripts::INHERITED as u16),
            ("unknown", PT_SC, scripts::UNKNOWN as u16),
            ("zzzz", PT_SC, scripts::UNKNOWN as u16),
            ("hebrew", PT_SC, scripts::HEBREW as u16),
            ("oldturkic", PT_SC, scripts::OLD_TURKIC as u16),
            ("orkh", PT_SC, scripts::OLD_TURKIC as u16),
            ("signwriting", PT_SC, scripts::SIGNWRITING as u16),
            ("sgnw", PT_SC, scripts::SIGNWRITING as u16),
            ("vithkuqi", PT_SC, scripts::VITHKUQI as u16),
            ("vith", PT_SC, scripts::VITHKUQI as u16),
            ("tangsa", PT_SC, scripts::TANGSA as u16),
            ("tnsa", PT_SC, scripts::TANGSA as u16),
            ("toto", PT_SC, scripts::TOTO as u16),
            ("kawi", PT_SC, scripts::KAWI as u16),
            ("nagmundari", PT_SC, scripts::NAG_MUNDARI as u16),
            ("nagm", PT_SC, scripts::NAG_MUNDARI as u16),
            ("ascii", PT_BOOL, bprops::ASCII as u16),
            ("alphabetic", PT_BOOL, bprops::ALPHABETIC as u16),
            ("alpha", PT_BOOL, bprops::ALPHABETIC as u16),
            ("whitespace", PT_BOOL, bprops::WHITE_SPACE as u16),
            ("wspace", PT_BOOL, bprops::WHITE_SPACE as u16),
            ("space", PT_BOOL, bprops::WHITE_SPACE as u16),
            ("asciihexdigit", PT_BOOL, bprops::ASCII_HEX_DIGIT as u16),
            ("ahex", PT_BOOL, bprops::ASCII_HEX_DIGIT as u16),
            ("bidicontrol", PT_BOOL, bprops::BIDI_CONTROL as u16),
            ("bidic", PT_BOOL, bprops::BIDI_CONTROL as u16),
            ("bidimirrored", PT_BOOL, bprops::BIDI_MIRRORED as u16),
            ("bidim", PT_BOOL, bprops::BIDI_MIRRORED as u16),
            (
                "extendedpictographic",
                PT_BOOL,
                bprops::EXTENDED_PICTOGRAPHIC as u16,
            ),
            ("extpict", PT_BOOL, bprops::EXTENDED_PICTOGRAPHIC as u16),
            ("graphemelink", PT_BOOL, bprops::GRAPHEME_LINK as u16),
            ("grlink", PT_BOOL, bprops::GRAPHEME_LINK as u16),
            ("xidstart", PT_BOOL, bprops::XID_START as u16),
            ("xids", PT_BOOL, bprops::XID_START as u16),
            ("bidial", PT_BIDICL, BIDI_AL as u16),
            ("bidil", PT_BIDICL, BIDI_L as u16),
            ("bidinsm", PT_BIDICL, BIDI_NSM as u16),
            ("bidiws", PT_BIDICL, BIDI_WS as u16),
        ];
        for &(name, ptype, pvalue) in cases {
            assert_eq!(
                lookup_property(name.as_bytes()),
                Some((ptype, pvalue)),
                "{name}"
            );
        }
        for name in [
            "",
            "foo",
            "latinx",
            "lat",
            "Latin", // not normalized
            "old_turkic",
            "letter",
            "uppercaseletter",
            "l_",
            "lt&",
            "katakanaorhiragana",
            "hrkt",
            "bidi",
            "bidiarabicletter",
            "bc",
            "scx",
            "script",
            "hyphen",
            "othermath",
            "compositionexclusion",
            "anyx",
            "xx",
        ] {
            assert_eq!(lookup_property(name.as_bytes()), None, "{name}");
        }
    }

    #[test]
    fn property_name_table_round_trips() {
        let names = property_names();
        // PCRE2 10.43/10.44's utt table has 489 entries.
        assert_eq!(names.len(), 489);
        assert!(
            names
                .windows(2)
                .all(|w| w[0].0.as_bytes() < w[1].0.as_bytes())
        );
        for &(name, ptype, pvalue) in names {
            assert_eq!(
                lookup_property(name.as_bytes()),
                Some((ptype, pvalue)),
                "{name}"
            );
            assert!(
                name.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'&')
            );
            let v = usize::from(pvalue);
            match ptype {
                PT_ANY | PT_LAMP | PT_ALNUM | PT_SPACE | PT_PXSPACE | PT_WORD | PT_UCNC => {
                    assert_eq!(pvalue, 0)
                }
                PT_GC => assert!(v <= usize::from(GT_Z)),
                PT_PC => assert!(v <= usize::from(UCP_ZS)),
                PT_SC => assert!(v < script_count() && v >= usize::from(scripts::UNKNOWN)),
                PT_SCX => assert!(v < usize::from(scripts::UNKNOWN)),
                PT_BOOL => assert!(v < BPROP_COUNT),
                PT_BIDICL => assert!(v <= usize::from(BIDI_WS)),
                _ => panic!("unexpected type {ptype} for {name}"),
            }
        }
        // Every script is reachable by its normalized long name.
        for s in 0..script_count() {
            let name: String = script_name(s as u8)
                .chars()
                .filter(|&c| c != '_')
                .map(|c| c.to_ascii_lowercase())
                .collect();
            let pt = if s < usize::from(scripts::UNKNOWN) {
                PT_SCX
            } else {
                PT_SC
            };
            assert_eq!(
                lookup_property(name.as_bytes()),
                Some((pt, s as u16)),
                "{name}"
            );
        }
        let count = |pt: u8| names.iter().filter(|e| e.1 == pt).count();
        assert_eq!(count(PT_GC), 7);
        assert_eq!(count(PT_PC), 30);
        assert_eq!(count(PT_BIDICL), 23);
        assert_eq!(count(PT_LAMP), 2);
        assert_eq!(count(PT_BOOL), 99);
        assert_eq!(count(PT_SC) + count(PT_SCX), 322);
    }

    #[test]
    fn range_tables_are_well_formed() {
        fn check(starts: &[u32], n_values: usize) {
            assert_eq!(starts[0], 0);
            assert_eq!(starts.len(), n_values);
            assert!(starts.windows(2).all(|w| w[0] < w[1]));
            assert!(*starts.last().unwrap() <= MAX_CODE_POINT);
        }
        check(&tables::SCRIPT_STARTS, tables::SCRIPT_VALUES.len());
        check(&tables::GB_STARTS, tables::GB_VALUES.len());
        check(&tables::BIDI_STARTS, tables::BIDI_VALUES.len());
        check(&tables::SCX_STARTS, tables::SCX_VALUES.len());
        assert!(tables::SCX_SETS[0].is_empty());
        for set in &tables::SCX_SETS[1..] {
            assert!(!set.is_empty());
            assert!(set.windows(2).all(|w| w[0] < w[1]));
        }
        for list in tables::BPROP_LISTS {
            assert!(list.windows(2).all(|w| w[0] < w[1]));
            assert!(list.last().is_none_or(|&l| l <= MAX_CODE_POINT + 1));
        }
        assert!(tables::OTHER_CASE.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(
            tables::CHARTYPE_STAGE1.len(),
            (MAX_CODE_POINT as usize + 1) / 128
        );
        assert_eq!(tables::CHARTYPE_STAGE2.len() % 128, 0);
    }
}
