//! `DateTimeZone` and the `timezone_*` functions operating on it.

use php_runtime::class::{ClassConstant, ClassEntry, PropPurpose};
use php_runtime::types::TypeDecl;
use php_runtime::value::convert::object_property_table;
use php_runtime::{
    Args, ClassRef, NativeFunction, ObjRef, PResult, PhpArray, PhpStr, Registry, Value, Vm,
};
use php_timelib::php::{
    PhpTimeZone, timezone_open as timelib_timezone_open, timezone_transitions_get,
};

use super::common::{
    DATE_EXCEPTION, DATE_OBJECT_ERROR, INVALID_TIMEZONE_EXCEPTION, TIMEZONE_CLASS,
    WAKEUP_DEPRECATION, add_common_properties, check_path, class, deprecated_method,
    has_debug_info, hash_update, object_props, restore_custom_properties, this,
    uninitialized_error,
};
use super::datetime::{PUB, STATIC, add_methods, checked_time, nf, same_compare_handler, target};
use super::functions::{
    TIMEZONE_GROUP_AFRICA, TIMEZONE_GROUP_ALL, TIMEZONE_GROUP_ALL_W_BC, TIMEZONE_GROUP_AMERICA,
    TIMEZONE_GROUP_ANTARCTICA, TIMEZONE_GROUP_ARCTIC, TIMEZONE_GROUP_ASIA, TIMEZONE_GROUP_ATLANTIC,
    TIMEZONE_GROUP_AUSTRALIA, TIMEZONE_GROUP_EUROPE, TIMEZONE_GROUP_INDIAN, TIMEZONE_GROUP_PACIFIC,
    TIMEZONE_GROUP_UTC, TIMEZONE_PER_COUNTRY, timezone_abbreviations_list,
    timezone_identifiers_list,
};

/// The state of a `DateTimeZone` (`php_timezone_obj`); `None` until
/// initialised.
#[derive(Clone, Debug, Default)]
pub struct TimezoneObj {
    pub tz: Option<PhpTimeZone>,
}

/// `date_object_new_timezone`.
fn create_timezone_object(_vm: &mut Vm, ce: &ClassRef) -> PResult<ObjRef> {
    let obj = ObjRef::new(ce.clone());
    obj.set_native(TimezoneObj::default());
    Ok(obj)
}

/// The zone of a `DateTimeZone` object (`None` if uninitialised).
pub fn timezone_of(obj: &ObjRef) -> Option<PhpTimeZone> {
    obj.native::<TimezoneObj>().and_then(|t| t.tz.clone())
}

fn set_timezone_of(obj: &ObjRef, tz: Option<PhpTimeZone>) {
    let done = match obj.native_mut::<TimezoneObj>() {
        Some(mut t) => {
            t.tz = tz.clone();
            true
        }
        None => false,
    };
    if !done {
        obj.set_native(TimezoneObj { tz });
    }
}

/// A new `DateTimeZone` holding `zone` (`set_timezone_from_timelib_time`).
pub fn new_timezone_object(vm: &mut Vm, zone: PhpTimeZone) -> PResult<ObjRef> {
    let ce = class(vm, TIMEZONE_CLASS);
    let obj = vm.instantiate(&ce)?;
    set_timezone_of(&obj, Some(zone));
    Ok(obj)
}

/// `DATE_CHECK_INITIALIZED(tzobj->initialized, Z_OBJCE_P(object))`.
fn checked_zone(vm: &mut Vm, obj: &ObjRef) -> PResult<PhpTimeZone> {
    match timezone_of(obj) {
        Some(z) => Ok(z),
        None => {
            let ce = obj.class().clone();
            Err(uninitialized_error(vm, &ce))
        }
    }
}

/// `date_object_clone_timezone`.
fn clone_timezone_object(vm: &mut Vm, old: &ObjRef) -> PResult<ObjRef> {
    let ce = old.class().clone();
    let new = create_timezone_object(vm, &ce)?;
    *new.obj().props.borrow_mut() = old.obj().props.borrow().clone();
    *new.obj().dyn_props.borrow_mut() = old.obj().dyn_props.borrow().clone();
    set_timezone_of(&new, timezone_of(old));
    Ok(new)
}

