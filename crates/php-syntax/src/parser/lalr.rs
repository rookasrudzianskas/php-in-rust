//! An LALR(1) automaton built (lazily, once) from php-src's grammar, used to
//! reproduce bison's syntax error messages exactly.
//!
//! `grammar.txt` is extracted from `Zend/zend_language_parser.y` (rules
//! without their C actions; mid-rule actions become empty nonterminals).
//! The construction follows bison: LALR(1) lookaheads, shift/reduce
//! conflicts resolved with the precedence declarations (`%nonassoc` turns
//! into explicit error entries), and "most common reduction" default
//! reductions. Replaying the token stream through this automaton with
//! bison's `yyparse()` logic yields the exact state in which bison detects
//! an error, hence the exact `unexpected ..., expecting ...` message, and
//! also reproduces bison's stack limit (`memory exhausted`).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::Parser;
use crate::ParseErrorKind;
use crate::token::{BisonName, TokenKind};

const GRAMMAR: &str = include_str!("grammar.txt");

/// bison's `YYMAXDEPTH`.
const MAX_DEPTH: usize = 10000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Assoc {
    Left,
    Right,
    NonAssociative,
    Precedence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Shift(u32),
    Reduce(u32),
    /// `%nonassoc` error entry.
    Error,
}

struct Rule {
    lhs: u16,
    rhs: Vec<u16>,
    /// Precedence level of the rule (0 = none).
    prec: u8,
}

const WORDS: usize = 3;
type Set = [u64; WORDS];

fn set_add(s: &mut Set, t: usize) {
    s[t / 64] |= 1 << (t % 64);
}

fn set_has(s: &Set, t: usize) -> bool {
    s[t / 64] & (1 << (t % 64)) != 0
}

fn set_clear(s: &mut Set, t: usize) {
    s[t / 64] &= !(1 << (t % 64));
}

fn set_union(dst: &mut Set, src: &Set) -> bool {
    let mut changed = false;
    for i in 0..WORDS {
        let n = dst[i] | src[i];
        changed |= n != dst[i];
        dst[i] = n;
    }
    changed
}

/// The parse tables.
pub(crate) struct Tables {
    rules: Vec<Rule>,
    /// Explicit actions per state, sorted by terminal.
    actions: Vec<Vec<(u16, Action)>>,
    default_reduction: Vec<Option<u32>>,
    /// Gotos on nonterminals per state, sorted by nonterminal.
    gotos: Vec<Vec<(u16, u32)>>,
    accept_state: u32,
    token_terms: HashMap<u16, u16>,
    term_names: Vec<Option<BisonName>>,
    /// Unresolved conflicts found while building (0 for php-src's grammar,
    /// which bison builds with `%expect 0`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) conflicts: usize,
}

struct Grammar {
    names: Vec<String>,
    nterms: usize,
    term_prec: Vec<(u8, Assoc)>,
    rules: Vec<Rule>,
    rules_of: Vec<Vec<u32>>,
}

