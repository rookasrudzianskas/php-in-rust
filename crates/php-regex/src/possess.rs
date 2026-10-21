//! Auto-possessification of single-character repeats, following the
//! structure of PCRE2's `pcre2_auto_possess.c`: a repeated item is made
//! possessive when what can follow it can never start with a character the
//! item could match, so backtracking into it could never help.
//!
//! The comparisons of properties and types are done semantically (only
//! provably disjoint combinations are accepted), so results are always the
//! same as without possessification; only the amount of backtracking (and
//! hence the backtrack/depth counters) is reduced, as in PCRE2.

use crate::chars::{self, CTYPE_DIGIT, CTYPE_SPACE, CTYPE_WORD};
use crate::compile::Program;
use crate::ops::*;
use crate::unicode::*;

/// The "property list" of an item, as get_chr_property_list() builds it.
#[derive(Clone, Debug)]
enum PList {
    /// OP_CHAR list: matches any of these characters.
    Chars(Vec<u32>),
    /// OP_NOT list: matches any character except these.
    NotChars(Vec<u32>),
    Type(TypeOp),
    Prop(bool, u8, u16),
    Class(Class),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeOp {
    NotDigit,
    Digit,
    NotWhitespace,
    Whitespace,
    NotWordchar,
    Wordchar,
    Any,
    AllAny,
    AnyNl,
    NotHspace,
    Hspace,
    NotVspace,
    Vspace,
    Extuni,
    Eodn,
    Eod,
    Doll,
    DollM,
}

impl TypeOp {
    fn idx(self) -> usize {
        self as usize
    }
}

/// Column order: \D \d \S \s \W \w . .+ \R \H \h \V \v \X \Z \z $ $M
/// (OP_ANYBYTE and the property columns are omitted; they are never accepted).
/// Rows: \D \d \S \s \W \w . .+ \R \H \h \V \v \X.
/// 1 means the left item can be possessified when followed by the right one.
/// These are PCRE2's `autoposstab` values (including its few deliberate
/// approximations, e.g. `\R` before `\s`), so backtracking behaviour and the
/// match/depth counters are the same as PCRE2's.
const AUTOPOSSTAB: [[u8; 18]; 14] = [
    //\D \d \S \s \W \w  . .+ \R \H \h \V \v \X \Z \z  $ $M
    [0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], // \D
    [1, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1], // \d
    [0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1], // \S
    [0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], // \s
    [0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], // \W
    [0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1], // \w
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0], // .
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], // .+
    [0, 1, 0, 1, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0], // \R
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0], // \H
    [0, 1, 1, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0], // \h
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 1, 0, 0], // \V
    [0, 1, 1, 0, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0], // \v
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], // \X
];

fn type_of(item: &Item) -> Option<TypeOp> {
    Some(match item {
        Item::NotDigit => TypeOp::NotDigit,
        Item::Digit => TypeOp::Digit,
        Item::NotWhitespace => TypeOp::NotWhitespace,
        Item::Whitespace => TypeOp::Whitespace,
        Item::NotWordchar => TypeOp::NotWordchar,
        Item::Wordchar => TypeOp::Wordchar,
        Item::Any => TypeOp::Any,
        Item::AllAny => TypeOp::AllAny,
        Item::AnyNl => TypeOp::AnyNl,
        Item::NotHspace => TypeOp::NotHspace,
        Item::Hspace => TypeOp::Hspace,
        Item::NotVspace => TypeOp::NotVspace,
        Item::Vspace => TypeOp::Vspace,
        Item::Extuni => TypeOp::Extuni,
        _ => return None,
    })
}

fn item_plist(item: &Item) -> Option<PList> {
    Some(match item {
        Item::Char(c) => PList::Chars(vec![*c]),
        Item::CharI(c, oc) => {
            if c == oc {
                PList::Chars(vec![*c])
            } else {
                PList::Chars(vec![*c, *oc])
            }
        }
        Item::Not(c) => PList::NotChars(vec![*c]),
        Item::NotI(c, oc) => {
            if c == oc {
                PList::NotChars(vec![*c])
            } else {
                PList::NotChars(vec![*c, *oc])
            }
        }
        Item::Prop(PT_CLIST, v) => PList::Chars(caseless_set(*v as u32)?.to_vec()),
        Item::NotProp(PT_CLIST, v) => PList::NotChars(caseless_set(*v as u32)?.to_vec()),
        Item::Prop(t, v) => PList::Prop(false, *t, *v),
        Item::NotProp(t, v) => PList::Prop(true, *t, *v),
        Item::Class(cl) => PList::Class((**cl).clone()),
        Item::AnyByte => return None,
        other => PList::Type(type_of(other)?),
    })
}

