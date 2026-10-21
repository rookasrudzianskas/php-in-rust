//! Searching and reading arrays: `in_array`, `array_search`, `array_keys`,
//! `array_key_exists`, `array_key_first/last`, `array_first/last`,
//! `array_values`, `array_count_values`, `array_column`, `array_is_list`.

use php_runtime::class::member_flags;
use php_runtime::value::KeyRef;
use php_runtime::{Args, ArrayKey, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Value, Vm};

use super::util::copy_elem;

// ---------------------------------------------------------------------------
// in_array / array_search
// ---------------------------------------------------------------------------

/// `_php_search_array`: whether `needle` matches `entry` (loose or strict).
fn matches(vm: &mut Vm, needle: &Value, entry: &Value, strict: bool) -> PResult<bool> {
    if strict {
        let e = entry.deref();
        vm.is_identical(needle, &e)
    } else {
        vm.is_equal(needle, entry)
    }
}

fn search(vm: &mut Vm, args: &mut Args, want_key: bool) -> PResult<Value> {
    let needle = args.get(0).clone();
    let array = args.array(1).clone();
    let strict = args.bool(2);
    for (key, entry) in array.iter() {
        if matches(vm, &needle, entry, strict)? {
            return Ok(if want_key {
                key.into_value()
            } else {
                Value::Bool(true)
            });
        }
    }
    Ok(Value::Bool(false))
}

fn in_array(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    search(vm, args, false)
}

fn array_search(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    search(vm, args, true)
}

// ---------------------------------------------------------------------------
// array_keys / array_values / first / last
// ---------------------------------------------------------------------------

fn array_keys(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    if input.is_empty() {
        return Ok(Value::empty_array());
    }
    if !args.is_passed(1) {
        let keys: Vec<Value> = input.keys().map(ArrayKey::into_value).collect();
        return Ok(Value::array(PhpArray::from_values(keys)));
    }
    let search_value = args.get(1).clone();
    let strict = args.bool(2);
    let mut out = Vec::new();
    for (key, entry) in input.iter() {
        if matches(vm, &search_value, entry, strict)? {
            out.push(key.into_value());
        }
    }
    Ok(Value::array(PhpArray::from_values(out)))
}

fn array_key_first(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match args.array(0).first() {
        Some((k, _)) => k.into_value(),
        None => Value::Null,
    })
}

fn array_key_last(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match args.array(0).last() {
        Some((k, _)) => k.into_value(),
        None => Value::Null,
    })
}

fn array_first(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match args.array(0).first() {
        Some((_, v)) => v.unref(),
        None => Value::Null,
    })
}

fn array_last(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(match args.array(0).last() {
        Some((_, v)) => v.unref(),
        None => Value::Null,
    })
}

fn array_values(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    if input.is_empty() {
        return Ok(Value::empty_array());
    }
    // Vector-like packed arrays are returned as-is.
    if input.is_packed() && input.is_list() && input.next_free_raw() == input.len() as i64 {
        return Ok(Value::Array(input));
    }
    let vals: Vec<Value> = input.values().map(copy_elem).collect();
    Ok(Value::array(PhpArray::from_values(vals)))
}

fn array_is_list(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    Ok(Value::Bool(args.array(0).is_list()))
}

// ---------------------------------------------------------------------------
// array_key_exists
// ---------------------------------------------------------------------------

fn array_key_exists(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let key = args.get(0).clone();
    let array = args.array(1).clone();
    let found = match &key {
        Value::String(s) => array.get_sym(s).is_some(),
        Value::Int(i) => array.contains_int(*i),
        Value::Null => {
            vm.engine_deprecated(
                "Using null as the key parameter for array_key_exists() is deprecated, use an empty string instead",
            )?;
            array.get_str(b"").is_some()
        }
        Value::Float(d) => {
            let l = vm.dval_to_lval_safe(*d)?;
            array.contains_int(l)
        }
        Value::Bool(b) => array.contains_int(*b as i64),
        Value::Resource(r) => {
            let id = r.id();
            vm.engine_warning(format!(
                "Resource ID#{id} used as offset, casting to integer ({id})"
            ))?;
            array.contains_int(id)
        }
        _ => return Err(vm.argument_type_error(1, "must be a valid array offset type")),
    };
    Ok(Value::Bool(found))
}

// ---------------------------------------------------------------------------
// array_count_values
// ---------------------------------------------------------------------------

fn array_count_values(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let mut out = PhpArray::new();
    for entry in input.values() {
        let key = match &*entry.deref() {
            Value::Int(i) => ArrayKey::Int(*i),
            Value::String(s) => ArrayKey::from_php_str(s.clone()),
            _ => {
                vm.warning("Can only count string and integer values, entry skipped")?;
                continue;
            }
        };
        match out.get_mut(&key) {
            Some(Value::Int(n)) => *n += 1,
            _ => {
                out.insert(key, Value::Int(1));
            }
        }
    }
    Ok(Value::array(out))
}

// ---------------------------------------------------------------------------
// array_column
// ---------------------------------------------------------------------------

/// A normalised `array_column()` key argument.
struct ColumnKey {
    /// The original string (kept for object property names).
    name: Option<PhpStr>,
    long: i64,
    /// A string key that is not numeric.
    is_string_key: bool,
}

impl ColumnKey {
    fn new(v: &Value) -> Option<ColumnKey> {
        match v {
            Value::Int(i) => Some(ColumnKey {
                name: None,
                long: *i,
                is_string_key: false,
            }),
            Value::String(s) => Some(match ArrayKey::parse_numeric(s) {
                Some(n) => ColumnKey {
                    name: Some(s.clone()),
                    long: n,
                    is_string_key: false,
                },
                None => ColumnKey {
                    name: Some(s.clone()),
                    long: 0,
                    is_string_key: true,
                },
            }),
            _ => None,
        }
    }

