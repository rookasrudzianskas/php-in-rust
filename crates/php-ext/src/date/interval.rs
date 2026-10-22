//! `DateInterval` and the `date_interval_*` functions.

use php_runtime::class::{ClassEntry, PropPurpose};
use php_runtime::value::convert::object_property_table;
use php_runtime::{
    Args, ArrayKey, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value,
    Vm,
};
use php_timelib::{RelTime, UNSET, diff, interval_format, strtointerval, strtotime};

use super::common::{
    INTERVAL_CLASS, MALFORMED_INTERVAL_STRING_EXCEPTION, WAKEUP_DEPRECATION, add_common_properties,
    c_str, class, deprecated_method, first_error_message, has_debug_info,
    restore_custom_properties, this, tz_get, uninitialized_error,
};
use super::datetime::{PUB, STATIC, add_methods, nf, same_compare_handler, target};

/// `PHP_DATE_CIVIL`.
pub const CIVIL: i64 = 1;
/// `PHP_DATE_WALL`.
pub const WALL: i64 = 2;

/// The state of a `DateInterval` (`php_interval_obj`).
#[derive(Clone, Debug, Default)]
pub struct IntervalObj {
    pub diff: Option<RelTime>,
    pub civil_or_wall: i64,
    pub initialized: bool,
    pub from_string: bool,
    pub date_string: Option<PhpStr>,
}

/// `date_object_new_interval`.
fn create_interval_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(IntervalObj::default());
    Ok(obj)
}

/// The state of an interval object (a default, uninitialised one if the
/// object has none).
pub fn interval_of(obj: &ObjRef) -> IntervalObj {
    obj.native::<IntervalObj>()
        .map(|i| i.clone())
        .unwrap_or_default()
}

fn set_interval_of(obj: &ObjRef, state: IntervalObj) {
    let done = match obj.native_mut::<IntervalObj>() {
        Some(mut i) => {
            *i = state.clone();
            true
        }
        None => false,
    };
    if !done {
        obj.set_native(state);
    }
}

/// A new `DateInterval` holding `diff` (`from_string` when `date_string` is
/// given, as `date_interval_instantiate_from_time` does).
pub fn new_interval_object(
    vm: &mut Vm,
    diff: RelTime,
    civil_or_wall: i64,
    date_string: Option<PhpStr>,
) -> PResult<ObjRef> {
    let ce = class(vm, INTERVAL_CLASS);
    let obj = vm.instantiate(&ce)?;
    set_interval_of(
        &obj,
        IntervalObj {
            diff: Some(diff),
            civil_or_wall,
            initialized: true,
            from_string: date_string.is_some(),
            date_string,
        },
    );
    Ok(obj)
}

/// `date_object_clone_interval`.
fn clone_interval_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = create_interval_object(vm, &ce)?;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    set_interval_of(&new, interval_of(old));
    Ok(new)
}

/// `date_interval_compare_objects`: intervals are not comparable.
fn compare_intervals(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    if same_compare_handler(a, b).is_none() {
        return vm.std_compare_objects(a, b);
    }
    vm.engine_warning("Cannot compare DateInterval objects")?;
    Ok(1)
}

/// `date_interval_object_to_hash`.
fn interval_object_to_hash(intobj: &IntervalObj, props: &mut PhpArray) {
    let mut set = |k: &str, v: Value| {
        props.insert(ArrayKey::Str(PhpStr::from(k)), v);
    };
    if intobj.from_string {
        set("from_string", Value::Bool(true));
        set(
            "date_string",
            Value::String(intobj.date_string.clone().unwrap_or_default()),
        );
        return;
    }
    let d = intobj.diff.unwrap_or_default();
    set("y", Value::Int(d.y));
    set("m", Value::Int(d.m));
    set("d", Value::Int(d.d));
    set("h", Value::Int(d.h));
    set("i", Value::Int(d.i));
    set("s", Value::Int(d.s));
    set("f", Value::Float(d.us as f64 / 1_000_000.0));
    set("invert", Value::Int(i64::from(d.invert)));
    if d.days != UNSET {
        set("days", Value::Int(d.days));
    } else {
        set("days", Value::Bool(false));
    }
    set("from_string", Value::Bool(false));
}

/// `date_object_get_properties_interval`: writes the interval fields into
/// the object's property table (like php-src) and returns the table.
fn interval_properties(obj: &ObjRef) -> PhpArray {
    let intobj = interval_of(obj);
    if intobj.initialized {
        let mut dyn_props = obj.obj().dyn_props.borrow_mut();
        let table = dyn_props.get_or_insert_with(PhpArray::new);
        interval_object_to_hash(&intobj, table);
    }
    object_property_table(obj)
}

