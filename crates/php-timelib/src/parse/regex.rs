//! A tiny regular-expression engine with re2c scanner semantics.
//!
//! timelib's parsers are re2c scanners: at each position every rule is tried, the
//! longest match wins and ties go to the rule listed first. We express the rules as
//! [`Re`] trees (mirroring the re2c definitions one-to-one), compile all of them into a
//! single Thompson NFA whose accepting states carry the rule index, and simulate it to
//! find the longest match. Inputs are short (date strings), so simulation is cheap.

/// A set of bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ByteSet([u64; 4]);

impl ByteSet {
    pub(crate) const fn empty() -> Self {
        ByteSet([0; 4])
    }

    pub(crate) fn insert(&mut self, b: u8) {
        self.0[usize::from(b >> 6)] |= 1u64 << (b & 63);
    }

    pub(crate) fn contains(&self, b: u8) -> bool {
        self.0[usize::from(b >> 6)] & (1u64 << (b & 63)) != 0
    }
}

/// A regular expression.
#[derive(Clone, Debug)]
pub(crate) enum Re {
    /// One byte from the set.
    Set(ByteSet),
    /// Concatenation.
    Seq(Vec<Re>),
    /// Alternation.
    Alt(Vec<Re>),
    /// Bounded or unbounded repetition (`min`, `max`; `None` = unbounded).
    Repeat(Box<Re>, u32, Option<u32>),
}

/// Case-sensitive literal (`"..."` in re2c).
pub(crate) fn lit(s: &str) -> Re {
    Re::Seq(
        s.bytes()
            .map(|b| {
                let mut set = ByteSet::empty();
                set.insert(b);
                Re::Set(set)
            })
            .collect(),
    )
}

/// Case-insensitive literal (`'...'` in re2c); only ASCII letters are folded.
pub(crate) fn ilit(s: &str) -> Re {
    Re::Seq(
        s.bytes()
            .map(|b| {
                let mut set = ByteSet::empty();
                set.insert(b.to_ascii_lowercase());
                set.insert(b.to_ascii_uppercase());
                Re::Set(set)
            })
            .collect(),
    )
}

/// Character class, re2c style: `"0-9A-Z_"`. A `-` at the start or end is literal.
pub(crate) fn class(spec: &str) -> Re {
    let b = spec.as_bytes();
    let mut set = ByteSet::empty();
    let mut i = 0;
    while i < b.len() {
        if i + 2 < b.len() && b[i + 1] == b'-' {
            for c in b[i]..=b[i + 2] {
                set.insert(c);
            }
            i += 3;
        } else {
            set.insert(b[i]);
            i += 1;
        }
    }
    Re::Set(set)
}

/// Any single byte.
pub(crate) fn any() -> Re {
    Re::Set(ByteSet([u64::MAX; 4]))
}

/// Concatenation.
pub(crate) fn seq(parts: Vec<Re>) -> Re {
    Re::Seq(parts)
}

/// Alternation.
pub(crate) fn alt(parts: Vec<Re>) -> Re {
    Re::Alt(parts)
}

/// Optional (`?`).
pub(crate) fn opt(r: Re) -> Re {
    Re::Repeat(Box::new(r), 0, Some(1))
}

/// Zero or more (`*`).
pub(crate) fn star(r: Re) -> Re {
    Re::Repeat(Box::new(r), 0, None)
}

/// One or more (`+`).
pub(crate) fn plus(r: Re) -> Re {
    Re::Repeat(Box::new(r), 1, None)
}

/// `{min,max}` repetition.
pub(crate) fn rep(r: Re, min: u32, max: u32) -> Re {
    Re::Repeat(Box::new(r), min, Some(max))
}

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Clone, Debug)]
enum Inst {
    /// Consume one byte from `sets[set]`, then go to `next`.
    Byte { set: u32, next: u32 },
    /// Epsilon transitions to all targets.
    Fork(Vec<u32>),
    /// Accept with the given rule index.
    Match(u16),
}

