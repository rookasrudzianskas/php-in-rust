//! Compiled-pattern cache, per-VM state and error mapping
//! (`pcre_get_compiled_regex_cache_ex()`, `pcre_handle_exec_error()` and the
//! module globals of `php_pcre.c`).

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use php_regex::{Captures, MatchError, MatchOptions, PregError, Regex, compile_php_pattern};
use php_runtime::{PResult, PhpStr, Vm};

/// `PCRE_CACHE_SIZE`.
pub const PCRE_CACHE_SIZE: usize = 4096;

/// A compiled pattern (`pcre_cache_entry`).
pub struct Pce {
    pub regex: Regex,
    /// Whether the pattern runs in UTF mode (`compile_options & PCRE2_UTF`;
    /// also true for a pattern that enables it with `(*UTF)`).
    pub utf: bool,
    /// `capture_count`.
    pub capture_count: usize,
    /// `name_count`.
    pub name_count: usize,
    /// `subpats_table`: the name of every group (index = group number), or
    /// empty when the pattern has no named groups.
    pub subpat_names: Vec<Option<PhpStr>>,
}

impl Pce {
    /// `num_subpats`: capture groups plus the whole match.
    pub fn num_subpats(&self) -> usize {
        self.capture_count + 1
    }

    /// The group-name table for populating result arrays (`None` without
    /// named groups, like a NULL `subpat_names`).
    pub fn names(&self) -> Option<&[Option<PhpStr>]> {
        (self.name_count > 0).then_some(self.subpat_names.as_slice())
    }

    /// `calculate_unit_length()`: the byte length of the character at `pos`
    /// (a single byte unless the pattern is in UTF mode).
    pub fn unit_len(&self, subject: &[u8], pos: usize) -> usize {
        if !self.utf {
            return 1;
        }
        let mut end = pos + 1;
        while end < subject.len() && subject[end] & 0xC0 == 0x80 {
            end += 1;
        }
        end - pos
    }

    fn new(regex: Regex) -> Pce {
        let capture_count = regex.capture_count();
        let name_count = regex.name_count();
        let mut subpat_names = Vec::new();
        if name_count > 0 {
            subpat_names.resize(capture_count + 1, None);
            for (idx, name) in regex.capture_names() {
                subpat_names[idx] = Some(PhpStr::from_vec(name));
            }
        }
        Pce {
            utf: regex.is_utf(),
            capture_count,
            name_count,
            subpat_names,
            regex,
        }
    }
}

/// The regex cache (`PCRE_G(pcre_cache)`), insertion-ordered so that the
/// oldest entries are dropped first when it is full.
#[derive(Default)]
struct RegexCache {
    entries: HashMap<Rc<[u8]>, (u64, Rc<Pce>)>,
    order: BTreeMap<u64, Rc<[u8]>>,
    next_seq: u64,
}

impl RegexCache {
    fn get(&self, key: &[u8]) -> Option<Rc<Pce>> {
        self.entries.get(key).map(|(_, pce)| pce.clone())
    }

    fn insert(&mut self, key: &[u8], pce: Rc<Pce>) {
        // "If we reached cache limit, clean out the items from the head".
        if self.entries.len() == PCRE_CACHE_SIZE {
            self.clean(PCRE_CACHE_SIZE / 8);
        }
        let key: Rc<[u8]> = Rc::from(key);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.order.insert(seq, key.clone());
        self.entries.insert(key, (seq, pce));
    }

    /// `pcre_clean_cache`: removes up to `num_clean` of the oldest entries
    /// that are not in use (`refcount == 0`, i.e. nobody else holds the `Rc`).
    fn clean(&mut self, mut num_clean: usize) {
        let mut victims = Vec::with_capacity(num_clean);
        for (&seq, key) in &self.order {
            if Rc::strong_count(&self.entries[key].1) == 1 {
                victims.push(seq);
                num_clean -= 1;
                if num_clean == 0 {
                    break;
                }
            }
        }
        for seq in victims {
            if let Some(key) = self.order.remove(&seq) {
                self.entries.remove(&key);
            }
        }
    }
}

/// Per-VM state of the extension (`zend_pcre_globals`).
pub struct PcreState {
    /// `PCRE_G(error_code)`.
    pub error_code: PregError,
    /// The last subject a UTF-8 pattern matched without an error from offset
    /// 0, so it is known to be valid UTF-8 (php-src caches this as the
    /// `IS_STR_VALID_UTF8` flag on the string itself).
    valid_utf8: Option<PhpStr>,
    cache: RegexCache,
}

