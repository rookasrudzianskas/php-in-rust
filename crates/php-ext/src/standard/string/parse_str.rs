//! ext/standard/string.c — `parse_str()`, i.e. main/php_variables.c's
//! `php_default_treat_data(PARSE_STRING, ...)` and `php_register_variable_ex`.

use std::rc::Rc;

use php_runtime::errors::DisplayErrors;
use php_runtime::{Args, ArrayKey, NativeFunction, PResult, PhpArray, PhpStr, Registry, Value, Vm};

/// C-locale `isspace()`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `php_url_decode`: `+` becomes a space and `%XX` a byte; malformed escapes
/// are kept verbatim.
fn url_decode(s: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'+' {
            out.push(b' ');
        } else if c == b'%' && i + 2 < s.len() {
            match (hex(s[i + 1]), hex(s[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 2;
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// `php_is_forbidden_variable_name`: mangling must not create a `__Host-` or
/// `__Secure-` prefix that the raw name did not have.
fn is_forbidden(mangled: &[u8], pre_mangled: &[u8]) -> bool {
    [&b"__Host-"[..], &b"__Secure-"[..]]
        .iter()
        .any(|prefix| mangled.starts_with(prefix) && !pre_mangled.starts_with(prefix))
}

/// One operation of `php_register_variable_ex` on the current array.
#[derive(Debug, PartialEq)]
enum Step {
    /// Descend into the array at this key (`None`: a new appended array),
    /// creating it or replacing a non-array.
    Container(Option<Vec<u8>>),
    /// Store the value at this key (`None`: append).
    Leaf(Option<Vec<u8>>),
    /// Too deeply nested: drop the whole top-level variable.
    NestingExceeded,
    /// Forbidden name: stop without storing.
    Stop,
}

/// Parses a (url-decoded) variable name like `a.b[x][][y` into its base name
/// and the steps `php_register_variable_ex` performs; `None` when the name is
/// empty. Mirrors php-src's in-place C-string surgery exactly (including
/// unmatched `[` handling and the single whitespace skipped after `[`).
fn plan_variable(var_name: &[u8], max_nesting: i64) -> Option<(Vec<u8>, Vec<Step>)> {
    // The name is a C string.
    let var_name = &var_name[..var_name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(var_name.len())];
    // Ignore leading spaces.
    let pre = &var_name[var_name
        .iter()
        .position(|&c| c != b' ')
        .unwrap_or(var_name.len())..];
    let mut buf = pre.to_vec();
    buf.push(0);
    let cstrlen = |buf: &[u8], from: usize| buf[from..].iter().position(|&c| c == 0).unwrap_or(0);

    // Spaces and dots become underscores up to the first `[`.
    let mut p = 0;
    let mut is_array = false;
    while buf[p] != 0 {
        match buf[p] {
            b' ' | b'.' => buf[p] = b'_',
            b'[' => {
                is_array = true;
                buf[p] = 0;
                break;
            }
            _ => {}
        }
        p += 1;
    }
    let var_len = p;
    if var_len == 0 {
        return None;
    }
    let base = buf[..var_len].to_vec();

    let key = |buf: &[u8], idx: Option<(usize, usize)>| idx.map(|(s, l)| buf[s..s + l].to_vec());
    let checked = |k: Option<Vec<u8>>, f: fn(Option<Vec<u8>>) -> Step| match k {
        Some(k) if is_forbidden(&k, pre) => Step::Stop,
        k => f(k),
    };

    let mut steps = Vec::new();
    let mut index = Some((0usize, var_len));
    if !is_array {
        steps.push(checked(key(&buf, index), Step::Leaf));
        return Some((base, steps));
    }
    let mut ip = p;
    let mut nest_level = 0i64;
    loop {
        nest_level += 1;
        if nest_level > max_nesting {
            steps.push(Step::NestingExceeded);
            return Some((base, steps));
        }
        ip += 1;
        let index_s = ip;
        if is_space(buf[ip]) {
            ip += 1;
        }
        let new_index = if buf[ip] == b']' {
            None
        } else {
            let end = ip + cstrlen(&buf, ip);
            match buf[ip..end].iter().position(|&c| c == b']') {
                Some(off) => {
                    ip += off;
                    buf[ip] = 0;
                    Some((index_s, ip - index_s))
                }
                None => {
                    // Not an index: un-terminate the name and mangle the rest.
                    buf[index_s - 1] = b'_';
                    let mut q = index_s;
                    while buf[q] != 0 {
                        if matches!(buf[q], b' ' | b'.' | b'[') {
                            buf[q] = b'_';
                        }
                        q += 1;
                    }
                    let idx = index.map(|(s, _)| (s, cstrlen(&buf, s)));
                    steps.push(checked(key(&buf, idx), Step::Leaf));
                    return Some((base, steps));
                }
            }
        };
        let step = checked(key(&buf, index), Step::Container);
        let stop = step == Step::Stop;
        steps.push(step);
        if stop {
            return Some((base, steps));
        }
        index = new_index;
        ip += 1;
        if buf[ip] == b'[' {
            buf[ip] = 0;
        } else {
            steps.push(checked(key(&buf, index), Step::Leaf));
            return Some((base, steps));
        }
    }
}

/// Executes `steps` on `arr`; returns `true` when the nesting limit was hit.
fn apply_steps(arr: &mut PhpArray, steps: &[Step], val: Value) -> bool {
    let Some(step) = steps.first() else {
        return false;
    };
    match step {
        Step::Stop => false,
        Step::NestingExceeded => true,
        Step::Leaf(None) => {
            let _ = arr.append(val);
            false
        }
        Step::Leaf(Some(k)) => {
            arr.insert(ArrayKey::from_bytes(k), val);
            false
        }
        Step::Container(k) => {
            let slot = match k {
                None => match arr.append(Value::empty_array()) {
                    Ok(i) => arr.get_mut(&ArrayKey::Int(i)).expect("just appended"),
                    Err(_) => return false,
                },
                Some(k) => arr.entry_mut(ArrayKey::from_bytes(k)),
            };
            if !matches!(slot, Value::Array(_)) {
                *slot = Value::empty_array();
            }
            let Value::Array(inner) = slot else {
                unreachable!()
            };
            apply_steps(Rc::make_mut(inner), &steps[1..], val)
        }
    }
}

/// `php_register_variable_ex(var_name, val, track_vars_array)` for a plain
/// array target.
pub fn register_variable(
    vm: &mut Vm,
    var_name: &[u8],
    val: Value,
    track: &mut PhpArray,
) -> PResult<()> {
    let max_nesting = match vm.ini_get("max_input_nesting_level") {
        Some(_) => vm.ini_int("max_input_nesting_level"),
        None => 64,
    };
    let Some((base, steps)) = plan_variable(var_name, max_nesting) else {
        return Ok(());
    };
    if apply_steps(track, &steps, val) {
        track.remove(&ArrayKey::from_bytes(&base));
        // Not displayed, to avoid information disclosure.
        if vm.errors.display == DisplayErrors::Off {
            vm.warning(format!(
                "Input variable nesting level exceeded {max_nesting}. To increase the limit change max_input_nesting_level in php.ini."
            ))?;
        }
    }
    Ok(())
}

/// `php_default_treat_data(PARSE_STRING, ...)`: parses a query string into
/// `track` using `arg_separator.input` and the `max_input_vars` limit.
pub fn treat_data_string(vm: &mut Vm, input: &[u8], track: &mut PhpArray) -> PResult<()> {
    let separator = vm
        .ini_get("arg_separator.input")
        .map(|s| {
            let s = s.to_vec();
            s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())].to_vec()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| b"&".to_vec());
    let max_input_vars = match vm.ini_get("max_input_vars") {
        Some(_) => vm.ini_int("max_input_vars"),
        None => 1000,
    };
    // The input is a C string; `php_strtok_r` skips empty tokens.
    let input = &input[..input.iter().position(|&c| c == 0).unwrap_or(input.len())];
    let mut count: i64 = 0;
    for var in input
        .split(|c| separator.contains(c))
        .filter(|t| !t.is_empty())
    {
        count += 1;
        if count > max_input_vars {
            vm.warning(format!(
                "Input variables exceeded {max_input_vars}. To increase the limit change max_input_vars in php.ini."
            ))?;
            break;
        }
        let (name, val) = match var.iter().position(|&c| c == b'=') {
            Some(eq) => (&var[..eq], url_decode(&var[eq + 1..])),
            None => (var, Vec::new()),
        };
        let name = url_decode(name);
        register_variable(vm, &name, Value::String(PhpStr::from_vec(val)), track)?;
    }
    Ok(())
}

fn parse_str(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.str(0).clone();
    if input.contains(&0) {
        return Err(vm.argument_value_error(1, "must not contain any null bytes"));
    }
    let result = args.by_ref(1);
    // `zend_try_array_init`: the result is always a fresh array.
    result.set(Value::empty_array());
    let mut arr = PhpArray::new();
    let r = treat_data_string(vm, &input, &mut arr);
    result.set(Value::array(arr));
    r.map(|()| Value::Null)
}

pub static PARSE_STR: NativeFunction = NativeFunction {
    sig: "parse_str(string $string, &$result): void",
    handler: parse_str,
};

pub fn register(r: &mut Registry) {
    r.function(&PARSE_STR);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestVm, thrown};
    use php_runtime::PhpRef;

    /// Renders a parse result compactly: `{k: v, k: {..}}`.
    fn render(v: &Value) -> String {
        match v {
            Value::String(s) => format!("{:?}", s.to_string_lossy()),
            Value::Array(a) => {
                let items: Vec<String> = a
                    .iter()
                    .map(|(k, v)| match k {
                        ArrayKey::Int(i) => format!("{i}: {}", render(v)),
                        ArrayKey::Str(s) => format!("{:?}: {}", s.to_string_lossy(), render(v)),
                    })
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            v => format!("{v:?}"),
        }
    }

    fn parse(t: &mut TestVm, s: &str) -> String {
        let r = PhpRef::new(Value::Int(99));
        let ret = t
            .call("parse_str", vec![Value::str(s), Value::Ref(r.clone())])
            .unwrap();
        assert!(ret.is_null());
        render(&r.get())
    }

    #[test]
    fn basic1() {
        let mut t = TestVm::new();
        assert_eq!(
            parse(&mut t, "first=val1&second=val2&third=val3"),
            r#"{"first": "val1", "second": "val2", "third": "val3"}"#
        );
        // bug77439 / parse_str_memory_error
        assert_eq!(parse(&mut t, "a[1]=1"), r#"{"a": {1: "1"}}"#);
        assert_eq!(parse(&mut t, "abcd=1"), r#"{"abcd": "1"}"#);
        assert_eq!(parse(&mut t, ""), "{}");
        assert_eq!(parse(&mut t, "&&a&&b=&"), r#"{"a": "", "b": ""}"#);
    }

    #[test]
    fn custom_separator() {
        // parse_str_basic2.phpt
        let mut t = TestVm::new();
        t.vm.ini_set_system("arg_separator.input", "/");
        assert_eq!(
            parse(&mut t, "first=val1/second=val2/third=val3"),
            r#"{"first": "val1", "second": "val2", "third": "val3"}"#
        );
        t.vm.ini_set_system("arg_separator.input", ";&");
        assert_eq!(
            parse(&mut t, "a=1;b=2&c=3"),
            r#"{"a": "1", "b": "2", "c": "3"}"#
        );
    }

    #[test]
    fn basic3_arrays_and_decoding() {
        // parse_str_basic3.phpt
        let mut t = TestVm::new();
        assert_eq!(
            parse(
                &mut t,
                "first=abc&a[]=123&a[]=false&b[]=str&c[]=3.5&a[]=last"
            ),
            r#"{"first": "abc", "a": {0: "123", 1: "false", 2: "last"}, "b": {0: "str"}, "c": {0: "3.5"}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[1]=sid&arr[4]=bill"),
            r#"{"arr": {1: "sid", 4: "bill"}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[first]=sid&arr[forth]=bill"),
            r#"{"arr": {"first": "sid", "forth": "bill"}}"#
        );
        assert_eq!(
            parse(
                &mut t,
                "a=%3c%3d%3d%20%20foo+bar++%3d%3d%3e&b=%23%23%23Hello+World%23%23%23"
            ),
            r####"{"a": "<==  foo bar  ==>", "b": "###Hello World###"}"####
        );
        assert_eq!(
            parse(&mut t, "firstname=Bill&surname=O%27Reilly"),
            r#"{"firstname": "Bill", "surname": "O'Reilly"}"#
        );
        assert_eq!(parse(&mut t, "sum=10%5c2%3d5"), r#"{"sum": "10\\2=5"}"#);
        assert_eq!(
            parse(
                &mut t,
                "str=A%20string%20with%20containing%20%00%00%00%20nulls"
            ),
            r#"{"str": "A string with containing \0\0\0 nulls"}"#
        );
        assert_eq!(
            parse(&mut t, "arr[3][4]=sid&arr[3][6]=fred"),
            r#"{"arr": {3: {4: "sid", 6: "fred"}}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[][]=sid&arr[][]=fred"),
            r#"{"arr": {0: {0: "sid"}, 1: {0: "fred"}}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[one][four]=sid&arr[three][six]=fred"),
            r#"{"arr": {"one": {"four": "sid"}, "three": {"six": "fred"}}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[1][2][3]=sid&arr[1][2][6]=fred"),
            r#"{"arr": {1: {2: {3: "sid", 6: "fred"}}}}"#
        );
    }

    #[test]
    fn basic4_malformed() {
        // parse_str_basic4.phpt
        let mut t = TestVm::new();
        assert_eq!(
            parse(&mut t, "arr[1=sid&arr[4][2=fred"),
            r#"{"arr_1": "sid", "arr": {4: "fred"}}"#
        );
        assert_eq!(
            parse(&mut t, "arr1]=sid&arr[4]2]=fred"),
            r#"{"arr1]": "sid", "arr": {4: "fred"}}"#
        );
        assert_eq!(
            parse(&mut t, "arr[one=sid&arr[4][two=fred"),
            r#"{"arr_one": "sid", "arr": {4: "fred"}}"#
        );
        assert_eq!(
            parse(&mut t, "first=%41&second=%a&third=%b"),
            r#"{"first": "A", "second": "%a", "third": "%b"}"#
        );
        assert_eq!(
            parse(&mut t, "arr.test[1]=sid&arr test[4][two]=fred"),
            r#"{"arr_test": {1: "sid", 4: {"two": "fred"}}}"#
        );
    }

    #[test]
    fn name_edge_cases() {
        let mut t = TestVm::new();
        // Leading spaces are skipped, empty names dropped.
        assert_eq!(parse(&mut t, "  a=1&=2&+=3&[x]=4"), r#"{"a": "1"}"#);
        // A NUL in the decoded name ends it; values are binary safe.
        assert_eq!(parse(&mut t, "a%00b=1"), r#"{"a": "1"}"#);
        // Unmatched `[` with mangling of the remainder.
        assert_eq!(parse(&mut t, "a[b.c d[e=1"), r#"{"a_b_c_d_e": "1"}"#);
        assert_eq!(parse(&mut t, "a[=1"), r#"{"a_": "1"}"#);
        // Previous level `[]` then an unmatched `[`: appends.
        assert_eq!(parse(&mut t, "a[][x=1"), r#"{"a": {0: "1"}}"#);
        // One whitespace after `[` is skipped only before `]`.
        assert_eq!(
            parse(&mut t, "a[ ]=1&a[ b]=2"),
            r#"{"a": {0: "1", " b": "2"}}"#
        );
        // Overwriting a scalar with an array and vice versa.
        assert_eq!(parse(&mut t, "a=1&a[x]=2"), r#"{"a": {"x": "2"}}"#);
        assert_eq!(parse(&mut t, "a[x]=2&a=1"), r#"{"a": "1"}"#);
        // Numeric keys are normalised, others are not.
        assert_eq!(
            parse(&mut t, "5=a&-3=b&07=c&a[1.5]=d"),
            r#"{5: "a", -3: "b", "07": "c", "a": {"1.5": "d"}}"#
        );
        // Mangling that creates a __Host- prefix is rejected.
        assert_eq!(
            parse(&mut t, "..Host-x=1&__Host-y=2"),
            r#"{"__Host-y": "2"}"#
        );
        assert_eq!(parse(&mut t, "a[__Host-x]=1"), r#"{"a": {}}"#);
    }

    #[test]
    fn nesting_limit() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("max_input_nesting_level", "2");
        assert_eq!(
            parse(&mut t, "a[b]=1&c[d][e]=2"),
            r#"{"a": {"b": "1"}, "c": {"d": {"e": "2"}}}"#
        );
        // Exceeding the limit drops the whole top-level variable.
        assert_eq!(parse(&mut t, "a[b]=1&a[x][y][z]=2&q=3"), r#"{"q": "3"}"#);
        // The warning is only shown when display_errors is off.
        assert_eq!(t.output(), "");
    }

    #[test]
    fn max_input_vars() {
        let mut t = TestVm::new();
        t.vm.ini_set_system("max_input_vars", "2");
        assert_eq!(parse(&mut t, "a=1&b=2&c=3&d=4"), r#"{"a": "1", "b": "2"}"#);
        assert_eq!(
            t.output(),
            "\nWarning: parse_str(): Input variables exceeded 2. To increase the limit change max_input_vars in php.ini. in /t.php on line 3\n"
        );
    }

    #[test]
    fn null_bytes() {
        // parse_str_null_bytes.phpt
        let mut t = TestVm::new();
        let r = PhpRef::new(Value::Null);
        let res = t.call("parse_str", vec![Value::bytes(b"a=1\0&b=2"), Value::Ref(r)]);
        assert_eq!(
            thrown(res),
            (
                "ValueError".into(),
                "parse_str(): Argument #1 ($string) must not contain any null bytes".into()
            )
        );
    }

    #[test]
    fn plan_steps() {
        assert_eq!(plan_variable(b"", 64), None);
        assert_eq!(plan_variable(b"   ", 64), None);
        let (base, steps) = plan_variable(b"a[b][]", 64).unwrap();
        assert_eq!(base, b"a");
        assert_eq!(
            steps,
            [
                Step::Container(Some(b"a".to_vec())),
                Step::Container(Some(b"b".to_vec())),
                Step::Leaf(None)
            ]
        );
        let (_, steps) = plan_variable(b"a[b]", 0).unwrap();
        assert_eq!(steps, [Step::NestingExceeded]);
    }
}