fn load_grammar() -> Grammar {
    let mut names: Vec<String> = Vec::new();
    let mut index: HashMap<String, u16> = HashMap::new();
    let mut prec_lines = Vec::new();
    let mut raw_rules: Vec<(String, Vec<String>, Option<String>)> = Vec::new();
    for line in GRAMMAR.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("TOKENS") => {
                for t in it {
                    index.insert(t.to_string(), names.len() as u16);
                    names.push(t.to_string());
                }
            }
            Some("PREC") => {
                let assoc = match it.next() {
                    Some("left") => Assoc::Left,
                    Some("right") => Assoc::Right,
                    Some("nonassoc") => Assoc::NonAssociative,
                    _ => Assoc::Precedence,
                };
                prec_lines.push((assoc, it.map(str::to_string).collect::<Vec<_>>()));
            }
            Some("RULE") => {
                let lhs = it.next().expect("lhs").to_string();
                let mut rhs = Vec::new();
                let mut prec = None;
                while let Some(s) = it.next() {
                    if s == "%prec" {
                        prec = it.next().map(str::to_string);
                    } else {
                        rhs.push(s.to_string());
                    }
                }
                raw_rules.push((lhs, rhs, prec));
            }
            _ => {}
        }
    }
    let nterms = names.len();
    let mut term_prec = vec![(0u8, Assoc::Precedence); nterms];
    for (level, (assoc, syms)) in prec_lines.iter().enumerate() {
        for s in syms {
            if let Some(&t) = index.get(s) {
                term_prec[t as usize] = (level as u8 + 1, *assoc);
            }
        }
    }
    let mut intern = |name: &str, names: &mut Vec<String>| -> u16 {
        if let Some(&i) = index.get(name) {
            return i;
        }
        let i = names.len() as u16;
        names.push(name.to_string());
        index.insert(name.to_string(), i);
        i
    };
    let accept = intern("$accept", &mut names);
    for (lhs, _, _) in &raw_rules {
        intern(lhs, &mut names);
    }
    let start = intern("start", &mut names);
    let mut rules = vec![Rule {
        lhs: accept,
        rhs: vec![start, 0],
        prec: 0,
    }];
    for (lhs, rhs, prec) in &raw_rules {
        let rhs: Vec<u16> = rhs.iter().map(|s| intern(s, &mut names)).collect();
        let prec = match prec {
            Some(p) => term_prec[intern(p, &mut names) as usize].0,
            None => rhs
                .iter()
                .rev()
                .find(|&&s| (s as usize) < nterms)
                .map_or(0, |&s| term_prec[s as usize].0),
        };
        let lhs = intern(lhs, &mut names);
        rules.push(Rule { lhs, rhs, prec });
    }
    let mut rules_of = vec![Vec::new(); names.len()];
    for (i, r) in rules.iter().enumerate() {
        rules_of[r.lhs as usize].push(i as u32);
    }
    Grammar {
        names,
        nterms,
        term_prec,
        rules,
        rules_of,
    }
}

impl Tables {
    pub(crate) fn get() -> &'static Tables {
        static TABLES: OnceLock<Tables> = OnceLock::new();
        TABLES.get_or_init(build)
    }

    fn term_of(&self, k: TokenKind) -> Option<u16> {
        self.token_terms.get(&k.id()).copied()
    }

    fn action(&self, state: u32, term: u16) -> Option<Action> {
        let row = &self.actions[state as usize];
        row.binary_search_by_key(&term, |&(t, _)| t)
            .ok()
            .map(|i| row[i].1)
    }

    fn goto(&self, state: u32, nt: u16) -> u32 {
        let row = &self.gotos[state as usize];
        let i = row.binary_search_by_key(&nt, |&(n, _)| n).expect("goto");
        row[i].1
    }
}

