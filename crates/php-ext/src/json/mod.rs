//! ext/json (port of `ext/json/json.c`, `json_encoder.c`, `json_scanner.re`
//! and `json_parser.y`).
//!
//! * [`encoder`] — `json_encode()`: `php_json_encode_zval()` and
//!   `php_json_escape_string()`.
//! * [`scanner`] / [`parser`] — `json_decode()` / `json_validate()`: the re2c
//!   scanner and the bison grammar.
//! * This module registers the functions, constants, `JsonSerializable` and
//!   `JsonException`, and keeps the per-request state (`JSON_G`).

pub mod encoder;
pub mod error;
pub mod parser;
pub mod scanner;

use php_runtime::class::{ClassEntry, Function, class_flags, member_flags};
use php_runtime::{Args, Flow, NativeFunction, PResult, Registry, Value, Vm};

use error::{ErrorCode, ErrorDetails};

// json_encode() options (`php_json.h`).
pub const HEX_TAG: i32 = 1 << 0;
pub const HEX_AMP: i32 = 1 << 1;
pub const HEX_APOS: i32 = 1 << 2;
pub const HEX_QUOT: i32 = 1 << 3;
pub const FORCE_OBJECT: i32 = 1 << 4;
pub const NUMERIC_CHECK: i32 = 1 << 5;
pub const UNESCAPED_SLASHES: i32 = 1 << 6;
pub const PRETTY_PRINT: i32 = 1 << 7;
pub const UNESCAPED_UNICODE: i32 = 1 << 8;
pub const PARTIAL_OUTPUT_ON_ERROR: i32 = 1 << 9;
pub const PRESERVE_ZERO_FRACTION: i32 = 1 << 10;
pub const UNESCAPED_LINE_TERMINATORS: i32 = 1 << 11;

// json_decode() options.
pub const OBJECT_AS_ARRAY: i32 = 1 << 0;
pub const BIGINT_AS_STRING: i32 = 1 << 1;

// Options shared by json_validate(), json_decode() and json_encode().
pub const INVALID_UTF8_IGNORE: i32 = 1 << 20;
pub const INVALID_UTF8_SUBSTITUTE: i32 = 1 << 21;
pub const THROW_ON_ERROR: i32 = 1 << 22;

/// `PHP_JSON_PARSER_DEFAULT_DEPTH`.
pub const DEFAULT_DEPTH: i64 = 512;

/// Per-VM state (`ZEND_BEGIN_MODULE_GLOBALS(json)` plus the recursion
/// markers php-src keeps on the values themselves).
#[derive(Default)]
pub struct JsonGlobals {
    /// `JSON_G(error_details)`: the outcome of the last call.
    pub error: ErrorDetails,
    /// Arrays and objects being encoded (`GC_PROTECT_RECURSION`), shared by
    /// nested `json_encode()` calls made from `jsonSerialize()`.
    pub(crate) in_progress: Vec<usize>,
    /// Objects inside `jsonSerialize()` (`ZEND_GUARD_PROTECT_RECURSION(guard, JSON)`).
    pub(crate) serializing: Vec<u32>,
}

fn globals(vm: &mut Vm) -> &mut JsonGlobals {
    vm.ext_data::<JsonGlobals>()
}

/// `zend_throw_exception(php_json_exception_ce, msg, code)`.
fn throw_json_exception(vm: &mut Vm, msg: String, code: ErrorCode) -> Flow {
    Flow::Exception(vm.create_exception("JsonException", msg, code as i64))
}

/// `php_json_decode_ex()` / `php_json_validate_ex()` argument validation
/// shared by both functions: `$depth` must be in `1..=INT_MAX`.
fn check_depth(vm: &mut Vm, arg_num: usize, depth: i64) -> PResult<()> {
    if depth <= 0 {
        return Err(vm.argument_value_error(arg_num, "must be greater than 0"));
    }
    if depth > i32::MAX as i64 {
        return Err(vm.argument_value_error(arg_num, &format!("must be less than {}", i32::MAX)));
    }
    Ok(())
}

