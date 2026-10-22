//! PHP's shared CSV code: `php_fgetcsv`, `php_fputcsv`,
//! `php_csv_handle_escape_argument`, `php_bc_fgetcsv_empty_line` and
//! `php_fgetcsv_lookup_trailing_spaces` (ext/standard/file.c),
//! `PHP_FUNCTION(str_getcsv)` (ext/standard/string.c) and the argument
//! handling of `PHP_FUNCTION(fgetcsv)` / `PHP_FUNCTION(fputcsv)`.
//!
//! The stream layer wires `fgetcsv()` / `fputcsv()` onto stream resources
//! through [`CsvStream`], [`fgetcsv_impl`] and [`fputcsv_impl`].
//!
//! Platform-dependent details of the C code, resolved as follows:
//!
//! * **Multibyte handling.** The parser walks the line with `php_mblen()`
//!   (`mblen(3)` in a non-ZTS build). PHP sets `LC_CTYPE` to `C.UTF-8` at
//!   startup (`zend_reset_lc_ctype_locale()`), so [`mblen`] decodes strict
//!   UTF-8 (no overlong forms, surrogates or code points above U+10FFFF).
//!   A valid multibyte character is skipped as a unit, so its trail bytes
//!   never act as delimiter, enclosure or escape. Invalid or truncated
//!   sequences make `mblen()` return -1, which every call site of the C code
//!   treats as a one-byte character. `php_fgetcsv_lookup_trailing_spaces`
//!   also does not record such bytes as "last characters", so
//!   `"a\n\xFF"` loses its last byte as if it were the line ending.
//! * **Signedness of `char`.** `php_fgetcsv` and `php_fputcsv` compare a
//!   `char` from the line with the `int` escape character
//!   (`(unsigned char) $escape[0]`). With a signed `char`, as on x86-64 (the
//!   platform php-src's tests run on), an escape byte of 0x80 or above
//!   never matches, so such an escape character has no effect. It still
//!   makes `fputcsv()` enclose a field that contains it, because that check
//!   uses `memchr()`. Delimiter and enclosure comparisons are `char` against
//!   `char` and match on every platform.
//!
//! Not modelled: `ZEND_HAS_OOM()` (the parser stops early when the memory
//! limit is reached) and the allocation of a `$length + 1` byte buffer, which
//! makes huge `fgetcsv()` lengths a memory-limit fatal error in PHP.

use std::rc::Rc;

use php_runtime::{Args, Flow, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `PHP_CSV_NO_ESCAPE` (file.h): `EOF`, i.e. no escape character.
pub const CSV_NO_ESCAPE: i32 = -1;

/// Stream access needed by fgetcsv/fputcsv (implemented by the stream layer).
pub trait CsvStream {
    /// `php_stream_get_line(stream, NULL, 0, &len)` when `maxlen` is `None`
    /// (grow mode), else `php_stream_get_line(stream, buf, maxlen, &len)`
    /// where `maxlen` is the C buffer size *including* the NUL, so at most
    /// `maxlen - 1` bytes come back. `fgetcsv()` passes `$length + 1`.
    /// `None` at EOF.
    fn get_line(&mut self, vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>>;

    /// `php_stream_write`: bytes written, or `None` on failure.
    fn write(&mut self, vm: &mut Vm, data: &[u8]) -> PResult<Option<usize>>;
}

// ---------------------------------------------------------------------------
// Character classification
// ---------------------------------------------------------------------------

/// `mblen(s, s.len())` in the `C.UTF-8` locale, for `s[0] != 0` (every
/// caller in file.c handles NUL itself): the length of the complete UTF-8
/// character at the start of `s`, or -1 if it is invalid or truncated.
fn mblen(s: &[u8]) -> i32 {
    let (len, second) = match s[0] {
        0x00..=0x7f => return 1,
        0xc2..=0xdf => (2, 0x80..=0xbf),
        0xe0 => (3, 0xa0..=0xbf),
        0xe1..=0xec | 0xee..=0xef => (3, 0x80..=0xbf),
        0xed => (3, 0x80..=0x9f),
        0xf0 => (4, 0x90..=0xbf),
        0xf1..=0xf3 => (4, 0x80..=0xbf),
        0xf4 => (4, 0x80..=0x8f),
        _ => return -1,
    };
    if s.len() < len
        || !second.contains(&s[1])
        || s[2..len].iter().any(|b| !(0x80..=0xbf).contains(b))
    {
        return -1;
    }
    len as i32
}

/// `isspace((unsigned char) c)` in the C / `C.UTF-8` locale.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `*p == escape_char` with `*p` a (signed, see the module docs) `char`.
fn is_escape(c: u8, escape: i32) -> bool {
    escape != CSV_NO_ESCAPE && i32::from(c as i8) == escape
}

/// `php_fgetcsv_lookup_trailing_spaces`: the length of `s` without a
/// trailing `"\r\n"`, `"\n"` or `"\r"` (despite its name it strips nothing
/// else). Invalid multibyte bytes do not count as last characters.
fn lookup_trailing_spaces(s: &[u8]) -> usize {
    let mut last_chars = [0u8; 2];
    let mut p = 0;
    while p < s.len() {
        let inc_len = if s[p] == 0 { 1 } else { mblen(&s[p..]) };
        if inc_len < 0 {
            p += 1;
            continue;
        }
        last_chars[0] = last_chars[1];
        last_chars[1] = s[p];
        p += inc_len as usize;
    }
    match last_chars {
        [b'\r', b'\n'] => p - 2,
        [_, b'\n' | b'\r'] => p - 1,
        _ => p,
    }
}

// ---------------------------------------------------------------------------
// Parsing (php_fgetcsv)
// ---------------------------------------------------------------------------

/// The line being parsed. `buf` keeps the NUL terminator the C buffers have,
/// because the state machine reads (and in corner cases copies) the byte
/// just past the stripped line end.
struct Line {
    buf: Vec<u8>,
    len: usize,
    /// `limit` (equal to `line_end` throughout `php_fgetcsv`): where the
    /// trailing line ending starts.
    limit: usize,
}

impl Line {
    fn new(mut buf: Vec<u8>) -> Line {
        let len = buf.len();
        let limit = lookup_trailing_spaces(&buf);
        buf.push(0);
        Line { buf, len, limit }
    }

    /// The stripped line ending (`line_end` .. `line_end + line_end_len`).
    fn line_end(&self) -> &[u8] {
        &self.buf[self.limit..self.len]
    }

    /// `inc_len = (bptr < limit ? (*bptr == '\0' ? 1 : php_mblen(bptr, limit - bptr)) : 0)`.
    fn inc_len(&self, p: usize) -> i32 {
        if p >= self.limit {
            0
        } else if self.buf[p] == 0 {
            1
        } else {
            mblen(&self.buf[p..self.limit])
        }
    }

    /// The "look up for a delimiter" loops (`quit_loop_3` / `quit_loop_4`):
    /// advances `bptr` to the next delimiter or the end of the line and
    /// returns the final `inc_len` (0 at the end, 1 on a delimiter).
    fn find_delimiter(&self, bptr: &mut usize, mut inc_len: i32, delimiter: u8) -> i32 {
        loop {
            match inc_len {
                0 => return 0,
                // -1 (invalid byte) is handled as a one-byte character.
                ..=-1 | 1 => {
                    inc_len = 1;
                    if self.buf[*bptr] == delimiter {
                        return 1;
                    }
                }
                _ => {}
            }
            *bptr += inc_len as usize;
            inc_len = self.inc_len(*bptr);
        }
    }
}

/// `php_fgetcsv`: splits `buf` (one line, including its line ending) into
/// fields. An enclosed field that does not end on this line continues on the
/// next lines read from `stream` (in grow mode); without a stream
/// (`str_getcsv()`) it ends with the input. `None` for a blank line.
pub fn parse(
    vm: &mut Vm,
    mut stream: Option<&mut dyn CsvStream>,
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    buf: Vec<u8>,
) -> PResult<Option<PhpArray>> {
    debug_assert!((0..=255).contains(&escape) || escape == CSV_NO_ESCAPE);

    let mut line = Line::new(buf);
    let mut bptr = 0;
    let mut temp: Vec<u8> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut first_field = true;

    loop {
        temp.clear();

        let mut inc_len = line.inc_len(bptr);
        if inc_len == 1 {
            // Whitespace before an enclosure is skipped. The scan may run past
            // `limit` into the line ending; the NUL terminator stops it.
            let mut tmp = bptr;
            while line.buf[tmp] != delimiter && is_space(line.buf[tmp]) {
                tmp += 1;
            }
            if line.buf[tmp] == enclosure && tmp < line.limit {
                bptr = tmp;
            }
        }

        if first_field && bptr == line.limit {
            return Ok(None);
        }
        first_field = false;

        let comp_end;
        if inc_len != 0 && line.buf[bptr] == enclosure {
            // 2A. Enclosed field. `state`: 0 = plain, 1 = after the escape
            // character, 2 = after an enclosure (closing or doubled?).
            let mut state = 0;
            bptr += 1;
            let mut hunk_begin = bptr;

            // The first iteration still uses `inc_len` of the opening
            // enclosure (as the C does), so it can step past `limit`.
            loop {
                match inc_len {
                    0 => {
                        if state == 2 {
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr - 1]);
                            hunk_begin = bptr;
                            break;
                        }
                        if state == 1 {
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
                            hunk_begin = bptr;
                        }
                        if hunk_begin != line.limit {
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
                            hunk_begin = bptr;
                        }
                        // The field continues: keep the line ending.
                        temp.extend_from_slice(line.line_end());

                        let next = match stream.as_deref_mut() {
                            Some(s) => s.get_line(vm, None)?,
                            None => None,
                        };
                        match next {
                            Some(new_buf) => {
                                line = Line::new(new_buf);
                                bptr = 0;
                                hunk_begin = 0;
                                state = 0;
                            }
                            None => {
                                // Unterminated enclosure: the field takes the
                                // rest of the data. If the line ended with the
                                // opening enclosure, step back so the NUL is
                                // not copied.
                                if bptr > line.limit {
                                    if hunk_begin == bptr {
                                        hunk_begin -= 1;
                                    }
                                    bptr -= 1;
                                }
                                break;
                            }
                        }
                    }
                    // -1 (invalid byte) is handled as a one-byte character.
                    ..=-1 | 1 => match state {
                        1 => {
                            bptr += 1;
                            state = 0;
                        }
                        2 => {
                            if line.buf[bptr] != enclosure {
                                // The previous enclosure closed the field.
                                temp.extend_from_slice(&line.buf[hunk_begin..bptr - 1]);
                                hunk_begin = bptr;
                                break;
                            }
                            // A doubled enclosure stands for one.
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
                            bptr += 1;
                            hunk_begin = bptr;
                            state = 0;
                        }
                        _ => {
                            let c = line.buf[bptr];
                            if c == enclosure {
                                state = 2;
                            } else if is_escape(c, escape) {
                                state = 1;
                            }
                            bptr += 1;
                        }
                    },
                    n => match state {
                        2 => {
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr - 1]);
                            hunk_begin = bptr;
                            break;
                        }
                        1 => {
                            bptr += n as usize;
                            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
                            hunk_begin = bptr;
                            state = 0;
                        }
                        _ => bptr += n as usize,
                    },
                }
                inc_len = line.inc_len(bptr);
            }

            // quit_loop_2: text between the closing enclosure and the next
            // delimiter is appended to the field.
            inc_len = line.find_delimiter(&mut bptr, inc_len, delimiter);
            // quit_loop_3
            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
            bptr += inc_len as usize;
            comp_end = temp.len();
        } else {
            // 2B. Unenclosed field.
            let hunk_begin = bptr;
            inc_len = line.find_delimiter(&mut bptr, inc_len, delimiter);
            // quit_loop_4
            temp.extend_from_slice(&line.buf[hunk_begin..bptr]);
            comp_end = lookup_trailing_spaces(&temp);
            if line.buf[bptr] == delimiter {
                bptr += 1;
            }
        }

        values.push(Value::from(&temp[..comp_end]));
        if inc_len <= 0 {
            break;
        }
    }

    Ok(Some(PhpArray::from_values(values)))
}

