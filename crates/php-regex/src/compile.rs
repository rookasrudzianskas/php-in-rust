//! Second compile pass: a port of PCRE2's `compile_regex()` /
//! `compile_branch()` turning the parsed pattern into opcodes, plus the
//! driver (`pcre2_compile()`) that handles leading (*XXX) settings, UTF
//! validation, the lookbehind check, recursion resolution and the
//! start-of-match optimisations.

use crate::chars::{self, CBIT_DIGIT, CBIT_SPACE, CBIT_WORD, encode_utf8, utf8_len};
use crate::error::CompileError;
use crate::error::codes::*;
use crate::lookbehind::{LOOKBEHIND_MAX, LookbehindChecker};
use crate::ops::*;
use crate::opts::*;
use crate::parse::{
    CondNameKind, Esc, LbKind, MAX_GROUP_NUMBER, Meta, NamedGroup, Newline, PC_GRAPH, PC_PRINT,
    PC_PUNCT, Parser, QuantMode, REPEAT_UNLIMITED, VerbKind,
};
use crate::unicode;

/// Maximum compiled pattern size (PCRE2 with LINK_SIZE=2).
const MAX_PATTERN_SIZE: usize = 1 << 16;
const RREF_ANY: u16 = 0xffff;

type CResult<T> = Result<T, (i32, usize)>;

/// The compiled program and everything the matcher needs.
#[derive(Clone, Debug)]
pub(crate) struct Program {
    pub ops: Vec<Op>,
    pub marks: Vec<Vec<u8>>,
    pub utf: bool,
    pub ucp: bool,
    pub top_bracket: u32,
    pub top_backref: u32,
    /// Name table sorted as PCRE2 sorts it: (group number, name).
    pub name_table: Vec<(u16, Vec<u8>)>,
    pub overall_options: u32,
    pub flags: u32,
    pub first_codeunit: u32,
    pub last_codeunit: u32,
    pub start_bitmap: [u8; 32],
    pub minlength: u32,
    pub max_lookbehind: u32,
    pub newline: Newline,
    pub bsr_anycrlf: bool,
    pub limit_match: u32,
    pub limit_depth: u32,
    pub limit_heap: u32,
}

/// First/required code unit information.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cu {
    pub firstcu: u32,
    pub firstcuflags: u32,
    pub reqcu: u32,
    pub reqcuflags: u32,
}

impl Cu {
    fn unset() -> Self {
        Cu {
            firstcu: 0,
            firstcuflags: REQ_UNSET,
            reqcu: 0,
            reqcuflags: REQ_UNSET,
        }
    }
}

/// Per-branch state of compile_branch().
struct Br {
    options: u32,
    xoptions: u32,
    firstcu: u32,
    firstcuflags: u32,
    reqcu: u32,
    reqcuflags: u32,
    zerofirstcu: u32,
    zerofirstcuflags: u32,
    zeroreqcu: u32,
    zeroreqcuflags: u32,
    req_caseopt: u32,
    previous: usize,
    groupsetfirstcu: bool,
    had_accept: bool,
    matched_char: bool,
    previous_matched_char: bool,
    reset_caseful: bool,
    okreturn: i32,
    group_return: i32,
    offset: usize,
}

/// State for building one character class (classbits + XCLASS data).
struct ClassBuilder {
    map: [u8; 32],
    xitems: Vec<XItem>,
    class_has_8bitchar: i32,
    xclass_has_prop: bool,
    range_start: u32,
    range_end: u32,
}

impl ClassBuilder {
    fn new() -> Self {
        ClassBuilder {
            map: [0; 32],
            xitems: Vec::new(),
            class_has_8bitchar: 0,
            xclass_has_prop: false,
            range_start: 0,
            range_end: 0,
        }
    }

    #[inline]
    fn setbit(&mut self, c: u32) {
        self.map[(c / 8) as usize] |= 1 << (c % 8);
    }

    /// `add_to_class()`: external entry point.
    fn add_to_class(&mut self, options: u32, xoptions: u32, start: u32, end: u32) -> i32 {
        self.range_start = start;
        self.range_end = end;
        self.add_to_class_internal(options, xoptions, start, end)
    }

    fn add_to_class_internal(
        &mut self,
        mut options: u32,
        xoptions: u32,
        mut start: u32,
        mut end: u32,
    ) -> i32 {
        let mut classbits_end = end.min(0xff);
        let mut n8 = 0i32;

        if options & PCRE2_CASELESS != 0 {
            if options & (PCRE2_UTF | PCRE2_UCP) != 0 {
                options &= !PCRE2_CASELESS;
                let restricted = xoptions & PCRE2_EXTRA_CASELESS_RESTRICT != 0;
                let mut c = start;
                loop {
                    match get_othercase_range(&mut c, end, restricted) {
                        OtherCase::Done => break,
                        OtherCase::Set(set, oc) => {
                            n8 += self.add_list_to_class_internal(options, xoptions, set, oc);
                        }
                        OtherCase::Range(oc, od) => {
                            if oc >= self.range_start && od <= self.range_end {
                                continue;
                            } else if oc < start && od + 1 >= start {
                                start = oc;
                            } else if od > end && oc <= end + 1 {
                                end = od;
                                if end > classbits_end {
                                    classbits_end = end.min(0xff);
                                }
                            } else {
                                n8 += self.add_to_class_internal(options, xoptions, oc, od);
                            }
                        }
                    }
                }
            } else {
                let mut c = start;
                while c <= classbits_end {
                    self.setbit(chars::fcc(c));
                    n8 += 1;
                    c += 1;
                }
            }
        }

        if options & PCRE2_UTF == 0 && end > 0xff {
            end = 0xff;
        }
        if start > self.range_start && end < self.range_end {
            return n8;
        }
        let mut c = start;
        while c <= classbits_end {
            self.setbit(c);
            n8 += 1;
            c += 1;
        }
        if start <= 0xff {
            start = 0x100;
        }
        if end >= start && options & PCRE2_UTF != 0 {
            if start < end {
                self.xitems.push(XItem::Range(start, end));
            } else {
                self.xitems.push(XItem::Single(start));
            }
        }
        n8
    }

    fn add_list_to_class_internal(
        &mut self,
        options: u32,
        xoptions: u32,
        p: &[u32],
        except: u32,
    ) -> i32 {
        let mut n8 = 0;
        let mut i = 0;
        while i < p.len() {
            let mut n = 0;
            if p[i] != except {
                while i + n + 1 < p.len() && p[i + n + 1] == p[i] + n as u32 + 1 {
                    n += 1;
                }
                n8 += self.add_to_class_internal(options, xoptions, p[i], p[i + n]);
            }
            i += n + 1;
        }
        n8
    }

    /// `add_list_to_class()` (external entry).
    fn add_list_to_class(&mut self, options: u32, xoptions: u32, p: &[u32]) -> i32 {
        let mut n8 = 0;
        let mut i = 0;
        while i < p.len() {
            let mut n = 0;
            while i + n + 1 < p.len() && p[i + n + 1] == p[i] + n as u32 + 1 {
                n += 1;
            }
            n8 += self.add_to_class(options, xoptions, p[i], p[i + n]);
            i += n + 1;
        }
        n8
    }

    /// `add_not_list_to_class()`.
    fn add_not_list_to_class(&mut self, options: u32, xoptions: u32, p: &[u32]) -> i32 {
        let utf = options & PCRE2_UTF != 0;
        let mut n8 = 0;
        if p[0] > 0 {
            n8 += self.add_to_class(options, xoptions, 0, p[0] - 1);
        }
        let mut i = 0;
        while i < p.len() {
            while i + 1 < p.len() && p[i + 1] == p[i] + 1 {
                i += 1;
            }
            let hi = if i + 1 >= p.len() {
                if utf { 0x10ffff } else { 0xffff_ffff }
            } else {
                p[i + 1] - 1
            };
            n8 += self.add_to_class(options, xoptions, p[i] + 1, hi);
            i += 1;
        }
        n8
    }
}

enum OtherCase {
    Done,
    Set(&'static [u32], u32),
    Range(u32, u32),
}

/// `get_othercase_range()`.
fn get_othercase_range(cptr: &mut u32, d: u32, restricted: bool) -> OtherCase {
    let mut c = *cptr;
    let mut othercase = 0;
    while c <= d {
        if let Some(set) = unicode::caseless_set(c)
            && (!restricted || set[0] > 127)
        {
            let oc = c;
            *cptr = c + 1;
            return OtherCase::Set(set, oc);
        }
        othercase = unicode::other_case(c);
        if othercase != c {
            break;
        }
        if c == u32::MAX {
            break;
        }
        c += 1;
    }
    if c > d {
        return OtherCase::Done;
    }
    let oc = othercase;
    let mut next = othercase + 1;
    c += 1;
    while c <= d {
        if unicode::caseless_set(c).is_some() || unicode::other_case(c) != next {
            break;
        }
        next += 1;
        c += 1;
    }
    *cptr = c;
    OtherCase::Range(oc, next - 1)
}

/// Posix class base maps: (base, second, op) as in posix_class_maps.
const POSIX_CLASS_MAPS: [(usize, i32, i32); 14] = [
    (CBIT_WORD, chars::CBIT_DIGIT as i32, -2),        // alpha
    (chars::CBIT_LOWER, -1, 0),                       // lower
    (chars::CBIT_UPPER, -1, 0),                       // upper
    (CBIT_WORD, -1, 2),                               // alnum
    (chars::CBIT_PRINT, chars::CBIT_CNTRL as i32, 0), // ascii
    (CBIT_SPACE, -1, 1),                              // blank
    (chars::CBIT_CNTRL, -1, 0),                       // cntrl
    (CBIT_DIGIT, -1, 0),                              // digit
    (chars::CBIT_GRAPH, -1, 0),                       // graph
    (chars::CBIT_PRINT, -1, 0),                       // print
    (chars::CBIT_PUNCT, -1, 0),                       // punct
    (CBIT_SPACE, -1, 0),                              // space
    (CBIT_WORD, -1, 0),                               // word
    (chars::CBIT_XDIGIT, -1, 0),                      // xdigit
];

fn cbits(off: usize) -> &'static [u8] {
    &chars::DEFAULT_TABLES.cbits[off..off + 32]
}