/// `date_object_compare_timezone`.
fn compare_timezones(vm: &mut Vm, a: &Value, b: &Value) -> PResult<i32> {
    let Some((o1, o2)) = same_compare_handler(a, b) else {
        return vm.std_compare_objects(a, b);
    };
    let (Some(z1), Some(z2)) = (timezone_of(&o1), timezone_of(&o2)) else {
        return Err(vm.throw_error(
            DATE_OBJECT_ERROR,
            "Trying to compare uninitialized DateTimeZone objects",
        ));
    };
    Ok(match (&z1, &z2) {
        (PhpTimeZone::Offset(x), PhpTimeZone::Offset(y)) => i32::from(x != y),
        (PhpTimeZone::Abbr(x), PhpTimeZone::Abbr(y)) => i32::from(x.abbr != y.abbr),
        (PhpTimeZone::Id(x), PhpTimeZone::Id(y)) => i32::from(x.name != y.name),
        _ => {
            return Err(vm.throw_error(
                DATE_EXCEPTION,
                "Cannot compare two different kinds of DateTimeZone objects",
            ));
        }
    })
}

/// `php_timezone_to_string`.
pub fn timezone_to_string(zone: &PhpTimeZone) -> Value {
    Value::from(zone.name())
}

/// `date_timezone_object_to_hash`.
fn timezone_object_to_hash(zone: &PhpTimeZone, props: &mut PhpArray) {
    hash_update(props, "timezone_type", i64::from(zone.zone_type().as_i32()));
    hash_update(props, "timezone", timezone_to_string(zone));
}

/// `date_object_get_properties_for_timezone`.
fn timezone_properties_for(
    _vm: &mut Vm,
    obj: &ObjRef,
    purpose: PropPurpose,
) -> PResult<Option<PhpArray>> {
    match purpose {
        PropPurpose::Serialize
        | PropPurpose::VarExport
        | PropPurpose::Json
        | PropPurpose::ArrayCast => {}
        PropPurpose::Debug => {
            if has_debug_info(obj.class()) {
                return Ok(None);
            }
        }
        PropPurpose::GetObjectVars => return Ok(None),
    }
    let mut props = object_property_table(obj);
    if let Some(z) = timezone_of(obj) {
        timezone_object_to_hash(&z, &mut props);
    }
    Ok(Some(props))
}

/// `timezone_initialize`: the zone for `tz`, or the warning/exception text.
pub fn timezone_initialize(tz: &[u8]) -> Result<PhpTimeZone, Vec<u8>> {
    match timelib_timezone_open(&String::from_utf8_lossy(tz)) {
        Ok(z) => Ok(z),
        Err(e) => {
            let mut msg = if e.starts_with("Timezone offset is out of range") {
                b"Timezone offset is out of range (".to_vec()
            } else {
                b"Unknown or bad timezone (".to_vec()
            };
            msg.extend_from_slice(tz);
            msg.push(b')');
            Err(msg)
        }
    }
}

fn timezone_open(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let tz = args.str(0).clone();
    check_path(vm, 1, tz.as_bytes())?;
    match timezone_initialize(tz.as_bytes()) {
        Ok(zone) => Ok(Value::Object(new_timezone_object(vm, zone)?)),
        Err(msg) => {
            vm.warning(PhpStr::from_vec(msg))?;
            Ok(Value::Bool(false))
        }
    }
}

fn construct(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let tz = args.str(0).clone();
    check_path(vm, 1, tz.as_bytes())?;
    match timezone_initialize(tz.as_bytes()) {
        Ok(zone) => {
            set_timezone_of(&obj, Some(zone));
            Ok(Value::Null)
        }
        Err(msg) => {
            let mut full = b"DateTimeZone::__construct(): ".to_vec();
            full.extend_from_slice(&msg);
            Err(vm.throw_error(INVALID_TIMEZONE_EXCEPTION, PhpStr::from_vec(full)))
        }
    }
}

/// `timezone_name_get()` / `getName()`.
fn timezone_name_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, _) = target(args);
    let zone = checked_zone(vm, &obj)?;
    Ok(timezone_to_string(&zone))
}

/// `timezone_offset_get()` / `getOffset()`.
fn timezone_offset_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let dateobj = args.object(off).clone();
    let zone = checked_zone(vm, &obj)?;
    let t = checked_time(vm, &dateobj)?;
    Ok(Value::Int(match zone {
        PhpTimeZone::Id(tz) => i64::from(tz.get_time_zone_info(t.sse).offset),
        PhpTimeZone::Offset(o) => o,
        PhpTimeZone::Abbr(a) => a.utc_offset + i64::from(a.dst) * 3600,
    }))
}

