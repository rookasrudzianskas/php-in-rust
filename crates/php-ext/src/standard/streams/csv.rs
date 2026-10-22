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

