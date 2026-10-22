//! `ini_get`, `ini_get_all`, `ini_set`, `ini_alter`, `ini_restore`,
//! `ini_parse_quantity`, `set_include_path`, `get_include_path`
//! (`basic_functions.c`, quantities from `Zend/zend_ini.c`).

use std::collections::HashMap;

use php_runtime::errors::append_escaped;
use php_runtime::ini::modifiable;
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

use super::path_arg;
use crate::standard::assert;

// ---------------------------------------------------------------------------
// zend_ini_parse_quantity
// ---------------------------------------------------------------------------

/// `zend_is_whitespace()`
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn escaped(s: &[u8]) -> String {
    let mut out = Vec::new();
    append_escaped(&mut out, s);
    String::from_utf8_lossy(&out).into_owned()
}

/// `strtoull(s, &end, base)` for input that starts at a digit (no sign or
/// leading whitespace): `(value, bytes consumed, ERANGE)`.
fn strtoull(s: &[u8], mut base: u32) -> (u64, usize, bool) {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut i = 0;
    if (base == 0 || base == 16)
        && at(0) == b'0'
        && (at(1) | 0x20) == b'x'
        && (at(2) as char).is_ascii_hexdigit()
    {
        i = 2;
        base = 16;
    } else if base == 0 {
        base = if at(0) == b'0' { 8 } else { 10 };
    }
    let start = i;
    let mut acc: u64 = 0;
    let mut overflow = false;
    while i < s.len() {
        let Some(d) = (s[i] as char).to_digit(base) else {
            break;
        };
        match acc
            .checked_mul(base as u64)
            .and_then(|v| v.checked_add(d as u64))
        {
            Some(v) => acc = v,
            None => {
                overflow = true;
                acc = u64::MAX;
            }
        }
        i += 1;
    }
    if i == start {
        return (0, 0, false);
    }
    (acc, i, overflow)
}

/// `zend_ini_consume_quantity_prefix`: where a base prefix, sign and
/// whitespace would end.
fn consume_quantity_prefix(s: &[u8], digits: usize, str_end: usize, base: u32) -> usize {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let mut d = digits;
    while d < str_end && is_ws(at(d)) {
        d += 1;
    }
    if at(d) == b'+' || at(d) == b'-' {
        d += 1;
    }
    if at(d) == b'0' && !at(d + 1).is_ascii_digit() {
        // Value is just 0
        if d + 1 == str_end {
            return d;
        }
        match at(d + 1) {
            b'x' | b'X' | b'o' | b'O' => d += 2,
            // 0b is valid in base 16, but not in the other supported bases
            b'b' | b'B' if base != 16 => d += 2,
            _ => {}
        }
    }
    d
}