/// The "other case" used for OP_CHARI / OP_NOTI.
pub(crate) fn othercase_for(c: u32, utf: bool, ucp: bool) -> u32 {
    if c < 128 || !(utf || ucp) {
        chars::fcc(c)
    } else {
        unicode::other_case(c)
    }
}

pub(crate) struct Compiler<'a> {
    pattern: &'a [u8],
    parsed: Vec<Meta>,
    utf: bool,
    ucp: bool,
    pub code: Vec<Op>,
    length: usize,
    pub marks: Vec<Vec<u8>>,
    bracount: u32,
    named_groups: Vec<NamedGroup>,
    pub name_table: Vec<(u16, Vec<u8>)>,
    small_ref_offset: [usize; 10],
    assert_depth: u32,
    pub had_accept: bool,
    pub had_pruneorskip: bool,
    pub had_recurse: bool,
    pub backref_map: u32,
    pub top_backref: u32,
    req_varyopt: u32,
    pub max_lookbehind: u32,
    pub external_flags: u32,
    open_caps: Vec<(u32, u32)>,
}

impl<'a> Compiler<'a> {
    fn emit(&mut self, op: Op) -> CResult<()> {
        self.length += op.pcre2_len(self.utf, &self.marks);
        self.code.push(op);
        self.check_length()
    }

    fn insert(&mut self, at: usize, op: Op) -> CResult<()> {
        self.length += op.pcre2_len(self.utf, &self.marks);
        self.code.insert(at, op);
        self.check_length()
    }

    fn check_length(&self) -> CResult<()> {
        if self.length > MAX_PATTERN_SIZE {
            Err((PATTERN_TOO_LARGE, self.pattern.len()))
        } else {
            Ok(())
        }
    }

    fn frag_len(&self, from: usize, to: usize) -> usize {
        self.code[from..to]
            .iter()
            .map(|o| o.pcre2_len(self.utf, &self.marks))
            .sum()
    }

    /// Copy `code[from..from+len]` to the end, `times` times.
    fn replicate(&mut self, from: usize, len: usize, times: u32) -> CResult<()> {
        if times == 0 {
            return Ok(());
        }
        let flen = self.frag_len(from, from + len);
        let delta = flen.saturating_mul(times as usize);
        self.length = self.length.saturating_add(delta);
        self.check_length()?;
        for _ in 0..times {
            self.code.extend_from_within(from..from + len);
        }
        Ok(())
    }

    fn add_mark(&mut self, name: &[u32]) -> u16 {
        let mut v = Vec::new();
        for &c in name {
            if self.utf {
                encode_utf8(c, &mut v);
            } else {
                v.push(c as u8);
            }
        }
        self.marks.push(v);
        (self.marks.len() - 1) as u16
    }

    /// `find_dupname_details()`: index and count in the sorted name table.
    fn find_dupname_details(&mut self, name: &[u8], offset: usize) -> CResult<(u16, u16)> {
        let Some(index) = self.name_table.iter().position(|(_, n)| n == name) else {
            return Err((INTERNAL_MISSING_SUBPATTERN, offset));
        };
        let mut count = 0;
        let mut i = index;
        while i < self.name_table.len() && self.name_table[i].1 == name {
            let g = self.name_table[i].0 as u32;
            self.backref_map |= if g < 32 { 1 << g } else { 1 };
            if g > self.top_backref {
                self.top_backref = g;
            }
            count += 1;
            i += 1;
        }
        Ok((index as u16, count))
    }

    // ------------------------------------------------------------------
    // compile_regex()
    // ------------------------------------------------------------------

    /// Compile a group whose bracket opcode is at `start` (already emitted,
    /// together with any condition opcode). `pptr` points at the first item
    /// of the first branch; on return it points at the closing Ket or End.
    fn compile_regex(
        &mut self,
        options_in: u32,
        xoptions_in: u32,
        pptr: &mut usize,
        start: usize,
    ) -> CResult<(i32, Cu)> {
        let mut options = options_in;
        let mut xoptions = xoptions_in;
        let (is_lb, capnumber) = match self.code[start] {
            Op::Bra { kind, group, .. } => (
                kind.is_lookbehind(),
                if kind == BraKind::CBra { group } else { 0 },
            ),
            _ => (false, 0),
        };
        let (mut lblen, lbmin) = if is_lb {
            match self.parsed[*pptr - 1] {
                Meta::Lookbehind { len, minlen, .. } => (len, minlen),
                _ => (0, 0),
            }
        } else {
            (0, 0)
        };
        if capnumber != 0 {
            self.open_caps.push((capnumber as u32, self.assert_depth));
        }
        // Bracket overhead (BRA + KET) for the size limit.
        let mut cu = Cu::unset();
        let mut okreturn = 1;
        let mut links: Vec<usize> = vec![start];
        let mut first = true;

        loop {
            if is_lb && lblen > 0 {
                if lbmin == LOOKBEHIND_MAX || lbmin == lblen {
                    self.emit(Op::Reverse(lblen))?;
                } else {
                    self.emit(Op::VReverse(lbmin, lblen))?;
                }
            }
            let (branch_return, b) = self.compile_branch(&mut options, &mut xoptions, pptr)?;
            if branch_return < 0 {
                okreturn = -1;
            }
            if first {
                cu = b;
            } else {
                let mut bcu = b;
                if cu.firstcuflags != bcu.firstcuflags || cu.firstcu != bcu.firstcu {
                    if cu.firstcuflags < REQ_NONE && cu.reqcuflags >= REQ_NONE {
                        cu.reqcu = cu.firstcu;
                        cu.reqcuflags = cu.firstcuflags;
                    }
                    cu.firstcuflags = REQ_NONE;
                }
                if cu.firstcuflags >= REQ_NONE
                    && bcu.firstcuflags < REQ_NONE
                    && bcu.reqcuflags >= REQ_NONE
                {
                    bcu.reqcu = bcu.firstcu;
                    bcu.reqcuflags = bcu.firstcuflags;
                }
                if (cu.reqcuflags & !REQ_VARY) != (bcu.reqcuflags & !REQ_VARY)
                    || cu.reqcu != bcu.reqcu
                {
                    cu.reqcuflags = REQ_NONE;
                } else {
                    cu.reqcu = bcu.reqcu;
                    cu.reqcuflags |= bcu.reqcuflags;
                }
            }
            first = false;

            if !matches!(self.parsed[*pptr], Meta::Alt(_)) {
                let ket = self.code.len();
                links.push(ket);
                for w in links.windows(2) {
                    let (from, to) = (w[0], w[1]);
                    match &mut self.code[from] {
                        Op::Bra { next, .. } => *next = to - from,
                        Op::Alt(next) => *next = to - from,
                        _ => {}
                    }
                }
                self.emit(Op::Ket(KetKind::Ket, ket - start))?;
                if capnumber != 0 {
                    self.open_caps.pop();
                }
                return Ok((okreturn, cu));
            }

            lblen = match self.parsed[*pptr] {
                Meta::Alt(l) => l,
                _ => 0,
            };
            links.push(self.code.len());
            self.emit(Op::Alt(0))?;
            *pptr += 1;
        }
    }

    // ------------------------------------------------------------------
    // compile_branch()
    // ------------------------------------------------------------------