/// get_chr_property_list(): (list, "may match empty").
fn get_chr_property_list(op: &Op) -> Option<(PList, bool)> {
    match op {
        Op::Item(it) => Some((item_plist(it)?, false)),
        Op::Rep { item, min, .. } => Some((item_plist(item)?, *min == 0)),
        Op::Eodn => Some((PList::Type(TypeOp::Eodn), false)),
        Op::Eod => Some((PList::Type(TypeOp::Eod), false)),
        Op::Doll => Some((PList::Type(TypeOp::Doll), false)),
        Op::DollM => Some((PList::Type(TypeOp::DollM), false)),
        _ => None,
    }
}

fn next_of(op: &Op) -> usize {
    match op {
        Op::Bra { next, .. } | Op::Alt(next) => *next,
        _ => 1,
    }
}

/// Index after the Ket of the bracket starting at `i`.
fn skip_bracket(ops: &[Op], mut i: usize) -> usize {
    loop {
        i += next_of(&ops[i]);
        if !matches!(ops[i], Op::Alt(_)) {
            break;
        }
    }
    i + 1
}

/// Does character `chr` possibly match `other`? (Conservative: true when unsure.)
fn char_may_match(chr: u32, other: &PList) -> bool {
    match other {
        PList::Chars(cs) => cs.contains(&chr),
        PList::NotChars(cs) => !cs.contains(&chr),
        PList::Type(t) => match t {
            TypeOp::Digit => chr < 256 && chars::ctype(chr) & CTYPE_DIGIT != 0,
            TypeOp::NotDigit => chr > 255 || chars::ctype(chr) & CTYPE_DIGIT == 0,
            TypeOp::Whitespace => chr < 256 && chars::ctype(chr) & CTYPE_SPACE != 0,
            TypeOp::NotWhitespace => chr > 255 || chars::ctype(chr) & CTYPE_SPACE == 0,
            TypeOp::Wordchar => chr < 256 && chars::ctype(chr) & CTYPE_WORD != 0,
            TypeOp::NotWordchar => chr > 255 || chars::ctype(chr) & CTYPE_WORD == 0,
            TypeOp::Hspace => chars::is_hspace(chr),
            TypeOp::NotHspace => !chars::is_hspace(chr),
            TypeOp::AnyNl | TypeOp::Vspace => chars::is_vspace(chr),
            TypeOp::NotVspace => !chars::is_vspace(chr),
            TypeOp::Doll | TypeOp::Eodn => chars::is_vspace(chr),
            TypeOp::Eod => false,
            _ => true,
        },
        // As in PCRE2's check_char_prop(), bidi classes, boolean properties
        // and other specials are not analysed (treated as "may match").
        PList::Prop(_, PT_BIDICL | PT_BOOL | PT_UCNC | PT_ANY, _) => true,
        PList::Prop(neg, t, v) => unicode_prop_matches(chr, *t, *v) != *neg,
        PList::Class(cl) => cl.matches(chr),
    }
}

/// PCRE2's propposstab: how to compare two property types (rows: left,
/// columns: right; PT_ANY .. PT_BOOL).
const PROPPOSSTAB: [[u8; 14]; 14] = [
    // ANY LAMP GC PC SC SCX ALNUM SPACE PXSPACE WORD CLIST UCNC BIDICL BOOL
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT_ANY
    [0, 3, 0, 0, 0, 0, 3, 1, 1, 0, 0, 0, 0, 0], // PT_LAMP
    [0, 0, 2, 4, 0, 0, 9, 10, 10, 11, 0, 0, 0, 0], // PT_GC
    [0, 0, 5, 2, 0, 0, 15, 16, 16, 17, 0, 0, 0, 0], // PT_PC
    [0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0], // PT_SC
    [0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0], // PT_SCX
    [0, 3, 6, 12, 0, 0, 3, 1, 1, 0, 0, 0, 0, 0], // PT_ALNUM
    [0, 1, 7, 13, 0, 0, 1, 3, 3, 1, 0, 0, 0, 0], // PT_SPACE
    [0, 1, 7, 13, 0, 0, 1, 3, 3, 1, 0, 0, 0, 0], // PT_PXSPACE
    [0, 0, 8, 14, 0, 0, 0, 1, 1, 3, 0, 0, 0, 0], // PT_WORD
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT_CLIST
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0], // PT_UCNC
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT_BIDICL
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT_BOOL
];

/// PCRE2's posspropstab rows for ALNUM, SPACE/PXSPACE and WORD.
const POSSPROPSTAB: [[u8; 4]; 3] = [
    [GT_L, GT_N, GT_N, UCP_NL],
    [GT_Z, GT_Z, GT_C, UCP_CC],
    [GT_L, GT_N, GT_P, UCP_PO],
];

/// catposstab[general][particular]: 1 if the particular category is not
/// part of the general category.
fn catposstab(g: u8, p: u16) -> bool {
    gentype(p as u8) != g
}