    fn property_name(&self) -> PhpStr {
        match &self.name {
            Some(s) => s.clone(),
            None => PhpStr::from(self.long.to_string()),
        }
    }
}

/// Reads property `name` of `obj` as `array_column()` does: the property must
/// exist (or `__isset` must say so) and is then read (through `__get` if
/// needed). Only public properties are visible: the scope of the calling code
/// is not available to native functions.
// TODO(runtime): honour the calling scope for protected/private properties.
fn read_property(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Option<Value>> {
    if let Some(h) = obj.class().handlers.read_property {
        if let Some(v) = h(vm, obj, name)? {
            return Ok(Some(v.into_unref()));
        }
    }
    let class = obj.class().clone();
    let mut declared_hidden = false;
    if let Some(info) = class.find_property(name) {
        if !info.is_static() {
            if info.flags & member_flags::PUBLIC != 0 {
                let slot = obj.obj().props.borrow().get(info.slot).cloned();
                if let Some(v) = slot {
                    if !v.is_undef() {
                        return Ok(Some(v.into_unref()));
                    }
                }
            } else {
                declared_hidden = true;
            }
        }
    }
    if !declared_hidden {
        let dynamic = obj
            .obj()
            .dyn_props
            .borrow()
            .as_ref()
            .and_then(|d| d.get_by(KeyRef::Str(name)).cloned());
        if let Some(v) = dynamic {
            return Ok(Some(v.into_unref()));
        }
    }
    // has_property in ISSET mode calls __isset, then __get supplies the value.
    if class.magic.isset.is_some() {
        let isset = vm.call_method(obj, b"__isset", vec![Value::String(name.clone())])?;
        if vm.to_bool(&isset)? && class.magic.get.is_some() {
            let v = vm.call_method(obj, b"__get", vec![Value::String(name.clone())])?;
            return Ok(Some(v.into_unref()));
        }
    }
    Ok(None)
}

/// `array_column_fetch_prop`.
fn fetch_prop(vm: &mut Vm, data: &Value, key: &ColumnKey) -> PResult<Option<Value>> {
    match data {
        Value::Object(o) => read_property(vm, o, &key.property_name()),
        Value::Array(a) => {
            let v = if key.is_string_key {
                match &key.name {
                    Some(s) => a.get_str(s),
                    None => None,
                }
            } else {
                a.get_int(key.long)
            };
            Ok(v.map(Value::unref))
        }
        _ => Ok(None),
    }
}

/// `array_set_zval_key`.
fn set_zval_key(vm: &mut Vm, out: &mut PhpArray, key: &Value, val: Value) -> PResult<()> {
    let k = match key {
        Value::Null => ArrayKey::Str(PhpStr::new()),
        other => vm.to_array_key(other, php_runtime::value::convert::OffsetKind::Write)?,
    };
    out.insert(k, val);
    Ok(())
}

fn array_column(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let input = args.array(0).clone();
    let column = ColumnKey::new(args.get(1));
    let index = ColumnKey::new(args.get(2));

    if input.is_empty() {
        return Ok(Value::empty_array());
    }

    if column.is_none() && index.is_none() {
        let vals: Vec<Value> = input.values().map(Value::unref).collect();
        return Ok(Value::array(PhpArray::from_values(vals)));
    }

    let mut out = PhpArray::new();
    for data in input.values() {
        let data = data.unref();
        let colval = match &column {
            None => data.clone(),
            Some(c) => match fetch_prop(vm, &data, c)? {
                Some(v) => v,
                None => continue,
            },
        };
        match &index {
            None => {
                let _ = out.append(colval);
            }
            Some(ik) => match fetch_prop(vm, &data, ik)? {
                Some(keyval) => set_zval_key(vm, &mut out, &keyval, colval)?,
                None => {
                    let _ = out.append(colval);
                }
            },
        }
    }
    Ok(Value::array(out))
}

pub static FUNCTIONS: &[NativeFunction] = &[
    NativeFunction {
        sig: "in_array(mixed $needle, array $haystack, bool $strict = false): bool",
        handler: in_array,
    },
    NativeFunction {
        sig: "array_search(mixed $needle, array $haystack, bool $strict = false): int|string|false",
        handler: array_search,
    },
    NativeFunction {
        sig: "array_keys(array $array, mixed $filter_value = UNKNOWN, bool $strict = false): array",
        handler: array_keys,
    },
    NativeFunction {
        sig: "array_key_first(array $array): int|string|null",
        handler: array_key_first,
    },
    NativeFunction {
        sig: "array_key_last(array $array): int|string|null",
        handler: array_key_last,
    },
    NativeFunction {
        sig: "array_first(array $array): mixed",
        handler: array_first,
    },
    NativeFunction {
        sig: "array_last(array $array): mixed",
        handler: array_last,
    },
    NativeFunction {
        sig: "array_values(array $array): array",
        handler: array_values,
    },
    NativeFunction {
        sig: "array_count_values(array $array): array",
        handler: array_count_values,
    },
    NativeFunction {
        sig: "array_column(array $array, int|string|null $column_key, int|string|null $index_key = null): array",
        handler: array_column,
    },
    NativeFunction {
        sig: "array_key_exists($key, array $array): bool",
        handler: array_key_exists,
    },
    NativeFunction {
        sig: "key_exists($key, array $array): bool",
        handler: array_key_exists,
    },
    NativeFunction {
        sig: "array_is_list(array $array): bool",
        handler: array_is_list,
    },
];