    fn compile_branch(
        &mut self,
        optionsp: &mut u32,
        xoptionsp: &mut u32,
        pptr: &mut usize,
    ) -> CResult<(i32, Cu)> {
        let options = *optionsp;
        let mut b = Br {
            options,
            xoptions: *xoptionsp,
            firstcu: 0,
            firstcuflags: REQ_UNSET,
            reqcu: 0,
            reqcuflags: REQ_UNSET,
            zerofirstcu: 0,
            zerofirstcuflags: REQ_UNSET,
            zeroreqcu: 0,
            zeroreqcuflags: REQ_UNSET,
            req_caseopt: if options & PCRE2_CASELESS != 0 {
                REQ_CASELESS
            } else {
                0
            },
            previous: self.code.len(),
            groupsetfirstcu: false,
            had_accept: false,
            matched_char: false,
            previous_matched_char: false,
            reset_caseful: false,
            okreturn: -1,
            group_return: 0,
            offset: 0,
        };

        loop {
            let meta = self.parsed[*pptr].clone();
            if !meta.is_quantifier() {
                b.previous = self.code.len();
                if b.matched_char && !b.had_accept {
                    b.okreturn = 1;
                }
            }
            b.previous_matched_char = b.matched_char;
            b.matched_char = false;

            match meta {
                Meta::End | Meta::Alt(_) | Meta::Ket => {
                    *optionsp = b.options;
                    *xoptionsp = b.xoptions;
                    return Ok((
                        b.okreturn,
                        Cu {
                            firstcu: b.firstcu,
                            firstcuflags: b.firstcuflags,
                            reqcu: b.reqcu,
                            reqcuflags: b.reqcuflags,
                        },
                    ));
                }

                Meta::Circumflex => {
                    if b.options & PCRE2_MULTILINE != 0 {
                        if b.firstcuflags == REQ_UNSET {
                            b.zerofirstcuflags = REQ_NONE;
                            b.firstcuflags = REQ_NONE;
                        }
                        self.emit(Op::CircM)?;
                    } else {
                        self.emit(Op::Circ)?;
                    }
                }
                Meta::Dollar => {
                    self.emit(if b.options & PCRE2_MULTILINE != 0 {
                        Op::DollM
                    } else {
                        Op::Doll
                    })?;
                }
                Meta::Dot => {
                    b.matched_char = true;
                    if b.firstcuflags == REQ_UNSET {
                        b.firstcuflags = REQ_NONE;
                    }
                    b.zerofirstcu = b.firstcu;
                    b.zerofirstcuflags = b.firstcuflags;
                    b.zeroreqcu = b.reqcu;
                    b.zeroreqcuflags = b.reqcuflags;
                    self.emit(Op::Item(if b.options & PCRE2_DOTALL != 0 {
                        Item::AllAny
                    } else {
                        Item::Any
                    }))?;
                }
                Meta::ClassEmpty(neg) => {
                    b.matched_char = true;
                    self.emit(if neg {
                        Op::Item(Item::AllAny)
                    } else {
                        Op::Fail
                    })?;
                    if b.firstcuflags == REQ_UNSET {
                        b.firstcuflags = REQ_NONE;
                    }
                    b.zerofirstcu = b.firstcu;
                    b.zerofirstcuflags = b.firstcuflags;
                }
                Meta::Class(negate_class) => {
                    b.matched_char = true;
                    if let Some(ch) = self.compile_class(&mut b, pptr, negate_class)? {
                        // Single positive char or caseless pair: normal char.
                        self.normal_char(&mut b, ch)?;
                    }
                }

                Meta::Accept => {
                    self.had_accept = true;
                    b.had_accept = true;
                    let depth = self.assert_depth;
                    let closes: Vec<u32> = self
                        .open_caps
                        .iter()
                        .rev()
                        .take_while(|(_, d)| *d >= depth)
                        .map(|(n, _)| *n)
                        .collect();
                    for n in closes {
                        self.emit(Op::Close(n as u16))?;
                    }
                    self.emit(if self.assert_depth > 0 {
                        Op::AssertAccept
                    } else {
                        Op::Accept
                    })?;
                    if b.firstcuflags == REQ_UNSET {
                        b.firstcuflags = REQ_NONE;
                    }
                }
                Meta::Prune => {
                    self.had_pruneorskip = true;
                    self.emit(Op::Prune)?;
                }
                Meta::Skip => {
                    self.had_pruneorskip = true;
                    self.emit(Op::Skip)?;
                }
                Meta::Commit => self.emit(Op::Commit)?,
                Meta::Fail => self.emit(Op::Fail)?,
                Meta::Then => {
                    self.external_flags |= FLAG_HASTHEN;
                    self.emit(Op::Then)?;
                }
                Meta::VerbArg(kind, name) => {
                    let idx = self.add_mark(&name);
                    let op = match kind {
                        VerbKind::Mark => Op::Mark(idx),
                        VerbKind::Commit => Op::CommitArg(idx),
                        VerbKind::Prune => {
                            self.had_pruneorskip = true;
                            Op::PruneArg(idx)
                        }
                        VerbKind::Skip => {
                            self.had_pruneorskip = true;
                            Op::SkipArg(idx)
                        }
                        VerbKind::Then => {
                            self.external_flags |= FLAG_HASTHEN;
                            Op::ThenArg(idx)
                        }
                    };
                    self.emit(op)?;
                }
                Meta::Options { options, xoptions } => {
                    b.options = options;
                    b.xoptions = xoptions;
                    *optionsp = options;
                    *xoptionsp = xoptions;
                    b.req_caseopt = if options & PCRE2_CASELESS != 0 {
                        REQ_CASELESS
                    } else {
                        0
                    };
                }

                Meta::CondName { kind, offset, len } => {
                    b.offset = offset;
                    let name = self.pattern[offset..offset + len].to_vec();
                    let found = self.named_groups.iter().find(|ng| ng.name == name).cloned();
                    let cond_op = match found {
                        Some(ng) if !ng.isdup => {
                            if ng.number > self.top_backref {
                                self.top_backref = ng.number;
                            }
                            if kind == CondNameKind::RName {
                                Op::RRef(ng.number as u16)
                            } else {
                                Op::CRef(ng.number as u16)
                            }
                        }
                        Some(_) => {
                            let (index, count) = self.find_dupname_details(&name, offset)?;
                            if kind == CondNameKind::RName {
                                Op::DnRRef(index, count)
                            } else {
                                Op::DnCRef(index, count)
                            }
                        }
                        None => {
                            let mut groupnumber: u32 = 0;
                            if kind == CondNameKind::RNumber {
                                for (i, &d) in name.iter().enumerate().skip(1) {
                                    groupnumber = groupnumber * 10 + (d - b'0') as u32;
                                    if groupnumber > MAX_GROUP_NUMBER {
                                        return Err((SUBPATTERN_NUMBER_TOO_BIG, offset + i));
                                    }
                                }
                            }
                            if kind != CondNameKind::RNumber || groupnumber > self.bracount {
                                return Err((BAD_SUBPATTERN_REFERENCE, offset));
                            }
                            Op::RRef(if groupnumber == 0 {
                                RREF_ANY
                            } else {
                                groupnumber as u16
                            })
                        }
                    };
                    self.group(&mut b, pptr, BraKind::Cond, 0, Some(cond_op), true, false)?;
                }
                Meta::CondDefine { offset } => {
                    b.offset = offset;
                    self.group(&mut b, pptr, BraKind::Cond, 0, Some(Op::False), false, true)?;
                }
                Meta::CondNumber { n, offset } => {
                    b.offset = offset;
                    if n > self.bracount {
                        return Err((BAD_SUBPATTERN_REFERENCE, offset));
                    }
                    if n > self.top_backref {
                        self.top_backref = n;
                    }
                    b.offset = offset.wrapping_sub(2);
                    self.group(
                        &mut b,
                        pptr,
                        BraKind::Cond,
                        0,
                        Some(Op::CRef(n as u16)),
                        true,
                        false,
                    )?;
                }
                Meta::CondVersion { ge, major, minor } => {
                    let t = if ge {
                        10 > major || (10 == major && 44 >= minor)
                    } else {
                        10 == major && 44 == minor
                    };
                    self.group(
                        &mut b,
                        pptr,
                        BraKind::Cond,
                        0,
                        Some(if t { Op::True } else { Op::False }),
                        true,
                        false,
                    )?;
                }
                Meta::CondAssert => {
                    self.group(&mut b, pptr, BraKind::Cond, 0, None, true, false)?;
                }
                Meta::Lookahead => {
                    self.assert_depth += 1;
                    self.group(&mut b, pptr, BraKind::Assert, 0, None, false, false)?;
                }
                Meta::LookaheadNa => {
                    self.assert_depth += 1;
                    self.group(&mut b, pptr, BraKind::AssertNa, 0, None, false, false)?;
                }
                Meta::LookaheadNot => {
                    if matches!(self.parsed[*pptr + 1], Meta::Ket)
                        && !self.parsed[*pptr + 2].is_quantifier()
                    {
                        self.emit(Op::Fail)?;
                        *pptr += 1;
                    } else {
                        self.assert_depth += 1;
                        self.group(&mut b, pptr, BraKind::AssertNot, 0, None, false, false)?;
                    }
                }
                Meta::Lookbehind { kind, .. } => {
                    self.assert_depth += 1;
                    let bk = match kind {
                        LbKind::Pos => BraKind::AssertBack,
                        LbKind::Neg => BraKind::AssertBackNot,
                        LbKind::NonAtomic => BraKind::AssertBackNa,
                    };
                    self.group(&mut b, pptr, bk, 0, None, false, false)?;
                }
                Meta::Atomic => self.group(&mut b, pptr, BraKind::Once, 0, None, true, false)?,
                Meta::ScriptRun => {
                    self.group(&mut b, pptr, BraKind::ScriptRun, 0, None, true, false)?
                }
                Meta::NoCapture => self.group(&mut b, pptr, BraKind::Bra, 0, None, true, false)?,
                Meta::Capture(n) => {
                    self.group(&mut b, pptr, BraKind::CBra, n as u16, None, true, false)?
                }

                Meta::BackrefByName { offset, len } | Meta::RecurseByName { offset, len } => {
                    let is_recurse = matches!(meta, Meta::RecurseByName { .. });
                    b.offset = offset;
                    let name = self.pattern[offset..offset + len].to_vec();
                    let mut groupnumber = 0u32;
                    let mut is_dupname = false;
                    let mut recursed = false;
                    for ng in self.named_groups.clone().iter() {
                        if ng.name == name {
                            is_dupname = ng.isdup;
                            groupnumber = ng.number;
                            if is_recurse {
                                self.numerical_recursion(&mut b, groupnumber)?;
                                recursed = true;
                                break;
                            }
                            self.backref_map |= if groupnumber < 32 {
                                1 << groupnumber
                            } else {
                                1
                            };
                            if groupnumber > self.top_backref {
                                self.top_backref = groupnumber;
                            }
                        }
                    }
                    if !recursed {
                        if groupnumber == 0 {
                            return Err((BAD_SUBPATTERN_REFERENCE, offset));
                        }
                        if !is_dupname {
                            self.single_reference(&mut b, groupnumber)?;
                        } else {
                            let (index, count) = self.find_dupname_details(&name, offset)?;
                            if b.firstcuflags == REQ_UNSET {
                                b.firstcuflags = REQ_NONE;
                            }
                            self.emit(Op::DnRef {
                                index,
                                count,
                                caseless: b.options & PCRE2_CASELESS != 0,
                                rep: None,
                            })?;
                        }
                    }
                }

                Meta::Callout => self.emit(Op::Callout)?,

                Meta::Asterisk(m) => self.compile_repeat(&mut b, 0, REPEAT_UNLIMITED, m)?,
                Meta::Plus(m) => self.compile_repeat(&mut b, 1, REPEAT_UNLIMITED, m)?,
                Meta::Query(m) => self.compile_repeat(&mut b, 0, 1, m)?,
                Meta::MinMax(min, max, m) => self.compile_repeat(&mut b, min, max, m)?,

                Meta::Backref { n, offset } => {
                    let off = if n < 10 {
                        self.small_ref_offset[n as usize]
                    } else {
                        offset
                    };
                    b.offset = off;
                    if n > self.bracount {
                        return Err((BAD_SUBPATTERN_REFERENCE, off));
                    }
                    self.single_reference(&mut b, n)?;
                }
                Meta::Recurse { n, offset } => {
                    b.offset = offset;
                    if n > self.bracount {
                        return Err((BAD_SUBPATTERN_REFERENCE, offset));
                    }
                    self.numerical_recursion(&mut b, n)?;
                }

                Meta::Escape(esc) => {
                    if esc.consumes_char() {
                        b.matched_char = true;
                        if b.firstcuflags == REQ_UNSET {
                            b.firstcuflags = REQ_NONE;
                        }
                    }
                    b.zerofirstcu = b.firstcu;
                    b.zerofirstcuflags = b.firstcuflags;
                    b.zeroreqcu = b.reqcu;
                    b.zeroreqcuflags = b.reqcuflags;
                    if esc == Esc::K
                        && self.assert_depth > 0
                        && b.xoptions & PCRE2_EXTRA_ALLOW_LOOKAROUND_BSK == 0
                    {
                        return Err((BACKSLASH_K_IN_LOOKAROUND, self.pattern.len()));
                    }
                    let ucp_bsw =
                        b.options & PCRE2_UCP != 0 && b.xoptions & PCRE2_EXTRA_ASCII_BSW == 0;
                    let op = match esc {
                        Esc::A => {
                            if self.max_lookbehind == 0 {
                                self.max_lookbehind = 1;
                            }
                            Op::Sod
                        }
                        Esc::G => Op::Som,
                        Esc::K => Op::SetSom,
                        Esc::UpperB | Esc::LowerB => {
                            if self.max_lookbehind == 0 {
                                self.max_lookbehind = 1;
                            }
                            match (esc == Esc::UpperB, ucp_bsw) {
                                (true, true) => Op::NotUcpWordBoundary,
                                (true, false) => Op::NotWordBoundary,
                                (false, true) => Op::UcpWordBoundary,
                                (false, false) => Op::WordBoundary,
                            }
                        }
                        Esc::UpperD => Op::Item(Item::NotDigit),
                        Esc::LowerD => Op::Item(Item::Digit),
                        Esc::UpperS => Op::Item(Item::NotWhitespace),
                        Esc::LowerS => Op::Item(Item::Whitespace),
                        Esc::UpperW => Op::Item(Item::NotWordchar),
                        Esc::LowerW => Op::Item(Item::Wordchar),
                        Esc::N => Op::Item(Item::Any),
                        Esc::C => {
                            self.external_flags |= FLAG_HASBKC;
                            if self.utf {
                                Op::Item(Item::AnyByte)
                            } else {
                                Op::Item(Item::AllAny)
                            }
                        }
                        Esc::R => Op::Item(Item::AnyNl),
                        Esc::UpperH => Op::Item(Item::NotHspace),
                        Esc::LowerH => Op::Item(Item::Hspace),
                        Esc::UpperV => Op::Item(Item::NotVspace),
                        Esc::LowerV => Op::Item(Item::Vspace),
                        Esc::X => Op::Item(Item::Extuni),
                        Esc::UpperZ => Op::Eodn,
                        Esc::LowerZ => Op::Eod,
                        _ => return Err((INTERNAL_BAD_CODE, self.pattern.len())),
                    };
                    self.emit(op)?;
                }
                Meta::Prop {
                    negated,
                    ptype,
                    pvalue,
                } => {
                    b.matched_char = true;
                    if b.firstcuflags == REQ_UNSET {
                        b.firstcuflags = REQ_NONE;
                    }
                    b.zerofirstcu = b.firstcu;
                    b.zerofirstcuflags = b.firstcuflags;
                    b.zeroreqcu = b.reqcu;
                    b.zeroreqcuflags = b.reqcuflags;
                    if !negated && ptype == unicode::PT_ANY {
                        self.emit(Op::Item(Item::AllAny))?;
                    } else if negated {
                        self.emit(Op::Item(Item::NotProp(ptype, pvalue)))?;
                    } else {
                        self.emit(Op::Item(Item::Prop(ptype, pvalue)))?;
                    }
                }

                Meta::Lit(c) => self.normal_char(&mut b, c)?,

                // These can only appear inside classes.
                Meta::ClassEnd | Meta::Posix { .. } | Meta::RangeLiteral | Meta::RangeEscaped => {
                    return Err((INTERNAL_BAD_CODE, self.pattern.len()));
                }
            }
            *pptr += 1;
        }
    }