/// The OP_PROP / OP_NOTPROP comparison of PCRE2's compare_opcodes().
fn props_disjoint(ln: bool, lt: u8, lv: u16, rn: bool, rt: u8, rv: u16) -> bool {
    if lt as usize >= 14 || rt as usize >= 14 {
        return false;
    }
    let same = ln == rn;
    let lisprop = !ln;
    let risprop = !rn;
    let bothprop = lisprop && risprop;
    let n = PROPPOSSTAB[lt as usize][rt as usize];
    match n {
        1 => bothprop,
        2 => (lv == rv) != same,
        3 => !same,
        4 => risprop && catposstab(lv as u8, rv) == same,
        5 => lisprop && catposstab(rv as u8, lv) == same,
        6..=8 => {
            let p = POSSPROPSTAB[(n - 6) as usize];
            risprop
                && lisprop
                    == (rv != p[0] as u16 && rv != p[1] as u16 && (rv != p[2] as u16 || !lisprop))
        }
        9..=11 => {
            let p = POSSPROPSTAB[(n - 9) as usize];
            lisprop
                && risprop
                    == (lv != p[0] as u16 && lv != p[1] as u16 && (lv != p[2] as u16 || !risprop))
        }
        12..=14 => {
            let p = POSSPROPSTAB[(n - 12) as usize];
            risprop
                && lisprop
                    == (catposstab(p[0], rv)
                        && catposstab(p[1], rv)
                        && (rv != p[3] as u16 || !lisprop))
        }
        15..=17 => {
            let p = POSSPROPSTAB[(n - 15) as usize];
            lisprop
                && risprop
                    == (catposstab(p[0], lv)
                        && catposstab(p[1], lv)
                        && (lv != p[3] as u16 || !risprop))
        }
        _ => false,
    }
}

/// Outcome of comparing the base iterator with the next item.
enum Cmp {
    /// Possessification not possible.
    No,
    /// Disjoint; if the next item cannot match empty, possessify.
    Yes,
    /// XCLASS without a map: the answer if the next item cannot be empty.
    YesIfNotEmpty(bool),
}

fn is_bitmap_class(l: &PList, utf: bool) -> Option<&Class> {
    match l {
        PList::Class(cl)
            if matches!(cl.kind, ClassKind::Class)
                || (!utf && matches!(cl.kind, ClassKind::NClass)) =>
        {
            Some(cl)
        }
        _ => None,
    }
}

/// The base iterator vs the next item (`list`), structured exactly like the
/// comparisons in PCRE2's compare_opcodes().
fn disjoint(base: &PList, list: &PList, utf: bool) -> Cmp {
    let to = |b: bool| if b { Cmp::Yes } else { Cmp::No };
    // A small character list on either side.
    if let PList::Chars(cs) = base {
        return to(cs.iter().all(|&c| !char_may_match(c, list)));
    }
    if let PList::Chars(cs) = list {
        return to(cs.iter().all(|&c| !char_may_match(c, base)));
    }
    // Bitmap classes.
    let bc = is_bitmap_class(base, utf);
    let lc = is_bitmap_class(list, utf);
    if bc.is_some() || lc.is_some() {
        let (set1, other) = match bc {
            Some(cl) => (cl.map, list),
            None => (lc.unwrap().map, base),
        };
        let (set2, invert): ([u8; 32], bool) = match other {
            PList::Class(cl) => match cl.kind {
                ClassKind::Class | ClassKind::NClass => (cl.map, false),
                ClassKind::XClass {
                    not,
                    has_map,
                    has_prop,
                } => {
                    if has_prop {
                        return Cmp::No;
                    }
                    if !has_map {
                        return Cmp::YesIfNotEmpty(!not);
                    }
                    (cl.map, false)
                }
            },
            PList::Type(t) => {
                let (cbit, inv) = match t {
                    TypeOp::NotDigit => (chars::CBIT_DIGIT, true),
                    TypeOp::Digit => (chars::CBIT_DIGIT, false),
                    TypeOp::NotWhitespace => (chars::CBIT_SPACE, true),
                    TypeOp::Whitespace => (chars::CBIT_SPACE, false),
                    TypeOp::NotWordchar => (chars::CBIT_WORD, true),
                    TypeOp::Wordchar => (chars::CBIT_WORD, false),
                    _ => return Cmp::No,
                };
                let mut m = [0u8; 32];
                m.copy_from_slice(&chars::DEFAULT_TABLES.cbits[cbit..cbit + 32]);
                (m, inv)
            }
            _ => return Cmp::No,
        };
        for k in 0..32 {
            let s2 = if invert { !set2[k] } else { set2[k] };
            if set1[k] & s2 != 0 {
                return Cmp::No;
            }
        }
        return Cmp::Yes;
    }
    match (base, list) {
        (PList::Prop(..), PList::Type(TypeOp::Eod)) => Cmp::Yes,
        (PList::Prop(ln, lt, lv), PList::Prop(rn, rt, rv)) => {
            to(props_disjoint(*ln, *lt, *lv, *rn, *rt, *rv))
        }
        (PList::Type(l), PList::Type(r)) => {
            let li = l.idx();
            if li > TypeOp::Extuni.idx() {
                return Cmp::No;
            }
            to(AUTOPOSSTAB[li][r.idx()] != 0)
        }
        _ => Cmp::No,
    }
}

