//! `DatePeriod` and its iterator.
//!
//! `DatePeriod::getIterator()` returns an `InternalIterator` (the class
//! `zend_create_internal_iterator_zval` produces) whose methods drive the
//! `date_period_it_*` callbacks, so `foreach` over a period works through the
//! regular `IteratorAggregate` protocol.

use php_runtime::class::{
    ClassConstant, ClassEntry, PropPurpose, PropertyInfo, class_flags, fn_flags, member_flags,
};
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::object_property_table;
use php_runtime::{
    Args, ClassRef, Flow, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};
use php_timelib::{RelTime, Time, strtointerval};

use super::common::{
    DATE_INTERFACE, IMMUTABLE_CLASS, INTERVAL_CLASS, MALFORMED_PERIOD_STRING_EXCEPTION,
    MALFORMED_STRING_EXCEPTION, PERIOD_CLASS, WAKEUP_DEPRECATION, add_common_properties, c_str,
    called_class, class, deprecated_method, has_debug_info, hash_update, object_init_ex,
    restore_custom_properties, this, uninitialized_error,
};
use super::datetime::{PUB, STATIC, add_methods, base_date_class, date_time, new_date_object, nf};
use super::interval::{interval_of, new_interval_object};

/// `PHP_DATE_PERIOD_EXCLUDE_START_DATE`.
pub const EXCLUDE_START_DATE: i64 = 0x0001;
/// `PHP_DATE_PERIOD_INCLUDE_END_DATE`.
pub const INCLUDE_END_DATE: i64 = 0x0002;

/// The iterator class returned by `getIterator()`.
pub const ITERATOR_CLASS: &str = "InternalIterator";

/// The state of a `DatePeriod` (`php_period_obj`).
#[derive(Clone, Debug, Default)]
pub struct PeriodObj {
    pub start: Option<Time>,
    pub start_ce: Option<ClassRef>,
    pub current: Option<Time>,
    pub end: Option<Time>,
    pub interval: Option<RelTime>,
    pub recurrences: i64,
    pub initialized: bool,
    pub include_start_date: bool,
    pub include_end_date: bool,
}

fn create_period_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(PeriodObj::default());
    Ok(obj)
}

/// The state of a period object.
pub fn period_of(obj: &ObjRef) -> PeriodObj {
    obj.native::<PeriodObj>()
        .map(|p| p.clone())
        .unwrap_or_default()
}

fn set_period_of(obj: &ObjRef, state: PeriodObj) {
    let done = match obj.native_mut::<PeriodObj>() {
        Some(mut p) => {
            *p = state.clone();
            true
        }
        None => false,
    };
    if !done {
        obj.set_native(state);
    }
}

/// `date_object_clone_period`.
fn clone_period_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = create_period_object(vm, &ce)?;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    set_period_of(&new, period_of(old));
    Ok(new)
}

/// `create_date_period_datetime`.
fn period_datetime(vm: &mut Vm, time: &Option<Time>, ce: &Option<ClassRef>) -> PResult<Value> {
    match (time, ce) {
        (Some(t), Some(ce)) => Ok(Value::Object(new_date_object(vm, ce, t.clone())?)),
        (Some(t), None) => {
            let ce = class(vm, super::common::DATE_CLASS);
            Ok(Value::Object(new_date_object(vm, &ce, t.clone())?))
        }
        _ => Ok(Value::Null),
    }
}

/// `create_date_period_interval`.
fn period_interval(vm: &mut Vm, interval: &Option<RelTime>) -> PResult<Value> {
    match interval {
        Some(i) => Ok(Value::Object(new_interval_object(vm, *i, 0, None)?)),
        None => Ok(Value::Null),
    }
}