    /// NORMAL_CHAR handling for a literal character.
    fn normal_char(&mut self, b: &mut Br, c: u32) -> CResult<()> {
        b.matched_char = true;
        let utf = self.utf;
        let ucp = self.ucp;
        if (utf || ucp)
            && b.options & PCRE2_CASELESS != 0
            && let Some(set) = unicode::caseless_set(c)
            && (b.xoptions & PCRE2_EXTRA_CASELESS_RESTRICT == 0 || set[0] > 127)
            && c <= 0xffff
        {
            self.emit(Op::Item(Item::Prop(unicode::PT_CLIST, c as u16)))?;
            if b.firstcuflags == REQ_UNSET {
                b.firstcuflags = REQ_NONE;
                b.zerofirstcuflags = REQ_NONE;
            }
            return Ok(());
        }
        self.class_caseless_char(b, c)
    }

    /// CLASS_CASELESS_CHAR: emit OP_CHAR / OP_CHARI and update first/req cu.
    fn class_caseless_char(&mut self, b: &mut Br, c: u32) -> CResult<()> {
        let utf = self.utf;
        let caseless = b.options & PCRE2_CASELESS != 0;
        let mut buf = Vec::with_capacity(4);
        if utf {
            encode_utf8(c, &mut buf);
        } else {
            buf.push(c as u8);
        }
        let mclength = buf.len();
        self.emit(Op::Item(if caseless {
            Item::CharI(c, othercase_for(c, utf, self.ucp))
        } else {
            Item::Char(c)
        }))?;
        if buf[0] == b'\r' || buf[0] == b'\n' {
            self.external_flags |= FLAG_HASCRORLF;
        }
        let last = *buf.last().unwrap() as u32;
        if b.firstcuflags == REQ_UNSET {
            b.zerofirstcuflags = REQ_NONE;
            b.zeroreqcu = b.reqcu;
            b.zeroreqcuflags = b.reqcuflags;
            if mclength == 1 || b.req_caseopt == 0 {
                b.firstcu = buf[0] as u32;
                b.firstcuflags = b.req_caseopt;
                if mclength != 1 {
                    b.reqcu = last;
                    b.reqcuflags = self.req_varyopt;
                }
            } else {
                b.firstcuflags = REQ_NONE;
                b.reqcuflags = REQ_NONE;
            }
        } else {
            b.zerofirstcu = b.firstcu;
            b.zerofirstcuflags = b.firstcuflags;
            b.zeroreqcu = b.reqcu;
            b.zeroreqcuflags = b.reqcuflags;
            if mclength == 1 || b.req_caseopt == 0 {
                b.reqcu = last;
                b.reqcuflags = b.req_caseopt | self.req_varyopt;
            }
        }
        if b.reset_caseful {
            b.options &= !PCRE2_CASELESS;
            b.req_caseopt = 0;
            b.reset_caseful = false;
        }
        Ok(())
    }

    /// HANDLE_SINGLE_REFERENCE
    fn single_reference(&mut self, b: &mut Br, n: u32) -> CResult<()> {
        if b.firstcuflags == REQ_UNSET {
            b.zerofirstcuflags = REQ_NONE;
            b.firstcuflags = REQ_NONE;
        }
        self.emit(Op::Ref {
            group: n as u16,
            caseless: b.options & PCRE2_CASELESS != 0,
            rep: None,
        })?;
        self.backref_map |= if n < 32 { 1 << n } else { 1 };
        if n > self.top_backref {
            self.top_backref = n;
        }
        Ok(())
    }

    /// HANDLE_NUMERICAL_RECURSION
    fn numerical_recursion(&mut self, b: &mut Br, n: u32) -> CResult<()> {
        self.emit(Op::Recurse {
            group: n as u16,
            target: 0,
        })?;
        b.groupsetfirstcu = false;
        self.had_recurse = true;
        if b.firstcuflags == REQ_UNSET {
            b.firstcuflags = REQ_NONE;
        }
        b.zerofirstcu = b.firstcu;
        b.zerofirstcuflags = b.firstcuflags;
        Ok(())
    }