/// `php_bc_fgetcsv_empty_line`: what fgetcsv()/str_getcsv() return for a
/// blank line, `[null]`.
pub fn bc_empty_line() -> PhpArray {
    PhpArray::from_values(vec![Value::Null])
}

// ---------------------------------------------------------------------------
// Formatting (php_fputcsv)
// ---------------------------------------------------------------------------

/// `zval_get_tmp_string` on a field. A thrown exception does not stop the
/// line: the field becomes `""`, the exception is kept in `pending`, and
/// later objects are not converted at all (with an exception pending
/// `zend_call_function` does not run `__toString`).
fn field_string(vm: &mut Vm, v: &Value, pending: &mut Option<Flow>) -> PResult<PhpStr> {
    if pending.is_some() && matches!(v.unref(), Value::Object(_)) {
        return Ok(PhpStr::new());
    }
    match vm.to_php_string(v) {
        Ok(s) => Ok(s),
        Err(Flow::Exception(e)) => {
            if pending.is_none() {
                *pending = Some(Flow::Exception(e));
            }
            Ok(PhpStr::new())
        }
        Err(other) => Err(other),
    }
}

/// The line building of `php_fputcsv`: the CSV line and the exception a
/// field conversion threw, if any (the C builds the whole line regardless).
fn format_line(
    vm: &mut Vm,
    fields: &PhpArray,
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    eol: Option<&[u8]>,
) -> PResult<(Vec<u8>, Option<Flow>)> {
    debug_assert!((0..=255).contains(&escape) || escape == CSV_NO_ESCAPE);

    let mut line = Vec::new();
    let mut pending = None;
    let count = fields.len();
    for (i, v) in fields.values().enumerate() {
        let field = field_string(vm, v, &mut pending)?;
        let field = field.as_bytes();

        // FPUTCSV_FLD_CHK: enclose a field that contains a delimiter, an
        // enclosure or escape character, or whitespace.
        let needs_enclosure = field.iter().any(|&c| {
            c == delimiter
                || c == enclosure
                || (escape != CSV_NO_ESCAPE && i32::from(c) == escape)
                || matches!(c, b'\n' | b'\r' | b'\t' | b' ')
        });
        if needs_enclosure {
            line.push(enclosure);
            let mut escaped = false;
            for &c in field {
                if is_escape(c, escape) {
                    escaped = true;
                } else if !escaped && c == enclosure {
                    line.push(enclosure);
                } else {
                    escaped = false;
                }
                line.push(c);
            }
            line.push(enclosure);
        } else {
            line.extend_from_slice(field);
        }

        if i + 1 != count {
            line.push(delimiter);
        }
    }
    line.extend_from_slice(eol.unwrap_or(b"\n"));
    Ok((line, pending))
}

/// `php_fputcsv`'s line building: `fields` as one CSV line ending in `eol`
/// (`"\n"` when `None`). An exception thrown by a field conversion
/// (`__toString`) is returned as `Err`; [`put_line`] keeps the C behaviour of
/// writing the line anyway. Never `Ok(None)`.
pub fn format(
    vm: &mut Vm,
    fields: &PhpArray,
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    eol: Option<&[u8]>,
) -> PResult<Option<Vec<u8>>> {
    match format_line(vm, fields, delimiter, enclosure, escape, eol)? {
        (_, Some(exception)) => Err(exception),
        (line, None) => Ok(Some(line)),
    }
}

/// `php_fputcsv`: formats `fields` and writes the line to `stream`. As in the
/// C, the line is written even if a field conversion threw; the exception is
/// returned afterwards. `Ok(None)` if the write failed.
pub fn put_line(
    vm: &mut Vm,
    stream: &mut dyn CsvStream,
    fields: &PhpArray,
    delimiter: u8,
    enclosure: u8,
    escape: i32,
    eol: Option<&[u8]>,
) -> PResult<Option<usize>> {
    let (line, pending) = format_line(vm, fields, delimiter, enclosure, escape, eol)?;
    let written = stream.write(vm, &line);
    match pending {
        Some(exception) => Err(exception),
        None => written,
    }
}

// ---------------------------------------------------------------------------
// Argument handling
// ---------------------------------------------------------------------------

/// `php_csv_handle_escape_argument`: the escape character for `$escape`
/// (`None` when the argument was not passed), or [`CSV_NO_ESCAPE`] for `""`.
/// An omitted `$escape` is deprecated. The ValueError for a longer string
/// (and an exception thrown by an error handler for the deprecation) is
/// returned as `Err`, so this never returns `Ok(None)`.
pub fn handle_escape_argument(
    vm: &mut Vm,
    escape: Option<&[u8]>,
    arg_num: usize,
) -> PResult<Option<i32>> {
    match escape {
        Some([]) => Ok(Some(CSV_NO_ESCAPE)),
        Some(&[c]) => Ok(Some(i32::from(c))),
        Some(_) => Err(vm.argument_value_error(arg_num, "must be empty or a single character")),
        None => {
            vm.deprecated(
                "the $escape parameter must be provided as its default value will change",
            )?;
            Ok(Some(i32::from(b'\\')))
        }
    }
}