struct Ctx<'a> {
    ops: &'a [Op],
    had_recurse: bool,
    utf: bool,
    rec_limit: i32,
}

/// compare_opcodes(): can the iterator described by `base` (greedy flag
/// `greedy`) be possessified given what follows at `code`?
fn compare_opcodes(
    cx: &mut Ctx,
    mut code: usize,
    base: &PList,
    base_is_char: bool,
    greedy: bool,
    entered: bool,
) -> bool {
    cx.rec_limit -= 1;
    if cx.rec_limit <= 0 {
        return false;
    }
    let ops = cx.ops;
    let mut entered_a_group = entered;
    loop {
        if matches!(ops[code], Op::Callout) {
            code += 1;
            continue;
        }
        if matches!(ops[code], Op::Alt(_)) {
            while matches!(ops[code], Op::Alt(_)) {
                code += next_of(&ops[code]);
            }
        }
        match &ops[code] {
            Op::End => return greedy,
            Op::Ket(KetKind::Ket, back) | Op::Ket(KetKind::KetRPos, back) => {
                if !greedy {
                    return false;
                }
                if let Op::Bra { kind, .. } = ops[code - back] {
                    match kind {
                        BraKind::CBra | BraKind::SCBra | BraKind::CBraPos | BraKind::SCBraPos => {
                            if cx.had_recurse {
                                return false;
                            }
                        }
                        BraKind::ScriptRun => {
                            if !base_is_char {
                                return false;
                            }
                        }
                        BraKind::Assert
                        | BraKind::AssertNot
                        | BraKind::AssertBack
                        | BraKind::AssertBackNot
                        | BraKind::Once => return !entered_a_group,
                        BraKind::AssertNa | BraKind::AssertBackNa => return false,
                        _ => {}
                    }
                }
                code += 1;
                continue;
            }
            Op::Bra {
                kind: BraKind::Once | BraKind::Bra | BraKind::CBra,
                next,
                ..
            } => {
                let mut next_code = code + next;
                code += 1;
                while matches!(ops[next_code], Op::Alt(_)) {
                    if !compare_opcodes(cx, code, base, base_is_char, greedy, true) {
                        return false;
                    }
                    code = next_code + 1;
                    next_code += next_of(&ops[next_code]);
                }
                entered_a_group = true;
                continue;
            }
            Op::BraZero | Op::BraMinZero => {
                let nc = code + 1;
                if !matches!(
                    ops[nc],
                    Op::Bra {
                        kind: BraKind::Bra | BraKind::CBra | BraKind::Once,
                        ..
                    }
                ) {
                    return false;
                }
                let after = skip_bracket(ops, nc);
                if !compare_opcodes(cx, after, base, base_is_char, greedy, entered_a_group) {
                    return false;
                }
                code += 1;
                continue;
            }
            _ => {}
        }
        let Some((list, can_be_empty)) = get_chr_property_list(&ops[code]) else {
            return false;
        };
        code += 1;
        match disjoint(base, &list, cx.utf) {
            Cmp::No => return false,
            Cmp::Yes => {}
            Cmp::YesIfNotEmpty(v) => {
                if !can_be_empty {
                    return v;
                }
                continue;
            }
        }
        if !can_be_empty {
            return true;
        }
    }
}

/// Possessify suitable repeats in the program.
pub(crate) fn auto_possessify(prog: &mut Program) {
    let had_recurse = prog.ops.iter().any(|o| matches!(o, Op::Recurse { .. }));
    let n = prog.ops.len();
    for i in 0..n {
        let (base, greedy, is_char) = match &prog.ops[i] {
            Op::Rep {
                item,
                min,
                max,
                kind,
            } if *kind != RepKind::Pos && min != max => {
                let Some(base) = item_plist(item) else {
                    continue;
                };
                let is_char = matches!(item, Item::Char(_) | Item::CharI(..));
                (base, *kind == RepKind::Max, is_char)
            }
            _ => continue,
        };
        let mut cx = Ctx {
            ops: &prog.ops,
            had_recurse,
            utf: prog.utf,
            rec_limit: 1000,
        };
        if compare_opcodes(&mut cx, i + 1, &base, is_char, greedy, false)
            && let Op::Rep { kind, .. } = &mut prog.ops[i]
        {
            *kind = RepKind::Pos;
        }
    }
}