    /// GROUP_PROCESS / GROUP_PROCESS_NOTE_EMPTY. `pptr` points at the group
    /// meta; on return it points at the group's Ket.
    #[allow(clippy::too_many_arguments)]
    fn group(
        &mut self,
        b: &mut Br,
        pptr: &mut usize,
        bravalue: BraKind,
        capnum: u16,
        cond_op: Option<Op>,
        note_group_empty: bool,
        is_define: bool,
    ) -> CResult<()> {
        let start = self.code.len();
        self.emit(Op::Bra {
            kind: bravalue,
            next: 0,
            group: capnum,
        })?;
        if let Some(op) = cond_op {
            self.emit(op)?;
        }
        *pptr += 1;
        let tempreqvary = self.req_varyopt;
        let (group_return, sub) = self.compile_regex(b.options, b.xoptions, pptr, start)?;
        b.group_return = group_return;
        let mut subfirstcu = sub.firstcu;
        let mut subfirstcuflags = sub.firstcuflags;
        let mut subreqcu = sub.reqcu;
        let mut subreqcuflags = sub.reqcuflags;

        if note_group_empty && bravalue != BraKind::Cond && group_return > 0 {
            b.matched_char = true;
        }
        if bravalue.is_assertion() {
            self.assert_depth -= 1;
        }
        let mut define_group = false;
        if bravalue == BraKind::Cond {
            let mut condcount = 0;
            let mut tc = start;
            loop {
                condcount += 1;
                let nx = match self.code[tc] {
                    Op::Bra { next, .. } | Op::Alt(next) => next,
                    _ => 0,
                };
                tc += nx;
                if matches!(self.code[tc], Op::Ket(..)) {
                    break;
                }
            }
            if is_define {
                if condcount > 1 {
                    return Err((DEFINE_TOO_MANY_BRANCHES, b.offset));
                }
                define_group = true;
            } else {
                if condcount > 2 {
                    return Err((TOO_MANY_CONDITION_BRANCHES, b.offset));
                }
                if condcount == 1 {
                    subfirstcuflags = REQ_NONE;
                    subreqcuflags = REQ_NONE;
                } else if group_return > 0 {
                    b.matched_char = true;
                }
            }
        }
        if define_group {
            return Ok(());
        }

        b.zeroreqcu = b.reqcu;
        b.zeroreqcuflags = b.reqcuflags;
        b.zerofirstcu = b.firstcu;
        b.zerofirstcuflags = b.firstcuflags;
        b.groupsetfirstcu = false;

        if bravalue >= BraKind::Once {
            if b.firstcuflags == REQ_UNSET && subfirstcuflags != REQ_UNSET {
                if subfirstcuflags < REQ_NONE {
                    b.firstcu = subfirstcu;
                    b.firstcuflags = subfirstcuflags;
                    b.groupsetfirstcu = true;
                } else {
                    b.firstcuflags = REQ_NONE;
                }
                b.zerofirstcuflags = REQ_NONE;
            } else if subfirstcuflags < REQ_NONE && subreqcuflags >= REQ_NONE {
                subreqcu = subfirstcu;
                subreqcuflags = subfirstcuflags | tempreqvary;
            }
            if subreqcuflags < REQ_NONE {
                b.reqcu = subreqcu;
                b.reqcuflags = subreqcuflags;
            }
        } else if (bravalue == BraKind::Assert || bravalue == BraKind::AssertNa)
            && subreqcuflags < REQ_NONE
            && subfirstcuflags < REQ_NONE
        {
            b.reqcu = subreqcu;
            b.reqcuflags = subreqcuflags;
        }
        let _ = &mut subfirstcu;
        Ok(())
    }

    /// Compile a character class. `pptr` points at the Class meta; on exit
    /// at the ClassEnd. Returns Some(c) when the class must be compiled as a
    /// normal character (single positive char, or a caseless pair - in which
    /// case caselessness has been temporarily enabled).
    fn compile_class(
        &mut self,
        b: &mut Br,
        pptr: &mut usize,
        negate_class: bool,
    ) -> CResult<Option<u32>> {
        let utf = self.utf;
        let ucp = b.options & PCRE2_UCP != 0;
        let options = b.options;
        let xoptions = b.xoptions;
        let p = *pptr;

        // Single-character class optimisation.
        if let (Meta::Lit(c), Meta::ClassEnd) = (&self.parsed[p + 1], &self.parsed[p + 2]) {
            let c = *c;
            *pptr += 2;
            if !negate_class {
                return Ok(Some(c));
            }
            b.zeroreqcu = b.reqcu;
            b.zeroreqcuflags = b.reqcuflags;
            if b.firstcuflags == REQ_UNSET {
                b.firstcuflags = REQ_NONE;
            }
            b.zerofirstcu = b.firstcu;
            b.zerofirstcuflags = b.firstcuflags;
            if (utf || ucp)
                && options & PCRE2_CASELESS != 0
                && let Some(set) = unicode::caseless_set(c)
                && (xoptions & PCRE2_EXTRA_CASELESS_RESTRICT == 0 || set[0] > 127)
                && c <= 0xffff
            {
                self.emit(Op::Item(Item::NotProp(unicode::PT_CLIST, c as u16)))?;
                return Ok(None);
            }
            self.emit(Op::Item(if options & PCRE2_CASELESS != 0 {
                Item::NotI(c, othercase_for(c, utf, self.ucp))
            } else {
                Item::Not(c)
            }))?;
            return Ok(None);
        }

        // Two characters that are case partners.
        if !negate_class
            && let (Meta::Lit(c), Meta::Lit(c2), Meta::ClassEnd) = (
                &self.parsed[p + 1],
                &self.parsed[p + 2],
                &self.parsed[p + 3],
            )
        {
            let (c, c2) = (*c, *c2);
            let cs = unicode::caseless_set(c);
            if cs.is_none()
                || (xoptions & PCRE2_EXTRA_CASELESS_RESTRICT != 0 && c < 128 && c2 < 128)
            {
                let d = if (utf || ucp) && c > 127 {
                    unicode::other_case(c)
                } else if c > 255 {
                    c
                } else {
                    chars::fcc(c)
                };
                if c != d && c2 == d {
                    *pptr += 3;
                    if options & PCRE2_CASELESS == 0 {
                        b.reset_caseful = true;
                        b.options |= PCRE2_CASELESS;
                        b.req_caseopt = REQ_CASELESS;
                    }
                    // CLASS_CASELESS_CHAR: skip the caseless-set check.
                    b.matched_char = true;
                    self.class_caseless_char(b, c)?;
                    return Ok(None);
                }
            }
        }

        let mut should_flip_negation = false;
        let mut match_all_or_no_wide_chars = false;
        let mut cb = ClassBuilder::new();
        let mut i = p + 1;
        loop {
            let m = self.parsed[i].clone();
            match m {
                Meta::ClassEnd => break,
                Meta::Posix { negated, class } => {
                    let local_negate = negated;
                    let mut posix_class = class;
                    should_flip_negation = local_negate;
                    if options & PCRE2_CASELESS != 0 && posix_class <= 2 {
                        posix_class = 0;
                    }
                    let mut done = false;
                    if options & PCRE2_UCP != 0 && xoptions & PCRE2_EXTRA_ASCII_POSIX == 0 {
                        match posix_class {
                            PC_GRAPH | PC_PRINT | PC_PUNCT => {
                                let pt = match posix_class {
                                    PC_GRAPH => unicode::PT_PXGRAPH,
                                    PC_PRINT => unicode::PT_PXPRINT,
                                    _ => unicode::PT_PXPUNCT,
                                };
                                cb.xitems.push(if local_negate {
                                    XItem::NotProp(pt, 0)
                                } else {
                                    XItem::Prop(pt, 0)
                                });
                                cb.xclass_has_prop = true;
                                done = true;
                            }
                            _ => {
                                if utf {
                                    match_all_or_no_wide_chars |= local_negate;
                                }
                            }
                        }
                    }
                    if !done {
                        let (base, second, tabopt) = POSIX_CLASS_MAPS[posix_class as usize];
                        let mut pbits = [0u8; 32];
                        pbits.copy_from_slice(cbits(base));
                        if second >= 0 {
                            let s = cbits(second as usize);
                            if tabopt >= 0 {
                                for k in 0..32 {
                                    pbits[k] |= s[k];
                                }
                            } else {
                                for k in 0..32 {
                                    pbits[k] &= !s[k];
                                }
                            }
                        }
                        let t = tabopt.abs();
                        if t == 1 {
                            pbits[1] &= !0x3c;
                        } else if t == 2 {
                            pbits[11] &= 0x7f;
                        }
                        for (m, &p) in cb.map.iter_mut().zip(pbits.iter()) {
                            *m |= if local_negate { !p } else { p };
                        }
                        cb.class_has_8bitchar = 1;
                    }
                }
                Meta::Escape(esc) => {
                    cb.class_has_8bitchar += 1;
                    let noc = options & !PCRE2_CASELESS;
                    match esc {
                        Esc::LowerD => or_bits(&mut cb.map, cbits(CBIT_DIGIT), false),
                        Esc::UpperD => {
                            should_flip_negation = true;
                            or_bits(&mut cb.map, cbits(CBIT_DIGIT), true);
                        }
                        Esc::LowerW => or_bits(&mut cb.map, cbits(CBIT_WORD), false),
                        Esc::UpperW => {
                            should_flip_negation = true;
                            or_bits(&mut cb.map, cbits(CBIT_WORD), true);
                        }
                        Esc::LowerS => or_bits(&mut cb.map, cbits(CBIT_SPACE), false),
                        Esc::UpperS => {
                            should_flip_negation = true;
                            or_bits(&mut cb.map, cbits(CBIT_SPACE), true);
                        }
                        Esc::LowerH => {
                            cb.add_list_to_class(noc, xoptions, &chars::HSPACE_LIST);
                        }
                        Esc::UpperH => {
                            cb.add_not_list_to_class(noc, xoptions, &chars::HSPACE_LIST);
                        }
                        Esc::LowerV => {
                            cb.add_list_to_class(noc, xoptions, &chars::VSPACE_LIST);
                        }
                        Esc::UpperV => {
                            cb.add_not_list_to_class(noc, xoptions, &chars::VSPACE_LIST);
                        }
                        _ => {}
                    }
                }
                Meta::Prop {
                    negated,
                    ptype,
                    pvalue,
                } => {
                    cb.class_has_8bitchar += 1;
                    cb.xitems.push(if negated {
                        XItem::NotProp(ptype, pvalue)
                    } else {
                        XItem::Prop(ptype, pvalue)
                    });
                    cb.xclass_has_prop = true;
                    cb.class_has_8bitchar -= 1;
                }
                Meta::Lit(c) => {
                    let mut d = c;
                    if c == b'\r' as u32 || c == b'\n' as u32 {
                        self.external_flags |= FLAG_HASCRORLF;
                    }
                    if matches!(self.parsed[i + 1], Meta::RangeLiteral | Meta::RangeEscaped) {
                        i += 2;
                        if let Meta::Lit(x) = self.parsed[i] {
                            d = x;
                        }
                        if d == b'\r' as u32 || d == b'\n' as u32 {
                            self.external_flags |= FLAG_HASCRORLF;
                        }
                    }
                    cb.class_has_8bitchar += cb.add_to_class(options, xoptions, c, d);
                }
                _ => {}
            }
            i += 1;
        }
        *pptr = i;

        if b.firstcuflags == REQ_UNSET {
            b.firstcuflags = REQ_NONE;
        }
        b.zerofirstcu = b.firstcu;
        b.zerofirstcuflags = b.firstcuflags;
        b.zeroreqcu = b.reqcu;
        b.zeroreqcuflags = b.reqcuflags;

        let xclass = !cb.xitems.is_empty();
        let class = if xclass
            && (options & PCRE2_UCP != 0 || cb.xclass_has_prop || !should_flip_negation)
        {
            if match_all_or_no_wide_chars
                || (utf && should_flip_negation && !negate_class && options & PCRE2_UCP == 0)
            {
                cb.xitems.push(XItem::Range(0x100, 0x10ffff));
            }
            let has_map = cb.class_has_8bitchar > 0;
            let mut map = cb.map;
            if has_map && negate_class && !cb.xclass_has_prop {
                for x in map.iter_mut() {
                    *x = !*x;
                }
            }
            Class {
                kind: ClassKind::XClass {
                    not: negate_class,
                    has_map,
                    has_prop: cb.xclass_has_prop,
                },
                map: if has_map { map } else { [0; 32] },
                xitems: cb.xitems,
            }
        } else {
            let mut map = cb.map;
            if negate_class {
                for x in map.iter_mut() {
                    *x = !*x;
                }
            }
            Class {
                kind: if negate_class == should_flip_negation {
                    ClassKind::Class
                } else {
                    ClassKind::NClass
                },
                map,
                xitems: Vec::new(),
            }
        };
        self.emit(Op::Item(Item::Class(Box::new(class))))?;
        Ok(None)
    }