/// A `$separator` / `$enclosure` argument at index `i` (argument number
/// `i + 1`): its single character, `default` when not passed.
fn single_char_arg(vm: &mut Vm, args: &Args, i: usize, default: u8) -> PResult<u8> {
    if !args.is_passed(i) {
        return Ok(default);
    }
    match args.str(i).as_bytes() {
        &[c] => Ok(c),
        _ => Err(vm.argument_value_error(i + 1, "must be a single character")),
    }
}

/// The `$escape` argument at index `i` (argument number `i + 1`).
fn escape_arg(vm: &mut Vm, args: &Args, i: usize) -> PResult<i32> {
    let escape = args.is_passed(i).then(|| args.str(i).as_bytes());
    let escape = handle_escape_argument(vm, escape, i + 1)?;
    Ok(escape.unwrap_or(i32::from(b'\\')))
}

/// The body of `PHP_FUNCTION(fgetcsv)` after the stream argument (#1) was
/// resolved: validates `$length`, `$separator`, `$enclosure` and `$escape`
/// (args 1..) like the C, reads a line from `stream` and parses it.
pub fn fgetcsv_impl(vm: &mut Vm, args: &mut Args, stream: &mut dyn CsvStream) -> PResult<Value> {
    let delimiter = single_char_arg(vm, args, 2, b',')?;
    let enclosure = single_char_arg(vm, args, 3, b'"')?;
    let escape = escape_arg(vm, args, 4)?;

    let maxlen = match args.int_opt(1) {
        None | Some(0) => None,
        Some(len) if !(0..i64::MAX).contains(&len) => {
            return Err(
                vm.argument_value_error(2, &format!("must be between 0 and {}", i64::MAX - 1))
            );
        }
        // The C reads into an `emalloc($length + 1)` buffer.
        Some(len) => Some(usize::try_from(len + 1).unwrap_or(usize::MAX)),
    };

    let Some(buf) = stream.get_line(vm, maxlen)? else {
        return Ok(Value::Bool(false));
    };
    let values = parse(vm, Some(stream), delimiter, enclosure, escape, buf)?;
    Ok(Value::array(values.unwrap_or_else(bc_empty_line)))
}

/// The body of `PHP_FUNCTION(fputcsv)` after the stream argument (#1) was
/// resolved: validates `$fields`, `$separator`, `$enclosure`, `$escape` and
/// `$eol` (args 1..) like the C and writes the line to `stream`.
///
/// The C parses `$eol` with `Z_PARAM_STR_OR_NULL` although the stub declares
/// `string`: a `null` reaching this function selects `"\n"`.
pub fn fputcsv_impl(vm: &mut Vm, args: &mut Args, stream: &mut dyn CsvStream) -> PResult<Value> {
    let fields = Rc::clone(args.array(1));
    let delimiter = single_char_arg(vm, args, 2, b',')?;
    let enclosure = single_char_arg(vm, args, 3, b'"')?;
    let escape = escape_arg(vm, args, 4)?;
    let eol = match args.get(5) {
        Value::String(s) if args.is_passed(5) => Some(s.clone()),
        _ => None,
    };

    let written = put_line(
        vm,
        stream,
        &fields,
        delimiter,
        enclosure,
        escape,
        eol.as_ref().map(PhpStr::as_bytes),
    )?;
    Ok(match written {
        Some(n) => Value::Int(n as i64),
        None => Value::Bool(false),
    })
}

// ---------------------------------------------------------------------------
// str_getcsv()
// ---------------------------------------------------------------------------

/// `PHP_FUNCTION(str_getcsv)`.
fn str_getcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let string = args.str(0).clone();
    let delimiter = single_char_arg(vm, args, 1, b',')?;
    let enclosure = single_char_arg(vm, args, 2, b'"')?;
    let escape = escape_arg(vm, args, 3)?;

    let values = parse(vm, None, delimiter, enclosure, escape, string.to_vec())?;
    Ok(Value::array(values.unwrap_or_else(bc_empty_line)))
}

pub static STR_GETCSV: NativeFunction = NativeFunction {
    sig: r#"str_getcsv(string $string, string $separator = ",", string $enclosure = "\"", string $escape = "\\"): array"#,
    handler: str_getcsv,
};

