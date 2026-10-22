//! Port of `ext/json/json_encoder.c`.

use std::rc::Rc;

use php_runtime::class::{EnumBacking, PropPurpose};
use php_runtime::object::PhpObject;
use php_runtime::value::convert::object_property_table;
use php_runtime::value::float_fmt;
use php_runtime::value::numeric::{self, Numeric};
use php_runtime::{ArrayKey, ObjRef, PResult, PhpArray, Value, Vm};

use super::error::ErrorCode;
use super::{
    FORCE_OBJECT, HEX_AMP, HEX_APOS, HEX_QUOT, HEX_TAG, INVALID_UTF8_IGNORE,
    INVALID_UTF8_SUBSTITUTE, JsonGlobals, NUMERIC_CHECK, PARTIAL_OUTPUT_ON_ERROR,
    PRESERVE_ZERO_FRACTION, PRETTY_PRINT, UNESCAPED_LINE_TERMINATORS, UNESCAPED_SLASHES,
    UNESCAPED_UNICODE,
};

/// How much native stack nested containers may use before the encoder gives
/// up with `JSON_ERROR_DEPTH` (php-src's `php_json_check_stack_limit()`).
/// A level of nesting costs about 1.5 KiB in debug builds (far less with
/// optimisations), so the default `$depth` of 512 always fits.
const STACK_BUDGET: usize = 1024 * 1024;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// `php_json_encoder`.
pub struct Encoder<'a> {
    vm: &'a mut Vm,
    pub buf: Vec<u8>,
    depth: i32,
    max_depth: i32,
    pub error: ErrorCode,
    stack_base: usize,
}

/// `SUCCESS` (`true`) or `FAILURE` (`false`); thrown exceptions are the `Err` side.
type EncResult = PResult<bool>;

/// Result of `php_next_utf8_char()` (`ext/standard/html.c`, UTF-8 case).
struct Utf8Char {
    code: u32,
    /// Bytes consumed (also for failures: the invalid prefix).
    len: usize,
    valid: bool,
}

/// `get_next_char_utf8()`; `s` is non-empty.
fn next_utf8_char(s: &[u8]) -> Utf8Char {
    let fail = |len: usize| Utf8Char {
        code: 0,
        len,
        valid: false,
    };
    let lead = |c: u8| c < 0x80 || (0xC2..=0xF4).contains(&c);
    let trail = |c: u8| (0x80..=0xBF).contains(&c);
    let avail = s.len();
    let c = s[0];
    if c < 0x80 {
        Utf8Char {
            code: c as u32,
            len: 1,
            valid: true,
        }
    } else if c < 0xC2 {
        fail(1)
    } else if c < 0xE0 {
        if avail < 2 {
            return fail(1);
        }
        if !trail(s[1]) {
            return fail(if lead(s[1]) { 1 } else { 2 });
        }
        let code = ((c as u32 & 0x1F) << 6) | (s[1] as u32 & 0x3F);
        if code < 0x80 {
            return fail(2);
        }
        Utf8Char {
            code,
            len: 2,
            valid: true,
        }
    } else if c < 0xF0 {
        if avail < 3 || !trail(s[1]) || !trail(s[2]) {
            return if avail < 2 || lead(s[1]) {
                fail(1)
            } else if avail < 3 || lead(s[2]) {
                fail(2)
            } else {
                fail(3)
            };
        }
        let code = ((c as u32 & 0x0F) << 12) | ((s[1] as u32 & 0x3F) << 6) | (s[2] as u32 & 0x3F);
        if code < 0x800 || (0xD800..=0xDFFF).contains(&code) {
            return fail(3);
        }
        Utf8Char {
            code,
            len: 3,
            valid: true,
        }
    } else if c < 0xF5 {
        if avail < 4 || !trail(s[1]) || !trail(s[2]) || !trail(s[3]) {
            return if avail < 2 || lead(s[1]) {
                fail(1)
            } else if avail < 3 || lead(s[2]) {
                fail(2)
            } else if avail < 4 || lead(s[3]) {
                fail(3)
            } else {
                fail(4)
            };
        }
        let code = ((c as u32 & 0x07) << 18)
            | ((s[1] as u32 & 0x3F) << 12)
            | ((s[2] as u32 & 0x3F) << 6)
            | (s[3] as u32 & 0x3F);
        if !(0x10000..=0x10FFFF).contains(&code) {
            return fail(4);
        }
        Utf8Char {
            code,
            len: 4,
            valid: true,
        }
    } else {
        fail(1)
    }
}

/// The `charmap` of `php_json_escape_string()`: bytes that need more than a copy.
fn needs_escape(c: u8) -> bool {
    !(0x20..0x80).contains(&c) || matches!(c, b'"' | b'&' | b'\'' | b'/' | b'<' | b'>' | b'\\')
}

