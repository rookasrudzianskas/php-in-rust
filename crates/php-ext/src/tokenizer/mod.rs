//! ext/tokenizer (port of `ext/tokenizer/tokenizer.c`).
//!
//! The scanner and parser are `php-syntax`'s [`php_syntax::tokenize`]; this
//! module turns its tokens into `token_get_all()` arrays or `PhpToken`
//! objects and registers `token_get_all()`, `token_name()`, `PhpToken` and
//! the `T_*` / `TOKEN_PARSE` constants.

pub mod data;
pub mod phptoken;

use php_runtime::errors::level::{E_COMPILE_WARNING, E_DEPRECATED, E_WARNING};
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};
use php_syntax::{
    Diagnostic, DiagnosticLevel, ParseError, ParseErrorKind, ParseOptions, Token, TokenizeFlags,
};

/// `TOKEN_PARSE` (`1 << 0`).
pub const TOKEN_PARSE: i64 = 1;

/// Creates the `ParseError`/`CompileError` php-src throws from the parser.
/// Like `zend_default_exception_new()` does for these classes, the exception
/// reports the compiled file name (empty for strings) and the error line.
fn parse_error_exception(vm: &mut Vm, e: &ParseError) -> Flow {
    let class = match e.kind {
        ParseErrorKind::Parse => "ParseError",
        ParseErrorKind::Compile => "CompileError",
    };
    let obj = vm.create_exception(class, e.message.clone(), 0);
    obj.set_prop(b"file", Value::empty_string());
    obj.set_prop(b"line", Value::Int(i64::from(e.line)));
    Flow::Exception(obj)
}

/// Raises the deprecations and warnings the lexer and parser emitted, at the
/// compiled file name (empty) and their line.
fn emit_diagnostics(vm: &mut Vm, diagnostics: &[Diagnostic]) -> PResult<()> {
    for d in diagnostics {
        let level = match d.level {
            DiagnosticLevel::Deprecated => E_DEPRECATED,
            DiagnosticLevel::CompileWarning => E_COMPILE_WARNING,
            DiagnosticLevel::Warning => E_WARNING,
        };
        vm.zend_error_at(
            level,
            PhpStr::new(),
            d.line,
            PhpStr::from(d.message.as_str()),
        )?;
    }
    Ok(())
}

/// `add_token()`: one token as an array, a single-character string or an object.
fn add_token(out: &mut PhpArray, token: &Token, source: &[u8], class: Option<&ClassRef>) {
    let id = i64::from(token.kind.id());
    let text = PhpStr::from_bytes(token.text(source));
    let line = i64::from(token.line);
    let value = match class {
        Some(class) => {
            // `zend_objects_new()`: no `create_object` handler and no constructor;
            // properties declared by subclasses get their default values.
            let obj = ObjRef::new(class.clone());
            phptoken::init_token(&obj, id, text, line, i64::from(token.start));
            Value::Object(obj)
        }
        None if id >= 256 => Value::array(PhpArray::from_values(vec![
            Value::Int(id),
            Value::String(text),
            Value::Int(line),
        ])),
        None => Value::String(text),
    };
    let _ = out.append(value);
}

/// `tokenize_common()`: `token_class` is `None` for `token_get_all()`.
pub fn tokenize_common(
    vm: &mut Vm,
    source: &PhpStr,
    flags: i64,
    token_class: Option<&ClassRef>,
) -> PResult<Value> {
    let parse = flags & TOKEN_PARSE != 0;
    let tokens = match php_syntax::tokenize(source, TokenizeFlags { parse }) {
        Ok(tokens) => tokens,
        Err(e) => {
            emit_diagnostics(vm, &e.diagnostics)?;
            return Err(parse_error_exception(vm, &e));
        }
    };
    if parse {
        // `php_syntax::tokenize` does not report the diagnostics of a successful
        // parse (e.g. non-canonical casts), so they are collected from a second
        // run of the parser.
        let opts = ParseOptions {
            no_resolve: true,
            ..ParseOptions::default()
        };
        if let Ok(program) = php_syntax::parse_with(source, "", &opts) {
            emit_diagnostics(vm, &program.diagnostics)?;
        }
    }

    let mut out = PhpArray::with_capacity(tokens.len());
    for token in &tokens {
        add_token(&mut out, token, source, token_class);
    }
    Ok(Value::array(out))
}

/// `token_get_all()`.
fn token_get_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let source = args.str(0).clone();
    let flags = args.int(1);
    tokenize_common(vm, &source, flags, None)
}

/// `token_name()`.
fn token_name(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    // `get_token_type_name(int)` receives the `zend_long` converted to `int`.
    let name = data::get_token_type_name(args.int(0) as i32).unwrap_or("UNKNOWN");
    Ok(Value::str(name))
}

static TOKEN_GET_ALL: NativeFunction = NativeFunction {
    sig: "token_get_all(string $code, int $flags = 0): array",
    handler: token_get_all,
};
static TOKEN_NAME: NativeFunction = NativeFunction {
    sig: "token_name(int $id): string",
    handler: token_name,
};

pub fn register(r: &mut Registry) {
    // register_tokenizer_data_symbols()
    for name in data::TOKEN_CONSTANTS {
        let kind = data::kind_by_name(name)
            .unwrap_or_else(|| panic!("php-syntax has no token named {name}"));
        r.constant(name, i64::from(kind.id()));
    }
    // register_tokenizer_symbols()
    r.constant("TOKEN_PARSE", TOKEN_PARSE);
    r.functions(&[&TOKEN_GET_ALL, &TOKEN_NAME]);
    phptoken::register(r);
}

#[cfg(test)]
mod tests;
