//! ext/ctype (port of `ext/ctype/ctype.c`).
//!
//! The classification functions use the "C" locale: only ASCII bytes are ever
//! members of a character class (`setlocale()` is not modelled).

use php_runtime::{Args, NativeFunction, PResult, Registry, Value, Vm};

/// The `ctype_*` predicates for one byte (C locale, `<ctype.h>` semantics).
mod class {
    pub fn alnum(c: u8) -> bool {
        c.is_ascii_alphanumeric()
    }
    pub fn alpha(c: u8) -> bool {
        c.is_ascii_alphabetic()
    }
    pub fn cntrl(c: u8) -> bool {
        c.is_ascii_control()
    }
    pub fn digit(c: u8) -> bool {
        c.is_ascii_digit()
    }
    pub fn lower(c: u8) -> bool {
        c.is_ascii_lowercase()
    }
    pub fn graph(c: u8) -> bool {
        c.is_ascii_graphic()
    }
    pub fn print(c: u8) -> bool {
        c.is_ascii_graphic() || c == b' '
    }
    pub fn punct(c: u8) -> bool {
        c.is_ascii_punctuation()
    }
    pub fn space(c: u8) -> bool {
        // isspace(): space, \t, \n, \v, \f, \r (u8::is_ascii_whitespace omits \v).
        matches!(c, b' ' | b'\t'..=b'\r')
    }
    pub fn upper(c: u8) -> bool {
        c.is_ascii_uppercase()
    }
    pub fn xdigit(c: u8) -> bool {
        c.is_ascii_hexdigit()
    }
}

/// `ctype_fallback()`: non-string arguments are deprecated; integers are
/// interpreted as character codes.
fn ctype_fallback(
    vm: &mut Vm,
    c: &Value,
    iswhat: fn(u8) -> bool,
    allow_digits: bool,
    allow_minus: bool,
) -> PResult<Value> {
    vm.deprecated(format!(
        "Argument of type {} will be interpreted as string in the future",
        c.type_name()
    ))?;
    Ok(Value::Bool(match c {
        Value::Int(n) => match *n {
            0..=255 => iswhat(*n as u8),
            -128..=-1 => iswhat((*n + 256) as u8),
            n if n >= 0 => allow_digits,
            _ => allow_minus,
        },
        _ => false,
    }))
}

/// `ctype_impl()`.
fn ctype_impl(
    vm: &mut Vm,
    args: &Args,
    iswhat: fn(u8) -> bool,
    allow_digits: bool,
    allow_minus: bool,
) -> PResult<Value> {
    let c = args.get(0).unref();
    if let Value::String(s) = &c {
        return Ok(Value::Bool(!s.is_empty() && s.iter().all(|&b| iswhat(b))));
    }
    ctype_fallback(vm, &c, iswhat, allow_digits, allow_minus)
}

macro_rules! ctype_function {
    ($handler:ident, $static_name:ident, $sig:literal, $what:path, $digits:expr, $minus:expr) => {
        fn $handler(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
            ctype_impl(vm, args, $what, $digits, $minus)
        }
        static $static_name: NativeFunction = NativeFunction {
            sig: $sig,
            handler: $handler,
        };
    };
}

ctype_function!(
    ctype_alnum,
    CTYPE_ALNUM,
    "ctype_alnum(mixed $text): bool",
    class::alnum,
    true,
    false
);
ctype_function!(
    ctype_alpha,
    CTYPE_ALPHA,
    "ctype_alpha(mixed $text): bool",
    class::alpha,
    false,
    false
);
ctype_function!(
    ctype_cntrl,
    CTYPE_CNTRL,
    "ctype_cntrl(mixed $text): bool",
    class::cntrl,
    false,
    false
);
ctype_function!(
    ctype_digit,
    CTYPE_DIGIT,
    "ctype_digit(mixed $text): bool",
    class::digit,
    true,
    false
);
ctype_function!(
    ctype_lower,
    CTYPE_LOWER,
    "ctype_lower(mixed $text): bool",
    class::lower,
    false,
    false
);
ctype_function!(
    ctype_graph,
    CTYPE_GRAPH,
    "ctype_graph(mixed $text): bool",
    class::graph,
    true,
    true
);
ctype_function!(
    ctype_print,
    CTYPE_PRINT,
    "ctype_print(mixed $text): bool",
    class::print,
    true,
    true
);
ctype_function!(
    ctype_punct,
    CTYPE_PUNCT,
    "ctype_punct(mixed $text): bool",
    class::punct,
    false,
    false
);
ctype_function!(
    ctype_space,
    CTYPE_SPACE,
    "ctype_space(mixed $text): bool",
    class::space,
    false,
    false
);
ctype_function!(
    ctype_upper,
    CTYPE_UPPER,
    "ctype_upper(mixed $text): bool",
    class::upper,
    false,
    false
);
ctype_function!(
    ctype_xdigit,
    CTYPE_XDIGIT,
    "ctype_xdigit(mixed $text): bool",
    class::xdigit,
    true,
    false
);

pub fn register(r: &mut Registry) {
    r.functions(&[
        &CTYPE_ALNUM,
        &CTYPE_ALPHA,
        &CTYPE_CNTRL,
        &CTYPE_DIGIT,
        &CTYPE_LOWER,
        &CTYPE_GRAPH,
        &CTYPE_PRINT,
        &CTYPE_PUNCT,
        &CTYPE_SPACE,
        &CTYPE_UPPER,
        &CTYPE_XDIGIT,
    ]);
}

#[cfg(test)]
mod tests;
