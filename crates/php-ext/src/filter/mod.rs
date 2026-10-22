//! ext/filter (port of `ext/filter/filter.c`).
//!
//! * this module — the filter table, constants, ini entries, the input
//!   storage (`IF_G(*_array)`), `php_zval_filter()`, `php_filter_call()`,
//!   `php_filter_array_handler()` and the `filter_*()` functions;
//! * [`logical`] — the validating filters (`logical_filters.c`);
//! * [`sanitize`] — the sanitizing filters (`sanitizing_filters.c`);
//! * [`callback`] — `FILTER_CALLBACK` (`callback_filter.c`).
//!
//! The CLI has no request variables: the `INPUT_*` storages stay
//! uninitialised (as with `php -r`) unless the SAPI installs them with
//! [`set_input_array`].

mod callback;
mod logical;
mod sanitize;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::rc::Rc;

use php_regex::Regex;
use php_runtime::ini::{IniDef, modifiable};
use php_runtime::value::numeric::c_str;
use php_runtime::{
    Args, ArrayKey, ClassEntry, Flow, NativeFunction, PResult, PhpArray, PhpArrayRef, PhpRef,
    PhpStr, Registry, Value, Vm,
};

// `main/php_variables.h`
pub const PARSE_POST: i64 = 0;
pub const PARSE_GET: i64 = 1;
pub const PARSE_COOKIE: i64 = 2;
pub const PARSE_STRING: i64 = 3;
pub const PARSE_ENV: i64 = 4;
pub const PARSE_SERVER: i64 = 5;

// `filter_private.h`
pub const FILTER_FLAG_NONE: i64 = 0x0000;

pub const FILTER_REQUIRE_ARRAY: i64 = 0x1000000;
pub const FILTER_REQUIRE_SCALAR: i64 = 0x2000000;

pub const FILTER_FORCE_ARRAY: i64 = 0x4000000;
pub const FILTER_NULL_ON_FAILURE: i64 = 0x8000000;
pub const FILTER_THROW_ON_FAILURE: i64 = 0x10000000;

pub const FILTER_FLAG_ALLOW_OCTAL: i64 = 0x0001;
pub const FILTER_FLAG_ALLOW_HEX: i64 = 0x0002;

pub const FILTER_FLAG_STRIP_LOW: i64 = 0x0004;
pub const FILTER_FLAG_STRIP_HIGH: i64 = 0x0008;
pub const FILTER_FLAG_ENCODE_LOW: i64 = 0x0010;
pub const FILTER_FLAG_ENCODE_HIGH: i64 = 0x0020;
pub const FILTER_FLAG_ENCODE_AMP: i64 = 0x0040;
pub const FILTER_FLAG_NO_ENCODE_QUOTES: i64 = 0x0080;
pub const FILTER_FLAG_EMPTY_STRING_NULL: i64 = 0x0100;
pub const FILTER_FLAG_STRIP_BACKTICK: i64 = 0x0200;

pub const FILTER_FLAG_ALLOW_FRACTION: i64 = 0x1000;
pub const FILTER_FLAG_ALLOW_THOUSAND: i64 = 0x2000;
pub const FILTER_FLAG_ALLOW_SCIENTIFIC: i64 = 0x4000;

pub const FILTER_FLAG_SCHEME_REQUIRED: i64 = 0x010000;
pub const FILTER_FLAG_HOST_REQUIRED: i64 = 0x020000;
pub const FILTER_FLAG_PATH_REQUIRED: i64 = 0x040000;
pub const FILTER_FLAG_QUERY_REQUIRED: i64 = 0x080000;

pub const FILTER_FLAG_IPV4: i64 = 0x00100000;
pub const FILTER_FLAG_IPV6: i64 = 0x00200000;
pub const FILTER_FLAG_NO_RES_RANGE: i64 = 0x00400000;
pub const FILTER_FLAG_NO_PRIV_RANGE: i64 = 0x00800000;
pub const FILTER_FLAG_GLOBAL_RANGE: i64 = 0x20000000;

pub const FILTER_FLAG_HOSTNAME: i64 = 0x100000;

pub const FILTER_FLAG_EMAIL_UNICODE: i64 = 0x100000;

pub const FILTER_VALIDATE_INT: i64 = 0x0101;
pub const FILTER_VALIDATE_BOOL: i64 = 0x0102;
pub const FILTER_VALIDATE_FLOAT: i64 = 0x0103;

pub const FILTER_VALIDATE_REGEXP: i64 = 0x0110;
pub const FILTER_VALIDATE_URL: i64 = 0x0111;
pub const FILTER_VALIDATE_EMAIL: i64 = 0x0112;
pub const FILTER_VALIDATE_IP: i64 = 0x0113;
pub const FILTER_VALIDATE_MAC: i64 = 0x0114;
pub const FILTER_VALIDATE_DOMAIN: i64 = 0x0115;
pub const FILTER_VALIDATE_LAST: i64 = 0x0115;

pub const FILTER_VALIDATE_ALL: i64 = 0x0100;

