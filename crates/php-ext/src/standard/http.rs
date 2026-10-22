//! ext/standard/http.c

use php_runtime::value::convert::object_property_table;
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value,
    Vm,
};

use super::basic::ini::parse_quantity;

pub const PHP_QUERY_RFC1738: i64 = 1;
pub const PHP_QUERY_RFC3986: i64 = 2;

/// `BG(last_http_headers)`, set by the http stream wrapper.
#[derive(Default)]
struct HttpState {
    last_response_headers: Option<Value>,
}

/// Records the response headers of the last HTTP request
/// (`http_get_last_response_headers()`); the http stream wrapper calls it.
pub fn set_last_response_headers(vm: &mut Vm, headers: Option<Value>) {
    vm.ext_data::<HttpState>().last_response_headers = headers;
}

/// `php_url_encode_to_smart_str()`: `urlencode()` or, when `raw`,
/// `rawurlencode()`.
fn url_encode_to(out: &mut Vec<u8>, s: &[u8], raw: bool) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &c in s {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.') || (raw && c == b'~') {
            out.push(c);
        } else if c == b' ' && !raw {
            out.push(b'+');
        } else {
            out.push(b'%');
            out.push(HEX[(c >> 4) as usize]);
            out.push(HEX[(c & 0xf) as usize]);
        }
    }
}

fn url_encode(s: &[u8], raw: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    url_encode_to(&mut out, s, raw);
    out
}

struct Encoder {
    enc_raw: bool,
    arg_sep: PhpStr,
    /// The class code runs in (`zend_check_property_access`).
    scope: Option<ClassRef>,
    max_depth: usize,
}

/// The unmangled name of a property-table key, or `None` when the property
/// is not accessible from `scope` (`zend_check_property_access`).
fn visible_property_name<'a>(
    obj: &ObjRef,
    key: &'a [u8],
    scope: Option<&ClassRef>,
) -> Option<&'a [u8]> {
    if key.first() != Some(&0) {
        return Some(key);
    }
    let rest = &key[1..];
    let nul = rest.iter().position(|&c| c == 0)?;
    let (class, name) = (&rest[..nul], &rest[nul + 1..]);
    let scope = scope?;
    if class == b"*" {
        // protected: scope and object class must be related
        let ok = obj.class().instance_of(scope) || scope.instance_of(obj.class());
        ok.then_some(name)
    } else {
        // private: only the declaring class sees it
        scope.name.eq_ignore_ascii_case(class).then_some(name)
    }
}

