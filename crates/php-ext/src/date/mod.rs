//! ext/date (`php_date.c`): the date/time functions, the `DATE_*` and
//! `SUNFUNCS_*` constants, the `date.*` ini directives and the classes
//! `DateTimeInterface`, `DateTime`, `DateTimeImmutable`, `DateTimeZone`,
//! `DateInterval`, `DatePeriod` and the `Date*Error` / `Date*Exception`
//! hierarchy, built on [`php_timelib`].
//!
//! * [`functions`] — `strtotime`, `date`, `mktime`, `strftime`, `date_parse`,
//!   timezone listings, `date_sun_info`, ...
//! * [`datetime`] — `DateTimeInterface`, `DateTime`, `DateTimeImmutable` and
//!   the procedural `date_*` aliases.
//! * [`timezone`], [`interval`], [`period`] — the other classes.
//! * [`common`] — module globals (`DATEG`), default timezone, helpers.

pub mod common;
pub mod datetime;
pub mod functions;
pub mod interval;
pub mod period;
pub mod strftime;
pub mod timezone;

#[cfg(test)]
mod tests;

use php_runtime::Registry;
use php_runtime::class::ClassEntry;
use php_runtime::ini::{IniDef, modifiable};
use php_timelib::timezone_id_is_valid;

use common::{
    DATE_ERROR, DATE_EXCEPTION, DATE_OBJECT_ERROR, DATE_RANGE_ERROR, INVALID_OPERATION_EXCEPTION,
    INVALID_TIMEZONE_EXCEPTION, MALFORMED_INTERVAL_STRING_EXCEPTION,
    MALFORMED_PERIOD_STRING_EXCEPTION, MALFORMED_STRING_EXCEPTION,
};

/// `OnUpdate_date_timezone`: only valid identifiers are accepted.
fn validate_timezone(v: &[u8]) -> bool {
    std::str::from_utf8(v).is_ok_and(timezone_id_is_valid)
}

const ALL: u32 = modifiable::ALL;

/// The `date.*` ini directives.
pub const INI: &[IniDef] = &[
    IniDef::new("date.timezone", Some("UTC"), ALL).validated(validate_timezone),
    IniDef::new("date.default_latitude", Some("31.7667"), ALL),
    IniDef::new("date.default_longitude", Some("35.2333"), ALL),
    IniDef::new("date.sunset_zenith", Some("90.833333"), ALL),
    IniDef::new("date.sunrise_zenith", Some("90.833333"), ALL),
];

/// The `SUNFUNCS_*` deprecation suffix.
const SUNFUNCS_DEPRECATION: &str =
    "since 8.4, as date_sunrise() and date_sunset() were deprecated in 8.1";

fn register_constants(r: &mut Registry) {
    for (name, value) in datetime::FORMATS {
        let name = format!("DATE_{name}");
        if name == "DATE_RFC7231" {
            r.deprecated_constant(&name, *value, datetime::RFC7231_DEPRECATION);
        } else {
            r.constant(&name, *value);
        }
    }
    r.deprecated_constant(
        "SUNFUNCS_RET_TIMESTAMP",
        functions::SUNFUNCS_RET_TIMESTAMP,
        SUNFUNCS_DEPRECATION,
    );
    r.deprecated_constant(
        "SUNFUNCS_RET_STRING",
        functions::SUNFUNCS_RET_STRING,
        SUNFUNCS_DEPRECATION,
    );
    r.deprecated_constant(
        "SUNFUNCS_RET_DOUBLE",
        functions::SUNFUNCS_RET_DOUBLE,
        SUNFUNCS_DEPRECATION,
    );
}

/// The `Date*Error` / `Date*Exception` classes.
fn register_exceptions(r: &mut Registry) {
    let error = r.find_class("Error").expect("Error is a core class");
    let date_error = r.class(ClassEntry::new_subclass(DATE_ERROR, 0, &error));
    r.class(ClassEntry::new_subclass(DATE_OBJECT_ERROR, 0, &date_error));
    r.class(ClassEntry::new_subclass(DATE_RANGE_ERROR, 0, &date_error));
    let exception = r
        .find_class("Exception")
        .expect("Exception is a core class");
    let date_exception = r.class(ClassEntry::new_subclass(DATE_EXCEPTION, 0, &exception));
    for name in [
        INVALID_TIMEZONE_EXCEPTION,
        INVALID_OPERATION_EXCEPTION,
        MALFORMED_STRING_EXCEPTION,
        MALFORMED_INTERVAL_STRING_EXCEPTION,
        MALFORMED_PERIOD_STRING_EXCEPTION,
    ] {
        r.class(ClassEntry::new_subclass(name, 0, &date_exception));
    }
}

pub fn register(r: &mut Registry) {
    r.ini_entries(INI);

    // Classes in date_register_classes() order.
    datetime::register(r);
    timezone::register(r);
    interval::register(r);
    period::register(r);
    register_exceptions(r);

    register_constants(r);

    // Functions in php_date.stub.php order.
    use datetime as d;
    use functions as f;
    r.functions(&[
        &f::STRTOTIME,
        &f::DATE,
        &f::IDATE,
        &f::GMDATE,
        &f::MKTIME,
        &f::GMMKTIME,
        &f::CHECKDATE,
        &f::STRFTIME,
        &f::GMSTRFTIME,
        &f::TIME,
        &f::LOCALTIME,
        &f::GETDATE,
        &d::DATE_CREATE,
        &d::DATE_CREATE_IMMUTABLE,
        &d::DATE_CREATE_FROM_FORMAT,
        &d::DATE_CREATE_IMMUTABLE_FROM_FORMAT,
        &f::DATE_PARSE,
        &f::DATE_PARSE_FROM_FORMAT,
        &f::DATE_GET_LAST_ERRORS,
        &d::DATE_FORMAT,
        &d::DATE_MODIFY,
        &d::DATE_ADD,
        &d::DATE_SUB,
        &d::DATE_TIMEZONE_GET,
        &d::DATE_TIMEZONE_SET,
        &d::DATE_OFFSET_GET,
        &d::DATE_DIFF,
        &d::DATE_TIME_SET,
        &d::DATE_DATE_SET,
        &d::DATE_ISODATE_SET,
        &d::DATE_TIMESTAMP_SET,
        &d::DATE_TIMESTAMP_GET,
        &timezone::TIMEZONE_OPEN,
        &timezone::TIMEZONE_NAME_GET,
        &f::TIMEZONE_NAME_FROM_ABBR,
        &timezone::TIMEZONE_OFFSET_GET,
        &timezone::TIMEZONE_TRANSITIONS_GET,
        &timezone::TIMEZONE_LOCATION_GET,
        &f::TIMEZONE_IDENTIFIERS_LIST,
        &f::TIMEZONE_ABBREVIATIONS_LIST,
        &f::TIMEZONE_VERSION_GET,
        &interval::DATE_INTERVAL_CREATE_FROM_DATE_STRING,
        &interval::DATE_INTERVAL_FORMAT,
        &f::DATE_DEFAULT_TIMEZONE_SET,
        &f::DATE_DEFAULT_TIMEZONE_GET,
        &f::DATE_SUNRISE,
        &f::DATE_SUNSET,
        &f::DATE_SUN_INFO,
    ]);
}