/// `date_period_object_to_hash`.
fn period_object_to_hash(vm: &mut Vm, p: &PeriodObj, props: &mut PhpArray) -> PResult<()> {
    let v = period_datetime(vm, &p.start, &p.start_ce)?;
    hash_update(props, "start", v);
    let v = period_datetime(vm, &p.current, &p.start_ce)?;
    hash_update(props, "current", v);
    let v = period_datetime(vm, &p.end, &p.start_ce)?;
    hash_update(props, "end", v);
    let v = period_interval(vm, &p.interval)?;
    hash_update(props, "interval", v);
    hash_update(props, "recurrences", p.recurrences);
    hash_update(props, "include_start_date", p.include_start_date);
    hash_update(props, "include_end_date", p.include_end_date);
    Ok(())
}

/// `date_period_get_properties_for`.
fn period_properties_for(
    vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    match purpose {
        PropPurpose::Debug if has_debug_info(obj.class()) => return Ok(None),
        PropPurpose::GetObjectVars => return Ok(None),
        _ => {}
    }
    let mut props = object_property_table(obj);
    let p = period_of(obj);
    if p.initialized {
        period_object_to_hash(vm, &p, &mut props)?;
    }
    Ok(Some(props))
}

/// `date_period_is_internal_property`.
fn is_internal_property(name: &[u8]) -> bool {
    matches!(
        name,
        b"start"
            | b"current"
            | b"end"
            | b"interval"
            | b"recurrences"
            | b"include_start_date"
            | b"include_end_date"
    )
}

/// `date_period_read_property`.
fn period_read_property(vm: &mut Vm, obj: &ObjRef, name: &PhpStr) -> PResult<Option<Value>> {
    if !is_internal_property(name.as_bytes()) {
        return Ok(None);
    }
    let p = period_of(obj);
    Ok(Some(match name.as_bytes() {
        b"start" => period_datetime(vm, &p.start, &p.start_ce)?,
        b"current" => period_datetime(vm, &p.current, &p.start_ce)?,
        b"end" => period_datetime(vm, &p.end, &p.start_ce)?,
        b"interval" => period_interval(vm, &p.interval)?,
        b"recurrences" => Value::Int(p.recurrences),
        b"include_start_date" => Value::Bool(p.include_start_date),
        _ => Value::Bool(p.include_end_date),
    }))
}

/// `date_period_write_property`: the internal properties are read-only.
fn period_write_property(vm: &mut Vm, _obj: &ObjRef, name: &PhpStr, _v: Value) -> PResult<bool> {
    if is_internal_property(name.as_bytes()) {
        return Err(vm.throw_error(
            "Error",
            format!("Cannot modify readonly property DatePeriod::${name}"),
        ));
    }
    Ok(false)
}

/// `date_period_advance`.
fn period_advance(it_time: &mut Time, interval: &RelTime) {
    it_time.have_relative = true;
    it_time.relative = *interval;
    it_time.sse_uptodate = false;
    it_time.update_ts(None);
    it_time.update_from_sse();
}

/// `date_period_initialize`: the parts of an ISO 8601 interval string.
fn period_initialize_iso(vm: &mut Vm, p: &mut PeriodObj, isostr: &[u8]) -> PResult<i64> {
    let r = strtointerval(isostr);
    if r.errors.error_count() > 0 {
        let mut msg = b"Unknown or bad format (".to_vec();
        msg.extend_from_slice(c_str(isostr));
        msg.push(b')');
        return Err(vm.throw_error(MALFORMED_PERIOD_STRING_EXCEPTION, PhpStr::from_vec(msg)));
    }
    p.start = r.begin;
    p.end = r.end;
    p.interval = r.period;
    Ok(i64::from(r.recurrences.unwrap_or(0)))
}

