//! ext/standard/string.c — `setlocale()`, `localeconv()`, `nl_langinfo()`,
//! `strcoll()` and the `LC_*`, `CHAR_MAX` and `nl_langinfo()` item constants.
//!
//! Only the built-in C/POSIX locale exists. Constant values and the answers of
//! `nl_langinfo()` / `localeconv()` are those of glibc's C locale.

use php_runtime::class::fn_flags;
use php_runtime::{Args, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// `CHAR_MAX` (signed `char`, as on x86-64 Linux).
pub const CHAR_MAX: i64 = 127;

pub const LC_CTYPE: i64 = 0;
pub const LC_NUMERIC: i64 = 1;
pub const LC_TIME: i64 = 2;
pub const LC_COLLATE: i64 = 3;
pub const LC_MONETARY: i64 = 4;
pub const LC_MESSAGES: i64 = 5;
pub const LC_ALL: i64 = 6;
/// glibc's `__LC_LAST`: categories are `0..LC_LAST` (incl. `LC_PAPER` & co.).
const LC_LAST: i64 = 13;

/// glibc `_NL_ITEM(category, index)`.
const fn nl_item(category: i64, index: i64) -> i64 {
    (category << 16) | index
}

/// `nl_langinfo()` items with their glibc values and C-locale answers, in
/// stub order.
const NL_ITEMS: &[(&str, i64, &str)] = &[
    ("ABDAY_1", nl_item(LC_TIME, 0), "Sun"),
    ("ABDAY_2", nl_item(LC_TIME, 1), "Mon"),
    ("ABDAY_3", nl_item(LC_TIME, 2), "Tue"),
    ("ABDAY_4", nl_item(LC_TIME, 3), "Wed"),
    ("ABDAY_5", nl_item(LC_TIME, 4), "Thu"),
    ("ABDAY_6", nl_item(LC_TIME, 5), "Fri"),
    ("ABDAY_7", nl_item(LC_TIME, 6), "Sat"),
    ("DAY_1", nl_item(LC_TIME, 7), "Sunday"),
    ("DAY_2", nl_item(LC_TIME, 8), "Monday"),
    ("DAY_3", nl_item(LC_TIME, 9), "Tuesday"),
    ("DAY_4", nl_item(LC_TIME, 10), "Wednesday"),
    ("DAY_5", nl_item(LC_TIME, 11), "Thursday"),
    ("DAY_6", nl_item(LC_TIME, 12), "Friday"),
    ("DAY_7", nl_item(LC_TIME, 13), "Saturday"),
    ("ABMON_1", nl_item(LC_TIME, 14), "Jan"),
    ("ABMON_2", nl_item(LC_TIME, 15), "Feb"),
    ("ABMON_3", nl_item(LC_TIME, 16), "Mar"),
    ("ABMON_4", nl_item(LC_TIME, 17), "Apr"),
    ("ABMON_5", nl_item(LC_TIME, 18), "May"),
    ("ABMON_6", nl_item(LC_TIME, 19), "Jun"),
    ("ABMON_7", nl_item(LC_TIME, 20), "Jul"),
    ("ABMON_8", nl_item(LC_TIME, 21), "Aug"),
    ("ABMON_9", nl_item(LC_TIME, 22), "Sep"),
    ("ABMON_10", nl_item(LC_TIME, 23), "Oct"),
    ("ABMON_11", nl_item(LC_TIME, 24), "Nov"),
    ("ABMON_12", nl_item(LC_TIME, 25), "Dec"),
    ("MON_1", nl_item(LC_TIME, 26), "January"),
    ("MON_2", nl_item(LC_TIME, 27), "February"),
    ("MON_3", nl_item(LC_TIME, 28), "March"),
    ("MON_4", nl_item(LC_TIME, 29), "April"),
    ("MON_5", nl_item(LC_TIME, 30), "May"),
    ("MON_6", nl_item(LC_TIME, 31), "June"),
    ("MON_7", nl_item(LC_TIME, 32), "July"),
    ("MON_8", nl_item(LC_TIME, 33), "August"),
    ("MON_9", nl_item(LC_TIME, 34), "September"),
    ("MON_10", nl_item(LC_TIME, 35), "October"),
    ("MON_11", nl_item(LC_TIME, 36), "November"),
    ("MON_12", nl_item(LC_TIME, 37), "December"),
    ("AM_STR", nl_item(LC_TIME, 38), "AM"),
    ("PM_STR", nl_item(LC_TIME, 39), "PM"),
    ("D_T_FMT", nl_item(LC_TIME, 40), "%a %b %e %H:%M:%S %Y"),
    ("D_FMT", nl_item(LC_TIME, 41), "%m/%d/%y"),
    ("T_FMT", nl_item(LC_TIME, 42), "%H:%M:%S"),
    ("T_FMT_AMPM", nl_item(LC_TIME, 43), "%I:%M:%S %p"),
    ("ERA", nl_item(LC_TIME, 44), ""),
    ("ERA_YEAR", nl_item(LC_TIME, 45), ""),
    ("ERA_D_T_FMT", nl_item(LC_TIME, 48), ""),
    ("ERA_D_FMT", nl_item(LC_TIME, 46), ""),
    ("ERA_T_FMT", nl_item(LC_TIME, 49), ""),
    ("ALT_DIGITS", nl_item(LC_TIME, 47), ""),
    ("INT_CURR_SYMBOL", nl_item(LC_MONETARY, 0), ""),
    ("CURRENCY_SYMBOL", nl_item(LC_MONETARY, 1), ""),
    ("CRNCYSTR", nl_item(LC_MONETARY, 15), "-"),
    ("MON_DECIMAL_POINT", nl_item(LC_MONETARY, 2), ""),
    ("MON_THOUSANDS_SEP", nl_item(LC_MONETARY, 3), ""),
    ("MON_GROUPING", nl_item(LC_MONETARY, 4), "\x7f"),
    ("POSITIVE_SIGN", nl_item(LC_MONETARY, 5), ""),
    ("NEGATIVE_SIGN", nl_item(LC_MONETARY, 6), ""),
    ("INT_FRAC_DIGITS", nl_item(LC_MONETARY, 7), "\x7f"),
    ("FRAC_DIGITS", nl_item(LC_MONETARY, 8), "\x7f"),
    ("P_CS_PRECEDES", nl_item(LC_MONETARY, 9), "\x7f"),
    ("P_SEP_BY_SPACE", nl_item(LC_MONETARY, 10), "\x7f"),
    ("N_CS_PRECEDES", nl_item(LC_MONETARY, 11), "\x7f"),
    ("N_SEP_BY_SPACE", nl_item(LC_MONETARY, 12), "\x7f"),
    ("P_SIGN_POSN", nl_item(LC_MONETARY, 13), "\x7f"),
    ("N_SIGN_POSN", nl_item(LC_MONETARY, 14), "\x7f"),
    ("DECIMAL_POINT", nl_item(LC_NUMERIC, 0), "."),
    ("RADIXCHAR", nl_item(LC_NUMERIC, 0), "."),
    ("THOUSANDS_SEP", nl_item(LC_NUMERIC, 1), ""),
    ("THOUSEP", nl_item(LC_NUMERIC, 1), ""),
    ("GROUPING", nl_item(LC_NUMERIC, 2), "\x7f"),
    ("YESEXPR", nl_item(LC_MESSAGES, 0), "^[yY]"),
    ("NOEXPR", nl_item(LC_MESSAGES, 1), "^[nN]"),
    ("YESSTR", nl_item(LC_MESSAGES, 2), ""),
    ("NOSTR", nl_item(LC_MESSAGES, 3), ""),
    ("CODESET", nl_item(LC_CTYPE, 14), "ANSI_X3.4-1968"),
];

/// The C locale's answer for an `nl_langinfo()` item, `None` for items the
/// function rejects.
pub fn c_langinfo(item: i64) -> Option<&'static str> {
    NL_ITEMS
        .iter()
        .find(|&&(_, v, _)| v == item)
        .map(|&(_, _, answer)| answer)
}

