//! Start-of-match analysis: ports of PCRE2's `is_anchored()`,
//! `is_startline()`, `find_firstassertedcu()`, `first_significant_code()`
//! and `pcre2_study()` (`set_start_bits()` and `find_minlength()`).

use crate::chars::{self, CBIT_DIGIT, CBIT_SPACE, CBIT_WORD, utf8_first_byte};
use crate::compile::Program;
use crate::ops::*;
use crate::opts::*;
use crate::unicode;

pub(crate) struct AnchorInfo {
    pub backref_map: u32,
    pub had_pruneorskip: bool,
    pub no_dotstar_anchor: bool,
}

#[inline]
pub(crate) fn next_of(op: &Op) -> usize {
    match op {
        Op::Bra { next, .. } | Op::Alt(next) => *next,
        _ => 1,
    }
}

/// Index of the Ket of the bracket at `i`.
pub(crate) fn bracket_ket(ops: &[Op], mut i: usize) -> usize {
    loop {
        i += next_of(&ops[i]);
        if !matches!(ops[i], Op::Alt(_)) {
            return i;
        }
    }
}

/// Index after the Ket of the bracket at `i`.
#[inline]
pub(crate) fn skip_bracket(ops: &[Op], i: usize) -> usize {
    bracket_ket(ops, i) + 1
}

/// first_significant_code()
pub(crate) fn first_significant_code(ops: &[Op], mut i: usize, skipassert: bool) -> usize {
    loop {
        match &ops[i] {
            Op::Bra {
                kind:
                    BraKind::AssertNot
                    | BraKind::AssertBack
                    | BraKind::AssertBackNot
                    | BraKind::AssertBackNa,
                ..
            } => {
                if !skipassert {
                    return i;
                }
                i = skip_bracket(ops, i);
            }
            Op::WordBoundary
            | Op::NotWordBoundary
            | Op::UcpWordBoundary
            | Op::NotUcpWordBoundary => {
                if !skipassert {
                    return i;
                }
                i += 1;
            }
            Op::Callout
            | Op::CRef(_)
            | Op::DnCRef(..)
            | Op::RRef(_)
            | Op::DnRRef(..)
            | Op::False
            | Op::True => i += 1,
            Op::SkipZero => {
                // code += 2 + GET(code, 2) + LINK_SIZE: past the first Alt/Ket.
                i = i + 1 + next_of(&ops[i + 1]) + 1;
            }
            Op::Bra {
                kind: BraKind::Cond | BraKind::SCond,
                next,
                ..
            } => {
                if !matches!(ops[i + 1], Op::False) || !matches!(ops[i + next], Op::Ket(..)) {
                    return i;
                }
                i += next + 1;
            }
            Op::Mark(_) | Op::CommitArg(_) | Op::PruneArg(_) | Op::SkipArg(_) | Op::ThenArg(_) => {
                i += 1
            }
            _ => return i,
        }
    }
}

fn is_dotstar(op: &Op, allany: bool) -> bool {
    match op {
        Op::Rep {
            item,
            min: 0,
            max: REP_INF,
            ..
        } => {
            if allany {
                matches!(item, Item::AllAny)
            } else {
                matches!(item, Item::Any)
            }
        }
        _ => false,
    }
}