/// `timezone_transitions_get()` / `getTransitions()`.
fn transitions_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, off) = target(args);
    let (begin, end) = (args.int(off), args.int(off + 1));
    let zone = checked_zone(vm, &obj)?;
    let PhpTimeZone::Id(tz) = zone else {
        return Ok(Value::Bool(false));
    };
    let mut out = PhpArray::new();
    for tr in timezone_transitions_get(&tz, begin, end) {
        let mut e = PhpArray::with_capacity(5);
        e.insert_str("ts", tr.ts);
        e.insert_str("time", tr.time());
        e.insert_str("offset", i64::from(tr.offset));
        e.insert_str("isdst", tr.isdst);
        e.insert_str("abbr", tr.abbr.as_str());
        let _ = out.append(Value::array(e));
    }
    Ok(Value::array(out))
}

/// `timezone_location_get()` / `getLocation()`.
fn location_get(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let (obj, _) = target(args);
    let zone = checked_zone(vm, &obj)?;
    let PhpTimeZone::Id(tz) = zone else {
        return Ok(Value::Bool(false));
    };
    let mut a = PhpArray::with_capacity(4);
    a.insert_str("country_code", tz.location.country_code.as_str());
    a.insert_str("latitude", tz.location.latitude);
    a.insert_str("longitude", tz.location.longitude);
    a.insert_str("comments", tz.location.comments.as_str());
    Ok(Value::array(a))
}

/// `php_date_timezone_initialize_from_hash`.
fn timezone_initialize_from_hash(obj: &ObjRef, myht: &PhpArray) -> bool {
    let Some(tz_type) = myht.get_str(b"timezone_type") else {
        return false;
    };
    let Some(tz) = myht.get_str(b"timezone") else {
        return false;
    };
    let Value::Int(tz_type) = tz_type else {
        return false;
    };
    if !(1..=3).contains(tz_type) {
        return false;
    }
    let Value::String(tz) = tz else {
        return false;
    };
    if tz.contains(&0) {
        return false;
    }
    match timezone_initialize(tz.as_bytes()) {
        Ok(zone) => {
            set_timezone_of(obj, Some(zone));
            true
        }
        Err(_) => false,
    }
}

fn invalid_serialization(vm: &mut Vm) -> php_runtime::Flow {
    vm.throw_error(
        "Error",
        "Invalid serialization data for DateTimeZone object",
    )
}

fn set_state(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let ce = class(vm, TIMEZONE_CLASS);
    let obj = vm.instantiate(&ce)?;
    if !timezone_initialize_from_hash(&obj, args.array(0)) {
        return Err(invalid_serialization(vm));
    }
    Ok(Value::Object(obj))
}

fn wakeup(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    deprecated_method(vm, TIMEZONE_CLASS, "__wakeup", WAKEUP_DEPRECATION)?;
    let obj = this(args);
    let props = object_props(&obj);
    if !timezone_initialize_from_hash(&obj, &props) {
        return Err(invalid_serialization(vm));
    }
    Ok(Value::Null)
}

fn serialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let zone = checked_zone(vm, &obj)?;
    let mut a = PhpArray::new();
    timezone_object_to_hash(&zone, &mut a);
    add_common_properties(&mut a, &obj);
    Ok(Value::array(a))
}

/// `date_timezone_is_internal_property`.
fn is_internal_property(name: &[u8]) -> bool {
    matches!(name, b"timezone_type" | b"timezone")
}

fn unserialize(vm: &mut Vm, args: &mut Args) -> PResult<Value> {
    let obj = this(args);
    let data = args.array(0).clone();
    if !timezone_initialize_from_hash(&obj, &data) {
        return Err(invalid_serialization(vm));
    }
    restore_custom_properties(vm, &obj, &data, is_internal_property)?;
    Ok(Value::Null)
}

nf!(
    TIMEZONE_OPEN,
    "function timezone_open(string $timezone): DateTimeZone|false {}",
    timezone_open
);
nf!(
    TIMEZONE_NAME_GET,
    "function timezone_name_get(DateTimeZone $object): string {}",
    timezone_name_get
);
nf!(
    TIMEZONE_OFFSET_GET,
    "function timezone_offset_get(DateTimeZone $object, DateTimeInterface $datetime): int {}",
    timezone_offset_get
);
nf!(
    TIMEZONE_TRANSITIONS_GET,
    "function timezone_transitions_get(
    DateTimeZone $object, int $timestampBegin = PHP_INT_MIN, int $timestampEnd = 2147483647): array|false {}",
    transitions_get
);
nf!(
    TIMEZONE_LOCATION_GET,
    "function timezone_location_get(DateTimeZone $object): array|false {}",
    location_get
);

