//! `parse_ini_string()` / `parse_ini_file()`.
//!
//! A port of the Zend INI scanner (`Zend/zend_ini_scanner.l`), the Bison INI
//! parser (`Zend/zend_ini_parser.y`) and the array-building callbacks of
//! `ext/standard/basic_functions.c` (`php_simple_ini_parser_cb`,
//! `php_ini_parser_cb_with_sections`).
//!
//! * [`Scanner`] reproduces the re2c scanner rule by rule: per start
//!   condition the longest match wins, ties go to the earlier rule, and the
//!   rule actions (including the hand-written loops of the raw-value,
//!   variable-name and double-quoted-string rules) are ported verbatim. The
//!   generated scanner reads one NUL past the input; its
//!   `YYFILL` (`if (YYCURSOR > YYLIMIT) return 0;`) ends the scan with token
//!   0 ("end of file") whenever a pattern would need to read beyond it.
//! * [`Parser`] is Bison's LALR(1) automaton for `zend_ini_parser.y`: the
//!   tables are the ones Bison generates from the grammar and the driver is
//!   a port of the `yacc.c` skeleton, so errors are detected at the same
//!   point (after default reductions) and reported with Bison's verbose
//!   "syntax error, unexpected X, expecting Y or Z" messages.

use std::rc::Rc;

use php_runtime::value::numeric::{self, Numeric};
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `ZEND_INI_SCANNER_NORMAL`: values are parsed (constants, operators, booleans).
pub const INI_SCANNER_NORMAL: i64 = 0;
/// `ZEND_INI_SCANNER_RAW`: option values are not parsed.
pub const INI_SCANNER_RAW: i64 = 1;
/// `ZEND_INI_SCANNER_TYPED`: booleans, null and numbers keep their types.
pub const INI_SCANNER_TYPED: i64 = 2;

pub static PARSE_INI_FILE: NativeFunction = NativeFunction {
    sig: "parse_ini_file(string $filename, bool $process_sections = false, int $scanner_mode = INI_SCANNER_NORMAL): array|false",
    handler: parse_ini_file,
};

pub static PARSE_INI_STRING: NativeFunction = NativeFunction {
    sig: "parse_ini_string(string $ini_string, bool $process_sections = false, int $scanner_mode = INI_SCANNER_NORMAL): array|false",
    handler: parse_ini_string,
};

pub fn register(r: &mut Registry) {
    r.constant("INI_SCANNER_NORMAL", INI_SCANNER_NORMAL);
    r.constant("INI_SCANNER_RAW", INI_SCANNER_RAW);
    r.constant("INI_SCANNER_TYPED", INI_SCANNER_TYPED);
    r.functions(&[&PARSE_INI_FILE, &PARSE_INI_STRING]);
}

// ---------------------------------------------------------------------------
// PHP functions
// ---------------------------------------------------------------------------

fn check_scanner_mode(vm: &mut Vm, mode: i64) -> PResult<()> {
    if !matches!(
        mode,
        INI_SCANNER_NORMAL | INI_SCANNER_RAW | INI_SCANNER_TYPED
    ) {
        return Err(vm.argument_value_error(
            3,
            "must be one of INI_SCANNER_NORMAL, INI_SCANNER_RAW, or INI_SCANNER_TYPED",
        ));
    }
    Ok(())
}

fn array_or_false(r: Option<PhpArray>) -> Value {
    r.map_or(Value::Bool(false), Value::from)
}

/// `PHP_FUNCTION(parse_ini_file)`.
fn parse_ini_file(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let filename = args.str(0).clone();
    // Z_PARAM_PATH_STR
    if filename.as_bytes().contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    let process_sections = args.bool(1);
    let mode = args.int(2);
    if filename.is_empty() {
        return Err(vm.argument_value_error(1, "cannot be empty"));
    }
    check_scanner_mode(vm, mode)?;
    let Some(contents) = read_ini_file(vm, filename.as_bytes())? else {
        return Ok(Value::Bool(false));
    };
    let r = parse_ini(
        vm,
        &contents,
        Some(filename.as_bytes()),
        process_sections,
        mode,
    )?;
    Ok(array_or_false(r))
}

/// `PHP_FUNCTION(parse_ini_string)`.
fn parse_ini_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0).clone();
    let process_sections = args.bool(1);
    let mode = args.int(2);
    check_scanner_mode(vm, mode)?;
    let r = parse_ini(vm, input.as_bytes(), None, process_sections, mode)?;
    Ok(array_or_false(r))
}

/// Reads the file `parse_ini_file()` scans (`zend_stream_fixup` on a handle
/// from `zend_stream_init_filename_ex`). `None` when it cannot be opened,
/// after the "Failed to open stream" warning.
fn read_ini_file(vm: &mut Vm, filename: &[u8]) -> PResult<Option<Vec<u8>>> {
    use super::stream::open;
    let opts = open::USE_PATH | open::REPORT_ERRORS | open::STREAM_OPEN_FOR_INCLUDE;
    let Some(res) = super::wrappers::open_wrapper(vm, filename, b"rb", opts, None, None)? else {
        return Ok(None);
    };
    let data = super::read_to_end(vm, &res)?.unwrap_or_default();
    super::close(vm, &res)?;
    Ok(Some(data))
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// A parser callback (`zend_ini_parser_cb_t` with its callback types).
#[derive(Debug)]
pub enum IniEvent<'a> {
    /// `ZEND_INI_PARSER_ENTRY`: `key = value`, or a bare `key` (no value).
    Entry {
        key: &'a [u8],
        value: Option<&'a Value>,
    },
    /// `ZEND_INI_PARSER_POP_ENTRY`: `key[offset] = value` (`offset` is
    /// empty for `key[]`).
    PopEntry {
        key: &'a [u8],
        offset: &'a [u8],
        value: &'a Value,
    },
    /// `ZEND_INI_PARSER_SECTION`: `[name]`.
    Section { name: &'a [u8] },
}

/// `parse_ini_string()` / `parse_ini_file()` on already loaded contents.
///
/// `filename` `None` parses `input` as a string (`zend_parse_ini_string`:
/// the input ends at its first NUL byte and errors say "in Unknown");
/// `Some(name)` parses file contents and names the file in errors. Syntax
/// errors are reported as warnings and give `Ok(None)` (the function then
/// returns `false`).
pub fn parse_ini(
    vm: &mut Vm,
    input: &[u8],
    filename: Option<&[u8]>,
    process_sections: bool,
    mode: i64,
) -> PResult<Option<PhpArray>> {
    let mut builder = IniArray {
        arr: PhpArray::new(),
        sections: process_sections,
        active: None,
    };
    let ok = parse_ini_with(vm, input, filename, mode, &mut |_, ev| {
        builder.event(ev);
        Ok(())
    })?;
    Ok(ok.then_some(builder.arr))
}

/// `zend_parse_ini_string` (`filename` `None`) / `zend_parse_ini_file`:
/// scans and parses `input`, invoking `callback` for every entry and section
/// as they are recognised. Returns `false` after a syntax error (reported as
/// a warning).
pub fn parse_ini_with(
    vm: &mut Vm,
    input: &[u8],
    filename: Option<&[u8]>,
    mode: i64,
    callback: &mut dyn FnMut(&mut Vm, IniEvent<'_>) -> PResult<()>,
) -> PResult<bool> {
    // zend_ini_prepare_string_for_scanning() measures the string with strlen().
    let input = match filename {
        None => numeric::c_str(input),
        Some(_) => input,
    };
    let mut parser = Parser {
        scanner: Scanner::new(input, mode),
        filename,
        callback,
    };
    parser.parse(vm)
}

// ---------------------------------------------------------------------------
// Callbacks building the result array (ext/standard/basic_functions.c)
// ---------------------------------------------------------------------------

/// `php_simple_ini_parser_cb` and `php_ini_parser_cb_with_sections`.
struct IniArray {
    arr: PhpArray,
    sections: bool,
    /// Key of `BG(active_ini_file_section)` in `arr`.
    active: Option<ArrayKey>,
}

impl IniArray {
    fn event(&mut self, ev: IniEvent<'_>) {
        match ev {
            IniEvent::Section { name } => {
                if self.sections {
                    let key = ArrayKey::from_bytes(name);
                    self.arr.insert(key.clone(), Value::empty_array());
                    self.active = Some(key);
                }
            }
            // Bare string - nothing to do.
            IniEvent::Entry { value: None, .. } => {}
            IniEvent::Entry {
                key,
                value: Some(value),
            } => {
                self.target()
                    .insert(ArrayKey::from_bytes(key), value.clone());
            }
            IniEvent::PopEntry { key, offset, value } => {
                pop_entry(self.target(), key, offset, value);
            }
        }
    }

    fn target(&mut self) -> &mut PhpArray {
        let IniArray { arr, active, .. } = self;
        match active {
            None => arr,
            Some(key) => match arr.get_mut(key) {
                Some(Value::Array(a)) => Rc::make_mut(a),
                _ => unreachable!("the active section is an array"),
            },
        }
    }
}

/// `ZEND_INI_PARSER_POP_ENTRY` in `php_simple_ini_parser_cb`: an entry in
/// the form `x[a]=b` where `x` might need to be an array index.
fn pop_entry(arr: &mut PhpArray, key: &[u8], offset: &[u8], value: &Value) {
    let numeric_key = !(key.len() > 1 && key[0] == b'0')
        && matches!(
            numeric::is_numeric_string(key, false),
            Some(Numeric::Int(_))
        );
    let key = if numeric_key {
        // (zend_ulong) ZEND_STRTOUL(key, NULL, 0)
        ArrayKey::Int(strtoull_base0(key) as i64)
    } else {
        // zend_hash_find(): no numeric-string normalisation.
        ArrayKey::Str(PhpStr::from_bytes(key))
    };
    if arr.get(&key).is_none() {
        arr.insert(key.clone(), Value::empty_array());
    }
    let slot = arr.get_mut(&key).expect("just inserted");
    if !matches!(slot, Value::Array(_)) {
        *slot = Value::empty_array();
    }
    let Value::Array(hash) = slot else {
        unreachable!()
    };
    let hash = Rc::make_mut(hash);
    if offset.is_empty() {
        // add_next_index_zval() fails silently when the next index is taken.
        let _ = hash.append(value.clone());
    } else {
        // array_set_zval_key() with a string key: zend_symtable_update().
        hash.insert(ArrayKey::from_bytes(offset), value.clone());
    }
}

/// C `isspace` in the "C" locale.
fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `strtoull(s, NULL, 0)`: optional whitespace and sign, then a `0x` (hex),
/// `0` (octal) or decimal number; saturates on overflow.
fn strtoull_base0(s: &[u8]) -> u64 {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while is_c_space(at(i)) {
        i += 1;
    }
    let neg = at(i) == b'-';
    if matches!(at(i), b'-' | b'+') {
        i += 1;
    }
    let base: u64 =
        if at(i) == b'0' && matches!(at(i + 1), b'x' | b'X') && at(i + 2).is_ascii_hexdigit() {
            i += 2;
            16
        } else if at(i) == b'0' {
            8
        } else {
            10
        };
    let mut acc: u64 = 0;
    let mut overflow = false;
    loop {
        let d = match at(i) {
            c @ b'0'..=b'9' => (c - b'0') as u64,
            c @ b'a'..=b'z' => (c - b'a') as u64 + 10,
            c @ b'A'..=b'Z' => (c - b'A') as u64 + 10,
            _ => break,
        };
        if d >= base {
            break;
        }
        match acc.checked_mul(base).and_then(|a| a.checked_add(d)) {
            Some(a) => acc = a,
            None => overflow = true,
        }
        i += 1;
    }
    if overflow {
        u64::MAX
    } else if neg {
        acc.wrapping_neg()
    } else {
        acc
    }
}

/// `atoi()` (`(int) strtol(s, NULL, 10)`) on the C string `s`.
fn atoi(s: &[u8]) -> i32 {
    let s = numeric::c_str(s);
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut i = 0;
    while is_c_space(at(i)) {
        i += 1;
    }
    let neg = at(i) == b'-';
    if matches!(at(i), b'-' | b'+') {
        i += 1;
    }
    // strtol() saturates at LONG_MIN / LONG_MAX.
    let mut acc: i128 = 0;
    while at(i).is_ascii_digit() {
        acc = (acc * 10 + (at(i) - b'0') as i128).min(i64::MAX as i128 + 1);
        i += 1;
    }
    let v = if neg { -acc } else { acc };
    let v = v.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    v as i32
}

// ---------------------------------------------------------------------------
// Scanner (Zend/zend_ini_scanner.l)
// ---------------------------------------------------------------------------

/// Scanner start conditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cond {
    Initial,
    Offset,
    SectionValue,
    Value,
    SectionRaw,
    DoubleQuotes,
    VarName,
    VarFallback,
    Raw,
}

/// The generated scanner's `YYFILL` fired: `ini_lex()` returns 0.
struct Fill;

// Token numbers as Bison translates them (`YYTRANSLATE`).
const END: u8 = 0;
const TC_SECTION: u8 = 3;
const TC_RAW: u8 = 4;
const TC_CONSTANT: u8 = 5;
const TC_NUMBER: u8 = 6;
const TC_STRING: u8 = 7;
const TC_WHITESPACE: u8 = 8;
const TC_LABEL: u8 = 9;
const TC_OFFSET: u8 = 10;
const TC_DOLLAR_CURLY: u8 = 11;
const TC_VARNAME: u8 = 12;
const TC_QUOTED_STRING: u8 = 13;
const TC_FALLBACK: u8 = 14;
const BOOL_TRUE: u8 = 15;
const BOOL_FALSE: u8 = 16;
const NULL_NULL: u8 = 17;
const END_OF_LINE: u8 = 18;

/// `YYTRANSLATE` for character tokens.
fn char_token(c: u8) -> u8 {
    match c {
        b'=' => 19,
        b':' => 20,
        b',' => 21,
        b'.' => 22,
        b'"' => 23,
        b'\'' => 24,
        b'^' => 25,
        b'+' => 26,
        b'-' => 27,
        b'/' => 28,
        b'*' => 29,
        b'%' => 30,
        b'$' => 31,
        b'~' => 32,
        b'<' => 33,
        b'>' => 34,
        b'?' => 35,
        b'@' => 36,
        b'{' => 37,
        b'}' => 38,
        b'|' => 39,
        b'&' => 40,
        b'!' => 41,
        b']' => 42,
        b'(' => 43,
        b')' => 44,
        _ => 2,
    }
}

/// A token and its semantic value (`ini_lval`; `None` when the rule leaves
/// it untouched).
struct Token {
    sym: u8,
    lval: Option<Value>,
}

impl Token {
    fn bare(sym: u8) -> Token {
        Token { sym, lval: None }
    }
}

/// The scanner rules, named after their patterns.
#[derive(Clone, Copy, Debug)]
enum Rule {
    /// `<INITIAL>"["`
    SectionStart,
    /// `"'"{SINGLE_QUOTED_CHARS}+"'"`
    RawString,
    /// `"]"{TABS_AND_SPACES}*{NEWLINE}?`
    SectionEnd,
    /// `{LABEL}"["{TABS_AND_SPACES}*`
    OffsetStart,
    /// `<ST_OFFSET>{TABS_AND_SPACES}*"]"`
    OffsetEnd,
    /// `{DOLLAR_CURLY}`
    DollarCurly,
    /// `<ST_VARNAME>":-"`
    FallbackStart,
    /// `<ST_VARNAME>{LABEL_CHAR}`
    VarName,
    /// `<ST_VARNAME,ST_VAR_FALLBACK>"}"`
    VarEnd,
    /// `("true"|"on"|"yes"){TABS_AND_SPACES}*`
    True,
    /// `("false"|"off"|"no"|"none"){TABS_AND_SPACES}*`
    False,
    /// `("null"){TABS_AND_SPACES}*`
    Null,
    /// `<INITIAL>{LABEL}`
    Label,
    /// `<INITIAL>{TABS_AND_SPACES}*[=]{TABS_AND_SPACES}*`
    Assign,
    /// `<ST_RAW>{RAW_VALUE_CHARS}`
    RawValue,
    /// `<ST_SECTION_RAW>{SECTION_RAW_CHARS}+`
    SectionRaw,
    /// `<ST_VALUE,ST_RAW>{TABS_AND_SPACES}*{NEWLINE}`
    ValueEol,
    /// `{CONSTANT}`
    Constant,
    /// `{NUMBER}`
    Number,
    /// `<INITIAL>{TOKENS}`
    Tokens,
    /// `<ST_VALUE>{OPERATORS}{TABS_AND_SPACES}*`
    Operator,
    /// `<ST_VALUE>[=]`
    ValueAssign,
    /// `<ST_VALUE>{VALUE_CHARS}+`
    ValueChars,
    /// `<ST_VAR_FALLBACK>{FALLBACK_CHARS}+`
    FallbackChars,
    /// `<ST_SECTION_VALUE,ST_OFFSET>{SECTION_VALUE_CHARS}+`
    SectionValueChars,
    /// `{TABS_AND_SPACES}*["]`
    QuoteStart,
    /// `<ST_DOUBLE_QUOTES>["]{TABS_AND_SPACES}*`
    QuoteEnd,
    /// `<ST_DOUBLE_QUOTES>[^]`
    Quoted,
    /// `{WHITESPACE}`
    Whitespace,
    /// `<INITIAL,ST_RAW>{TABS_AND_SPACES}+`
    EatWhitespace,
    /// `<INITIAL>{TABS_AND_SPACES}*{NEWLINE}`
    Eol,
    /// `{TABS_AND_SPACES}*[;][^\r\n]*{NEWLINE}`
    Comment,
    /// `<ST_VALUE,ST_RAW>[^]`
    ValueEnd,
    /// `<*>[^]`
    Any,
}

/// The rules active in each start condition, in source order.
fn rules(cond: Cond) -> &'static [Rule] {
    use Rule::*;
    match cond {
        Cond::Initial => &[
            SectionStart,
            OffsetStart,
            True,
            False,
            Null,
            Label,
            Assign,
            Tokens,
            EatWhitespace,
            Eol,
            Comment,
            Any,
        ],
        Cond::Offset => &[
            RawString,
            OffsetEnd,
            DollarCurly,
            Constant,
            Number,
            SectionValueChars,
            QuoteStart,
            Whitespace,
            Any,
        ],
        Cond::SectionValue => &[
            RawString,
            SectionEnd,
            DollarCurly,
            Constant,
            Number,
            SectionValueChars,
            QuoteStart,
            Whitespace,
            Any,
        ],
        Cond::Value => &[
            RawString,
            DollarCurly,
            True,
            False,
            Null,
            ValueEol,
            Constant,
            Number,
            Operator,
            ValueAssign,
            ValueChars,
            QuoteStart,
            Whitespace,
            Comment,
            ValueEnd,
            Any,
        ],
        Cond::SectionRaw => &[SectionEnd, SectionRaw, Any],
        Cond::DoubleQuotes => &[DollarCurly, QuoteEnd, Quoted, Any],
        Cond::VarName => &[FallbackStart, VarName, VarEnd, Any],
        Cond::VarFallback => &[
            DollarCurly,
            VarEnd,
            Constant,
            Number,
            FallbackChars,
            QuoteStart,
            Whitespace,
            Any,
        ],
        Cond::Raw => &[RawValue, ValueEol, EatWhitespace, Comment, ValueEnd, Any],
    }
}