/// Per-request locale state: the locale name of every category, as
/// `setlocale(cat, NULL)` reports it.
struct LocaleState {
    names: [&'static str; LC_LAST as usize],
}

impl Default for LocaleState {
    fn default() -> Self {
        LocaleState {
            names: ["C"; LC_LAST as usize],
        }
    }
}

impl LocaleState {
    /// The C library's `setlocale()` with only the built-in locale: `"C"`,
    /// `"POSIX"` and `""` (the environment's locale, which is always C here)
    /// select it; `None` queries.
    fn setlocale(&mut self, cat: i64, loc: Option<&[u8]>) -> Option<&'static str> {
        if !(0..LC_LAST).contains(&cat) {
            return None;
        }
        let Some(loc) = loc else {
            // A composite `LC_ALL` name only arises with differing categories.
            return Some(self.names[cat as usize]);
        };
        if !matches!(loc, b"C" | b"POSIX" | b"") {
            return None;
        }
        if cat == LC_ALL {
            self.names = ["C"; LC_LAST as usize];
        } else {
            self.names[cat as usize] = "C";
        }
        Some("C")
    }
}

/// `try_setlocale_str`.
fn try_setlocale_str(vm: &mut Vm, cat: i64, loc: &[u8]) -> PResult<Option<PhpStr>> {
    let loc = if loc == b"0" {
        None
    } else {
        if loc.len() >= 255 {
            vm.warning("Specified locale name is too long")?;
            return Ok(None);
        }
        Some(loc)
    };
    let state = vm.ext_data::<LocaleState>();
    let Some(retval) = state.setlocale(cat, loc) else {
        return Ok(None);
    };
    if let Some(loc) = loc {
        // php-src returns the requested name when the C library echoes it,
        // and "C" for the C locale (`BG(ctype_string)` = NULL).
        if retval != "C" && loc == retval.as_bytes() {
            return Ok(Some(PhpStr::from(loc)));
        }
    }
    Ok(Some(PhpStr::from(retval)))
}