/// `zend_ini_parse_quantity_internal`: the value and the warning, if any.
fn parse_quantity_internal(value: &[u8], signed: bool) -> (u64, Option<String>) {
    let at = |i: usize| value.get(i).copied().unwrap_or(0);
    let mut digits = 0usize;
    let mut str_end = value.len();
    // Ignore leading and trailing whitespace.
    while digits < str_end && is_ws(at(digits)) {
        digits += 1;
    }
    while digits < str_end && is_ws(at(str_end - 1)) {
        str_end -= 1;
    }
    if digits == str_end {
        return (0, None);
    }

    let mut is_negative = false;
    if at(digits) == b'+' {
        digits += 1;
    } else if at(digits) == b'-' {
        is_negative = true;
        digits += 1;
    }

    let no_leading_digits = || {
        format!(
            "Invalid quantity \"{}\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            escaped(value)
        )
    };
    if !at(digits).is_ascii_digit() {
        return (0, Some(no_leading_digits()));
    }

    let mut base = 0u32;
    if at(digits) == b'0' && !at(digits + 1).is_ascii_digit() {
        // Value is just 0
        if digits + 1 == str_end {
            return (0, None);
        }
        match at(digits + 1) {
            // Multiplier suffixes
            b'g' | b'G' | b'm' | b'M' | b'k' | b'K' => {}
            b'x' | b'X' => base = 16,
            b'o' | b'O' => base = 8,
            b'b' | b'B' => base = 2,
            c => {
                return (
                    0,
                    Some(format!(
                        "Invalid prefix \"0{}\", interpreting as \"0\" for backwards compatibility",
                        c as char
                    )),
                );
            }
        }
        if base != 0 {
            digits += 2;
            // strtoull() may silently skip whitespace, a sign and a base
            // prefix, which would be invalid at this position.
            if digits == str_end || digits != consume_quantity_prefix(value, digits, str_end, base)
            {
                return (
                    0,
                    Some(format!(
                        "Invalid quantity \"{}\": no digits after base prefix, interpreting as \"0\" for backwards compatibility",
                        escaped(value)
                    )),
                );
            }
        }
    }

    // `strtoul` runs over the NUL-terminated string, past the trimmed end.
    let (mut retval, consumed, erange) = strtoull(&value[digits.min(value.len())..], base);
    let mut digits_end = digits + consumed;
    let mut overflow = false;

    if erange {
        overflow = true;
    } else if !signed {
        if is_negative {
            // Ignore "-1" as it is commonly used as max value (memory_limit=-1).
            if retval == 1 && digits_end == str_end {
                retval = u64::MAX;
            } else {
                overflow = true;
            }
        }
    } else if is_negative && retval == i64::MAX as u64 + 1 {
        // PHP_INT_MIN
        retval = 0u64.wrapping_sub(retval);
    } else if (retval as i64) < 0 {
        overflow = true;
    } else if is_negative {
        retval = 0u64.wrapping_sub(retval);
    }

    if digits_end == digits {
        // No leading digits
        return (0, Some(no_leading_digits()));
    }

    // Allow for whitespace between the integer portion and any suffix.
    while digits_end < str_end && is_ws(at(digits_end)) {
        digits_end += 1;
    }

    if digits_end != str_end {
        let last = value[str_end - 1];
        let factor: u64 = match last {
            b'g' | b'G' => 1 << 30,
            b'm' | b'M' => 1 << 20,
            b'k' | b'K' => 1 << 10,
            _ => {
                // Unknown suffix
                return (
                    retval,
                    Some(format!(
                        "Invalid quantity \"{}\": unknown multiplier \"{}\", interpreting as \"{}\" for backwards compatibility",
                        escaped(value),
                        escaped(&[last]),
                        escaped(&value[..digits_end])
                    )),
                );
            }
        };

        if !overflow {
            if signed {
                let s = retval as i64;
                overflow = if s > 0 {
                    s > i64::MAX / factor as i64
                } else {
                    s < i64::MIN / factor as i64
                };
            } else {
                overflow = retval > u64::MAX / factor;
            }
        }
        retval = retval.wrapping_mul(factor);

        if digits_end != str_end - 1 {
            // More than one character in the suffix
            return (
                retval,
                Some(format!(
                    "Invalid quantity \"{}\", interpreting as \"{}{}\" for backwards compatibility",
                    escaped(value),
                    escaped(&value[..digits_end]),
                    escaped(&[last])
                )),
            );
        }
    }

    if overflow {
        return (
            retval,
            Some(format!(
                "Invalid quantity \"{}\": value is out of range, using overflow result for backwards compatibility",
                escaped(value)
            )),
        );
    }
    (retval, None)
}

/// `zend_ini_parse_quantity()`: the value and the warning text.
pub fn parse_quantity(value: &[u8]) -> (i64, Option<String>) {
    let (v, e) = parse_quantity_internal(value, true);
    (v as i64, e)
}

fn ini_parse_quantity(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (v, err) = parse_quantity(args.str(0));
    if let Some(e) = err {
        vm.engine_warning(e)?;
    }
    Ok(Value::Int(v))
}

// ---------------------------------------------------------------------------
// Directive access
// ---------------------------------------------------------------------------

/// The compiled-in defaults of the directives (`ini_entry->def->value`),
/// recorded before any `-d` / php.ini value replaced them.
#[derive(Default)]
struct BuiltinDefaults {
    map: HashMap<Vec<u8>, Option<PhpStr>>,
}

/// Directives whose default the CLI SAPI overrides (`sapi_cli_ini_defaults`
/// and `HARDCODED_INI`) although the compiled-in default differs.
const CLI_OVERRIDDEN: &[(&str, &str)] = &[
    ("html_errors", "1"),
    ("implicit_flush", "0"),
    ("max_execution_time", "30"),
    ("max_input_time", "-1"),
];