/// A compiled scanner: all rules in one NFA, simulated through a lazily built DFA.
#[derive(Debug)]
pub(crate) struct Scanner {
    insts: Vec<Inst>,
    sets: Vec<ByteSet>,
    start_closure: Vec<u32>,
    dfa: Mutex<Dfa>,
}

/// Transition marker: not computed yet.
const UNKNOWN: u32 = u32::MAX;
/// Transition marker: no NFA state survives.
const DEAD: u32 = u32::MAX - 1;
/// Upper bound on cached DFA states before the cache is flushed.
const MAX_DFA_STATES: usize = 20_000;

#[derive(Debug)]
struct DState {
    /// Sorted NFA states (only `Byte` and `Match` instructions).
    nfa: Vec<u32>,
    /// Lowest rule index accepting in this state.
    accept: Option<u16>,
    next: Box<[u32; 256]>,
}

#[derive(Debug, Default)]
struct Dfa {
    states: Vec<DState>,
    index: HashMap<Vec<u32>, u32>,
}

struct Compiler {
    insts: Vec<Inst>,
    sets: Vec<ByteSet>,
}

impl Compiler {
    fn push(&mut self, inst: Inst) -> u32 {
        self.insts.push(inst);
        (self.insts.len() - 1) as u32
    }

    fn set_id(&mut self, set: ByteSet) -> u32 {
        if let Some(i) = self.sets.iter().position(|s| *s == set) {
            return i as u32;
        }
        self.sets.push(set);
        (self.sets.len() - 1) as u32
    }

    /// Compiles `re` so that after matching it control continues at `next`; returns the
    /// entry state.
    fn compile(&mut self, re: &Re, next: u32) -> u32 {
        match re {
            Re::Set(set) => {
                let set = self.set_id(*set);
                self.push(Inst::Byte { set, next })
            }
            Re::Seq(parts) => {
                let mut k = next;
                for part in parts.iter().rev() {
                    k = self.compile(part, k);
                }
                k
            }
            Re::Alt(parts) => {
                let starts: Vec<u32> = parts.iter().map(|p| self.compile(p, next)).collect();
                self.push(Inst::Fork(starts))
            }
            Re::Repeat(inner, min, max) => {
                let mut k = next;
                match max {
                    None => {
                        let lp = self.push(Inst::Fork(Vec::new()));
                        let body = self.compile(inner, lp);
                        self.insts[lp as usize] = Inst::Fork(vec![body, next]);
                        k = lp;
                    }
                    Some(max) => {
                        for _ in *min..*max {
                            let body = self.compile(inner, k);
                            k = self.push(Inst::Fork(vec![body, k]));
                        }
                    }
                }
                for _ in 0..*min {
                    k = self.compile(inner, k);
                }
                k
            }
        }
    }
}

impl Scanner {
    /// Builds a scanner from rules in priority order.
    pub(crate) fn new(rules: &[Re]) -> Scanner {
        let mut c = Compiler {
            insts: Vec::new(),
            sets: Vec::new(),
        };
        let mut starts = Vec::with_capacity(rules.len());
        for (i, r) in rules.iter().enumerate() {
            let m = c.push(Inst::Match(i as u16));
            starts.push(c.compile(r, m));
        }
        let start = c.push(Inst::Fork(starts));
        let mut sc = Scanner {
            insts: c.insts,
            sets: c.sets,
            start_closure: Vec::new(),
            dfa: Mutex::new(Dfa::default()),
        };
        let mut marks = vec![u32::MAX; sc.insts.len()];
        let mut closure = Vec::new();
        sc.add_closure(start, &mut closure, &mut marks, 0);
        closure.sort_unstable();
        sc.start_closure = closure;
        sc
    }

    fn accept_of(&self, states: &[u32]) -> Option<u16> {
        states
            .iter()
            .filter_map(|&s| match self.insts[s as usize] {
                Inst::Match(r) => Some(r),
                _ => None,
            })
            .min()
    }