impl Default for PcreState {
    fn default() -> Self {
        PcreState {
            error_code: PregError::NoError,
            valid_utf8: None,
            cache: RegexCache::default(),
        }
    }
}

/// Sets `PCRE_G(error_code)`.
pub fn set_error(vm: &mut Vm, code: PregError) {
    vm.ext_data::<PcreState>().error_code = code;
}

/// The current `PCRE_G(error_code)`.
pub fn error_code(vm: &mut Vm) -> PregError {
    vm.ext_data::<PcreState>().error_code
}

/// `is_known_valid_utf8()`: whether `subject` was already validated and
/// `start_offset` is not inside a character.
pub fn is_known_valid_utf8(vm: &mut Vm, subject: &PhpStr, start_offset: usize) -> bool {
    let known = vm
        .ext_data::<PcreState>()
        .valid_utf8
        .as_ref()
        .is_some_and(|s| PhpStr::ptr_eq(s, subject));
    // Offset at the end, or not on a UTF-8 continuation byte.
    known && (start_offset == subject.len() || subject[start_offset] & 0xc0 != 0x80)
}

/// Remembers that `subject` is valid UTF-8.
pub fn remember_valid_utf8(vm: &mut Vm, subject: &PhpStr) {
    vm.ext_data::<PcreState>().valid_utf8 = Some(subject.clone());
}

/// `pcre_handle_exec_error()`.
pub fn handle_exec_error(vm: &mut Vm, error: &MatchError) {
    set_error(vm, error.preg_error());
}

/// `pcre_get_compiled_regex_cache()`: looks `regex` (a pattern with
/// delimiters and modifiers) up in the cache or compiles it. A failure emits
/// php-src's warning, sets `preg_last_error()` to `PREG_INTERNAL_ERROR` and
/// returns `Ok(None)`.
pub fn get_compiled_regex(vm: &mut Vm, regex: &[u8]) -> PResult<Option<Rc<Pce>>> {
    if let Some(pce) = vm.ext_data::<PcreState>().cache.get(regex) {
        return Ok(Some(pce));
    }
    match compile_php_pattern(regex) {
        Ok(compiled) => {
            let pce = Rc::new(Pce::new(compiled.regex));
            vm.ext_data::<PcreState>().cache.insert(regex, pce.clone());
            Ok(Some(pce))
        }
        Err(e) => {
            let warned = vm.warning(e.message_bytes());
            set_error(vm, PregError::Internal);
            warned?;
            Ok(None)
        }
    }
}

/// The match limits from `pcre.backtrack_limit` / `pcre.recursion_limit`
/// (`(uint32_t)` casts, as in the ini handlers).
pub fn match_options(vm: &Vm) -> MatchOptions {
    MatchOptions {
        match_limit: vm.ini_int("pcre.backtrack_limit") as u32,
        depth_limit: vm.ini_int("pcre.recursion_limit") as u32,
        ..MatchOptions::default()
    }
}

/// The result of one `pcre2_match()` call.
pub type MatchResult = Result<Option<Captures>, MatchError>;

/// A subject being matched with the options php-src passes to `pcre2_match()`.
pub struct Exec<'a> {
    pub regex: &'a Regex,
    pub subject: &'a [u8],
    pub base: MatchOptions,
}

impl<'a> Exec<'a> {
    pub fn new(vm: &Vm, pce: &'a Pce, subject: &'a [u8]) -> Exec<'a> {
        Exec {
            regex: &pce.regex,
            subject,
            base: match_options(vm),
        }
    }

    /// Re-reads the ini limits (they may have been changed by a callback).
    pub fn refresh_limits(&mut self, vm: &Vm) {
        self.base = match_options(vm);
    }

    /// The first match attempt: a UTF-mode pattern validates the subject
    /// (`options = utf ? 0 : PCRE2_NO_UTF_CHECK`).
    pub fn first(&self, check_utf: bool, start: usize) -> MatchResult {
        let mut o = self.base;
        o.no_utf_check = !check_utf;
        self.regex.match_at(self.subject, start, o)
    }

    /// Subsequent attempts (`PCRE2_NO_UTF_CHECK`).
    pub fn next(&self, start: usize) -> MatchResult {
        let mut o = self.base;
        o.no_utf_check = true;
        self.regex.match_at(self.subject, start, o)
    }

    /// After an empty match: retry at the same offset with
    /// `PCRE2_NOTEMPTY_ATSTART | PCRE2_ANCHORED`.
    pub fn retry_empty(&self, start: usize) -> MatchResult {
        let mut o = self.base;
        o.no_utf_check = true;
        o.not_empty_atstart = true;
        o.anchored = true;
        self.regex.match_at(self.subject, start, o)
    }
}