/// `json_encode()`.
fn json_encode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let value = args.get(0).clone();
    let options = args.int(1);
    let depth = args.int(2);

    let mut enc = encoder::Encoder::new(vm, depth as i32);
    enc.encode_zval(&value, options as i32)?;
    let (buf, error) = (std::mem::take(&mut enc.buf), enc.error);

    if options & THROW_ON_ERROR as i64 == 0 || options & PARTIAL_OUTPUT_ON_ERROR as i64 != 0 {
        globals(vm).error = ErrorDetails::new(error);
        if error != ErrorCode::None && options & PARTIAL_OUTPUT_ON_ERROR as i64 == 0 {
            return Ok(Value::Bool(false));
        }
    } else if error != ErrorCode::None {
        return Err(throw_json_exception(vm, error.message().to_string(), error));
    }
    Ok(Value::from(buf))
}

/// `json_decode()`.
fn json_decode(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let json = args.str(0).clone();
    let assoc = args.bool_opt(1);
    let depth = args.int(2);
    let mut options = args.int(3);
    let throw = options & THROW_ON_ERROR as i64 != 0;

    if !throw {
        globals(vm).error = ErrorDetails::default();
    }

    if json.is_empty() {
        if !throw {
            globals(vm).error = ErrorDetails::new(ErrorCode::Syntax);
        } else {
            let msg = ErrorCode::Syntax.message().to_string();
            return Err(throw_json_exception(vm, msg, ErrorCode::Syntax));
        }
        return Ok(Value::Null);
    }

    check_depth(vm, 3, depth)?;

    // For BC reasons, the bool $assoc overrides the OBJECT_AS_ARRAY option bit.
    if let Some(assoc) = assoc {
        if assoc {
            options |= OBJECT_AS_ARRAY as i64;
        } else {
            options &= !(OBJECT_AS_ARRAY as i64);
        }
    }

    let std_class = vm.find_class(b"stdClass");
    match parser::parse(&json, options as i32, depth as i32, std_class, false) {
        Ok(v) => Ok(v),
        Err(details) => {
            if !throw {
                globals(vm).error = details;
            } else {
                let msg = details.message_with_location();
                return Err(throw_json_exception(vm, msg, details.code));
            }
            Ok(Value::Null)
        }
    }
}

/// `json_validate()`.
fn json_validate(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let json = args.str(0).clone();
    let depth = args.int(1);
    let options = args.int(2);

    if options != 0 && options != INVALID_UTF8_IGNORE as i64 {
        return Err(vm.argument_value_error(
            3,
            "must be a valid flag (allowed flags: JSON_INVALID_UTF8_IGNORE)",
        ));
    }

    if json.is_empty() {
        globals(vm).error = ErrorDetails::new(ErrorCode::Syntax);
        return Ok(Value::Bool(false));
    }

    globals(vm).error = ErrorDetails::default();

    check_depth(vm, 2, depth)?;

    match parser::parse(&json, options as i32, depth as i32, None, true) {
        Ok(_) => Ok(Value::Bool(true)),
        Err(details) => {
            globals(vm).error = details;
            Ok(Value::Bool(false))
        }
    }
}

/// `json_last_error()`.
fn json_last_error(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(globals(vm).error.code as i64))
}

/// `json_last_error_msg()`.
fn json_last_error_msg(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::from(globals(vm).error.message_with_location()))
}

/// `JsonSerializable::jsonSerialize()` is abstract; this body is never reached
/// through a well-formed class hierarchy.
fn json_serialize_abstract(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "Error",
        "Cannot call abstract method JsonSerializable::jsonSerialize()",
    ))
}

static JSON_ENCODE: NativeFunction = NativeFunction {
    sig: "json_encode(mixed $value, int $flags = 0, int $depth = 512): string|false",
    handler: json_encode,
};
static JSON_DECODE: NativeFunction = NativeFunction {
    sig: "json_decode(string $json, ?bool $associative = null, int $depth = 512, int $flags = 0): mixed",
    handler: json_decode,
};
static JSON_VALIDATE: NativeFunction = NativeFunction {
    sig: "json_validate(string $json, int $depth = 512, int $flags = 0): bool",
    handler: json_validate,
};
static JSON_LAST_ERROR: NativeFunction = NativeFunction {
    sig: "json_last_error(): int",
    handler: json_last_error,
};
static JSON_LAST_ERROR_MSG: NativeFunction = NativeFunction {
    sig: "json_last_error_msg(): string",
    handler: json_last_error_msg,
};
static JSON_SERIALIZE: NativeFunction = NativeFunction {
    sig: "public function jsonSerialize(): mixed",
    handler: json_serialize_abstract,
};