fn build() -> Tables {
    let g = load_grammar();
    let nsyms = g.names.len();
    let nterms = g.nterms;
    let is_term = |s: u16| (s as usize) < nterms;

    // nullable and FIRST
    let mut nullable = vec![false; nsyms];
    let mut first = vec![[0u64; WORDS]; nsyms];
    for (t, f) in first.iter_mut().enumerate().take(nterms) {
        set_add(f, t);
    }
    let mut changed = true;
    while changed {
        changed = false;
        for r in &g.rules {
            let lhs = r.lhs as usize;
            let mut all_nullable = true;
            for &s in &r.rhs {
                let fs = first[s as usize];
                if set_union(&mut first[lhs], &fs) {
                    changed = true;
                }
                if !nullable[s as usize] {
                    all_nullable = false;
                    break;
                }
            }
            if all_nullable && !nullable[lhs] {
                nullable[lhs] = true;
                changed = true;
            }
        }
    }
    let first_of_seq = |seq: &[u16]| -> (Set, bool) {
        let mut s = [0u64; WORDS];
        for &x in seq {
            set_union(&mut s, &first[x as usize]);
            if !nullable[x as usize] {
                return (s, false);
            }
        }
        (s, true)
    };

    // Item ids: item_base[rule] + dot.
    let mut item_base = Vec::with_capacity(g.rules.len());
    let mut nitems = 0u32;
    for r in &g.rules {
        item_base.push(nitems);
        nitems += r.rhs.len() as u32 + 1;
    }
    let mut item_rule = vec![0u32; nitems as usize];
    for (ri, r) in g.rules.iter().enumerate() {
        for d in 0..=r.rhs.len() {
            item_rule[(item_base[ri] + d as u32) as usize] = ri as u32;
        }
    }
    let item_dot = |it: u32| it - item_base[item_rule[it as usize] as usize];
    let next_sym = |it: u32| -> Option<u16> {
        let r = &g.rules[item_rule[it as usize] as usize];
        r.rhs.get(item_dot(it) as usize).copied()
    };

    // Nonterminals derivable in leftmost position from each nonterminal.
    let mut nt_close: Vec<Vec<u16>> = vec![Vec::new(); nsyms];
    for (n, close) in nt_close.iter_mut().enumerate().skip(nterms) {
        let mut seen = vec![false; nsyms];
        let mut stack = vec![n as u16];
        seen[n] = true;
        while let Some(x) = stack.pop() {
            close.push(x);
            for &ri in &g.rules_of[x as usize] {
                if let Some(&s) = g.rules[ri as usize].rhs.first()
                    && !is_term(s)
                    && !seen[s as usize]
                {
                    seen[s as usize] = true;
                    stack.push(s);
                }
            }
        }
    }

    // LR(0) automaton.
    struct State {
        kernel: Vec<u32>,
        closure_nts: Vec<u16>,
        trans: Vec<(u16, u32)>,
    }
    let mut states: Vec<State> = Vec::new();
    let mut kernel_index: HashMap<Vec<u32>, u32> = HashMap::new();
    let start_kernel = vec![item_base[0]];
    kernel_index.insert(start_kernel.clone(), 0);
    states.push(State {
        kernel: start_kernel,
        closure_nts: Vec::new(),
        trans: Vec::new(),
    });
    let mut todo = 0;
    let mut seen = vec![false; nsyms];
    while todo < states.len() {
        let kernel = states[todo].kernel.clone();
        let mut nts: Vec<u16> = Vec::new();
        for &it in &kernel {
            if let Some(s) = next_sym(it)
                && !is_term(s)
            {
                for &m in &nt_close[s as usize] {
                    if !seen[m as usize] {
                        seen[m as usize] = true;
                        nts.push(m);
                    }
                }
            }
        }
        for &m in &nts {
            seen[m as usize] = false;
        }
        nts.sort_unstable();
        let mut by_sym: std::collections::BTreeMap<u16, Vec<u32>> =
            std::collections::BTreeMap::new();
        for &it in &kernel {
            if let Some(s) = next_sym(it) {
                by_sym.entry(s).or_default().push(it + 1);
            }
        }
        for &n in &nts {
            for &ri in &g.rules_of[n as usize] {
                let it = item_base[ri as usize];
                if let Some(s) = next_sym(it) {
                    by_sym.entry(s).or_default().push(it + 1);
                }
            }
        }
        let mut trans = Vec::new();
        for (sym, mut k) in by_sym {
            k.sort_unstable();
            k.dedup();
            let id = match kernel_index.get(&k) {
                Some(&id) => id,
                None => {
                    let id = states.len() as u32;
                    kernel_index.insert(k.clone(), id);
                    states.push(State {
                        kernel: k,
                        closure_nts: Vec::new(),
                        trans: Vec::new(),
                    });
                    id
                }
            };
            trans.push((sym, id));
        }
        states[todo].closure_nts = nts;
        states[todo].trans = trans;
        todo += 1;
    }

    // Lookahead propagation graph. Nodes: one per kernel item, one per
    // closure nonterminal (all its dot-0 items share a lookahead set).
    let mut kernel_node_base = Vec::with_capacity(states.len());
    let mut nnodes = 0usize;
    for s in &states {
        kernel_node_base.push(nnodes);
        nnodes += s.kernel.len();
    }
    let mut nt_node_base = Vec::with_capacity(states.len());
    for s in &states {
        nt_node_base.push(nnodes);
        nnodes += s.closure_nts.len();
    }
    let mut sets = vec![[0u64; WORDS]; nnodes];
    let mut edges: Vec<Vec<u32>> = vec![Vec::new(); nnodes];
    let kernel_node = |state: u32, item: u32| -> usize {
        let s = &states[state as usize];
        kernel_node_base[state as usize] + s.kernel.binary_search(&item).expect("kernel item")
    };
    let nt_node = |state: u32, nt: u16| -> usize {
        let s = &states[state as usize];
        nt_node_base[state as usize] + s.closure_nts.binary_search(&nt).expect("closure nt")
    };
    let goto_of = |state: u32, sym: u16| -> u32 {
        let t = &states[state as usize].trans;
        t[t.binary_search_by_key(&sym, |&(s, _)| s)
            .expect("transition")]
        .1
    };
    for (si, s) in states.iter().enumerate() {
        let si = si as u32;
        let handle = |src: usize, item: u32, sets: &mut Vec<Set>, edges: &mut Vec<Vec<u32>>| {
            let r = &g.rules[item_rule[item as usize] as usize];
            let dot = item_dot(item) as usize;
            if dot >= r.rhs.len() {
                return;
            }
            let x = r.rhs[dot];
            let target = goto_of(si, x);
            edges[src].push(kernel_node(target, item + 1) as u32);
            if !is_term(x) {
                let (f, null) = first_of_seq(&r.rhs[dot + 1..]);
                let n = nt_node(si, x);
                set_union(&mut sets[n], &f);
                if null {
                    edges[src].push(n as u32);
                }
            }
        };
        for (ki, &item) in s.kernel.iter().enumerate() {
            handle(
                kernel_node_base[si as usize] + ki,
                item,
                &mut sets,
                &mut edges,
            );
        }
        for (ni, &nt) in s.closure_nts.iter().enumerate() {
            let src = nt_node_base[si as usize] + ni;
            for &ri in &g.rules_of[nt as usize] {
                handle(src, item_base[ri as usize], &mut sets, &mut edges);
            }
        }
    }
    let mut work: Vec<u32> = (0..nnodes as u32).collect();
    let mut queued = vec![true; nnodes];
    while let Some(n) = work.pop() {
        queued[n as usize] = false;
        let src = sets[n as usize];
        for &m in &edges[n as usize] {
            let m = m as usize;
            if set_union(&mut sets[m], &src) && !queued[m] {
                queued[m] = true;
                work.push(m as u32);
            }
        }
    }

    // Actions (bison's conflicts.c + tables.c action_row()).
    let mut actions = Vec::with_capacity(states.len());
    let mut default_reduction = Vec::with_capacity(states.len());
    let mut gotos = Vec::with_capacity(states.len());
    let mut conflicts = 0usize;
    let mut accept_state = 0;
    for (si, s) in states.iter().enumerate() {
        let mut reds: Vec<(u32, Set)> = Vec::new();
        for (ki, &item) in s.kernel.iter().enumerate() {
            let ri = item_rule[item as usize];
            if item_dot(item) as usize == g.rules[ri as usize].rhs.len() && ri != 0 {
                reds.push((ri, sets[kernel_node_base[si] + ki]));
            }
        }
        for (ni, &nt) in s.closure_nts.iter().enumerate() {
            for &ri in &g.rules_of[nt as usize] {
                if g.rules[ri as usize].rhs.is_empty() {
                    reds.push((ri, sets[nt_node_base[si] + ni]));
                }
            }
        }
        reds.sort_by_key(|r| r.0);
        let mut shift_set = [0u64; WORDS];
        let mut shifts: Vec<(u16, u32)> = Vec::new();
        let mut state_gotos = Vec::new();
        for &(sym, to) in &s.trans {
            if is_term(sym) {
                shifts.push((sym, to));
                set_add(&mut shift_set, sym as usize);
            } else {
                state_gotos.push((sym, to));
            }
        }
        if s.kernel.contains(&(item_base[0] + 1)) {
            accept_state = si as u32;
        }
        let consistent = reds.is_empty() || (reds.len() == 1 && shifts.is_empty());
        let mut errs = [0u64; WORDS];
        if !consistent {
            for red in reds.iter_mut() {
                let rprec = g.rules[red.0 as usize].prec;
                if rprec == 0 {
                    continue;
                }
                for t in 0..nterms {
                    if !set_has(&red.1, t) || !set_has(&shift_set, t) {
                        continue;
                    }
                    let (tprec, assoc) = g.term_prec[t];
                    if tprec == 0 {
                        continue;
                    }
                    if tprec < rprec {
                        set_clear(&mut shift_set, t);
                    } else if tprec > rprec {
                        set_clear(&mut red.1, t);
                    } else {
                        match assoc {
                            Assoc::Right => set_clear(&mut red.1, t),
                            Assoc::Left => set_clear(&mut shift_set, t),
                            Assoc::NonAssociative => {
                                set_clear(&mut shift_set, t);
                                set_clear(&mut red.1, t);
                                set_add(&mut errs, t);
                            }
                            Assoc::Precedence => conflicts += 1,
                        }
                    }
                }
            }
            let mut seen = shift_set;
            for red in &reds {
                for t in 0..nterms {
                    if set_has(&red.1, t) && set_has(&seen, t) {
                        conflicts += 1;
                    }
                }
                set_union(&mut seen, &red.1);
            }
        }
        let mut row: Vec<Option<Action>> = vec![None; nterms];
        if !consistent {
            for red in reds.iter().rev() {
                for (t, slot) in row.iter_mut().enumerate() {
                    if set_has(&red.1, t) {
                        *slot = Some(Action::Reduce(red.0));
                    }
                }
            }
        }
        for &(sym, to) in &shifts {
            if set_has(&shift_set, sym as usize) {
                row[sym as usize] = Some(Action::Shift(to));
            }
        }
        for (t, slot) in row.iter_mut().enumerate() {
            if set_has(&errs, t) {
                *slot = Some(Action::Error);
            }
        }
        let mut default = None;
        if consistent {
            default = reds.first().map(|r| r.0);
        } else if !reds.is_empty() {
            let mut max = 0;
            for red in &reds {
                let count = row
                    .iter()
                    .filter(|a| **a == Some(Action::Reduce(red.0)))
                    .count();
                if count > max {
                    max = count;
                    default = Some(red.0);
                }
            }
            if let Some(d) = default {
                for a in row.iter_mut() {
                    if *a == Some(Action::Reduce(d)) {
                        *a = None;
                    }
                }
            }
        }
        if default.is_none() {
            for a in row.iter_mut() {
                if *a == Some(Action::Error) {
                    *a = None;
                }
            }
        }
        let explicit: Vec<(u16, Action)> = row
            .iter()
            .enumerate()
            .filter_map(|(t, a)| a.map(|a| (t as u16, a)))
            .collect();
        actions.push(explicit);
        default_reduction.push(default);
        gotos.push(state_gotos);
    }

    let mut token_terms = HashMap::new();
    let mut term_names = vec![None; nterms];
    let name_index: HashMap<&str, u16> = g.names[..nterms]
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i as u16))
        .collect();
    for &k in TokenKind::ALL {
        let name = k.grammar_name();
        if let Some(&t) = name_index.get(&*name) {
            token_terms.insert(k.id(), t);
            term_names[t as usize] = Some(k.display());
        }
    }
    Tables {
        rules: g.rules,
        actions,
        default_reduction,
        gotos,
        accept_state,
        token_terms,
        term_names,
        conflicts,
    }
}