/// is_anchored()
pub(crate) fn is_anchored(
    ops: &[Op],
    mut code: usize,
    bracket_map: u32,
    info: &AnchorInfo,
    atomcount: u32,
    inassert: bool,
) -> bool {
    loop {
        let scode = first_significant_code(ops, code + 1, false);
        match &ops[scode] {
            Op::Bra { kind, group, next } => match kind {
                BraKind::Bra | BraKind::BraPos | BraKind::SBra | BraKind::SBraPos => {
                    if !is_anchored(ops, scode, bracket_map, info, atomcount, inassert) {
                        return false;
                    }
                }
                BraKind::CBra | BraKind::CBraPos | BraKind::SCBra | BraKind::SCBraPos => {
                    let n = *group as u32;
                    let new_map = bracket_map | if n < 32 { 1 << n } else { 1 };
                    if !is_anchored(ops, scode, new_map, info, atomcount, inassert) {
                        return false;
                    }
                }
                BraKind::Assert | BraKind::AssertNa => {
                    if !is_anchored(ops, scode, bracket_map, info, atomcount, true) {
                        return false;
                    }
                }
                BraKind::Cond | BraKind::SCond => {
                    if !matches!(ops[scode + next], Op::Alt(_)) {
                        return false;
                    }
                    if !is_anchored(ops, scode, bracket_map, info, atomcount, inassert) {
                        return false;
                    }
                }
                BraKind::Once => {
                    if !is_anchored(ops, scode, bracket_map, info, atomcount + 1, inassert) {
                        return false;
                    }
                }
                _ => return false,
            },
            op if is_dotstar(op, true) => {
                if bracket_map & info.backref_map != 0
                    || atomcount > 0
                    || info.had_pruneorskip
                    || inassert
                    || info.no_dotstar_anchor
                {
                    return false;
                }
            }
            Op::Rep {
                min: 0,
                max: REP_INF,
                ..
            } => return false,
            Op::Sod | Op::Som | Op::Circ => {}
            _ => return false,
        }
        code += next_of(&ops[code]);
        if !matches!(ops[code], Op::Alt(_)) {
            break;
        }
    }
    true
}

/// is_startline()
pub(crate) fn is_startline(
    ops: &[Op],
    mut code: usize,
    bracket_map: u32,
    info: &AnchorInfo,
    atomcount: u32,
    inassert: bool,
) -> bool {
    loop {
        let mut scode = first_significant_code(ops, code + 1, false);
        if let Op::Bra {
            kind: BraKind::Cond,
            ..
        } = ops[scode]
        {
            scode += 1;
            if matches!(ops[scode], Op::Callout) {
                scode += 1;
            }
            match ops[scode] {
                Op::CRef(_)
                | Op::DnCRef(..)
                | Op::RRef(_)
                | Op::DnRRef(..)
                | Op::Fail
                | Op::False
                | Op::True => return false,
                _ => {
                    if !is_startline(ops, scode, bracket_map, info, atomcount, true) {
                        return false;
                    }
                    scode = skip_bracket(ops, scode);
                }
            }
            scode = first_significant_code(ops, scode, false);
        }
        match &ops[scode] {
            Op::Bra { kind, group, .. } => match kind {
                BraKind::Bra | BraKind::BraPos | BraKind::SBra | BraKind::SBraPos => {
                    if !is_startline(ops, scode, bracket_map, info, atomcount, inassert) {
                        return false;
                    }
                }
                BraKind::CBra | BraKind::CBraPos | BraKind::SCBra | BraKind::SCBraPos => {
                    let n = *group as u32;
                    let new_map = bracket_map | if n < 32 { 1 << n } else { 1 };
                    if !is_startline(ops, scode, new_map, info, atomcount, inassert) {
                        return false;
                    }
                }
                BraKind::Assert | BraKind::AssertNa => {
                    if !is_startline(ops, scode, bracket_map, info, atomcount, true) {
                        return false;
                    }
                }
                BraKind::Once => {
                    if !is_startline(ops, scode, bracket_map, info, atomcount + 1, inassert) {
                        return false;
                    }
                }
                _ => return false,
            },
            op if is_dotstar(op, false) => {
                if bracket_map & info.backref_map != 0
                    || atomcount > 0
                    || info.had_pruneorskip
                    || inassert
                    || info.no_dotstar_anchor
                {
                    return false;
                }
            }
            Op::Circ | Op::CircM => {}
            _ => return false,
        }
        code += next_of(&ops[code]);
        if !matches!(ops[code], Op::Alt(_)) {
            break;
        }
    }
    true
}