fn interval_properties_for(
    _vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    if purpose == PropPurpose::Debug && has_debug_info(obj.class()) {
        return Ok(None);
    }
    Ok(Some(interval_properties(obj)))
}

/// `date_interval_read_property`.
fn interval_read_property(_vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Option<Value>> {
    let intobj = interval_of(obj);
    if !intobj.initialized {
        return Ok(None);
    }
    let d = intobj.diff.unwrap_or_default();
    let mut value: i64 = -1;
    let mut fvalue: f64 = -1.0;
    match name.as_bytes() {
        b"y" => value = d.y,
        b"m" => value = d.m,
        b"d" => value = d.d,
        b"h" => value = d.h,
        b"i" => value = d.i,
        b"s" => value = d.s,
        b"f" => fvalue = d.us as f64 / 1_000_000.0,
        b"invert" => value = i64::from(d.invert),
        b"days" => value = d.days,
        _ => return Ok(None),
    }
    Ok(Some(if fvalue != -1.0 {
        Value::Float(fvalue)
    } else if value != UNSET {
        Value::Int(value)
    } else {
        Value::Bool(false)
    }))
}

/// `date_interval_write_property`.
fn interval_write_property(
    vm: &mut Vm,
    obj: &ObjRef,
    name: &PhpStr,
    value: Value,
) -> PResult<bool> {
    let mut intobj = interval_of(obj);
    if !intobj.initialized {
        return Ok(false);
    }
    let mut d = intobj.diff.unwrap_or_default();
    match name.as_bytes() {
        b"y" => d.y = vm.to_int(&value)?,
        b"m" => d.m = vm.to_int(&value)?,
        b"d" => d.d = vm.to_int(&value)?,
        b"h" => d.h = vm.to_int(&value)?,
        b"i" => d.i = vm.to_int(&value)?,
        b"s" => d.s = vm.to_int(&value)?,
        b"f" => {
            let f = vm.to_float(&value)?;
            d.us = vm.dval_to_lval(f * 1_000_000.0)?;
        }
        b"invert" => d.invert = vm.to_int(&value)? != 0,
        _ => return Ok(false),
    }
    intobj.diff = Some(d);
    set_interval_of(obj, intobj);
    Ok(true)
}

/// `date_interval_initialize`: parses an ISO 8601 duration.
fn date_interval_initialize(vm: &mut Vm, format: &[u8]) -> PResult<RelTime> {
    let r = strtointerval(format);
    let message = |prefix: &[u8]| {
        let mut m = prefix.to_vec();
        m.extend_from_slice(c_str(format));
        m.push(b')');
        PhpStr::from_vec(m)
    };
    if r.errors.error_count() > 0 {
        let msg = message(b"Unknown or bad format (");
        return Err(vm.throw_error(MALFORMED_INTERVAL_STRING_EXCEPTION, msg));
    }
    if let Some(p) = r.period {
        return Ok(p);
    }
    match (r.begin, r.end) {
        (Some(mut b), Some(mut e)) => {
            b.update_ts(None);
            e.update_ts(None);
            Ok(diff(&b, &e))
        }
        _ => {
            let msg = message(b"Failed to parse interval (");
            Err(vm.throw_error(MALFORMED_INTERVAL_STRING_EXCEPTION, msg))
        }
    }
}

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let spec = args.str(0).clone();
    let reltime = date_interval_initialize(vm, spec.as_bytes())?;
    let mut state = interval_of(&obj);
    state.diff = Some(reltime);
    state.initialized = true;
    state.civil_or_wall = WALL;
    set_interval_of(&obj, state);
    Ok(Value::Null)
}

/// `strtoll()` of a string: leading whitespace, sign and digits, saturating.
fn strtoll(s: &[u8]) -> i64 {
    let mut i = 0;
    while s.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let mut v: i64 = 0;
    while let Some(c) = s.get(i).filter(|c| c.is_ascii_digit()) {
        let digit = i64::from(c - b'0');
        v = match v.checked_mul(10).and_then(|v| {
            if neg {
                v.checked_sub(digit)
            } else {
                v.checked_add(digit)
            }
        }) {
            Some(v) => v,
            None => return if neg { i64::MIN } else { i64::MAX },
        };
        i += 1;
    }
    v
}

/// `Z_TYPE_P(z_arg) <= IS_STRING`: null, booleans, numbers and strings.
fn is_scalar_or_null(v: &Value) -> bool {
    matches!(
        v,
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::String(_)
    )
}