nf!(
    TZ_CONSTRUCT,
    "public function __construct(string $timezone) {}",
    construct
);
nf!(
    TZ_GET_NAME,
    "public function getName(): string {}",
    timezone_name_get
);
nf!(
    TZ_GET_OFFSET,
    "public function getOffset(DateTimeInterface $datetime): int {}",
    timezone_offset_get
);
nf!(
    TZ_GET_TRANSITIONS,
    "public function getTransitions(int $timestampBegin = PHP_INT_MIN, int $timestampEnd = 2147483647): array|false {}",
    transitions_get
);
nf!(
    TZ_GET_LOCATION,
    "public function getLocation(): array|false {}",
    location_get
);
nf!(
    TZ_LIST_ABBREVIATIONS,
    "public static function listAbbreviations(): array {}",
    timezone_abbreviations_list
);
nf!(
    TZ_LIST_IDENTIFIERS,
    "public static function listIdentifiers(int $timezoneGroup = DateTimeZone::ALL, ?string $countryCode = null): array {}",
    timezone_identifiers_list
);
nf!(
    TZ_SERIALIZE,
    "public function __serialize(): array {}",
    serialize
);
nf!(
    TZ_UNSERIALIZE,
    "public function __unserialize(array $data): void {}",
    unserialize
);
nf!(TZ_WAKEUP, "public function __wakeup(): void {}", wakeup);
nf!(
    TZ_SET_STATE,
    "public static function __set_state(array $array): static {}",
    set_state
);

pub fn register(r: &mut Registry) {
    let mut ce = ClassEntry::new(TIMEZONE_CLASS, 0);
    let int_ty = TypeDecl::parse("int").ok();
    for (name, value) in [
        ("AFRICA", TIMEZONE_GROUP_AFRICA),
        ("AMERICA", TIMEZONE_GROUP_AMERICA),
        ("ANTARCTICA", TIMEZONE_GROUP_ANTARCTICA),
        ("ARCTIC", TIMEZONE_GROUP_ARCTIC),
        ("ASIA", TIMEZONE_GROUP_ASIA),
        ("ATLANTIC", TIMEZONE_GROUP_ATLANTIC),
        ("AUSTRALIA", TIMEZONE_GROUP_AUSTRALIA),
        ("EUROPE", TIMEZONE_GROUP_EUROPE),
        ("INDIAN", TIMEZONE_GROUP_INDIAN),
        ("PACIFIC", TIMEZONE_GROUP_PACIFIC),
        ("UTC", TIMEZONE_GROUP_UTC),
        ("ALL", TIMEZONE_GROUP_ALL),
        ("ALL_WITH_BC", TIMEZONE_GROUP_ALL_W_BC),
        ("PER_COUNTRY", TIMEZONE_PER_COUNTRY),
    ] {
        let mut c = ClassConstant::new(name, Value::Int(value), TIMEZONE_CLASS);
        c.ty = int_ty.clone();
        ce.add_constant(c);
    }
    add_methods(
        &mut ce,
        TIMEZONE_CLASS,
        &[
            (&TZ_CONSTRUCT, PUB | php_runtime::class::fn_flags::CTOR),
            (&TZ_GET_NAME, PUB),
            (&TZ_GET_OFFSET, PUB),
            (&TZ_GET_TRANSITIONS, PUB),
            (&TZ_GET_LOCATION, PUB),
            (&TZ_LIST_ABBREVIATIONS, STATIC),
            (&TZ_LIST_IDENTIFIERS, STATIC),
            (&TZ_SERIALIZE, PUB),
            (&TZ_UNSERIALIZE, PUB),
            (&TZ_WAKEUP, PUB),
            (&TZ_SET_STATE, STATIC),
        ],
    );
    ce.handlers.create_object = Some(create_timezone_object);
    ce.handlers.clone_obj = Some(clone_timezone_object);
    ce.handlers.compare = Some(compare_timezones);
    ce.handlers.get_properties_for = Some(timezone_properties_for);
    r.class(ce);
}