fn push_u_escape(buf: &mut Vec<u8>, us: u32) {
    buf.extend_from_slice(b"\\u");
    for shift in [12, 8, 4, 0] {
        buf.push(HEX_DIGITS[((us >> shift) & 0xF) as usize]);
    }
}

impl<'a> Encoder<'a> {
    pub fn new(vm: &'a mut Vm, max_depth: i32) -> Encoder<'a> {
        let marker = 0u8;
        Encoder {
            vm,
            buf: Vec::new(),
            depth: 0,
            max_depth,
            error: ErrorCode::None,
            stack_base: std::ptr::addr_of!(marker) as usize,
        }
    }

    fn stack_overflowed(&self) -> bool {
        let marker = 0u8;
        let here = std::ptr::addr_of!(marker) as usize;
        self.stack_base.abs_diff(here) > STACK_BUDGET
    }

    fn pretty_char(&mut self, options: i32, c: u8) {
        if options & PRETTY_PRINT != 0 {
            self.buf.push(c);
        }
    }

    fn pretty_indent(&mut self, options: i32) {
        if options & PRETTY_PRINT != 0 {
            for _ in 0..self.depth {
                self.buf.extend_from_slice(b"    ");
            }
        }
    }

    /// `php_json_encode_double()`.
    fn encode_double(&mut self, d: f64, options: i32) {
        let num = float_fmt::gcvt(d, self.vm.serialize_precision as i32, b'e');
        self.buf.extend_from_slice(num.as_bytes());
        if options & PRESERVE_ZERO_FRACTION != 0 && !num.contains('.') {
            self.buf.extend_from_slice(b".0");
        }
    }

    /// `php_json_encode_zval()`.
    pub fn encode_zval(&mut self, val: &Value, options: i32) -> EncResult {
        match val {
            Value::Ref(r) => {
                let inner = r.get();
                self.encode_zval(&inner, options)
            }
            Value::Null => {
                self.buf.extend_from_slice(b"null");
                Ok(true)
            }
            Value::Bool(true) => {
                self.buf.extend_from_slice(b"true");
                Ok(true)
            }
            Value::Bool(false) => {
                self.buf.extend_from_slice(b"false");
                Ok(true)
            }
            Value::Int(i) => {
                self.buf.extend_from_slice(i.to_string().as_bytes());
                Ok(true)
            }
            Value::Float(d) => {
                if d.is_finite() {
                    self.encode_double(*d, options);
                } else {
                    self.error = ErrorCode::InfOrNan;
                    self.buf.push(b'0');
                }
                Ok(true)
            }
            Value::String(s) => Ok(self.escape_string(s.as_bytes(), options)),
            Value::Object(o) => {
                if o.class().instance_of_name(b"JsonSerializable") {
                    return self.encode_serializable_object(o, options);
                }
                if o.class().is_enum() {
                    return self.encode_serializable_enum(o, options);
                }
                self.encode_array(val, options)
            }
            Value::Array(_) => self.encode_array(val, options),
            Value::Undef | Value::Resource(_) => {
                self.error = ErrorCode::UnsupportedType;
                if options & PARTIAL_OUTPUT_ON_ERROR != 0 {
                    self.buf.extend_from_slice(b"null");
                }
                Ok(false)
            }
        }
    }

    /// `php_json_encode_serializable_enum()`.
    fn encode_serializable_enum(&mut self, obj: &ObjRef, options: i32) -> EncResult {
        let backed = matches!(
            obj.class().enum_backing,
            Some(EnumBacking::Int | EnumBacking::String)
        );
        if !backed {
            self.error = ErrorCode::NonBackedEnum;
            self.buf.push(b'0');
            return Ok(false);
        }
        let value = obj.get_prop(b"value").unwrap_or(Value::Null);
        self.encode_zval(&value, options)
    }

    /// `php_json_encode_serializable_object()`.
    fn encode_serializable_object(&mut self, obj: &ObjRef, options: i32) -> EncResult {
        let id = obj.id();
        if self.vm.ext_data::<JsonGlobals>().serializing.contains(&id) {
            self.error = ErrorCode::Recursion;
            if options & PARTIAL_OUTPUT_ON_ERROR != 0 {
                self.buf.extend_from_slice(b"null");
            }
            return Ok(false);
        }
        self.vm.ext_data::<JsonGlobals>().serializing.push(id);
        let retval = match self.vm.call_method(obj, b"jsonSerialize", Vec::new()) {
            Ok(v) => v.into_unref(),
            Err(e) => {
                // The exception propagates out of json_encode().
                self.release_serializing(id);
                return Err(e);
            }
        };
        let returned_this = matches!(&retval, Value::Object(o) if ObjRef::ptr_eq(o, obj));
        if returned_this {
            // `return $this;`: straight to encoding its properties.
            self.release_serializing(id);
            self.encode_array(&retval, options)
        } else {
            let r = self.encode_zval(&retval, options);
            self.release_serializing(id);
            r
        }
    }

    /// `ZEND_GUARD_UNPROTECT_RECURSION(guard, JSON)`.
    fn release_serializing(&mut self, id: u32) {
        let g = self.vm.ext_data::<JsonGlobals>();
        if let Some(pos) = g.serializing.iter().rposition(|&x| x == id) {
            g.serializing.remove(pos);
        }
    }

    /// The property table of an object for JSON (`zend_get_properties_for(..., ZEND_PROP_PURPOSE_JSON)`).
    fn object_properties(&mut self, obj: &ObjRef) -> PResult<PhpArray> {
        let table = match obj.class().handlers.get_properties_for {
            Some(f) => f(self.vm, obj, PropPurpose::Json)?,
            None => None,
        };
        Ok(table.unwrap_or_else(|| object_property_table(obj)))
    }

    /// `php_json_encode_array()` (arrays and plain objects).
    fn encode_array(&mut self, val: &Value, options: i32) -> EncResult {
        let partial = options & PARTIAL_OUTPUT_ON_ERROR != 0;
        if self.stack_overflowed() {
            self.error = ErrorCode::Depth;
            if partial {
                self.buf.extend_from_slice(b"null");
            }
            return Ok(false);
        }

        let mut encode_as_object = options & FORCE_OBJECT != 0;
        let is_object = matches!(val, Value::Object(_));
        // Identity used for recursion detection (`GC_PROTECT_RECURSION`).
        let identity: usize;
        let table: Rc<PhpArray>;
        match val {
            Value::Array(a) => {
                identity = Rc::as_ptr(a) as usize;
                encode_as_object = encode_as_object || !a.is_list();
                table = a.clone();
            }
            Value::Object(o) => {
                identity = o.obj() as *const PhpObject as usize;
                encode_as_object = true;
                table = Rc::new(self.object_properties(o)?);
            }
            _ => unreachable!("encode_array takes arrays and objects"),
        }

        if self
            .vm
            .ext_data::<JsonGlobals>()
            .in_progress
            .contains(&identity)
        {
            self.error = ErrorCode::Recursion;
            self.buf.extend_from_slice(b"null");
            return Ok(false);
        }
        self.vm.ext_data::<JsonGlobals>().in_progress.push(identity);

        let result = self.encode_container(&table, is_object, encode_as_object, options);

        let g = self.vm.ext_data::<JsonGlobals>();
        if let Some(pos) = g.in_progress.iter().rposition(|&x| x == identity) {
            g.in_progress.remove(pos);
        }
        result
    }

    /// The part of `php_json_encode_array()` between protecting and
    /// unprotecting against recursion: opening bracket, members, depth check,
    /// closing bracket.
    fn encode_container(
        &mut self,
        table: &PhpArray,
        is_object: bool,
        encode_as_object: bool,
        options: i32,
    ) -> EncResult {
        let partial = options & PARTIAL_OUTPUT_ON_ERROR != 0;
        self.buf.push(if encode_as_object { b'{' } else { b'[' });
        self.depth += 1;

        let mut empty = true;
        for (key, data) in table.iter() {
            if !encode_as_object {
                self.pretty_char(options, b'\n');
                self.pretty_indent(options);
            } else {
                match &key {
                    ArrayKey::Str(k) => {
                        if is_object && !k.is_empty() && k.as_bytes()[0] == 0 {
                            // Skip protected and private members.
                            continue;
                        }
                        self.pretty_char(options, b'\n');
                        self.pretty_indent(options);
                        if !self.escape_string(k.as_bytes(), options & !NUMERIC_CHECK) && partial {
                            // Replace the "null" of the failed key by an empty string.
                            let new_len = self.buf.len().saturating_sub(4);
                            self.buf.truncate(new_len);
                            self.buf.extend_from_slice(b"\"\"");
                        }
                    }
                    ArrayKey::Int(i) => {
                        self.pretty_char(options, b'\n');
                        self.pretty_indent(options);
                        self.buf.push(b'"');
                        self.buf.extend_from_slice(i.to_string().as_bytes());
                        self.buf.push(b'"');
                    }
                }
                self.buf.push(b':');
                self.pretty_char(options, b' ');
            }

            if !self.encode_zval(data, options)? && !partial {
                return Ok(false);
            }
            self.buf.push(b',');
            empty = false;
        }
        if !empty {
            // Drop the trailing comma.
            self.buf.pop();
        }

        if self.depth > self.max_depth {
            self.error = ErrorCode::Depth;
            if !partial {
                return Ok(false);
            }
        }
        self.depth -= 1;

        // Only keep the closing bracket on the same line for empty arrays/objects.
        if !empty {
            self.pretty_char(options, b'\n');
            self.pretty_indent(options);
        }
        self.buf.push(if encode_as_object { b'}' } else { b']' });
        Ok(true)
    }

    /// `php_json_escape_string()`: `false` is `FAILURE`.
    pub fn escape_string(&mut self, s: &[u8], options: i32) -> bool {
        if s.is_empty() {
            self.buf.extend_from_slice(b"\"\"");
            return true;
        }

        if options & NUMERIC_CHECK != 0 {
            match numeric::is_numeric_string(s, false) {
                Some(Numeric::Int(i)) => {
                    self.buf.extend_from_slice(i.to_string().as_bytes());
                    return true;
                }
                Some(Numeric::Float(d)) if d.is_finite() => {
                    self.encode_double(d, options);
                    return true;
                }
                _ => {}
            }
        }

        let checkpoint = self.buf.len();
        self.buf.reserve(s.len() + 2);
        self.buf.push(b'"');

        let mut pos = 0;
        while pos < s.len() {
            let c = s[pos];
            if !needs_escape(c) {
                let start = pos;
                while pos < s.len() && !needs_escape(s[pos]) {
                    pos += 1;
                }
                self.buf.extend_from_slice(&s[start..pos]);
                continue;
            }
            if c >= 0x80 {
                let ch = next_utf8_char(&s[pos..]);
                if !ch.valid {
                    if options & INVALID_UTF8_IGNORE != 0 {
                        // Ignore the invalid UTF-8 character.
                    } else if options & INVALID_UTF8_SUBSTITUTE != 0 {
                        // U+FFFD REPLACEMENT CHARACTER.
                        if options & UNESCAPED_UNICODE != 0 {
                            self.buf.extend_from_slice(b"\xef\xbf\xbd");
                        } else {
                            self.buf.extend_from_slice(b"\\ufffd");
                        }
                    } else {
                        self.buf.truncate(checkpoint);
                        self.error = ErrorCode::Utf8;
                        if options & PARTIAL_OUTPUT_ON_ERROR != 0 {
                            self.buf.extend_from_slice(b"null");
                        }
                        return false;
                    }
                } else if options & UNESCAPED_UNICODE != 0
                    && (options & UNESCAPED_LINE_TERMINATORS != 0
                        || ch.code < 0x2028
                        || ch.code > 0x2029)
                {
                    self.buf.extend_from_slice(&s[pos..pos + ch.len]);
                } else {
                    let mut us = ch.code;
                    if us >= 0x10000 {
                        // UTF-16 surrogate pair.
                        us -= 0x10000;
                        let high = (us >> 10) | 0xD800;
                        let low = (us & 0x3FF) | 0xDC00;
                        push_u_escape(&mut self.buf, high);
                        us = low;
                    }
                    push_u_escape(&mut self.buf, us);
                }
                pos += ch.len;
            } else {
                pos += 1;
                match c {
                    b'"' => {
                        if options & HEX_QUOT != 0 {
                            self.buf.extend_from_slice(b"\\u0022");
                        } else {
                            self.buf.extend_from_slice(b"\\\"");
                        }
                    }
                    b'\\' => self.buf.extend_from_slice(b"\\\\"),
                    b'/' => {
                        if options & UNESCAPED_SLASHES != 0 {
                            self.buf.push(b'/');
                        } else {
                            self.buf.extend_from_slice(b"\\/");
                        }
                    }
                    0x08 => self.buf.extend_from_slice(b"\\b"),
                    0x0C => self.buf.extend_from_slice(b"\\f"),
                    b'\n' => self.buf.extend_from_slice(b"\\n"),
                    b'\r' => self.buf.extend_from_slice(b"\\r"),
                    b'\t' => self.buf.extend_from_slice(b"\\t"),
                    b'<' => {
                        if options & HEX_TAG != 0 {
                            self.buf.extend_from_slice(b"\\u003C");
                        } else {
                            self.buf.push(b'<');
                        }
                    }
                    b'>' => {
                        if options & HEX_TAG != 0 {
                            self.buf.extend_from_slice(b"\\u003E");
                        } else {
                            self.buf.push(b'>');
                        }
                    }
                    b'&' => {
                        if options & HEX_AMP != 0 {
                            self.buf.extend_from_slice(b"\\u0026");
                        } else {
                            self.buf.push(b'&');
                        }
                    }
                    b'\'' => {
                        if options & HEX_APOS != 0 {
                            self.buf.extend_from_slice(b"\\u0027");
                        } else {
                            self.buf.push(b'\'');
                        }
                    }
                    _ => {
                        debug_assert!(c < b' ');
                        self.buf.extend_from_slice(b"\\u00");
                        self.buf.push(HEX_DIGITS[(c >> 4) as usize]);
                        self.buf.push(HEX_DIGITS[(c & 0xF) as usize]);
                    }
                }
            }
        }
        self.buf.push(b'"');
        true
    }
}