/// Records the compiled-in defaults of every directive registered so far
/// (`builtin_default_value`); modules call it right after registering theirs.
pub(crate) fn snapshot_defaults(vm: &mut Vm) {
    let entries: Vec<(Vec<u8>, Option<PhpStr>)> = vm
        .ini
        .iter()
        .map(|e| {
            let orig = match &e.orig_value {
                Some(o) => o.clone(),
                None => e.value.clone(),
            };
            (e.name.to_vec(), orig)
        })
        .collect();
    let d = vm.ext_data::<BuiltinDefaults>();
    for (name, value) in entries {
        d.map.entry(name).or_insert(value);
    }
}

fn builtin_default(vm: &mut Vm, name: &[u8]) -> Option<PhpStr> {
    if let Some((_, v)) = CLI_OVERRIDDEN.iter().find(|(n, _)| n.as_bytes() == name) {
        return Some(PhpStr::from(*v));
    }
    if !vm.ext_data::<BuiltinDefaults>().map.contains_key(name) {
        snapshot_defaults(vm);
    }
    vm.ext_data::<BuiltinDefaults>()
        .map
        .get(name)
        .cloned()
        .flatten()
}

fn ini_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match vm.ini.value(args.str(0)) {
        Some(v) => Value::String(v),
        None => Value::Bool(false),
    })
}

/// `(name, value, orig_value, modifiable, module)` of a directive.
type IniRow = (
    PhpStr,
    Option<PhpStr>,
    Option<Option<PhpStr>>,
    u32,
    &'static str,
);

fn ini_get_all(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let extname = args.str_opt(0).cloned();
    let details = args.bool(1);

    if let Some(ext) = &extname {
        // The module registry is keyed by lowercase names.
        let found = vm
            .modules
            .iter()
            .any(|m| m.to_ascii_lowercase().as_bytes() == ext.as_bytes());
        if !found {
            vm.warning(format!("Extension \"{ext}\" cannot be found"))?;
            return Ok(Value::Bool(false));
        }
    }

    let mut rows: Vec<IniRow> = vm
        .ini
        .iter()
        .map(|e| {
            (
                e.name.clone(),
                e.value.clone(),
                e.orig_value.clone(),
                e.modifiable,
                e.module,
            )
        })
        .collect();
    // zend_ini_sort_entries(): case-insensitive by name
    rows.sort_by(|a, b| {
        a.0.to_ascii_lowercase()
            .as_bytes()
            .cmp(b.0.to_ascii_lowercase().as_bytes())
    });

    let mut out = PhpArray::new();
    for (name, value, orig, modif, module) in rows {
        if let Some(ext) = &extname {
            if module.to_ascii_lowercase().as_bytes() != ext.as_bytes() {
                continue;
            }
        }
        if details {
            let mut opt = PhpArray::new();
            let global = match &orig {
                Some(o) => o.clone(),
                None => value.clone(),
            };
            opt.insert_str("global_value", global.map_or(Value::Null, Value::String));
            opt.insert_str("local_value", value.map_or(Value::Null, Value::String));
            let default = builtin_default(vm, &name);
            opt.insert_str(
                "builtin_default_value",
                default.map_or(Value::Null, Value::String),
            );
            opt.insert_str("access", modif as i64);
            out.insert(ArrayKey::from_bytes(&name), Value::array(opt));
        } else {
            out.insert(
                ArrayKey::from_bytes(&name),
                value.map_or(Value::Null, Value::String),
            );
        }
    }
    Ok(Value::array(out))
}

/// The `on_modify` checks php-src runs before a directive changes at
/// runtime. `Ok(false)` rejects the value (`FAILURE`).
fn pre_modify(vm: &mut Vm, name: &[u8], new_value: &PhpStr) -> PResult<bool> {
    match name {
        // OnUpdateStringUnempty / OnUpdateStrNotEmpty
        b"include_path"
        | b"extension_dir"
        | b"upload_tmp_dir"
        | b"variables_order"
        | b"arg_separator.output"
        | b"arg_separator.input"
        | b"doc_root"
        | b"sys_temp_dir"
        | b"output_handler" => Ok(!new_value.is_empty()),
        // OnUpdateAssertions (Zend/zend.c)
        b"zend.assertions" => {
            let cur = vm.ini_int("zend.assertions");
            let (val, err) = parse_quantity(new_value);
            if let Some(e) = err {
                vm.engine_warning(format!("Invalid \"zend.assertions\" setting. {e}"))?;
            }
            if cur != val && (cur < 0 || val < 0) {
                vm.engine_warning(
                    "zend.assertions may be completely enabled or disabled only in php.ini",
                )?;
                return Ok(false);
            }
            Ok(true)
        }
        _ => Ok(true),
    }
}

