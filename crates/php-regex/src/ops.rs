//! The compiled program: a vector of opcodes closely mirroring PCRE2's
//! compiled code (OP_xxx). Links between brackets, alternatives and kets are
//! *relative* offsets (in opcodes), exactly like PCRE2's LINK fields, so that
//! fragments can be moved and duplicated during compilation.

use crate::chars::utf8_len;
use crate::unicode;

/// Repetition type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RepKind {
    /// Greedy.
    Max,
    /// Lazy.
    Min,
    /// Possessive.
    Pos,
}

/// Unlimited maximum repeat.
pub(crate) const REP_INF: u32 = u32::MAX;

/// Ket variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KetKind {
    Ket,
    KetRMax,
    KetRMin,
    KetRPos,
}

/// Bracket opcodes, in PCRE2 order (OP_ASSERT .. OP_SCOND).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum BraKind {
    Assert,
    AssertNot,
    AssertBack,
    AssertBackNot,
    AssertNa,
    AssertBackNa,
    Once,
    ScriptRun,
    Bra,
    BraPos,
    CBra,
    CBraPos,
    Cond,
    SBra,
    SBraPos,
    SCBra,
    SCBraPos,
    SCond,
}

impl BraKind {
    pub(crate) fn is_assertion(self) -> bool {
        self <= BraKind::AssertBackNa
    }
    pub(crate) fn is_lookbehind(self) -> bool {
        matches!(
            self,
            BraKind::AssertBack | BraKind::AssertBackNot | BraKind::AssertBackNa
        )
    }
    pub(crate) fn is_capture(self) -> bool {
        matches!(
            self,
            BraKind::CBra | BraKind::CBraPos | BraKind::SCBra | BraKind::SCBraPos
        )
    }
    /// OP_SBRA - OP_BRA conversion ("could be empty" variant).
    pub(crate) fn to_s(self) -> BraKind {
        match self {
            BraKind::Bra => BraKind::SBra,
            BraKind::CBra => BraKind::SCBra,
            BraKind::Cond => BraKind::SCond,
            BraKind::BraPos => BraKind::SBraPos,
            BraKind::CBraPos => BraKind::SCBraPos,
            k => k,
        }
    }
    /// `*bracode += 1`: switch to the POS variant.
    pub(crate) fn to_pos(self) -> BraKind {
        match self {
            BraKind::Bra => BraKind::BraPos,
            BraKind::SBra => BraKind::SBraPos,
            BraKind::CBra => BraKind::CBraPos,
            BraKind::SCBra => BraKind::SCBraPos,
            k => k,
        }
    }
}

/// An item in an extended class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum XItem {
    Single(u32),
    Range(u32, u32),
    Prop(u8, u16),
    NotProp(u8, u16),
}

/// Which PCRE2 class opcode this class corresponds to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassKind {
    /// OP_CLASS: chars > 255 never match.
    Class,
    /// OP_NCLASS: chars > 255 always match.
    NClass,
    /// OP_XCLASS.
    XClass {
        not: bool,
        has_map: bool,
        has_prop: bool,
    },
}

/// A compiled character class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Class {
    pub kind: ClassKind,
    /// Bitmap for characters < 256 (as PCRE2 stores it, negation applied
    /// where PCRE2 applies it).
    pub map: [u8; 32],
    pub xitems: Vec<XItem>,
}

impl Class {
    #[inline]
    pub(crate) fn map_has(&self, c: u32) -> bool {
        c < 256 && self.map[(c / 8) as usize] & (1 << (c % 8)) != 0
    }

    /// Does the class match character `c`?
    #[inline]
    pub(crate) fn matches(&self, c: u32) -> bool {
        match self.kind {
            ClassKind::Class => self.map_has(c),
            ClassKind::NClass => c > 255 || self.map_has(c),
            ClassKind::XClass {
                not,
                has_map,
                has_prop,
            } => {
                let not_negated = !not;
                if c < 256 {
                    if !has_prop {
                        if !has_map {
                            return !not_negated;
                        }
                        return self.map_has(c);
                    }
                    if has_map && self.map_has(c) {
                        return not_negated;
                    }
                }
                for it in &self.xitems {
                    match *it {
                        XItem::Single(x) => {
                            if c == x {
                                return not_negated;
                            }
                        }
                        XItem::Range(x, y) => {
                            if c >= x && c <= y {
                                return not_negated;
                            }
                        }
                        XItem::Prop(t, v) => {
                            if unicode_prop_matches(c, t, v) {
                                return not_negated;
                            }
                        }
                        XItem::NotProp(t, v) => {
                            if !unicode_prop_matches(c, t, v) {
                                return not_negated;
                            }
                        }
                    }
                }
                !not_negated
            }
        }
    }