/// Outcome of replaying a token stream.
pub(crate) enum Outcome {
    Accept,
    /// Syntax error with the lookahead at token `at`, detected in `state`.
    SyntaxError {
        at: usize,
        state: u32,
    },
    /// The lexer threw while scanning token `at`.
    LexError {
        at: usize,
    },
    /// bison's stack overflowed; `last` is the last scanned token.
    MemoryExhausted {
        last: usize,
    },
}

impl Tables {
    /// bison's `yyparse()` over `kinds`.
    pub(crate) fn run(&self, kinds: &[TokenKind]) -> Outcome {
        let mut stack: Vec<u32> = vec![0];
        let mut la: Option<(usize, u16)> = None;
        let mut next = 0usize;
        loop {
            let state = *stack.last().expect("stack");
            let row = &self.actions[state as usize];
            let act = if row.is_empty() {
                None
            } else {
                let (at, term) = match la {
                    Some(x) => x,
                    None => {
                        let i = next.min(kinds.len() - 1);
                        next = i + 1;
                        if kinds[i] == TokenKind::Error {
                            return Outcome::LexError { at: i };
                        }
                        let term = self.term_of(kinds[i]).unwrap_or(u16::MAX);
                        la = Some((i, term));
                        (i, term)
                    }
                };
                match self.action(state, term) {
                    Some(Action::Error) => return Outcome::SyntaxError { at, state },
                    a => a,
                }
            };
            match act {
                Some(Action::Shift(to)) => {
                    let (at, term) = la.take().expect("lookahead");
                    if term == 0 && state == self.accept_state {
                        return Outcome::Accept;
                    }
                    stack.push(to);
                    if stack.len() >= MAX_DEPTH {
                        return Outcome::MemoryExhausted { last: at };
                    }
                }
                Some(Action::Reduce(r)) => self.reduce(&mut stack, r),
                Some(Action::Error) => unreachable!("handled above"),
                None => match self.default_reduction[state as usize] {
                    Some(r) => self.reduce(&mut stack, r),
                    None => {
                        let at = la.map_or(next.saturating_sub(1), |l| l.0);
                        return Outcome::SyntaxError { at, state };
                    }
                },
            }
            if stack.len() >= MAX_DEPTH {
                return Outcome::MemoryExhausted {
                    last: next.saturating_sub(1),
                };
            }
        }
    }