/// `try_setlocale_zval`: one element of a `$locales` array.
fn try_setlocale_zval(vm: &mut Vm, cat: i64, loc: &Value) -> PResult<Option<PhpStr>> {
    let loc = vm.to_php_string(&loc.deref())?;
    if loc.contains(&0) {
        return Err(vm.argument_value_error(2, "must not contain any null bytes"));
    }
    try_setlocale_str(vm, cat, &loc)
}

/// `PHP_FUNCTION(setlocale)`.
fn setlocale(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let cat = args.int(0);
    let locales: Vec<Value> = std::iter::once(args.get(1).deref().clone())
        .chain(args.rest(2).iter().map(|v| v.deref().clone()))
        .collect();
    let num_args = locales.len();

    // Validate everything before setting anything (as php-src does).
    let mut strings: Vec<Option<PhpStr>> = Vec::with_capacity(num_args);
    for (i, v) in locales.iter().enumerate() {
        let arg_num = i + 2;
        if let Value::Array(_) = v {
            if i != 0 {
                return Err(vm.argument_type_error(
                    arg_num,
                    &format!("must be of type ?string, {} given", v.value_name()),
                ));
            }
            if num_args > 1 {
                return Err(vm.argument_count_error(format!(
                    "setlocale() expects exactly 2 arguments when argument #2 ($locales) is an array, {} given",
                    num_args + 1
                )));
            }
            break;
        }
        // `zend_parse_arg_path_str` with null allowed.
        if v.is_null() {
            strings.push(None);
            continue;
        }
        match vm.parse_arg_str_weak(v)? {
            Some(s) if !s.contains(&0) => strings.push(Some(s)),
            Some(_) => {
                return Err(vm.argument_value_error(arg_num, "must not contain any null bytes"));
            }
            None => {
                let expected = if i == 0 {
                    "array|string|null"
                } else {
                    "?string"
                };
                return Err(vm.argument_type_error(
                    arg_num,
                    &format!("must be of type {expected}, {} given", v.value_name()),
                ));
            }
        }
    }

    for (i, v) in locales.iter().enumerate() {
        let result = if let Value::Array(arr) = v {
            for (_, elem) in arr.iter() {
                if let Some(r) = try_setlocale_zval(vm, cat, elem)? {
                    return Ok(Value::String(r));
                }
            }
            continue;
        } else {
            match &strings[i] {
                None => try_setlocale_str(vm, cat, b"")?,
                Some(s) => try_setlocale_str(vm, cat, s)?,
            }
        };
        if let Some(r) = result {
            return Ok(Value::String(r));
        }
    }
    Ok(Value::Bool(false))
}