/// `date_period_init_iso8601_string`.
fn period_init_iso8601_string(
    vm: &mut Vm,
    p: &mut PeriodObj,
    base_ce: &ClassRef,
    isostr: &[u8],
) -> PResult<i64> {
    let recurrences = period_initialize_iso(vm, p, isostr)?;
    let fail = |vm: &mut Vm, what: &str| -> Flow {
        let mut msg = format!(
            "{}(): ISO interval must contain {what}, \"",
            vm.current_function_display_name()
        )
        .into_bytes();
        msg.extend_from_slice(c_str(isostr));
        msg.extend_from_slice(b"\" given");
        vm.throw_error(MALFORMED_PERIOD_STRING_EXCEPTION, PhpStr::from_vec(msg))
    };
    if p.start.is_none() {
        return Err(fail(vm, "a start date"));
    }
    if p.interval.is_none() {
        return Err(fail(vm, "an interval"));
    }
    if p.end.is_none() && recurrences == 0 {
        return Err(fail(vm, "an end date or a recurrence count"));
    }
    if let Some(s) = p.start.as_mut() {
        s.update_ts(None);
    }
    if let Some(e) = p.end.as_mut() {
        e.update_ts(None);
    }
    p.start_ce = Some(base_ce.clone());
    Ok(recurrences)
}

/// `date_period_init_finish`.
fn period_init_finish(
    vm: &mut Vm,
    p: &mut PeriodObj,
    options: i64,
    mut recurrences: i64,
) -> PResult<()> {
    let max_recurrences: i64 = i64::from(i32::MAX) - 8;
    if p.end.is_none() && (recurrences < 1 || recurrences > max_recurrences) {
        let msg = format!(
            "{}(): Recurrence count must be greater or equal to 1 and lower than {}",
            vm.current_function_display_name(),
            max_recurrences + 1
        );
        return Err(vm.throw_error(MALFORMED_PERIOD_STRING_EXCEPTION, msg));
    }
    p.include_start_date = options & EXCLUDE_START_DATE == 0;
    p.include_end_date = options & INCLUDE_END_DATE != 0;
    recurrences += i64::from(p.include_start_date) + i64::from(p.include_end_date);
    if recurrences > max_recurrences {
        let msg = format!(
            "{}(): Recurrence count must be greater or equal to 1 and lower than {} (including options)",
            vm.current_function_display_name(),
            max_recurrences + 1
        );
        return Err(vm.throw_error(MALFORMED_STRING_EXCEPTION, msg));
    }
    p.recurrences = i64::from(recurrences as i32);
    p.initialized = true;
    Ok(())
}

fn create_from_iso8601_string(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let isostr = args.str(0).clone();
    let options = args.int(1);
    let ce = called_class(vm, args, PERIOD_CLASS);
    let obj = object_init_ex(vm, &ce)?;
    let mut p = period_of(&obj);
    p.current = None;
    let immutable = class(vm, IMMUTABLE_CLASS);
    let r = period_init_iso8601_string(vm, &mut p, &immutable, isostr.as_bytes());
    let recurrences = match r {
        Ok(r) => r,
        Err(e) => {
            set_period_of(&obj, p);
            return Err(e);
        }
    };
    let r = period_init_finish(vm, &mut p, options, recurrences);
    set_period_of(&obj, p);
    r?;
    Ok(Value::Object(obj))
}

/// The argument forms `DatePeriod::__construct()` accepts.
enum CtorArgs {
    Recurrences(ObjRef, ObjRef, i64, i64),
    End(ObjRef, ObjRef, ObjRef, i64),
    Iso(PhpStr, i64),
}

/// The `Passing null to parameter` deprecation of the quiet ZPP passes.
fn null_arg_deprecated(vm: &mut Vm, ty: &str, arg_num: usize) -> PResult<()> {
    let names = ["start", "interval", "end", "options"];
    let msg = format!(
        "{}(): Passing null to parameter #{arg_num} (${}) of type {ty} is deprecated",
        vm.current_function_display_name(),
        names.get(arg_num - 1).copied().unwrap_or("")
    );
    vm.engine_deprecated(msg)
}

/// `Z_PARAM_LONG` in quiet mode.
fn quiet_long(vm: &mut Vm, v: &Value, arg_num: usize) -> PResult<Option<i64>> {
    let v = v.unref();
    match v {
        Value::Int(i) => Ok(Some(i)),
        Value::Null => {
            null_arg_deprecated(vm, "int", arg_num)?;
            Ok(Some(0))
        }
        other => vm.parse_arg_long_weak(&other, false),
    }
}

