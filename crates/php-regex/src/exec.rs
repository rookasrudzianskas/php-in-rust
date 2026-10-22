//! The backtracking matcher: a port of PCRE2's `match()` (pcre2_match.c).
//!
//! As in PCRE2 10.30+, backtracking points are kept in an explicit vector of
//! "frames" instead of native recursion. `RMATCH` pushes a copy of the
//! current frame and continues at a new code position; `RRETURN` pops back to
//! the frame that created the backtracking point and resumes it at the
//! remembered return point. The match limit counts frame creations and the
//! depth limit bounds the number of frames, exactly as in PCRE2, so PHP's
//! `pcre.backtrack_limit` / `pcre.recursion_limit` behave the same.

use crate::chars::{self, CTYPE_DIGIT, CTYPE_SPACE, CTYPE_WORD};
use crate::compile::Program;
use crate::error::MatchError;
use crate::ops::*;
use crate::opts::*;
use crate::parse::Newline;
use crate::unicode;

pub(crate) const UNSET: usize = usize::MAX;
const RECURSE_UNSET: u32 = u32::MAX;
const RREF_ANY: u16 = 0xffff;

// Internal return codes (as in pcre2_match.c).
const MATCH_MATCH: i32 = 1;
const MATCH_NOMATCH: i32 = 0;
const MATCH_ACCEPT: i32 = -999;
const MATCH_KETRPOS: i32 = -998;
const MATCH_COMMIT: i32 = -997;
const MATCH_PRUNE: i32 = -996;
const MATCH_SKIP: i32 = -995;
const MATCH_SKIP_ARG: i32 = -994;
const MATCH_THEN: i32 = -993;
const MATCH_BACKTRACK_MAX: i32 = MATCH_THEN;
const MATCH_BACKTRACK_MIN: i32 = MATCH_COMMIT;

// Group frame types.
const GF_CAPTURE: u32 = 0x0001_0000;
const GF_NOCAPTURE: u32 = 0x0002_0000;
const GF_CONDASSERT: u32 = 0x0003_0000;
const GF_RECURSE: u32 = 0x0004_0000;
#[inline]
fn gf_id(a: u32) -> u32 {
    a & 0xffff_0000
}
#[inline]
fn gf_data(a: u32) -> u32 {
    a & 0x0000_ffff
}

/// Where to resume a frame when a backtracking "call" returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rid {
    None,
    BraFast,
    GroupLoop,
    Assert,
    AssertNot,
    CondAssert,
    KetRMin,
    KetRMax,
    PosGroup,
    BraZero,
    BraMinZero,
    Recurse,
    Mark,
    Commit,
    Prune,
    Skip,
    SkipArg,
    Then,
    RefMin,
    RefMaxSame,
    RefMaxDiff,
    RepMin,
    RepMax,
    RepMaxClass,
    SCond,
    VReverse,
}

#[derive(Clone, Debug)]
struct Frame {
    // Not copied to new frames.
    ecode: usize,
    t_sptr: [usize; 2],
    length: usize,
    back_frame: usize,
    t_size: usize,
    group_frame_type: u32,
    t32: [u32; 4],
    return_id: Rid,
    // Copied to new frames.
    eptr: usize,
    start_match: usize,
    mark: Option<u16>,
    recurse_last_used: usize,
    current_recurse: u32,
    capture_last: u32,
    last_group_offset: usize,
    offset_top: usize,
}

enum State {
    Exec,
    Return(i32),
}

/// Options for one match.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExecOptions {
    pub moptions: u32,
    pub match_limit: u32,
    pub depth_limit: u32,
    pub heap_limit: u32,
    pub no_utf_check: bool,
}

/// Result of a successful match.
#[derive(Clone, Debug)]
pub(crate) struct ExecResult {
    pub ovector: Vec<usize>,
    pub count: usize,
    pub mark: Option<u16>,
}

thread_local! {
    /// Reused backtracking-frame storage (avoids allocating per match).
    static POOL: std::cell::RefCell<(Vec<Frame>, Vec<usize>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
}

pub(crate) struct Matcher<'a> {
    prog: &'a Program,
    s: &'a [u8],
    utf: bool,
    ucp: bool,
    end_subject: usize,
    true_end_subject: usize,
    start_offset: usize,
    check_subject: usize,
    moptions: u32,
    poptions: u32,
    match_limit: u32,
    depth_limit: u32,
    heap_limit: u32,
    match_call_count: u32,
    hasthen: bool,
    nl: Newline,
    nllen: usize,
    bsr_anycrlf: bool,
    start_used_ptr: usize,
    last_used_ptr: usize,
    mark: Option<u16>,
    nomatch_mark: Option<u16>,
    verb_ecode_ptr: usize,
    verb_skip_pos: usize,
    verb_skip_name: u16,
    verb_current_recurse: u32,
    skip_arg_count: u32,
    ignore_skip_arg: u32,
    end_match_ptr: usize,
    end_offset_top: usize,
    branch_end: Option<usize>,
    frames: Vec<Frame>,
    ovecs: Vec<usize>,
    ovsize: usize,
    frame_bytes: usize,
    accept_ov: Vec<usize>,
    accept_offset_top: usize,
    accept_mark: Option<u16>,
    result_ov: Vec<usize>,
}

impl<'a> Matcher<'a> {
    // ------------------------------------------------------------------
    // Subject helpers
    // ------------------------------------------------------------------

    #[inline]
    fn getchar(&self, p: usize) -> (u32, usize) {
        if self.utf {
            let b = self.s[p];
            if b < 0x80 {
                (b as u32, 1)
            } else {
                chars::decode_utf8(self.s, p)
            }
        } else {
            (self.s[p] as u32, 1)
        }
    }

    /// BACKCHAR: given `p` pointing after a character, return its start.
    #[inline]
    fn backchar(&self, mut p: usize) -> usize {
        p -= 1;
        if self.utf {
            while p > 0 && self.s[p] & 0xc0 == 0x80 {
                p -= 1;
            }
        }
        p
    }

    #[inline]
    fn is_newline(&mut self, p: usize) -> bool {
        match self.nl.is_newline(self.s, p, self.end_subject, self.utf) {
            Some(l) => {
                self.nllen = l;
                true
            }
            None => false,
        }
    }

    #[inline]
    fn was_newline(&mut self, p: usize) -> bool {
        match self.nl.was_newline(self.s, p, 0, self.utf) {
            Some(l) => {
                self.nllen = l;
                true
            }
            None => false,
        }
    }

    #[inline]
    fn top(&self) -> usize {
        self.frames.len() - 1
    }

    #[inline]
    fn ov(&self, f: usize) -> &[usize] {
        &self.ovecs[f * self.ovsize..(f + 1) * self.ovsize]
    }

    #[inline]
    fn ov_mut(&mut self, f: usize) -> &mut [usize] {
        let n = self.ovsize;
        &mut self.ovecs[f * n..(f + 1) * n]
    }

    /// Copy the ovector of frame `from` into frame `to` (first `n` entries).
    fn copy_ov(&mut self, from: usize, to: usize, n: usize) {
        let os = self.ovsize;
        let n = n.min(os);
        self.ovecs.copy_within(from * os..from * os + n, to * os);
    }

    // ------------------------------------------------------------------
    // Frame handling (RMATCH / RRETURN)
    // ------------------------------------------------------------------

    /// RMATCH: remember `rid` in the current frame and create a new frame
    /// that starts executing at `ecode`.
    fn rmatch(&mut self, ecode: usize, rid: Rid, gft: u32) -> Result<(), MatchError> {
        let f = self.top();
        self.frames[f].return_id = rid;
        let p = &self.frames[f];
        let mut n = Frame {
            ecode,
            t_sptr: [0; 2],
            length: 0,
            back_frame: 1,
            t_size: 0,
            group_frame_type: gft,
            t32: [0; 4],
            return_id: Rid::None,
            eptr: p.eptr,
            start_match: p.start_match,
            mark: p.mark,
            recurse_last_used: p.recurse_last_used,
            current_recurse: p.current_recurse,
            capture_last: p.capture_last,
            last_group_offset: p.last_group_offset,
            offset_top: p.offset_top,
        };
        if gft != 0 {
            n.last_group_offset = f + 1;
            if gf_id(gft) == GF_RECURSE {
                n.current_recurse = gf_data(gft);
            }
        }
        if self.heap_limit != u32::MAX
            && ((f + 2) * self.frame_bytes) / 1024 >= self.heap_limit as usize
        {
            return Err(MatchError::HeapLimit);
        }
        self.frames.push(n);
        let os = self.ovsize;
        if os > 0 {
            self.ovecs.extend_from_within(f * os..(f + 1) * os);
        }
        self.new_frame_checks(f + 1)
    }

    #[inline]
    fn new_frame_checks(&mut self, rdepth: usize) -> Result<(), MatchError> {
        let c = self.match_call_count;
        self.match_call_count = c.wrapping_add(1);
        if c >= self.match_limit {
            return Err(MatchError::MatchLimit);
        }
        if rdepth as u64 >= self.depth_limit as u64 {
            return Err(MatchError::DepthLimit);
        }
        Ok(())
    }

    /// Discard frames above index `keep`.
    fn truncate_frames(&mut self, keep: usize) {
        self.frames.truncate(keep + 1);
        self.ovecs.truncate((keep + 1) * self.ovsize);
    }

    // ------------------------------------------------------------------
    // The main loop
    // ------------------------------------------------------------------