    fn reduce(&self, stack: &mut Vec<u32>, r: u32) {
        let rule = &self.rules[r as usize];
        stack.truncate(stack.len() - rule.rhs.len());
        let top = *stack.last().expect("stack");
        let to = self.goto(top, rule.lhs);
        stack.push(to);
    }

    /// The `expecting ...` names for an error in `state` (empty when bison
    /// would not list them: none, or more than four).
    fn expecting(&self, state: u32) -> Vec<String> {
        let exp: Vec<u16> = self.actions[state as usize]
            .iter()
            .filter(|(_, a)| *a != Action::Error)
            .map(|(t, _)| *t)
            .collect();
        if exp.is_empty() || exp.len() > 4 {
            return Vec::new();
        }
        exp.iter()
            .map(|&t| match self.term_names[t as usize] {
                _ if t == 0 => "end of file".to_string(),
                Some(BisonName::Literal(s)) => format!("\"{s}\""),
                Some(BisonName::Amp) => "token \"&\"".to_string(),
                Some(BisonName::Descr(d)) | Some(BisonName::Raw(d)) => d.to_string(),
                None => format!("#{t}"),
            })
            .collect()
    }
}

impl<'a> Parser<'a> {
    /// Replay the token stream through the LALR automaton and describe the
    /// first error bison would report.
    pub(crate) fn locate_syntax_error(&self, idx: usize) -> (ParseErrorKind, String, u32, usize) {
        let tables = Tables::get();
        let kinds: Vec<TokenKind> = self.toks.iter().map(|t| t.kind).collect();
        match tables.run(&kinds) {
            Outcome::LexError { at } => match self.lex_error() {
                Some(e) => (ParseErrorKind::Parse, e.message.clone(), e.line, at),
                None => self.describe_at(at),
            },
            Outcome::SyntaxError { at, state } => {
                let mut msg = format!("syntax error, unexpected {}", self.describe_unexpected(at));
                let exp = tables.expecting(state);
                if !exp.is_empty() {
                    msg.push_str(", expecting ");
                    msg.push_str(&exp.join(" or "));
                }
                (ParseErrorKind::Parse, msg, self.toks[at].end_line, at)
            }
            Outcome::MemoryExhausted { last } => (
                ParseErrorKind::Parse,
                "memory exhausted".to_string(),
                self.toks[last].end_line,
                last,
            ),
            Outcome::Accept => {
                // Should not happen: the automaton accepts what the
                // recursive-descent parser rejected.
                if self.toks[idx].kind == TokenKind::Error
                    && let Some(e) = self.lex_error()
                {
                    return (ParseErrorKind::Parse, e.message.clone(), e.line, idx);
                }
                self.describe_at(idx)
            }
        }
    }