/// `Z_PARAM_OBJECT_OF_CLASS` in quiet mode.
fn quiet_object(v: &Value, class_name: &str) -> Option<ObjRef> {
    match v.unref() {
        Value::Object(o) if o.class().instance_of_name(class_name.as_bytes()) => Some(o),
        _ => None,
    }
}

/// The three `zend_parse_parameters_ex(ZEND_PARSE_PARAMS_QUIET, ...)` passes
/// of `DatePeriod::__construct()`.
fn parse_ctor_args(vm: &mut Vm, args: &Args) -> PResult<Option<CtorArgs>> {
    let n = args.len();
    let a = |i: usize| args.get(i).clone();
    // "OOl|l"
    if (3..=4).contains(&n)
        && let (Some(start), Some(interval)) = (
            quiet_object(&a(0), DATE_INTERFACE),
            quiet_object(&a(1), INTERVAL_CLASS),
        )
        && !matches!(a(2).unref(), Value::Object(_))
        && let Some(recurrences) = quiet_long(vm, &a(2), 3)?
    {
        let options = if n == 4 {
            quiet_long(vm, &a(3), 4)?
        } else {
            Some(0)
        };
        if let Some(options) = options {
            return Ok(Some(CtorArgs::Recurrences(
                start,
                interval,
                recurrences,
                options,
            )));
        }
    }
    // "OOO|l"
    if (3..=4).contains(&n)
        && let (Some(start), Some(interval), Some(end)) = (
            quiet_object(&a(0), DATE_INTERFACE),
            quiet_object(&a(1), INTERVAL_CLASS),
            quiet_object(&a(2), DATE_INTERFACE),
        )
    {
        let options = if n == 4 {
            quiet_long(vm, &a(3), 4)?
        } else {
            Some(0)
        };
        if let Some(options) = options {
            return Ok(Some(CtorArgs::End(start, interval, end, options)));
        }
    }
    // "s|l"
    if (1..=2).contains(&n) {
        let v = a(0).unref();
        let s = match v {
            Value::String(s) => Some(s),
            Value::Null => {
                null_arg_deprecated(vm, "string", 1)?;
                Some(PhpStr::new())
            }
            Value::Array(_) => None,
            other => vm.parse_arg_str_weak(&other)?,
        };
        if let Some(s) = s {
            let options = if n == 2 {
                quiet_long(vm, &a(1), 2)?
            } else {
                Some(0)
            };
            if let Some(options) = options {
                return Ok(Some(CtorArgs::Iso(s, options)));
            }
        }
    }
    Ok(None)
}

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let Some(parsed) = parse_ctor_args(vm, args)? else {
        return Err(vm.throw_type_error(
            "DatePeriod::__construct() accepts (DateTimeInterface, DateInterval, int [, int]), or (DateTimeInterface, DateInterval, DateTime [, int]), or (string [, int]) as arguments",
        ));
    };
    // date_period_reset
    let mut p = PeriodObj::default();
    let (options, recurrences) = match parsed {
        CtorArgs::Iso(isostr, options) => {
            vm.engine_deprecated(
                "Calling DatePeriod::__construct(string $isostr, int $options = 0) is deprecated, use DatePeriod::createFromISO8601String() instead",
            )?;
            let date_ce = class(vm, super::common::DATE_CLASS);
            match period_init_iso8601_string(vm, &mut p, &date_ce, isostr.as_bytes()) {
                Ok(r) => (options, r),
                Err(e) => {
                    set_period_of(&obj, p);
                    return Err(e);
                }
            }
        }
        CtorArgs::Recurrences(start, interval, recurrences, options) => {
            init_from_objects(vm, &mut p, &start, &interval, None)?;
            (options, recurrences)
        }
        CtorArgs::End(start, interval, end, options) => {
            init_from_objects(vm, &mut p, &start, &interval, Some(&end))?;
            (options, 0)
        }
    };
    let r = period_init_finish(vm, &mut p, options, recurrences);
    set_period_of(&obj, p);
    r?;
    Ok(Value::Null)
}