pub fn register(r: &mut Registry) {
    // Classes first, like MINIT: `JsonSerializable`, `JsonException`.
    let mut serializable = ClassEntry::new("JsonSerializable", class_flags::INTERFACE);
    serializable.add_method(Function::native_method(
        &JSON_SERIALIZE,
        "JsonSerializable",
        member_flags::PUBLIC | member_flags::ABSTRACT,
    ));
    r.class(serializable);

    let exception = r
        .find_class("Exception")
        .expect("Exception is registered by the engine");
    r.class(ClassEntry::new_subclass("JsonException", 0, &exception));

    r.functions(&[
        &JSON_ENCODE,
        &JSON_DECODE,
        &JSON_VALIDATE,
        &JSON_LAST_ERROR,
        &JSON_LAST_ERROR_MSG,
    ]);

    for (name, value) in [
        ("JSON_HEX_TAG", HEX_TAG),
        ("JSON_HEX_AMP", HEX_AMP),
        ("JSON_HEX_APOS", HEX_APOS),
        ("JSON_HEX_QUOT", HEX_QUOT),
        ("JSON_FORCE_OBJECT", FORCE_OBJECT),
        ("JSON_NUMERIC_CHECK", NUMERIC_CHECK),
        ("JSON_UNESCAPED_SLASHES", UNESCAPED_SLASHES),
        ("JSON_PRETTY_PRINT", PRETTY_PRINT),
        ("JSON_UNESCAPED_UNICODE", UNESCAPED_UNICODE),
        ("JSON_PARTIAL_OUTPUT_ON_ERROR", PARTIAL_OUTPUT_ON_ERROR),
        ("JSON_PRESERVE_ZERO_FRACTION", PRESERVE_ZERO_FRACTION),
        (
            "JSON_UNESCAPED_LINE_TERMINATORS",
            UNESCAPED_LINE_TERMINATORS,
        ),
        ("JSON_OBJECT_AS_ARRAY", OBJECT_AS_ARRAY),
        ("JSON_BIGINT_AS_STRING", BIGINT_AS_STRING),
        ("JSON_INVALID_UTF8_IGNORE", INVALID_UTF8_IGNORE),
        ("JSON_INVALID_UTF8_SUBSTITUTE", INVALID_UTF8_SUBSTITUTE),
        ("JSON_THROW_ON_ERROR", THROW_ON_ERROR),
    ] {
        r.constant(name, value as i64);
    }
    for (name, code) in [
        ("JSON_ERROR_NONE", ErrorCode::None),
        ("JSON_ERROR_DEPTH", ErrorCode::Depth),
        ("JSON_ERROR_STATE_MISMATCH", ErrorCode::StateMismatch),
        ("JSON_ERROR_CTRL_CHAR", ErrorCode::CtrlChar),
        ("JSON_ERROR_SYNTAX", ErrorCode::Syntax),
        ("JSON_ERROR_UTF8", ErrorCode::Utf8),
        ("JSON_ERROR_RECURSION", ErrorCode::Recursion),
        ("JSON_ERROR_INF_OR_NAN", ErrorCode::InfOrNan),
        ("JSON_ERROR_UNSUPPORTED_TYPE", ErrorCode::UnsupportedType),
        (
            "JSON_ERROR_INVALID_PROPERTY_NAME",
            ErrorCode::InvalidPropertyName,
        ),
        ("JSON_ERROR_UTF16", ErrorCode::Utf16),
        ("JSON_ERROR_NON_BACKED_ENUM", ErrorCode::NonBackedEnum),
    ] {
        r.constant(name, code as i64);
    }
}

#[cfg(test)]
mod tests;