pub fn register(r: &mut Registry) {
    r.function(&STR_GETCSV);
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use php_runtime::class::{CastKind, CastObjectFn};
    use php_runtime::{ClassEntry, ObjRef, PhpRef};

    use super::*;
    use crate::testing::{TestVm, thrown};

    // ----- fake streams ------------------------------------------------------

    /// An in-memory stream with `php_stream_get_line` semantics.
    #[derive(Default)]
    struct Mem {
        data: Vec<u8>,
        pos: usize,
        out: Vec<u8>,
        fail_write: bool,
        maxlens: Vec<Option<usize>>,
    }

    impl CsvStream for Mem {
        fn get_line(&mut self, _vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
            self.maxlens.push(maxlen);
            let rest = &self.data[self.pos..];
            if rest.is_empty() {
                return Ok(None);
            }
            let mut n = rest
                .iter()
                .position(|&c| c == b'\n')
                .map_or(rest.len(), |i| i + 1);
            if let Some(maxlen) = maxlen {
                n = n.min(maxlen - 1);
            }
            self.pos += n;
            Ok(Some(rest[..n].to_vec()))
        }

        fn write(&mut self, _vm: &mut Vm, data: &[u8]) -> PResult<Option<usize>> {
            if self.fail_write {
                return Ok(None);
            }
            self.out.extend_from_slice(data);
            Ok(Some(data.len()))
        }
    }

    /// A stream handing out the given lines (and recording `maxlen`s).
    struct Lines(VecDeque<Vec<u8>>, Vec<Option<usize>>);

    impl Lines {
        fn new(lines: &[&[u8]]) -> Lines {
            Lines(lines.iter().map(|l| l.to_vec()).collect(), Vec::new())
        }
    }

    impl CsvStream for Lines {
        fn get_line(&mut self, _vm: &mut Vm, maxlen: Option<usize>) -> PResult<Option<Vec<u8>>> {
            self.1.push(maxlen);
            Ok(self.0.pop_front())
        }

        fn write(&mut self, _vm: &mut Vm, _data: &[u8]) -> PResult<Option<usize>> {
            unreachable!("parse() never writes")
        }
    }

    thread_local! {
        static MEM: RefCell<Mem> = RefCell::default();
        static CASTS: Cell<usize> = const { Cell::new(0) };
    }

    fn load(data: &[u8]) {
        MEM.with(|m| {
            *m.borrow_mut() = Mem {
                data: data.to_vec(),
                ..Mem::default()
            }
        });
    }

    fn with_mem<R>(f: impl FnOnce(&mut Mem) -> R) -> R {
        MEM.with(|m| f(&mut m.borrow_mut()))
    }

    fn pos() -> usize {
        with_mem(|m| m.pos)
    }

    fn written() -> Vec<u8> {
        with_mem(|m| std::mem::take(&mut m.out))
    }

    type Impl = fn(&mut Vm, &mut Args, &mut dyn CsvStream) -> PResult<Value>;

    fn on_mem(vm: &mut Vm, args: &mut Args, f: Impl) -> PResult<Value> {
        let mut mem = MEM.with(RefCell::take);
        let r = f(vm, args, &mut mem);
        MEM.with(|m| m.replace(mem));
        r
    }

    fn test_fgetcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        on_mem(vm, args, fgetcsv_impl)
    }

    fn test_fputcsv(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
        on_mem(vm, args, fputcsv_impl)
    }

    // The stub signatures of fgetcsv()/fputcsv(); `$stream` is ignored.
    static FGETCSV: NativeFunction = NativeFunction {
        sig: r#"fgetcsv($stream, ?int $length = null, string $separator = ",", string $enclosure = "\"", string $escape = "\\"): array|false"#,
        handler: test_fgetcsv,
    };

    static FPUTCSV: NativeFunction = NativeFunction {
        sig: r#"fputcsv($stream, array $fields, string $separator = ",", string $enclosure = "\"", string $escape = "\\", string $eol = "\n"): int|false"#,
        handler: test_fputcsv,
    };

    fn vm() -> TestVm {
        let mut t = TestVm::new();
        let mut r = Registry::new(&mut t.vm, "standard");
        r.function(&FGETCSV);
        r.function(&FPUTCSV);
        load(b"");
        t
    }

    // ----- helpers -----------------------------------------------------------

    fn s(v: &str) -> Value {
        Value::from(v)
    }

    fn bs(v: &[u8]) -> Value {
        Value::from(v)
    }

    fn strs(list: &[&str]) -> Vec<Vec<u8>> {
        list.iter().map(|f| f.as_bytes().to_vec()).collect()
    }

    fn fields(v: &Value) -> Vec<Vec<u8>> {
        match v {
            Value::Array(a) => a
                .values()
                .map(|v| match v {
                    Value::String(f) => f.to_vec(),
                    other => panic!("expected a string field, got {other:?}"),
                })
                .collect(),
            other => panic!("expected an array, got {other:?}"),
        }
    }

    fn array_fields(a: PhpArray) -> Vec<Vec<u8>> {
        fields(&Value::array(a))
    }

    fn is_empty_line(v: &Value) -> bool {
        matches!(v, Value::Array(a) if a.len() == 1 && matches!(a.get_int(0), Some(Value::Null)))
    }

    fn str_getcsv(t: &mut TestVm, input: &[u8], sep: &[u8], enc: &[u8], esc: &[u8]) -> Value {
        t.call("str_getcsv", vec![bs(input), bs(sep), bs(enc), bs(esc)])
            .unwrap()
    }

    fn str_getcsv_named(t: &mut TestVm, args: Vec<Value>, named: Vec<(&str, Value)>) -> Value {
        t.vm.call_native_named("str_getcsv", args, named).unwrap()
    }

    /// `fgetcsv($stream, ...$args)` on the in-memory stream.
    fn fgetcsv(t: &mut TestVm, args: Vec<Value>) -> PResult<Value> {
        let mut all = vec![Value::Null];
        all.extend(args);
        t.call("fgetcsv", all)
    }

    /// `fgetcsv($stream, $length, $separator, $enclosure, $escape)`.
    fn fgetcsv5(t: &mut TestVm, len: Value, sep: &str, enc: &str, esc: &str) -> Value {
        fgetcsv(t, vec![len, s(sep), s(enc), s(esc)]).unwrap()
    }

    /// Reads records with `fgetcsv5` until it returns false, each joined by ",".
    fn read_all(t: &mut TestVm, len: Value, sep: &str, enc: &str, esc: &str) -> Vec<Vec<u8>> {
        let mut records = Vec::new();
        loop {
            match fgetcsv5(t, len.clone(), sep, enc, esc) {
                r @ Value::Array(_) => records.push(fields(&r).join(&b","[..])),
                Value::Bool(false) => return records,
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// `fputcsv($stream, $fields, ...$args)` on the in-memory stream.
    fn fputcsv(t: &mut TestVm, fields: Vec<Value>, args: Vec<Value>) -> PResult<Value> {
        let mut all = vec![Value::Null, Value::array(PhpArray::from_values(fields))];
        all.extend(args);
        t.call("fputcsv", all)
    }

    fn explode(line: &str) -> Vec<Value> {
        line.split(',').map(s).collect()
    }

    fn deprecation(fname: &str) -> String {
        format!(
            "\nDeprecated: {fname}(): the $escape parameter must be provided as its default value will change in /t.php on line 3\n"
        )
    }

    fn err(class: &str, msg: &str) -> (String, String) {
        (class.to_string(), msg.to_string())
    }

    fn stringy(_vm: &mut Vm, _o: &ObjRef, kind: CastKind) -> PResult<Option<Value>> {
        CASTS.with(|c| c.set(c.get() + 1));
        Ok(matches!(kind, CastKind::String).then(|| Value::from("o b")))
    }

    fn throwing(vm: &mut Vm, _o: &ObjRef, _kind: CastKind) -> PResult<Option<Value>> {
        Err(vm.throw_error("Exception", "no string"))
    }

    fn object(name: &str, cast: Option<CastObjectFn>) -> Value {
        let mut ce = ClassEntry::new(name, 0);
        ce.handlers.cast_object = cast;
        Value::Object(ObjRef::new(Rc::new(ce)))
    }

    // ----- internals ---------------------------------------------------------

    #[test]
    fn mblen_decodes_strict_utf8() {
        assert_eq!(mblen(b"a"), 1);
        assert_eq!(mblen("é".as_bytes()), 2);
        assert_eq!(mblen("€".as_bytes()), 3);
        assert_eq!(mblen("𝄞".as_bytes()), 4);
        assert_eq!(mblen(b"\xC3"), -1); // truncated
        assert_eq!(mblen(b"\xC3a"), -1);
        assert_eq!(mblen(b"\xA9"), -1); // lone trail byte
        assert_eq!(mblen(b"\xC0\x80"), -1); // overlong
        assert_eq!(mblen(b"\xE0\x80\x80"), -1); // overlong
        assert_eq!(mblen(b"\xED\xA0\x80"), -1); // surrogate
        assert_eq!(mblen(b"\xF4\x90\x80\x80"), -1); // above U+10FFFF
        assert_eq!(mblen(b"\xFF"), -1);
    }

    #[test]
    fn lookup_trailing_spaces_strips_one_line_ending() {
        assert_eq!(lookup_trailing_spaces(b""), 0);
        assert_eq!(lookup_trailing_spaces(b"a"), 1);
        assert_eq!(lookup_trailing_spaces(b"a "), 2);
        assert_eq!(lookup_trailing_spaces(b"a\n"), 1);
        assert_eq!(lookup_trailing_spaces(b"a\r"), 1);
        assert_eq!(lookup_trailing_spaces(b"a\r\n"), 1);
        assert_eq!(lookup_trailing_spaces(b"a\n\r"), 2);
        assert_eq!(lookup_trailing_spaces(b"a\n\n"), 2);
        assert_eq!(lookup_trailing_spaces(b"\r\n"), 0);
        // Invalid bytes are not "last characters"...
        assert_eq!(lookup_trailing_spaces(b"a\n\xFF"), 2);
        assert_eq!(lookup_trailing_spaces(b"a\r\n\xFF"), 2);
        // ...but a multibyte character is.
        assert_eq!(lookup_trailing_spaces("a\né".as_bytes()), 4);
    }

    #[test]
    fn handle_escape_argument_results() {
        let mut t = vm();
        let vm = &mut t.vm;
        assert_eq!(
            handle_escape_argument(vm, Some(b""), 4).unwrap(),
            Some(CSV_NO_ESCAPE)
        );
        assert_eq!(handle_escape_argument(vm, Some(b"/"), 4).unwrap(), Some(47));
        assert_eq!(
            handle_escape_argument(vm, Some(b"\xFF"), 4).unwrap(),
            Some(255)
        );
        assert!(t.output().is_empty());
        assert_eq!(
            handle_escape_argument(&mut t.vm, None, 4).unwrap(),
            Some(92)
        );
        assert!(
            t.output().contains(
                "the $escape parameter must be provided as its default value will change"
            )
        );
    }

    // ----- str_getcsv() ------------------------------------------------------

    #[test]
    fn str_getcsv_001() {
        let mut t = vm();
        let esc = || vec![("escape", s(""))];
        let r = str_getcsv_named(&mut t, vec![s(r#""f", "o", """#)], esc());
        assert_eq!(fields(&r), strs(&["f", "o", ""]));
        let r = str_getcsv_named(&mut t, vec![s("foo||bar"), s("|")], esc());
        assert_eq!(fields(&r), strs(&["foo", "", "bar"]));
        let r = str_getcsv_named(&mut t, vec![s("foo|bar"), s("|")], esc());
        assert_eq!(fields(&r), strs(&["foo", "bar"]));
        let r = str_getcsv_named(&mut t, vec![s("|foo|-|bar|"), s("-"), s("|")], esc());
        assert_eq!(fields(&r), strs(&["foo", "bar"]));
        let r = str_getcsv(&mut t, b"|f.|.|bar|.|-|-.|", b".", b"|", b"-");
        assert_eq!(fields(&r), strs(&["f.", "bar", "-|-."]));
        let r = str_getcsv(&mut t, b".foo..bar.", b".", b".", b".");
        assert_eq!(fields(&r), strs(&["foo.bar"]));
        let r = str_getcsv(&mut t, b".foo  . .  bar  .", b" ", b".", b"");
        assert_eq!(fields(&r), strs(&["foo  ", "  bar  "]));
        let r = str_getcsv_named(&mut t, vec![s(r#"" "" ""#), s(" ")], esc());
        assert_eq!(fields(&r), strs(&[r#" " "#]));
        let r = str_getcsv_named(&mut t, vec![s("")], esc());
        assert!(is_empty_line(&r));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn str_getcsv_002_empty_escape() {
        let mut t = vm();
        let r = str_getcsv(
            &mut t,
            br#""cell1","cell2\","cell3","cell4""#,
            b",",
            b"\"",
            b"",
        );
        assert_eq!(fields(&r), strs(&["cell1", r"cell2\", "cell3", "cell4"]));
    }

    #[test]
    fn str_getcsv_errors() {
        let mut t = vm();
        let mut call = |sep: &str, enc: &str, esc: &str| {
            thrown(t.call("str_getcsv", vec![s("csv_string"), s(sep), s(enc), s(esc)]))
        };
        let sep_err = "str_getcsv(): Argument #2 ($separator) must be a single character";
        assert_eq!(call("separator", "\"", ""), err("ValueError", sep_err));
        assert_eq!(call("", "\"", ""), err("ValueError", sep_err));
        assert_eq!(
            call(",", "enclosure", ""),
            err(
                "ValueError",
                "str_getcsv(): Argument #3 ($enclosure) must be a single character"
            )
        );
        assert_eq!(
            call(",", "\"", "escape"),
            err(
                "ValueError",
                "str_getcsv(): Argument #4 ($escape) must be empty or a single character"
            )
        );
    }

    #[test]
    fn str_getcsv_default_escape_is_deprecated() {
        let mut t = vm();
        let r = t.call("str_getcsv", vec![s(r#"a,"b\"c",d"#)]).unwrap();
        assert_eq!(t.output(), deprecation("str_getcsv"));
        assert_eq!(fields(&r), strs(&["a", r#"b\"c"#, "d"]));

        // Skipping $escape with named arguments still means "not passed".
        let r = str_getcsv_named(&mut t, vec![s("'a'")], vec![("enclosure", s("'"))]);
        assert_eq!(t.output(), deprecation("str_getcsv"));
        assert_eq!(fields(&r), strs(&["a"]));

        // The separator and enclosure are validated first.
        let r = thrown(t.call("str_getcsv", vec![s("a"), s("")]));
        assert_eq!(r.0, "ValueError");
        assert_eq!(t.output(), "");

        str_getcsv_named(&mut t, vec![s("a")], vec![("escape", s("\\"))]);
        assert_eq!(t.output(), "");
    }

    #[test]
    fn bug55674_empty_fields_with_tabs() {
        let mut t = vm();
        let r = str_getcsv(&mut t, b"0\t\t\"2\"\n", b"\t", b"\"", b"");
        assert_eq!(fields(&r), strs(&["0", "", "2"]));
        let r = str_getcsv(&mut t, b"0\t \t'2'\n", b"\t", b"'", b"");
        assert_eq!(fields(&r), strs(&["0", " ", "2"]));
        let r = str_getcsv(&mut t, b",,,,", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["", "", "", "", ""]));
        let r = str_getcsv(&mut t, b" \t  \t\t\t ", b"\t", b"\"", b"");
        assert_eq!(fields(&r), strs(&[" ", "  ", "", "", " "]));
    }

    #[test]
    fn bug65947_lone_latin1_byte() {
        let mut t = vm();
        let r = str_getcsv(&mut t, b"\xE9", b";", b"\"", b"");
        assert_eq!(fields(&r), vec![b"\xE9".to_vec()]);
    }

    #[test]
    fn bug72330_escape_before_utf8() {
        let mut t = vm();
        let r = str_getcsv(
            &mut t,
            b"\"first #\xD1\x81\xD8\x80\";\"second\"",
            b";",
            b"\"",
            b"#",
        );
        assert_eq!(
            fields(&r),
            vec![b"first #\xD1\x81\xD8\x80".to_vec(), b"second".to_vec()]
        );
    }

    #[test]
    fn utf8_trail_bytes_are_not_separators_or_enclosures() {
        let mut t = vm();
        // 0xA9 is the trail byte of "é" (C3 A9) and a separator on its own.
        let r = str_getcsv(&mut t, b"\xC3\xA9\xA9x", b"\xA9", b"\"", b"");
        assert_eq!(fields(&r), vec![b"\xC3\xA9".to_vec(), b"x".to_vec()]);
        let r = str_getcsv(&mut t, b"\xA9a\xC3\xA9b\xA9", b",", b"\xA9", b"");
        assert_eq!(fields(&r), vec![b"a\xC3\xA9b".to_vec()]);
    }

    #[test]
    fn invalid_byte_after_line_ending_is_stripped() {
        let mut t = vm();
        let r = str_getcsv(&mut t, b"a\n\xFF", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a"]));
    }

    #[test]
    fn high_escape_byte_never_matches() {
        let mut t = vm();
        let r = str_getcsv(&mut t, b"\"a\xE9\"\",b\"", b",", b"\"", b"\xE9");
        assert_eq!(fields(&r), vec![b"a\xE9\",b".to_vec()]);
    }

    #[test]
    fn str_getcsv_line_endings_and_whitespace() {
        let mut t = vm();
        for input in [&b""[..], b"\n", b"\r\n", b"\r"] {
            assert!(is_empty_line(&str_getcsv(&mut t, input, b",", b"\"", b"")));
        }
        let r = str_getcsv(&mut t, b"  \n", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["  "]));
        // The trailing line ending is stripped, embedded ones are kept.
        let r = str_getcsv(&mut t, b"a,b\r\n", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a", "b"]));
        let r = str_getcsv(&mut t, b"a\r,b", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a", "b"]));
        let r = str_getcsv(&mut t, b"\"a\r\nb\",c", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a\r\nb", "c"]));
        // Whitespace before an enclosure is skipped, after it kept.
        let r = str_getcsv(&mut t, b"a, \t\"b\" ,c", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a", "b ", "c"]));
        // Unterminated enclosures take the rest of the input.
        let r = str_getcsv(&mut t, b"a,\"b,c\n", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a", "b,c\n"]));
        let r = str_getcsv(&mut t, b"a,\"", b",", b"\"", b"");
        assert_eq!(fields(&r), strs(&["a", ""]));
        // A NUL escape character copies the terminator, as in the C.
        let r = str_getcsv(&mut t, b"a,\"", b",", b"\"", b"\0");
        assert_eq!(fields(&r), vec![b"a".to_vec(), b"\0".to_vec()]);
        // NUL bytes are ordinary characters.
        let r = str_getcsv(&mut t, b"a\0b,\"c\0d\"", b",", b"\"", b"");
        assert_eq!(fields(&r), vec![b"a\0b".to_vec(), b"c\0d".to_vec()]);
    }

    // ----- parse() -----------------------------------------------------------

    #[test]
    fn parse_continues_enclosures_on_later_lines() {
        let mut t = vm();
        let esc = i32::from(b'\\');
        let mut lines = Lines::new(&[b"b\r\n", b"c\",d\n", b"unused\n"]);
        let r = parse(
            &mut t.vm,
            Some(&mut lines),
            b',',
            b'"',
            esc,
            b"x,\"a\n".to_vec(),
        );
        assert_eq!(
            array_fields(r.unwrap().unwrap()),
            strs(&["x", "a\nb\r\nc", "d"])
        );
        // Continuation lines are read in grow mode.
        assert_eq!(lines.1, vec![None, None]);
        assert_eq!(lines.0.len(), 1);

        // At EOF the field takes everything read.
        let mut lines = Lines::new(&[b"b\n"]);
        let r = parse(
            &mut t.vm,
            Some(&mut lines),
            b',',
            b'"',
            esc,
            b"\"a\n".to_vec(),
        );
        assert_eq!(array_fields(r.unwrap().unwrap()), strs(&["a\nb\n"]));

        // Without a stream (str_getcsv) the input is all there is.
        let r = parse(&mut t.vm, None, b',', b'"', esc, b"\"a\n".to_vec());
        assert_eq!(array_fields(r.unwrap().unwrap()), strs(&["a\n"]));
        let r = parse(&mut t.vm, None, b',', b'"', esc, b"\n".to_vec());
        assert!(r.unwrap().is_none());
        assert!(is_empty_line(&Value::array(bc_empty_line())));
    }

    // ----- fgetcsv() ---------------------------------------------------------

    #[test]
    fn fgetcsv_phpt() {
        let cases: &[(&str, &[&str])] = &[
            ("aaa,bbb", &["aaa", "bbb"]),
            ("aaa,\"bbb\"", &["aaa", "bbb"]),
            ("\"aaa\",\"bbb\"", &["aaa", "bbb"]),
            ("aaa,bbb", &["aaa", "bbb"]),
            ("\"aaa\",bbb", &["aaa", "bbb"]),
            ("\"aaa\",   \"bbb\"", &["aaa", "bbb"]),
            (",", &["", ""]),
            ("aaa,", &["aaa", ""]),
            (",\"aaa\"", &["", "aaa"]),
            ("\"\",\"\"", &["", ""]),
            ("\"\\\"\",\"aaa\"", &[r#"\""#, "aaa"]),
            ("\"\"\"\"\"\",", &[r#""""#, ""]),
            ("\"\"\"\"\",aaa\n", &["\"\",aaa\n"]),
            ("\"\\\"\"\",aaa\n", &["\\\"\",aaa\n"]),
            ("aaa,\"\\\"bbb,ccc\n", &["aaa", "\\\"bbb,ccc\n"]),
            ("aaa,bbb   ", &["aaa", "bbb   "]),
            ("aaa,\"bbb   \"", &["aaa", "bbb   "]),
            ("aaa\"aaa\",\"bbb\"bbb", &[r#"aaa"aaa""#, "bbbbbb"]),
            ("aaa\"aaa\"\"\",bbb", &[r#"aaa"aaa""""#, "bbb"]),
            ("aaa\"\\\"a\",\"bbb\"", &[r#"aaa"\"a""#, "bbb"]),
            ("aaa,\"bbb\n", &["aaa", "bbb\n"]),
            ("aaa,\"bbb", &["aaa", "bbb"]),
        ];
        let mut t = vm();
        for (input, expected) in cases {
            load(input.as_bytes());
            let r =
                t.vm.call_native_named(
                    "fgetcsv",
                    vec![Value::Null, Value::Int(1024)],
                    vec![("escape", s("\\"))],
                )
                .unwrap();
            assert_eq!(fields(&r), strs(expected), "input {input:?}");
        }
        assert_eq!(t.output(), "");
    }

    #[test]
    fn fgetcsv_error_conditions() {
        let mut t = vm();
        load(b"a,b\n");
        let len_err = "fgetcsv(): Argument #2 ($length) must be between 0 and 9223372036854775806";
        let cases = [
            vec![Value::Null, Value::Int(-10)],
            vec![Value::Null, Value::Int(-10), s(",")],
            vec![Value::Null, Value::Int(-10), s(","), s("\"")],
        ];
        for args in cases {
            let r =
                t.vm.call_native_named("fgetcsv", args, vec![("escape", s("\\"))]);
            assert_eq!(thrown(r), err("ValueError", len_err));
        }
        let sep_err = "fgetcsv(): Argument #3 ($separator) must be a single character";
        let enc_err = "fgetcsv(): Argument #4 ($enclosure) must be a single character";
        let esc_err = "fgetcsv(): Argument #5 ($escape) must be empty or a single character";
        let mut call = |sep: &str, enc: &str, esc: &str| {
            thrown(fgetcsv(
                &mut t,
                vec![Value::Int(1024), s(sep), s(enc), s(esc)],
            ))
        };
        assert_eq!(call("", "\"", "\\"), err("ValueError", sep_err));
        assert_eq!(call(",", "", "\\"), err("ValueError", enc_err));
        assert_eq!(call("", "", "\\"), err("ValueError", sep_err));
        assert_eq!(call(",", "\"", "ab"), err("ValueError", esc_err));
        let r = fgetcsv(&mut t, vec![Value::Int(i64::MAX), s(","), s("\""), s("")]);
        assert_eq!(thrown(r), err("ValueError", len_err));
        // Nothing was read.
        assert_eq!(pos(), 0);
        assert_eq!(t.output(), "");

        let r = fgetcsv5(&mut t, Value::Int(i64::MAX - 1), ",", "\"", "");
        assert_eq!(fields(&r), strs(&["a", "b"]));
        assert_eq!(
            with_mem(|m| m.maxlens.clone()),
            vec![Some(i64::MAX as usize)]
        );
    }

    #[test]
    fn fgetcsv_default_escape_deprecation() {
        let mut t = vm();
        load(b"test1,test2");
        let r = fgetcsv(&mut t, vec![Value::Int(1024)]).unwrap();
        assert_eq!(t.output(), deprecation("fgetcsv"));
        assert_eq!(fields(&r), strs(&["test1", "test2"]));
        // The deprecation is raised before $length is checked.
        let r = thrown(fgetcsv(&mut t, vec![Value::Int(-1)]));
        assert_eq!(r.0, "ValueError");
        assert_eq!(t.output(), deprecation("fgetcsv"));
    }

    #[test]
    fn fgetcsv_length_selects_the_buffer_size() {
        let mut t = vm();
        load(b"a,b\nc,d\ne,f\n\"g\nh\"\n");
        for len in [Value::Null, Value::Int(0), Value::Int(9)] {
            fgetcsv5(&mut t, len, ",", "\"", "");
        }
        let r = fgetcsv5(&mut t, Value::Int(3), ",", "\"", "");
        assert_eq!(fields(&r), strs(&["g\nh"]));
        assert_eq!(
            with_mem(|m| m.maxlens.clone()),
            vec![None, None, Some(10), Some(4), None]
        );
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "");
        assert!(matches!(r, Value::Bool(false)));
    }

    /// The fields one `fgetcsv()` call returns and the stream position after it.
    type Read<'a> = (&'a [&'a str], usize);

    #[test]
    fn fgetcsv_variation6_length_less_than_line() {
        const TEXT: &str = "This is line of text without csv fields";
        let words = [
            "This", "is", "line", "of", "text", "without", "csv", "fields",
        ];
        // (separator, enclosure, line, read with $length 9, then with 1024)
        let cases: &[(&str, &str, &str, Read, Read)] = &[
            (
                ",",
                "\"",
                "\"water\",fruit",
                (&["water", "f"], 9),
                (&["ruit"], 14),
            ),
            (
                ",",
                "\"",
                "\"water\",\"fruit\"",
                (&["water", "fruit"], 16),
                (&[TEXT], 56),
            ),
            (
                " ",
                "^",
                "^water^ ^fruit^",
                (&["water", "fruit"], 16),
                (&words, 56),
            ),
            (
                ":",
                "&",
                "&water&:&fruit&",
                (&["water", "fruit"], 16),
                (&[TEXT], 56),
            ),
            (
                "=",
                "=",
                "=water===fruit=",
                (&["water="], 9),
                (&["fruit", ""], 16),
            ),
            (
                "-",
                "-",
                "-water--fruit-air",
                (&["water-fruitair"], 18),
                (&[TEXT], 58),
            ),
            (
                "-",
                "-",
                "-water---fruit---air-",
                (&["water-"], 9),
                (&["fruit", "air", ""], 22),
            ),
            (
                ":",
                "&",
                "&\"\"\"\"&:&\"&:,:\":&,&:,,,,",
                (&["\"\"\"\"", "\"", ",", "\"", ",", ",,,,"], 24),
                (&[TEXT], 64),
            ),
        ];
        let mut t = vm();
        for &(sep, enc, line, first, second) in cases {
            load(format!("{line}\n{TEXT}\n\n").as_bytes());
            for (len, (expected, end)) in [(9, first), (1024, second)] {
                let r = fgetcsv5(&mut t, Value::Int(len), sep, enc, "\\");
                assert_eq!(fields(&r), strs(expected), "{line}");
                assert_eq!(pos(), end, "{line}");
            }
        }
    }

    #[test]
    fn fgetcsv_blank_lines_and_eof() {
        let mut t = vm();
        load(b"\n\r\n  \nx\n");
        assert!(is_empty_line(&fgetcsv5(&mut t, Value::Null, ",", "\"", "")));
        assert!(is_empty_line(&fgetcsv5(&mut t, Value::Null, ",", "\"", "")));
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "");
        assert_eq!(fields(&r), strs(&["  "]));
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "");
        assert_eq!(fields(&r), strs(&["x"]));
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "");
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn fgetcsv_variation32_empty_escape() {
        let mut t = vm();
        load(b"\"cell1\",\"cell2\\\",\"cell3\",\"cell4\"\n\"\\\\\\line1\nline2\\\\\\\"");
        let records = read_all(&mut t, Value::Int(0), ",", "\"", "");
        assert_eq!(
            records,
            strs(&["cell1,cell2\\,cell3,cell4", "\\\\\\line1\nline2\\\\\\"])
        );
    }

    #[test]
    fn fgetcsv_variation33_unterminated_enclosure_at_eof() {
        let mut t = vm();
        load(b"\"cell1\",\"cell2\"\n\"cell1\",\"");
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\\");
        assert_eq!(fields(&r), strs(&["cell1", "cell2"]));
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\\");
        assert_eq!(fields(&r), strs(&["cell1", ""]));
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\\");
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn fgetcsv_tab_delimiter() {
        let mut t = vm();
        load(b"name\tvalue\tcomment\ntrue\t1\tboolean true\nfalse\t0\tboolean false\nempty\t\tnothing\n");
        let records = read_all(&mut t, Value::Int(100), "\t", "\"", "");
        assert_eq!(
            records,
            strs(&[
                "name,value,comment",
                "true,1,boolean true",
                "false,0,boolean false",
                "empty,,nothing",
            ])
        );
    }

    #[test]
    fn bug12556_length_ignored_for_unterminated_enclosure() {
        let mut t = vm();
        let mut data = b"6,7,8,line1\n1,2,3,\"line2\n".to_vec();
        let mut rest = b"line2\n".to_vec();
        for _ in 0..15 {
            data.extend_from_slice(b"2,4,5,line3\n");
            rest.extend_from_slice(b"2,4,5,line3\n");
        }
        load(&data);
        let r = fgetcsv5(&mut t, Value::Int(24), ",", "\"", "\\");
        assert_eq!(fields(&r), strs(&["6", "7", "8", "line1"]));
        let r = fgetcsv5(&mut t, Value::Int(24), ",", "\"", "\\");
        let mut expected = strs(&["1", "2", "3"]);
        expected.push(rest);
        assert_eq!(fields(&r), expected);
        assert_eq!(fields(&r)[3].len(), 186);
        let r = fgetcsv5(&mut t, Value::Int(24), ",", "\"", "\\");
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn bug26003_binary_safe() {
        let mut t = vm();
        load(b"abc,d\0e\n");
        let r = fgetcsv5(&mut t, Value::Int(4096), ",", "\"", "\\");
        assert_eq!(fields(&r), vec![b"abc".to_vec(), b"d\0e".to_vec()]);
    }

    #[test]
    fn bug22382_escaped_quotes() {
        let mut t = vm();
        let mut data = br#""One","\"Two\"","Three\"","Four","\\",""#.to_vec();
        let mut last = vec![b'\\'; 23];
        last.push(b'"');
        last.extend_from_slice(&[b'\\'; 4]);
        data.extend_from_slice(&last);
        data.extend_from_slice(b"\"\n");
        load(&data);
        let r = fgetcsv5(&mut t, Value::Int(1024), ",", "\"", "\\");
        let mut expected = strs(&["One", r#"\"Two\""#, r#"Three\""#, "Four", r"\\"]);
        expected.push(last);
        assert_eq!(fields(&r), expected);
        assert_eq!(fields(&r)[5].len(), 28);
    }

    #[test]
    fn bug39538_line_endings_inside_enclosures() {
        let mut t = vm();
        for eol in ["\n", "\r\n", "\n\r"] {
            let line = format!("\"{eol}this is a test\", \"next data\", \"p{eol}arsed\"");
            load(line.as_bytes());
            let len = Value::Int(line.len() as i64);
            let r = fgetcsv5(&mut t, len, ",", "\"", "\\");
            let expected = [
                format!("{eol}this is a test"),
                "next data".to_string(),
                format!("p{eol}arsed"),
            ];
            assert_eq!(fields(&r), strs(&expected.each_ref().map(String::as_str)));
        }
    }

    #[test]
    fn bug40501_escape_same_as_enclosure() {
        let mut t = vm();
        let first = "this element contains the delimiter, and ends with an odd number of\nbackslashes (ex: 1)\\";
        load(format!("\"{first}\",and it isn't the last element$").as_bytes());
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\"");
        assert_eq!(fields(&r), strs(&[first, "and it isn't the last element$"]));
    }

    #[test]
    fn bug53848_leading_spaces_kept() {
        let mut t = vm();
        load(b"a,b\n  c,  d");
        let records = read_all(&mut t, Value::Null, ",", "\"", "\\");
        assert_eq!(records, strs(&["a,b", "  c,  d"]));
    }

    #[test]
    fn bug66588_premature_eof() {
        let mut t = vm();
        load(b"\",bar");
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\\");
        assert_eq!(fields(&r), strs(&[",bar"]));
        load(b"\",bar\n");
        let r = fgetcsv5(&mut t, Value::Null, ",", "\"", "\\");
        assert_eq!(fields(&r), strs(&[",bar\n"]));
    }

    // ----- fputcsv() ---------------------------------------------------------

    /// The `$list` of fputcsv.phpt.
    const FPUTCSV_LIST: [&str; 20] = [
        "aaa,bbb",
        "aaa,\"bbb\"",
        "\"aaa\",\"bbb\"",
        "aaa,bbb",
        "\"aaa\",bbb",
        "\"aaa\",   \"bbb\"",
        ",",
        "aaa,",
        ",\"aaa\"",
        "\"\",\"\"",
        "\"\"\"\"\"\",",
        "\"\"\"\"\",aaa",
        "aaa,bbb   ",
        "aaa,\"bbb   \"",
        "aaa\"aaa\",\"bbb\"bbb",
        "aaa\"aaa\"\"\",bbb",
        "aaa,\"\\\"bbb,ccc",
        "aaa\"\\\"a\",\"bbb\"",
        "\"\\\"\",\"aaa\"",
        "\"\\\"\"\",aaa",
    ];

    /// The lines fputcsv.phpt expects for [`FPUTCSV_LIST`].
    const FPUTCSV_LINES: [&str; 20] = [
        "aaa,bbb",
        "aaa,\"\"\"bbb\"\"\"",
        "\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"",
        "aaa,bbb",
        "\"\"\"aaa\"\"\",bbb",
        "\"\"\"aaa\"\"\",\"   \"\"bbb\"\"\"",
        ",",
        "aaa,",
        ",\"\"\"aaa\"\"\"",
        "\"\"\"\"\"\",\"\"\"\"\"\"",
        "\"\"\"\"\"\"\"\"\"\"\"\"\"\",",
        "\"\"\"\"\"\"\"\"\"\"\"\",aaa",
        "aaa,\"bbb   \"",
        "aaa,\"\"\"bbb   \"\"\"",
        "\"aaa\"\"aaa\"\"\",\"\"\"bbb\"\"bbb\"",
        "\"aaa\"\"aaa\"\"\"\"\"\"\",bbb",
        "aaa,\"\"\"\\\"bbb\",ccc",
        "\"aaa\"\"\\\"a\"\"\",\"\"\"bbb\"\"\"",
        "\"\"\"\\\"\"\"\",\"\"\"aaa\"\"\"",
        "\"\"\"\\\"\"\"\"\"\",aaa",
    ];

    fn round_trip(t: &mut TestVm, n: usize, esc: &str) {
        load(b"");
        for v in &FPUTCSV_LIST[..n] {
            fputcsv(t, explode(v), vec![s(","), s("\""), s(esc)]).unwrap();
        }
        let out = written();
        let expected: String = FPUTCSV_LINES[..n]
            .iter()
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(String::from_utf8_lossy(&out), expected);
        load(&out);
        let records = read_all(t, Value::Int(0), ",", "\"", esc);
        assert_eq!(records, strs(&FPUTCSV_LIST[..n]));
        assert_eq!(t.output(), "");
    }

    #[test]
    fn fputcsv_phpt() {
        round_trip(&mut vm(), 20, "\\");
    }

    #[test]
    fn fputcsv_variation15_empty_escape() {
        round_trip(&mut vm(), 16, "");
    }

    #[test]
    fn fputcsv_variation16_empty_escape() {
        let mut t = vm();
        for field in ["\\", "\\\""] {
            fputcsv(&mut t, vec![s(field)], vec![s(","), s("\""), s("")]).unwrap();
        }
        assert_eq!(written(), b"\\\n\"\\\"\"\"\n");
    }

    #[test]
    fn fputcsv_variation17_user_eol() {
        let mut t = vm();
        let data = [
            &["aaa", "bbb", "ccc", "dddd"][..],
            &["123", "456", "789"],
            &["\"aaa\"", "\"bbb\""],
        ];
        let cases: [(&[u8], &[u8]); 5] = [
            (
                b"||",
                b"aaa,bbb,ccc,dddd||123,456,789||\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"||",
            ),
            (
                b"|",
                b"aaa,bbb,ccc,dddd|123,456,789|\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"|",
            ),
            (
                b"\\n",
                b"aaa,bbb,ccc,dddd\\n123,456,789\\n\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"\\n",
            ),
            (
                b"\n",
                b"aaa,bbb,ccc,dddd\n123,456,789\n\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"\n",
            ),
            (
                b"\0",
                b"aaa,bbb,ccc,dddd\x00123,456,789\x00\"\"\"aaa\"\"\",\"\"\"bbb\"\"\"\x00",
            ),
        ];
        for (eol, expected) in cases {
            for record in data {
                let args = vec![s(","), s("\""), s(""), bs(eol)];
                fputcsv(&mut t, record.iter().map(|f| s(f)).collect(), args).unwrap();
            }
            assert_eq!(written(), expected);
        }
    }

    #[test]
    fn fputcsv_variation18_escape_matters() {
        let mut t = vm();
        let list = [
            r#"aaa,"/"bbb,ccc"#,
            r#"aaa"/"a","bbb""#,
            r#""/"","aaa""#,
            r#""/""",aaa"#,
        ];
        for v in list {
            fputcsv(&mut t, explode(v), vec![s(","), s("\""), s("/")]).unwrap();
        }
        let out = written();
        let expected = [
            r#"aaa,"""/"bbb",ccc"#,
            r#""aaa""/"a""","""bbb""""#,
            r#""""/"""","""aaa""""#,
            r#""""/"""""",aaa"#,
        ];
        let expected: String = expected.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(String::from_utf8_lossy(&out), expected);
        load(&out);
        assert_eq!(read_all(&mut t, Value::Int(0), ",", "\"", "/"), strs(&list));
    }

    #[test]
    fn fputcsv_002_non_string_fields() {
        let mut t = vm();
        let nested = Value::array(PhpArray::from_values(vec![
            Value::Int(4),
            Value::Int(5),
            Value::Int(6),
        ]));
        let fields = vec![
            Value::Int(1),
            Value::Int(2),
            s("foo"),
            s("haha"),
            nested,
            Value::Float(1.3),
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Ref(PhpRef::new(s("a b"))),
        ];
        let r = fputcsv(&mut t, fields, vec![s(","), s("\""), s("\\")]).unwrap();
        assert_eq!(
            t.output(),
            "\nWarning: Array to string conversion in /t.php on line 3\n"
        );
        let line = b"1,2,foo,haha,Array,1.3,,1,,\"a b\"\n";
        assert_eq!(written(), line);
        assert!(matches!(r, Value::Int(n) if n == line.len() as i64));
    }

    #[test]
    fn fputcsv_default_escape_deprecation() {
        let mut t = vm();
        let r = fputcsv(&mut t, vec![s("test1"), s("test2")], vec![]).unwrap();
        assert_eq!(t.output(), deprecation("fputcsv"));
        assert_eq!(written(), b"test1,test2\n");
        assert!(matches!(r, Value::Int(12)));

        // Passing $eol by name fills the skipped $escape with its default.
        let fields = Value::array(PhpArray::from_values(vec![s("a\\b")]));
        let r =
            t.vm.call_native_named(
                "fputcsv",
                vec![Value::Null, fields],
                vec![("eol", s("\r\n"))],
            )
            .unwrap();
        assert_eq!(t.output(), "");
        assert_eq!(written(), b"\"a\\b\"\r\n");
        assert!(matches!(r, Value::Int(7)));
    }

    #[test]
    fn fputcsv_errors() {
        let mut t = vm();
        let mut call = |args: &[&str]| {
            thrown(fputcsv(
                &mut t,
                vec![s("a")],
                args.iter().map(|a| s(a)).collect(),
            ))
        };
        assert_eq!(
            call(&["", "\"", ""]),
            err(
                "ValueError",
                "fputcsv(): Argument #3 ($separator) must be a single character"
            )
        );
        assert_eq!(
            call(&[",", "ab", ""]),
            err(
                "ValueError",
                "fputcsv(): Argument #4 ($enclosure) must be a single character"
            )
        );
        assert_eq!(
            call(&[",", "\"", "ab"]),
            err(
                "ValueError",
                "fputcsv(): Argument #5 ($escape) must be empty or a single character"
            )
        );
        assert!(written().is_empty());

        with_mem(|m| m.fail_write = true);
        let r = fputcsv(&mut t, vec![s("a")], vec![s(","), s("\""), s("")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn format_quoting_rules() {
        let mut t = vm();
        let mut line = |field: &[u8], escape: i32| {
            let fields = PhpArray::from_values(vec![bs(field)]);
            format(&mut t.vm, &fields, b',', b'"', escape, Some(b""))
                .unwrap()
                .unwrap()
        };
        let bsl = i32::from(b'\\');
        let cases: [(&[u8], i32, &[u8]); 14] = [
            (b"plain", bsl, b"plain"),
            (b"", bsl, b""),
            (b"a b", bsl, b"\"a b\""),
            (b"a\tb", bsl, b"\"a\tb\""),
            (b"a\rb", bsl, b"\"a\rb\""),
            (b"a\nb", bsl, b"\"a\nb\""),
            (b"a,b", bsl, b"\"a,b\""),
            (b"a\"b", bsl, b"\"a\"\"b\""),
            // The escape character forces an enclosure...
            (b"a\\b", bsl, b"\"a\\b\""),
            (b"a\\b", CSV_NO_ESCAPE, b"a\\b"),
            // ...and an enclosure right after it is not doubled.
            (b"a\\\"b", bsl, b"\"a\\\"b\""),
            (b"a\\\"b", CSV_NO_ESCAPE, b"\"a\\\"\"b\""),
            // A high escape byte encloses the field but never escapes.
            (b"\xE9", 0xE9, b"\"\xE9\""),
            (b"a\xE9\"b", 0xE9, b"\"a\xE9\"\"b\""),
        ];
        for (field, escape, expected) in cases {
            assert_eq!(line(field, escape), expected, "{:?}", PhpStr::from(field));
        }

        let fields = PhpArray::from_values(vec![s("a"), s("b c")]);
        let r = format(&mut t.vm, &fields, b';', b'\'', CSV_NO_ESCAPE, None);
        assert_eq!(r.unwrap().unwrap(), b"a;'b c'\n");
        let r = format(&mut t.vm, &PhpArray::new(), b',', b'"', CSV_NO_ESCAPE, None);
        assert_eq!(r.unwrap().unwrap(), b"\n");
    }

    #[test]
    fn fputcsv_objects() {
        let mut t = vm();
        let args = || vec![s(","), s("\""), s("")];
        fputcsv(&mut t, vec![object("S", Some(stringy)), s("x")], args()).unwrap();
        assert_eq!(written(), b"\"o b\",x\n");

        // A throwing conversion does not stop the line, which is written
        // before the exception propagates; later objects are not converted.
        CASTS.with(|c| c.set(0));
        let fields = vec![
            s("a"),
            object("T", Some(throwing)),
            object("S", Some(stringy)),
            s("b"),
        ];
        let r = fputcsv(&mut t, fields, args());
        assert_eq!(thrown(r), err("Exception", "no string"));
        assert_eq!(written(), b"a,,,b\n");
        assert_eq!(CASTS.with(Cell::get), 0);

        let r = fputcsv(&mut t, vec![object("Plain", None)], args());
        assert_eq!(
            thrown(r),
            err(
                "Error",
                "Object of class Plain could not be converted to string"
            )
        );
        assert_eq!(written(), b"\n");

        // format() reports the exception instead of a line.
        let fields = PhpArray::from_values(vec![object("T", Some(throwing))]);
        let r = format(&mut t.vm, &fields, b',', b'"', CSV_NO_ESCAPE, None);
        assert_eq!(
            thrown(r.map(|_| Value::Null)),
            err("Exception", "no string")
        );
    }
}