    fn describe_at(&self, idx: usize) -> (ParseErrorKind, String, u32, usize) {
        let msg = format!("syntax error, unexpected {}", self.describe_unexpected(idx));
        (ParseErrorKind::Parse, msg, self.toks[idx].end_line, idx)
    }

    /// `zend_yytnamerr()` for the unexpected token.
    pub(crate) fn describe_unexpected(&self, idx: usize) -> String {
        let t = &self.toks[idx];
        if t.kind == TokenKind::End {
            return "end of file".to_string();
        }
        match t.kind.display() {
            BisonName::Literal("\"") => "double-quote mark".to_string(),
            BisonName::Literal(s) => format!("token \"{s}\""),
            BisonName::Amp => "token \"&\"".to_string(),
            BisonName::Descr(d) | BisonName::Raw(d) => {
                let text = self.text(idx);
                if t.kind == TokenKind::BadCharacter && text.len() == 1 {
                    return format!("character 0x{:02X}", text[0]);
                }
                let mut d = d;
                let mut content: &[u8] = match text.iter().position(|&c| c == b'\n') {
                    Some(p) => &text[..p],
                    None => text,
                };
                if t.kind == TokenKind::ConstantEncapsedString && !content.is_empty() {
                    if content[0] == b'"' {
                        d = "double-quoted string";
                    } else if content[0] == b'\'' {
                        d = "single-quoted string";
                    }
                }
                if !content.is_empty() && (content[0] == b'\'' || content[0] == b'"') {
                    content = &content[1..];
                }
                if let Some(&last) = content.last()
                    && (last == b'\'' || last == b'"')
                {
                    content = &content[..content.len() - 1];
                }
                if content.len() > 30 + 3 {
                    format!("{d} \"{}...\"", String::from_utf8_lossy(&content[..30]))
                } else {
                    format!("{d} \"{}\"", String::from_utf8_lossy(content))
                }
            }
        }
    }

    /// Whether bison accepts this token stream (testing oracle).
    pub(crate) fn lalr_accepts(&self) -> bool {
        let kinds: Vec<TokenKind> = self.toks.iter().map(|t| t.kind).collect();
        matches!(Tables::get().run(&kinds), Outcome::Accept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammar_has_no_conflicts() {
        let t = Tables::get();
        assert_eq!(t.conflicts, 0);
        assert!(t.actions.len() > 500);
    }
}