/// find_firstassertedcu(): returns (code unit, flags).
pub(crate) fn find_firstassertedcu(
    ops: &[Op],
    mut code: usize,
    inassert: u32,
    utf: bool,
) -> (u32, u32) {
    let mut c = 0u32;
    let mut cflags = REQ_NONE;
    loop {
        let scode = first_significant_code(ops, code + 1, true);
        match &ops[scode] {
            Op::Bra { kind, .. }
                if matches!(
                    kind,
                    BraKind::Bra
                        | BraKind::BraPos
                        | BraKind::CBra
                        | BraKind::SCBra
                        | BraKind::CBraPos
                        | BraKind::SCBraPos
                        | BraKind::Assert
                        | BraKind::AssertNa
                        | BraKind::Once
                        | BraKind::ScriptRun
                ) =>
            {
                let add = if matches!(kind, BraKind::Assert | BraKind::AssertNa) {
                    1
                } else {
                    0
                };
                let (d, dflags) = find_firstassertedcu(ops, scode, inassert + add, utf);
                if dflags >= REQ_NONE {
                    return (0, REQ_NONE);
                }
                if cflags >= REQ_NONE {
                    c = d;
                    cflags = dflags;
                } else if c != d || cflags != dflags {
                    return (0, REQ_NONE);
                }
            }
            op => {
                let (ch, caseless) = match op {
                    Op::Item(Item::Char(ch)) => (*ch, false),
                    Op::Item(Item::CharI(ch, _)) => (*ch, true),
                    Op::Rep {
                        item: Item::Char(ch),
                        min,
                        ..
                    } if *min >= 1 => (*ch, false),
                    Op::Rep {
                        item: Item::CharI(ch, _),
                        min,
                        ..
                    } if *min >= 1 => (*ch, true),
                    _ => return (0, REQ_NONE),
                };
                if inassert == 0 {
                    return (0, REQ_NONE);
                }
                // The first code unit of the character.
                let cu = utf8_first_byte_if(ch, utf);
                if caseless {
                    if cu >= 0x80 {
                        return (0, REQ_NONE);
                    }
                    if cflags >= REQ_NONE {
                        c = cu;
                        cflags = REQ_CASELESS;
                    } else if c != cu {
                        return (0, REQ_NONE);
                    }
                } else if cflags >= REQ_NONE {
                    c = cu;
                    cflags = 0;
                } else if c != cu {
                    return (0, REQ_NONE);
                }
            }
        }
        code += next_of(&ops[code]);
        if !matches!(ops[code], Op::Alt(_)) {
            break;
        }
    }
    (c, cflags)
}

fn utf8_first_byte_if(c: u32, utf: bool) -> u32 {
    if utf { utf8_first_byte(c) as u32 } else { c }
}

// ----------------------------------------------------------------------
// pcre2_study()
// ----------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ssb {
    Fail,
    Done,
    Continue,
    TooDeep,
}

struct StartBits<'a> {
    ops: &'a [Op],
    utf: bool,
    ucp: bool,
    bits: [u8; 32],
}