    fn intern(&self, dfa: &mut Dfa, nfa: Vec<u32>) -> u32 {
        if let Some(&id) = dfa.index.get(&nfa) {
            return id;
        }
        let id = dfa.states.len() as u32;
        let accept = self.accept_of(&nfa);
        dfa.index.insert(nfa.clone(), id);
        dfa.states.push(DState {
            nfa,
            accept,
            next: Box::new([UNKNOWN; 256]),
        });
        id
    }

    fn step(&self, nfa: &[u32], b: u8) -> Vec<u32> {
        let mut marks = vec![u32::MAX; self.insts.len()];
        let mut out = Vec::new();
        for &s in nfa {
            if let Inst::Byte { set, next } = self.insts[s as usize]
                && self.sets[set as usize].contains(b)
            {
                self.add_closure(next, &mut out, &mut marks, 0);
            }
        }
        out.sort_unstable();
        out
    }

    fn add_closure(&self, s: u32, out: &mut Vec<u32>, marks: &mut [u32], generation: u32) {
        let mut stack = vec![s];
        while let Some(s) = stack.pop() {
            if marks[s as usize] == generation {
                continue;
            }
            marks[s as usize] = generation;
            match &self.insts[s as usize] {
                Inst::Fork(targets) => {
                    for &t in targets.iter().rev() {
                        stack.push(t);
                    }
                }
                _ => out.push(s),
            }
        }
    }

    /// Longest match starting at `start`: returns `(end, rule)`; ties go to the lowest
    /// rule index. Zero-length matches are ignored.
    pub(crate) fn longest_match(&self, input: &[u8], start: usize) -> Option<(usize, usize)> {
        let mut dfa = self.dfa.lock().unwrap_or_else(|e| e.into_inner());
        if dfa.states.len() > MAX_DFA_STATES {
            *dfa = Dfa::default();
        }
        if dfa.states.is_empty() {
            // The start state is always DFA state 0.
            self.intern(&mut dfa, self.start_closure.clone());
        }
        let mut cur = 0u32;
        let mut best = None;
        let mut pos = start;
        while pos < input.len() {
            let b = input[pos];
            let mut next = dfa.states[cur as usize].next[usize::from(b)];
            if next == UNKNOWN {
                let set = self.step(&dfa.states[cur as usize].nfa, b);
                next = if set.is_empty() {
                    DEAD
                } else {
                    self.intern(&mut dfa, set)
                };
                dfa.states[cur as usize].next[usize::from(b)] = next;
            }
            if next == DEAD {
                break;
            }
            cur = next;
            pos += 1;
            if let Some(rule) = dfa.states[cur as usize].accept {
                best = Some((pos, usize::from(rule)));
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_and_priority() {
        let rules = vec![
            ilit("mon"),
            seq(vec![ilit("mon"), ilit("day")]),
            plus(class("a-z")),
            any(),
        ];
        let sc = Scanner::new(&rules);
        assert_eq!(sc.longest_match(b"monday!", 0), Some((6, 1)));
        assert_eq!(sc.longest_match(b"MON ", 0), Some((3, 0)));
        assert_eq!(sc.longest_match(b"mond", 0), Some((4, 2)));
        assert_eq!(sc.longest_match(b"!", 0), Some((1, 3)));
        assert_eq!(sc.longest_match(b"", 0), None);
    }

    #[test]
    fn bounded_repeat() {
        let sc = Scanner::new(&[rep(class("0-9"), 2, 4)]);
        assert_eq!(sc.longest_match(b"1", 0), None);
        assert_eq!(sc.longest_match(b"123456", 0), Some((4, 0)));
        let sc = Scanner::new(&[seq(vec![opt(lit("t")), lit("x")])]);
        assert_eq!(sc.longest_match(b"tx", 0), Some((2, 0)));
        assert_eq!(sc.longest_match(b"x", 0), Some((1, 0)));
    }
}