/// Side effects of a successful change (`on_modify` handlers with output).
fn post_modify(vm: &mut Vm, name: &[u8]) -> PResult<()> {
    if name.starts_with(b"assert.") {
        let v = vm.ini.get(name).and_then(|e| e.value.clone());
        assert::on_ini_modified(vm, name, v.as_ref())?;
    }
    Ok(())
}

/// `zend_alter_ini_entry_ex(name, value, PHP_INI_USER, PHP_INI_STAGE_RUNTIME)`:
/// the old value, or `None` on failure.
pub(crate) fn alter_ini_runtime(
    vm: &mut Vm,
    name: &PhpStr,
    new_value: PhpStr,
) -> PResult<Option<PhpStr>> {
    if vm.ini.get(name).is_none() {
        return Ok(None);
    }
    if !pre_modify(vm, name, &new_value)? {
        return Ok(None);
    }
    let old = vm.ini_set(name, new_value);
    if old.is_some() {
        post_modify(vm, name)?;
    }
    Ok(old)
}

fn ini_set(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let new_value = args.get(1).clone();
    if matches!(
        new_value,
        Value::Array(_) | Value::Object(_) | Value::Resource(_)
    ) {
        return Err(vm.argument_type_error(2, "must be of type string|int|float|bool|null"));
    }
    let old = vm.ini.value(&name);
    let new_str = vm.to_php_string(&new_value)?;
    Ok(match alter_ini_runtime(vm, &name, new_str)? {
        Some(_) => old.map_or(Value::Bool(false), Value::String),
        None => Value::Bool(false),
    })
}

fn ini_restore(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let name = args.str(0).clone();
    let eligible = vm
        .ini
        .get(&name)
        .is_some_and(|e| e.modifiable & modifiable::USER != 0 && e.is_modified());
    if eligible {
        vm.ini_restore(&name);
        post_modify(vm, &name)?;
    }
    Ok(Value::Null)
}

fn set_include_path(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let new_value = path_arg(vm, args, 0)?;
    let old = vm.ini.value(b"include_path");
    let key = PhpStr::from("include_path");
    Ok(match alter_ini_runtime(vm, &key, new_value)? {
        Some(_) => old.map_or(Value::Bool(false), Value::String),
        None => Value::Bool(false),
    })
}

fn get_include_path(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(match vm.ini.value(b"include_path") {
        Some(v) => Value::String(v),
        None => Value::Bool(false),
    })
}

pub static INI_PARSE_QUANTITY: NativeFunction = NativeFunction {
    sig: "ini_parse_quantity(string $shorthand): int",
    handler: ini_parse_quantity,
};
pub static INI_GET: NativeFunction = NativeFunction {
    sig: "ini_get(string $option): string|false",
    handler: ini_get,
};
pub static INI_GET_ALL: NativeFunction = NativeFunction {
    sig: "ini_get_all(?string $extension = null, bool $details = true): array|false",
    handler: ini_get_all,
};
pub static INI_SET: NativeFunction = NativeFunction {
    sig: "ini_set(string $option, string|int|float|bool|null $value): string|false",
    handler: ini_set,
};
pub static INI_ALTER: NativeFunction = NativeFunction {
    sig: "ini_alter(string $option, string|int|float|bool|null $value): string|false",
    handler: ini_set,
};
pub static INI_RESTORE: NativeFunction = NativeFunction {
    sig: "ini_restore(string $option): void",
    handler: ini_restore,
};
pub static SET_INCLUDE_PATH: NativeFunction = NativeFunction {
    sig: "set_include_path(string $include_path): string|false",
    handler: set_include_path,
};
pub static GET_INCLUDE_PATH: NativeFunction = NativeFunction {
    sig: "get_include_path(): string|false",
    handler: get_include_path,
};