/// `php_date_interval_initialize_from_hash`.
fn interval_initialize_from_hash(vm: &mut Vm, obj: &ObjRef, myht: &PhpArray) -> PResult<()> {
    // If we have a date_string, use that instead
    if let Some(Value::String(date_str)) = myht.get_str(b"date_string") {
        let date_str = date_str.clone();
        let res = strtotime(date_str.as_bytes(), &tz_get);
        if res.errors.error_count() > 0 {
            let msg = first_error_message(
                b"Unknown or bad format ",
                date_str.as_bytes(),
                b" while unserializing",
                &res.errors,
            );
            return Err(vm.throw_error("Error", PhpStr::from_vec(msg)));
        }
        let mut state = interval_of(obj);
        state.diff = Some(res.time.relative);
        state.initialized = true;
        state.civil_or_wall = CIVIL;
        state.from_string = true;
        state.date_string = Some(date_str);
        set_interval_of(obj, state);
        return Ok(());
    }

    let mut d = RelTime::default();
    let long_prop = |vm: &mut Vm, key: &[u8], def: i64| -> PResult<i64> {
        match myht.get_str(key) {
            Some(v) if is_scalar_or_null(v) => vm.to_int(v),
            _ => Ok(def),
        }
    };
    d.y = long_prop(vm, b"y", -1)?;
    d.m = long_prop(vm, b"m", -1)?;
    d.d = long_prop(vm, b"d", -1)?;
    d.h = long_prop(vm, b"h", -1)?;
    d.i = long_prop(vm, b"i", -1)?;
    d.s = long_prop(vm, b"s", -1)?;
    if let Some(f) = myht.get_str(b"f") {
        let f = f.clone();
        let f = vm.to_float(&f)?;
        d.us = vm.dval_to_lval(f * 1_000_000.0)?;
    }
    d.weekday = long_prop(vm, b"weekday", -1)? as i32;
    d.weekday_behavior = long_prop(vm, b"weekday_behavior", -1)? as i32;
    d.first_last_day_of = long_prop(vm, b"first_last_day_of", -1)? as i32;
    d.invert = long_prop(vm, b"invert", 0)? as i32 != 0;
    d.days = match myht.get_str(b"days") {
        Some(Value::Bool(false)) => UNSET,
        Some(v) if is_scalar_or_null(v) => {
            let s = vm.to_php_string(v)?;
            strtoll(s.as_bytes())
        }
        _ => -1,
    };
    d.special.kind = long_prop(vm, b"special_type", 0)? as u32;
    d.special.amount = match myht.get_str(b"special_amount") {
        Some(v) if is_scalar_or_null(v) => {
            let s = vm.to_php_string(v)?;
            strtoll(s.as_bytes())
        }
        _ => -1,
    };
    d.have_weekday_relative = long_prop(vm, b"have_weekday_relative", 0)? as u32 != 0;
    d.have_special_relative = long_prop(vm, b"have_special_relative", 0)? as u32 != 0;
    let civil_or_wall = match myht.get_str(b"civil_or_wall") {
        Some(v) => {
            let v = v.clone();
            vm.to_int(&v)?
        }
        None => CIVIL,
    };
    let mut state = interval_of(obj);
    state.diff = Some(d);
    state.civil_or_wall = civil_or_wall;
    state.initialized = true;
    set_interval_of(obj, state);
    Ok(())
}

fn set_state(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ce = class(vm, INTERVAL_CLASS);
    let obj = vm.instantiate(&ce)?;
    let data = args.array(0).clone();
    interval_initialize_from_hash(vm, &obj, &data)?;
    Ok(Value::Object(obj))
}

fn checked_interval(vm: &mut Vm, obj: &ObjRef) -> PResult<IntervalObj> {
    let state = interval_of(obj);
    if !state.initialized {
        let ce = obj.class().clone();
        return Err(uninitialized_error(vm, &ce));
    }
    Ok(state)
}

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let state = checked_interval(vm, &obj)?;
    let mut a = PhpArray::new();
    interval_object_to_hash(&state, &mut a);
    add_common_properties(&mut a, &obj);
    Ok(Value::array(a))
}

/// `date_interval_is_internal_property`.
fn is_internal_property(name: &[u8]) -> bool {
    matches!(
        name,
        b"date_string"
            | b"from_string"
            | b"y"
            | b"m"
            | b"d"
            | b"h"
            | b"i"
            | b"s"
            | b"f"
            | b"invert"
            | b"days"
    )
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let data = args.array(0).clone();
    interval_initialize_from_hash(vm, &obj, &data)?;
    restore_custom_properties(vm, &obj, &data, is_internal_property)?;
    Ok(Value::Null)
}