/// `LABEL_CHAR`: `[^=\n\r\t;&|^$~(){}!"[\]\x00]`.
fn is_label_char(c: u8) -> bool {
    !matches!(
        c,
        b'=' | b'\n'
            | b'\r'
            | b'\t'
            | b';'
            | b'&'
            | b'|'
            | b'^'
            | b'$'
            | b'~'
            | b'('
            | b')'
            | b'{'
            | b'}'
            | b'!'
            | b'"'
            | b'['
            | b']'
            | 0
    )
}

/// `TOKENS`: `[:,.[\]"'()&|^+-/*=%$!~<>?@{}]`.
fn is_token_char(c: u8) -> bool {
    matches!(
        c,
        b':' | b',' | b'.' | b'[' | b']' | b'"' | b'\'' | b'(' | b')' | b'&' | b'|' | b'^' | b'+'
            ..=b'/'
                | b'*'
                | b'='
                | b'%'
                | b'$'
                | b'!'
                | b'~'
                | b'<'
                | b'>'
                | b'?'
                | b'@'
                | b'{'
                | b'}'
    )
}

fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// Bit `k` set: a repetition unit of length `k` (1..=3) starts here.
type Units = u8;

/// `LITERAL_DOLLAR`: `"$"([^{\000]|("\\"{ANY_CHAR}))` at a `$`.
fn literal_dollar(s: &Scanner<'_>, p: usize) -> Units {
    let c = s.at(p + 1);
    let mut u = 0;
    if c != b'{' && c != 0 {
        u |= 1 << 2;
    }
    if c == b'\\' {
        u |= 1 << 3;
    }
    u
}

/// `VALUE_CHARS`: `[^$= \t\n\r;&|^~()!"'\000]|{LITERAL_DOLLAR}`.
fn value_unit(s: &Scanner<'_>, p: usize) -> Units {
    match s.at(p) {
        b'$' => literal_dollar(s, p),
        b'=' | b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b'^' | b'~' | b'(' | b')'
        | b'!' | b'"' | b'\'' | 0 => 0,
        _ => 1 << 1,
    }
}

/// `FALLBACK_CHARS`: `[^$\n\r;"'}\\]|("\\"{ANY_CHAR})|{LITERAL_DOLLAR}`.
fn fallback_unit(s: &Scanner<'_>, p: usize) -> Units {
    match s.at(p) {
        b'$' => literal_dollar(s, p),
        b'\\' => 1 << 2,
        b'\n' | b'\r' | b';' | b'"' | b'\'' | b'}' => 0,
        _ => 1 << 1,
    }
}

/// `SECTION_VALUE_CHARS`: `[^$\n\r;"'\]\\]|("\\"{ANY_CHAR})|{LITERAL_DOLLAR}`.
fn section_value_unit(s: &Scanner<'_>, p: usize) -> Units {
    match s.at(p) {
        b'$' => literal_dollar(s, p),
        b'\\' => 1 << 2,
        b'\n' | b'\r' | b';' | b'"' | b'\'' | b']' => 0,
        _ => 1 << 1,
    }
}

/// `zend_ini_scanner_globals` plus the re2c registers.
struct Scanner<'a> {
    buf: &'a [u8],
    cursor: usize,
    /// `yytext` (start of the current token).
    text: usize,
    /// `yyleng`.
    leng: usize,
    cond: Cond,
    stack: Vec<Cond>,
    lineno: u32,
    mode: i64,
}