/// The `(start, interval[, end])` forms of the constructor.
fn init_from_objects(
    vm: &mut Vm,
    p: &mut PeriodObj,
    start: &ObjRef,
    interval: &ObjRef,
    end: Option<&ObjRef>,
) -> PResult<()> {
    let iface = class(vm, DATE_INTERFACE);
    let Some(start_time) = date_time(start) else {
        return Err(uninitialized_error(vm, &iface));
    };
    let end_time = match end {
        Some(e) => match date_time(e) {
            Some(t) => Some(t),
            None => return Err(uninitialized_error(vm, &iface)),
        },
        None => None,
    };
    let intobj = interval_of(interval);
    if !intobj.initialized {
        let ce = interval.class().clone();
        return Err(uninitialized_error(vm, &ce));
    }
    p.start = Some(start_time);
    p.start_ce = Some(start.class().clone());
    p.interval = intobj.diff;
    p.end = end_time;
    Ok(())
}

fn checked_start(vm: &mut Vm, obj: &ObjRef) -> PResult<PeriodObj> {
    let p = period_of(obj);
    if p.start.is_none() {
        let ce = obj.class().clone();
        return Err(uninitialized_error(vm, &ce));
    }
    Ok(p)
}

fn get_start_date(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let p = checked_start(vm, &obj)?;
    period_datetime(vm, &p.start, &p.start_ce)
}

fn get_end_date(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let p = period_of(&obj);
    if p.end.is_none() {
        return Ok(Value::Null);
    }
    period_datetime(vm, &p.end, &p.start_ce)
}

fn get_date_interval(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let p = period_of(&obj);
    if p.interval.is_none() {
        let ce = obj.class().clone();
        return Err(uninitialized_error(vm, &ce));
    }
    period_interval(vm, &p.interval)
}

fn get_recurrences(_vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let p = period_of(&this(args));
    let r = p.recurrences - i64::from(p.include_start_date) - i64::from(p.include_end_date);
    if r == 0 {
        return Ok(Value::Null);
    }
    Ok(Value::Int(r))
}

/// `php_date_period_initialize_from_hash` (no rollback on error).
fn period_initialize_from_hash(obj: &ObjRef, myht: &PhpArray) -> bool {
    let mut p = period_of(obj);
    let ok = (|| {
        let date_entry = |key: &[u8]| -> Result<Option<(Time, ClassRef)>, ()> {
            match myht.get_str(key) {
                None => Err(()),
                Some(Value::Null) => Ok(None),
                Some(Value::Object(o)) if o.class().instance_of_name(DATE_INTERFACE.as_bytes()) => {
                    match date_time(o) {
                        Some(t) => Ok(Some((t, o.class().clone()))),
                        None => Err(()),
                    }
                }
                Some(_) => Err(()),
            }
        };
        if let Some((t, ce)) = date_entry(b"start")? {
            p.start = Some(t);
            p.start_ce = Some(ce);
        }
        if let Some((t, _)) = date_entry(b"end")? {
            if p.start_ce.is_none() {
                return Err(());
            }
            p.end = Some(t);
        }
        if let Some((t, _)) = date_entry(b"current")? {
            if p.start_ce.is_none() {
                return Err(());
            }
            p.current = Some(t);
        }
        match myht.get_str(b"interval") {
            Some(Value::Object(o)) if o.class().lc_name.as_bytes() == b"dateinterval" => {
                let i = interval_of(o);
                if !i.initialized {
                    return Err(());
                }
                p.interval = i.diff;
            }
            _ => return Err(()),
        }
        match myht.get_str(b"recurrences") {
            Some(Value::Int(r)) if (0..=i64::from(i32::MAX)).contains(r) => p.recurrences = *r,
            _ => return Err(()),
        }
        match myht.get_str(b"include_start_date") {
            Some(Value::Bool(b)) => p.include_start_date = *b,
            _ => return Err(()),
        }
        match myht.get_str(b"include_end_date") {
            Some(Value::Bool(b)) => p.include_end_date = *b,
            _ => return Err(()),
        }
        p.initialized = true;
        Ok(())
    })()
    .is_ok();
    set_period_of(obj, p);
    ok
}