    /// match(): run one matching attempt starting at `start`.
    fn run_match(&mut self, start: usize) -> Result<i32, MatchError> {
        self.frames.clear();
        self.ovecs.clear();
        self.frames.push(Frame {
            ecode: 0,
            t_sptr: [0; 2],
            length: 0,
            back_frame: 1,
            t_size: 0,
            group_frame_type: 0,
            t32: [0; 4],
            return_id: Rid::None,
            eptr: start,
            start_match: start,
            mark: None,
            recurse_last_used: 0,
            current_recurse: RECURSE_UNSET,
            capture_last: 0,
            last_group_offset: UNSET,
            offset_top: 0,
        });
        self.ovecs.resize(self.ovsize, UNSET);
        self.new_frame_checks(0)?;
        let mut state = State::Exec;
        loop {
            state = match state {
                State::Exec => match self.exec()? {
                    Some(st) => st,
                    None => return Ok(MATCH_MATCH),
                },
                State::Return(rrc) => {
                    let f = self.top();
                    if self.frames[f].eptr > self.last_used_ptr {
                        self.last_used_ptr = self.frames[f].eptr;
                    }
                    if f == 0 {
                        return Ok(rrc);
                    }
                    let back = self.frames[f].back_frame;
                    self.truncate_frames(f - back);
                    self.resume(rrc)?
                }
            };
        }
    }