impl StartBits<'_> {
    #[inline]
    fn set(&mut self, c: u32) {
        let c = c.min(255);
        self.bits[(c / 8) as usize] |= 1 << (c % 8);
    }

    /// set_table_bit()
    fn set_table_bit(&mut self, c: u32, caseless: bool) {
        let first = if self.utf {
            utf8_first_byte(c) as u32
        } else {
            c
        };
        self.set(first);
        if caseless {
            if self.utf || self.ucp {
                let oc = unicode::other_case(c);
                if self.utf {
                    self.set(utf8_first_byte(oc) as u32);
                } else if oc < 256 {
                    self.set(oc);
                }
            } else if c < 256 {
                self.set(chars::fcc(c));
            }
        }
    }

    fn set_type_bits(&mut self, cbit: usize, table_limit: usize) {
        let t = &chars::DEFAULT_TABLES.cbits[cbit..cbit + 32];
        for (b, &x) in self.bits.iter_mut().zip(t).take(table_limit) {
            *b |= x;
        }
        if table_limit == 32 {
            return;
        }
        for c in 128u32..256 {
            if chars::DEFAULT_TABLES.cbits[(c / 8) as usize] & (1 << (c & 7)) != 0 {
                self.set(utf8_first_byte(c) as u32);
            }
        }
    }

    fn set_nottype_bits(&mut self, cbit: usize, table_limit: usize) {
        let t = &chars::DEFAULT_TABLES.cbits[cbit..cbit + 32];
        for (b, &x) in self.bits.iter_mut().zip(t).take(table_limit) {
            *b |= !x;
        }
        if table_limit != 32 {
            for c in 24..32 {
                self.bits[c] = 0xff;
            }
        }
    }

    fn hspace_bits(&mut self) {
        self.set(0x09);
        self.set(0x20);
        if self.utf {
            for b in [0xc2, 0xe1, 0xe2, 0xe3] {
                self.set(b);
            }
        } else {
            self.set(0xa0);
        }
    }

    fn vspace_bits(&mut self) {
        for b in [0x0a, 0x0b, 0x0c, 0x0d] {
            self.set(b);
        }
        if self.utf {
            self.set(0xc2);
            self.set(0xe2);
        } else {
            self.set(0x85);
        }
    }

    fn type_bits(&mut self, item: &Item) -> bool {
        let table_limit = if self.utf { 16 } else { 32 };
        match item {
            Item::NotDigit => self.set_nottype_bits(CBIT_DIGIT, table_limit),
            Item::Digit => self.set_type_bits(CBIT_DIGIT, table_limit),
            Item::NotWhitespace => self.set_nottype_bits(CBIT_SPACE, table_limit),
            Item::Whitespace => self.set_type_bits(CBIT_SPACE, table_limit),
            Item::NotWordchar => self.set_nottype_bits(CBIT_WORD, table_limit),
            Item::Wordchar => self.set_type_bits(CBIT_WORD, table_limit),
            Item::Hspace => self.hspace_bits(),
            Item::AnyNl | Item::Vspace => self.vspace_bits(),
            _ => return false,
        }
        true
    }

    /// Class handling. Returns false for SSB_FAIL.
    fn class_bits(&mut self, cl: &Class) -> bool {
        let mut classmap: Option<[u8; 32]> = None;
        let mut nclass_bits = false;
        match cl.kind {
            ClassKind::XClass {
                not,
                has_map,
                has_prop,
            } => {
                if has_prop || (not && !has_map) {
                    return false;
                }
                if has_map {
                    classmap = Some(cl.map);
                }
                if self.utf && !not {
                    for it in &cl.xitems {
                        match *it {
                            XItem::Single(x) => {
                                let b = utf8_first_byte(x) as u32;
                                self.set(b);
                            }
                            XItem::Range(x, y) => {
                                let b = utf8_first_byte(x) as u32;
                                let e = utf8_first_byte(y) as u32;
                                for k in b..=e {
                                    self.set(k);
                                }
                            }
                            _ => {}
                        }
                    }
                } else {
                    nclass_bits = true;
                }
            }
            ClassKind::NClass => {
                nclass_bits = true;
                classmap = Some(cl.map);
            }
            ClassKind::Class => classmap = Some(cl.map),
        }
        if nclass_bits && self.utf {
            self.bits[24] |= 0xf0;
            for k in 25..32 {
                self.bits[k] = 0xff;
            }
        }
        if let Some(map) = classmap {
            if self.utf {
                for (b, &x) in self.bits.iter_mut().zip(map.iter()).take(16) {
                    *b |= x;
                }
                let mut c = 128u32;
                while c < 256 {
                    if map[(c / 8) as usize] & (1 << (c & 7)) != 0 {
                        let d = (c >> 6) | 0xc0;
                        self.set(d);
                        c = (c & 0xc0) + 0x40 - 1;
                    }
                    c += 1;
                }
            } else {
                for (b, &x) in self.bits.iter_mut().zip(map.iter()) {
                    *b |= x;
                }
            }
        }
        true
    }

    fn set_start_bits(&mut self, mut code: usize, depth: &mut u32) -> Ssb {
        let ops = self.ops;
        *depth += 1;
        if *depth > 1000 {
            return Ssb::TooDeep;
        }
        let mut yield_ = Ssb::Done;
        loop {
            let mut tcode = code + 1;
            let mut try_next = true;
            while try_next {
                match &ops[tcode] {
                    Op::Accept
                    | Op::AssertAccept
                    | Op::CircM
                    | Op::Close(_)
                    | Op::Commit
                    | Op::CommitArg(_)
                    | Op::CRef(_)
                    | Op::False
                    | Op::True
                    | Op::DnCRef(..)
                    | Op::DnRef { .. }
                    | Op::DnRRef(..)
                    | Op::Doll
                    | Op::DollM
                    | Op::End
                    | Op::Eod
                    | Op::Eodn
                    | Op::Fail
                    | Op::Mark(_)
                    | Op::Prune
                    | Op::PruneArg(_)
                    | Op::Recurse { .. }
                    | Op::Ref { .. }
                    | Op::Reverse(_)
                    | Op::VReverse(..)
                    | Op::RRef(_)
                    | Op::SetSom
                    | Op::Skip
                    | Op::SkipArg(_)
                    | Op::Sod
                    | Op::Som
                    | Op::Then
                    | Op::ThenArg(_) => return Ssb::Fail,
                    Op::Bra {
                        kind: BraKind::Cond | BraKind::SCond,
                        ..
                    } => return Ssb::Fail,
                    Op::Circ => tcode += 1,
                    Op::Callout => tcode += 1,
                    Op::WordBoundary
                    | Op::NotWordBoundary
                    | Op::UcpWordBoundary
                    | Op::NotUcpWordBoundary => tcode += 1,
                    Op::Bra { kind, .. } => {
                        let kind = *kind;
                        let mut do_group = true;
                        match kind {
                            BraKind::Assert | BraKind::AssertNa => {
                                let mut ncode = skip_bracket(ops, tcode);
                                loop {
                                    match &ops[ncode] {
                                        Op::Bra { kind: k, .. } if k.is_assertion() => {
                                            ncode = skip_bracket(ops, ncode)
                                        }
                                        Op::WordBoundary
                                        | Op::NotWordBoundary
                                        | Op::UcpWordBoundary
                                        | Op::NotUcpWordBoundary
                                        | Op::Callout => ncode += 1,
                                        _ => break,
                                    }
                                }
                                let mandatory = match &ops[ncode] {
                                    Op::Item(Item::Prop(unicode::PT_CLIST, _)) => true,
                                    Op::Item(
                                        Item::AnyNl
                                        | Item::Char(_)
                                        | Item::CharI(..)
                                        | Item::Hspace
                                        | Item::Vspace
                                        | Item::Digit
                                        | Item::NotDigit
                                        | Item::Wordchar
                                        | Item::NotWordchar
                                        | Item::Whitespace
                                        | Item::NotWhitespace,
                                    ) => true,
                                    Op::Rep {
                                        item: Item::Char(_) | Item::CharI(..),
                                        min,
                                        ..
                                    } => *min >= 1,
                                    _ => false,
                                };
                                if mandatory {
                                    tcode = ncode;
                                    do_group = false;
                                }
                            }
                            BraKind::AssertNot
                            | BraKind::AssertBack
                            | BraKind::AssertBackNot
                            | BraKind::AssertBackNa => {
                                tcode = skip_bracket(ops, tcode);
                                do_group = false;
                            }
                            _ => {}
                        }
                        if do_group {
                            let rc = self.set_start_bits(tcode, depth);
                            match rc {
                                Ssb::Done => try_next = false,
                                Ssb::Continue => tcode = skip_bracket(ops, tcode),
                                _ => return rc,
                            }
                        }
                    }
                    Op::Alt(_) => {
                        yield_ = Ssb::Continue;
                        try_next = false;
                    }
                    Op::Ket(..) => return Ssb::Continue,
                    Op::BraZero | Op::BraMinZero | Op::BraPosZero => {
                        tcode += 1;
                        let rc = self.set_start_bits(tcode, depth);
                        if matches!(rc, Ssb::Fail | Ssb::TooDeep) {
                            return rc;
                        }
                        tcode = skip_bracket(ops, tcode);
                    }
                    Op::SkipZero => tcode = skip_bracket(ops, tcode + 1),
                    Op::Item(item) => match item {
                        Item::Char(c) => {
                            self.set_table_bit(*c, false);
                            try_next = false;
                        }
                        Item::CharI(c, _) => {
                            self.set_table_bit(*c, true);
                            try_next = false;
                        }
                        Item::Prop(unicode::PT_CLIST, v) => {
                            if let Some(set) = unicode::caseless_set(*v as u32) {
                                for &c in set {
                                    let c = if self.utf {
                                        utf8_first_byte(c) as u32
                                    } else {
                                        c
                                    };
                                    self.set(c);
                                }
                            }
                            try_next = false;
                        }
                        Item::Class(cl) => {
                            if !self.class_bits(cl) {
                                return Ssb::Fail;
                            }
                            try_next = false;
                        }
                        it => {
                            if self.type_bits(it) {
                                try_next = false;
                            } else {
                                return Ssb::Fail;
                            }
                        }
                    },
                    Op::Rep { item, min, .. } => {
                        let min = *min;
                        match item {
                            Item::Char(c) | Item::CharI(c, _) => {
                                self.set_table_bit(*c, matches!(item, Item::CharI(..)));
                                if min >= 1 {
                                    try_next = false;
                                } else {
                                    tcode += 1;
                                }
                            }
                            Item::Not(_) | Item::NotI(..) => return Ssb::Fail,
                            Item::Class(cl) => {
                                if !self.class_bits(cl) {
                                    return Ssb::Fail;
                                }
                                if min == 0 {
                                    tcode += 1;
                                } else {
                                    try_next = false;
                                }
                            }
                            Item::Prop(unicode::PT_CLIST, v) if min >= 1 => {
                                if let Some(set) = unicode::caseless_set(*v as u32) {
                                    for &c in set {
                                        let c = if self.utf {
                                            utf8_first_byte(c) as u32
                                        } else {
                                            c
                                        };
                                        self.set(c);
                                    }
                                }
                                try_next = false;
                            }
                            it => {
                                if min >= 1 {
                                    // TYPEPLUS / TYPEEXACT: behave like the single item.
                                    if self.type_bits(it) {
                                        try_next = false;
                                    } else {
                                        return Ssb::Fail;
                                    }
                                } else {
                                    if !self.type_bits(it) {
                                        return Ssb::Fail;
                                    }
                                    tcode += 1;
                                }
                            }
                        }
                    }
                }
            }
            code += next_of(&ops[code]);
            if !matches!(ops[code], Op::Alt(_)) {
                break;
            }
        }
        yield_
    }
}