fn invalid_serialization(vm: &mut Vm) -> Flow {
    vm.throw_error("Error", "Invalid serialization data for DatePeriod object")
}

fn set_state(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ce = class(vm, PERIOD_CLASS);
    let obj = object_init_ex(vm, &ce)?;
    if !period_initialize_from_hash(&obj, args.array(0)) {
        return Err(invalid_serialization(vm));
    }
    Ok(Value::Object(obj))
}

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let p = checked_start(vm, &obj)?;
    let mut a = PhpArray::new();
    period_object_to_hash(vm, &p, &mut a)?;
    add_common_properties(&mut a, &obj);
    Ok(Value::array(a))
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let data = args.array(0).clone();
    if !period_initialize_from_hash(&obj, &data) {
        return Err(invalid_serialization(vm));
    }
    restore_custom_properties(vm, &obj, &data, is_internal_property)?;
    Ok(Value::Null)
}

fn wakeup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, PERIOD_CLASS, "__wakeup", WAKEUP_DEPRECATION)?;
    let obj = this(args);
    let props = object_property_table(&obj);
    if !period_initialize_from_hash(&obj, &props) {
        return Err(invalid_serialization(vm));
    }
    restore_custom_properties(vm, &obj, &props, is_internal_property)?;
    Ok(Value::Null)
}

// ---------------------------------------------------------------------------
// Iteration (date_period_it_* + InternalIterator)
// ---------------------------------------------------------------------------

/// The state of an `InternalIterator` over a period.
#[derive(Clone, Debug)]
pub struct PeriodIterator {
    period: ObjRef,
    current_index: i64,
    rewind_called: bool,
}

/// The `InternalIterator` class used by `getIterator()`.
#[derive(Default)]
struct IteratorClass(Option<ClassRef>);

/// `date_period_it_rewind`.
fn it_rewind(vm: &mut Vm, it: &mut PeriodIterator) -> PResult<()> {
    it.current_index = 0;
    let mut p = period_of(&it.period);
    p.current = None;
    let Some(start) = p.start.clone() else {
        set_period_of(&it.period, p);
        let ce = class(vm, PERIOD_CLASS);
        return Err(uninitialized_error(vm, &ce));
    };
    let mut current = start;
    if !p.include_start_date
        && let Some(interval) = &p.interval
    {
        period_advance(&mut current, interval);
    }
    p.current = Some(current);
    set_period_of(&it.period, p);
    Ok(())
}

/// `date_period_it_has_more`.
fn it_valid(it: &PeriodIterator) -> bool {
    let p = period_of(&it.period);
    let Some(current) = &p.current else {
        return false;
    };
    match &p.end {
        Some(end) => {
            if current.sse == end.sse {
                if p.include_end_date {
                    current.us <= end.us
                } else {
                    current.us < end.us
                }
            } else {
                current.sse < end.sse
            }
        }
        None => it.current_index < p.recurrences,
    }
}

/// `date_period_it_current_data`.
fn it_current(vm: &mut Vm, it: &PeriodIterator) -> PResult<Value> {
    let p = period_of(&it.period);
    let Some(current) = p.current else {
        return Ok(Value::Null);
    };
    let ce = match &p.start_ce {
        Some(ce) => base_date_class(ce),
        None => class(vm, super::common::DATE_CLASS),
    };
    Ok(Value::Object(new_date_object(vm, &ce, current)?))
}

/// `date_period_it_move_forward`.
fn it_move_forward(it: &mut PeriodIterator) {
    let mut p = period_of(&it.period);
    if let (Some(current), Some(interval)) = (p.current.as_mut(), p.interval) {
        period_advance(current, &interval);
    }
    set_period_of(&it.period, p);
    it.current_index += 1;
}

