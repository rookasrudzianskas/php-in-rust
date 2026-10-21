//! Lookbehind length computation on the parsed pattern: a port of PCRE2's
//! `check_lookbehinds()`, `set_lookbehind_lengths()`, `get_branchlength()`,
//! `get_grouplength()` and `parsed_skip()`.

use crate::error::codes::*;
use crate::parse::{Esc, Meta, NamedGroup, REPEAT_UNLIMITED};

pub(crate) const LOOKBEHIND_MAX: u32 = u16::MAX as u32;
pub(crate) const MAX_VARLOOKBEHIND: u32 = 255;

const GI_SET_FIXED_LENGTH: u32 = 0x8000_0000;
const GI_NOT_FIXED_LENGTH: u32 = 0x4000_0000;
const GI_FIXED_LENGTH_MASK: u32 = 0x0000_ffff;

const UNSET: usize = usize::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Skip {
    Class,
    Alt,
    Ket,
}

fn is_group_open(m: &Meta) -> bool {
    matches!(
        m,
        Meta::Atomic
            | Meta::Capture(_)
            | Meta::CondAssert
            | Meta::CondDefine { .. }
            | Meta::CondName { .. }
            | Meta::CondNumber { .. }
            | Meta::CondVersion { .. }
            | Meta::Lookahead
            | Meta::LookaheadNot
            | Meta::LookaheadNa
            | Meta::Lookbehind { .. }
            | Meta::NoCapture
            | Meta::ScriptRun
    )
}

/// `parsed_skip()`.
fn parsed_skip(parsed: &[Meta], mut p: usize, skiptype: Skip) -> Option<usize> {
    let mut nestlevel = 0u32;
    loop {
        match &parsed[p] {
            Meta::End => return None,
            Meta::ClassEnd if skiptype == Skip::Class => return Some(p),
            Meta::Alt(_) if nestlevel == 0 && skiptype == Skip::Alt => return Some(p),
            Meta::Ket => {
                if nestlevel == 0 {
                    return Some(p);
                }
                nestlevel -= 1;
            }
            m if is_group_open(m) => nestlevel += 1,
            _ => {}
        }
        p += 1;
    }
}

pub(crate) struct LookbehindChecker<'a> {
    pub parsed: &'a mut Vec<Meta>,
    pub bracount: u32,
    pub named_groups: &'a [NamedGroup],
    pub pattern: &'a [u8],
    pub small_ref_offset: [usize; 10],
    pub dupcapused: bool,
    pub match_unset_backref: bool,
    pub utf: bool,
    groupinfo: Vec<u32>,
    pub erroroffset: usize,
    pub max_lookbehind: u32,
    loopcount: u32,
}