pub const FILTER_DEFAULT: i64 = 0x0204;
pub const FILTER_UNSAFE_RAW: i64 = 0x0204;

pub const FILTER_SANITIZE_STRING: i64 = 0x0201;
pub const FILTER_SANITIZE_ENCODED: i64 = 0x0202;
pub const FILTER_SANITIZE_SPECIAL_CHARS: i64 = 0x0203;
pub const FILTER_SANITIZE_EMAIL: i64 = 0x0205;
pub const FILTER_SANITIZE_URL: i64 = 0x0206;
pub const FILTER_SANITIZE_NUMBER_INT: i64 = 0x0207;
pub const FILTER_SANITIZE_NUMBER_FLOAT: i64 = 0x0208;
pub const FILTER_SANITIZE_FULL_SPECIAL_CHARS: i64 = 0x020a;
pub const FILTER_SANITIZE_ADD_SLASHES: i64 = 0x020b;
pub const FILTER_SANITIZE_LAST: i64 = 0x020b;

pub const FILTER_SANITIZE_ALL: i64 = 0x0200;

pub const FILTER_CALLBACK: i64 = 0x0400;

pub const FILTER_EXCEPTION: &str = "Filter\\FilterException";
pub const FILTER_FAILED_EXCEPTION: &str = "Filter\\FilterFailedException";

/// `PHP_FILTER_ID_EXISTS(id)`.
fn filter_id_exists(id: i64) -> bool {
    (FILTER_SANITIZE_ALL..=FILTER_SANITIZE_LAST).contains(&id)
        || (FILTER_VALIDATE_ALL..=FILTER_VALIDATE_LAST).contains(&id)
        || id == FILTER_CALLBACK
}

/// The `zend_result` of a filter handler. `Failure` only ever means "the
/// validation failed and `FILTER_THROW_ON_FAILURE` asks for an exception"
/// (`RETURN_VALIDATION_FAILED` in `filter_private.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Success,
    Failure,
}

/// A filter handler (`PHP_INPUT_FILTER_PARAM_DECL`): `value` is a string on
/// entry and holds the result on return; `options` is the `"options"` entry
/// of the filter arguments (an array, or anything for `FILTER_CALLBACK`).
type FilterFn = fn(&mut Vm, &mut Value, i64, Option<&Value>) -> PResult<Outcome>;

/// `filter_list_entry`.
struct FilterEntry {
    name: &'static str,
    id: i64,
    function: FilterFn,
}

const fn entry(name: &'static str, id: i64, function: FilterFn) -> FilterEntry {
    FilterEntry { name, id, function }
}

/// `filter_list[]`, in php-src order (`filter_list()` returns the names).
static FILTER_LIST: [FilterEntry; 21] = [
    entry("int", FILTER_VALIDATE_INT, logical::php_filter_int),
    entry("boolean", FILTER_VALIDATE_BOOL, logical::php_filter_boolean),
    entry("float", FILTER_VALIDATE_FLOAT, logical::php_filter_float),
    entry(
        "validate_regexp",
        FILTER_VALIDATE_REGEXP,
        logical::php_filter_validate_regexp,
    ),
    entry(
        "validate_domain",
        FILTER_VALIDATE_DOMAIN,
        logical::php_filter_validate_domain,
    ),
    entry(
        "validate_url",
        FILTER_VALIDATE_URL,
        logical::php_filter_validate_url,
    ),
    entry(
        "validate_email",
        FILTER_VALIDATE_EMAIL,
        logical::php_filter_validate_email,
    ),
    entry(
        "validate_ip",
        FILTER_VALIDATE_IP,
        logical::php_filter_validate_ip,
    ),
    entry(
        "validate_mac",
        FILTER_VALIDATE_MAC,
        logical::php_filter_validate_mac,
    ),
    entry(
        "string",
        FILTER_SANITIZE_STRING,
        sanitize::php_filter_string,
    ),
    entry(
        "stripped",
        FILTER_SANITIZE_STRING,
        sanitize::php_filter_string,
    ),
    entry(
        "encoded",
        FILTER_SANITIZE_ENCODED,
        sanitize::php_filter_encoded,
    ),
    entry(
        "special_chars",
        FILTER_SANITIZE_SPECIAL_CHARS,
        sanitize::php_filter_special_chars,
    ),
    entry(
        "full_special_chars",
        FILTER_SANITIZE_FULL_SPECIAL_CHARS,
        sanitize::php_filter_full_special_chars,
    ),
    entry(
        "unsafe_raw",
        FILTER_UNSAFE_RAW,
        sanitize::php_filter_unsafe_raw,
    ),
    entry("email", FILTER_SANITIZE_EMAIL, sanitize::php_filter_email),
    entry("url", FILTER_SANITIZE_URL, sanitize::php_filter_url),
    entry(
        "number_int",
        FILTER_SANITIZE_NUMBER_INT,
        sanitize::php_filter_number_int,
    ),
    entry(
        "number_float",
        FILTER_SANITIZE_NUMBER_FLOAT,
        sanitize::php_filter_number_float,
    ),
    entry(
        "add_slashes",
        FILTER_SANITIZE_ADD_SLASHES,
        sanitize::php_filter_add_slashes,
    ),
    entry("callback", FILTER_CALLBACK, callback::php_filter_callback),
];