fn iterator_state(vm: &mut Vm, args: &Args) -> PResult<(ObjRef, PeriodIterator)> {
    let obj = this(args);
    let state = obj.native::<PeriodIterator>().map(|s| s.clone());
    match state {
        Some(s) => Ok((obj, s)),
        None => Err(vm.throw_error(
            "Error",
            "The InternalIterator object has not been properly initialized",
        )),
    }
}

/// `zend_internal_iterator_ensure_rewound`.
fn ensure_rewound(vm: &mut Vm, it: &mut PeriodIterator) -> PResult<()> {
    if !it.rewind_called {
        it.rewind_called = true;
        it_rewind(vm, it)?;
    }
    Ok(())
}

fn iter_current(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, mut it) = iterator_state(vm, args)?;
    let r = ensure_rewound(vm, &mut it);
    obj.set_native(it.clone());
    r?;
    it_current(vm, &it)
}

fn iter_key(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, mut it) = iterator_state(vm, args)?;
    let r = ensure_rewound(vm, &mut it);
    obj.set_native(it.clone());
    r?;
    Ok(Value::Int(it.current_index))
}

fn iter_next(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, mut it) = iterator_state(vm, args)?;
    let r = ensure_rewound(vm, &mut it);
    if r.is_ok() {
        it_move_forward(&mut it);
    }
    obj.set_native(it);
    r?;
    Ok(Value::Null)
}

fn iter_valid(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, mut it) = iterator_state(vm, args)?;
    let r = ensure_rewound(vm, &mut it);
    obj.set_native(it.clone());
    r?;
    Ok(Value::Bool(it_valid(&it)))
}

fn iter_rewind(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, mut it) = iterator_state(vm, args)?;
    it.rewind_called = true;
    let r = it_rewind(vm, &mut it);
    obj.set_native(it);
    r?;
    Ok(Value::Null)
}

fn iter_construct(vm: &mut Vm, _args: &mut Args) -> PResult<Value> {
    Err(vm.throw_error(
        "Error",
        "Call to private InternalIterator::__construct() from global scope",
    ))
}

/// `DatePeriod::getIterator()` (`zend_create_internal_iterator_zval`).
fn get_iterator(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let period = this(args);
    let ce = match &vm.ext_data::<IteratorClass>().0 {
        Some(ce) => ce.clone(),
        None => class(vm, ITERATOR_CLASS),
    };
    let it = ObjRef::new(ce);
    it.set_native(PeriodIterator {
        period,
        current_index: 0,
        rewind_called: false,
    });
    Ok(Value::Object(it))
}

nf!(
    DP_CREATE_FROM_ISO8601_STRING,
    "public static function createFromISO8601String(string $specification, int $options = 0): static {}",
    create_from_iso8601_string
);
nf!(
    DP_CONSTRUCT,
    "public function __construct($start, $interval = UNKNOWN, $end = UNKNOWN, $options = UNKNOWN) {}",
    construct
);
nf!(
    DP_GET_START_DATE,
    "public function getStartDate(): DateTimeInterface {}",
    get_start_date
);
nf!(
    DP_GET_END_DATE,
    "public function getEndDate(): ?DateTimeInterface {}",
    get_end_date
);
nf!(
    DP_GET_DATE_INTERVAL,
    "public function getDateInterval(): DateInterval {}",
    get_date_interval
);
nf!(
    DP_GET_RECURRENCES,
    "public function getRecurrences(): ?int {}",
    get_recurrences
);
nf!(
    DP_SERIALIZE,
    "public function __serialize(): array;",
    serialize
);
nf!(
    DP_UNSERIALIZE,
    "public function __unserialize(array $data): void;",
    unserialize
);
nf!(DP_WAKEUP, "public function __wakeup(): void {}", wakeup);
nf!(
    DP_SET_STATE,
    "public static function __set_state(array $array): static {}",
    set_state
);
nf!(
    DP_GET_ITERATOR,
    "public function getIterator(): Iterator {}",
    get_iterator
);