fn wakeup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, INTERVAL_CLASS, "__wakeup", WAKEUP_DEPRECATION)?;
    let obj = this(args);
    let props = interval_properties(&obj);
    interval_initialize_from_hash(vm, &obj, &props)?;
    Ok(Value::Null)
}

/// Parses a relative date string for `createFromDateString()`: the parse
/// errors or the "non-relative elements" message on failure.
fn relative_from_string(s: &[u8]) -> Result<RelTime, Vec<u8>> {
    let res = strtotime(s, &tz_get);
    if res.errors.error_count() > 0 {
        return Err(first_error_message(
            b"Unknown or bad format ",
            s,
            b"",
            &res.errors,
        ));
    }
    let t = &res.time;
    if t.have_date != 0 || t.have_time != 0 || t.have_zone != 0 {
        let mut m = b"String '".to_vec();
        m.extend_from_slice(c_str(s));
        m.extend_from_slice(b"' contains non-relative elements");
        return Err(m);
    }
    Ok(t.relative)
}

fn date_interval_create_from_date_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    match relative_from_string(s.as_bytes()) {
        Ok(rel) => Ok(Value::Object(new_interval_object(vm, rel, CIVIL, Some(s))?)),
        Err(msg) => {
            vm.warning(PhpStr::from_vec(msg))?;
            Ok(Value::Bool(false))
        }
    }
}

fn create_from_date_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let s = args.str(0).clone();
    match relative_from_string(s.as_bytes()) {
        Ok(rel) => Ok(Value::Object(new_interval_object(vm, rel, CIVIL, Some(s))?)),
        Err(msg) => Err(vm.throw_error(MALFORMED_INTERVAL_STRING_EXCEPTION, PhpStr::from_vec(msg))),
    }
}

/// `date_interval_format()` / `DateInterval::format()`.
fn date_interval_format(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let state = checked_interval(vm, &obj)?;
    let format = args.str(off).clone();
    Ok(Value::from(interval_format(
        format.as_bytes(),
        &state.diff.unwrap_or_default(),
    )))
}

nf!(
    DATE_INTERVAL_CREATE_FROM_DATE_STRING,
    "function date_interval_create_from_date_string(string $datetime): DateInterval|false {}",
    date_interval_create_from_date_string
);
nf!(
    DATE_INTERVAL_FORMAT,
    "function date_interval_format(DateInterval $object, string $format): string {}",
    date_interval_format
);

nf!(
    DI_CONSTRUCT,
    "public function __construct(string $duration) {}",
    construct
);
nf!(
    DI_CREATE_FROM_DATE_STRING,
    "public static function createFromDateString(string $datetime): DateInterval {}",
    create_from_date_string
);
nf!(
    DI_FORMAT,
    "public function format(string $format): string {}",
    date_interval_format
);
nf!(
    DI_SERIALIZE,
    "public function __serialize(): array;",
    serialize
);
nf!(
    DI_UNSERIALIZE,
    "public function __unserialize(array $data): void;",
    unserialize
);
nf!(DI_WAKEUP, "public function __wakeup(): void {}", wakeup);
nf!(
    DI_SET_STATE,
    "public static function __set_state(array $array): static {}",
    set_state
);

pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(INTERVAL_CLASS, 0);
    add_methods(
        &mut ce,
        INTERVAL_CLASS,
        &[
            (&DI_CONSTRUCT, PUB | php_runtime::class::fn_flags::CTOR),
            (&DI_CREATE_FROM_DATE_STRING, STATIC),
            (&DI_FORMAT, PUB),
            (&DI_SERIALIZE, PUB),
            (&DI_UNSERIALIZE, PUB),
            (&DI_WAKEUP, PUB),
            (&DI_SET_STATE, STATIC),
        ],
    );
    ce.handlers.create_object = Some(create_interval_object);
    ce.handlers.clone_obj = Some(clone_interval_object);
    ce.handlers.compare = Some(compare_intervals);
    ce.handlers.get_properties_for = Some(interval_properties_for);
    ce.handlers.read_property = Some(interval_read_property);
    ce.handlers.write_property = Some(interval_write_property);
    r.class(ce);
}

#[cfg(test)]
mod tests {
    use super::strtoll;

    #[test]
    fn strtoll_like_c() {
        assert_eq!(strtoll(b" 42abc"), 42);
        assert_eq!(strtoll(b"-7"), -7);
        assert_eq!(strtoll(b"1.9"), 1);
        assert_eq!(strtoll(b"x"), 0);
        assert_eq!(strtoll(b"99999999999999999999"), i64::MAX);
        assert_eq!(strtoll(b"-99999999999999999999"), i64::MIN);
    }
}