    /// Execute opcodes in the top frame until a backtracking point is created
    /// (returns State::Exec), a return happens (State::Return), or the whole
    /// pattern has matched (None).
    fn exec(&mut self) -> Result<Option<State>, MatchError> {
        let prog = self.prog;
        let ops = &prog.ops;
        let f = self.top();
        loop {
            let ecode = self.frames[f].ecode;
            let eptr = self.frames[f].eptr;
            match &ops[ecode] {
                Op::End => {
                    return self.op_end(f, false);
                }
                Op::Accept => {
                    if self.frames[f].current_recurse != RECURSE_UNSET {
                        // Find the most recent recursion frame.
                        let mut offset = self.frames[f].last_group_offset;
                        loop {
                            if offset == UNSET {
                                return Err(MatchError::Internal);
                            }
                            if gf_id(self.frames[offset].group_frame_type) == GF_RECURSE {
                                break;
                            }
                            offset = self.frames[offset - 1].last_group_offset;
                        }
                        let p = offset - 1;
                        let (e, m, sm) = (
                            self.frames[f].eptr,
                            self.frames[f].mark,
                            self.frames[f].start_match,
                        );
                        self.frames[p].eptr = e;
                        self.frames[p].mark = m;
                        self.frames[p].start_match = sm;
                        self.truncate_frames(p);
                        self.frames[p].ecode += 1;
                        return Ok(Some(State::Exec));
                    }
                    return self.op_end(f, true);
                }
                Op::AssertAccept => {
                    if eptr > self.last_used_ptr {
                        self.last_used_ptr = eptr;
                    }
                    let ot = self.frames[f].offset_top;
                    self.accept_ov.clear();
                    let o = self.ov(f)[..ot.min(self.ovsize)].to_vec();
                    self.accept_ov = o;
                    self.accept_offset_top = ot;
                    self.accept_mark = self.frames[f].mark;
                    return Ok(Some(State::Return(MATCH_ACCEPT)));
                }
                Op::Close(n) => {
                    if self.frames[f].current_recurse == RECURSE_UNSET {
                        let number = *n as u32;
                        let mut offset = self.frames[f].last_group_offset;
                        loop {
                            if offset == UNSET {
                                return Err(MatchError::Internal);
                            }
                            if self.frames[offset].group_frame_type == (GF_CAPTURE | number) {
                                break;
                            }
                            offset = self.frames[offset - 1].last_group_offset;
                        }
                        let peptr = self.frames[offset - 1].eptr;
                        let o = (number as usize) * 2 - 2;
                        self.frames[f].capture_last = number;
                        let ov = self.ov_mut(f);
                        ov[o] = peptr;
                        ov[o + 1] = eptr;
                        if o >= self.frames[f].offset_top {
                            self.frames[f].offset_top = o + 2;
                        }
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Item(item) => {
                    let mut p = eptr;
                    if !self.match_one(item, &mut p) {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].eptr = p;
                    self.frames[f].ecode += 1;
                }
                Op::Rep {
                    item,
                    min,
                    max,
                    kind,
                } => {
                    let (min, max, kind) = (*min, *max, *kind);
                    let mut p = eptr;
                    for _ in 0..min {
                        if !self.match_one(item, &mut p) {
                            return Ok(Some(State::Return(MATCH_NOMATCH)));
                        }
                    }
                    self.frames[f].eptr = p;
                    if min == max {
                        self.frames[f].ecode += 1;
                        continue;
                    }
                    if kind == RepKind::Min {
                        self.frames[f].t32[0] = min;
                        self.frames[f].t32[1] = max;
                        self.rmatch(ecode + 1, Rid::RepMin, 0)?;
                        return Ok(Some(State::Exec));
                    }
                    let start_eptr = p;
                    let mut i = min;
                    while i < max {
                        if !self.match_one(item, &mut p) {
                            break;
                        }
                        i += 1;
                    }
                    self.frames[f].eptr = p;
                    if kind == RepKind::Pos {
                        self.frames[f].ecode += 1;
                        continue;
                    }
                    self.frames[f].t_sptr[0] = start_eptr;
                    if matches!(item, Item::Class(_)) {
                        // Class repeats also try the minimum position as a
                        // backtracking point and then fail (RM24/RM201/RM101).
                        self.rmatch(ecode + 1, Rid::RepMaxClass, 0)?;
                        return Ok(Some(State::Exec));
                    }
                    if p <= start_eptr {
                        self.frames[f].ecode += 1;
                        continue;
                    }
                    self.rmatch(ecode + 1, Rid::RepMax, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Ref {
                    group,
                    caseless,
                    rep,
                } => {
                    let offset = (*group as usize) * 2 - 2;
                    if let Some(st) = self.op_ref(f, offset, *caseless, *rep)? {
                        return Ok(Some(st));
                    }
                }
                Op::DnRef {
                    index,
                    count,
                    caseless,
                    rep,
                } => {
                    let mut offset = 0usize;
                    let ot = self.frames[f].offset_top;
                    for k in 0..*count {
                        let g = prog.name_table[(*index + k) as usize].0 as usize;
                        offset = g * 2 - 2;
                        if offset < ot && self.ov(f)[offset] != UNSET {
                            break;
                        }
                    }
                    if let Some(st) = self.op_ref(f, offset, *caseless, *rep)? {
                        return Ok(Some(st));
                    }
                }
                Op::BraZero => {
                    self.frames[f].t_sptr[0] = ecode + 1;
                    self.rmatch(ecode + 1, Rid::BraZero, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::BraMinZero => {
                    let ket = crate::study::bracket_ket(ops, ecode + 1);
                    self.rmatch(ket + 1, Rid::BraMinZero, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::SkipZero => {
                    self.frames[f].ecode = crate::study::skip_bracket(ops, ecode + 1);
                }
                Op::BraPosZero => {
                    self.frames[f].t32[2] = 1;
                    let bra = ecode + 1;
                    self.frames[f].ecode = bra;
                    let gft = match ops[bra] {
                        Op::Bra { kind, group, .. } if kind.is_capture() => {
                            GF_CAPTURE | group as u32
                        }
                        _ => GF_NOCAPTURE,
                    };
                    return self.pos_group_start(f, gft).map(Some);
                }
                Op::Bra { kind, next, group } => {
                    let (kind, next, group) = (*kind, *next, *group);
                    match kind {
                        BraKind::BraPos | BraKind::SBraPos => {
                            self.frames[f].t32[2] = 0;
                            return self.pos_group_start(f, GF_NOCAPTURE).map(Some);
                        }
                        BraKind::CBraPos | BraKind::SCBraPos => {
                            self.frames[f].t32[2] = 0;
                            return self.pos_group_start(f, GF_CAPTURE | group as u32).map(Some);
                        }
                        BraKind::Bra => {
                            if self.hasthen || f == 0 {
                                self.frames[f].t32[0] = 0;
                                self.rmatch(ecode + 1, Rid::GroupLoop, 0)?;
                                return Ok(Some(State::Exec));
                            }
                            let next_branch = ecode + next;
                            if !matches!(ops[next_branch], Op::Alt(_)) {
                                self.frames[f].ecode = ecode + 1;
                                continue;
                            }
                            self.frames[f].t_sptr[0] = next_branch;
                            self.rmatch(ecode + 1, Rid::BraFast, 0)?;
                            return Ok(Some(State::Exec));
                        }
                        BraKind::CBra | BraKind::SCBra => {
                            let ft = GF_CAPTURE | group as u32;
                            self.frames[f].t32[0] = ft;
                            self.rmatch(ecode + 1, Rid::GroupLoop, ft)?;
                            return Ok(Some(State::Exec));
                        }
                        BraKind::Once | BraKind::ScriptRun | BraKind::SBra => {
                            let ft = GF_NOCAPTURE | bra_code(kind);
                            self.frames[f].t32[0] = ft;
                            self.rmatch(ecode + 1, Rid::GroupLoop, ft)?;
                            return Ok(Some(State::Exec));
                        }
                        BraKind::Assert
                        | BraKind::AssertBack
                        | BraKind::AssertNa
                        | BraKind::AssertBackNa => {
                            let ft = GF_NOCAPTURE | bra_code(kind);
                            self.frames[f].t32[0] = ft;
                            self.rmatch(ecode + 1, Rid::Assert, ft)?;
                            return Ok(Some(State::Exec));
                        }
                        BraKind::AssertNot | BraKind::AssertBackNot => {
                            let ft = GF_NOCAPTURE | bra_code(kind);
                            self.frames[f].t32[0] = ft;
                            self.rmatch(ecode + 1, Rid::AssertNot, ft)?;
                            return Ok(Some(State::Exec));
                        }
                        BraKind::Cond | BraKind::SCond => {
                            if let Some(st) =
                                self.op_cond(f, ecode, next, kind == BraKind::SCond)?
                            {
                                return Ok(Some(st));
                            }
                        }
                    }
                }
                Op::Recurse { group, target } => {
                    let bracode = *target;
                    let number = if bracode == 0 { 0 } else { *group as u32 };
                    if self.frames[f].current_recurse != RECURSE_UNSET {
                        let mut offset = self.frames[f].last_group_offset;
                        while offset != UNSET {
                            let p = offset - 1;
                            if self.frames[offset].group_frame_type == (GF_RECURSE | number) {
                                if eptr == self.frames[p].eptr
                                    && self.last_used_ptr == self.frames[p].recurse_last_used
                                {
                                    return Err(MatchError::RecurseLoop);
                                }
                                break;
                            }
                            offset = self.frames[p].last_group_offset;
                        }
                    }
                    self.frames[f].recurse_last_used = self.last_used_ptr;
                    self.frames[f].t_sptr[0] = bracode;
                    self.frames[f].t32[0] = GF_RECURSE | number;
                    self.rmatch(bracode + 1, Rid::Recurse, GF_RECURSE | number)?;
                    return Ok(Some(State::Exec));
                }
                Op::Callout => self.frames[f].ecode += 1,
                Op::Reverse(number) => {
                    let mut p = eptr;
                    if self.utf {
                        for _ in 0..*number {
                            if p <= self.check_subject {
                                return Ok(Some(State::Return(MATCH_NOMATCH)));
                            }
                            p = self.backchar(p);
                        }
                    } else {
                        if *number as usize > p {
                            return Ok(Some(State::Return(MATCH_NOMATCH)));
                        }
                        p -= *number as usize;
                    }
                    if p < self.start_used_ptr {
                        self.start_used_ptr = p;
                    }
                    self.frames[f].eptr = p;
                    self.frames[f].ecode += 1;
                }
                Op::VReverse(lmin, lmax) => {
                    let (lmin, mut lmax) = (*lmin, *lmax);
                    let mut p = eptr;
                    if self.utf {
                        let mut i = 0;
                        while i < lmax {
                            if p == 0 {
                                if i < lmin {
                                    return Ok(Some(State::Return(MATCH_NOMATCH)));
                                }
                                lmax = i;
                                break;
                            }
                            p = self.backchar(p);
                            i += 1;
                        }
                    } else {
                        let available = p.min(65535) as u32;
                        if lmin > available {
                            return Ok(Some(State::Return(MATCH_NOMATCH)));
                        }
                        if lmax > available {
                            lmax = available;
                        }
                        p -= lmax as usize;
                    }
                    self.frames[f].eptr = p;
                    self.frames[f].t32[0] = lmin;
                    self.frames[f].t32[1] = lmax;
                    self.rmatch(ecode + 1, Rid::VReverse, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Alt(_) => {
                    self.branch_end = Some(ecode);
                    let mut e = ecode;
                    while let Op::Alt(n) = ops[e] {
                        e += n;
                    }
                    self.frames[f].ecode = e;
                }
                Op::Ket(ket_kind, back) => {
                    if let Some(st) = self.op_ket(f, ecode, *ket_kind, *back)? {
                        return Ok(Some(st));
                    }
                }
                Op::Circ => {
                    if eptr != 0 || self.moptions & PCRE2_NOTBOL != 0 {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Sod => {
                    if eptr != 0 {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Doll => {
                    if self.moptions & PCRE2_NOTEOL != 0 {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    if self.poptions & PCRE2_DOLLAR_ENDONLY == 0 {
                        if !self.assert_nl_or_eos(eptr) {
                            return Ok(Some(State::Return(MATCH_NOMATCH)));
                        }
                    } else if eptr < self.true_end_subject {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Eod => {
                    if eptr < self.true_end_subject {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Eodn => {
                    if !self.assert_nl_or_eos(eptr) {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::CircM => {
                    if self.moptions & PCRE2_NOTBOL != 0 && eptr == 0 {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    if eptr != 0
                        && ((eptr == self.end_subject && self.poptions & PCRE2_ALT_CIRCUMFLEX == 0)
                            || !self.was_newline(eptr))
                    {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::DollM => {
                    if eptr < self.end_subject {
                        if !self.is_newline(eptr) {
                            return Ok(Some(State::Return(MATCH_NOMATCH)));
                        }
                    } else if self.moptions & PCRE2_NOTEOL != 0 {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Som => {
                    if eptr != self.start_offset {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::SetSom => {
                    self.frames[f].start_match = eptr;
                    self.frames[f].ecode += 1;
                }
                Op::NotWordBoundary
                | Op::WordBoundary
                | Op::NotUcpWordBoundary
                | Op::UcpWordBoundary => {
                    let ucpb = matches!(ops[ecode], Op::NotUcpWordBoundary | Op::UcpWordBoundary);
                    let prev_is_word = if eptr == self.check_subject {
                        false
                    } else {
                        let lastptr = self.backchar(eptr);
                        let (fc, _) = self.getchar(lastptr);
                        if lastptr < self.start_used_ptr {
                            self.start_used_ptr = lastptr;
                        }
                        self.is_word(fc, ucpb)
                    };
                    let cur_is_word = if eptr >= self.end_subject {
                        false
                    } else {
                        let (fc, l) = self.getchar(eptr);
                        if eptr + l > self.last_used_ptr {
                            self.last_used_ptr = eptr + l;
                        }
                        self.is_word(fc, ucpb)
                    };
                    let want_boundary =
                        matches!(ops[ecode], Op::WordBoundary | Op::UcpWordBoundary);
                    if want_boundary == (cur_is_word == prev_is_word) {
                        return Ok(Some(State::Return(MATCH_NOMATCH)));
                    }
                    self.frames[f].ecode += 1;
                }
                Op::Mark(idx) => {
                    self.frames[f].mark = Some(*idx);
                    self.nomatch_mark = Some(*idx);
                    self.rmatch(ecode + 1, Rid::Mark, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Fail => return Ok(Some(State::Return(MATCH_NOMATCH))),
                Op::Commit => {
                    self.rmatch(ecode + 1, Rid::Commit, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::CommitArg(idx) => {
                    self.frames[f].mark = Some(*idx);
                    self.nomatch_mark = Some(*idx);
                    self.rmatch(ecode + 1, Rid::Commit, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Prune => {
                    self.rmatch(ecode + 1, Rid::Prune, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::PruneArg(idx) => {
                    self.frames[f].mark = Some(*idx);
                    self.nomatch_mark = Some(*idx);
                    self.rmatch(ecode + 1, Rid::Prune, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Skip => {
                    self.rmatch(ecode + 1, Rid::Skip, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::SkipArg(_) => {
                    self.skip_arg_count += 1;
                    if self.skip_arg_count <= self.ignore_skip_arg {
                        self.frames[f].ecode += 1;
                        continue;
                    }
                    self.rmatch(ecode + 1, Rid::SkipArg, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::Then => {
                    self.rmatch(ecode + 1, Rid::Then, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::ThenArg(idx) => {
                    self.frames[f].mark = Some(*idx);
                    self.nomatch_mark = Some(*idx);
                    self.rmatch(ecode + 1, Rid::Then, 0)?;
                    return Ok(Some(State::Exec));
                }
                Op::CRef(_)
                | Op::DnCRef(..)
                | Op::RRef(_)
                | Op::DnRRef(..)
                | Op::False
                | Op::True => {
                    // Conditions are consumed by op_cond.
                    return Err(MatchError::Internal);
                }
            }
        }
    }

    fn is_word(&self, c: u32, ucp: bool) -> bool {
        if ucp {
            let t = unicode::chartype(c);
            let g = unicode::gentype(t);
            g == unicode::GT_L || g == unicode::GT_N || t == unicode::UCP_MN || t == unicode::UCP_PC
        } else {
            c < 256 && chars::ctype(c) & CTYPE_WORD != 0
        }
    }

    /// ASSERT_NL_OR_EOS
    fn assert_nl_or_eos(&mut self, eptr: usize) -> bool {
        if eptr < self.end_subject
            && (!self.is_newline(eptr) || eptr != self.end_subject - self.nllen)
        {
            return false;
        }
        true
    }

    /// OP_END / OP_ACCEPT at top level. Returns None on a full match.
    fn op_end(&mut self, f: usize, _is_accept: bool) -> Result<Option<State>, MatchError> {
        let fr = &self.frames[f];
        let eptr = fr.eptr;
        if eptr == fr.start_match
            && (self.moptions & PCRE2_NOTEMPTY != 0
                || (self.moptions & PCRE2_NOTEMPTY_ATSTART != 0
                    && fr.start_match == self.start_offset))
        {
            return Ok(Some(State::Return(MATCH_NOMATCH)));
        }
        self.end_match_ptr = eptr;
        self.end_offset_top = fr.offset_top;
        self.mark = fr.mark;
        if eptr > self.last_used_ptr {
            self.last_used_ptr = eptr;
        }
        let ot = fr.offset_top;
        let sm = fr.start_match;
        let mut res = vec![UNSET; 2 + self.ovsize];
        res[0] = sm;
        res[1] = eptr;
        let n = ot.min(self.ovsize);
        res[2..2 + n].copy_from_slice(&self.ov(f)[..n]);
        self.result_ov = res;
        Ok(None)
    }

    /// Back reference matching (match_ref). Returns Some(length) or None.
    fn match_ref(&self, f: usize, offset: usize, caseless: bool, eptr: usize) -> Option<usize> {
        let fr = &self.frames[f];
        let ov = self.ov(f);
        if offset >= fr.offset_top || ov[offset] == UNSET {
            if self.poptions & PCRE2_MATCH_UNSET_BACKREF != 0 {
                return Some(0);
            }
            return None;
        }
        let mut e = eptr;
        let mut p = ov[offset];
        let length = ov[offset + 1] - ov[offset];
        let s = self.s;
        if caseless {
            if self.utf || self.ucp {
                let endptr = p + length;
                while p < endptr {
                    if e >= self.end_subject {
                        return None;
                    }
                    let (c, cl) = self.getchar(e);
                    let (d, dl) = self.getchar(p);
                    e += cl;
                    p += dl;
                    if c != d && c != unicode::other_case(d) {
                        match unicode::caseless_set(d) {
                            Some(set) if set.contains(&c) => {}
                            _ => return None,
                        }
                    }
                }
            } else {
                for _ in 0..length {
                    if e >= self.end_subject {
                        return None;
                    }
                    if chars::lcc(s[p] as u32) != chars::lcc(s[e] as u32) {
                        return None;
                    }
                    p += 1;
                    e += 1;
                }
            }
        } else {
            if self.end_subject - e < length {
                return None;
            }
            if s[p..p + length] != s[e..e + length] {
                return None;
            }
            e += length;
        }
        Some(e - eptr)
    }

    fn op_ref(
        &mut self,
        f: usize,
        offset: usize,
        caseless: bool,
        rep: Option<(u32, u32, RepKind)>,
    ) -> Result<Option<State>, MatchError> {
        let ecode = self.frames[f].ecode;
        let Some((lmin, lmax, kind)) = rep else {
            let eptr = self.frames[f].eptr;
            match self.match_ref(f, offset, caseless, eptr) {
                Some(l) => {
                    self.frames[f].eptr = eptr + l;
                    self.frames[f].ecode += 1;
                    return Ok(None);
                }
                None => return Ok(Some(State::Return(MATCH_NOMATCH))),
            }
        };
        let ot = self.frames[f].offset_top;
        if offset < ot && self.ov(f)[offset] != UNSET {
            if self.ov(f)[offset] == self.ov(f)[offset + 1] {
                self.frames[f].ecode += 1;
                return Ok(None);
            }
        } else if lmin == 0 || self.poptions & PCRE2_MATCH_UNSET_BACKREF != 0 {
            self.frames[f].ecode += 1;
            return Ok(None);
        }
        let mut eptr = self.frames[f].eptr;
        for _ in 0..lmin {
            match self.match_ref(f, offset, caseless, eptr) {
                Some(l) => eptr += l,
                None => return Ok(Some(State::Return(MATCH_NOMATCH))),
            }
        }
        self.frames[f].eptr = eptr;
        if lmin == lmax {
            self.frames[f].ecode += 1;
            return Ok(None);
        }
        self.frames[f].t32[0] = lmin;
        self.frames[f].t32[1] = lmax;
        self.frames[f].t32[2] = caseless as u32;
        self.frames[f].t_size = offset;
        if kind == RepKind::Min {
            self.rmatch(ecode + 1, Rid::RefMin, 0)?;
            return Ok(Some(State::Exec));
        }
        let start = eptr;
        self.frames[f].t_sptr[0] = start;
        let flength = self.ov(f)[offset + 1] - self.ov(f)[offset];
        self.frames[f].length = flength;
        let mut samelengths = true;
        let mut i = lmin;
        while i < lmax {
            match self.match_ref(f, offset, caseless, eptr) {
                Some(l) => {
                    if l != flength {
                        samelengths = false;
                    }
                    eptr += l;
                }
                None => break,
            }
            i += 1;
        }
        self.frames[f].eptr = eptr;
        if samelengths {
            // while (Feptr >= Lstart) { RMATCH; Feptr -= Flength }
            self.rmatch(ecode + 1, Rid::RefMaxSame, 0)?;
            Ok(Some(State::Exec))
        } else {
            self.frames[f].t32[1] = i;
            self.rmatch(ecode + 1, Rid::RefMaxDiff, 0)?;
            Ok(Some(State::Exec))
        }
    }

    /// Start of a possessive group (POSSESSIVE_GROUP). `t32[2]` holds
    /// Lzero_allowed and the frame's ecode points at the bracket.
    fn pos_group_start(&mut self, f: usize, frame_type: u32) -> Result<State, MatchError> {
        let ecode = self.frames[f].ecode;
        self.frames[f].t32[0] = frame_type;
        self.frames[f].t32[1] = 0;
        self.frames[f].t_sptr[1] = ecode;
        self.frames[f].t_sptr[0] = self.frames[f].eptr;
        self.rmatch(ecode + 1, Rid::PosGroup, frame_type)?;
        Ok(State::Exec)
    }

    /// OP_COND / OP_SCOND.
    fn op_cond(
        &mut self,
        f: usize,
        ecode: usize,
        next: usize,
        scond: bool,
    ) -> Result<Option<State>, MatchError> {
        let ops = &self.prog.ops;
        let false_target = if matches!(ops[ecode + next], Op::Alt(_)) {
            ecode + next + 1
        } else {
            ecode + next
        };
        let mut c = ecode + 1;
        if matches!(ops[c], Op::Callout) {
            c += 1;
        }
        let ov_top = self.frames[f].offset_top;
        let condition = match &ops[c] {
            Op::RRef(n) => {
                let cr = self.frames[f].current_recurse;
                cr != RECURSE_UNSET && (*n == RREF_ANY || *n as u32 == cr)
            }
            Op::DnRRef(index, count) => {
                let cr = self.frames[f].current_recurse;
                let mut cond = false;
                if cr != RECURSE_UNSET {
                    for k in 0..*count {
                        let g = self.prog.name_table[(*index + k) as usize].0 as u32;
                        if g == cr {
                            cond = true;
                            break;
                        }
                    }
                }
                cond
            }
            Op::CRef(n) => {
                let o = (*n as usize) * 2 - 2;
                o < ov_top && self.ov(f)[o] != UNSET
            }
            Op::DnCRef(index, count) => {
                let mut cond = false;
                for k in 0..*count {
                    let g = self.prog.name_table[(*index + k) as usize].0 as usize;
                    let o = g * 2 - 2;
                    if o < ov_top && self.ov(f)[o] != UNSET {
                        cond = true;
                        break;
                    }
                }
                cond
            }
            Op::False | Op::Fail => false,
            Op::True => true,
            Op::Bra { kind, .. } => {
                // Assertion condition.
                let positive = matches!(kind, BraKind::Assert | BraKind::AssertBack);
                self.frames[f].t32[0] = positive as u32;
                self.frames[f].t32[1] = scond as u32;
                self.frames[f].t_sptr[0] = c;
                self.frames[f].t_sptr[1] = c;
                self.frames[f].length = false_target;
                let ft = GF_CONDASSERT | bra_code(*kind);
                self.rmatch(c + 1, Rid::CondAssert, ft)?;
                return Ok(Some(State::Exec));
            }
            _ => return Err(MatchError::Internal),
        };
        let target = if condition { c + 1 } else { false_target };
        self.cond_continue(f, target, scond)
    }

    /// Choose the branch of a condition; for SCOND descend a level.
    fn cond_continue(
        &mut self,
        f: usize,
        target: usize,
        scond: bool,
    ) -> Result<Option<State>, MatchError> {
        if scond {
            self.rmatch(target, Rid::SCond, GF_NOCAPTURE | bra_code(BraKind::SCond))?;
            return Ok(Some(State::Exec));
        }
        self.frames[f].ecode = target;
        Ok(None)
    }

    /// OP_KET and friends.
    fn op_ket(
        &mut self,
        f: usize,
        ecode: usize,
        ket_kind: KetKind,
        back: usize,
    ) -> Result<Option<State>, MatchError> {
        let ops = &self.prog.ops;
        let bracode = ecode - back;
        let branch_end = self.branch_end.take().unwrap_or(ecode);
        let bra_kind = match ops[bracode] {
            Op::Bra { kind, .. } => kind,
            _ => return Err(MatchError::Internal),
        };
        let mut p: Option<usize> = None;
        if bra_kind != BraKind::Bra && bra_kind != BraKind::Cond {
            let n = self.frames[f].last_group_offset;
            if n == UNSET {
                return Err(MatchError::Internal);
            }
            let pp = n - 1;
            self.frames[f].last_group_offset = self.frames[pp].last_group_offset;
            if gf_id(self.frames[n].group_frame_type) == GF_CONDASSERT {
                let ot = self.frames[f].offset_top;
                self.copy_ov(f, pp, ot);
                self.frames[pp].offset_top = ot;
                self.frames[pp].mark = self.frames[f].mark;
                self.frames[f].back_frame = f - pp;
                return Ok(Some(State::Return(MATCH_MATCH)));
            }
            p = Some(pp);
        }

        let vreverse_check = |this: &Self, pp: usize| -> bool {
            // Find the start of the branch that just ended.
            let mut bs = bracode;
            loop {
                let nx = match ops[bs] {
                    Op::Bra { next, .. } | Op::Alt(next) => next,
                    _ => return true,
                };
                if bs + nx == branch_end {
                    break;
                }
                bs += nx;
            }
            !(matches!(ops[bs + 1], Op::VReverse(..))
                && this.frames[f].eptr != this.frames[pp].eptr)
        };

        match bra_kind {
            BraKind::Bra => {
                if self.frames[f].current_recurse == 0 && matches!(ops[ecode + 1], Op::End) {
                    let offset = self.frames[f].last_group_offset;
                    if offset == UNSET {
                        return Err(MatchError::Internal);
                    }
                    let pp = offset - 1;
                    self.frames[f].last_group_offset = self.frames[pp].last_group_offset;
                    let ot = self.frames[f].offset_top;
                    self.copy_ov(pp, f, ot);
                    self.frames[f].offset_top = self.frames[pp].offset_top;
                    self.frames[f].capture_last = self.frames[pp].capture_last;
                    self.frames[f].current_recurse = self.frames[pp].current_recurse;
                    self.frames[f].ecode = self.frames[pp].ecode + 1;
                    return Ok(None);
                }
            }
            BraKind::Cond | BraKind::SCond => {}
            BraKind::AssertBackNa | BraKind::AssertNa => {
                let pp = p.unwrap();
                if bra_kind == BraKind::AssertBackNa && !vreverse_check(self, pp) {
                    return Ok(Some(State::Return(MATCH_NOMATCH)));
                }
                if self.frames[f].eptr > self.last_used_ptr {
                    self.last_used_ptr = self.frames[f].eptr;
                }
                self.frames[f].eptr = self.frames[pp].eptr;
            }
            BraKind::AssertBack | BraKind::Assert | BraKind::Once => {
                let pp = p.unwrap();
                if bra_kind == BraKind::AssertBack && !vreverse_check(self, pp) {
                    return Ok(Some(State::Return(MATCH_NOMATCH)));
                }
                if bra_kind != BraKind::Once {
                    if self.frames[f].eptr > self.last_used_ptr {
                        self.last_used_ptr = self.frames[f].eptr;
                    }
                    self.frames[f].eptr = self.frames[pp].eptr;
                }
                self.frames[f].back_frame = f - pp;
                // Make the remaining branches of the group not be tried.
                loop {
                    let pe = self.frames[pp].ecode;
                    let y = match ops[pe] {
                        Op::Bra { next, .. } | Op::Alt(next) => next,
                        _ => break,
                    };
                    if !matches!(ops[pe + y], Op::Alt(_)) {
                        break;
                    }
                    self.frames[pp].ecode = pe + y;
                }
            }
            BraKind::AssertBackNot | BraKind::AssertNot => {
                let pp = p.unwrap();
                if bra_kind == BraKind::AssertBackNot && !vreverse_check(self, pp) {
                    return Ok(Some(State::Return(MATCH_NOMATCH)));
                }
                return Ok(Some(State::Return(MATCH_MATCH)));
            }
            BraKind::ScriptRun => {
                let pp = p.unwrap();
                let (from, to) = (self.frames[pp].eptr, self.frames[f].eptr);
                if !crate::scriptrun::script_run(self.s, from, to, self.utf) {
                    return Ok(Some(State::Return(MATCH_NOMATCH)));
                }
            }
            BraKind::CBra | BraKind::CBraPos | BraKind::SCBra | BraKind::SCBraPos => {
                let pp = p.unwrap();
                let number = match ops[bracode] {
                    Op::Bra { group, .. } => group as u32,
                    _ => 0,
                };
                if self.frames[f].current_recurse == number {
                    // pp is the frame before the recursion frame.
                    let ot = self.frames[f].offset_top;
                    self.copy_ov(pp, f, ot);
                    self.frames[f].offset_top = self.frames[pp].offset_top;
                    self.frames[f].capture_last = self.frames[pp].capture_last;
                    self.frames[f].current_recurse = self.frames[pp].current_recurse;
                    self.frames[f].ecode = self.frames[pp].ecode + 1;
                    return Ok(None);
                }
                let o = (number as usize) * 2 - 2;
                let peptr = self.frames[pp].eptr;
                let eptr = self.frames[f].eptr;
                self.frames[f].capture_last = number;
                let ov = self.ov_mut(f);
                ov[o] = peptr;
                ov[o + 1] = eptr;
                if o >= self.frames[f].offset_top {
                    self.frames[f].offset_top = o + 2;
                }
            }
            _ => {}
        }

        if ket_kind == KetKind::KetRPos {
            let pp = p.ok_or(MatchError::Internal)?;
            let src = self.frames[f].clone();
            let dst = &mut self.frames[pp];
            dst.eptr = src.eptr;
            dst.start_match = src.start_match;
            dst.mark = src.mark;
            dst.recurse_last_used = src.recurse_last_used;
            dst.current_recurse = src.current_recurse;
            dst.capture_last = src.capture_last;
            dst.last_group_offset = src.last_group_offset;
            dst.offset_top = src.offset_top;
            let os = self.ovsize;
            self.copy_ov(f, pp, os);
            return Ok(Some(State::Return(MATCH_KETRPOS)));
        }

        if ket_kind != KetKind::Ket
            && (p.is_none() || self.frames[f].eptr != self.frames[p.unwrap()].eptr)
        {
            if ket_kind == KetKind::KetRMin {
                self.rmatch(ecode + 1, Rid::KetRMin, 0)?;
                return Ok(Some(State::Exec));
            }
            self.rmatch(bracode, Rid::KetRMax, 0)?;
            return Ok(Some(State::Exec));
        }
        self.frames[f].ecode = ecode + 1;
        Ok(None)
    }

    // ------------------------------------------------------------------
    // Resuming after a backtracking return
    // ------------------------------------------------------------------

    fn then_check(&self, rrc: i32, ecode: usize) -> i32 {
        if rrc == MATCH_THEN {
            let ops = &self.prog.ops;
            let next_ecode = ecode + crate::study::next_of(&ops[ecode]);
            if self.verb_ecode_ptr < next_ecode
                && (matches!(ops[ecode], Op::Alt(_)) || matches!(ops[next_ecode], Op::Alt(_)))
            {
                return MATCH_NOMATCH;
            }
        }
        rrc
    }

    fn resume(&mut self, rrc_in: i32) -> Result<State, MatchError> {
        let prog = self.prog;
        let ops = &prog.ops;
        let f = self.top();
        let mut rrc = rrc_in;
        let ecode = self.frames[f].ecode;
        match self.frames[f].return_id {
            Rid::None => Err(MatchError::Internal),
            Rid::BraFast => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let e = self.frames[f].t_sptr[0];
                self.frames[f].ecode = e;
                let next_branch = e + crate::study::next_of(&ops[e]);
                if !matches!(ops[next_branch], Op::Alt(_)) {
                    self.frames[f].ecode = e + 1;
                    return Ok(State::Exec);
                }
                self.frames[f].t_sptr[0] = next_branch;
                self.rmatch(e + 1, Rid::BraFast, 0)?;
                Ok(State::Exec)
            }
            Rid::GroupLoop => {
                rrc = self.then_check(rrc, ecode);
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let e = ecode + crate::study::next_of(&ops[ecode]);
                self.frames[f].ecode = e;
                if !matches!(ops[e], Op::Alt(_)) {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                let ft = self.frames[f].t32[0];
                self.rmatch(e + 1, Rid::GroupLoop, ft)?;
                Ok(State::Exec)
            }
            Rid::Assert => {
                if rrc == MATCH_ACCEPT {
                    let n = self.accept_offset_top.min(self.ovsize);
                    let acc = std::mem::take(&mut self.accept_ov);
                    self.ov_mut(f)[..n].copy_from_slice(&acc[..n]);
                    self.accept_ov = acc;
                    self.frames[f].offset_top = self.accept_offset_top;
                    self.frames[f].mark = self.accept_mark;
                    let ket = crate::study::bracket_ket(ops, ecode);
                    self.frames[f].ecode = ket + 1;
                    return Ok(State::Exec);
                }
                if rrc != MATCH_NOMATCH && rrc != MATCH_THEN {
                    return Ok(State::Return(rrc));
                }
                let e = ecode + crate::study::next_of(&ops[ecode]);
                self.frames[f].ecode = e;
                if !matches!(ops[e], Op::Alt(_)) {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                let ft = self.frames[f].t32[0];
                self.rmatch(e + 1, Rid::Assert, ft)?;
                Ok(State::Exec)
            }
            Rid::AssertNot => match rrc {
                MATCH_ACCEPT | MATCH_MATCH => Ok(State::Return(MATCH_NOMATCH)),
                MATCH_NOMATCH | MATCH_THEN => {
                    let e = ecode + crate::study::next_of(&ops[ecode]);
                    if !matches!(ops[e], Op::Alt(_)) {
                        self.frames[f].ecode = e + 1;
                        return Ok(State::Exec);
                    }
                    self.frames[f].ecode = e;
                    let ft = self.frames[f].t32[0];
                    self.rmatch(e + 1, Rid::AssertNot, ft)?;
                    Ok(State::Exec)
                }
                MATCH_COMMIT | MATCH_SKIP | MATCH_PRUNE => {
                    let ket = crate::study::bracket_ket(ops, ecode);
                    self.frames[f].ecode = ket + 1;
                    Ok(State::Exec)
                }
                _ => Ok(State::Return(rrc)),
            },
            Rid::CondAssert => {
                let positive = self.frames[f].t32[0] != 0;
                let scond = self.frames[f].t32[1] != 0;
                let cond_start = self.frames[f].t_sptr[1];

                let condition = match rrc {
                    MATCH_ACCEPT | MATCH_MATCH => {
                        if rrc == MATCH_ACCEPT {
                            let n = self.accept_offset_top.min(self.ovsize);
                            let acc = std::mem::take(&mut self.accept_ov);
                            self.ov_mut(f)[..n].copy_from_slice(&acc[..n]);
                            self.accept_ov = acc;
                            self.frames[f].offset_top = self.accept_offset_top;
                        }
                        positive
                    }
                    MATCH_NOMATCH | MATCH_THEN => {
                        let sb = self.frames[f].t_sptr[0];
                        let nb = sb + crate::study::next_of(&ops[sb]);
                        if matches!(ops[nb], Op::Alt(_)) {
                            self.frames[f].t_sptr[0] = nb;
                            let ft = GF_CONDASSERT
                                | match ops[cond_start] {
                                    Op::Bra { kind, .. } => bra_code(kind),
                                    _ => 0,
                                };
                            self.rmatch(nb + 1, Rid::CondAssert, ft)?;
                            return Ok(State::Exec);
                        }
                        !positive
                    }
                    MATCH_COMMIT | MATCH_SKIP | MATCH_PRUNE => !positive,
                    _ => return Ok(State::Return(rrc)),
                };
                let target = if condition {
                    crate::study::bracket_ket(ops, cond_start) + 1
                } else {
                    self.frames[f].length
                };
                match self.cond_continue(f, target, scond)? {
                    Some(st) => Ok(st),
                    None => Ok(State::Exec),
                }
            }
            Rid::SCond => Ok(State::Return(rrc)),
            Rid::KetRMin => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let back = match ops[ecode] {
                    Op::Ket(_, b) => b,
                    _ => return Err(MatchError::Internal),
                };
                self.frames[f].ecode = ecode - back;
                Ok(State::Exec)
            }
            Rid::KetRMax => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.frames[f].ecode = ecode + 1;
                Ok(State::Exec)
            }
            Rid::PosGroup => {
                if rrc == MATCH_KETRPOS {
                    self.frames[f].t32[1] = 1;
                    if self.frames[f].eptr == self.frames[f].t_sptr[0] {
                        let ket = crate::study::bracket_ket(ops, ecode);
                        return self.pos_group_end(f, ket);
                    }
                    let sg = self.frames[f].t_sptr[1];
                    self.frames[f].ecode = sg;
                    self.frames[f].t_sptr[0] = self.frames[f].eptr;
                    let ft = self.frames[f].t32[0];
                    self.rmatch(sg + 1, Rid::PosGroup, ft)?;
                    return Ok(State::Exec);
                }
                rrc = self.then_check(rrc, ecode);
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let e = ecode + crate::study::next_of(&ops[ecode]);
                self.frames[f].ecode = e;
                if !matches!(ops[e], Op::Alt(_)) {
                    return self.pos_group_end(f, e);
                }
                self.frames[f].t_sptr[0] = self.frames[f].eptr;
                let ft = self.frames[f].t32[0];
                self.rmatch(e + 1, Rid::PosGroup, ft)?;
                Ok(State::Exec)
            }
            Rid::BraZero => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let bra = self.frames[f].t_sptr[0];
                self.frames[f].ecode = crate::study::skip_bracket(ops, bra);
                Ok(State::Exec)
            }
            Rid::BraMinZero => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.frames[f].ecode = ecode + 1;
                Ok(State::Exec)
            }
            Rid::Recurse => {
                let sb = self.frames[f].t_sptr[0];
                let next_ecode = sb + crate::study::next_of(&ops[sb]);
                let number = gf_data(self.frames[f].t32[0]);
                if (MATCH_BACKTRACK_MIN..=MATCH_BACKTRACK_MAX).contains(&rrc)
                    && self.verb_current_recurse == number
                {
                    if rrc == MATCH_THEN
                        && self.verb_ecode_ptr < next_ecode
                        && (matches!(ops[sb], Op::Alt(_)) || matches!(ops[next_ecode], Op::Alt(_)))
                    {
                        rrc = MATCH_NOMATCH;
                    } else {
                        return Ok(State::Return(MATCH_NOMATCH));
                    }
                }
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                if !matches!(ops[next_ecode], Op::Alt(_)) {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].t_sptr[0] = next_ecode;
                let ft = self.frames[f].t32[0];
                self.rmatch(next_ecode + 1, Rid::Recurse, ft)?;
                Ok(State::Exec)
            }
            Rid::Mark => {
                if rrc == MATCH_SKIP_ARG
                    && let Op::Mark(idx) = ops[ecode]
                    && prog.marks[idx as usize] == prog.marks[self.verb_skip_name as usize]
                {
                    self.verb_skip_pos = self.frames[f].eptr;
                    return Ok(State::Return(MATCH_SKIP));
                }
                Ok(State::Return(rrc))
            }
            Rid::Commit => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.verb_current_recurse = self.frames[f].current_recurse;
                Ok(State::Return(MATCH_COMMIT))
            }
            Rid::Prune => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.verb_current_recurse = self.frames[f].current_recurse;
                Ok(State::Return(MATCH_PRUNE))
            }
            Rid::Skip => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.verb_skip_pos = self.frames[f].eptr;
                self.verb_current_recurse = self.frames[f].current_recurse;
                Ok(State::Return(MATCH_SKIP))
            }
            Rid::SkipArg => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                if let Op::SkipArg(idx) = ops[ecode] {
                    self.verb_skip_name = idx;
                }
                self.verb_current_recurse = self.frames[f].current_recurse;
                Ok(State::Return(MATCH_SKIP_ARG))
            }
            Rid::Then => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                self.verb_ecode_ptr = ecode;
                self.verb_current_recurse = self.frames[f].current_recurse;
                Ok(State::Return(MATCH_THEN))
            }
            Rid::RepMin => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let lmin = self.frames[f].t32[0];
                if lmin >= self.frames[f].t32[1] {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].t32[0] = lmin + 1;
                let item = match &ops[ecode] {
                    Op::Rep { item, .. } => item,
                    _ => return Err(MatchError::Internal),
                };
                let mut p = self.frames[f].eptr;
                if !self.match_one(item, &mut p) {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].eptr = p;
                self.rmatch(ecode + 1, Rid::RepMin, 0)?;
                Ok(State::Exec)
            }
            Rid::RepMax => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let start = self.frames[f].t_sptr[0];
                let item = match &ops[ecode] {
                    Op::Rep { item, .. } => item,
                    _ => return Err(MatchError::Internal),
                };
                let p = self.backup_one(item, self.frames[f].eptr, start);
                self.frames[f].eptr = p;
                if p <= start {
                    self.frames[f].ecode = ecode + 1;
                    return Ok(State::Exec);
                }
                self.rmatch(ecode + 1, Rid::RepMax, 0)?;
                Ok(State::Exec)
            }
            Rid::RepMaxClass => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let start = self.frames[f].t_sptr[0];
                let eptr = self.frames[f].eptr;
                if eptr <= start {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].eptr = self.backchar(eptr);
                self.rmatch(ecode + 1, Rid::RepMaxClass, 0)?;
                Ok(State::Exec)
            }
            Rid::RefMin => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let lmin = self.frames[f].t32[0];
                if lmin >= self.frames[f].t32[1] {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].t32[0] = lmin + 1;
                let offset = self.frames[f].t_size;
                let caseless = self.frames[f].t32[2] != 0;
                let eptr = self.frames[f].eptr;
                match self.match_ref(f, offset, caseless, eptr) {
                    Some(l) => self.frames[f].eptr = eptr + l,
                    None => return Ok(State::Return(MATCH_NOMATCH)),
                }
                self.rmatch(ecode + 1, Rid::RefMin, 0)?;
                Ok(State::Exec)
            }
            Rid::RefMaxSame => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let start = self.frames[f].t_sptr[0];
                let flength = self.frames[f].length;
                let eptr = self.frames[f].eptr;
                // Feptr -= Flength; loop while Feptr >= Lstart.
                if eptr < start + flength {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                let ne = eptr - flength;
                self.frames[f].eptr = ne;
                self.rmatch(ecode + 1, Rid::RefMaxSame, 0)?;
                Ok(State::Exec)
            }
            Rid::RefMaxDiff => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let start = self.frames[f].t_sptr[0];
                if self.frames[f].eptr == start {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                let lmin = self.frames[f].t32[0];
                let lmax = self.frames[f].t32[1] - 1;
                self.frames[f].t32[1] = lmax;
                let offset = self.frames[f].t_size;
                let caseless = self.frames[f].t32[2] != 0;
                let mut e = start;
                let mut i = lmin;
                while i < lmax {
                    if let Some(l) = self.match_ref(f, offset, caseless, e) {
                        e += l;
                    }
                    i += 1;
                }
                self.frames[f].eptr = e;
                self.rmatch(ecode + 1, Rid::RefMaxDiff, 0)?;
                Ok(State::Exec)
            }
            Rid::VReverse => {
                if rrc != MATCH_NOMATCH {
                    return Ok(State::Return(rrc));
                }
                let lmin = self.frames[f].t32[0];
                let lmax = self.frames[f].t32[1];
                if lmax <= lmin {
                    return Ok(State::Return(MATCH_NOMATCH));
                }
                self.frames[f].t32[1] = lmax - 1;
                let mut p = self.frames[f].eptr + 1;
                if self.utf {
                    while p < self.end_subject && self.s[p] & 0xc0 == 0x80 {
                        p += 1;
                    }
                }
                self.frames[f].eptr = p;
                self.rmatch(ecode + 1, Rid::VReverse, 0)?;
                Ok(State::Exec)
            }
        }
    }

    fn pos_group_end(&mut self, f: usize, ket: usize) -> Result<State, MatchError> {
        if self.frames[f].t32[1] != 0 || self.frames[f].t32[2] != 0 {
            self.frames[f].ecode = ket + 1;
            return Ok(State::Exec);
        }
        Ok(State::Return(MATCH_NOMATCH))
    }

    // ------------------------------------------------------------------
    // Single character matching
    // ------------------------------------------------------------------

    /// Match one item at `*p`, advancing it. Returns false on no match.
    #[inline]
    fn match_one(&mut self, item: &Item, p: &mut usize) -> bool {
        let e = *p;
        if e >= self.end_subject {
            return false;
        }
        match item {
            Item::Char(c) => {
                let (fc, l) = self.getchar(e);
                if fc != *c {
                    return false;
                }
                *p = e + l;
            }
            Item::CharI(c, oc) => {
                let (fc, l) = self.getchar(e);
                if fc != *c && fc != *oc {
                    return false;
                }
                *p = e + l;
            }
            Item::Not(c) => {
                let (fc, l) = self.getchar(e);
                if fc == *c {
                    return false;
                }
                *p = e + l;
            }
            Item::NotI(c, oc) => {
                let (fc, l) = self.getchar(e);
                if fc == *c || fc == *oc {
                    return false;
                }
                *p = e + l;
            }
            Item::NotDigit
            | Item::Digit
            | Item::NotWhitespace
            | Item::Whitespace
            | Item::NotWordchar
            | Item::Wordchar => {
                let (fc, l) = self.getchar(e);
                let (bit, want) = match item {
                    Item::NotDigit => (CTYPE_DIGIT, false),
                    Item::Digit => (CTYPE_DIGIT, true),
                    Item::NotWhitespace => (CTYPE_SPACE, false),
                    Item::Whitespace => (CTYPE_SPACE, true),
                    Item::NotWordchar => (CTYPE_WORD, false),
                    _ => (CTYPE_WORD, true),
                };
                let has = fc < 256 && chars::ctype(fc) & bit != 0;
                if has != want {
                    return false;
                }
                *p = e + l;
            }
            Item::Any => {
                if self.is_newline(e) {
                    return false;
                }
                let (_, l) = self.getchar(e);
                *p = e + l;
            }
            Item::AllAny => {
                let (_, l) = self.getchar(e);
                *p = e + l;
            }
            Item::AnyByte => *p = e + 1,
            Item::Prop(t, v) | Item::NotProp(t, v) => {
                let (fc, l) = self.getchar(e);
                let m = unicode_prop_matches(fc, *t, *v);
                if m != matches!(item, Item::Prop(..)) {
                    return false;
                }
                *p = e + l;
            }
            Item::AnyNl => {
                let (fc, l) = self.getchar(e);
                let mut np = e + l;
                match fc {
                    0x0d => {
                        if np < self.end_subject && self.s[np] == b'\n' {
                            np += 1;
                        }
                    }
                    0x0a => {}
                    0x0b | 0x0c | 0x85 | 0x2028 | 0x2029 => {
                        if self.bsr_anycrlf {
                            return false;
                        }
                    }
                    _ => return false,
                }
                *p = np;
            }
            Item::NotHspace | Item::Hspace => {
                let (fc, l) = self.getchar(e);
                if chars::is_hspace(fc) != matches!(item, Item::Hspace) {
                    return false;
                }
                *p = e + l;
            }
            Item::NotVspace | Item::Vspace => {
                let (fc, l) = self.getchar(e);
                if chars::is_vspace(fc) != matches!(item, Item::Vspace) {
                    return false;
                }
                *p = e + l;
            }
            Item::Extuni => {
                let (fc, l) = self.getchar(e);
                *p = self.extuni(fc, e + l);
            }
            Item::Class(cl) => {
                let (fc, l) = self.getchar(e);
                if !cl.matches(fc) {
                    return false;
                }
                *p = e + l;
            }
        }
        true
    }

    /// Back up over one matched item during greedy-repeat backtracking.
    fn backup_one(&self, item: &Item, eptr: usize, start: usize) -> usize {
        match item {
            Item::AnyByte => eptr - 1,
            Item::AnyNl => {
                let mut p = self.backchar(eptr);
                if p > start && self.s[p] == b'\n' && self.s[p - 1] == b'\r' {
                    p -= 1;
                }
                p
            }
            Item::Extuni => {
                let mut p = self.backchar(eptr);
                let (fc, _) = self.getchar(p);
                let mut rgb = unicode::gbprop(fc);
                loop {
                    if p <= start {
                        break;
                    }
                    let fptr = self.backchar(p);
                    let (fc2, _) = self.getchar(fptr);
                    let lgb = unicode::gbprop(fc2);
                    if unicode::GB_TABLE[lgb as usize] & (1u32 << rgb) == 0 {
                        break;
                    }
                    p = fptr;
                    rgb = lgb;
                }
                p
            }
            _ => self.backchar(eptr),
        }
    }

    /// PRIV(extuni): match the rest of an extended grapheme cluster.
    fn extuni(&self, c: u32, mut eptr: usize) -> usize {
        let mut lgb = unicode::gbprop(c);
        while eptr < self.end_subject {
            let (c, len) = self.getchar(eptr);
            let rgb = unicode::gbprop(c);
            if unicode::GB_TABLE[lgb as usize] & (1u32 << rgb) == 0 {
                break;
            }
            if lgb == unicode::GB_REGIONAL_INDICATOR && rgb == unicode::GB_REGIONAL_INDICATOR {
                let mut ricount = 0;
                let mut bptr = self.backchar(eptr);
                while bptr > 0 {
                    bptr = self.backchar(bptr);
                    let (c2, _) = self.getchar(bptr);
                    if unicode::gbprop(c2) != unicode::GB_REGIONAL_INDICATOR {
                        break;
                    }
                    ricount += 1;
                }
                if ricount & 1 != 0 {
                    break;
                }
            }
            if (rgb != unicode::GB_EXTEND && rgb != unicode::GB_ZWJ)
                || lgb != unicode::GB_EXTENDED_PICTOGRAPHIC
            {
                lgb = rgb;
            }
            eptr += len;
        }
        eptr
    }
}

/// The opcode value used in group frame types (only needs to be distinct).
fn bra_code(k: BraKind) -> u32 {
    127 + k as u32
}

// ----------------------------------------------------------------------
// pcre2_match(): the driver
// ----------------------------------------------------------------------

/// Run a match. Returns Ok(None) for no match.
pub(crate) fn pcre2_match(
    prog: &Program,
    subject: &[u8],
    start_offset: usize,
    opts: ExecOptions,
) -> Result<(Option<ExecResult>, Option<u16>), MatchError> {
    let length = subject.len();
    if start_offset > length {
        return Err(MatchError::BadOffset);
    }
    let mut options = opts.moptions;
    if prog.flags & FLAG_NOTEMPTY_SET != 0 {
        options |= PCRE2_NOTEMPTY;
    }
    if prog.flags & FLAG_NE_ATST_SET != 0 {
        options |= PCRE2_NOTEMPTY_ATSTART;
    }
    let utf = prog.utf;
    let start_match = start_offset;
    let end_subject = length;

    let mut check_subject = 0usize;
    if utf && !opts.no_utf_check {
        if start_match < end_subject && subject[start_match] & 0xc0 == 0x80 {
            if start_offset > 0 {
                return Err(MatchError::BadUtf8Offset);
            }
            return Err(MatchError::BadUtf8 {
                code: -22,
                offset: 0,
            });
        }
        check_subject = start_match;
        let mut i = prog.max_lookbehind;
        while i > 0 && check_subject > 0 {
            check_subject -= 1;
            while check_subject > 0 && subject[check_subject] & 0xc0 == 0x80 {
                check_subject -= 1;
            }
            i -= 1;
        }
        if let Err((code, off)) = chars::valid_utf8(&subject[check_subject..]) {
            return Err(MatchError::BadUtf8 {
                code,
                offset: off + check_subject,
            });
        }
    }

    let anchored = (prog.overall_options | options) & PCRE2_ANCHORED != 0;
    let startline = prog.flags & FLAG_STARTLINE != 0;
    let ovsize = prog.top_bracket as usize * 2;

    let mut m = Matcher {
        prog,
        s: subject,
        utf,
        ucp: prog.ucp,
        end_subject,
        true_end_subject: length,
        start_offset,
        check_subject,
        moptions: options,
        poptions: prog.overall_options,
        match_limit: opts.match_limit.min(prog.limit_match),
        depth_limit: opts.depth_limit.min(prog.limit_depth),
        heap_limit: opts.heap_limit.min(prog.limit_heap),
        match_call_count: 0,
        hasthen: prog.flags & FLAG_HASTHEN != 0,
        nl: prog.newline,
        nllen: if prog.newline == Newline::CrLf { 2 } else { 1 },
        bsr_anycrlf: prog.bsr_anycrlf,
        start_used_ptr: 0,
        last_used_ptr: 0,
        mark: None,
        nomatch_mark: None,
        verb_ecode_ptr: 0,
        verb_skip_pos: 0,
        verb_skip_name: 0,
        verb_current_recurse: RECURSE_UNSET,
        skip_arg_count: 0,
        ignore_skip_arg: 0,
        end_match_ptr: 0,
        end_offset_top: 0,
        branch_end: None,
        frames: Vec::new(),
        ovecs: Vec::new(),
        ovsize,
        frame_bytes: 136 + 8 * ovsize,
        accept_ov: Vec::new(),
        accept_offset_top: 0,
        accept_mark: None,
        result_ov: Vec::new(),
    };

    POOL.with(|p| {
        if let Ok(mut p) = p.try_borrow_mut() {
            m.frames = std::mem::take(&mut p.0);
            m.ovecs = std::mem::take(&mut p.1);
        }
    });
    let r = search_loop(
        &mut m,
        prog,
        subject,
        start_offset,
        anchored,
        startline,
        utf,
        length,
        end_subject,
    );
    let frames = std::mem::take(&mut m.frames);
    let ovecs = std::mem::take(&mut m.ovecs);
    POOL.with(|p| {
        if let Ok(mut p) = p.try_borrow_mut() {
            // Do not keep huge vectors around after a pathological match.
            if frames.capacity() <= 4096 {
                p.0 = frames;
                p.1 = ovecs;
            }
        }
    });
    r
}

#[allow(clippy::too_many_arguments)]
fn search_loop(
    m: &mut Matcher<'_>,
    prog: &Program,
    subject: &[u8],
    start_offset: usize,
    anchored: bool,
    startline: bool,
    utf: bool,
    length: usize,
    end_subject: usize,
) -> Result<(Option<ExecResult>, Option<u16>), MatchError> {
    let mut start_match = start_offset;
    let has_first_cu = prog.flags & FLAG_FIRSTSET != 0;
    let first_cu = prog.first_codeunit as u8;
    let mut first_cu2 = first_cu;
    if has_first_cu && prog.flags & FLAG_FIRSTCASELESS != 0 {
        first_cu2 = chars::fcc(first_cu as u32) as u8;
        if first_cu > 127 && prog.ucp && !utf {
            first_cu2 = unicode::other_case(first_cu as u32) as u8;
        }
    }
    let start_bits: Option<&[u8; 32]> =
        if !has_first_cu && !startline && prog.flags & FLAG_FIRSTMAPSET != 0 {
            Some(&prog.start_bitmap)
        } else {
            None
        };
    let has_req_cu = prog.flags & FLAG_LASTSET != 0;
    let req_cu = prog.last_codeunit as u8;
    let mut req_cu2 = req_cu;
    if has_req_cu && prog.flags & FLAG_LASTCASELESS != 0 {
        req_cu2 = chars::fcc(req_cu as u32) as u8;
        if req_cu > 127 && prog.ucp && !utf {
            req_cu2 = unicode::other_case(req_cu as u32) as u8;
        }
    }
    let mut req_cu_ptr: isize = start_match as isize - 1;
    let mut req_cu_absent_from = usize::MAX;
    let no_start_opt = prog.overall_options & PCRE2_NO_START_OPTIMIZE != 0;
    let firstline = !anchored && prog.overall_options & PCRE2_FIRSTLINE != 0;
    const REQ_CU_MAX: usize = 5000;

    let mut rc: i32;
    loop {
        if !no_start_opt {
            let mut end = end_subject;
            if firstline {
                let mut t = start_match;
                while t < end && m.nl.is_newline(subject, t, end_subject, utf).is_none() {
                    t += 1;
                }
                end = t;
            }
            if anchored {
                if has_first_cu || start_bits.is_some() {
                    let mut ok = start_match < end;
                    if ok {
                        let c = subject[start_match];
                        ok = has_first_cu && (c == first_cu || c == first_cu2);
                        if !ok && let Some(sb) = start_bits {
                            ok = sb[(c / 8) as usize] & (1 << (c & 7)) != 0;
                        }
                    }
                    if !ok {
                        rc = MATCH_NOMATCH;
                        break;
                    }
                }
            } else if has_first_cu {
                let hay = &subject[start_match..end];
                let pos = if first_cu != first_cu2 {
                    chars::memchr2(first_cu, first_cu2, hay)
                } else {
                    chars::memchr(first_cu, hay)
                };
                start_match = match pos {
                    Some(p) => start_match + p,
                    None => end,
                };
                if start_match >= end_subject {
                    rc = MATCH_NOMATCH;
                    break;
                }
            } else if startline {
                if start_match > start_offset {
                    while start_match < end
                        && m.nl.was_newline(subject, start_match, 0, utf).is_none()
                    {
                        start_match += 1;
                        if utf {
                            while start_match < end && subject[start_match] & 0xc0 == 0x80 {
                                start_match += 1;
                            }
                        }
                    }
                    if start_match > 0
                        && subject[start_match - 1] == b'\r'
                        && matches!(m.nl, Newline::Any | Newline::AnyCrLf)
                        && start_match < end
                        && subject[start_match] == b'\n'
                    {
                        start_match += 1;
                    }
                }
            } else if let Some(sb) = start_bits {
                while start_match < end {
                    let c = subject[start_match];
                    if sb[(c / 8) as usize] & (1 << (c & 7)) != 0 {
                        break;
                    }
                    start_match += 1;
                }
                if start_match >= end_subject {
                    rc = MATCH_NOMATCH;
                    break;
                }
            }

            if end_subject - start_match.min(end_subject) < prog.minlength as usize {
                rc = MATCH_NOMATCH;
                break;
            }
            let p0 = start_match + if has_first_cu { 1 } else { 0 };
            if has_req_cu && p0 as isize > req_cu_ptr {
                let check_length = end_subject - start_match;
                if check_length < REQ_CU_MAX || (!anchored && check_length < REQ_CU_MAX * 1000) {
                    let hay = if p0 <= end_subject {
                        &subject[p0..end_subject]
                    } else {
                        &[][..]
                    };
                    let found = if req_cu != req_cu2 {
                        // Same result as memchr(req_cu) then memchr(req_cu2),
                        // remembering where req_cu is known to be absent.
                        let first = if p0 >= req_cu_absent_from {
                            None
                        } else {
                            let r = chars::memchr(req_cu, hay);
                            if r.is_none() {
                                req_cu_absent_from = p0;
                            }
                            r
                        };
                        first.or_else(|| chars::memchr(req_cu2, hay))
                    } else {
                        chars::memchr(req_cu, hay)
                    };
                    match found {
                        None => {
                            rc = MATCH_NOMATCH;
                            break;
                        }
                        Some(off) => req_cu_ptr = (p0 + off) as isize,
                    }
                }
            }
        }

        if start_match > length {
            rc = MATCH_NOMATCH;
            break;
        }

        m.start_used_ptr = start_match;
        m.last_used_ptr = start_match;
        m.match_call_count = 0;
        m.end_offset_top = 0;
        m.skip_arg_count = 0;

        rc = m.run_match(start_match)?;

        let new_start_match;
        match rc {
            MATCH_SKIP_ARG => {
                new_start_match = start_match;
                m.ignore_skip_arg = m.skip_arg_count;
            }
            MATCH_SKIP if m.verb_skip_pos > start_match => {
                new_start_match = m.verb_skip_pos;
            }
            MATCH_SKIP | MATCH_NOMATCH | MATCH_PRUNE | MATCH_THEN => {
                m.ignore_skip_arg = 0;
                let mut n = start_match + 1;
                if utf {
                    while n < end_subject && subject[n] & 0xc0 == 0x80 {
                        n += 1;
                    }
                }
                new_start_match = n;
            }
            MATCH_COMMIT => {
                rc = MATCH_NOMATCH;
                break;
            }
            _ => break,
        }
        rc = MATCH_NOMATCH;
        if firstline
            && m.nl
                .is_newline(subject, start_match, end_subject, utf)
                .is_some()
        {
            break;
        }
        start_match = new_start_match;
        if anchored || start_match > end_subject {
            break;
        }
        if start_match > start_offset
            && subject[start_match - 1] == b'\r'
            && start_match < end_subject
            && subject[start_match] == b'\n'
            && prog.flags & FLAG_HASCRORLF == 0
            && (matches!(m.nl, Newline::Any | Newline::AnyCrLf) || m.nl == Newline::CrLf)
        {
            start_match += 1;
        }
        m.mark = None;
    }

    if rc == MATCH_MATCH {
        let count = m.end_offset_top / 2 + 1;
        let ov = std::mem::take(&mut m.result_ov);
        return Ok((
            Some(ExecResult {
                ovector: ov,
                count,
                mark: m.mark,
            }),
            m.mark,
        ));
    }
    Ok((None, m.nomatch_mark))
}