/// `php_find_filter()`: the first entry with `id`, falling back to
/// `FILTER_DEFAULT` (`unsafe_raw`).
fn php_find_filter(id: i64) -> &'static FilterEntry {
    FILTER_LIST
        .iter()
        .find(|f| f.id == id)
        .or_else(|| FILTER_LIST.iter().find(|f| f.id == FILTER_DEFAULT))
        .expect("FILTER_DEFAULT is in the filter list")
}

// ---------------------------------------------------------------------------
// Per-VM state
// ---------------------------------------------------------------------------

/// `zend_filter_globals` (the raw request variables) plus the patterns
/// compiled for `FILTER_VALIDATE_REGEXP` / `FILTER_VALIDATE_EMAIL`.
#[derive(Default)]
struct FilterState {
    post_array: Option<PhpArrayRef>,
    get_array: Option<PhpArrayRef>,
    cookie_array: Option<PhpArrayRef>,
    server_array: Option<PhpArrayRef>,
    env_array: Option<PhpArrayRef>,
    /// `PG(http_globals)[TRACK_VARS_ENV]` (`$_ENV`), the `INPUT_ENV`
    /// fallback when the SAPI filtered no environment variable.
    http_env: Option<PhpArrayRef>,
    regex_cache: HashMap<Vec<u8>, Rc<Regex>>,
}

/// Installs the raw variables of an input source, as `php_sapi_filter()`
/// collects them while the SAPI registers request variables. `PARSE_POST`,
/// `PARSE_GET`, `PARSE_COOKIE`, `PARSE_SERVER` and `PARSE_ENV` fill
/// `IF_G(*_array)`; `PARSE_STRING` records `$_ENV` itself, which `INPUT_ENV`
/// falls back to. `None` marks a storage as uninitialised.
pub fn set_input_array(vm: &mut Vm, input_type: i64, array: Option<PhpArray>) {
    let st = vm.ext_data::<FilterState>();
    let array = array.map(Rc::new);
    match input_type {
        PARSE_POST => st.post_array = array,
        PARSE_GET => st.get_array = array,
        PARSE_COOKIE => st.cookie_array = array,
        PARSE_STRING => st.http_env = array,
        PARSE_ENV => st.env_array = array,
        PARSE_SERVER => st.server_array = array,
        _ => {}
    }
}

/// `php_filter_get_storage()`: `Ok(None)` for an uninitialised storage.
fn php_filter_get_storage(vm: &mut Vm, arg: i64) -> PResult<Option<PhpArrayRef>> {
    let st = vm.ext_data::<FilterState>();
    let array = match arg {
        PARSE_GET => st.get_array.clone(),
        PARSE_POST => st.post_array.clone(),
        PARSE_COOKIE => st.cookie_array.clone(),
        PARSE_SERVER => st.server_array.clone(),
        PARSE_ENV => st.env_array.clone().or_else(|| st.http_env.clone()),
        _ => return Err(vm.argument_value_error(1, "must be an INPUT_* constant")),
    };
    Ok(array)
}