/// pcre2_study(): set up the start bitmap (and possibly a first code unit)
/// and the minimum length.
pub(crate) fn study(prog: &mut Program) {
    let utf = prog.utf;
    let ucp = prog.ucp;
    if prog.flags & (FLAG_FIRSTSET | FLAG_STARTLINE) == 0 {
        let mut sb = StartBits {
            ops: &prog.ops,
            utf,
            ucp,
            bits: [0; 32],
        };
        let mut depth = 0;
        let rc = sb.set_start_bits(0, &mut depth);
        let bits = sb.bits;
        if rc == Ssb::Done {
            prog.start_bitmap = bits;
            let mut a: i32 = -1;
            let mut b: i32 = -1;
            let mut flags = FLAG_FIRSTMAPSET;
            let mut done_early = false;
            'scan: for (bi, &x) in bits.iter().enumerate() {
                if x != 0 {
                    let y = x.isolate_lowest_one();
                    if y != x {
                        done_early = true;
                        break 'scan;
                    }
                    let c = (bi * 8) as i32 + x.trailing_zeros() as i32;
                    if utf && c > 127 {
                        done_early = true;
                        break 'scan;
                    }
                    if a < 0 {
                        a = c;
                    } else if b < 0 {
                        let mut d = chars::fcc(c as u32) as i32;
                        if utf || ucp {
                            if unicode::caseless_set(c as u32).is_some() {
                                done_early = true;
                                break 'scan;
                            }
                            if c > 127 {
                                d = unicode::other_case(c as u32) as i32;
                            }
                        }
                        if d != a {
                            done_early = true;
                            break 'scan;
                        }
                        b = c;
                    } else {
                        done_early = true;
                        break 'scan;
                    }
                }
            }
            if !done_early && a >= 0 {
                if prog.flags & FLAG_LASTSET != 0
                    && (prog.last_codeunit == a as u32
                        || (b >= 0 && prog.last_codeunit == b as u32))
                {
                    prog.flags &= !(FLAG_LASTSET | FLAG_LASTCASELESS);
                    prog.last_codeunit = 0;
                }
                prog.first_codeunit = a as u32;
                flags = FLAG_FIRSTSET;
                if b >= 0 {
                    flags |= FLAG_FIRSTCASELESS;
                }
            }
            prog.flags |= flags;
        }
    }

    if prog.flags & (FLAG_MATCH_EMPTY | FLAG_HASACCEPT) == 0 && prog.top_backref <= 128 {
        let dupcapused = prog.flags & FLAG_DUPCAPUSED != 0;
        let mut ml = MinLen {
            prog,
            dupcapused,
            count: 0,
            backref_cache: vec![None; 130],
        };
        let min = ml.find_minlength(0, &[]);
        if min >= 0 {
            prog.minlength = min.min(u16::MAX as i32) as u32;
        }
    }
}