    // ------------------------------------------------------------------
    // Repeats
    // ------------------------------------------------------------------

    fn compile_repeat(
        &mut self,
        b: &mut Br,
        repeat_min_in: u32,
        repeat_max_in: u32,
        mode: QuantMode,
    ) -> CResult<()> {
        let mut repeat_min = repeat_min_in;
        let mut repeat_max = repeat_max_in;
        if b.previous_matched_char && repeat_min > 0 {
            b.matched_char = true;
        }
        let reqvary = if repeat_min == repeat_max {
            0
        } else {
            REQ_VARY
        };
        if repeat_min == 0 {
            b.firstcu = b.zerofirstcu;
            b.firstcuflags = b.zerofirstcuflags;
            b.reqcu = b.zeroreqcu;
            b.reqcuflags = b.zeroreqcuflags;
        }
        let greedy_default = b.options & PCRE2_UNGREEDY == 0;
        let (greedy, mut possessive) = match mode {
            QuantMode::Possessive => (true, true),
            QuantMode::Lazy => (!greedy_default, false),
            QuantMode::Greedy => (greedy_default, false),
        };
        let rep_kind = if greedy { RepKind::Max } else { RepKind::Min };
        let tempcode = b.previous;
        let previous = b.previous;
        if previous >= self.code.len() {
            return Err((INTERNAL_UNEXPECTED_REPEAT, self.pattern.len()));
        }
        let inf_max = |m: u32| if m == REPEAT_UNLIMITED { REP_INF } else { m };

        let op_previous = self.code[previous].clone();
        let mut handled_possessive = false;
        match op_previous {
            Op::Item(item) if !matches!(item, Item::Class(_)) => {
                if repeat_max == 1 && repeat_min == 1 {
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                // Character repeats: set reqcu for single-code-unit chars.
                match item {
                    Item::Char(c) | Item::CharI(c, _) => {
                        let single = !self.utf || c < 128;
                        if single && repeat_min > 1 {
                            b.reqcu = c;
                            b.reqcuflags = self.req_varyopt;
                            if matches!(item, Item::CharI(..)) {
                                b.reqcuflags |= REQ_CASELESS;
                            }
                        }
                    }
                    _ => {}
                }
                if repeat_max == 0 {
                    self.truncate_code(previous);
                } else {
                    self.replace_op(
                        previous,
                        Op::Rep {
                            item,
                            min: repeat_min,
                            max: inf_max(repeat_max),
                            kind: if possessive { RepKind::Pos } else { rep_kind },
                        },
                    )?;
                }
                handled_possessive = true;
            }
            Op::Item(Item::Class(cl)) => {
                if repeat_max == 0 {
                    self.truncate_code(previous);
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                if repeat_max == 1 && repeat_min == 1 {
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                self.replace_op(
                    previous,
                    Op::Rep {
                        item: Item::Class(cl),
                        min: repeat_min,
                        max: inf_max(repeat_max),
                        kind: if possessive { RepKind::Pos } else { rep_kind },
                    },
                )?;
                handled_possessive = true;
            }
            Op::Ref {
                group, caseless, ..
            } => {
                if repeat_max == 0 {
                    self.truncate_code(previous);
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                if repeat_max == 1 && repeat_min == 1 {
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                self.replace_op(
                    previous,
                    Op::Ref {
                        group,
                        caseless,
                        rep: Some((repeat_min, inf_max(repeat_max), rep_kind)),
                    },
                )?;
            }
            Op::DnRef {
                index,
                count,
                caseless,
                ..
            } => {
                if repeat_max == 0 {
                    self.truncate_code(previous);
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                if repeat_max == 1 && repeat_min == 1 {
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                self.replace_op(
                    previous,
                    Op::DnRef {
                        index,
                        count,
                        caseless,
                        rep: Some((repeat_min, inf_max(repeat_max), rep_kind)),
                    },
                )?;
            }
            Op::Fail => {
                self.req_varyopt |= reqvary;
                return Ok(());
            }
            Op::Recurse { .. } => {
                if repeat_max == 1 && repeat_min == 1 && !possessive {
                    self.req_varyopt |= reqvary;
                    return Ok(());
                }
                let mut prev = previous;
                let mut fall_through = true;
                if repeat_min > 0 && (repeat_min != 1 || repeat_max != REPEAT_UNLIMITED) {
                    let mut replicate = repeat_min;
                    if repeat_min == repeat_max {
                        replicate -= 1;
                    }
                    for _ in 0..replicate {
                        let op = self.code[prev].clone();
                        prev = self.code.len();
                        self.emit(op)?;
                    }
                    if repeat_min == repeat_max {
                        fall_through = false;
                    } else {
                        if repeat_max != REPEAT_UNLIMITED {
                            repeat_max -= repeat_min;
                        }
                        repeat_min = 0;
                    }
                }
                if fall_through {
                    // Wrap the (last) recursion in OP_BRA brackets.
                    self.insert(
                        prev,
                        Op::Bra {
                            kind: BraKind::Bra,
                            next: 2,
                            group: 0,
                        },
                    )?;
                    self.emit(Op::Ket(KetKind::Ket, 2))?;
                    b.group_return = -1;
                    self.repeat_group(b, prev, repeat_min, repeat_max, rep_kind, &mut possessive)?;
                }
            }
            Op::Bra { .. } => {
                self.repeat_group(
                    b,
                    previous,
                    repeat_min,
                    repeat_max,
                    rep_kind,
                    &mut possessive,
                )?;
            }
            _ => return Err((INTERNAL_UNEXPECTED_REPEAT, self.pattern.len())),
        }

        if possessive && !handled_possessive {
            // Wrap everything from tempcode in ONCE brackets.
            let len = self.code.len() - tempcode;
            if len > 0 {
                self.insert(
                    tempcode,
                    Op::Bra {
                        kind: BraKind::Once,
                        next: len + 1,
                        group: 0,
                    },
                )?;
                let ket = self.code.len();
                self.emit(Op::Ket(KetKind::Ket, ket - tempcode))?;
            }
        }
        self.req_varyopt |= reqvary;
        Ok(())
    }

    fn truncate_code(&mut self, at: usize) {
        // PCRE2 never reduces its length estimate.
        self.code.truncate(at);
    }

    fn replace_op(&mut self, at: usize, op: Op) -> CResult<()> {
        let old = self.code[at].pcre2_len(self.utf, &self.marks);
        let new = op.pcre2_len(self.utf, &self.marks);
        if new > old {
            self.length += new - old;
        }
        self.code[at] = op;
        self.check_length()
    }

    /// Repetition of a bracketed group starting at `previous`.
    fn repeat_group(
        &mut self,
        b: &mut Br,
        mut previous: usize,
        repeat_min: u32,
        mut repeat_max: u32,
        rep_kind: RepKind,
        possessive: &mut bool,
    ) -> CResult<()> {
        let (op_kind, cond_false_single) = match &self.code[previous] {
            Op::Bra { kind, next, .. } => {
                let single = !matches!(self.code[previous + *next], Op::Alt(_));
                let false_cond =
                    *kind == BraKind::Cond && matches!(self.code[previous + 1], Op::False);
                (*kind, false_cond && single)
            }
            _ => return Err((INTERNAL_UNEXPECTED_REPEAT, self.pattern.len())),
        };
        let mut len = self.code.len() - previous;
        if repeat_max == 1 && repeat_min == 1 && !*possessive {
            return Ok(());
        }
        if cond_false_single {
            return Ok(());
        }
        if op_kind.is_assertion() && repeat_max == REPEAT_UNLIMITED {
            repeat_max = repeat_min + 1;
        }
        let brazero_op = if rep_kind == RepKind::Min {
            Op::BraMinZero
        } else {
            Op::BraZero
        };
        let mut brazero_idx: Option<usize> = None;
        let mut bralinks: Vec<usize> = Vec::new();

        if repeat_min == 0 {
            if repeat_max <= 1 || repeat_max == REPEAT_UNLIMITED {
                if repeat_max == 0 {
                    self.insert(previous, Op::SkipZero)?;
                    return Ok(());
                }
                self.insert(previous, brazero_op.clone())?;
                brazero_idx = Some(previous);
                previous += 1;
            } else {
                self.insert(previous, brazero_op.clone())?;
                self.insert(
                    previous + 1,
                    Op::Bra {
                        kind: BraKind::Bra,
                        next: 0,
                        group: 0,
                    },
                )?;
                bralinks.push(previous + 1);
                previous += 2;
            }
            if repeat_max != REPEAT_UNLIMITED {
                repeat_max -= 1;
            }
        } else {
            if repeat_min > 1 {
                if b.groupsetfirstcu && b.reqcuflags >= REQ_NONE {
                    b.reqcu = b.firstcu;
                    b.reqcuflags = b.firstcuflags;
                }
                self.replicate(previous, len, repeat_min - 1)?;
            }
            if repeat_max != REPEAT_UNLIMITED {
                repeat_max -= repeat_min;
            }
        }

        if repeat_max != REPEAT_UNLIMITED {
            if repeat_max > 0 {
                // Size check before replicating.
                let flen = self.frag_len(previous, previous + len);
                let est = (flen + 1 + 2 + 2 * 2).saturating_mul(repeat_max as usize);
                if self.length.saturating_add(est) > MAX_PATTERN_SIZE + 6 {
                    return Err((PATTERN_TOO_LARGE, self.pattern.len()));
                }
            }
            let mut i = repeat_max;
            while i >= 1 {
                self.emit(brazero_op.clone())?;
                if i != 1 {
                    bralinks.push(self.code.len());
                    self.emit(Op::Bra {
                        kind: BraKind::Bra,
                        next: 0,
                        group: 0,
                    })?;
                }
                self.replicate(previous, len, 1)?;
                i -= 1;
            }
            while let Some(bra) = bralinks.pop() {
                let ket = self.code.len();
                if let Op::Bra { next, .. } = &mut self.code[bra] {
                    *next = ket - bra;
                }
                self.emit(Op::Ket(KetKind::Ket, ket - bra))?;
            }
        } else {
            let ketcode = self.code.len() - 1;
            let bracode = match self.code[ketcode] {
                Op::Ket(_, back) => ketcode - back,
                _ => return Err((INTERNAL_UNEXPECTED_REPEAT, self.pattern.len())),
            };
            let mut kind = match self.code[bracode] {
                Op::Bra { kind, .. } => kind,
                _ => BraKind::Bra,
            };
            if kind == BraKind::Once && *possessive {
                kind = BraKind::Bra;
            }
            let ket_kind_for = |rk: RepKind| {
                if rk == RepKind::Min {
                    KetKind::KetRMin
                } else {
                    KetKind::KetRMax
                }
            };
            if kind == BraKind::Once || kind == BraKind::ScriptRun {
                self.set_bra_kind(bracode, kind);
                self.code[ketcode] = Op::Ket(ket_kind_for(rep_kind), ketcode - bracode);
            } else {
                if b.group_return < 0 {
                    kind = kind.to_s();
                }
                if kind == BraKind::Cond
                    && let Op::Bra { next, .. } = self.code[bracode]
                    && !matches!(self.code[bracode + next], Op::Alt(_))
                {
                    kind = BraKind::SCond;
                }
                self.set_bra_kind(bracode, kind);
                if *possessive {
                    if kind == BraKind::Cond || kind == BraKind::SCond {
                        let nlen = self.code.len() - bracode;
                        self.insert(
                            bracode,
                            Op::Bra {
                                kind: if kind == BraKind::Cond {
                                    BraKind::BraPos
                                } else {
                                    BraKind::SBraPos
                                },
                                next: nlen + 1,
                                group: 0,
                            },
                        )?;
                        let k = self.code.len();
                        self.emit(Op::Ket(KetKind::KetRPos, k - bracode))?;
                    } else {
                        self.set_bra_kind(bracode, kind.to_pos());
                        self.code[ketcode] = Op::Ket(KetKind::KetRPos, ketcode - bracode);
                    }
                    if let Some(bz) = brazero_idx {
                        self.code[bz] = Op::BraPosZero;
                    }
                    if repeat_min < 2 {
                        *possessive = false;
                    }
                } else {
                    self.code[ketcode] = Op::Ket(ket_kind_for(rep_kind), ketcode - bracode);
                }
            }
        }
        len = 0;
        let _ = len;
        Ok(())
    }

    fn set_bra_kind(&mut self, at: usize, k: BraKind) {
        if let Op::Bra { kind, .. } = &mut self.code[at] {
            *kind = k;
        }
    }
}

fn or_bits(map: &mut [u8; 32], bits: &[u8], negate: bool) {
    for k in 0..32 {
        map[k] |= if negate { !bits[k] } else { bits[k] };
    }
}

/// Settings collected from the start of the pattern ((*UTF), (*CRLF) ...).
struct StartSettings {
    skip: usize,
    options: u32,
    flags: u32,
    newline: Option<Newline>,
    bsr_anycrlf: Option<bool>,
    limit_match: u32,
    limit_depth: u32,
    limit_heap: u32,
}

fn read_start_settings(pat: &[u8]) -> Result<StartSettings, (i32, usize)> {
    let mut s = StartSettings {
        skip: 0,
        options: 0,
        flags: 0,
        newline: None,
        bsr_anycrlf: None,
        limit_match: u32::MAX,
        limit_depth: u32::MAX,
        limit_heap: u32::MAX,
    };
    let at = |i: usize| -> u8 { pat.get(i).copied().unwrap_or(0) };
    enum T {
        Opt(u32),
        Flg(u32),
        Nl(Newline),
        Bsr(bool),
        LimH,
        LimM,
        LimD,
    }
    let list: [(&[u8], T); 21] = [
        (b"UTF8)", T::Opt(PCRE2_UTF)),
        (b"UTF)", T::Opt(PCRE2_UTF)),
        (b"UCP)", T::Opt(PCRE2_UCP)),
        (b"NOTEMPTY)", T::Flg(FLAG_NOTEMPTY_SET)),
        (b"NOTEMPTY_ATSTART)", T::Flg(FLAG_NE_ATST_SET)),
        (b"NO_AUTO_POSSESS)", T::Opt(PCRE2_NO_AUTO_POSSESS)),
        (b"NO_DOTSTAR_ANCHOR)", T::Opt(PCRE2_NO_DOTSTAR_ANCHOR)),
        (b"NO_JIT)", T::Flg(FLAG_NOJIT)),
        (b"NO_START_OPT)", T::Opt(PCRE2_NO_START_OPTIMIZE)),
        (b"LIMIT_HEAP=", T::LimH),
        (b"LIMIT_MATCH=", T::LimM),
        (b"LIMIT_DEPTH=", T::LimD),
        (b"LIMIT_RECURSION=", T::LimD),
        (b"CR)", T::Nl(Newline::Cr)),
        (b"LF)", T::Nl(Newline::Lf)),
        (b"CRLF)", T::Nl(Newline::CrLf)),
        (b"ANY)", T::Nl(Newline::Any)),
        (b"NUL)", T::Nl(Newline::Nul)),
        (b"ANYCRLF)", T::Nl(Newline::AnyCrLf)),
        (b"BSR_ANYCRLF)", T::Bsr(true)),
        (b"BSR_UNICODE)", T::Bsr(false)),
    ];
    let patlen = pat.len();
    while patlen - s.skip >= 2 && pat[s.skip] == b'(' && pat[s.skip + 1] == b'*' {
        let mut found = false;
        for (name, t) in list.iter() {
            if patlen - s.skip - 2 >= name.len()
                && &pat[s.skip + 2..s.skip + 2 + name.len()] == *name
            {
                s.skip += name.len() + 2;
                match t {
                    T::Opt(o) => s.options |= o,
                    T::Flg(f) => s.flags |= f,
                    T::Nl(n) => {
                        s.newline = Some(*n);
                        s.flags |= FLAG_NL_SET;
                    }
                    T::Bsr(b) => {
                        s.bsr_anycrlf = Some(*b);
                        s.flags |= FLAG_BSR_SET;
                    }
                    T::LimH | T::LimM | T::LimD => {
                        let mut c: u32 = 0;
                        let mut pp = s.skip;
                        if !at(pp).is_ascii_digit() {
                            return Err((VERB_UNKNOWN, pp));
                        }
                        while at(pp).is_ascii_digit() {
                            if c > u32::MAX / 10 - 1 {
                                break;
                            }
                            c = c * 10 + (at(pp) - b'0') as u32;
                            pp += 1;
                        }
                        let ch = at(pp);
                        pp += 1;
                        if ch != b')' {
                            return Err((VERB_UNKNOWN, pp));
                        }
                        match t {
                            T::LimH => s.limit_heap = c,
                            T::LimM => s.limit_match = c,
                            _ => s.limit_depth = c,
                        }
                        s.skip = pp;
                    }
                }
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }
    Ok(s)
}

/// Compile a pattern (the equivalent of `pcre2_compile()`).
pub(crate) fn compile(
    pattern: &[u8],
    options: u32,
    xoptions: u32,
) -> Result<Program, CompileError> {
    let err = |(code, off): (i32, usize)| CompileError::new(code, off);

    let st = read_start_settings(pattern).map_err(err)?;
    let mut external_options = options | st.options;
    let utf = external_options & PCRE2_UTF != 0;
    if utf {
        if options & PCRE2_NEVER_UTF != 0 {
            return Err(CompileError::new(UTF_IS_DISABLED, st.skip));
        }
        if options & PCRE2_NO_UTF_CHECK == 0
            && let Err((code, off)) = chars::valid_utf8(pattern)
        {
            return Err(CompileError::new(code, off));
        }
    }
    let ucp = external_options & PCRE2_UCP != 0;
    if ucp && external_options & PCRE2_NEVER_UCP != 0 {
        return Err(CompileError::new(UCP_IS_DISABLED, st.skip));
    }
    let newline = st.newline.unwrap_or(Newline::Lf);
    let bsr_anycrlf = st.bsr_anycrlf.unwrap_or(false);

    // Parse.
    let parser = Parser::new(pattern, utf, newline, external_options);
    let pr = parser.parse(st.skip, xoptions).map_err(err)?;
    let mut parsed = pr.parsed;
    let mut external_flags = st.flags | pr.external_flags;
    let dupcapused = external_flags & FLAG_DUPCAPUSED != 0;

    // Lookbehind lengths.
    let mut max_lookbehind = 0;
    if pr.has_lookbehind {
        let mut chk = LookbehindChecker::new(
            &mut parsed,
            pr.bracount,
            &pr.named_groups,
            pattern,
            pr.small_ref_offset,
            dupcapused,
            external_options & PCRE2_MATCH_UNSET_BACKREF != 0,
            utf,
        );
        chk.run().map_err(err)?;
        max_lookbehind = chk.max_lookbehind;
    }

    // Name table, sorted as add_name_to_table() does.
    let mut name_table: Vec<(u16, Vec<u8>)> = pr
        .named_groups
        .iter()
        .map(|ng| (ng.number as u16, ng.name.clone()))
        .collect();
    name_table.sort_by(|a, b| a.1.cmp(&b.1));

    let mut c = Compiler {
        pattern,
        parsed,
        utf,
        ucp,
        code: Vec::with_capacity(pattern.len() + 8),
        length: 1,
        marks: Vec::new(),
        bracount: pr.bracount,
        named_groups: pr.named_groups,
        name_table,
        small_ref_offset: pr.small_ref_offset,
        assert_depth: 0,
        had_accept: false,
        had_pruneorskip: false,
        had_recurse: false,
        backref_map: 0,
        top_backref: 0,
        req_varyopt: 0,
        max_lookbehind,
        external_flags,
        open_caps: Vec::new(),
    };

    c.code.push(Op::Bra {
        kind: BraKind::Bra,
        next: 0,
        group: 0,
    });
    c.length += 3;
    let mut pptr = 0usize;
    let (regexrc, cu) = c
        .compile_regex(external_options, xoptions, &mut pptr, 0)
        .map_err(err)?;
    c.code.push(Op::End);

    let total: usize = 1 + c
        .code
        .iter()
        .map(|o| o.pcre2_len(utf, &c.marks))
        .sum::<usize>();
    if total > MAX_PATTERN_SIZE + 1 {
        return Err(CompileError::new(PATTERN_TOO_LARGE, pattern.len()));
    }

    let mut flags = external_flags | c.external_flags;
    if regexrc < 0 {
        flags |= FLAG_MATCH_EMPTY;
    }
    let mut firstcu = cu.firstcu;
    let mut firstcuflags = cu.firstcuflags;
    let mut reqcu = cu.reqcu;
    let mut reqcuflags = cu.reqcuflags;
    if c.had_accept {
        reqcu = 0;
        reqcuflags = REQ_NONE;
        flags |= FLAG_HASACCEPT;
    }

    // Resolve recursion targets.
    if c.had_recurse {
        let mut group_starts: Vec<usize> = vec![usize::MAX; c.bracount as usize + 1];
        group_starts[0] = 0;
        for (i, op) in c.code.iter().enumerate() {
            if let Op::Bra { kind, group, .. } = op
                && kind.is_capture()
                && group_starts[*group as usize] == usize::MAX
            {
                group_starts[*group as usize] = i;
            }
        }
        for op in c.code.iter_mut() {
            if let Op::Recurse { group, target } = op {
                let t = group_starts[*group as usize];
                if t == usize::MAX {
                    return Err(CompileError::new(
                        INTERNAL_MISSING_SUBPATTERN,
                        pattern.len(),
                    ));
                }
                *target = t;
            }
        }
    }

    external_flags = flags;
    let mut prog = Program {
        ops: std::mem::take(&mut c.code),
        marks: std::mem::take(&mut c.marks),
        utf,
        ucp,
        top_bracket: c.bracount,
        top_backref: c.top_backref,
        name_table: std::mem::take(&mut c.name_table),
        overall_options: external_options,
        flags: external_flags,
        first_codeunit: 0,
        last_codeunit: 0,
        start_bitmap: [0; 32],
        minlength: 0,
        max_lookbehind: c.max_lookbehind,
        newline,
        bsr_anycrlf,
        limit_match: st.limit_match,
        limit_depth: st.limit_depth,
        limit_heap: st.limit_heap,
    };

    if prog.overall_options & PCRE2_NO_AUTO_POSSESS == 0 {
        crate::possess::auto_possessify(&mut prog);
    }

    let info = crate::study::AnchorInfo {
        backref_map: c.backref_map,
        had_pruneorskip: c.had_pruneorskip,
        no_dotstar_anchor: external_options & PCRE2_NO_DOTSTAR_ANCHOR != 0,
    };
    if prog.overall_options & PCRE2_ANCHORED == 0
        && crate::study::is_anchored(&prog.ops, 0, 0, &info, 0, false)
    {
        prog.overall_options |= PCRE2_ANCHORED;
        external_options = prog.overall_options;
    }
    let _ = external_options;

    if prog.overall_options & PCRE2_NO_START_OPTIMIZE == 0 {
        let mut minminlength = 0u32;
        if firstcuflags >= REQ_NONE {
            let (fc, ff) = crate::study::find_firstassertedcu(&prog.ops, 0, 0, utf);
            firstcu = fc;
            firstcuflags = ff;
        }
        if firstcuflags < REQ_NONE {
            prog.first_codeunit = firstcu;
            prog.flags |= FLAG_FIRSTSET;
            minminlength += 1;
            if firstcuflags & REQ_CASELESS != 0 {
                if firstcu < 128 || (!utf && !ucp && firstcu < 255) {
                    if chars::fcc(firstcu) != firstcu {
                        prog.flags |= FLAG_FIRSTCASELESS;
                    }
                } else if ucp && !utf && unicode::other_case(firstcu) != firstcu {
                    prog.flags |= FLAG_FIRSTCASELESS;
                }
            }
        } else if prog.overall_options & PCRE2_ANCHORED == 0
            && crate::study::is_startline(&prog.ops, 0, 0, &info, 0, false)
        {
            prog.flags |= FLAG_STARTLINE;
        }

        if reqcuflags < REQ_NONE {
            if !utf || firstcuflags >= REQ_NONE || firstcu & 0x80 == 0 || reqcu & 0x80 == 0 {
                minminlength += 1;
            }
            if prog.overall_options & PCRE2_ANCHORED == 0 || reqcuflags & REQ_VARY != 0 {
                prog.last_codeunit = reqcu;
                prog.flags |= FLAG_LASTSET;
                if reqcuflags & REQ_CASELESS != 0 {
                    if reqcu < 128 || (!utf && !ucp && reqcu < 255) {
                        if chars::fcc(reqcu) != reqcu {
                            prog.flags |= FLAG_LASTCASELESS;
                        }
                    } else if ucp && !utf && unicode::other_case(reqcu) != reqcu {
                        prog.flags |= FLAG_LASTCASELESS;
                    }
                }
            }
        }

        crate::study::study(&mut prog);

        if prog.flags & FLAG_FIRSTMAPSET != 0 && minminlength == 0 {
            minminlength = 1;
        }
        if prog.minlength < minminlength {
            prog.minlength = minminlength;
        }
    }

    // (keep utf8_len referenced for non-UTF builds of helper code)
    let _ = utf8_len(0);
    Ok(prog)
}