/// `pcre_get_compiled_regex()` for the filters: compiles (and caches) a
/// pattern with delimiters and modifiers, warning like ext/pcre on failure.
fn compiled_regex(vm: &mut Vm, pattern: &[u8]) -> PResult<Option<Rc<Regex>>> {
    if let Some(re) = vm.ext_data::<FilterState>().regex_cache.get(pattern) {
        return Ok(Some(re.clone()));
    }
    match php_regex::compile_php_pattern(pattern) {
        Ok(compiled) => {
            let re = Rc::new(compiled.regex);
            let cache = &mut vm.ext_data::<FilterState>().regex_cache;
            // PCRE_CACHE_SIZE
            if cache.len() >= 4096 {
                cache.clear();
            }
            cache.insert(pattern.to_vec(), re.clone());
            Ok(Some(re))
        }
        Err(e) => {
            vm.warning(e.message_bytes())?;
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Option helpers (`FETCH_*_OPTION`, `RETURN_VALIDATION_FAILED`)
// ---------------------------------------------------------------------------

/// `RETURN_VALIDATION_FAILED`.
fn validation_failed(value: &mut Value, flags: i64) -> Outcome {
    if flags & FILTER_THROW_ON_FAILURE != 0 {
        *value = Value::Null;
        return Outcome::Failure;
    }
    *value = failure_value(flags);
    Outcome::Success
}

/// `false`, or `null` with `FILTER_NULL_ON_FAILURE`.
fn failure_value(flags: i64) -> Value {
    if flags & FILTER_NULL_ON_FAILURE != 0 {
        Value::Null
    } else {
        Value::Bool(false)
    }
}

/// `zend_hash_str_find(Z_ARRVAL_P(option_array), name)`.
fn find_option<'a>(options: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    match options? {
        Value::Array(a) => a.get_str(name.as_bytes()),
        _ => None,
    }
}

/// `FETCH_LONG_OPTION`.
fn long_option(vm: &mut Vm, options: Option<&Value>, name: &str) -> PResult<Option<i64>> {
    match find_option(options, name) {
        Some(v) => Ok(Some(vm.to_int(v)?)),
        None => Ok(None),
    }
}

/// `FETCH_DOUBLE_OPTION`.
fn double_option(vm: &mut Vm, options: Option<&Value>, name: &str) -> PResult<Option<f64>> {
    match find_option(options, name) {
        Some(v) => Ok(Some(vm.to_float(v)?)),
        None => Ok(None),
    }
}

/// `FETCH_STRING_OPTION` / `FETCH_STR_OPTION`: only string values count.
fn string_option(options: Option<&Value>, name: &str) -> Option<PhpStr> {
    match find_option(options, name)?.unref() {
        Value::String(s) => Some(s),
        _ => None,
    }
}

/// The value being filtered (filter handlers always receive strings).
fn value_str(value: &Value) -> PhpStr {
    match value {
        Value::String(s) => s.clone(),
        other => unreachable!("filters operate on strings, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// php_zval_filter / php_filter_call / php_filter_array_handler
// ---------------------------------------------------------------------------

/// `zend_throw_exception_ex(php_filter_failed_exception_ce, 0, ...)`.
fn throw_failed(vm: &mut Vm, msg: impl Into<PhpStr>) -> Flow {
    vm.throw_error(FILTER_FAILED_EXCEPTION, msg)
}

/// The `handle_default:` part of `php_zval_filter()`.
fn handle_default(value: &mut Value, flags: i64, options: Option<&Value>) {
    let Some(Value::Array(opts)) = options else {
        return;
    };
    let failed = if flags & FILTER_NULL_ON_FAILURE != 0 {
        matches!(value, Value::Null)
    } else {
        matches!(value, Value::Bool(false))
    };
    if failed && let Some(default) = opts.get_str(b"default") {
        *value = default.unref();
    }
}

/// `php_zval_filter()`: applies one filter to a scalar or object.
fn php_zval_filter(
    vm: &mut Vm,
    value: &mut Value,
    filter: i64,
    flags: i64,
    options: Option<&Value>,
) -> PResult<()> {
    let filter_func = php_find_filter(filter);

    // #49274: objects without __toString() fail instead of erroring.
    if let Value::Object(o) = value
        && o.class().magic.tostring.is_none()
    {
        let class = o.class().name.clone();
        if flags & FILTER_THROW_ON_FAILURE != 0 {
            *value = Value::Null;
            let mut msg = b"filter validation failed: object of type ".to_vec();
            msg.extend_from_slice(c_str(&class));
            msg.extend_from_slice(b" has no __toString() method");
            return Err(throw_failed(vm, PhpStr::from_vec(msg)));
        }
        // #67167: objects honour FILTER_NULL_ON_FAILURE.
        *value = failure_value(flags);
        handle_default(value, flags, options);
        return Ok(());
    }

    // Here be strings.
    let copy_for_throwing = vm.to_php_string(value)?;
    *value = Value::String(copy_for_throwing.clone());

    let result = (filter_func.function)(vm, value, flags, options)?;

    if flags & FILTER_THROW_ON_FAILURE != 0 && result == Outcome::Failure {
        let mut msg = b"filter validation failed: filter ".to_vec();
        msg.extend_from_slice(filter_func.name.as_bytes());
        msg.extend_from_slice(b" not satisfied by '");
        msg.extend_from_slice(c_str(&copy_for_throwing));
        msg.push(b'\'');
        return Err(throw_failed(vm, PhpStr::from_vec(msg)));
    }

    handle_default(value, flags, options);
    Ok(())
}

/// `php_zval_filter_recursive()`. Elements held by reference are filtered
/// through the reference; `active` holds the references whose arrays are
/// being walked (`Z_PROTECT_RECURSION`).
fn php_zval_filter_recursive(
    vm: &mut Vm,
    value: &mut Value,
    filter: i64,
    flags: i64,
    options: Option<&Value>,
    active: &mut Vec<PhpRef>,
) -> PResult<()> {
    let Value::Array(arr) = value else {
        return php_zval_filter(vm, value, filter, flags, options);
    };
    for element in Rc::make_mut(arr).values_mut() {
        match element {
            Value::Ref(r) => {
                if active.iter().any(|a| PhpRef::ptr_eq(a, r)) {
                    continue;
                }
                let r = r.clone();
                let mut inner = r.get();
                if matches!(inner, Value::Array(_)) {
                    active.push(r.clone());
                    let res =
                        php_zval_filter_recursive(vm, &mut inner, filter, flags, options, active);
                    active.pop();
                    res?;
                } else {
                    php_zval_filter(vm, &mut inner, filter, flags, options)?;
                }
                r.set(inner);
            }
            Value::Array(_) => {
                php_zval_filter_recursive(vm, element, filter, flags, options, active)?;
            }
            _ => php_zval_filter(vm, element, filter, flags, options)?,
        }
    }
    Ok(())
}

/// `php_filter_call()`: resolves the filter, flags and options from the
/// filter arguments (`filter_args_ht` or `filter_args_long`) and filters
/// `filtered` in place. `filter == -1` takes the filter from the arguments
/// (the array handler).
fn php_filter_call(
    vm: &mut Vm,
    filtered: &mut Value,
    mut filter: i64,
    filter_args_ht: Option<&PhpArrayRef>,
    filter_args_long: i64,
    mut filter_flags: i64,
    options_arg_num: usize,
) -> PResult<()> {
    let mut options: Option<Value> = None;

    match filter_args_ht {
        None => {
            if filter != -1 {
                // The long argument holds the flags.
                filter_flags = filter_args_long;
                if filter_flags & (FILTER_REQUIRE_ARRAY | FILTER_FORCE_ARRAY) == 0 {
                    filter_flags |= FILTER_REQUIRE_SCALAR;
                }
            } else {
                filter = filter_args_long;
            }
        }
        Some(ht) => {
            if let Some(option) = ht.get_str(b"filter") {
                filter = vm.to_int(option)?;
            }
            if let Some(option) = ht.get_str(b"options") {
                let option = option.unref();
                if filter != FILTER_CALLBACK {
                    if matches!(option, Value::Array(_)) {
                        options = Some(option);
                    }
                } else {
                    options = Some(option);
                    filter_flags = 0;
                }
            }
            if let Some(option) = ht.get_str(b"flags") {
                filter_flags = vm.to_int(option)?;
                if filter_flags & (FILTER_REQUIRE_ARRAY | FILTER_FORCE_ARRAY) == 0 {
                    filter_flags |= FILTER_REQUIRE_SCALAR;
                }
            }
        }
    }

    if filter_flags & FILTER_NULL_ON_FAILURE != 0 && filter_flags & FILTER_THROW_ON_FAILURE != 0 {
        *filtered = Value::Null;
        return Err(vm.argument_value_error(
            options_arg_num,
            "cannot use both FILTER_NULL_ON_FAILURE and FILTER_THROW_ON_FAILURE",
        ));
    }

    if matches!(filtered, Value::Array(_)) {
        if filter_flags & FILTER_REQUIRE_SCALAR != 0 {
            if filter_flags & FILTER_THROW_ON_FAILURE != 0 {
                *filtered = Value::Null;
                return Err(throw_failed(
                    vm,
                    "filter validation failed: not a scalar value (got an array)",
                ));
            }
            *filtered = failure_value(filter_flags);
            return Ok(());
        }
        return php_zval_filter_recursive(
            vm,
            filtered,
            filter,
            filter_flags,
            options.as_ref(),
            &mut Vec::new(),
        );
    }
    if filter_flags & FILTER_REQUIRE_ARRAY != 0 {
        if filter_flags & FILTER_THROW_ON_FAILURE != 0 {
            let msg = format!(
                "filter validation failed: not an array (got {})",
                filtered.value_name()
            );
            *filtered = Value::Null;
            return Err(throw_failed(vm, msg));
        }
        *filtered = failure_value(filter_flags);
        return Ok(());
    }

    php_zval_filter(vm, filtered, filter, filter_flags, options.as_ref())?;
    if filter_flags & FILTER_FORCE_ARRAY != 0 {
        let scalar = std::mem::take(filtered);
        *filtered = Value::array(PhpArray::from_values(vec![scalar]));
    }
    Ok(())
}

/// `php_filter_array_handler()`: `filter_var_array()` /
/// `filter_input_array()`.
fn php_filter_array_handler(
    vm: &mut Vm,
    input: &PhpArrayRef,
    op_ht: Option<&PhpArrayRef>,
    op_long: i64,
    add_empty: bool,
) -> PResult<Value> {
    let Some(op_ht) = op_ht else {
        let mut ret = Value::Array(input.clone());
        php_filter_call(vm, &mut ret, -1, None, op_long, FILTER_REQUIRE_ARRAY, 2)?;
        return Ok(ret);
    };

    let mut ret = PhpArray::new();
    for (key, arg_elm) in op_ht.iter() {
        let ArrayKey::Str(arg_key) = key else {
            return Err(vm.argument_type_error(2, "must contain only string keys"));
        };
        if arg_key.is_empty() {
            return Err(vm.argument_value_error(2, "cannot contain empty keys"));
        }
        let Some(tmp) = input.get_str(&arg_key) else {
            if add_empty {
                ret.insert(ArrayKey::Str(arg_key), Value::Null);
            }
            continue;
        };
        let mut nval = tmp.unref();

        // The type check does not look through references, the conversion does.
        let (elm_ht, elm_long) = match arg_elm {
            Value::Array(a) => (Some(a.clone()), 0),
            other => {
                let filter_id = vm.to_int(other)?;
                if !filter_id_exists(filter_id) {
                    vm.warning(format!("Unknown filter with ID {filter_id}"))?;
                }
                (None, vm.to_int(other)?)
            }
        };
        php_filter_call(
            vm,
            &mut nval,
            -1,
            elm_ht.as_ref(),
            elm_long,
            FILTER_REQUIRE_SCALAR,
            2,
        )?;
        ret.insert(ArrayKey::Str(arg_key), nval);
    }
    Ok(Value::array(ret))
}

/// `Z_PARAM_ARRAY_HT_OR_LONG`.
fn array_or_long(v: &Value) -> (Option<PhpArrayRef>, i64) {
    match v.unref() {
        Value::Array(a) => (Some(a), 0),
        Value::Int(n) => (None, n),
        Value::Bool(b) => (None, i64::from(b)),
        _ => (None, 0),
    }
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

fn unknown_filter(vm: &mut Vm, id: i64) -> PResult<Value> {
    vm.warning(format!("Unknown filter with ID {id}"))?;
    Ok(Value::Bool(false))
}

/// `PHP_FUNCTION(filter_has_var)`.
fn filter_has_var(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let arg = args.int(0);
    let var = args.str(1).clone();
    let array = php_filter_get_storage(vm, arg)?;
    Ok(Value::Bool(
        array.is_some_and(|a| a.get_str(&var).is_some()),
    ))
}

/// `PHP_FUNCTION(filter_input)`.
fn filter_input(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let fetch_from = args.int(0);
    let var = args.str(1).clone();
    let filter = args.int(2);
    let (filter_args_ht, filter_args_long) = array_or_long(args.get(3));

    if !filter_id_exists(filter) {
        return unknown_filter(vm, filter);
    }

    let input = php_filter_get_storage(vm, fetch_from)?;
    let tmp = input
        .as_ref()
        .and_then(|a| a.get_str(&var))
        .map(Value::unref);

    let Some(tmp) = tmp else {
        let filter_flags = match &filter_args_ht {
            None => filter_args_long,
            Some(ht) => match ht.get_str(b"flags") {
                Some(option) => vm.to_int(option)?,
                None => 0,
            },
        };

        if filter_flags & FILTER_NULL_ON_FAILURE != 0 && filter_flags & FILTER_THROW_ON_FAILURE != 0
        {
            return Err(vm.argument_value_error(
                4,
                "cannot use both FILTER_NULL_ON_FAILURE and FILTER_THROW_ON_FAILURE",
            ));
        }

        if filter_flags & FILTER_THROW_ON_FAILURE != 0 {
            let mut msg = b"input value '".to_vec();
            msg.extend_from_slice(c_str(&var));
            msg.extend_from_slice(b"' not found");
            return Err(throw_failed(vm, PhpStr::from_vec(msg)));
        }

        // FILTER_THROW_ON_FAILURE overrides the default, so it comes first.
        if let Some(ht) = &filter_args_ht
            && let Some(Value::Array(opt)) = ht.get_str(b"options").map(Value::unref)
            && let Some(def) = opt.get_str(b"default")
        {
            return Ok(def.unref());
        }

        // FILTER_NULL_ON_FAILURE inverts the usual results: a missing
        // variable gives false (and a failed validation null).
        return Ok(if filter_flags & FILTER_NULL_ON_FAILURE != 0 {
            Value::Bool(false)
        } else {
            Value::Null
        });
    };

    let mut ret = tmp;
    php_filter_call(
        vm,
        &mut ret,
        filter,
        filter_args_ht.as_ref(),
        filter_args_long,
        FILTER_REQUIRE_SCALAR,
        4,
    )?;
    Ok(ret)
}

/// `PHP_FUNCTION(filter_var)`.
fn filter_var(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let data = args.get(0).unref();
    let filter = args.int(1);
    let (filter_args_ht, filter_args_long) = array_or_long(args.get(2));

    if !filter_id_exists(filter) {
        return unknown_filter(vm, filter);
    }

    let mut ret = data;
    php_filter_call(
        vm,
        &mut ret,
        filter,
        filter_args_ht.as_ref(),
        filter_args_long,
        FILTER_REQUIRE_SCALAR,
        3,
    )?;
    Ok(ret)
}

/// `PHP_FUNCTION(filter_input_array)`.
fn filter_input_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let fetch_from = args.int(0);
    let (op_ht, op_long) = array_or_long(args.get(1));
    let add_empty = args.bool(2);

    if op_ht.is_none() && !filter_id_exists(op_long) {
        return unknown_filter(vm, op_long);
    }

    let Some(input) = php_filter_get_storage(vm, fetch_from)? else {
        return Ok(Value::Null);
    };
    php_filter_array_handler(vm, &input, op_ht.as_ref(), op_long, add_empty)
}

/// `PHP_FUNCTION(filter_var_array)`.
fn filter_var_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let (op_ht, op_long) = array_or_long(args.get(1));
    let add_empty = args.bool(2);

    if op_ht.is_none() && !filter_id_exists(op_long) {
        return unknown_filter(vm, op_long);
    }

    php_filter_array_handler(vm, &input, op_ht.as_ref(), op_long, add_empty)
}

/// `PHP_FUNCTION(filter_list)`.
fn filter_list(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(Value::array(
        FILTER_LIST.iter().map(|f| Value::str(f.name)).collect(),
    ))
}

/// `PHP_FUNCTION(filter_id)`: the name is compared as a C string.
fn filter_id(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = c_str(args.str(0));
    Ok(FILTER_LIST
        .iter()
        .find(|f| f.name.as_bytes() == name)
        .map_or(Value::Bool(false), |f| Value::Int(f.id)))
}

static FILTER_HAS_VAR: NativeFunction = NativeFunction {
    sig: "function filter_has_var(int $input_type, string $var_name): bool {}",
    handler: filter_has_var,
};
static FILTER_INPUT: NativeFunction = NativeFunction {
    sig: "function filter_input(int $type, string $var_name, int $filter = FILTER_DEFAULT, array|int $options = 0): mixed {}",
    handler: filter_input,
};
static FILTER_VAR: NativeFunction = NativeFunction {
    sig: "function filter_var(mixed $value, int $filter = FILTER_DEFAULT, array|int $options = 0): mixed {}",
    handler: filter_var,
};
static FILTER_INPUT_ARRAY: NativeFunction = NativeFunction {
    sig: "function filter_input_array(int $type, array|int $options = FILTER_DEFAULT, bool $add_empty = true): array|false|null {}",
    handler: filter_input_array,
};
static FILTER_VAR_ARRAY: NativeFunction = NativeFunction {
    sig: "function filter_var_array(array $array, array|int $options = FILTER_DEFAULT, bool $add_empty = true): array|false {}",
    handler: filter_var_array,
};
static FILTER_LIST_FN: NativeFunction = NativeFunction {
    sig: "function filter_list(): array {}",
    handler: filter_list,
};
static FILTER_ID: NativeFunction = NativeFunction {
    sig: "function filter_id(string $name): int|false {}",
    handler: filter_id,
};

/// `filter.stub.php` constants up to `FILTER_UNSAFE_RAW`, in stub order.
const CONSTANTS: &[(&str, i64)] = &[
    ("INPUT_POST", PARSE_POST),
    ("INPUT_GET", PARSE_GET),
    ("INPUT_COOKIE", PARSE_COOKIE),
    ("INPUT_ENV", PARSE_ENV),
    ("INPUT_SERVER", PARSE_SERVER),
    ("FILTER_FLAG_NONE", FILTER_FLAG_NONE),
    ("FILTER_REQUIRE_SCALAR", FILTER_REQUIRE_SCALAR),
    ("FILTER_REQUIRE_ARRAY", FILTER_REQUIRE_ARRAY),
    ("FILTER_FORCE_ARRAY", FILTER_FORCE_ARRAY),
    ("FILTER_NULL_ON_FAILURE", FILTER_NULL_ON_FAILURE),
    ("FILTER_THROW_ON_FAILURE", FILTER_THROW_ON_FAILURE),
    ("FILTER_VALIDATE_INT", FILTER_VALIDATE_INT),
    ("FILTER_VALIDATE_BOOLEAN", FILTER_VALIDATE_BOOL),
    ("FILTER_VALIDATE_BOOL", FILTER_VALIDATE_BOOL),
    ("FILTER_VALIDATE_FLOAT", FILTER_VALIDATE_FLOAT),
    ("FILTER_VALIDATE_REGEXP", FILTER_VALIDATE_REGEXP),
    ("FILTER_VALIDATE_DOMAIN", FILTER_VALIDATE_DOMAIN),
    ("FILTER_VALIDATE_URL", FILTER_VALIDATE_URL),
    ("FILTER_VALIDATE_EMAIL", FILTER_VALIDATE_EMAIL),
    ("FILTER_VALIDATE_IP", FILTER_VALIDATE_IP),
    ("FILTER_VALIDATE_MAC", FILTER_VALIDATE_MAC),
    ("FILTER_DEFAULT", FILTER_DEFAULT),
    ("FILTER_UNSAFE_RAW", FILTER_UNSAFE_RAW),
];

/// The constants after the deprecated `FILTER_SANITIZE_STRING` pair.
const CONSTANTS_AFTER_STRING: &[(&str, i64)] = &[
    ("FILTER_SANITIZE_ENCODED", FILTER_SANITIZE_ENCODED),
    (
        "FILTER_SANITIZE_SPECIAL_CHARS",
        FILTER_SANITIZE_SPECIAL_CHARS,
    ),
    (
        "FILTER_SANITIZE_FULL_SPECIAL_CHARS",
        FILTER_SANITIZE_FULL_SPECIAL_CHARS,
    ),
    ("FILTER_SANITIZE_EMAIL", FILTER_SANITIZE_EMAIL),
    ("FILTER_SANITIZE_URL", FILTER_SANITIZE_URL),
    ("FILTER_SANITIZE_NUMBER_INT", FILTER_SANITIZE_NUMBER_INT),
    ("FILTER_SANITIZE_NUMBER_FLOAT", FILTER_SANITIZE_NUMBER_FLOAT),
    ("FILTER_SANITIZE_ADD_SLASHES", FILTER_SANITIZE_ADD_SLASHES),
    ("FILTER_CALLBACK", FILTER_CALLBACK),
    ("FILTER_FLAG_ALLOW_OCTAL", FILTER_FLAG_ALLOW_OCTAL),
    ("FILTER_FLAG_ALLOW_HEX", FILTER_FLAG_ALLOW_HEX),
    ("FILTER_FLAG_STRIP_LOW", FILTER_FLAG_STRIP_LOW),
    ("FILTER_FLAG_STRIP_HIGH", FILTER_FLAG_STRIP_HIGH),
    ("FILTER_FLAG_STRIP_BACKTICK", FILTER_FLAG_STRIP_BACKTICK),
    ("FILTER_FLAG_ENCODE_LOW", FILTER_FLAG_ENCODE_LOW),
    ("FILTER_FLAG_ENCODE_HIGH", FILTER_FLAG_ENCODE_HIGH),
    ("FILTER_FLAG_ENCODE_AMP", FILTER_FLAG_ENCODE_AMP),
    ("FILTER_FLAG_NO_ENCODE_QUOTES", FILTER_FLAG_NO_ENCODE_QUOTES),
    (
        "FILTER_FLAG_EMPTY_STRING_NULL",
        FILTER_FLAG_EMPTY_STRING_NULL,
    ),
    ("FILTER_FLAG_ALLOW_FRACTION", FILTER_FLAG_ALLOW_FRACTION),
    ("FILTER_FLAG_ALLOW_THOUSAND", FILTER_FLAG_ALLOW_THOUSAND),
    ("FILTER_FLAG_ALLOW_SCIENTIFIC", FILTER_FLAG_ALLOW_SCIENTIFIC),
    ("FILTER_FLAG_PATH_REQUIRED", FILTER_FLAG_PATH_REQUIRED),
    ("FILTER_FLAG_QUERY_REQUIRED", FILTER_FLAG_QUERY_REQUIRED),
    ("FILTER_FLAG_IPV4", FILTER_FLAG_IPV4),
    ("FILTER_FLAG_IPV6", FILTER_FLAG_IPV6),
    ("FILTER_FLAG_NO_RES_RANGE", FILTER_FLAG_NO_RES_RANGE),
    ("FILTER_FLAG_NO_PRIV_RANGE", FILTER_FLAG_NO_PRIV_RANGE),
    ("FILTER_FLAG_GLOBAL_RANGE", FILTER_FLAG_GLOBAL_RANGE),
    ("FILTER_FLAG_HOSTNAME", FILTER_FLAG_HOSTNAME),
    ("FILTER_FLAG_EMAIL_UNICODE", FILTER_FLAG_EMAIL_UNICODE),
];

pub fn register(r: &mut Registry) {
    // PHP_INI_BEGIN(): both entries are PHP_INI_SYSTEM|PHP_INI_PERDIR.
    r.ini_entries(&[
        IniDef::new(
            "filter.default",
            Some("unsafe_raw"),
            modifiable::SYSTEM | modifiable::PERDIR,
        ),
        IniDef::new(
            "filter.default_flags",
            None,
            modifiable::SYSTEM | modifiable::PERDIR,
        ),
    ]);

    for &(name, value) in CONSTANTS {
        r.constant(name, value);
    }
    const SANITIZE_STRING_DEPRECATION: &str = "since 8.1, use htmlspecialchars() instead";
    r.deprecated_constant(
        "FILTER_SANITIZE_STRING",
        FILTER_SANITIZE_STRING,
        SANITIZE_STRING_DEPRECATION,
    );
    r.deprecated_constant(
        "FILTER_SANITIZE_STRIPPED",
        FILTER_SANITIZE_STRING,
        SANITIZE_STRING_DEPRECATION,
    );
    for &(name, value) in CONSTANTS_AFTER_STRING {
        r.constant(name, value);
    }

    r.functions(&[
        &FILTER_HAS_VAR,
        &FILTER_INPUT,
        &FILTER_VAR,
        &FILTER_INPUT_ARRAY,
        &FILTER_VAR_ARRAY,
        &FILTER_LIST_FN,
        &FILTER_ID,
    ]);

    let exception = r
        .find_class("Exception")
        .expect("Exception is a core class");
    let filter_exception = r.class(ClassEntry::new_subclass(FILTER_EXCEPTION, 0, &exception));
    r.class(ClassEntry::new_subclass(
        FILTER_FAILED_EXCEPTION,
        0,
        &filter_exception,
    ));
}