/// `PHP_FUNCTION(localeconv)` for the C locale.
fn localeconv(_vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    let mut arr = PhpArray::new();
    arr.insert_str("decimal_point", ".");
    for key in [
        "thousands_sep",
        "int_curr_symbol",
        "currency_symbol",
        "mon_decimal_point",
        "mon_thousands_sep",
        "positive_sign",
        "negative_sign",
    ] {
        arr.insert_str(key, "");
    }
    for key in [
        "int_frac_digits",
        "frac_digits",
        "p_cs_precedes",
        "p_sep_by_space",
        "n_cs_precedes",
        "n_sep_by_space",
        "p_sign_posn",
        "n_sign_posn",
    ] {
        arr.insert_str(key, CHAR_MAX);
    }
    arr.insert_str("grouping", Value::empty_array());
    arr.insert_str("mon_grouping", Value::empty_array());
    Ok(Value::array(arr))
}

/// `PHP_FUNCTION(nl_langinfo)`.
fn nl_langinfo(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let item = args.int(0);
    match c_langinfo(item) {
        Some(answer) => Ok(Value::str(answer)),
        None => {
            vm.warning(format!("Item '{item}' is not valid"))?;
            Ok(Value::Bool(false))
        }
    }
}

/// `strcoll()` in the C locale is `strcmp()` on C strings: the comparison
/// stops at a NUL byte and returns the difference of the first differing
/// bytes (glibc).
pub fn c_strcoll(a: &[u8], b: &[u8]) -> i64 {
    let cstr = |s: &[u8]| -> Vec<u8> { s.iter().copied().take_while(|&c| c != 0).collect() };
    let (a, b) = (cstr(a), cstr(b));
    for i in 0..=a.len().min(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return i64::from(x) - i64::from(y);
        }
    }
    0
}

fn strcoll(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Int(c_strcoll(args.str(0), args.str(1))))
}

pub static SETLOCALE: NativeFunction = NativeFunction {
    sig: "setlocale(int $category, $locales, ...$rest): string|false",
    handler: setlocale,
};
pub static LOCALECONV: NativeFunction = NativeFunction {
    sig: "localeconv(): array",
    handler: localeconv,
};
pub static NL_LANGINFO: NativeFunction = NativeFunction {
    sig: "nl_langinfo(int $item): string|false",
    handler: nl_langinfo,
};
pub static STRCOLL: NativeFunction = NativeFunction {
    sig: "strcoll(string $string1, string $string2): int",
    handler: strcoll,
};