impl<'a> LookbehindChecker<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        parsed: &'a mut Vec<Meta>,
        bracount: u32,
        named_groups: &'a [NamedGroup],
        pattern: &'a [u8],
        small_ref_offset: [usize; 10],
        dupcapused: bool,
        match_unset_backref: bool,
        utf: bool,
    ) -> Self {
        LookbehindChecker {
            parsed,
            bracount,
            named_groups,
            pattern,
            small_ref_offset,
            dupcapused,
            match_unset_backref,
            utf,
            groupinfo: vec![0; 2 * (bracount as usize + 1)],
            erroroffset: UNSET,
            max_lookbehind: 0,
            loopcount: 0,
        }
    }

    /// Run the whole check. Returns Err((code, offset)).
    pub(crate) fn run(&mut self) -> Result<(), (i32, usize)> {
        let mut ret = 0usize;
        let rc = self.check_lookbehinds(0, false, &mut ret, &[]);
        if rc != 0 {
            let off = if self.erroroffset == UNSET {
                self.pattern.len()
            } else {
                self.erroroffset
            };
            return Err((rc, off));
        }
        Ok(())
    }

    /// `check_lookbehinds()`. If `stop_at_ket`, stops at an unmatched Ket and
    /// stores its index in `retptr`.
    fn check_lookbehinds(
        &mut self,
        mut p: usize,
        stop_at_ket: bool,
        retptr: &mut usize,
        recurses: &[usize],
    ) -> i32 {
        let mut nestlevel: i32 = 0;
        self.erroroffset = UNSET;
        while !matches!(self.parsed[p], Meta::End) {
            match &self.parsed[p] {
                Meta::Ket => {
                    nestlevel -= 1;
                    if nestlevel < 0 {
                        if stop_at_ket {
                            *retptr = p;
                        }
                        return 0;
                    }
                }
                Meta::Atomic
                | Meta::Capture(_)
                | Meta::CondAssert
                | Meta::Lookahead
                | Meta::LookaheadNot
                | Meta::LookaheadNa
                | Meta::NoCapture
                | Meta::ScriptRun
                | Meta::CondDefine { .. }
                | Meta::CondName { .. }
                | Meta::CondNumber { .. }
                | Meta::CondVersion { .. } => nestlevel += 1,
                Meta::Lookbehind { .. } => {
                    let mut errorcode = 0;
                    if !self.set_lookbehind_lengths(&mut p, &mut errorcode, recurses) {
                        return errorcode;
                    }
                }
                _ => {}
            }
            p += 1;
        }
        0
    }

    /// `set_lookbehind_lengths()`. `p` points at the Lookbehind meta; on exit
    /// it points at the final Ket.
    fn set_lookbehind_lengths(
        &mut self,
        p: &mut usize,
        errcode: &mut i32,
        recurses: &[usize],
    ) -> bool {
        let gb = *p;
        let offset = match self.parsed[gb] {
            Meta::Lookbehind { offset, .. } => offset,
            _ => 0,
        };
        let mut bptr = gb;
        let mut maxlength: i64 = 0;
        let mut minlength: i64 = i64::MAX;
        let mut variable = false;

        loop {
            *p += 1;
            let mut branchminlength = 0i64;
            let branchlength = self.get_branchlength(p, &mut branchminlength, errcode, recurses);
            if branchlength < 0 {
                if *errcode == 0 {
                    *errcode = LOOKBEHIND_NOT_FIXED_LENGTH;
                }
                if self.erroroffset == UNSET {
                    self.erroroffset = offset;
                }
                return false;
            }
            if branchlength != branchminlength {
                variable = true;
            }
            if branchminlength < minlength {
                minlength = branchminlength;
            }
            if branchlength > maxlength {
                maxlength = branchlength;
            }
            if branchlength as u32 > self.max_lookbehind {
                self.max_lookbehind = branchlength as u32;
            }
            match &mut self.parsed[bptr] {
                Meta::Lookbehind { len, .. } => *len = branchlength as u32,
                Meta::Alt(len) => *len = branchlength as u32,
                _ => {}
            }
            bptr = *p;
            if !matches!(self.parsed[bptr], Meta::Alt(_)) {
                break;
            }
        }

        if variable && maxlength as u32 > MAX_VARLOOKBEHIND {
            *errcode = VARLOOKBEHIND_TOO_LONG;
            self.erroroffset = offset;
            return false;
        }
        if let Meta::Lookbehind { minlen, .. } = &mut self.parsed[gb] {
            *minlen = if variable {
                minlength as u32
            } else {
                LOOKBEHIND_MAX
            };
        }
        true
    }

    /// `get_grouplength()`. `p` points at the first item inside the group;
    /// on exit at the group's Ket.
    #[allow(clippy::too_many_arguments)]
    fn get_grouplength(
        &mut self,
        p: &mut usize,
        minptr: &mut i64,
        isinline: bool,
        errcode: &mut i32,
        group: u32,
        recurses: &[usize],
    ) -> i64 {
        let gi = 2 * group as usize;
        if group > 0 && !self.dupcapused {
            let info = self.groupinfo[gi];
            if info & GI_NOT_FIXED_LENGTH != 0 {
                return -1;
            }
            if info & GI_SET_FIXED_LENGTH != 0 {
                if isinline && let Some(e) = parsed_skip(self.parsed, *p, Skip::Ket) {
                    *p = e;
                }
                *minptr = self.groupinfo[gi + 1] as i64;
                return (info & GI_FIXED_LENGTH_MASK) as i64;
            }
        }
        let mut grouplength: i64 = -1;
        let mut groupminlength: i64 = i64::MAX;
        loop {
            let mut branchminlength = 0i64;
            let branchlength = self.get_branchlength(p, &mut branchminlength, errcode, recurses);
            if branchlength < 0 {
                if group > 0 {
                    self.groupinfo[gi] |= GI_NOT_FIXED_LENGTH;
                }
                return -1;
            }
            if branchlength > grouplength {
                grouplength = branchlength;
            }
            if branchminlength < groupminlength {
                groupminlength = branchminlength;
            }
            if matches!(self.parsed[*p], Meta::Ket) {
                break;
            }
            *p += 1;
        }
        if group > 0 {
            self.groupinfo[gi] |= GI_SET_FIXED_LENGTH | grouplength as u32;
            self.groupinfo[gi + 1] = groupminlength as u32;
        }
        *minptr = groupminlength;
        grouplength
    }

    /// `get_branchlength()`. `pp` points at the first item in the branch; on
    /// exit at the Alt or Ket.
    fn get_branchlength(
        &mut self,
        pp: &mut usize,
        minptr: &mut i64,
        errcode: &mut i32,
        recurses: &[usize],
    ) -> i64 {
        let mut branchlength: i64 = 0;
        let mut branchminlength: i64 = 0;
        let mut lastitemlength: i64 = 0;
        let mut lastitemminlength: i64 = 0;
        let mut p = *pp;

        self.loopcount += 1;
        if self.loopcount > 2001 {
            *errcode = LOOKBEHIND_TOO_COMPLICATED;
            return -1;
        }

        macro_rules! not_fixed {
            () => {{
                *errcode = LOOKBEHIND_NOT_FIXED_LENGTH;
                return -1;
            }};
        }

        loop {
            let mut itemlength: i64 = 0;
            let mut itemminlength: i64 = 0;
            let mut group: u32 = 0;
            let mut ref_offset: usize = 0;
            let mut is_ref_or_recurse = false;
            let mut check_group = false;

            match self.parsed[p].clone() {
                Meta::Lit(_) => {
                    itemlength = 1;
                    itemminlength = 1;
                }
                Meta::Ket | Meta::Alt(_) => break,
                Meta::End => break,
                Meta::Accept | Meta::Fail => {
                    match parsed_skip(self.parsed, p, Skip::Alt) {
                        Some(e) => p = e,
                        None => {
                            *errcode = INTERNAL_BAD_CODE;
                            return -1;
                        }
                    }
                    break;
                }
                Meta::VerbArg(..)
                | Meta::Circumflex
                | Meta::Commit
                | Meta::Dollar
                | Meta::Prune
                | Meta::Skip
                | Meta::Then
                | Meta::Options { .. }
                | Meta::Callout => {}
                Meta::Class(_) => {
                    itemlength = 1;
                    itemminlength = 1;
                    match parsed_skip(self.parsed, p, Skip::Class) {
                        Some(e) => p = e,
                        None => {
                            *errcode = INTERNAL_BAD_CODE;
                            return -1;
                        }
                    }
                }
                Meta::ClassEmpty(true) | Meta::Dot => {
                    itemlength = 1;
                    itemminlength = 1;
                }
                Meta::Escape(e) => {
                    if e == Esc::X {
                        return -1;
                    }
                    if e == Esc::R {
                        itemminlength = 1;
                        itemlength = 2;
                    } else if e.consumes_char() {
                        if self.utf && e == Esc::C {
                            *errcode = LOOKBEHIND_INVALID_BACKSLASH_C;
                            return -1;
                        }
                        itemlength = 1;
                        itemminlength = 1;
                    }
                }
                Meta::Prop { .. } => {
                    itemlength = 1;
                    itemminlength = 1;
                }
                Meta::Lookahead | Meta::LookaheadNot | Meta::LookaheadNa => {
                    let mut ret = p;
                    let rc = self.check_lookbehinds(p + 1, true, &mut ret, recurses);
                    *errcode = rc;
                    if rc != 0 {
                        return -1;
                    }
                    p = ret;
                    // Ignore any qualifiers that follow a lookahead assertion.
                    if self.parsed[p + 1].is_quantifier() {
                        p += 1;
                    }
                }
                Meta::Lookbehind { .. } => {
                    if !self.set_lookbehind_lengths(&mut p, errcode, recurses) {
                        return -1;
                    }
                }
                Meta::BackrefByName { offset, len } | Meta::RecurseByName { offset, len } => {
                    let is_recurse = matches!(self.parsed[p], Meta::RecurseByName { .. });
                    if !is_recurse && self.match_unset_backref {
                        not_fixed!();
                    }
                    let name = &self.pattern[offset..offset + len];
                    let mut isdup = false;
                    if let Some(ng) = self.named_groups.iter().find(|ng| ng.name == name) {
                        group = ng.number;
                        isdup = ng.isdup;
                    }
                    if group == 0 {
                        *errcode = BAD_SUBPATTERN_REFERENCE;
                        self.erroroffset = offset;
                        return -1;
                    }
                    if is_recurse || (!isdup && !self.dupcapused) {
                        is_ref_or_recurse = true;
                        ref_offset = offset;
                    } else {
                        not_fixed!();
                    }
                }
                Meta::Backref { n, offset } => {
                    if self.match_unset_backref || self.dupcapused {
                        not_fixed!();
                    }
                    group = n;
                    ref_offset = if n < 10 {
                        self.small_ref_offset[n as usize]
                    } else {
                        offset
                    };
                    is_ref_or_recurse = true;
                }
                Meta::Recurse { n, offset } => {
                    group = n;
                    ref_offset = offset;
                    is_ref_or_recurse = true;
                }
                Meta::CondDefine { .. } => match parsed_skip(self.parsed, p + 1, Skip::Ket) {
                    Some(e) => p = e,
                    None => {
                        *errcode = INTERNAL_BAD_CODE;
                        return -1;
                    }
                },
                Meta::CondName { .. }
                | Meta::CondNumber { .. }
                | Meta::CondAssert
                | Meta::CondVersion { .. }
                | Meta::Atomic
                | Meta::NoCapture
                | Meta::ScriptRun => {
                    p += 1;
                    check_group = true;
                }
                Meta::Capture(n) => {
                    group = n;
                    p += 1;
                    check_group = true;
                }
                Meta::Query(_) => {
                    if let Some(r) = self.repetition(
                        0,
                        1,
                        &mut branchlength,
                        &mut branchminlength,
                        lastitemlength,
                        lastitemminlength,
                        &mut itemlength,
                        &mut itemminlength,
                        errcode,
                    ) {
                        return r;
                    }
                }
                Meta::MinMax(min, max, _) if max != REPEAT_UNLIMITED => {
                    if let Some(r) = self.repetition(
                        min,
                        max,
                        &mut branchlength,
                        &mut branchminlength,
                        lastitemlength,
                        lastitemminlength,
                        &mut itemlength,
                        &mut itemminlength,
                        errcode,
                    ) {
                        return r;
                    }
                }
                _ => not_fixed!(),
            }

            if is_ref_or_recurse {
                // RECURSE_OR_BACKREF_LENGTH
                if group > self.bracount {
                    self.erroroffset = ref_offset;
                    *errcode = BAD_SUBPATTERN_REFERENCE;
                    return -1;
                }
                if group == 0 {
                    not_fixed!();
                }
                let Some(gptr) = self.parsed.iter().position(|m| *m == Meta::Capture(group)) else {
                    not_fixed!();
                };
                let Some(gptrend) = parsed_skip(self.parsed, gptr + 1, Skip::Ket) else {
                    *errcode = INTERNAL_BAD_CODE;
                    return -1;
                };
                if p > gptr && p < gptrend {
                    not_fixed!();
                }
                if recurses.contains(&gptr) {
                    not_fixed!();
                }
                let mut new_recurses = recurses.to_vec();
                new_recurses.push(gptr);
                let mut gp = gptr + 1;
                let mut groupminlength = 0i64;
                let grouplength = self.get_grouplength(
                    &mut gp,
                    &mut groupminlength,
                    false,
                    errcode,
                    group,
                    &new_recurses,
                );
                if grouplength < 0 {
                    if *errcode == 0 {
                        not_fixed!();
                    }
                    return -1;
                }
                itemlength = grouplength;
                itemminlength = groupminlength;
            } else if check_group {
                let mut groupminlength = 0i64;
                let grouplength = self.get_grouplength(
                    &mut p,
                    &mut groupminlength,
                    true,
                    errcode,
                    group,
                    recurses,
                );
                if grouplength < 0 {
                    return -1;
                }
                itemlength = grouplength;
                itemminlength = groupminlength;
            }

            branchlength += itemlength;
            if branchlength > LOOKBEHIND_MAX as i64 {
                *errcode = LOOKBEHIND_TOO_LONG;
                return -1;
            }
            branchminlength += itemminlength;
            lastitemlength = itemlength;
            lastitemminlength = itemminlength;
            p += 1;
        }

        *pp = p;
        *minptr = branchminlength;
        branchlength
    }

    /// The REPETITION part of get_branchlength for bounded quantifiers.
    /// Returns Some(-1) on error.
    #[allow(clippy::too_many_arguments)]
    fn repetition(
        &self,
        min: u32,
        max: u32,
        branchlength: &mut i64,
        branchminlength: &mut i64,
        lastitemlength: i64,
        lastitemminlength: i64,
        itemlength: &mut i64,
        itemminlength: &mut i64,
        errcode: &mut i32,
    ) -> Option<i64> {
        let max = max as i64;
        let min = min as i64;
        if lastitemlength != 0
            && max != 0
            && (i32::MAX as i64 - *branchlength) / lastitemlength < max - 1
        {
            *errcode = LOOKBEHIND_TOO_LONG;
            return Some(-1);
        }
        if min == 0 {
            *branchminlength -= lastitemminlength;
        } else {
            *itemminlength = (min - 1) * lastitemminlength;
        }
        if max == 0 {
            *branchlength -= lastitemlength;
        } else {
            *itemlength = (max - 1) * lastitemlength;
        }
        None
    }
}