nf!(
    II_CONSTRUCT,
    "private function __construct() {}",
    iter_construct
);
nf!(
    II_CURRENT,
    "public function current(): mixed {}",
    iter_current
);
nf!(II_KEY, "public function key(): mixed {}", iter_key);
nf!(II_NEXT, "public function next(): void {}", iter_next);
nf!(II_VALID, "public function valid(): bool {}", iter_valid);
nf!(II_REWIND, "public function rewind(): void {}", iter_rewind);

fn virtual_property(name: &str, ty: &str) -> PropertyInfo {
    let mut info = PropertyInfo::new(
        name,
        PERIOD_CLASS,
        member_flags::PUBLIC | member_flags::READONLY | member_flags::VIRTUAL,
    );
    info.ty = TypeDecl::parse(ty).ok();
    info.default = Value::Undef;
    info
}

pub fn register(r: &mut Registry) {
    let aggregate = r
        .find_class("IteratorAggregate")
        .expect("IteratorAggregate is a core interface");
    let mut ce = ClassEntry::new(PERIOD_CLASS, 0);
    ce.interfaces.push(aggregate.clone());
    for parent in aggregate.interfaces.iter() {
        ce.interfaces.push(parent.clone());
    }
    let int_ty = TypeDecl::parse("int").ok();
    for (name, value) in [
        ("EXCLUDE_START_DATE", EXCLUDE_START_DATE),
        ("INCLUDE_END_DATE", INCLUDE_END_DATE),
    ] {
        let mut c = ClassConstant::new(name, Value::Int(value), PERIOD_CLASS);
        c.ty = int_ty.clone();
        ce.add_constant(c);
    }
    for (name, ty) in [
        ("start", "?DateTimeInterface"),
        ("current", "?DateTimeInterface"),
        ("end", "?DateTimeInterface"),
        ("interval", "?DateInterval"),
        ("recurrences", "int"),
        ("include_start_date", "bool"),
        ("include_end_date", "bool"),
    ] {
        ce.declare_property(virtual_property(name, ty));
    }
    add_methods(
        &mut ce,
        PERIOD_CLASS,
        &[
            (&DP_CREATE_FROM_ISO8601_STRING, STATIC),
            (&DP_CONSTRUCT, PUB | fn_flags::CTOR),
            (&DP_GET_START_DATE, PUB),
            (&DP_GET_END_DATE, PUB),
            (&DP_GET_DATE_INTERVAL, PUB),
            (&DP_GET_RECURRENCES, PUB),
            (&DP_SERIALIZE, PUB),
            (&DP_UNSERIALIZE, PUB),
            (&DP_WAKEUP, PUB),
            (&DP_SET_STATE, STATIC),
            (&DP_GET_ITERATOR, PUB),
        ],
    );
    ce.handlers.create_object = Some(create_period_object);
    ce.handlers.clone_obj = Some(clone_period_object);
    ce.handlers.get_properties_for = Some(period_properties_for);
    ce.handlers.read_property = Some(period_read_property);
    ce.handlers.write_property = Some(period_write_property);
    r.class(ce);

    // InternalIterator (Zend/zend_interfaces.c), unless the engine provides it.
    let iterator = r
        .find_class("Iterator")
        .expect("Iterator is a core interface");
    let mut it = ClassEntry::new(
        ITERATOR_CLASS,
        class_flags::FINAL | class_flags::NOT_SERIALIZABLE,
    );
    it.interfaces.push(iterator.clone());
    for parent in iterator.interfaces.iter() {
        it.interfaces.push(parent.clone());
    }
    add_methods(
        &mut it,
        ITERATOR_CLASS,
        &[
            (&II_CONSTRUCT, member_flags::PRIVATE | fn_flags::CTOR),
            (&II_CURRENT, PUB),
            (&II_KEY, PUB),
            (&II_NEXT, PUB),
            (&II_VALID, PUB),
            (&II_REWIND, PUB),
        ],
    );
    let ce = if r.find_class(ITERATOR_CLASS).is_some() {
        it.flags |= class_flags::INTERNAL;
        it.link_magic_methods();
        std::rc::Rc::new(it)
    } else {
        r.class(it)
    };
    r.vm.ext_data::<IteratorClass>().0 = Some(ce);
}