pub fn register(r: &mut Registry) {
    r.functions(&[
        &INI_PARSE_QUANTITY,
        &INI_GET,
        &INI_GET_ALL,
        &INI_SET,
        &INI_ALTER,
        &INI_RESTORE,
        &SET_INCLUDE_PATH,
        &GET_INCLUDE_PATH,
    ]);
    r.constant("INI_USER", modifiable::USER as i64);
    r.constant("INI_PERDIR", modifiable::PERDIR as i64);
    r.constant("INI_SYSTEM", modifiable::SYSTEM as i64);
    r.constant("INI_ALL", modifiable::ALL as i64);
    snapshot_defaults(r.vm);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn q(s: &str) -> (i64, Option<String>) {
        parse_quantity(s.as_bytes())
    }

    fn qb(s: &[u8]) -> (i64, Option<String>) {
        parse_quantity(s)
    }

    #[test]
    fn valid_quantities_do_not_warn() {
        // zend_ini_parse_quantity.phpt
        for lead in ["", " "] {
            for sign in ["", "+", "-"] {
                for mid in ["", " "] {
                    for (exp, mult) in [
                        ("", 1i64),
                        ("K", 1 << 10),
                        ("k", 1 << 10),
                        ("M", 1 << 20),
                        ("m", 1 << 20),
                        ("G", 1 << 30),
                        ("g", 1 << 30),
                    ] {
                        for trail in ["", " "] {
                            let sgn = if sign == "-" { -1 } else { 1 };
                            let s = format!("{lead}{sign}1{mid}{exp}{trail}");
                            assert_eq!(q(&s), (sgn * mult, None), "{s:?}");
                            if exp != "g" && exp != "G" {
                                let s = format!("{lead}{sign}0123{mid}{exp}{trail}");
                                assert_eq!(q(&s), (sgn * 83 * mult, None), "{s:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn quantity_errors() {
        // zend_ini_parse_quantity_error.phpt
        let cases: &[(&[u8], i64, &str)] = &[
            (
                b"K",
                0,
                "Invalid quantity \"K\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"1KM",
                1048576,
                "Invalid quantity \"1KM\", interpreting as \"1M\" for backwards compatibility",
            ),
            (
                b"1X",
                1,
                "Invalid quantity \"1X\": unknown multiplier \"X\", interpreting as \"1\" for backwards compatibility",
            ),
            (
                b"1.0K",
                1024,
                "Invalid quantity \"1.0K\", interpreting as \"1K\" for backwards compatibility",
            ),
            (
                b"0X",
                0,
                "Invalid quantity \"0X\": no digits after base prefix, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"0Z",
                0,
                "Invalid prefix \"0Z\", interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"0XK",
                0,
                "Invalid quantity \"0XK\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"++",
                0,
                "Invalid quantity \"++\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"-+",
                0,
                "Invalid quantity \"-+\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"+ 25",
                0,
                "Invalid quantity \"+ 25\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b"- 25",
                0,
                "Invalid quantity \"- 25\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b" 123\0K",
                125952,
                "Invalid quantity \" 123\\x00K\", interpreting as \" 123K\" for backwards compatibility",
            ),
            (
                b"\0 123K",
                0,
                "Invalid quantity \"\\x00 123K\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b" \x00123K",
                0,
                "Invalid quantity \" \\x00123K\": no valid leading digits, interpreting as \"0\" for backwards compatibility",
            ),
            (
                b" 123K\0",
                123,
                "Invalid quantity \" 123K\\x00\": unknown multiplier \"\\x00\", interpreting as \" 123\" for backwards compatibility",
            ),
            (
                b" 123\0",
                123,
                "Invalid quantity \" 123\\x00\": unknown multiplier \"\\x00\", interpreting as \" 123\" for backwards compatibility",
            ),
        ];
        for (input, want, msg) in cases {
            assert_eq!(
                qb(input),
                (*want, Some((*msg).to_string())),
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn prefixes_and_gh_regressions() {
        // GH-16886 / GH-16892 / gh11876
        for s in [
            "0x 0", "0x+0", "0x-0", "0b 0", "0b+0", "0b-0", "0o 0", "0o+0", "0o-0", "0x0x12",
            "0b+10", "0o+10", "0x+10", "0b 10", "0o 10", "0x 10",
        ] {
            let (v, e) = q(s);
            assert_eq!(v, 0, "{s}");
            assert_eq!(
                e.unwrap(),
                format!(
                    "Invalid quantity \"{s}\": no digits after base prefix, interpreting as \"0\" for backwards compatibility"
                )
            );
        }
        assert_eq!(q("0x0b"), (11, None));
        assert_eq!(q("0xb"), (11, None));
        assert_eq!(q("-0x0B"), (-11, None));
        assert_eq!(q("-0xB"), (-11, None));
        assert_eq!(q("0x0beef"), (48879, None));
        assert_eq!(q("-0xBEEF"), (-48879, None));
        assert_eq!(q("0o17"), (15, None));
        assert_eq!(q("0b101"), (5, None));
        assert_eq!(q("0"), (0, None));
        assert_eq!(q(""), (0, None));
        assert_eq!(q("   "), (0, None));
        for (s, ch) in [("0g10", '0'), ("0m10", '0'), ("0k10", '0')] {
            assert_eq!(
                q(s),
                (
                    0,
                    Some(format!(
                        "Invalid quantity \"{s}\": unknown multiplier \"{ch}\", interpreting as \"0\" for backwards compatibility"
                    ))
                )
            );
        }
    }

    #[test]
    fn quantity_overflow() {
        // zend_ini_parse_quantity_overflow.phpt
        assert_eq!(q("9223372036854775807"), (i64::MAX, None));
        assert_eq!(q("-9223372036854775808"), (i64::MIN, None));
        assert_eq!(q("2K"), (2048, None));
        assert_eq!(q("-2K"), (-2048, None));
        let msg = |s: &str| {
            format!(
                "Invalid quantity \"{s}\": value is out of range, using overflow result for backwards compatibility"
            )
        };
        let (v, e) = q("9223372036854775808");
        assert_eq!((v, e), (i64::MIN, Some(msg("9223372036854775808"))));
        let (_, e) = q("-9223372036854775809");
        assert_eq!(e, Some(msg("-9223372036854775809")));
        let (_, e) = q("9223372036854775807K");
        assert_eq!(e, Some(msg("9223372036854775807K")));
        let (_, e) = q("-9223372036854775808K");
        assert_eq!(e, Some(msg("-9223372036854775808K")));
    }

    #[test]
    fn ini_parse_quantity_function_warns_without_prefix() {
        let mut t = TestVm::new();
        let r = t
            .call("ini_parse_quantity", vec![Value::str("1X")])
            .unwrap();
        assert_eq!(r.as_int(), Some(1));
        assert_eq!(
            t.output(),
            "\nWarning: Invalid quantity \"1X\": unknown multiplier \"X\", interpreting as \"1\" for backwards compatibility in /t.php on line 3\n"
        );
        assert_eq!(
            t.call("ini_parse_quantity", vec![Value::str("128M")])
                .unwrap()
                .as_int(),
            Some(134217728)
        );
        assert_eq!(t.output(), "");
    }

    #[test]
    fn ini_get_and_set() {
        let mut t = TestVm::new();
        assert!(matches!(
            t.call("ini_get", vec![Value::str("no.such")]).unwrap(),
            Value::Bool(false)
        ));
        assert_eq!(
            t.call("ini_get", vec![Value::str("precision")])
                .unwrap()
                .as_str()
                .unwrap(),
            "14"
        );
        // ini_set returns the old value and applies the new one
        let old = t
            .call("ini_set", vec![Value::str("precision"), Value::Int(6)])
            .unwrap();
        assert_eq!(old.as_str().unwrap(), "14");
        assert_eq!(t.vm.precision, 6);
        assert_eq!(
            t.call("ini_get", vec![Value::str("precision")])
                .unwrap()
                .as_str()
                .unwrap(),
            "6"
        );
        // non-strings are converted (ini_set_types.phpt)
        t.call(
            "ini_set",
            vec![Value::str("html_errors"), Value::Bool(true)],
        )
        .unwrap();
        assert_eq!(t.vm.ini_get("html_errors").unwrap(), "1");
        t.call(
            "ini_set",
            vec![Value::str("html_errors"), Value::Bool(false)],
        )
        .unwrap();
        assert_eq!(t.vm.ini_get("html_errors").unwrap(), "");
        t.call(
            "ini_set",
            vec![Value::str("user_agent"), Value::Float(3.25)],
        )
        .unwrap();
        assert_eq!(t.vm.ini_get("user_agent").unwrap(), "3.25");
        t.call("ini_set", vec![Value::str("docref_root"), Value::Null])
            .unwrap();
        assert_eq!(t.vm.ini_get("docref_root").unwrap(), "");
        // failures
        assert!(matches!(
            t.call("ini_set", vec![Value::str("no.such"), Value::str("x")])
                .unwrap(),
            Value::Bool(false)
        ));
        assert!(matches!(
            t.call("ini_set", vec![Value::str("memory_limit"), Value::str("1")])
                .unwrap_or(Value::Null),
            Value::String(_) | Value::Bool(false)
        ));
        // PHP_INI_SYSTEM entries cannot change at runtime
        assert!(matches!(
            t.call(
                "ini_set",
                vec![Value::str("disable_functions"), Value::str("x")]
            )
            .unwrap(),
            Value::Bool(false)
        ));
        // precision's validator rejects values below -1
        assert!(matches!(
            t.call("ini_set", vec![Value::str("precision"), Value::Int(-5)])
                .unwrap(),
            Value::Bool(false)
        ));
        // ini_alter is the same function
        let old = t
            .call("ini_alter", vec![Value::str("precision"), Value::Int(10)])
            .unwrap();
        assert_eq!(old.as_str().unwrap(), "6");
    }

    #[test]
    fn ini_set_rejects_arrays() {
        let mut t = TestVm::new();
        let r = t.call("ini_set", vec![Value::str("foo"), Value::empty_array()]);
        let (c, m) = thrown(r);
        assert_eq!(c, "TypeError");
        assert!(
            m.starts_with(
                "ini_set(): Argument #2 ($value) must be of type string|int|float|bool|null"
            ),
            "{m}"
        );
    }

    #[test]
    fn ini_restore_returns_to_the_original() {
        let mut t = TestVm::new();
        t.call("ini_set", vec![Value::str("precision"), Value::Int(3)])
            .unwrap();
        assert_eq!(t.vm.precision, 3);
        assert!(matches!(
            t.call("ini_restore", vec![Value::str("precision")])
                .unwrap(),
            Value::Null
        ));
        assert_eq!(t.vm.precision, 14);
        assert_eq!(t.vm.ini_get("precision").unwrap(), "14");
        // unknown names are ignored
        t.call("ini_restore", vec![Value::str("no.such")]).unwrap();
    }

    #[test]
    fn include_path() {
        let mut t = TestVm::new();
        let old = t.call("get_include_path", vec![]).unwrap();
        assert_eq!(old.as_str().unwrap(), ".:/usr/share/php");
        let r = t
            .call("set_include_path", vec![Value::str("/a:/b")])
            .unwrap();
        assert_eq!(r.as_str().unwrap(), ".:/usr/share/php");
        assert_eq!(
            t.call("get_include_path", vec![])
                .unwrap()
                .as_str()
                .unwrap(),
            "/a:/b"
        );
        // an empty path is rejected (OnUpdateStringUnempty)
        assert!(matches!(
            t.call("set_include_path", vec![Value::str("")]).unwrap(),
            Value::Bool(false)
        ));
        assert_eq!(
            t.call("get_include_path", vec![])
                .unwrap()
                .as_str()
                .unwrap(),
            "/a:/b"
        );
        let (c, m) = thrown(t.call("set_include_path", vec![Value::bytes(b"a\0b")]));
        assert_eq!(c, "ValueError");
        assert_eq!(
            m,
            "set_include_path(): Argument #1 ($include_path) must not contain any null bytes"
        );
    }

    #[test]
    fn ini_get_all_shapes() {
        let mut t = TestVm::new();
        // unknown extension
        let r = t
            .call("ini_get_all", vec![Value::str("nosuchextension")])
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: ini_get_all(): Extension \"nosuchextension\" cannot be found in /t.php on line 3\n"
        );
        let r = t.call("ini_get_all", vec![Value::str("")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
        t.output();

        // an extension without directives
        let Value::Array(a) = t
            .call("ini_get_all", vec![Value::str("reflection")])
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(a.len(), 0);

        // details
        let Value::Array(all) = t.call("ini_get_all", vec![]).unwrap() else {
            panic!()
        };
        let Some(Value::Array(p)) = all.get_str(b"precision") else {
            panic!()
        };
        let keys: Vec<String> = p.keys().map(|k| k.to_string()).collect();
        assert_eq!(
            keys,
            [
                "global_value",
                "local_value",
                "builtin_default_value",
                "access"
            ]
        );
        assert_eq!(p.get_str(b"access").unwrap().as_int(), Some(7));
        // sorted by name
        let names: Vec<String> = all.keys().map(|k| k.to_string()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_ascii_lowercase());
        assert_eq!(names, sorted);

        // without details
        let Value::Array(flat) = t
            .call("ini_get_all", vec![Value::Null, Value::Bool(false)])
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(flat.get_str(b"precision").unwrap().as_str().unwrap(), "14");
    }

    #[test]
    fn ini_get_all_default_and_global_values() {
        // ini_get_all_builtin_default_value_null.phpt
        let mut t = TestVm::new();
        t.vm.ini_set_system("error_append_string", "FOO");
        let get = |t: &mut TestVm| {
            let Value::Array(all) = t
                .call("ini_get_all", vec![Value::Null, Value::Bool(true)])
                .unwrap()
            else {
                panic!()
            };
            let Some(Value::Array(e)) = all.get_str(b"error_append_string") else {
                panic!()
            };
            let f = |k: &[u8]| e.get_str(k).unwrap().clone();
            (
                f(b"global_value"),
                f(b"local_value"),
                f(b"builtin_default_value"),
            )
        };
        let (g, l, d) = get(&mut t);
        assert_eq!(
            (
                g.as_str().unwrap().to_string(),
                l.as_str().unwrap().to_string()
            ),
            ("FOO".into(), "FOO".into())
        );
        assert!(matches!(d, Value::Null));
        t.call(
            "ini_set",
            vec![Value::str("error_append_string"), Value::str("BAR")],
        )
        .unwrap();
        let (g, l, d) = get(&mut t);
        assert_eq!(
            (
                g.as_str().unwrap().to_string(),
                l.as_str().unwrap().to_string()
            ),
            ("FOO".into(), "BAR".into())
        );
        assert!(matches!(d, Value::Null));
    }

    #[test]
    fn ini_get_all_by_module() {
        let mut t = TestVm::new();
        let Value::Array(a) = t
            .call(
                "ini_get_all",
                vec![Value::str("standard"), Value::Bool(false)],
            )
            .unwrap()
        else {
            panic!()
        };
        assert!(a.get_str(b"assert.active").is_some());
        assert!(a.get_str(b"precision").is_none());
        // module names are matched in lowercase only (module_registry keys)
        let r = t.call("ini_get_all", vec![Value::str("Core")]).unwrap();
        assert!(matches!(r, Value::Bool(false)));
    }

    #[test]
    fn zend_assertions_cannot_be_toggled_at_runtime() {
        let mut t = TestVm::new();
        // 1 -> 0 is fine, 0 -> -1 is not
        let old = t
            .call(
                "ini_set",
                vec![Value::str("zend.assertions"), Value::Int(0)],
            )
            .unwrap();
        assert_eq!(old.as_str().unwrap(), "1");
        let r = t
            .call(
                "ini_set",
                vec![Value::str("zend.assertions"), Value::Int(-1)],
            )
            .unwrap();
        assert!(matches!(r, Value::Bool(false)));
        assert_eq!(
            t.output(),
            "\nWarning: zend.assertions may be completely enabled or disabled only in php.ini in /t.php on line 3\n"
        );
    }

    #[test]
    fn assert_directives_emit_deprecations() {
        let mut t = TestVm::new();
        t.call("ini_set", vec![Value::str("assert.active"), Value::Int(0)])
            .unwrap();
        assert_eq!(
            t.output(),
            "\nDeprecated: ini_set(): assert.active INI setting is deprecated in /t.php on line 3\n"
        );
        t.call("ini_set", vec![Value::str("assert.active"), Value::Int(1)])
            .unwrap();
        assert_eq!(t.output(), "");
        t.call(
            "ini_set",
            vec![Value::str("assert.callback"), Value::str("cb")],
        )
        .unwrap();
        assert!(
            t.output()
                .contains("assert.callback INI setting is deprecated")
        );
        // assert_options() reports the callback set through ini
        let v = t.call("assert_options", vec![Value::Int(2)]).unwrap();
        assert_eq!(v.as_str().unwrap(), "cb");
        let _ = t.output();
    }

    #[test]
    fn constants() {
        let t = TestVm::new();
        assert_eq!(t.vm.constant("INI_ALL").unwrap().as_int(), Some(7));
        assert_eq!(t.vm.constant("INI_USER").unwrap().as_int(), Some(1));
    }
}