    /// PCRE2 code length of the class opcode.
    pub(crate) fn pcre2_len(&self, utf: bool) -> usize {
        match self.kind {
            ClassKind::Class | ClassKind::NClass => 33,
            ClassKind::XClass { has_map, .. } => {
                let mut n = 4 + if has_map { 32 } else { 0 } + 1;
                for it in &self.xitems {
                    n += match *it {
                        XItem::Single(x) => 1 + clen(x, utf),
                        XItem::Range(x, y) => 1 + clen(x, utf) + clen(y, utf),
                        _ => 3,
                    };
                }
                n
            }
        }
    }
}

#[inline]
fn clen(c: u32, utf: bool) -> usize {
    if utf { utf8_len(c) } else { 1 }
}

/// Unicode property test used by OP_PROP and XCLASS property items.
pub(crate) fn unicode_prop_matches(c: u32, ptype: u8, pvalue: u16) -> bool {
    use unicode::*;
    match ptype {
        PT_ANY => true,
        PT_LAMP => {
            let t = chartype(c);
            t == UCP_LU || t == UCP_LL || t == UCP_LT
        }
        PT_GC => gentype(chartype(c)) as u16 == pvalue,
        PT_PC => chartype(c) as u16 == pvalue,
        PT_SC => script(c) as u16 == pvalue,
        PT_SCX => script_ext_has(c, pvalue as u8),
        PT_ALNUM => {
            let g = gentype(chartype(c));
            g == GT_L || g == GT_N
        }
        PT_SPACE | PT_PXSPACE => {
            if crate::chars::is_hspace(c) || crate::chars::is_vspace(c) {
                true
            } else {
                gentype(chartype(c)) == GT_Z
            }
        }
        PT_WORD => {
            let t = chartype(c);
            let g = gentype(t);
            g == GT_L || g == GT_N || t == UCP_MN || t == UCP_PC
        }
        // A caseless set, identified by any of its members (all such sets
        // are in the BMP).
        PT_CLIST => caseless_set(pvalue as u32).is_some_and(|s| s.contains(&c)),
        PT_UCNC => {
            c == b'$' as u32
                || c == b'@' as u32
                || c == b'`' as u32
                || (0xa0..=0xd7ff).contains(&c)
                || c >= 0xe000
        }
        PT_BIDICL => bidi_class(c) as u16 == pvalue,
        PT_BOOL => bool_prop(c, pvalue as u8),
        PT_PXGRAPH => {
            let t = chartype(c);
            let g = gentype(t);
            g != GT_Z
                && (g != GT_C
                    || (t == UCP_CF
                        && c != 0x061c
                        && c != 0x180e
                        && !(0x2066..=0x2069).contains(&c)))
        }
        PT_PXPRINT => {
            let t = chartype(c);
            t != UCP_ZL
                && t != UCP_ZP
                && (gentype(t) != GT_C
                    || (t == UCP_CF && c != 0x061c && !(0x2066..=0x2069).contains(&c)))
        }
        PT_PXPUNCT => {
            let g = gentype(chartype(c));
            g == GT_P || (c < 128 && g == GT_S)
        }
        PT_PXXDIGIT => {
            (b'0' as u32..=b'9' as u32).contains(&c)
                || (b'A' as u32..=b'F' as u32).contains(&c)
                || (b'a' as u32..=b'f' as u32).contains(&c)
                || (0xff10..=0xff19).contains(&c)
                || (0xff21..=0xff26).contains(&c)
                || (0xff41..=0xff46).contains(&c)
        }
        _ => false,
    }
}

/// A single-character matcher (the "item" part of PCRE2's character,
/// character-type and class opcodes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    /// OP_CHAR
    Char(u32),
    /// OP_CHARI: the character and its other case (may be equal).
    CharI(u32, u32),
    /// OP_NOT
    Not(u32),
    /// OP_NOTI: the character and its other case.
    NotI(u32, u32),
    NotDigit,
    Digit,
    NotWhitespace,
    Whitespace,
    NotWordchar,
    Wordchar,
    Any,
    AllAny,
    AnyByte,
    NotProp(u8, u16),
    Prop(u8, u16),
    AnyNl,
    NotHspace,
    Hspace,
    NotVspace,
    Vspace,
    Extuni,
    Class(Box<Class>),
}

impl Item {
    /// PCRE2 code length of the item when it stands alone.
    pub(crate) fn pcre2_len(&self, utf: bool) -> usize {
        match self {
            Item::Char(c) | Item::CharI(c, _) | Item::Not(c) | Item::NotI(c, _) => {
                1 + clen(*c, utf)
            }
            Item::Prop(..) | Item::NotProp(..) => 3,
            Item::Class(cl) => cl.pcre2_len(utf),
            _ => 1,
        }
    }

    /// Length of the character data (for repeat opcodes).
    fn data_len(&self, utf: bool) -> usize {
        match self {
            Item::Char(c) | Item::CharI(c, _) | Item::Not(c) | Item::NotI(c, _) => clen(*c, utf),
            Item::Prop(..) | Item::NotProp(..) => 3,
            _ => 1,
        }
    }
}