struct MinLen<'a> {
    prog: &'a Program,
    dupcapused: bool,
    count: u32,
    backref_cache: Vec<Option<i32>>,
}

impl MinLen<'_> {
    fn find_bracket(&self, from: usize, recno: u16) -> Option<usize> {
        let ops = &self.prog.ops;
        (from..ops.len()).find(|&i| matches!(ops[i], Op::Bra { kind, group, .. } if kind.is_capture() && group == recno))
    }

    fn ref_minlength(
        &mut self,
        cc: usize,
        recno: u16,
        recurses: &[usize],
        had_recurse: &mut bool,
    ) -> i32 {
        if let Some(Some(d)) = self.backref_cache.get(recno as usize) {
            return *d;
        }
        let mut d = 0;
        if self.prog.overall_options & PCRE2_MATCH_UNSET_BACKREF == 0 {
            let Some(cs) = self.find_bracket(0, recno) else {
                return -2;
            };
            let ce = bracket_ket(&self.prog.ops, cs);
            if !self.dupcapused || self.find_bracket(ce, recno).is_none() {
                if (cc > cs && cc < ce) || recurses.contains(&cs) {
                    *had_recurse = true;
                } else {
                    let mut r = recurses.to_vec();
                    r.push(cs);
                    d = self.find_minlength(cs, &r);
                    if d < 0 {
                        return d;
                    }
                }
            }
        }
        if (recno as usize) < self.backref_cache.len() {
            self.backref_cache[recno as usize] = Some(d);
        }
        d
    }

    fn find_minlength(&mut self, code: usize, recurses: &[usize]) -> i32 {
        let ops = &self.prog.ops;
        let utf = self.prog.utf;
        let mut length: i32 = -1;
        let mut branchlength: i32 = 0;
        let mut prev_cap_recno: i32 = -1;
        let mut prev_cap_d: i32 = 0;
        let mut prev_recurse_recno: i32 = -1;
        let mut prev_recurse_d: i32 = 0;
        let mut once_fudge = 0usize;
        let mut had_recurse = false;

        if let Op::Bra { kind, .. } = ops[code]
            && kind >= BraKind::SBra
        {
            return 0;
        }
        let mut nextbranch = code + next_of(&ops[code]);
        let mut cc = code + 1;
        self.count += 1;
        if self.count > 1001 {
            return -1;
        }

        loop {
            if branchlength >= u16::MAX as i32 {
                branchlength = u16::MAX as i32;
                cc = nextbranch;
            }
            let op = ops[cc].clone();
            match &op {
                Op::Bra { kind, group, next } => match kind {
                    BraKind::Cond | BraKind::SCond => {
                        let cs = cc + next;
                        if !matches!(ops[cs], Op::Alt(_)) {
                            cc = cs + 1;
                        } else {
                            let d = self.find_minlength(cc, recurses);
                            if d < 0 {
                                return d;
                            }
                            branchlength += d;
                            cc = skip_bracket(ops, cc);
                        }
                    }
                    BraKind::Bra
                        if matches!(ops[cc + 1], Op::Recurse { .. })
                            && matches!(ops[cc + 2], Op::Ket(..)) =>
                    {
                        once_fudge = 1;
                        cc += 1;
                    }
                    BraKind::Bra
                    | BraKind::Once
                    | BraKind::ScriptRun
                    | BraKind::SBra
                    | BraKind::BraPos
                    | BraKind::SBraPos => {
                        let d = self.find_minlength(cc, recurses);
                        if d < 0 {
                            return d;
                        }
                        branchlength += d;
                        cc = skip_bracket(ops, cc);
                    }
                    BraKind::CBra | BraKind::SCBra | BraKind::CBraPos | BraKind::SCBraPos => {
                        let recno = *group as i32;
                        if self.dupcapused || recno != prev_cap_recno {
                            prev_cap_recno = recno;
                            prev_cap_d = self.find_minlength(cc, recurses);
                            if prev_cap_d < 0 {
                                return prev_cap_d;
                            }
                        }
                        branchlength += prev_cap_d;
                        cc = skip_bracket(ops, cc);
                    }
                    _ => {
                        // Assertions are skipped.
                        cc = skip_bracket(ops, cc);
                    }
                },
                Op::Accept | Op::AssertAccept => return -1,
                Op::Alt(next) => {
                    if length < 0 || (!had_recurse && branchlength < length) {
                        length = branchlength;
                    }
                    if length == 0 {
                        return length;
                    }
                    nextbranch = cc + next;
                    cc += 1;
                    branchlength = 0;
                    had_recurse = false;
                }
                Op::Ket(..) | Op::End => {
                    if length < 0 || (!had_recurse && branchlength < length) {
                        length = branchlength;
                    }
                    return length;
                }
                Op::Reverse(_)
                | Op::VReverse(..)
                | Op::CRef(_)
                | Op::DnCRef(..)
                | Op::RRef(_)
                | Op::DnRRef(..)
                | Op::False
                | Op::True
                | Op::Callout
                | Op::Sod
                | Op::Som
                | Op::Eod
                | Op::Eodn
                | Op::Circ
                | Op::CircM
                | Op::Doll
                | Op::DollM
                | Op::NotWordBoundary
                | Op::WordBoundary
                | Op::NotUcpWordBoundary
                | Op::UcpWordBoundary => cc += 1,
                Op::BraZero | Op::BraMinZero | Op::BraPosZero | Op::SkipZero => {
                    cc = skip_bracket(ops, cc + 1);
                }
                Op::Item(item) => {
                    if utf && matches!(item, Item::AnyByte) {
                        return -1;
                    }
                    branchlength += 1;
                    cc += 1;
                }
                Op::Rep { min, .. } => {
                    branchlength = branchlength.saturating_add((*min).min(i32::MAX as u32) as i32);
                    cc += 1;
                }
                Op::DnRef {
                    index, count, rep, ..
                } => {
                    let mut d: i32;
                    if !self.dupcapused
                        && self.prog.overall_options & PCRE2_MATCH_UNSET_BACKREF == 0
                    {
                        d = i32::MAX;
                        for k in 0..*count {
                            let recno = self.prog.name_table[(*index + k) as usize].0;
                            let dd = self.ref_minlength(cc, recno, recurses, &mut had_recurse);
                            if dd < 0 {
                                return dd;
                            }
                            if dd < d {
                                d = dd;
                            }
                            if d <= 0 {
                                break;
                            }
                        }
                    } else {
                        d = 0;
                    }
                    cc += 1;
                    let min = rep.map_or(1, |r| r.0) as i32;
                    if (d > 0 && i32::MAX / d < min) || (u16::MAX as i32) - branchlength < min * d {
                        branchlength = u16::MAX as i32;
                    } else {
                        branchlength += min * d;
                    }
                }
                Op::Ref { group, rep, .. } => {
                    let d = self.ref_minlength(cc, *group, recurses, &mut had_recurse);
                    if d < 0 {
                        return d;
                    }
                    cc += 1;
                    let min = rep.map_or(1, |r| r.0) as i32;
                    if (d > 0 && i32::MAX / d < min)
                        || (u16::MAX as i32) - branchlength < min.saturating_mul(d)
                    {
                        branchlength = u16::MAX as i32;
                    } else {
                        branchlength += min * d;
                    }
                }
                Op::Recurse { group, target } => {
                    let cs = *target;
                    let recno = *group as i32;
                    if recno == prev_recurse_recno {
                        branchlength += prev_recurse_d;
                    } else {
                        let ce = bracket_ket(ops, cs);
                        if (cc > cs && cc < ce) || recurses.contains(&cs) {
                            had_recurse = true;
                        } else {
                            let mut r = recurses.to_vec();
                            r.push(cs);
                            prev_recurse_d = self.find_minlength(cs, &r);
                            if prev_recurse_d < 0 {
                                return prev_recurse_d;
                            }
                            prev_recurse_recno = recno;
                            branchlength += prev_recurse_d;
                        }
                    }
                    cc += 1 + once_fudge;
                    once_fudge = 0;
                }
                Op::Mark(_)
                | Op::CommitArg(_)
                | Op::PruneArg(_)
                | Op::SkipArg(_)
                | Op::ThenArg(_)
                | Op::Close(_)
                | Op::Commit
                | Op::Fail
                | Op::Prune
                | Op::SetSom
                | Op::Skip
                | Op::Then => cc += 1,
            }
        }
    }
}