impl<'a> Scanner<'a> {
    fn new(buf: &'a [u8], mode: i64) -> Scanner<'a> {
        Scanner {
            buf,
            cursor: 0,
            text: 0,
            leng: 0,
            cond: Cond::Initial,
            stack: Vec::new(),
            lineno: 1,
            mode,
        }
    }

    /// The byte at `p`; the buffer is followed by NUL padding
    /// (`ZEND_MMAP_AHEAD`).
    fn at(&self, p: usize) -> u8 {
        self.buf.get(p).copied().unwrap_or(0)
    }

    fn bytes(&self, from: usize, len: usize) -> Vec<u8> {
        (from..from + len).map(|p| self.at(p)).collect()
    }

    fn blanks(&self, mut p: usize) -> usize {
        while is_blank(self.at(p)) {
            p += 1;
        }
        p
    }

    /// `{NEWLINE}` (`"\r"|"\n"|"\r\n"`): its length at `p`.
    fn newline(&self, p: usize) -> usize {
        match self.at(p) {
            b'\r' if self.at(p + 1) == b'\n' => 2,
            b'\r' | b'\n' => 1,
            _ => 0,
        }
    }

    /// End of a run of bytes matching `pred` (which rejects NUL).
    fn run(&self, mut p: usize, pred: fn(u8) -> bool) -> usize {
        while pred(self.at(p)) {
            p += 1;
        }
        p
    }

    fn keyword(&self, p: usize, words: &[&[u8]]) -> usize {
        words
            .iter()
            .filter(|w| {
                w.iter()
                    .enumerate()
                    .all(|(i, c)| self.at(p + i).eq_ignore_ascii_case(c))
            })
            .map(|w| self.blanks(p + w.len()) - p)
            .max()
            .unwrap_or(0)
    }

    /// A loop over a class that includes NUL: running past the buffer's
    /// terminating NUL triggers `YYFILL`.
    fn class_plus(&self, p: usize, pred: fn(u8) -> bool) -> Result<usize, Fill> {
        if !pred(self.at(p)) {
            return Ok(0);
        }
        let mut q = p + 1;
        loop {
            if q > self.buf.len() {
                return Err(Fill);
            }
            if !pred(self.at(q)) {
                return Ok(q - p);
            }
            q += 1;
        }
    }

    /// Longest match of `unit+` at `p`, where a unit has one of the lengths
    /// `unit` reports. Every unit end is a loop head that reads the next
    /// byte; one beyond the terminating NUL triggers `YYFILL`.
    fn units_plus(&self, p: usize, unit: fn(&Scanner<'_>, usize) -> Units) -> Result<usize, Fill> {
        let mut best = 0;
        let mut base = p;
        let mut pending = unit(self, p);
        while pending != 0 {
            let k = pending.trailing_zeros() as usize;
            base += k;
            pending = (pending >> k) & !1;
            if base > self.buf.len() {
                return Err(Fill);
            }
            best = base - p;
            pending |= unit(self, base);
        }
        Ok(best)
    }

    /// `"'"{SINGLE_QUOTED_CHARS}+"'"` (`[^']` includes NUL and newlines).
    fn raw_string(&self, p: usize) -> Result<usize, Fill> {
        if self.at(p) != b'\'' || self.at(p + 1) == b'\'' {
            return Ok(0);
        }
        let mut q = p + 2;
        loop {
            if q > self.buf.len() {
                return Err(Fill);
            }
            if self.at(q) == b'\'' {
                return Ok(q + 1 - p);
            }
            q += 1;
        }
    }

    /// `{TABS_AND_SPACES}*[;][^\r\n]*{NEWLINE}`.
    fn comment(&self, p: usize) -> Result<usize, Fill> {
        let mut q = self.blanks(p);
        if self.at(q) != b';' {
            return Ok(0);
        }
        q += 1;
        loop {
            if q > self.buf.len() {
                return Err(Fill);
            }
            if matches!(self.at(q), b'\r' | b'\n') {
                return Ok(q + self.newline(q) - p);
            }
            q += 1;
        }
    }

    /// `NUMBER`: `[-]?{LNUM}|{DNUM}`.
    fn number(&self, p: usize) -> usize {
        let digits = |q: usize| self.run(q, |c| c.is_ascii_digit());
        let s = p + (self.at(p) == b'-') as usize;
        let lnum = if digits(s) > s { digits(s) - p } else { 0 };
        // DNUM: ([0-9]*[.][0-9]+)|([0-9]+[.][0-9]*)
        let a = digits(p);
        let dnum = if self.at(a) == b'.' {
            let b = digits(a + 1);
            if b > a + 1 || a > p { b - p } else { 0 }
        } else {
            0
        };
        lnum.max(dnum)
    }

    /// Length of `rule`'s longest match at `p` (0: no match).
    fn match_len(&self, rule: Rule, p: usize) -> Result<usize, Fill> {
        use Rule::*;
        let c = self.at(p);
        Ok(match rule {
            SectionStart => (c == b'[') as usize,
            RawString => return self.raw_string(p),
            SectionEnd => {
                if c == b']' {
                    let q = self.blanks(p + 1);
                    q + self.newline(q) - p
                } else {
                    0
                }
            }
            OffsetStart => {
                let q = self.run(p, is_label_char);
                if q > p && self.at(q) == b'[' {
                    self.blanks(q + 1) - p
                } else {
                    0
                }
            }
            OffsetEnd => {
                let q = self.blanks(p);
                if self.at(q) == b']' { q + 1 - p } else { 0 }
            }
            DollarCurly => 2 * (c == b'$' && self.at(p + 1) == b'{') as usize,
            FallbackStart => 2 * (c == b':' && self.at(p + 1) == b'-') as usize,
            VarName => is_label_char(c) as usize,
            VarEnd => (c == b'}') as usize,
            True => self.keyword(p, &[b"true", b"on", b"yes"]),
            False => self.keyword(p, &[b"false", b"off", b"no", b"none"]),
            Null => self.keyword(p, &[b"null"]),
            Label => self.run(p, is_label_char) - p,
            Assign => {
                let q = self.blanks(p);
                if self.at(q) == b'=' {
                    self.blanks(q + 1) - p
                } else {
                    0
                }
            }
            RawValue => !matches!(c, b'\n' | b'\r' | b';' | 0) as usize,
            SectionRaw => return self.class_plus(p, |c| !matches!(c, b']' | b'\n' | b'\r')),
            ValueEol | Eol => {
                let q = self.blanks(p);
                match self.newline(q) {
                    0 => 0,
                    n => q + n - p,
                }
            }
            Constant => {
                if c.is_ascii_alphabetic() || c == b'_' {
                    self.run(p + 1, |c| c.is_ascii_alphanumeric() || c == b'_') - p
                } else {
                    0
                }
            }
            Number => self.number(p),
            Tokens => is_token_char(c) as usize,
            Operator => {
                if matches!(c, b'&' | b'|' | b'^' | b'~' | b'(' | b')' | b'!') {
                    self.blanks(p + 1) - p
                } else {
                    0
                }
            }
            ValueAssign => (c == b'=') as usize,
            ValueChars => return self.units_plus(p, value_unit),
            FallbackChars => return self.units_plus(p, fallback_unit),
            SectionValueChars => return self.units_plus(p, section_value_unit),
            QuoteStart => {
                let q = self.blanks(p);
                if self.at(q) == b'"' { q + 1 - p } else { 0 }
            }
            QuoteEnd => {
                if c == b'"' {
                    self.blanks(p + 1) - p
                } else {
                    0
                }
            }
            Whitespace | EatWhitespace => self.blanks(p) - p,
            Comment => return self.comment(p),
            Quoted | ValueEnd | Any => 1,
        })
    }

    /// The rule re2c selects at the cursor: longest match, earliest rule.
    fn select(&self) -> Result<(Rule, usize), Fill> {
        let mut best = (Rule::Any, 0);
        for &rule in rules(self.cond) {
            let n = self.match_len(rule, self.cursor)?;
            if n > best.1 {
                best = (rule, n);
            }
        }
        Ok(best)
    }

    fn push_state(&mut self, cond: Cond) {
        self.stack.push(self.cond);
        self.cond = cond;
    }

    fn pop_state(&mut self) {
        self.cond = self.stack.pop().unwrap_or(Cond::Initial);
    }

    /// `EAT_LEADING_WHITESPACE()`.
    fn eat_leading_whitespace(&mut self) {
        while self.leng > 0 && is_blank(self.at(self.text)) {
            self.text += 1;
            self.leng -= 1;
        }
    }

    /// `EAT_TRAILING_WHITESPACE_EX(ch)` (`None` for `'X'`).
    fn eat_trailing_whitespace(&mut self, ch: Option<u8>) {
        while self.leng > 0 {
            let c = self.at(self.text + self.leng - 1);
            if ch == Some(c) || matches!(c, b'\n' | b'\r' | b'\t' | b' ') {
                self.leng -= 1;
            } else {
                break;
            }
        }
    }

    /// `RETURN_TOKEN(type, yytext, yyleng)`.
    fn token(&self, sym: u8) -> Token {
        let s = self.bytes(self.text, self.leng);
        self.token_with(sym, s)
    }

    /// `RETURN_TOKEN(type, str, len)`: typed values in TYPED mode while
    /// scanning an option value.
    fn token_with(&self, sym: u8, s: Vec<u8>) -> Token {
        let typed = self.mode == INI_SCANNER_TYPED && matches!(self.cond, Cond::Value | Cond::Raw);
        let v = match sym {
            BOOL_TRUE | BOOL_FALSE if typed => Value::Bool(sym == BOOL_TRUE),
            NULL_NULL if typed => Value::Null,
            _ => Value::from(s),
        };
        Token { sym, lval: Some(v) }
    }

    /// `<ST_VARNAME>":-"` and the `fallback_lexing` label.
    fn fallback_start(&mut self) -> Token {
        self.pop_state();
        self.push_state(Cond::VarFallback);
        Token::bare(TC_FALLBACK)
    }

    /// `ini_lex()`.
    fn lex(&mut self) -> Token {
        loop {
            self.text = self.cursor;
            // Detect EOF.
            if self.cursor >= self.buf.len() {
                if matches!(self.cond, Cond::Value | Cond::Raw) {
                    self.cond = Cond::Initial;
                    return Token::bare(END_OF_LINE);
                }
                return Token::bare(END);
            }
            // Eat any UTF-8 BOM we find in the first 3 bytes.
            if self.cursor == 0
                && self.cursor + 3 < self.buf.len()
                && self.buf.starts_with(b"\xEF\xBB\xBF")
            {
                self.cursor += 3;
                continue;
            }
            let Ok((rule, n)) = self.select() else {
                return Token::bare(END);
            };
            self.cursor += n;
            self.leng = n;
            return match rule {
                Rule::SectionStart => {
                    self.cond = if self.mode == INI_SCANNER_RAW {
                        Cond::SectionRaw
                    } else {
                        Cond::SectionValue
                    };
                    Token::bare(TC_SECTION)
                }
                Rule::RawString => {
                    // Eat leading and trailing single quotes.
                    if self.at(self.text) == b'\'' && self.at(self.text + self.leng - 1) == b'\'' {
                        self.text += 1;
                        self.leng -= 2;
                    }
                    self.token(TC_RAW)
                }
                Rule::SectionEnd => {
                    self.cond = Cond::Initial;
                    self.lineno += 1;
                    Token::bare(char_token(b']'))
                }
                Rule::OffsetStart => {
                    self.eat_leading_whitespace();
                    self.eat_trailing_whitespace(Some(b'['));
                    self.cond = Cond::Offset;
                    self.token(TC_OFFSET)
                }
                Rule::OffsetEnd => {
                    self.cond = Cond::Initial;
                    Token::bare(char_token(b']'))
                }
                Rule::DollarCurly => {
                    self.push_state(Cond::VarName);
                    Token::bare(TC_DOLLAR_CURLY)
                }
                Rule::FallbackStart => self.fallback_start(),
                Rule::VarName => self.var_name(),
                Rule::VarEnd => {
                    self.pop_state();
                    Token::bare(char_token(b'}'))
                }
                Rule::True => self.token_with(BOOL_TRUE, b"1".to_vec()),
                Rule::False => self.token_with(BOOL_FALSE, Vec::new()),
                Rule::Null => self.token_with(NULL_NULL, Vec::new()),
                Rule::Label => {
                    self.eat_leading_whitespace();
                    self.eat_trailing_whitespace(None);
                    self.token(TC_LABEL)
                }
                Rule::Assign => {
                    self.cond = if self.mode == INI_SCANNER_RAW {
                        Cond::Raw
                    } else {
                        Cond::Value
                    };
                    Token::bare(char_token(b'='))
                }
                Rule::RawValue => self.raw_value(),
                Rule::SectionRaw => self.token(TC_RAW),
                Rule::ValueEol => {
                    self.cond = Cond::Initial;
                    self.lineno += 1;
                    Token::bare(END_OF_LINE)
                }
                Rule::Constant => self.token(TC_CONSTANT),
                Rule::Number => self.token(TC_NUMBER),
                Rule::Tokens | Rule::Operator => Token::bare(char_token(self.at(self.text))),
                Rule::ValueAssign => {
                    // Make `=` used in an option value trigger an error.
                    self.cursor = self.text;
                    self.leng = 0;
                    self.cond = Cond::Initial;
                    Token::bare(END_OF_LINE)
                }
                Rule::ValueChars | Rule::FallbackChars | Rule::SectionValueChars => {
                    self.token(TC_STRING)
                }
                Rule::QuoteStart => {
                    self.push_state(Cond::DoubleQuotes);
                    Token::bare(char_token(b'"'))
                }
                Rule::QuoteEnd => {
                    self.pop_state();
                    Token::bare(char_token(b'"'))
                }
                Rule::Quoted => self.quoted(),
                Rule::Whitespace => self.token(TC_WHITESPACE),
                Rule::EatWhitespace => continue,
                Rule::Eol => {
                    self.lineno += 1;
                    Token::bare(END_OF_LINE)
                }
                Rule::Comment => {
                    self.cond = Cond::Initial;
                    self.lineno += 1;
                    Token::bare(END_OF_LINE)
                }
                Rule::ValueEnd => {
                    self.cond = Cond::Initial;
                    Token::bare(END_OF_LINE)
                }
                Rule::Any => Token::bare(END),
            };
        }
    }

    /// `<ST_VARNAME>{LABEL_CHAR}`: the variable name runs up to a delimiter
    /// (`:` only when followed by `-`).
    fn var_name(&mut self) -> Token {
        if self.at(self.cursor) == b':' && self.at(self.cursor + 1) == b'-' {
            self.cursor += 1;
            return self.fallback_start();
        }
        while self.cursor < self.buf.len() {
            let c = self.buf[self.cursor];
            self.cursor += 1;
            let stop = match c {
                b'=' | b'\n' | b'\r' | b'\t' | b';' | b'&' | b'|' | b'^' | b'$' | b'~' | b'('
                | b')' | b'{' | b'}' | b'!' | b'"' | b'[' | b']' => true,
                b':' => self.at(self.cursor) == b'-',
                _ => false,
            };
            if stop {
                self.cursor -= 1;
                self.leng = self.cursor - self.text;
                break;
            }
        }
        self.eat_leading_whitespace();
        self.eat_trailing_whitespace(None);
        self.token(TC_VARNAME)
    }

    /// `<ST_RAW>{RAW_VALUE_CHARS}`: the rest of the line, up to a `;` that is
    /// not inside a double-quoted value.
    fn raw_value(&mut self) -> Token {
        let mut sc = None;
        self.eat_leading_whitespace();
        while self.cursor < self.buf.len() {
            match self.buf[self.cursor] {
                b'\n' | b'\r' => break,
                b';' => {
                    if sc.is_none() {
                        sc = Some(self.cursor);
                    }
                    self.cursor += 1;
                }
                b'"' => {
                    if self.at(self.text) == b'"' {
                        sc = None;
                    }
                    self.cursor += 1;
                }
                _ => self.cursor += 1,
            }
        }
        self.leng = sc.unwrap_or(self.cursor) - self.text;
        self.eat_trailing_whitespace(None);
        // Eat leading and trailing double quotes.
        if self.leng > 1 && self.at(self.text) == b'"' && self.at(self.text + self.leng - 1) == b'"'
        {
            self.text += 1;
            self.leng -= 2;
        }
        self.token(TC_RAW)
    }

    /// `<ST_DOUBLE_QUOTES>[^]`: double-quoted contents up to the closing
    /// quote or a `${`.
    fn quoted(&mut self) -> Token {
        let limit = self.buf.len();
        let mut s = self.text;
        while s < limit {
            let c = self.buf[s];
            s += 1;
            let end = match c {
                b'"' => true,
                b'$' => s < limit && self.buf[s] == b'{',
                b'\\' => {
                    let mut end = false;
                    if s < limit {
                        let escaped = self.buf[s];
                        s += 1;
                        // A special case for Windows paths, e.g. key="C:\path\"
                        end = escaped == b'"'
                            && (s >= limit || self.buf[s] == b'\n' || self.buf[s] == b'\r');
                    }
                    end
                }
                _ => false,
            };
            if end {
                s -= 1;
                break;
            }
        }
        self.cursor = s;
        self.leng = s - self.text;
        let raw = self.bytes(self.text, self.leng);
        let v = self.escape_string(&raw);
        Token {
            sym: TC_QUOTED_STRING,
            lval: Some(Value::from(v)),
        }
    }

    /// `zend_ini_escape_string(lval, str, len, '"')`: `\"`, `\\` and `\$`
    /// lose their backslash; counts the newlines of the value.
    fn escape_string(&mut self, src: &[u8]) -> Vec<u8> {
        let at = |i: usize| src.get(i).copied().unwrap_or(0);
        let mut out = Vec::with_capacity(src.len());
        let mut s = 0;
        while s < src.len() {
            if src[s] == b'\\' {
                s += 1;
                if s >= src.len() {
                    out.push(b'\\');
                    continue;
                }
                match src[s] {
                    b'"' | b'\\' | b'$' => out.push(src[s]),
                    c => {
                        out.push(b'\\');
                        out.push(c);
                    }
                }
            } else {
                out.push(src[s]);
            }
            if src[s] == b'\n' || (src[s] == b'\r' && at(s + 1) != b'\n') {
                self.lineno += 1;
            }
            s += 1;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Parser (Zend/zend_ini_parser.y)
// ---------------------------------------------------------------------------
//
// LALR(1) tables generated by Bison from zend_ini_parser.y. Rule `r` of the
// tables is rule `r - 1` of Bison's report (rule 1 is `$accept`).

const YYFINAL: usize = 2;
const YYLAST: i32 = 143;
const YYNTOKENS: usize = 45;
const YYPACT_NINF: i32 = -46;
const YYTERROR: i32 = 1;
const YYMAXDEPTH: usize = 10000;

static YYR1: [u8; 54] = [
    0, 45, 46, 46, 47, 47, 47, 47, 47, 48, 48, 49, 49, 49, 49, 49, 50, 50, 51, 51, 51, 52, 52, 52,
    52, 52, 52, 53, 53, 53, 53, 53, 53, 54, 54, 54, 54, 54, 54, 54, 55, 55, 56, 56, 57, 57, 57, 57,
    57, 58, 58, 58, 58, 58,
];

static YYR2: [u8; 54] = [
    0, 2, 2, 0, 3, 3, 5, 1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 0, 2, 2, 0, 1, 1, 3, 2, 2, 4, 1, 1, 3, 2, 2,
    4, 1, 3, 3, 3, 2, 2, 3, 3, 5, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

static YYDEFACT: [u8; 76] = [
    3, 0, 1, 10, 7, 17, 8, 2, 45, 44, 46, 47, 48, 0, 20, 0, 9, 21, 22, 0, 50, 49, 51, 52, 53, 20,
    0, 16, 27, 28, 0, 0, 4, 20, 24, 25, 12, 13, 14, 15, 0, 0, 0, 5, 33, 11, 0, 0, 20, 30, 31, 43,
    40, 19, 23, 18, 0, 37, 38, 0, 0, 0, 0, 29, 0, 0, 42, 0, 26, 39, 36, 34, 35, 6, 32, 41,
];

static YYDEFGOTO: [i8; 14] = [-1, 1, 7, 15, 43, 26, 31, 16, 44, 45, 28, 67, 18, 29];

static YYPACT: [i8; 76] = [
    -46, 118, -46, 73, -17, 81, -46, -46, -46, -46, -46, -46, -46, 0, -46, -34, 94, -46, -46, -1,
    -46, -46, -46, -46, -46, -46, -31, 102, -46, -46, 6, 59, -46, -46, -46, -46, -46, -46, -46,
    -46, 28, 28, 28, -46, 102, 25, 80, 2, -46, -46, -46, 81, -46, -46, -46, -46, 109, -46, -46, 72,
    28, 28, 28, -46, -1, 120, 102, -20, -46, -46, -46, -46, -46, -46, -46, -46,
];

static YYPGOTO: [i8; 14] = [
    -46, -46, -46, -46, -45, -46, 4, -46, -4, 14, -3, -46, 7, -18,
];

static YYTABLE: [u8; 144] = [
    17, 27, 19, 20, 21, 22, 23, 24, 32, 50, 13, 47, 30, 34, 36, 37, 38, 39, 75, 73, 51, 64, 25, 35,
    49, 0, 50, 0, 55, 46, 0, 40, 20, 21, 22, 23, 24, 56, 0, 13, 41, 49, 42, 55, 52, 0, 0, 66, 50,
    0, 60, 25, 65, 55, 57, 58, 59, 0, 0, 0, 40, 0, 55, 49, 61, 62, 0, 0, 0, 41, 13, 42, 53, 0, 70,
    71, 72, 8, 9, 10, 11, 12, 54, 0, 13, 20, 21, 22, 23, 24, 0, 13, 13, 53, 0, 0, 14, 60, 8, 9, 10,
    11, 12, 63, 25, 13, 20, 21, 22, 23, 24, 61, 62, 13, 0, 0, 69, 33, 2, 0, 13, 3, 53, 0, 0, 48, 0,
    4, 5, 0, 0, 13, 68, 53, 0, 0, 6, 0, 0, 0, 0, 0, 0, 74,
];

static YYCHECK: [i8; 144] = [
    3, 5, 19, 4, 5, 6, 7, 8, 42, 27, 11, 42, 12, 16, 15, 16, 17, 18, 38, 64, 14, 19, 23, 16, 27,
    -1, 44, -1, 31, 25, -1, 32, 4, 5, 6, 7, 8, 33, -1, 11, 41, 44, 43, 46, 38, -1, -1, 51, 66, -1,
    25, 23, 48, 56, 40, 41, 42, -1, -1, -1, 32, -1, 65, 66, 39, 40, -1, -1, -1, 41, 11, 43, 13, -1,
    60, 61, 62, 4, 5, 6, 7, 8, 23, -1, 11, 4, 5, 6, 7, 8, -1, 11, 11, 13, -1, -1, 23, 25, 4, 5, 6,
    7, 8, 23, 23, 11, 4, 5, 6, 7, 8, 39, 40, 11, -1, -1, 44, 23, 0, -1, 11, 3, 13, -1, -1, 23, -1,
    9, 10, -1, -1, 11, 23, 13, -1, -1, 18, -1, -1, -1, -1, -1, -1, 23,
];

/// Token names as Bison's `yytnamerr` prints them.
static YYTNAME: [&str; YYNTOKENS] = [
    "end of file",
    "error",
    "invalid token",
    "TC_SECTION",
    "TC_RAW",
    "TC_CONSTANT",
    "TC_NUMBER",
    "TC_STRING",
    "TC_WHITESPACE",
    "TC_LABEL",
    "TC_OFFSET",
    "TC_DOLLAR_CURLY",
    "TC_VARNAME",
    "TC_QUOTED_STRING",
    "TC_FALLBACK",
    "BOOL_TRUE",
    "BOOL_FALSE",
    "NULL_NULL",
    "END_OF_LINE",
    "'='",
    "':'",
    "','",
    "'.'",
    "'\"'",
    "'\\''",
    "'^'",
    "'+'",
    "'-'",
    "'/'",
    "'*'",
    "'%'",
    "'$'",
    "'~'",
    "'<'",
    "'>'",
    "'?'",
    "'@'",
    "'{'",
    "'}'",
    "'|'",
    "'&'",
    "'!'",
    "']'",
    "'('",
    "')'",
];

/// `yysyntax_error`: Bison's verbose message (at most four expected tokens).
fn syntax_error_message(state: usize, token: Option<u8>) -> String {
    let Some(token) = token else {
        return "syntax error".to_string();
    };
    let mut args = vec![YYTNAME[token as usize]];
    let n = YYPACT[state] as i32;
    if n != YYPACT_NINF {
        let begin = if n < 0 { -n } else { 0 };
        let end = (YYLAST - n + 1).min(YYNTOKENS as i32);
        for x in begin..end {
            if YYCHECK[(x + n) as usize] as i32 == x && x != YYTERROR {
                if args.len() == 5 {
                    args.truncate(1);
                    break;
                }
                args.push(YYTNAME[x as usize]);
            }
        }
    }
    match args.as_slice() {
        [unexpected] => format!("syntax error, unexpected {unexpected}"),
        [unexpected, expected @ ..] => format!(
            "syntax error, unexpected {unexpected}, expecting {}",
            expected.join(" or ")
        ),
        [] => unreachable!(),
    }
}

/// A semantic value: the `zval` plus its `Z_EXTRA` flag
/// (`INI_ZVAL_IS_NUMBER`).
#[derive(Clone, Default)]
struct Sv {
    val: Value,
    number: bool,
}

impl Sv {
    fn new(val: Value) -> Sv {
        Sv { val, number: false }
    }

    /// `zend_ini_init_string()`.
    fn empty_string() -> Sv {
        Sv::new(Value::from(PhpStr::new()))
    }
}

struct Parser<'a, 'cb> {
    scanner: Scanner<'a>,
    filename: Option<&'a [u8]>,
    callback: &'cb mut dyn FnMut(&mut Vm, IniEvent<'_>) -> PResult<()>,
}

impl Parser<'_, '_> {
    /// `ini_parse()`: the `yacc.c` skeleton. `Ok(false)` after a syntax error.
    fn parse(&mut self, vm: &mut Vm) -> PResult<bool> {
        let mut states: Vec<usize> = vec![0];
        let mut values: Vec<Sv> = vec![Sv::default()];
        let mut lookahead: Option<Token> = None;
        loop {
            // yysetstate
            if states.len() >= YYMAXDEPTH {
                self.error(vm, "memory exhausted")?;
                return Ok(false);
            }
            let state = *states.last().expect("parser stack");
            if state == YYFINAL {
                return Ok(true);
            }
            // yybackup
            let pact = YYPACT[state] as i32;
            if pact != YYPACT_NINF {
                let token = lookahead.get_or_insert_with(|| self.scanner.lex());
                let idx = pact + token.sym as i32;
                if (0..=YYLAST).contains(&idx) && YYCHECK[idx as usize] as i32 == token.sym as i32 {
                    // This grammar's table only holds shifts.
                    let next = YYTABLE[idx as usize] as usize;
                    if next == 0 {
                        return self.syntax_error(vm, state, lookahead.as_ref());
                    }
                    let token = lookahead.take().expect("lookahead");
                    states.push(next);
                    values.push(Sv::new(token.lval.unwrap_or_default()));
                    continue;
                }
            }
            // yydefault
            let rule = YYDEFACT[state] as usize;
            if rule == 0 {
                return self.syntax_error(vm, state, lookahead.as_ref());
            }
            // yyreduce
            let base = values.len() - YYR2[rule] as usize;
            let val = self.reduce(vm, rule, &mut values[base..])?;
            values.truncate(base);
            states.truncate(base);
            let lhs = YYR1[rule] as usize - YYNTOKENS;
            let top = *states.last().expect("parser stack");
            let goto = YYPGOTO[lhs] as i32 + top as i32;
            let next =
                if (0..=YYLAST).contains(&goto) && YYCHECK[goto as usize] as i32 == top as i32 {
                    YYTABLE[goto as usize] as usize
                } else {
                    YYDEFGOTO[lhs] as usize
                };
            states.push(next);
            values.push(val);
        }
    }

    /// `yyerrlab`: the grammar has no `error` token, so the parse aborts.
    fn syntax_error(&mut self, vm: &mut Vm, state: usize, token: Option<&Token>) -> PResult<bool> {
        let msg = syntax_error_message(state, token.map(|t| t.sym));
        self.error(vm, &msg)?;
        Ok(false)
    }

    /// `ini_error()`: `"<msg> in <file> on line <n>\n"` as an E_WARNING.
    fn error(&mut self, vm: &mut Vm, msg: &str) -> PResult<()> {
        let mut buf = msg.as_bytes().to_vec();
        buf.extend_from_slice(b" in ");
        buf.extend_from_slice(self.filename.unwrap_or(b"Unknown"));
        buf.extend_from_slice(format!(" on line {}\n", self.scanner.lineno).as_bytes());
        vm.engine_warning(PhpStr::from_vec(buf))
    }

    fn emit(&mut self, vm: &mut Vm, ev: IniEvent<'_>) -> PResult<()> {
        (self.callback)(vm, ev)
    }

    /// The semantic actions; `v` holds `$1..$n`.
    fn reduce(&mut self, vm: &mut Vm, rule: usize, v: &mut [Sv]) -> PResult<Sv> {
        let take = |v: &mut [Sv], i: usize| std::mem::take(&mut v[i]);
        Ok(match rule {
            // statement: TC_SECTION section_string_or_value ']'
            4 => {
                let name = to_str(vm, &v[1].val)?;
                self.emit(vm, IniEvent::Section { name: &name })?;
                Sv::default()
            }
            // statement: TC_LABEL '=' string_or_value
            5 => {
                let key = to_str(vm, &v[0].val)?;
                let value = take(v, 2).val;
                self.emit(
                    vm,
                    IniEvent::Entry {
                        key: &key,
                        value: Some(&value),
                    },
                )?;
                Sv::default()
            }
            // statement: TC_OFFSET option_offset ']' '=' string_or_value
            6 => {
                let key = to_str(vm, &v[0].val)?;
                let offset = to_str(vm, &v[1].val)?;
                let value = take(v, 4).val;
                self.emit(
                    vm,
                    IniEvent::PopEntry {
                        key: &key,
                        offset: &offset,
                        value: &value,
                    },
                )?;
                Sv::default()
            }
            // statement: TC_LABEL
            7 => {
                let key = to_str(vm, &v[0].val)?;
                self.emit(
                    vm,
                    IniEvent::Entry {
                        key: &key,
                        value: None,
                    },
                )?;
                Sv::default()
            }
            // statement_list (both), statement: END_OF_LINE
            2 | 3 | 8 => Sv::default(),
            // `$$ = $1`
            9 | 12 | 13 | 14 | 16 | 21 | 22 | 27 | 28 | 33 | 42 | 44..=48 | 50 | 52 | 53 => {
                take(v, 0)
            }
            // %empty, END_OF_LINE: zend_ini_init_string(&$$)
            10 | 15 | 17 | 20 | 43 => Sv::empty_string(),
            // string_or_value: expr
            11 => self.normalize_value(take(v, 0)),
            // ... cfg_var_ref / TC_QUOTED_STRING / constant_literal / constant_string
            18 | 19 | 24 | 25 | 30 | 31 => add_string(vm, take(v, 0), &v[1].val)?,
            // ... '"' encapsed_list '"'
            26 | 32 => add_string(vm, take(v, 0), &v[2].val)?,
            // '"' encapsed_list '"', '(' expr ')': `$$ = $2`
            23 | 29 | 39 => take(v, 1),
            34 => self.do_op(b'|', &v[0], Some(&v[2])),
            35 => self.do_op(b'&', &v[0], Some(&v[2])),
            36 => self.do_op(b'^', &v[0], Some(&v[2])),
            37 => self.do_op(b'~', &v[1], None),
            38 => self.do_op(b'!', &v[1], None),
            // cfg_var_ref: TC_DOLLAR_CURLY TC_VARNAME '}'
            40 => Sv::new(get_var(vm, &v[1].val, None)?),
            // cfg_var_ref: TC_DOLLAR_CURLY TC_VARNAME TC_FALLBACK fallback '}'
            41 => Sv::new(get_var(vm, &v[1].val, Some(&v[3].val))?),
            // constant_string: TC_CONSTANT
            49 => get_constant(vm, take(v, 0))?,
            // constant_string: TC_NUMBER
            51 => Sv {
                number: true,
                ..take(v, 0)
            },
            _ => unreachable!("rule {rule} is never reduced"),
        })
    }

    /// `normalize_value()`: in TYPED mode a value that starts with a number
    /// becomes an int or float if the whole value is numeric.
    fn normalize_value(&self, sv: Sv) -> Sv {
        if self.scanner.mode != INI_SCANNER_TYPED || !sv.number {
            return sv;
        }
        let Value::String(s) = &sv.val else {
            return sv;
        };
        // convert_to_number()
        match numeric::is_numeric_string_ex(s.as_bytes(), false) {
            Some(info) => match info.num {
                Numeric::Int(i) => Sv {
                    val: Value::Int(i),
                    number: true,
                },
                Numeric::Float(d) if info.overflow == 0 => Sv {
                    val: Value::Float(d),
                    number: true,
                },
                Numeric::Float(_) => sv,
            },
            None => sv,
        }
    }

    /// `zend_ini_do_op()`: C `int` arithmetic on `atoi()`-converted operands.
    fn do_op(&self, op: u8, op1: &Sv, op2: Option<&Sv>) -> Sv {
        let i1 = int_val(&op1.val);
        let i2 = op2.map_or(0, |o| int_val(&o.val));
        let r = match op {
            b'|' => i1 | i2,
            b'&' => i1 & i2,
            b'^' => i1 ^ i2,
            b'~' => !i1,
            b'!' => (i1 == 0) as i32,
            _ => 0,
        };
        Sv::new(if self.scanner.mode != INI_SCANNER_TYPED {
            Value::from(r.to_string())
        } else {
            Value::Int(r as i64)
        })
    }
}

/// `get_int_val()`.
fn int_val(v: &Value) -> i32 {
    match v {
        Value::Int(i) => *i as i32,
        Value::Float(d) => *d as i32,
        Value::String(s) => atoi(s.as_bytes()),
        _ => 0,
    }
}

/// String conversion of a parser value (always a string in practice).
fn to_str(vm: &mut Vm, v: &Value) -> PResult<PhpStr> {
    match v {
        Value::String(s) => Ok(s.clone()),
        other => vm.to_php_string(other),
    }
}

/// `zend_ini_add_string()`: `$$` keeps `$1`'s `Z_EXTRA`.
fn add_string(vm: &mut Vm, op1: Sv, op2: &Value) -> PResult<Sv> {
    let mut s = to_str(vm, &op1.val)?;
    let tail = to_str(vm, op2)?;
    s.push_bytes(tail.as_bytes());
    Ok(Sv {
        val: Value::from(s),
        number: op1.number,
    })
}

/// `zend_ini_get_constant()`: a defined constant's value as a string, else
/// the name itself.
fn get_constant(vm: &mut Vm, name: Sv) -> PResult<Sv> {
    let Value::String(n) = &name.val else {
        return Ok(name);
    };
    // If name contains ':' it is not a constant. Bug #26893.
    if n.as_bytes().contains(&b':') {
        return Ok(name);
    }
    let Some(c) = vm.constant(n.as_bytes()) else {
        return Ok(name);
    };
    let s = to_str(vm, &c)?;
    Ok(Sv {
        val: Value::from(s),
        number: name.number,
    })
}

/// `zend_ini_get_var()`: a configuration directive, else an environment
/// variable, else the fallback (or `""`).
fn get_var(vm: &mut Vm, name: &Value, fallback: Option<&Value>) -> PResult<Value> {
    let name = to_str(vm, name)?;
    if let Some(v) = configuration_directive(vm, name.as_bytes()) {
        return Ok(Value::from(v));
    }
    if let Some(v) = getenv(numeric::c_str(name.as_bytes())) {
        return Ok(Value::from(v));
    }
    match fallback {
        Some(f) => {
            let f = to_str(vm, f)?;
            Ok(Value::from(numeric::c_str(f.as_bytes())))
        }
        None => Ok(Value::from(PhpStr::new())),
    }
}

/// `zend_get_configuration_directive()`: the directive's startup value
/// (php.ini / `-d`), unaffected by `ini_set()`.
fn configuration_directive(vm: &Vm, name: &[u8]) -> Option<PhpStr> {
    let e = vm.ini.get(name)?;
    match &e.orig_value {
        Some(orig) => orig.clone(),
        None => e.value.clone(),
    }
}

/// `getenv()`.
fn getenv(name: &[u8]) -> Option<Vec<u8>> {
    if name.is_empty() || name.contains(&b'=') {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        std::env::var_os(std::ffi::OsStr::from_bytes(name)).map(|v| v.into_vec())
    }
    #[cfg(not(unix))]
    {
        std::env::var_os(String::from_utf8_lossy(name).as_ref())
            .map(|v| v.to_string_lossy().into_owned().into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::ini::{IniDef, modifiable};
    use php_runtime::value::float_fmt;
    use std::path::PathBuf;

    // ----- helpers -------------------------------------------------------

    fn var_dump_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
        out.extend(std::iter::repeat_n(b' ', indent));
        match v {
            Value::Null => out.extend_from_slice(b"NULL\n"),
            Value::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
            Value::Int(i) => out.extend_from_slice(format!("int({i})\n").as_bytes()),
            Value::Float(d) => {
                out.extend_from_slice(format!("float({})\n", float_fmt::repr(*d, -1)).as_bytes())
            }
            Value::String(s) => {
                out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
                out.extend_from_slice(s.as_bytes());
                out.extend_from_slice(b"\"\n");
            }
            Value::Array(a) => {
                out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
                for (k, v) in a.iter() {
                    out.extend(std::iter::repeat_n(b' ', indent + 2));
                    match k {
                        ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                        ArrayKey::Str(s) => {
                            out.extend_from_slice(b"[\"");
                            out.extend_from_slice(s.as_bytes());
                            out.extend_from_slice(b"\"]=>\n");
                        }
                    }
                    var_dump_into(out, v, indent + 2);
                }
                out.extend(std::iter::repeat_n(b' ', indent));
                out.extend_from_slice(b"}\n");
            }
            other => panic!("unexpected value {other:?}"),
        }
    }

    /// `var_dump($v)`.
    fn var_dump(v: &Value) -> String {
        let mut out = Vec::new();
        var_dump_into(&mut out, v, 0);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn print_r_into(out: &mut Vec<u8>, v: &Value, indent: usize) {
        match v {
            Value::Array(a) => {
                out.extend_from_slice(b"Array\n");
                out.extend(std::iter::repeat_n(b' ', indent));
                out.extend_from_slice(b"(\n");
                for (k, v) in a.iter() {
                    out.extend(std::iter::repeat_n(b' ', indent + 4));
                    out.extend_from_slice(format!("[{k}] => ").as_bytes());
                    print_r_into(out, v, indent + 8);
                    out.push(b'\n');
                }
                out.extend(std::iter::repeat_n(b' ', indent));
                out.extend_from_slice(b")\n");
            }
            Value::String(s) => out.extend_from_slice(s.as_bytes()),
            Value::Bool(true) => out.push(b'1'),
            Value::Bool(false) | Value::Null => {}
            Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            other => panic!("unexpected value {other:?}"),
        }
    }

    /// `print_r($v)`.
    fn print_r(v: &Value) -> String {
        let mut out = Vec::new();
        print_r_into(&mut out, v, 0);
        String::from_utf8_lossy(&out).into_owned()
    }

    fn parse_str(t: &mut TestVm, s: &[u8], sections: bool, mode: i64) -> Value {
        t.call(
            "parse_ini_string",
            vec![Value::from(s), Value::Bool(sections), Value::Int(mode)],
        )
        .unwrap()
    }

    /// Diagnostics plus `var_dump(parse_ini_string($s, $sections, $mode))`.
    fn dump_mode(s: &str, sections: bool, mode: i64) -> String {
        let mut t = TestVm::new();
        let v = parse_str(&mut t, s.as_bytes(), sections, mode);
        t.output() + &var_dump(&v)
    }

    fn dump(s: &str) -> String {
        dump_mode(s, false, INI_SCANNER_NORMAL)
    }

    fn parse_file(t: &mut TestVm, path: &std::path::Path, sections: bool, mode: i64) -> Value {
        t.call(
            "parse_ini_file",
            vec![
                Value::from(path.to_str().unwrap()),
                Value::Bool(sections),
                Value::Int(mode),
            ],
        )
        .unwrap()
    }

    fn phpt_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/phpt")
    }

    /// The `--EXPECT--` section of a .phpt file.
    fn phpt_expect(rel: &str) -> String {
        let src = std::fs::read_to_string(phpt_dir().join(rel)).unwrap();
        let start = src.find("--EXPECT--\n").unwrap() + "--EXPECT--\n".len();
        let rest = &src[start..];
        let end = rest.find("\n--CLEAN--").unwrap_or(rest.len());
        rest[..end].trim_end().to_string()
    }

    /// A scratch file holding `contents`, removed on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &[u8]) -> TempFile {
            let path =
                std::env::temp_dir().join(format!("php_ini_{}_{}", std::process::id(), name));
            std::fs::write(&path, contents).unwrap();
            TempFile(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// The displayed `ini_error()` warning (its message ends with "\n").
    fn ini_warning(msg: &str) -> String {
        format!("\nWarning: {msg}\n in /t.php on line 3\n")
    }

    // ----- registration and argument validation --------------------------

    #[test]
    fn constants() {
        let t = TestVm::new();
        assert_eq!(
            t.vm.constant("INI_SCANNER_NORMAL").unwrap().as_int(),
            Some(0)
        );
        assert_eq!(t.vm.constant("INI_SCANNER_RAW").unwrap().as_int(), Some(1));
        assert_eq!(
            t.vm.constant("INI_SCANNER_TYPED").unwrap().as_int(),
            Some(2)
        );
    }

    #[test]
    fn argument_validation() {
        let mut t = TestVm::new();
        let r = t.call("parse_ini_file", vec![Value::from("")]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "parse_ini_file(): Argument #1 ($filename) cannot be empty".to_string()
            )
        );
        let r = t.call("parse_ini_file", vec![Value::from(&b"a\0b"[..])]);
        assert_eq!(
            thrown(r).1,
            "parse_ini_file(): Argument #1 ($filename) must not contain any null bytes"
        );
        let r = t.call(
            "parse_ini_string",
            vec![Value::from("a=1"), Value::Bool(false), Value::Int(3)],
        );
        assert_eq!(
            thrown(r),
            (
                "ValueError".to_string(),
                "parse_ini_string(): Argument #3 ($scanner_mode) must be one of INI_SCANNER_NORMAL, INI_SCANNER_RAW, or INI_SCANNER_TYPED".to_string()
            )
        );
        let r = t.call(
            "parse_ini_file",
            vec![Value::from("x.ini"), Value::Bool(false), Value::Int(-1)],
        );
        assert_eq!(
            thrown(r).1,
            "parse_ini_file(): Argument #3 ($scanner_mode) must be one of INI_SCANNER_NORMAL, INI_SCANNER_RAW, or INI_SCANNER_TYPED"
        );
    }

    // ----- general_functions/parse_ini_string_*.phpt ---------------------

    #[test]
    fn parse_ini_string_001() {
        let src = std::fs::read_to_string(
            phpt_dir().join("ext/standard/tests/general_functions/parse_ini_string_001.phpt"),
        )
        .unwrap();
        let start = src.find("<<<EOD\n").unwrap() + "<<<EOD\n".len();
        let end = src.find("\nEOD;").unwrap();
        // The heredoc escapes the test uses: \n, \t and \0.
        let raw = &src.as_bytes()[start..end];
        let mut ini = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            match (raw[i], raw.get(i + 1)) {
                (b'\\', Some(b'n')) => ini.push(b'\n'),
                (b'\\', Some(b't')) => ini.push(b'\t'),
                (b'\\', Some(b'0')) => ini.push(0),
                (c, _) => {
                    ini.push(c);
                    i += 1;
                    continue;
                }
            }
            i += 2;
        }
        let mut t = TestVm::new();
        t.vm.define_constant("BIRD", Value::from("Humming bird"));
        let flat = parse_str(&mut t, &ini, false, INI_SCANNER_NORMAL);
        let sections = parse_str(&mut t, &ini, true, INI_SCANNER_NORMAL);
        let out = format!(
            "*** Test parse_ini_string() function:  with various keys and values given in string ***\n\
             -- ini string without process_sections optional arg --\n{}\n\
             -- ini string with process_sections as TRUE --\n{}*** Done **",
            print_r(&flat),
            print_r(&sections)
        );
        assert_eq!(t.output(), "");
        assert_eq!(
            out,
            phpt_expect("ext/standard/tests/general_functions/parse_ini_string_001.phpt")
        );
    }

    #[test]
    fn parse_ini_string_002() {
        assert_eq!(
            dump("\ntest =\n"),
            "array(1) {\n  [\"test\"]=>\n  string(0) \"\"\n}\n"
        );
        let err =
            ini_warning("syntax error, unexpected '=' in Unknown on line 2") + "bool(false)\n";
        assert_eq!(dump("\ntest==\n"), err);
        assert_eq!(dump("\ntest=test=\n"), err);
        assert_eq!(
            dump("\ntest= \"new\nline\"\n"),
            "array(1) {\n  [\"test\"]=>\n  string(8) \"new\nline\"\n}\n"
        );
        let mut t = TestVm::new();
        t.vm.define_constant("TEST_CONST", Value::from("test const value"));
        let v = parse_str(&mut t, b"\ntest=TEST_CONST\n", false, 0);
        assert_eq!(
            var_dump(&v),
            "array(1) {\n  [\"test\"]=>\n  string(16) \"test const value\"\n}\n"
        );
        let section = "\n[section]\ntest=hello\n";
        assert_eq!(
            dump_mode(section, true, 0),
            "array(1) {\n  [\"section\"]=>\n  array(1) {\n    [\"test\"]=>\n    string(5) \"hello\"\n  }\n}\n"
        );
        assert_eq!(
            dump_mode(section, false, 0),
            "array(1) {\n  [\"test\"]=>\n  string(5) \"hello\"\n}\n"
        );
        assert_eq!(
            dump_mode("\nsection.test=hello\n", true, 0),
            "array(1) {\n  [\"section.test\"]=>\n  string(5) \"hello\"\n}\n"
        );
        assert_eq!(
            dump_mode("\n[section]\nsection.test=hello\n", true, 0),
            "array(1) {\n  [\"section\"]=>\n  array(1) {\n    [\"section.test\"]=>\n    string(5) \"hello\"\n  }\n}\n"
        );
        assert_eq!(
            dump_mode("\n[section]\n1=2\n", true, 0),
            "array(1) {\n  [\"section\"]=>\n  array(1) {\n    [1]=>\n    string(1) \"2\"\n  }\n}\n"
        );
        assert_eq!(
            dump_mode("\n1=2\n", true, 0),
            "array(1) {\n  [1]=>\n  string(1) \"2\"\n}\n"
        );
        assert_eq!(
            dump_mode("\ntest=test2\ntest=test3\ntest=test4\n", true, 0),
            "array(1) {\n  [\"test\"]=>\n  string(5) \"test4\"\n}\n"
        );
        // Bug #44574
        assert_eq!(
            dump_mode("[section1]\nname = value", true, 0),
            "array(1) {\n  [\"section1\"]=>\n  array(1) {\n    [\"name\"]=>\n    string(5) \"value\"\n  }\n}\n"
        );
        // Bug #44842: labels starting with underscore.
        assert_eq!(
            dump_mode("foo=bar1\n_foo=bar2\nfoo_=bar3", true, 0),
            "array(3) {\n  [\"foo\"]=>\n  string(4) \"bar1\"\n  [\"_foo\"]=>\n  string(4) \"bar2\"\n  [\"foo_\"]=>\n  string(4) \"bar3\"\n}\n"
        );
    }

    #[test]
    fn parse_ini_string_003_typed() {
        let ini = "foo = 1\nbar = 1.3\nbaz = null\nqux[] = false\nqux[] = off\nqux[] = something\nqux[] = \"something else\"";
        assert_eq!(
            dump_mode(ini, false, INI_SCANNER_TYPED),
            "array(4) {\n  [\"foo\"]=>\n  int(1)\n  [\"bar\"]=>\n  float(1.3)\n  [\"baz\"]=>\n  NULL\n  [\"qux\"]=>\n  array(4) {\n    [0]=>\n    bool(false)\n    [1]=>\n    bool(false)\n    [2]=>\n    string(9) \"something\"\n    [3]=>\n    string(14) \"something else\"\n  }\n}\n"
        );
    }

    #[test]
    fn parse_ini_string_bug76068() {
        let typed = |s: &str| dump_mode(s, true, INI_SCANNER_TYPED);
        let wrap = |inner: &str| {
            format!(
                "array(1) {{\n  [\"foo\"]=>\n  array(1) {{\n    [\"bar\"]=>\n    {inner}\n  }}\n}}\n"
            )
        };
        assert_eq!(typed("[foo]\nbar=1|>baz"), wrap("int(1)"));
        assert_eq!(typed("[foo]\nbar=\"1|>baz\""), wrap("string(6) \"1|>baz\""));
        assert_eq!(typed("[foo]\nbar=1"), wrap("int(1)"));
        assert_eq!(typed("[foo]\nbar=42|>baz"), wrap("int(42)"));
    }

    #[test]
    fn parse_ini_string_error() {
        assert_eq!(
            dump("a=\"b"),
            ini_warning(
                "syntax error, unexpected end of file, expecting TC_DOLLAR_CURLY or TC_QUOTED_STRING or '\"' in Unknown on line 1"
            ) + "bool(false)\n"
        );
        assert_eq!(
            dump("a=${b"),
            ini_warning(
                "syntax error, unexpected end of file, expecting TC_FALLBACK or '}' in Unknown on line 1"
            ) + "bool(false)\n"
        );
        assert_eq!(
            dump("a=${b:-a"),
            ini_warning(
                "syntax error, unexpected TC_FALLBACK, expecting TC_VARNAME in Unknown on line 1"
            ) + "bool(false)\n"
        );
    }

    #[test]
    fn parse_ini_numeric_entry_name() {
        assert_eq!(
            dump("\n1[]=1\n2M[]=2\n"),
            "array(2) {\n  [1]=>\n  array(1) {\n    [0]=>\n    string(1) \"1\"\n  }\n  [\"2M\"]=>\n  array(1) {\n    [0]=>\n    string(1) \"2\"\n  }\n}\n"
        );
    }

    // ----- ext/standard/tests/{gh*,strings/bug51899}.phpt -----------------

    #[test]
    fn gh11010_concatenated_numbers_stay_strings() {
        let mut t = TestVm::new();
        let v = parse_str(&mut t, b"variable = -00 20 30", false, INI_SCANNER_TYPED);
        let a = v.as_array().unwrap();
        assert_eq!(
            var_dump(a.get_str(b"variable").unwrap()),
            "string(9) \"-00 20 30\"\n"
        );
    }

    #[test]
    fn gh20695_malformed_offset() {
        assert_eq!(
            dump_mode("8 [[] = !!$]", true, INI_SCANNER_TYPED),
            "array(1) {\n  [8]=>\n  array(1) {\n    [\"[\"]=>\n    int(0)\n  }\n}\n"
        );
    }

    #[test]
    fn bug51899_empty_value_without_newline() {
        let a_is = |s: &str| format!("array(1) {{\n  [\"a\"]=>\n  {s}\n}}\n");
        assert_eq!(dump("a="), a_is("string(0) \"\""));
        assert_eq!(dump("a= "), a_is("string(0) \"\""));
        assert_eq!(dump("a=\n"), a_is("string(0) \"\""));
        assert_eq!(dump("a=b "), a_is("string(2) \"b \""));
        assert_eq!(dump(""), "array(0) {\n}\n");
        assert_eq!(dump("\0"), "array(0) {\n}\n");
    }

    // ----- general_functions/bug*.phpt ------------------------------------

    #[test]
    fn bug40752_scalar_redeclared_as_array() {
        assert_eq!(
            dump("\nfoo   = 1;\nfoo[] = 1;\n"),
            "array(1) {\n  [\"foo\"]=>\n  array(1) {\n    [0]=>\n    string(1) \"1\"\n  }\n}\n"
        );
        assert_eq!(
            dump("\nfoo[] = 1;\nfoo   = 1;\n"),
            "array(1) {\n  [\"foo\"]=>\n  string(1) \"1\"\n}\n"
        );
    }

    #[test]
    fn bug41445_numeric_section_names() {
        let f = TempFile::new(
            "bug41445.ini",
            b"[001099030277]\noption1 = yes\n\n[011099030277]\noption2 = yes",
        );
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, 0)),
            "array(2) {\n  [\"001099030277\"]=>\n  array(1) {\n    [\"option1\"]=>\n    string(1) \"1\"\n  }\n  [\"011099030277\"]=>\n  array(1) {\n    [\"option2\"]=>\n    string(1) \"1\"\n  }\n}\n"
        );
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, false, 0)),
            "array(2) {\n  [\"option1\"]=>\n  string(1) \"1\"\n  [\"option2\"]=>\n  string(1) \"1\"\n}\n"
        );
        let f = TempFile::new(
            "bug41445b.ini",
            b"[23.44]\noption1 = yes\n\n[9633337363542736472364]\noption2 = yes",
        );
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, 0)),
            "array(2) {\n  [\"23.44\"]=>\n  array(1) {\n    [\"option1\"]=>\n    string(1) \"1\"\n  }\n  [\"9633337363542736472364\"]=>\n  array(1) {\n    [\"option2\"]=>\n    string(1) \"1\"\n  }\n}\n"
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn bug41445_1_numeric_keys() {
        let ini = "[2454.33]\n09 = yes\n\n[9876543]\n098765434567876543 = yes\n\n[09876543]\n987654345678765432456798765434567876543 = yes";
        assert_eq!(
            dump_mode(ini, true, 0),
            "array(3) {\n  [\"2454.33\"]=>\n  array(1) {\n    [\"09\"]=>\n    string(1) \"1\"\n  }\n  [9876543]=>\n  array(1) {\n    [\"098765434567876543\"]=>\n    string(1) \"1\"\n  }\n  [\"09876543\"]=>\n  array(1) {\n    [\"987654345678765432456798765434567876543\"]=>\n    string(1) \"1\"\n  }\n}\n"
        );
        assert_eq!(
            dump(ini),
            "array(3) {\n  [\"09\"]=>\n  string(1) \"1\"\n  [\"098765434567876543\"]=>\n  string(1) \"1\"\n  [\"987654345678765432456798765434567876543\"]=>\n  string(1) \"1\"\n}\n"
        );
    }

    #[test]
    fn bug44461_trailing_comment_at_eof() {
        let f = TempFile::new(
            "bug44461.ini",
            b"[attachments]\nzip = \"application/zip\" ; MIME-type for ZIP files",
        );
        let mut t = TestVm::new();
        let v = parse_file(&mut t, &f.0, true, 0);
        assert_eq!(t.output(), "");
        assert_eq!(
            var_dump(&v),
            "array(1) {\n  [\"attachments\"]=>\n  array(1) {\n    [\"zip\"]=>\n    string(15) \"application/zip\"\n  }\n}\n"
        );
    }

    #[test]
    fn bug47859_special_chars_in_keys() {
        let cases = [
            ("*key = \"*value\"", "*key", "*value"),
            ("-key = \"-value\"", "-key", "-value"),
            ("_key = \"_value\"", "_key", "_value"),
            ("key* = \"value*\"", "key*", "value*"),
            ("key.*.* = \"value.*.*\"", "key.*.*", "value.*.*"),
            ("*.*.key = \"*.*.value\"", "*.*.key", "*.*.value"),
            ("k*e*y = \"v*a*lue\"", "k*e*y", "v*a*lue"),
        ];
        for (ini, k, v) in cases {
            assert_eq!(
                dump(ini),
                format!(
                    "array(1) {{\n  [\"{k}\"]=>\n  string({}) \"{v}\"\n}}\n",
                    v.len()
                )
            );
        }
    }

    #[test]
    fn bug48660_dollar_sign_at_end_of_value() {
        let ini = "\n[cases]\n\nCase.a = avalue\nCase.b = \"$dollar_sign\"\nCase.c = \"dollar_sign$\"\nCase.d = \"$dollar_sign$\"\nCase.e = 10\n";
        let expected = "array(1) {\n  [\"cases\"]=>\n  array(5) {\n    [\"Case.a\"]=>\n    string(6) \"avalue\"\n    [\"Case.b\"]=>\n    string(12) \"$dollar_sign\"\n    [\"Case.c\"]=>\n    string(12) \"dollar_sign$\"\n    [\"Case.d\"]=>\n    string(13) \"$dollar_sign$\"\n    [\"Case.e\"]=>\n    string(2) \"10\"\n  }\n}\n";
        let f = TempFile::new("bug48660.ini", ini.as_bytes());
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, INI_SCANNER_RAW)),
            expected
        );
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, INI_SCANNER_NORMAL)),
            expected
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn bug48768_quoted_equals_sign() {
        let expected = "array(1) {\n  [\"equal\"]=>\n  string(1) \"=\"\n}\n";
        assert_eq!(
            dump_mode("equal = \"=\"\n", false, INI_SCANNER_RAW),
            expected
        );
        assert_eq!(
            dump_mode("equal = \"=\"\n", false, INI_SCANNER_NORMAL),
            expected
        );
    }

    #[test]
    fn bug49056_non_ascii_keys() {
        let ini =
            "Cooking_furniture=\"KÃ¼chen MÃ¶bel (en)\"\nKÃ¼chen_MÃ¶bel=\"Cooking furniture (en)\"";
        let f = TempFile::new("bug49056.ini", ini.as_bytes());
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, false, 0)),
            "array(2) {\n  [\"Cooking_furniture\"]=>\n  string(23) \"KÃ¼chen MÃ¶bel (en)\"\n  [\"KÃ¼chen_MÃ¶bel\"]=>\n  string(22) \"Cooking furniture (en)\"\n}\n"
        );
    }

    #[test]
    fn bug49692_slash_in_keys() {
        let path = phpt_dir().join("ext/standard/tests/general_functions/bug49692.ini");
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &path, true, 0)),
            "array(1) {\n  [\"sitemap\"]=>\n  array(2) {\n    [\"/home\"]=>\n    string(13) \"default:index\"\n    [\"/info\"]=>\n    string(12) \"default:info\"\n  }\n}\n"
        );
    }

    #[test]
    fn bug52138_constants_not_expanded_in_sections() {
        let path = phpt_dir().join("ext/standard/tests/general_functions/bug52138.data");
        let mut t = TestVm::new();
        t.vm.define_constant("MYCONST", Value::Int(1));
        t.vm.define_constant("A", Value::from("B"));
        t.vm.define_constant("M_PI", Value::Float(std::f64::consts::PI));
        assert_eq!(
            var_dump(&parse_file(&mut t, &path, true, 0)),
            "array(4) {\n  [\"MYCONST\"]=>\n  array(1) {\n    [\"MYCONST\"]=>\n    string(1) \"1\"\n  }\n  [\"M_PI\"]=>\n  array(1) {\n    [\"FOO\"]=>\n    string(20) \"3.1415926535898 test\"\n  }\n  [\"foo::bar\"]=>\n  array(2) {\n    [\"A\"]=>\n    string(1) \"1\"\n    [\"B\"]=>\n    string(3) \"BAB\"\n  }\n  [\"MYCONST M_PI\"]=>\n  array(0) {\n  }\n}\n"
        );
    }

    #[test]
    fn bug70157_typed_numbers_and_strings() {
        let ini = "[agatha.christie]\ntitle = 10 little indians\nfoo[123] = E_ALL & ~E_DEPRECATED\nfoo[456] = 123";
        assert_eq!(
            dump_mode(ini, false, INI_SCANNER_TYPED),
            "array(2) {\n  [\"title\"]=>\n  string(17) \"10 little indians\"\n  [\"foo\"]=>\n  array(2) {\n    [123]=>\n    int(22527)\n    [456]=>\n    int(123)\n  }\n}\n"
        );
    }

    #[test]
    fn bug70947_typed_string_with_number() {
        assert_eq!(
            dump_mode("foo = bar 123", false, INI_SCANNER_TYPED),
            "array(1) {\n  [\"foo\"]=>\n  string(7) \"bar 123\"\n}\n"
        );
    }

    #[test]
    fn bug77844_typed_operators_on_floats() {
        assert_eq!(
            dump_mode("val1=3.7&2\nval2=2&3.7", true, INI_SCANNER_TYPED),
            "array(2) {\n  [\"val1\"]=>\n  int(2)\n  [\"val2\"]=>\n  int(2)\n}\n"
        );
    }

    // ----- general_functions/parse_ini_{basic,booleans,file}.phpt --------

    #[test]
    fn parse_ini_basic() {
        let mut t = TestVm::new();
        // The test's --ENV-- / --INI-- values as configuration directives
        // (run-tests drops empty environment variables: `emptyval` is unset).
        for (name, value) in [
            ("basicval", "FUBAR_VARIABLE"),
            ("basicqval", "FUBAR_QUOTES_VARIABLE"),
            ("falsyval", "false"),
            ("INI:WITH:COLON", "ini_with_colon"),
        ] {
            t.vm.ini
                .register(&IniDef::new(name, Some(value), modifiable::ALL), "test");
        }
        t.vm.define_constant("TEST_CONSTANT", Value::from("this_is_test_constant"));
        let path = phpt_dir().join("ext/standard/tests/general_functions/parse_ini_basic.data");
        let v = parse_file(&mut t, &path, true, 0);
        assert_eq!(t.output(), "");
        assert_eq!(
            var_dump(&v) + "Done.",
            phpt_expect("ext/standard/tests/general_functions/parse_ini_basic.phpt")
        );
    }

    #[test]
    fn parse_ini_booleans() {
        let mut t = TestVm::new();
        let path = phpt_dir().join("ext/standard/tests/general_functions/parse_ini_booleans.data");
        let v = parse_file(&mut t, &path, true, 0);
        assert_eq!(
            t.output() + &var_dump(&v) + "Done.",
            phpt_expect("ext/standard/tests/general_functions/parse_ini_booleans.phpt")
        );
    }

    #[test]
    fn parse_ini_file_multiple_calls() {
        let mut t = TestVm::new();
        let missing =
            std::env::temp_dir().join(format!("php_ini_{}_does_not_exist.dat", std::process::id()));
        let v = parse_file(&mut t, &missing, false, 0);
        assert!(matches!(v, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: parse_ini_file(): Failed to open stream: No such file or directory in /t.php on line 3\n"
        );

        let f = TempFile::new("parse_ini_file1.dat", b"\ntest =\n");
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, false, 0)),
            "array(1) {\n  [\"test\"]=>\n  string(0) \"\"\n}\n"
        );
        let f = TempFile::new("parse_ini_file2.dat", b"\ntest==\n");
        let v = parse_file(&mut t, &f.0, false, 0);
        assert!(matches!(v, Value::Bool(false)));
        assert_eq!(
            t.output(),
            ini_warning(&format!(
                "syntax error, unexpected '=' in {} on line 2",
                f.0.display()
            ))
        );
        // #44575: `;` comments.
        let f = TempFile::new(
            "parse_ini_file3.dat",
            b"foo=bar1\n; comment\n_foo=bar2\n; comment\nfoo_=bar3",
        );
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, 0)),
            "array(3) {\n  [\"foo\"]=>\n  string(4) \"bar1\"\n  [\"_foo\"]=>\n  string(4) \"bar2\"\n  [\"foo_\"]=>\n  string(4) \"bar3\"\n}\n"
        );
    }

    // ----- Zend/tests ------------------------------------------------------

    #[test]
    fn zend_bug52508_raw_without_newline() {
        let f = TempFile::new("bug52508.ini", b"a = 1");
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &f.0, true, INI_SCANNER_RAW)),
            "array(1) {\n  [\"a\"]=>\n  string(1) \"1\"\n}\n"
        );
    }

    #[test]
    fn zend_bug69551_raw_unterminated_quote() {
        assert_eq!(
            dump_mode(
                "[Network.eth0]\nSubnetMask = \"\n\"",
                false,
                INI_SCANNER_RAW
            ),
            ini_warning("syntax error, unexpected '\"' in Unknown on line 3") + "bool(false)\n"
        );
    }

    #[test]
    fn zend_bug70748_unterminated_variable_in_section() {
        let f = TempFile::new("bug70748.ini", b"[${ \t");
        let mut t = TestVm::new();
        let v = parse_file(&mut t, &f.0, false, 0);
        assert!(matches!(v, Value::Bool(false)));
        assert_eq!(
            t.output(),
            ini_warning(&format!(
                "syntax error, unexpected end of file, expecting TC_FALLBACK or '}}' in {} on line 1",
                f.0.display()
            ))
        );
    }

    #[test]
    fn zend_bug74603_large_operand() {
        let path = phpt_dir().join("Zend/tests/bug74603.ini");
        let mut t = TestVm::new();
        assert_eq!(
            var_dump(&parse_file(&mut t, &path, true, INI_SCANNER_NORMAL)),
            "array(1) {\n  [0]=>\n  string(1) \"0\"\n}\n"
        );
    }

    #[test]
    fn zend_bug76965_raw_trailing_whitespace() {
        let ini = "1=\"foo\"\n2=\"bar\" ; comment\n3= baz\n4= \"foo;bar\"\n5= \"foo\" ; bar ; baz\n6= \"foo;bar\" ; baz\n7= foo\"bar ; \"ok";
        assert_eq!(
            dump_mode(ini, false, INI_SCANNER_RAW),
            "array(7) {\n  [1]=>\n  string(3) \"foo\"\n  [2]=>\n  string(3) \"bar\"\n  [3]=>\n  string(3) \"baz\"\n  [4]=>\n  string(7) \"foo;bar\"\n  [5]=>\n  string(3) \"foo\"\n  [6]=>\n  string(7) \"foo;bar\"\n  [7]=>\n  string(7) \"foo\"bar\"\n}\n"
        );
    }

    #[test]
    fn zend_bug77589_typed_numeric_sections() {
        let ini = "[0]\na = 1\nb = on\nc = true\n\n[\"true\"]\na = 100\nb = null\nc = yes";
        assert_eq!(
            dump_mode(ini, true, INI_SCANNER_TYPED),
            "array(2) {\n  [0]=>\n  array(3) {\n    [\"a\"]=>\n    int(1)\n    [\"b\"]=>\n    bool(true)\n    [\"c\"]=>\n    bool(true)\n  }\n  [\"true\"]=>\n  array(3) {\n    [\"a\"]=>\n    int(100)\n    [\"b\"]=>\n    NULL\n    [\"c\"]=>\n    bool(true)\n  }\n}\n"
        );
    }

    #[test]
    fn zend_oss_fuzz_428983568() {
        assert_eq!(
            dump("[${zz:-x\n"),
            ini_warning("syntax error, unexpected end of file, expecting '}' in Unknown on line 1")
                + "bool(false)\n"
        );
    }

    // ----- behaviour derived from zend_ini_scanner.l / zend_ini_parser.y --

    fn one(v: &str) -> String {
        format!("array(1) {{\n  [\"a\"]=>\n  {v}\n}}\n")
    }

    #[test]
    fn expressions_use_c_int_arithmetic() {
        assert_eq!(dump("a = 1 | 2"), one("string(1) \"3\""));
        assert_eq!(dump("a = ~0"), one("string(2) \"-1\""));
        assert_eq!(dump("a = !0"), one("string(1) \"1\""));
        assert_eq!(dump("a = (1|6)&2"), one("string(1) \"2\""));
        assert_eq!(dump("a = 5 ^ 1"), one("string(1) \"4\""));
        // atoi() truncated to a C int.
        assert_eq!(dump("a = 4294967297 | 0"), one("string(1) \"1\""));
        assert_eq!(
            dump_mode("a = ~1", false, INI_SCANNER_TYPED),
            one("int(-2)")
        );
        assert_eq!(
            dump_mode("a = (5)", false, INI_SCANNER_TYPED),
            one("int(5)")
        );
    }

    #[test]
    fn typed_numbers_follow_the_leading_token() {
        // Z_EXTRA(INI_ZVAL_IS_NUMBER) is kept by concatenation onto a number.
        assert_eq!(
            dump_mode("a = 1\"2\"", false, INI_SCANNER_TYPED),
            one("int(12)")
        );
        assert_eq!(
            dump_mode("a = \"1\"2", false, INI_SCANNER_TYPED),
            one("string(2) \"12\"")
        );
        assert_eq!(
            dump_mode("a = \"1\"", false, INI_SCANNER_TYPED),
            one("string(1) \"1\"")
        );
        // `-1.5` scans as TC_STRING (`-` only prefixes LNUM).
        assert_eq!(
            dump_mode("a = -1.5", false, INI_SCANNER_TYPED),
            one("string(4) \"-1.5\"")
        );
        assert_eq!(
            dump_mode("a = -15", false, INI_SCANNER_TYPED),
            one("int(-15)")
        );
        assert_eq!(
            dump_mode("a = .5", false, INI_SCANNER_TYPED),
            one("float(0.5)")
        );
        // Integer overflow is not converted.
        assert_eq!(
            dump_mode("a = 99999999999999999999", false, INI_SCANNER_TYPED),
            one("string(20) \"99999999999999999999\"")
        );
        assert_eq!(
            dump_mode("a = yes", false, INI_SCANNER_RAW),
            one("string(3) \"yes\"")
        );
    }

    #[test]
    fn reserved_words_as_keys() {
        assert_eq!(
            dump("true = 1"),
            ini_warning("syntax error, unexpected BOOL_TRUE in Unknown on line 1")
                + "bool(false)\n"
        );
        assert_eq!(
            dump("\n\nnull = 1"),
            ini_warning("syntax error, unexpected NULL_NULL in Unknown on line 3")
                + "bool(false)\n"
        );
        // A bare label is accepted and ignored.
        assert_eq!(
            dump("lonely\nb = 1"),
            "array(1) {\n  [\"b\"]=>\n  string(1) \"1\"\n}\n"
        );
    }

    #[test]
    fn line_numbers_count_quoted_newlines_and_sections() {
        assert_eq!(
            dump("[a]\nb = \"x\ny\"\nc = 1 = 2"),
            ini_warning("syntax error, unexpected '=' in Unknown on line 4") + "bool(false)\n"
        );
    }

    #[test]
    fn quoted_string_escapes_and_concatenation() {
        assert_eq!(
            dump(r#"a = "x\"y\\z\$w\n" 'r\aw' tail"#),
            one(r#"string(18) "x"y\z$w\nr\aw tail""#)
        );
        // Whitespace before an opening quote and after a closing one is dropped.
        assert_eq!(dump("a = x \"y\" z"), one("string(3) \"xyz\""));
    }

    #[test]
    fn variables_come_from_ini_then_environment() {
        let mut t = TestVm::new();
        t.vm.ini.register(
            &IniDef::new("ini.test.var", Some("startup"), modifiable::ALL),
            "test",
        );
        t.vm.ini_set("ini.test.var", "runtime");
        let v = parse_str(&mut t, b"a = ${ini.test.var}", false, 0);
        // zend_get_configuration_directive() sees the startup value.
        assert_eq!(var_dump(&v), one("string(7) \"startup\""));
        if let Some(path) = std::env::var_os("PATH").and_then(|p| p.into_string().ok()) {
            let v = parse_str(&mut t, b"a = \"${PATH}\"", false, 0);
            assert_eq!(
                var_dump(&v),
                one(&format!("string({}) \"{path}\"", path.len()))
            );
        }
        let v = parse_str(
            &mut t,
            b"a = ${php_ini_test_undefined:-${php_ini_test_undefined2:-x} y}",
            false,
            0,
        );
        assert_eq!(var_dump(&v), one("string(3) \"x y\""));
    }

    #[test]
    fn constants_in_values_and_offsets() {
        let mut t = TestVm::new();
        t.vm.define_constant("ARR", Value::empty_array());
        let v = parse_str(&mut t, b"a[E_WARNING] = ARR\nb[true] = x", false, 0);
        assert_eq!(
            t.output(),
            "\nWarning: Array to string conversion in /t.php on line 3\n"
        );
        assert_eq!(
            var_dump(&v),
            "array(2) {\n  [\"a\"]=>\n  array(1) {\n    [2]=>\n    string(5) \"Array\"\n  }\n  [\"b\"]=>\n  array(1) {\n    [1]=>\n    string(1) \"x\"\n  }\n}\n"
        );
    }

    #[test]
    fn duplicate_sections_replace_earlier_ones() {
        assert_eq!(
            dump_mode("top = 1\n[s]\nx = 1\n[t]\n[s]\ny = 2", true, 0),
            "array(3) {\n  [\"top\"]=>\n  string(1) \"1\"\n  [\"s\"]=>\n  array(1) {\n    [\"y\"]=>\n    string(1) \"2\"\n  }\n  [\"t\"]=>\n  array(0) {\n  }\n}\n"
        );
    }

    #[test]
    fn raw_sections_are_literal() {
        assert_eq!(
            dump_mode("[${x} y]\na = ${x}", true, INI_SCANNER_RAW),
            "array(1) {\n  [\"${x} y\"]=>\n  array(1) {\n    [\"a\"]=>\n    string(4) \"${x}\"\n  }\n}\n"
        );
    }

    #[test]
    fn utf8_bom_and_crlf() {
        assert_eq!(
            dump("\u{feff}a = 1\r\nb = 2\r\n"),
            "array(2) {\n  [\"a\"]=>\n  string(1) \"1\"\n  [\"b\"]=>\n  string(1) \"2\"\n}\n"
        );
    }

    #[test]
    fn parser_stack_exhaustion() {
        let ini = format!("a = {}1", "!".repeat(10_000));
        assert_eq!(
            dump(&ini),
            ini_warning("memory exhausted in Unknown on line 1") + "bool(false)\n"
        );
    }

    #[test]
    fn parse_ini_with_reports_events() {
        let mut t = TestVm::new();
        let mut events = Vec::new();
        let ok = parse_ini_with(
            &mut t.vm,
            b"[sec]\nk = v\nbare\nk[o] = w\n",
            Some(b"x.ini"),
            INI_SCANNER_NORMAL,
            &mut |_, ev| {
                events.push(format!("{ev:?}"));
                Ok(())
            },
        )
        .unwrap();
        assert!(ok);
        assert_eq!(events.len(), 4);
        assert!(events[0].starts_with("Section"));
        assert!(events[2].contains("value: None"));
        assert!(events[3].starts_with("PopEntry"));
    }
}