impl Encoder {
    fn encode_scalar(
        &self,
        vm: &mut Vm,
        scalar: &Value,
        out: &mut Vec<u8>,
        index: Option<&ArrayKey>,
        num_prefix: Option<&[u8]>,
        key_prefix: Option<&[u8]>,
    ) -> PResult<()> {
        if !out.is_empty() {
            out.extend_from_slice(&self.arg_sep);
        }
        // Simple key=value
        if let Some(p) = key_prefix {
            out.extend_from_slice(p);
        }
        match index {
            Some(ArrayKey::Str(s)) => url_encode_to(out, s, self.enc_raw),
            Some(ArrayKey::Int(i)) => {
                if let Some(p) = num_prefix {
                    out.extend_from_slice(p);
                }
                out.extend_from_slice(i.to_string().as_bytes());
            }
            None => {}
        }
        if key_prefix.is_some() {
            out.extend_from_slice(b"%5D");
        }
        out.push(b'=');

        let mut scalar = scalar.clone();
        loop {
            match &scalar {
                Value::String(s) => url_encode_to(out, s, self.enc_raw),
                Value::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
                Value::Float(d) => {
                    let s = vm.float_to_string(*d)?;
                    url_encode_to(out, &s, self.enc_raw);
                }
                Value::Bool(false) => out.push(b'0'),
                Value::Bool(true) => out.push(b'1'),
                Value::Object(o) => {
                    // only enums reach this point
                    let ce = o.class().clone();
                    match o.get_prop(b"value") {
                        Some(v) if ce.is_enum() => {
                            scalar = v;
                            continue;
                        }
                        _ => {
                            return Err(vm.throw_value_error(format!(
                                "Unbacked enum {} cannot be converted to a string",
                                ce.name
                            )));
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
    }

    /// `php_url_encode_hash_ex()`, iterating with an explicit stack so that
    /// deeply nested data cannot overflow the native stack.
    fn run(
        &mut self,
        vm: &mut Vm,
        table: &PhpArray,
        id: usize,
        out: &mut Vec<u8>,
        num_prefix: Option<Vec<u8>>,
        object: Option<ObjRef>,
    ) -> PResult<()> {
        struct Level {
            entries: std::vec::IntoIter<(ArrayKey, Value)>,
            id: usize,
            key_prefix: Option<Vec<u8>>,
            num_prefix: Option<Vec<u8>>,
            object: Option<ObjRef>,
        }
        let entries = |t: &PhpArray| -> std::vec::IntoIter<(ArrayKey, Value)> {
            t.iter()
                .map(|(k, v)| (k, v.clone()))
                .collect::<Vec<_>>()
                .into_iter()
        };
        let mut stack = vec![Level {
            entries: entries(table),
            id,
            key_prefix: None,
            num_prefix,
            object,
        }];

        while let Some(level) = stack.last_mut() {
            let Some((key, zdata)) = level.entries.next() else {
                stack.pop();
                continue;
            };
            // handling of private & protected object properties
            let prop_key = match (&key, &level.object) {
                (ArrayKey::Str(k), Some(o)) => {
                    match visible_property_name(o, k, self.scope.as_ref()) {
                        Some(name) => ArrayKey::Str(PhpStr::from(name)),
                        // property not visible in this scope
                        None => continue,
                    }
                }
                _ => key.clone(),
            };

            let zdata = zdata.unref();
            let is_enum = matches!(&zdata, Value::Object(o) if o.class().is_enum());
            let nested_object = matches!(&zdata, Value::Object(_)) && !is_enum;
            if !(matches!(zdata, Value::Array(_)) || nested_object) {
                if matches!(zdata, Value::Null | Value::Resource(_) | Value::Undef) {
                    // Skip these types
                    continue;
                }
                self.encode_scalar(
                    vm,
                    &zdata,
                    out,
                    Some(&prop_key),
                    level.num_prefix.as_deref(),
                    level.key_prefix.as_deref(),
                )?;
                continue;
            }

            if nested_object {
                vm.deprecated(
                    "object values within argument #1 $data to http_build_query() being interpreted as arrays is deprecated, instead the $data argument should be preprocessed with get_object_vars()",
                )?;
            }
            let key_prefix = level.key_prefix.as_deref();
            let num_prefix = level.num_prefix.as_deref();
            let new_prefix: Vec<u8> = match &prop_key {
                ArrayKey::Str(name) => {
                    let encoded = url_encode(name, self.enc_raw);
                    match key_prefix {
                        Some(kp) => [kp, &encoded, b"%5D%5B"].concat(),
                        None => [&encoded[..], b"%5B"].concat(),
                    }
                }
                ArrayKey::Int(idx) => {
                    let idx = idx.to_string();
                    match (key_prefix, num_prefix) {
                        (Some(kp), Some(np)) => [kp, np, idx.as_bytes(), b"%5D%5B"].concat(),
                        (Some(kp), None) => [kp, idx.as_bytes(), b"%5D%5B"].concat(),
                        (None, Some(np)) => [np, idx.as_bytes(), b"%5B"].concat(),
                        (None, None) => [idx.as_bytes(), b"%5B"].concat(),
                    }
                }
            };
            let (ents, id, object) = match &zdata {
                Value::Array(a) => (entries(a), std::rc::Rc::as_ptr(a) as usize, None),
                Value::Object(o) => (
                    entries(&object_property_table(o)),
                    o.id() as usize * 2 + 1,
                    Some(o.clone()),
                ),
                _ => unreachable!(),
            };
            if stack.iter().any(|l| l.id == id) {
                // Prevent recursion
                continue;
            }
            // Very deeply structured data could overflow the stack.
            if stack.len() >= self.max_depth {
                return Err(vm.throw_error("Error", "Maximum call stack size reached."));
            }
            stack.push(Level {
                entries: ents,
                id,
                key_prefix: Some(new_prefix),
                num_prefix: None,
                object,
            });
        }
        Ok(())
    }
}

/// The class of the PHP code that called the active native function.
fn calling_scope(vm: &Vm) -> Option<ClassRef> {
    let scope_name = vm
        .frames
        .iter()
        .rev()
        .nth(1)
        .and_then(|f| f.function.as_ref())
        .and_then(|f| f.scope.clone())?;
    vm.find_class(scope_name.as_bytes())
}

/// The recursion limit standing in for `zend.max_allowed_stack_size`.
fn max_depth(vm: &Vm) -> usize {
    let limit = vm.ini_int("zend.max_allowed_stack_size");
    if limit > 0 {
        (limit as usize / 96).max(64)
    } else {
        20_000
    }
}

fn http_build_query(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let formdata = args.get(0).clone();
    let prefix = args.str(1).clone();
    let arg_sep = match args.str_opt(2) {
        Some(s) => s.clone(),
        None => {
            let s = vm.ini_get("arg_separator.output").unwrap_or_default();
            if s.is_empty() { PhpStr::from("&") } else { s }
        }
    };
    let enc_type = args.int(3);

    let mut enc = Encoder {
        enc_raw: enc_type == PHP_QUERY_RFC3986,
        arg_sep,
        scope: calling_scope(vm),
        max_depth: max_depth(vm),
    };
    let mut out = Vec::new();
    match &formdata {
        Value::Object(o) => {
            if o.class().is_enum() {
                return Err(vm.argument_type_error(
                    1,
                    &format!("must not be an enum, {} given", formdata.value_name()),
                ));
            }
            vm.deprecated(
                "Passing an object for argument #1 $data to http_build_query() is deprecated, call get_object_vars() first instead",
            )?;
            let table = object_property_table(o);
            let id = o.id() as usize * 2 + 1;
            enc.run(
                vm,
                &table,
                id,
                &mut out,
                Some(prefix.to_vec()),
                Some(o.clone()),
            )?;
        }
        Value::Array(a) => {
            let id = std::rc::Rc::as_ptr(a) as usize;
            enc.run(vm, a, id, &mut out, Some(prefix.to_vec()), None)?;
        }
        _ => unreachable!("array|object after ZPP"),
    }
    Ok(Value::String(PhpStr::from_vec(out)))
}

/// `request_parse_body()` option cache: validates `$options`.
fn check_request_parse_body_options(vm: &mut Vm, options: &PhpArray) -> PResult<()> {
    for (key, value) in options.iter() {
        let ArrayKey::Str(key) = &key else {
            return Err(vm.throw_value_error("Invalid integer key in $options argument"));
        };
        if key.is_empty() {
            return Err(vm.throw_value_error("Invalid empty string key in $options argument"));
        }
        let lk = key.to_ascii_lowercase();
        let known = matches!(
            lk.as_bytes(),
            b"max_file_uploads"
                | b"max_input_vars"
                | b"max_multipart_body_parts"
                | b"post_max_size"
                | b"upload_max_filesize"
        );
        if !known {
            return Err(vm.throw_value_error(format!("Invalid key \"{key}\" in $options argument")));
        }
        match &value.unref() {
            Value::String(s) => {
                let (_, err) = parse_quantity(s);
                if let Some(e) = err {
                    vm.engine_warning(e)?;
                }
            }
            Value::Int(_) => {}
            other => {
                return Err(vm.throw_value_error(format!(
                    "Invalid {} value in $options argument",
                    other.value_name()
                )));
            }
        }
    }
    Ok(())
}

fn request_parse_body(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    if let Some(options) = args.array_opt(0).cloned() {
        check_request_parse_body_options(vm, &options)?;
    }
    // The CLI has no request: there is no Content-Type to dispatch on.
    Err(vm.throw_error(
        "RequestParseBodyException",
        "Request does not provide a content type",
    ))
}

fn http_get_last_response_headers(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Ok(vm
        .ext_data::<HttpState>()
        .last_response_headers
        .clone()
        .unwrap_or(Value::Null))
}

fn http_clear_last_response_headers(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    vm.ext_data::<HttpState>().last_response_headers = None;
    Ok(Value::Null)
}

pub static HTTP_BUILD_QUERY: NativeFunction = NativeFunction {
    sig: "http_build_query(array|object $data, string $numeric_prefix = \"\", ?string $arg_separator = null, int $encoding_type = PHP_QUERY_RFC1738): string",
    handler: http_build_query,
};
pub static HTTP_GET_LAST_RESPONSE_HEADERS: NativeFunction = NativeFunction {
    sig: "http_get_last_response_headers(): ?array",
    handler: http_get_last_response_headers,
};
pub static HTTP_CLEAR_LAST_RESPONSE_HEADERS: NativeFunction = NativeFunction {
    sig: "http_clear_last_response_headers(): void",
    handler: http_clear_last_response_headers,
};
pub static REQUEST_PARSE_BODY: NativeFunction = NativeFunction {
    sig: "request_parse_body(?array $options = null): array",
    handler: request_parse_body,
};

pub fn register(r: &mut Registry) {
    r.constant("PHP_QUERY_RFC1738", PHP_QUERY_RFC1738);
    r.constant("PHP_QUERY_RFC3986", PHP_QUERY_RFC3986);
    r.functions(&[
        &HTTP_BUILD_QUERY,
        &HTTP_GET_LAST_RESPONSE_HEADERS,
        &HTTP_CLEAR_LAST_RESPONSE_HEADERS,
        &REQUEST_PARSE_BODY,
    ]);
}

#[cfg(test)]
mod tests;