pub fn register(r: &mut Registry) {
    r.constant("CHAR_MAX", CHAR_MAX);
    r.constant("LC_CTYPE", LC_CTYPE);
    r.constant("LC_NUMERIC", LC_NUMERIC);
    r.constant("LC_TIME", LC_TIME);
    r.constant("LC_COLLATE", LC_COLLATE);
    r.constant("LC_MONETARY", LC_MONETARY);
    r.constant("LC_ALL", LC_ALL);
    r.constant("LC_MESSAGES", LC_MESSAGES);
    for &(name, value, _) in NL_ITEMS {
        r.constant(name, value);
    }
    r.functions(&[&SETLOCALE, &LOCALECONV, &NL_LANGINFO]);
    r.function_with_flags(&STRCOLL, fn_flags::DEPRECATED);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};

    fn constant(t: &TestVm, name: &str) -> i64 {
        t.vm.constant(name).and_then(|v| v.as_int()).unwrap()
    }

    fn show(v: Value) -> String {
        match v {
            Value::String(s) => format!("{:?}", s.to_string_lossy()),
            Value::Bool(false) => "false".into(),
            v => panic!("unexpected {v:?}"),
        }
    }

    fn set(t: &mut TestVm, args: Vec<Value>) -> String {
        show(t.call("setlocale", args).unwrap())
    }

    #[test]
    fn constants() {
        let t = TestVm::new();
        assert_eq!(constant(&t, "LC_CTYPE"), 0);
        assert_eq!(constant(&t, "LC_ALL"), 6);
        assert_eq!(constant(&t, "LC_MESSAGES"), 5);
        assert_eq!(constant(&t, "CHAR_MAX"), 127);
        assert_eq!(constant(&t, "ABDAY_1"), 131072);
        assert_eq!(constant(&t, "D_T_FMT"), 131112);
        assert_eq!(constant(&t, "ERA_T_FMT"), 131121);
        assert_eq!(constant(&t, "RADIXCHAR"), 65536);
        assert_eq!(constant(&t, "THOUSEP"), 65537);
        assert_eq!(constant(&t, "CRNCYSTR"), 262159);
        assert_eq!(constant(&t, "YESEXPR"), 327680);
        assert_eq!(constant(&t, "CODESET"), 14);
    }

    #[test]
    fn setlocale_c_only() {
        let mut t = TestVm::new();
        let lc_all = Value::Int(LC_ALL);
        assert_eq!(set(&mut t, vec![lc_all.clone(), Value::str("C")]), "\"C\"");
        assert_eq!(set(&mut t, vec![lc_all.clone(), Value::Int(0)]), "\"C\"");
        assert_eq!(set(&mut t, vec![lc_all.clone(), Value::str("0")]), "\"C\"");
        assert_eq!(set(&mut t, vec![lc_all.clone(), Value::Null]), "\"C\"");
        assert_eq!(set(&mut t, vec![lc_all.clone(), Value::str("")]), "\"C\"");
        assert_eq!(
            set(&mut t, vec![Value::Int(LC_CTYPE), Value::str("POSIX")]),
            "\"C\""
        );
        assert_eq!(set(&mut t, vec![Value::Int(99), Value::str("C")]), "false");
        // setlocale_error.phpt
        let invalid = PhpArray::from_values(vec![
            Value::str("en_US.invalid"),
            Value::str("en_AU.invalid"),
        ]);
        assert_eq!(
            set(&mut t, vec![lc_all.clone(), Value::array(invalid)]),
            "false"
        );
        assert_eq!(
            set(
                &mut t,
                vec![
                    lc_all.clone(),
                    Value::str("en_US.invalid"),
                    Value::str("ko_KR.invalid")
                ]
            ),
            "false"
        );
        assert_eq!(t.output(), "");
        assert_eq!(
            set(&mut t, vec![lc_all.clone(), Value::str(&"A".repeat(255))]),
            "false"
        );
        assert_eq!(
            t.output(),
            "\nWarning: setlocale(): Specified locale name is too long in /t.php on line 3\n"
        );
        // First accepted candidate wins.
        let cands = PhpArray::from_values(vec![Value::str("de_DE"), Value::str("C")]);
        assert_eq!(
            set(&mut t, vec![lc_all.clone(), Value::array(cands)]),
            "\"C\""
        );
        assert_eq!(
            set(
                &mut t,
                vec![
                    lc_all,
                    Value::str("de_DE"),
                    Value::str("de"),
                    Value::str("POSIX")
                ]
            ),
            "\"C\""
        );
    }

    #[test]
    fn setlocale_null_bytes() {
        // setlocale_null_byte.phpt
        let mut t = TestVm::new();
        let lc_all = || Value::Int(LC_ALL);
        let r = t.call("setlocale", vec![lc_all(), Value::bytes(b"C\0locale")]);
        assert_eq!(
            thrown(r),
            (
                "ValueError".into(),
                "setlocale(): Argument #2 ($locales) must not contain any null bytes".into()
            )
        );
        let arr = PhpArray::from_values(vec![Value::bytes(b"locale\0name"), Value::str("C")]);
        let r = t.call("setlocale", vec![lc_all(), Value::array(arr)]);
        assert_eq!(
            thrown(r).1,
            "setlocale(): Argument #2 ($locales) must not contain any null bytes"
        );
        let arr = PhpArray::from_values(vec![Value::bytes(b"C\0locale")]);
        let r = t.call(
            "setlocale",
            vec![lc_all(), Value::str("zz_ZZ.nope"), Value::array(arr)],
        );
        assert_eq!(
            thrown(r),
            (
                "TypeError".into(),
                "setlocale(): Argument #3 must be of type ?string, array given".into()
            )
        );
        let r = t.call(
            "setlocale",
            vec![lc_all(), Value::str("zz_ZZ.nope"), Value::bytes(b"C\0x")],
        );
        assert_eq!(
            thrown(r).1,
            "setlocale(): Argument #3 must not contain any null bytes"
        );
        let r = t.call(
            "setlocale",
            vec![lc_all(), Value::empty_array(), Value::str("C")],
        );
        assert_eq!(
            thrown(r),
            (
                "ArgumentCountError".into(),
                "setlocale() expects exactly 2 arguments when argument #2 ($locales) is an array, 3 given".into()
            )
        );
    }

    #[test]
    fn localeconv_c() {
        let mut t = TestVm::new();
        let Value::Array(a) = t.call("localeconv", vec![]).unwrap() else {
            panic!()
        };
        let keys: Vec<String> = a.keys().map(|k| k.to_string()).collect();
        assert_eq!(
            keys,
            [
                "decimal_point",
                "thousands_sep",
                "int_curr_symbol",
                "currency_symbol",
                "mon_decimal_point",
                "mon_thousands_sep",
                "positive_sign",
                "negative_sign",
                "int_frac_digits",
                "frac_digits",
                "p_cs_precedes",
                "p_sep_by_space",
                "n_cs_precedes",
                "n_sep_by_space",
                "p_sign_posn",
                "n_sign_posn",
                "grouping",
                "mon_grouping"
            ]
        );
        assert_eq!(a.get_str(b"decimal_point").unwrap().as_str().unwrap(), ".");
        assert_eq!(a.get_str(b"thousands_sep").unwrap().as_str().unwrap(), "");
        assert_eq!(a.get_str(b"int_frac_digits").unwrap().as_int(), Some(127));
        assert_eq!(a.get_str(b"n_sign_posn").unwrap().as_int(), Some(127));
        assert!(
            a.get_str(b"grouping")
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn nl_langinfo_items() {
        // nl_langinfo_basic.phpt
        let mut t = TestVm::new();
        let info = |t: &mut TestVm, name: &str| {
            let item = t.vm.constant(name).unwrap();
            show(t.call("nl_langinfo", vec![item]).unwrap())
        };
        assert_eq!(info(&mut t, "ABDAY_2"), "\"Mon\"");
        assert_eq!(info(&mut t, "DAY_4"), "\"Wednesday\"");
        assert_eq!(info(&mut t, "ABMON_7"), "\"Jul\"");
        assert_eq!(info(&mut t, "MON_4"), "\"April\"");
        assert_eq!(info(&mut t, "RADIXCHAR"), "\".\"");
        assert_eq!(info(&mut t, "CODESET"), "\"ANSI_X3.4-1968\"");
        assert_eq!(info(&mut t, "D_T_FMT"), "\"%a %b %e %H:%M:%S %Y\"");
        assert_eq!(info(&mut t, "YESEXPR"), "\"^[yY]\"");
        let r = t.call("nl_langinfo", vec![Value::Int(-1)]).unwrap();
        assert_eq!(show(r), "false");
        assert_eq!(
            t.output(),
            "\nWarning: nl_langinfo(): Item '-1' is not valid in /t.php on line 3\n"
        );
    }

    #[test]
    fn strcoll_is_strcmp() {
        // strcoll.phpt
        let mut t = TestVm::new();
        let r = t
            .call("strcoll", vec![Value::str("a"), Value::str("A")])
            .unwrap();
        assert!(r.as_int().unwrap() > 0);
        assert_eq!(c_strcoll(b"abc", b"abc"), 0);
        assert!(c_strcoll(b"ab", b"abc") < 0);
        assert_eq!(c_strcoll(b"a\0x", b"a\0y"), 0);
        assert!(c_strcoll(b"\xff", b"a") > 0);
    }
}