/// One opcode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    End,
    Sod,
    Som,
    SetSom,
    NotWordBoundary,
    WordBoundary,
    NotUcpWordBoundary,
    UcpWordBoundary,
    Eodn,
    Eod,
    Doll,
    DollM,
    Circ,
    CircM,
    /// A single character matcher.
    Item(Item),
    /// A repeated single character matcher (OP_STAR..OP_TYPEPOSUPTO and
    /// classes followed by OP_CRxxx).
    Rep {
        item: Item,
        min: u32,
        max: u32,
        kind: RepKind,
    },
    /// Back reference, possibly repeated (`rep` = None means not repeated).
    Ref {
        group: u16,
        caseless: bool,
        rep: Option<(u32, u32, RepKind)>,
    },
    /// Back reference to a duplicated name: index/count in the name table.
    DnRef {
        index: u16,
        count: u16,
        caseless: bool,
        rep: Option<(u32, u32, RepKind)>,
    },
    /// Recursion / subroutine call. `target` is the absolute index of the
    /// called group's bracket opcode (0 for the whole pattern).
    Recurse {
        group: u16,
        target: usize,
    },
    Callout,
    /// Alternation; relative offset to the next Alt or Ket.
    Alt(usize),
    /// End of group; relative (backwards) offset to the bracket.
    Ket(KetKind, usize),
    Reverse(u32),
    VReverse(u32, u32),
    /// Start of a bracket; `next` is the relative offset to the first Alt or
    /// the Ket; `group` is the capture number for capturing brackets.
    Bra {
        kind: BraKind,
        next: usize,
        group: u16,
    },
    CRef(u16),
    DnCRef(u16, u16),
    RRef(u16),
    DnRRef(u16, u16),
    False,
    True,
    BraZero,
    BraMinZero,
    BraPosZero,
    SkipZero,
    Mark(u16),
    Prune,
    PruneArg(u16),
    Skip,
    SkipArg(u16),
    Then,
    ThenArg(u16),
    Commit,
    CommitArg(u16),
    Fail,
    Accept,
    AssertAccept,
    Close(u16),
}

impl Op {
    /// PCRE2 code length (in code units) of this opcode, used for the
    /// "regular expression is too large" limit.
    pub(crate) fn pcre2_len(&self, utf: bool, marks: &[Vec<u8>]) -> usize {
        match self {
            Op::Item(it) => it.pcre2_len(utf),
            Op::Rep { item, min, max, .. } => {
                if let Item::Class(cl) = item {
                    let base = cl.pcre2_len(utf);
                    return base
                        + if *min == 1 && *max == 1 {
                            0
                        } else if (*min == 0 || *min == 1) && (*max == REP_INF || *max == 1) {
                            1
                        } else {
                            5
                        };
                }
                let d = item.data_len(utf);
                let single = |n: u32| -> usize {
                    // one repeat opcode with optional count
                    if n == REP_INF { 1 + d } else { 3 + d }
                };
                if *min == 0 {
                    if *max == REP_INF || *max == 1 {
                        1 + d
                    } else {
                        single(*max)
                    }
                } else if *min == 1 {
                    if *max == REP_INF {
                        1 + d
                    } else {
                        let mut n = item.pcre2_len(utf);
                        if *max > 1 {
                            n += 3 + d;
                        }
                        n
                    }
                } else {
                    let mut n = 3 + d;
                    if *max != *min {
                        n += if *max == REP_INF || *max - *min == 1 {
                            1 + d
                        } else {
                            3 + d
                        };
                    }
                    n
                }
            }
            Op::Ref { rep, .. } => 3 + rep_len(rep),
            Op::DnRef { rep, .. } => 5 + rep_len(rep),
            Op::Recurse { .. } => 3,
            Op::Callout => 6,
            Op::Alt(_) | Op::Ket(..) | Op::Reverse(_) => 3,
            Op::VReverse(..) => 5,
            Op::Bra { kind, .. } => {
                if kind.is_capture() {
                    5
                } else {
                    3
                }
            }
            Op::CRef(_) | Op::RRef(_) | Op::Close(_) => 3,
            Op::DnCRef(..) | Op::DnRRef(..) => 5,
            Op::Mark(i) | Op::PruneArg(i) | Op::SkipArg(i) | Op::ThenArg(i) | Op::CommitArg(i) => {
                3 + marks.get(*i as usize).map_or(0, |m| m.len())
            }
            _ => 1,
        }
    }
}

fn rep_len(rep: &Option<(u32, u32, RepKind)>) -> usize {
    match rep {
        None => 0,
        Some((min, max, _)) => {
            if (*min == 0 || *min == 1) && (*max == REP_INF || *max == 1) {
                1
            } else {
                5
            }
        }
    }
}
